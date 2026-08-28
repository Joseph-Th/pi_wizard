use std::collections::HashMap;

use thiserror::Error;

use super::{RpcConcurrencyClass, RpcRequest};
use crate::{RequestId, RuntimeLimits};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActiveRpcCommand {
    pub id: RequestId,
    pub command: &'static str,
    pub class: RpcConcurrencyClass,
}

/// Client-side ordering barrier for Pi RPC operations whose upstream runtime
/// ownership is not safe to replace concurrently.
///
/// Extension UI responses intentionally do not pass through this gate. A
/// `session_before_switch` hook may be waiting for one while a replacement is
/// active, so the extension-response subprotocol is a separate control plane.
#[derive(Debug)]
pub struct RpcCommandGate {
    max_active: usize,
    active: HashMap<String, ActiveRpcCommand>,
    replacement_id: Option<String>,
    compaction_id: Option<String>,
}

impl RpcCommandGate {
    #[must_use]
    pub fn new(max_active: usize) -> Self {
        assert!(max_active > 0, "RPC command gate limit must be non-zero");
        Self {
            max_active,
            active: HashMap::new(),
            replacement_id: None,
            compaction_id: None,
        }
    }

    #[must_use]
    pub fn from_limits(limits: RuntimeLimits) -> Self {
        Self::new(limits.max_pending_rpc_requests_per_run)
    }

    pub fn begin(&mut self, request: &RpcRequest) -> Result<(), RpcGateError> {
        let id = request.id.as_str();
        if self.active.contains_key(id) {
            return Err(RpcGateError::DuplicateId { id: id.to_owned() });
        }
        if self.active.len() >= self.max_active {
            return Err(RpcGateError::Limit {
                limit: self.max_active,
            });
        }

        let class = request.command.concurrency_class();
        if let Some(replacement_id) = &self.replacement_id {
            return Err(RpcGateError::SessionReplacementInFlight {
                request_id: replacement_id.clone(),
            });
        }

        if let Some(compaction_id) = &self.compaction_id
            && request.command.blocked_by_manual_compaction()
        {
            return Err(RpcGateError::ManualCompactionBlocksCommand {
                request_id: compaction_id.clone(),
                command: request.command.wire_type(),
            });
        }

        match class {
            RpcConcurrencyClass::SessionReplacement => {
                if !self.active.is_empty() {
                    return Err(RpcGateError::SessionReplacementRequiresQuiescence {
                        active: self.active.len(),
                    });
                }
                self.replacement_id = Some(id.to_owned());
            }
            RpcConcurrencyClass::ManualCompaction => {
                if let Some(compaction_id) = &self.compaction_id {
                    return Err(RpcGateError::ManualCompactionInFlight {
                        request_id: compaction_id.clone(),
                    });
                }
                self.compaction_id = Some(id.to_owned());
            }
            RpcConcurrencyClass::Ordinary => {}
        }

        self.active.insert(
            id.to_owned(),
            ActiveRpcCommand {
                id: request.id.clone(),
                command: request.command.wire_type(),
                class,
            },
        );
        Ok(())
    }

