use std::time::Duration;

use thiserror::Error;
use tokio::sync::mpsc;

use crate::process::{
    PiProcessDiagnostics, PiProcessIdentity, SpawnedPiProcess, TerminationOutcome,
};
use crate::rpc::{ExtensionUiResponse, InboundMessage, RpcRequest};
use crate::{RunId, RuntimeLimits};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProcessTerminationReport {
    Exited {
        code: Option<i32>,
        diagnostics: PiProcessDiagnostics,
        kill_requested: bool,
    },
    Unconfirmed {
        identity: PiProcessIdentity,
        diagnostics: PiProcessDiagnostics,
    },
}

impl From<TerminationOutcome> for ProcessTerminationReport {
    fn from(value: TerminationOutcome) -> Self {
        match value {
            TerminationOutcome::Exited {
                status,
                diagnostics,
                kill_requested,
            } => Self::Exited {
                code: status.code(),
                diagnostics,
                kill_requested,
            },
            TerminationOutcome::Unconfirmed {
                identity,
                diagnostics,
            } => Self::Unconfirmed {
                identity,
                diagnostics,
            },
        }
    }
}

#[derive(Debug)]
pub enum RunProcessEvent {
    Inbound(InboundMessage),
    RequestWriteFailed {
        request_id: crate::RequestId,
        detail: String,
    },
    ExtensionUiResponseWritten {
        request_id: String,
    },
    ExtensionUiResponseWriteFailed {
        request_id: String,
        detail: String,
    },
    Exited {
        code: Option<i32>,
        diagnostics: PiProcessDiagnostics,
    },
    ProtocolFailure {
        detail: String,
        termination: ProcessTerminationReport,
    },
    TerminationFinished {
        result: Result<ProcessTerminationReport, String>,
    },
}

#[derive(Debug)]
pub struct RunProcessEnvelope {
    pub run_id: RunId,
    pub event: RunProcessEvent,
}

#[derive(Clone, Debug)]
pub struct RunProcessHandle {
    run_id: RunId,
    commands: mpsc::Sender<RunProcessCommand>,
    control: mpsc::Sender<RunProcessControlCommand>,
}

impl RunProcessHandle {
    #[must_use]
    pub const fn run_id(&self) -> RunId {
        self.run_id
    }

    pub fn send_request(&self, request: RpcRequest) -> Result<(), RunProcessCommandError> {
        self.commands
            .try_send(RunProcessCommand::SendRequest { request })
            .map_err(map_try_send_error)
    }

    pub fn send_extension_ui_response(
        &self,
        response_value: ExtensionUiResponse,
    ) -> Result<(), RunProcessCommandError> {
        self.commands
            .try_send(RunProcessCommand::SendExtensionUiResponse {
                response: response_value,
            })
            .map_err(map_try_send_error)
    }

