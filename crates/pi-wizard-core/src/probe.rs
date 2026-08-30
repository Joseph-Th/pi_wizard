use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::path::Path;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Debug)]
pub(crate) struct ProbeOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stdout_exceeded: bool,
}

pub(crate) async fn run_bounded_command(
    executable: &Path,
    args: &[OsString],
    cwd: Option<&Path>,
    environment: &BTreeMap<OsString, OsString>,
    max_bytes_per_stream: usize,
    deadline: Duration,
) -> Result<ProbeOutput, ProbeCommandError> {
    let mut command = Command::new(executable);
    command.args(args).env_clear().envs(environment);
    run_bounded_prepared_command(command, cwd, max_bytes_per_stream, deadline).await
}

pub(crate) async fn run_bounded_prepared_command(
    mut command: Command,
    cwd: Option<&Path>,
    max_bytes_per_stream: usize,
    deadline: Duration,
) -> Result<ProbeOutput, ProbeCommandError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }

    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = command.spawn().map_err(ProbeCommandError::Spawn)?;
    let stdout = child
        .stdout
        .take()
        .ok_or(ProbeCommandError::MissingPipe("stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or(ProbeCommandError::MissingPipe("stderr"))?;
    let stdout_task = tokio::spawn(capture_and_drain(stdout, max_bytes_per_stream));
    let stderr_task = tokio::spawn(capture_and_drain(stderr, max_bytes_per_stream));

    let status = match timeout(deadline, child.wait()).await {
        Ok(result) => result.map_err(ProbeCommandError::Wait)?,
        Err(_) => {
            let _ = child.start_kill();
            let _ = timeout(Duration::from_secs(1), child.wait()).await;
            stdout_task.abort();
            stderr_task.abort();
            return Err(ProbeCommandError::TimedOut);
        }
    };
    let stdout = stdout_task.await.map_err(ProbeCommandError::ReaderTask)??;
    // Stderr is deliberately drained but never surfaced because environment
    // probes can execute user shell startup code that prints secrets.
    let _stderr = stderr_task.await.map_err(ProbeCommandError::ReaderTask)??;

    Ok(ProbeOutput {
        status,
        stdout: stdout.bytes,
        stdout_exceeded: stdout.exceeded,
    })
}

#[derive(Debug)]
struct BoundedCapture {
    bytes: Vec<u8>,
    exceeded: bool,
}

async fn capture_and_drain<R>(mut reader: R, max_bytes: usize) -> Result<BoundedCapture, io::Error>
where
    R: AsyncRead + Unpin,
{
    let mut capture = BoundedCapture {
        bytes: Vec::with_capacity(max_bytes.min(16 * 1024)),
        exceeded: false,
    };
    let mut chunk = [0_u8; 8 * 1024];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Ok(capture);
        }
        let available = max_bytes.saturating_sub(capture.bytes.len());
        let keep = available.min(read);
        capture.bytes.extend_from_slice(&chunk[..keep]);
        if keep < read {
            capture.exceeded = true;
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum ProbeCommandError {
    #[error("failed to spawn bounded probe command")]
    Spawn(#[source] io::Error),
    #[error("bounded probe command is missing required {0} pipe")]
    MissingPipe(&'static str),
    #[error("failed waiting for bounded probe command")]
    Wait(#[source] io::Error),
    #[error("bounded probe command exceeded its deadline")]
    TimedOut,
    #[error("bounded probe stream reader task failed")]
    ReaderTask(#[source] tokio::task::JoinError),
    #[error("failed reading bounded probe output")]
    Read(#[from] io::Error),
}
