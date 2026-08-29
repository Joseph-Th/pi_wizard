use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::{Instant, timeout};

use crate::RuntimeLimits;
use crate::bounded::ByteRing;
use crate::environment::ResolvedLaunchEnvironment;
use crate::launch::ResolvedPiLaunchSpec;
use crate::rpc::{RpcReader, RpcWriter};

const STDERR_READ_CHUNK_BYTES: usize = 8 * 1024;
const DEFAULT_DIAGNOSTIC_EOF_DEADLINE: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PiProcessIdentity {
    pub pid: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PiProcessDiagnostics {
    pub stderr: Vec<u8>,
    pub dropped_stderr_bytes: u64,
    /// True only when the stderr drain observed EOF. A false value means the
    /// bounded finalization deadline expired, usually because a descendant
    /// retained an inherited pipe handle.
    pub stderr_complete: bool,
}

pub struct SpawnedPiProcess {
    pub control: PiProcessControl,
    pub reader: RpcReader<ChildStdout>,
    pub writer: RpcWriter<ChildStdin>,
}

impl std::fmt::Debug for SpawnedPiProcess {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SpawnedPiProcess")
            .field("identity", &self.control.identity)
            .finish_non_exhaustive()
    }
}

/// Exact owned child handle plus bounded stderr drain.
///
/// Lifecycle actions operate on this handle, never by executable name or a PID
/// discovered later. The spawn-time PID scopes exact process-tree termination
/// on platforms where a script wrapper may own the actual Pi/Node descendant.
pub struct PiProcessControl {
    child: Child,
    identity: PiProcessIdentity,
    stderr: Arc<Mutex<ByteRing>>,
    stderr_task: Option<JoinHandle<io::Result<()>>>,
}

impl std::fmt::Debug for PiProcessControl {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PiProcessControl")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl PiProcessControl {
    #[must_use]
    pub const fn identity(&self) -> PiProcessIdentity {
        self.identity
    }

    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, io::Error> {
        self.child.try_wait()
    }

    pub async fn wait(&mut self) -> Result<ExitStatus, io::Error> {
        self.child.wait().await
    }

    pub async fn diagnostics(&self) -> PiProcessDiagnostics {
        let stderr = self.stderr.lock().await;
        PiProcessDiagnostics {
            stderr: stderr.to_vec(),
            dropped_stderr_bytes: stderr.dropped_bytes(),
            stderr_complete: false,
        }
    }

    /// Waits for stderr EOF through a small fixed deadline. Callers that own a
    /// lifecycle deadline should use [`Self::finish_diagnostics_bounded`] so
    /// pipe finalization cannot extend that transaction indefinitely.
    pub async fn finish_diagnostics(&mut self) -> Result<PiProcessDiagnostics, ProcessError> {
        self.finish_diagnostics_bounded(DEFAULT_DIAGNOSTIC_EOF_DEADLINE)
            .await
    }

    pub async fn finish_diagnostics_bounded(
        &mut self,
        deadline: Duration,
    ) -> Result<PiProcessDiagnostics, ProcessError> {
        let mut stderr_complete = self.stderr_task.is_none();
        if let Some(mut task) = self.stderr_task.take() {
            match timeout(deadline, &mut task).await {
                Ok(joined) => {
                    joined.map_err(ProcessError::StderrTaskJoin)??;
                    stderr_complete = true;
                }
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                }
            }
        }
        let mut diagnostics = self.diagnostics().await;
        diagnostics.stderr_complete = stderr_complete;
        Ok(diagnostics)
    }

    /// Exact-handle hard termination used only after the higher-level Stop
    /// transaction has exhausted RPC cancellation or when shutdown cannot use
    /// RPC safely.
    pub async fn terminate(
        &mut self,
        deadline: Duration,
    ) -> Result<TerminationOutcome, ProcessError> {
        let expires_at = Instant::now() + deadline;
        if let Some(status) = self.child.try_wait()? {
            let diagnostics = self
                .finish_diagnostics_bounded(remaining(expires_at))
                .await?;
            return Ok(TerminationOutcome::Exited {
                status,
                diagnostics,
                kill_requested: false,
            });
        }

        let tree_kill_confirmed =
            match request_process_tree_termination(self.identity, remaining(expires_at)).await {
                Ok(()) => true,
                Err(_) => {
                    // The exact child handle remains useful as a best-effort
                    // fallback, but a failed tree-kill request means descendants
                    // cannot be claimed terminated.
                    self.child.start_kill()?;
                    false
                }
            };
        match timeout(remaining(expires_at), self.child.wait()).await {
            Ok(Ok(status)) => {
                let diagnostics = self
                    .finish_diagnostics_bounded(remaining(expires_at))
                    .await?;
                if tree_kill_confirmed {
                    Ok(TerminationOutcome::Exited {
                        status,
                        diagnostics,
                        kill_requested: true,
                    })
                } else {
                    Ok(TerminationOutcome::Unconfirmed {
                        identity: self.identity,
                        diagnostics,
                    })
                }
            }
            Ok(Err(source)) => Err(ProcessError::Wait(source)),
            Err(_) => Ok(TerminationOutcome::Unconfirmed {
                identity: self.identity,
                diagnostics: self.diagnostics().await,
            }),
        }
    }
}