    pub fn terminate(&self, deadline: Duration) -> Result<(), RunProcessCommandError> {
        self.control
            .try_send(RunProcessControlCommand::Terminate { deadline })
            .map_err(map_control_try_send_error)
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RunProcessCommandError {
    #[error("run process actor is closed")]
    ActorClosed,
    #[error("run process actor command queue is full")]
    CommandQueueFull,
}

enum RunProcessCommand {
    SendRequest { request: RpcRequest },
    SendExtensionUiResponse { response: ExtensionUiResponse },
}

enum RunProcessControlCommand {
    Terminate { deadline: Duration },
}

enum RunProcessWriterEvent {
    RequestWriteFailed {
        request_id: crate::RequestId,
        detail: String,
    },
    ExtensionUiResponseWritten {
        request_id: String,
    },
    ExtensionUiResponseWriteFailed {
        request_id: String,
        detail: String,
    },
    CommandChannelClosed,
}

fn map_try_send_error(
    error: mpsc::error::TrySendError<RunProcessCommand>,
) -> RunProcessCommandError {
    match error {
        mpsc::error::TrySendError::Full(_) => RunProcessCommandError::CommandQueueFull,
        mpsc::error::TrySendError::Closed(_) => RunProcessCommandError::ActorClosed,
    }
}

fn map_control_try_send_error(
    error: mpsc::error::TrySendError<RunProcessControlCommand>,
) -> RunProcessCommandError {
    match error {
        mpsc::error::TrySendError::Full(_) => RunProcessCommandError::CommandQueueFull,
        mpsc::error::TrySendError::Closed(_) => RunProcessCommandError::ActorClosed,
    }
}

pub fn spawn_run_process_actor(
    run_id: RunId,
    process: SpawnedPiProcess,
    events: mpsc::Sender<RunProcessEnvelope>,
    limits: RuntimeLimits,
) -> RunProcessHandle {
    let (commands, command_rx) = mpsc::channel(limits.max_runtime_command_queue);
    let (control, control_rx) = mpsc::channel(1);
    tokio::spawn(run_process_actor(
        run_id, process, command_rx, control_rx, events, limits,
    ));
    RunProcessHandle {
        run_id,
        commands,
        control,
    }
}

async fn run_process_actor(
    run_id: RunId,
    process: SpawnedPiProcess,
    commands: mpsc::Receiver<RunProcessCommand>,
    mut control_commands: mpsc::Receiver<RunProcessControlCommand>,
    events: mpsc::Sender<RunProcessEnvelope>,
    limits: RuntimeLimits,
) {
    let SpawnedPiProcess {
        mut control,
        mut reader,
        writer,
    } = process;
    let (writer_events_tx, mut writer_events_rx) = mpsc::channel(limits.max_runtime_command_queue);
    let writer_task = tokio::spawn(run_process_writer(writer, commands, writer_events_tx));
    let mut control_open = true;

    loop {
        tokio::select! {
            biased;
            command = control_commands.recv(), if control_open => {
                match command {
                    Some(RunProcessControlCommand::Terminate { deadline }) => {
                        let result = control
                            .terminate(deadline)
                            .await
                            .map(ProcessTerminationReport::from)
                            .map_err(|error| error.to_string());
                        let _ = events
                            .send(RunProcessEnvelope {
                                run_id,
                                event: RunProcessEvent::TerminationFinished { result },
                            })
                            .await;
                        writer_task.abort();
                        return;
                    }
                    None => control_open = false,
                }
            }
            writer_event = writer_events_rx.recv() => {
                let Some(writer_event) = writer_event else {
                    let termination = terminate_for_failure(&mut control, limits).await;
                    let _ = events
                        .send(RunProcessEnvelope {
                            run_id,
                            event: RunProcessEvent::ProtocolFailure {
                                detail: "Pi stdin writer task ended unexpectedly".to_owned(),
                                termination,
                            },
                        })
                        .await;
                    writer_task.abort();
                    return;
                };
                let event = match writer_event {
                    RunProcessWriterEvent::RequestWriteFailed { request_id, detail } => {
                        RunProcessEvent::RequestWriteFailed { request_id, detail }
                    }
                    RunProcessWriterEvent::ExtensionUiResponseWritten { request_id } => {
                        RunProcessEvent::ExtensionUiResponseWritten { request_id }
                    }
                    RunProcessWriterEvent::ExtensionUiResponseWriteFailed { request_id, detail } => {
                        RunProcessEvent::ExtensionUiResponseWriteFailed { request_id, detail }
                    }
                    RunProcessWriterEvent::CommandChannelClosed => {
                        let termination = terminate_for_failure(&mut control, limits).await;
                        let _ = events
                            .send(RunProcessEnvelope {
                                run_id,
                                event: RunProcessEvent::ProtocolFailure {
                                    detail: "Pi stdin command channel closed while child was live".to_owned(),
                                    termination,
                                },
                            })
                            .await;
                        writer_task.abort();
                        return;
                    }
                };
                if send_actor_event(&events, run_id, event, &mut control, limits)
                    .await
                    .is_err()
                {
                    writer_task.abort();
                    return;
                }
            }
            message = reader.next_message() => {
                match message {
                    Some(Ok(message)) => {
                        if send_actor_event(
                            &events,
                            run_id,
                            RunProcessEvent::Inbound(message),
                            &mut control,
                            limits,
                        )
                        .await
                        .is_err()
                        {
                            writer_task.abort();
                            return;
                        }
                    }
                    Some(Err(error)) => {
                        let termination = terminate_for_failure(&mut control, limits).await;
                        let _ = events
                            .send(RunProcessEnvelope {
                                run_id,
                                event: RunProcessEvent::ProtocolFailure {
                                    detail: error.to_string(),
                                    termination,
                                },
                            })
                            .await;
                        writer_task.abort();
                        return;
                    }
                    None => {
                        let status = match control.wait().await {
                            Ok(status) => status,
                            Err(error) => {
                                let termination = ProcessTerminationReport::Unconfirmed {
                                    identity: control.identity(),
                                    diagnostics: control.diagnostics().await,
                                };
                                let _ = events
                                    .send(RunProcessEnvelope {
                                        run_id,
                                        event: RunProcessEvent::ProtocolFailure {
                                            detail: format!("Pi stdout closed and process wait failed: {error}"),
                                            termination,
                                        },
                                    })
                                    .await;
                                writer_task.abort();
                                return;
                            }
                        };
                        let diagnostics = match control
                            .finish_diagnostics_bounded(Duration::from_millis(
                                limits.stop_termination_deadline_ms,
                            ))
                            .await
                        {
                            Ok(diagnostics) => diagnostics,
                            Err(error) => {
                                let _ = events
                                    .send(RunProcessEnvelope {
                                        run_id,
                                        event: RunProcessEvent::ProtocolFailure {
                                            detail: format!("Pi process exited but stderr drain failed: {error}"),
                                            termination: ProcessTerminationReport::Exited {
                                                code: status.code(),
                                                diagnostics: control.diagnostics().await,
                                                kill_requested: false,
                                            },
                                        },
                                    })
                                    .await;
                                writer_task.abort();
                                return;
                            }
                        };
                        let _ = events
                            .send(RunProcessEnvelope {
                                run_id,
                                event: RunProcessEvent::Exited {
                                    code: status.code(),
                                    diagnostics,
                                },
                            })
                            .await;
                        writer_task.abort();
                        return;
                    }
                }
            }
        }
    }
}

async fn run_process_writer(
    mut writer: crate::rpc::RpcWriter<tokio::process::ChildStdin>,
    mut commands: mpsc::Receiver<RunProcessCommand>,
    writer_events: mpsc::Sender<RunProcessWriterEvent>,
) {
    while let Some(command) = commands.recv().await {
        match command {
            RunProcessCommand::SendRequest { request } => {
                if let Err(error) = writer.send_request(&request).await {
                    let _ = writer_events
                        .send(RunProcessWriterEvent::RequestWriteFailed {
                            request_id: request.id,
                            detail: error.to_string(),
                        })
                        .await;
                    return;
                }
            }
            RunProcessCommand::SendExtensionUiResponse { response } => {
                let request_id = match &response {
                    ExtensionUiResponse::Value { id, .. }
                    | ExtensionUiResponse::Confirmation { id, .. }
                    | ExtensionUiResponse::Cancelled { id } => id.clone(),
                };
                let event = match writer.send_extension_ui_response(&response).await {
                    Ok(()) => RunProcessWriterEvent::ExtensionUiResponseWritten { request_id },
                    Err(error) => RunProcessWriterEvent::ExtensionUiResponseWriteFailed {
                        request_id,
                        detail: error.to_string(),
                    },
                };
                let failed = matches!(
                    event,
                    RunProcessWriterEvent::ExtensionUiResponseWriteFailed { .. }
                );
                if writer_events.send(event).await.is_err() || failed {
                    return;
                }
            }
        }
    }
    let _ = writer_events
        .send(RunProcessWriterEvent::CommandChannelClosed)
        .await;
}

async fn terminate_for_failure(
    control: &mut crate::process::PiProcessControl,
    limits: RuntimeLimits,
) -> ProcessTerminationReport {
    match control
        .terminate(Duration::from_millis(limits.stop_termination_deadline_ms))
        .await
    {
        Ok(outcome) => ProcessTerminationReport::from(outcome),
        Err(_) => ProcessTerminationReport::Unconfirmed {
            identity: control.identity(),
            diagnostics: control.diagnostics().await,
        },
    }
}

async fn send_actor_event(
    events: &mpsc::Sender<RunProcessEnvelope>,
    run_id: RunId,
    event: RunProcessEvent,
    control: &mut crate::process::PiProcessControl,
    limits: RuntimeLimits,
) -> Result<(), ()> {
    match events.try_send(RunProcessEnvelope { run_id, event }) {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Closed(_)) => {
            let _ = control
                .terminate(Duration::from_millis(limits.stop_termination_deadline_ms))
                .await;
            Err(())
        }
        Err(mpsc::error::TrySendError::Full(_)) => {
            let termination = match control
                .terminate(Duration::from_millis(limits.stop_termination_deadline_ms))
                .await
            {
                Ok(outcome) => ProcessTerminationReport::from(outcome),
                Err(_) => ProcessTerminationReport::Unconfirmed {
                    identity: control.identity(),
                    diagnostics: control.diagnostics().await,
                },
            };
            let _ = events
                .send(RunProcessEnvelope {
                    run_id,
                    event: RunProcessEvent::ProtocolFailure {
                        detail: "bounded internal Pi event queue exhausted".to_owned(),
                        termination,
                    },
                })
                .await;
            Err(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::environment::{LaunchEnvironmentInput, resolve_launch_environment};
    use crate::launch::{PiLaunchSpec, ProjectTrustPolicy};
    use crate::process::spawn_pi_process;
    use crate::rpc::{RpcCommand, RpcResponseOutcome};

    struct Fixture {
        root: PathBuf,
        fake_pi: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("pi-wizard-actor-{}", RunId::new()));
            fs::create_dir_all(&root).expect("fixture root");
            #[cfg(windows)]
            let fake_pi = {
                let cli = root
                    .join("node_modules")
                    .join("@earendil-works")
                    .join("pi-coding-agent")
                    .join("dist")
                    .join("bundle")
                    .join("cli.js");
                fs::create_dir_all(cli.parent().expect("fake Pi CLI parent"))
                    .expect("create fake Pi npm layout");
                fs::write(
                    &cli,
                    concat!(
                        "let buffer = '';\n",
                        "process.stdin.setEncoding('utf8');\n",
                        "process.stdin.on('data', chunk => {\n",
                        "  buffer += chunk;\n",
                        "  while (buffer.includes('\\n')) {\n",
                        "    const index = buffer.indexOf('\\n');\n",
                        "    const line = buffer.slice(0, index).replace(/\\r$/, '');\n",
                        "    buffer = buffer.slice(index + 1);\n",
                        "    if (!line) continue;\n",
                        "    const request = JSON.parse(line);\n",
                        "    if (request.type === 'get_state') process.stdout.write(JSON.stringify({id:request.id,type:'response',command:'get_state',success:true,data:{model:null,thinkingLevel:'medium',isStreaming:false,isCompacting:false,steeringMode:'all',followUpMode:'one-at-a-time',sessionId:'fake',autoCompactionEnabled:true,messageCount:0,pendingMessageCount:0}}) + '\\n');\n",
                        "  }\n",
                        "});\n"
                    ),
                )
                .expect("write direct fake Pi CLI");
                let path = root.join("pi.cmd");
                fs::write(
                    &path,
                    "@echo off\r\nnode \"%~dp0node_modules\\@earendil-works\\pi-coding-agent\\dist\\bundle\\cli.js\" %*\r\n",
                )
                .expect("write wrapped Pi shim");
                path
            };
            #[cfg(not(windows))]
            let fake_pi = root.join("pi");
            #[cfg(not(windows))]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::write(
                    &fake_pi,
                    concat!(
                        "#!/bin/sh\n",
                        "while IFS= read -r request; do\n",
                        "case \"$request\" in\n",
                        "*get_state*) printf '%s\\n' '{\"id\":\"state-1\",\"type\":\"response\",\"command\":\"get_state\",\"success\":true,\"data\":{\"model\":null,\"thinkingLevel\":\"medium\",\"isStreaming\":false,\"isCompacting\":false,\"steeringMode\":\"all\",\"followUpMode\":\"one-at-a-time\",\"sessionId\":\"fake\",\"autoCompactionEnabled\":true,\"messageCount\":0,\"pendingMessageCount\":0}}' ;;\n",
                        "esac\n",
                        "done\n"
                    ),
                )
                .expect("fake Pi");
                let mut permissions = fs::metadata(&fake_pi).expect("metadata").permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&fake_pi, permissions).expect("permissions");
            }

            Self { root, fake_pi }
        }