    pub fn finish(&mut self, id: &RequestId) -> Option<ActiveRpcCommand> {
        let finished = self.active.remove(id.as_str())?;
        if self.replacement_id.as_deref() == Some(id.as_str()) {
            self.replacement_id = None;
        }
        if self.compaction_id.as_deref() == Some(id.as_str()) {
            self.compaction_id = None;
        }
        Some(finished)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.active.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum RpcGateError {
    #[error("RPC command gate limit {limit} reached")]
    Limit { limit: usize },
    #[error("RPC request id {id} is already active in command gate")]
    DuplicateId { id: String },
    #[error("session replacement {request_id} is already in flight")]
    SessionReplacementInFlight { request_id: String },
    #[error("session replacement requires zero ordinary in-flight RPC commands; {active} remain")]
    SessionReplacementRequiresQuiescence { active: usize },
    #[error("manual compaction {request_id} is already in flight")]
    ManualCompactionInFlight { request_id: String },
    #[error("manual compaction {request_id} blocks RPC command {command}")]
    ManualCompactionBlocksCommand {
        request_id: String,
        command: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::rpc::RpcCommand;

    fn request(id: &str, command: RpcCommand) -> RpcRequest {
        RpcRequest::with_id(RequestId::from_wire(id), command)
    }

    #[test]
    fn session_replacement_is_a_full_command_barrier() {
        let mut gate = RpcCommandGate::new(4);
        let replacement = request(
            "switch",
            RpcCommand::SwitchSession {
                session_path: PathBuf::from("session.jsonl"),
            },
        );
        gate.begin(&replacement).expect("begin replacement");

        assert_eq!(
            gate.begin(&request("state", RpcCommand::GetState)),
            Err(RpcGateError::SessionReplacementInFlight {
                request_id: "switch".to_owned()
            })
        );

        gate.finish(&replacement.id).expect("finish replacement");
        gate.begin(&request("state", RpcCommand::GetState))
            .expect("ordinary command resumes");
    }

    #[test]
    fn replacement_waits_for_prior_command_to_finish() {
        let mut gate = RpcCommandGate::new(4);
        let state = request("state", RpcCommand::GetState);
        gate.begin(&state).expect("begin state");

        assert_eq!(
            gate.begin(&request(
                "new",
                RpcCommand::NewSession {
                    parent_session: None
                }
            )),
            Err(RpcGateError::SessionReplacementRequiresQuiescence { active: 1 })
        );

        gate.finish(&state.id).expect("finish state");
        gate.begin(&request(
            "new",
            RpcCommand::NewSession {
                parent_session: None,
            },
        ))
        .expect("replacement after quiescence");
    }

    #[test]
    fn duplicate_manual_compaction_is_rejected_client_side() {
        let mut gate = RpcCommandGate::new(4);
        let first = request(
            "compact-1",
            RpcCommand::Compact {
                custom_instructions: None,
            },
        );
        gate.begin(&first).expect("first compaction");

        assert_eq!(
            gate.begin(&request(
                "compact-2",
                RpcCommand::Compact {
                    custom_instructions: None
                }
            )),
            Err(RpcGateError::ManualCompactionInFlight {
                request_id: "compact-1".to_owned()
            })
        );
    }

    #[test]
    fn direct_bash_and_abort_bash_may_overlap_without_crossing_replacement_barrier() {
        let mut gate = RpcCommandGate::new(4);
        let bash = request(
            "bash",
            RpcCommand::Bash {
                command: "sleep 10".to_owned(),
                exclude_from_context: None,
            },
        );
        let abort = request("abort-bash", RpcCommand::AbortBash);
        gate.begin(&bash).expect("begin bash");
        gate.begin(&abort).expect("abort control must be accepted");
        assert_eq!(gate.len(), 2);
    }

    #[test]
    fn manual_compaction_blocks_composer_commands_before_start_event_arrives() {
        let mut gate = RpcCommandGate::new(8);
        let compact = request(
            "compact",
            RpcCommand::Compact {
                custom_instructions: None,
            },
        );
        gate.begin(&compact).expect("begin compaction");

        for command in [
            RpcCommand::Prompt {
                message: "prompt".to_owned(),
                images: Vec::new(),
                streaming_behavior: None,
            },
            RpcCommand::Steer {
                message: "steer".to_owned(),
                images: Vec::new(),
            },
            RpcCommand::FollowUp {
                message: "follow up".to_owned(),
                images: Vec::new(),
            },
        ] {
            let wire_type = command.wire_type();
            assert_eq!(
                gate.begin(&request(wire_type, command)),
                Err(RpcGateError::ManualCompactionBlocksCommand {
                    request_id: "compact".to_owned(),
                    command: wire_type,
                })
            );
        }
    }

    #[test]
    fn manual_compaction_allows_read_only_state_probe_and_releases_barrier_on_response() {
        let mut gate = RpcCommandGate::new(8);
        let compact = request(
            "compact",
            RpcCommand::Compact {
                custom_instructions: None,
            },
        );
        gate.begin(&compact).expect("begin compaction");

        let state = request("state", RpcCommand::GetState);
        gate.begin(&state).expect("state probe remains safe");
        gate.finish(&state.id).expect("finish state");
        gate.finish(&compact.id).expect("finish compaction");

        gate.begin(&request(
            "prompt",
            RpcCommand::Prompt {
                message: "after".to_owned(),
                images: Vec::new(),
                streaming_behavior: None,
            },
        ))
        .expect("composer resumes after compaction response");
    }
}