#[derive(Debug)]
pub enum TerminationOutcome {
    Exited {
        status: ExitStatus,
        diagnostics: PiProcessDiagnostics,
        kill_requested: bool,
    },
    Unconfirmed {
        identity: PiProcessIdentity,
        diagnostics: PiProcessDiagnostics,
    },
}

pub fn spawn_pi_process(
    spec: &ResolvedPiLaunchSpec,
    environment: &ResolvedLaunchEnvironment,
    limits: RuntimeLimits,
) -> Result<SpawnedPiProcess, ProcessError> {
    let launch_executable = canonicalize_launch_executable(&spec.executable)?;
    if launch_executable != environment.pi_executable() {
        return Err(ProcessError::ExecutableEnvironmentMismatch {
            launch: launch_executable,
            environment: environment.pi_executable().to_path_buf(),
        });
    }

    let invocation = environment.pi_invocation();
    let mut command = Command::new(invocation.executable());
    command
        .args(invocation.prefix_args())
        .args(spec.args())
        .current_dir(spec.cwd())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env_clear()
        .envs(environment.variables());

    #[cfg(unix)]
    {
        // The exact Pi child becomes leader of an app-owned process group so
        // wrappers and ordinary descendants can be terminated as one unit.
        // This is lifecycle isolation, not a security sandbox.
        command.process_group(0);
    }

    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command.spawn().map_err(ProcessError::Spawn)?;
    let pid = child.id().ok_or(ProcessError::MissingProcessId)?;
    let stdin = child
        .stdin
        .take()
        .ok_or(ProcessError::MissingPipe("stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or(ProcessError::MissingPipe("stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or(ProcessError::MissingPipe("stderr"))?;

    let stderr_ring = Arc::new(Mutex::new(ByteRing::new(limits.max_stderr_bytes_per_run)));
    let stderr_task = tokio::spawn(drain_stderr(stderr, Arc::clone(&stderr_ring)));

    Ok(SpawnedPiProcess {
        control: PiProcessControl {
            child,
            identity: PiProcessIdentity { pid },
            stderr: stderr_ring,
            stderr_task: Some(stderr_task),
        },
        reader: RpcReader::new(stdout, limits.max_rpc_frame_bytes),
        writer: RpcWriter::new(stdin, limits),
    })
}

fn remaining(expires_at: Instant) -> Duration {
    expires_at.saturating_duration_since(Instant::now())
}

#[cfg(any(windows, test))]
fn windows_tree_termination_args(identity: PiProcessIdentity) -> [String; 4] {
    [
        "/PID".to_owned(),
        identity.pid.to_string(),
        "/T".to_owned(),
        "/F".to_owned(),
    ]
}

#[cfg(any(unix, test))]
fn unix_tree_termination_args(identity: PiProcessIdentity) -> [String; 2] {
    ["-KILL".to_owned(), format!("-{}", identity.pid)]
}

#[cfg(windows)]
async fn request_process_tree_termination(
    identity: PiProcessIdentity,
    deadline: Duration,
) -> Result<(), ProcessError> {
    let taskkill = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .map(|root| root.join("System32").join("taskkill.exe"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("taskkill.exe"));
    let args = windows_tree_termination_args(identity);
    let mut child = Command::new(taskkill)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(ProcessError::TreeTerminateSpawn)?;
    let status = match timeout(deadline, child.wait()).await {
        Ok(result) => result.map_err(ProcessError::TreeTerminateWait)?,
        Err(_) => {
            let _ = child.start_kill();
            return Err(ProcessError::TreeTerminateDeadline);
        }
    };
    if status.success() {
        Ok(())
    } else {
        Err(ProcessError::TreeTerminateRejected {
            code: status.code(),
        })
    }
}

#[cfg(unix)]
async fn request_process_tree_termination(
    identity: PiProcessIdentity,
    deadline: Duration,
) -> Result<(), ProcessError> {
    let args = unix_tree_termination_args(identity);
    let mut child = Command::new("/bin/kill")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(ProcessError::TreeTerminateSpawn)?;
    let status = match timeout(deadline, child.wait()).await {
        Ok(result) => result.map_err(ProcessError::TreeTerminateWait)?,
        Err(_) => {
            let _ = child.start_kill();
            return Err(ProcessError::TreeTerminateDeadline);
        }
    };
    if status.success() {
        Ok(())
    } else {
        Err(ProcessError::TreeTerminateRejected {
            code: status.code(),
        })
    }
}

async fn drain_stderr(
    mut stderr: ChildStderr,
    ring: Arc<Mutex<ByteRing>>,
) -> Result<(), io::Error> {
    let mut chunk = [0_u8; STDERR_READ_CHUNK_BYTES];
    loop {
        let read = stderr.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        ring.lock().await.push(&chunk[..read]);
    }
}

fn canonicalize_launch_executable(path: &Path) -> Result<PathBuf, ProcessError> {
    path.canonicalize()
        .map_err(|source| ProcessError::CanonicalizeExecutable {
            path: path.to_path_buf(),
            source,
        })
}

#[derive(Debug, Error)]
pub enum ProcessError {
    #[error("failed to canonicalize launch executable {path}: {source}")]
    CanonicalizeExecutable { path: PathBuf, source: io::Error },
    #[error("launch executable {launch} differs from resolved environment Pi {environment}")]
    ExecutableEnvironmentMismatch {
        launch: PathBuf,
        environment: PathBuf,
    },
    #[error("failed to spawn Pi child: {0}")]
    Spawn(io::Error),
    #[error("spawned Pi child did not expose a process id")]
    MissingProcessId,
    #[error("spawned Pi child is missing required {0} pipe")]
    MissingPipe(&'static str),
    #[error("failed waiting for Pi child: {0}")]
    Wait(io::Error),
    #[error("failed controlling Pi child: {0}")]
    Io(#[from] io::Error),
    #[error("failed spawning exact process-tree termination helper: {0}")]
    TreeTerminateSpawn(io::Error),
    #[error("failed waiting for exact process-tree termination helper: {0}")]
    TreeTerminateWait(io::Error),
    #[error("exact process-tree termination exceeded its lifecycle deadline")]
    TreeTerminateDeadline,
    #[error("exact process-tree termination helper rejected the request with code {code:?}")]
    TreeTerminateRejected { code: Option<i32> },
    #[error("stderr drain task failed: {0}")]
    StderrTaskJoin(tokio::task::JoinError),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;

    use super::*;
    use crate::environment::{LaunchEnvironmentInput, resolve_launch_environment};
    use crate::launch::{PiLaunchSpec, ProjectTrustPolicy};
    use crate::rpc::{InboundMessage, RpcCommand, RpcRequest};
    use crate::{RequestId, RunId};

    struct Fixture {
        root: PathBuf,
        fake_pi: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("pi-wizard-process-{name}-{}", RunId::new()));
            fs::create_dir_all(&root).expect("create fixture");
            #[cfg(windows)]
            let fake_pi = root.join("pi.cmd");
            #[cfg(not(windows))]
            let fake_pi = root.join("pi");

            #[cfg(windows)]
            fs::write(
                &fake_pi,
                concat!(
                    "@echo off\r\n",
                    "set /p request=\r\n",
                    "1>&2 echo 0123456789abcdefghijklmnopqrstuvwxyz\r\n",
                    "echo {\"id\":\"req-1\",\"type\":\"response\",\"command\":\"get_state\",\"success\":true,\"data\":{\"model\":null,\"thinkingLevel\":\"medium\",\"isStreaming\":false,\"isCompacting\":false,\"steeringMode\":\"all\",\"followUpMode\":\"one-at-a-time\",\"sessionId\":\"fake-session\",\"autoCompactionEnabled\":true,\"messageCount\":0,\"pendingMessageCount\":0}}\r\n"
                ),
            )
            .expect("write fake Pi");
            #[cfg(not(windows))]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::write(
                    &fake_pi,
                    concat!(
                        "#!/bin/sh\n",
                        "IFS= read -r request\n",
                        "printf '%s\\n' '0123456789abcdefghijklmnopqrstuvwxyz' >&2\n",
                        "printf '%s\\n' '{\"id\":\"req-1\",\"type\":\"response\",\"command\":\"get_state\",\"success\":true,\"data\":{\"model\":null,\"thinkingLevel\":\"medium\",\"isStreaming\":false,\"isCompacting\":false,\"steeringMode\":\"all\",\"followUpMode\":\"one-at-a-time\",\"sessionId\":\"fake-session\",\"autoCompactionEnabled\":true,\"messageCount\":0,\"pendingMessageCount\":0}}'\n"
                    ),
                )
                .expect("write fake Pi");
                let mut permissions = fs::metadata(&fake_pi).expect("metadata").permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&fake_pi, permissions).expect("permissions");
            }

            Self { root, fake_pi }
        }

        fn environment(&self) -> ResolvedLaunchEnvironment {
            let desktop_environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
            resolve_launch_environment(LaunchEnvironmentInput {
                configured_pi: Some(self.fake_pi.clone()),
                desktop_environment,
                ..LaunchEnvironmentInput::default()
            })
            .expect("resolve fake Pi environment")
        }

        fn launch(&self) -> ResolvedPiLaunchSpec {
            PiLaunchSpec::new(
                self.fake_pi.clone(),
                self.root.clone(),
                ProjectTrustPolicy::Ignore,
            )
            .resolve()
            .expect("resolve launch")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn platform_tree_termination_specs_are_exact_and_never_name_based() {
        let identity = PiProcessIdentity { pid: 4242 };
        assert_eq!(
            windows_tree_termination_args(identity),
            ["/PID", "4242", "/T", "/F"].map(str::to_owned)
        );
        assert_eq!(
            unix_tree_termination_args(identity),
            ["-KILL", "-4242"].map(str::to_owned)
        );
    }

    #[tokio::test]
    async fn supervised_fake_pi_round_trip_and_stderr_are_bounded() {
        let fixture = Fixture::new("round-trip");
        let limits = RuntimeLimits {
            max_stderr_bytes_per_run: 12,
            ..RuntimeLimits::default()
        };
        let mut process = spawn_pi_process(&fixture.launch(), &fixture.environment(), limits)
            .expect("spawn fake");
        assert!(process.control.identity().pid > 0);
        process
            .writer
            .send_request(&RpcRequest::with_id(
                RequestId::from_wire("req-1"),
                RpcCommand::GetState,
            ))
            .await
            .expect("send state request");

        let message = process
            .reader
            .next_message()
            .await
            .expect("response frame")
            .expect("valid response");
        let InboundMessage::Response(response) = message else {
            panic!("expected response");
        };
        assert_eq!(response.id.as_deref(), Some("req-1"));
        assert_eq!(response.command, "get_state");

        let status = process.control.wait().await.expect("wait fake");
        assert!(status.success());
        let diagnostics = process
            .control
            .finish_diagnostics()
            .await
            .expect("finish diagnostics");
        assert!(diagnostics.stderr.len() <= 12);
        assert!(diagnostics.dropped_stderr_bytes > 0);
        assert!(diagnostics.stderr_complete);
    }

    #[tokio::test]
    async fn launch_executable_must_match_environment_discovery_identity() {
        let fixture = Fixture::new("mismatch");
        let other = fixture.root.join("other-pi");
        fs::copy(&fixture.fake_pi, &other).expect("copy fake");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = fs::metadata(&other).expect("metadata").permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&other, permissions).expect("permissions");
        }
        let mut launch = fixture.launch();
        launch.executable = other;

        assert!(matches!(
            spawn_pi_process(&launch, &fixture.environment(), RuntimeLimits::default()),
            Err(ProcessError::ExecutableEnvironmentMismatch { .. })
        ));
    }
}