        fn environment(&self) -> crate::environment::ResolvedLaunchEnvironment {
            let desktop_environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
            resolve_launch_environment(LaunchEnvironmentInput {
                configured_pi: Some(self.fake_pi.clone()),
                desktop_environment,
                ..LaunchEnvironmentInput::default()
            })
            .expect("environment")
        }

        fn launch(&self) -> crate::launch::ResolvedPiLaunchSpec {
            PiLaunchSpec::new(
                self.fake_pi.clone(),
                self.root.clone(),
                ProjectTrustPolicy::Ignore,
            )
            .resolve()
            .expect("launch")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn actor_keeps_process_alive_across_requests_until_exact_termination() {
        let fixture = Fixture::new();
        let limits = RuntimeLimits::default();
        let process = spawn_pi_process(&fixture.launch(), &fixture.environment(), limits)
            .expect("spawn fake Pi");
        let run_id = RunId::new();
        let (event_tx, mut event_rx) = mpsc::channel(limits.max_process_event_queue);
        let actor = spawn_run_process_actor(run_id, process, event_tx, limits);

        actor
            .send_request(RpcRequest::with_id(
                crate::RequestId::from_wire("state-1"),
                RpcCommand::GetState,
            ))
            .expect("write state request");
        let envelope = event_rx.recv().await.expect("response event");
        let RunProcessEvent::Inbound(InboundMessage::Response(response)) = envelope.event else {
            panic!("expected response");
        };
        assert_eq!(response.outcome(), RpcResponseOutcome::Accepted);

        actor
            .terminate(Duration::from_secs(1))
            .expect("queue exact termination");
        let envelope = event_rx.recv().await.expect("termination event");
        assert!(matches!(
            envelope.event,
            RunProcessEvent::TerminationFinished {
                result: Ok(ProcessTerminationReport::Exited {
                    kill_requested: true,
                    ..
                })
            }
        ));
    }
}
