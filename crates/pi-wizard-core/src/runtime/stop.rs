use std::time::{Duration, Instant};

use thiserror::Error;

use crate::rpc::{ClearQueueResult, RpcCommand, RpcRequest, RpcResponse, RpcResponseOutcome};
use crate::{RequestId, RunId, RuntimeLimits};

use super::{ActivityState, ProcessState, RunMutation, RuntimeError, RuntimeStore};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopEscalationReason {
    ProcessStillStarting,
    ClearQueueRejected,
    ClearQueueInvalidResponse,
    AbortRejected,
    RpcDeadlineExpired,
    CompactionHasNoRpcAbort,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StopPhase {
    AwaitingClearQueue { request_id: RequestId },
    AwaitingAbort { request_id: RequestId },
    WaitingForAgentSettled,
    Complete,
    EscalationRequired,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StopDirective {
    Send(RpcRequest),
    WaitForAgentSettled,
    Complete {
        recovered: ClearQueueResult,
    },
    TerminateProcess {
        reason: StopEscalationReason,
        recovered: Option<ClearQueueResult>,
        termination_deadline: Duration,
    },
    None,
}

/// User-facing Stop transaction.
///
/// The transaction deliberately keeps "abort agent work" separate from
/// "terminate the owned OS process". A successful normal Stop leaves the RPC
/// child alive and reusable after `agent_settled`; escalation moves the process
/// lifecycle to `Stopping` before the process manager performs exact-identity
/// termination.
#[derive(Clone, Debug)]
pub struct StopTransaction {
    run_id: RunId,
    limits: RuntimeLimits,
    phase: StopPhase,
    recovered: Option<ClearQueueResult>,
    agent_settled: bool,
    rpc_deadline: Instant,
    termination_deadline: Duration,
}

impl StopTransaction {
    pub fn begin(
        run_id: RunId,
        now: Instant,
        limits: RuntimeLimits,
        store: &mut RuntimeStore,
    ) -> Result<(Self, StopDirective), StopTransactionError> {
        let process = store
            .get(run_id)
            .ok_or(StopTransactionError::UnknownRun { run_id })?
            .process_state();
        let mut transaction = Self {
            run_id,
            limits,
            phase: StopPhase::Complete,
            recovered: None,
            agent_settled: false,
            rpc_deadline: now + Duration::from_millis(limits.stop_abort_deadline_ms),
            termination_deadline: Duration::from_millis(limits.stop_termination_deadline_ms),
        };

        match process {
            ProcessState::Starting => {
                let directive =
                    transaction.escalate(StopEscalationReason::ProcessStillStarting, store)?;
                Ok((transaction, directive))
            }
            ProcessState::Ready => {
                let request = RpcRequest::new(RpcCommand::ClearQueue);
                transaction.phase = StopPhase::AwaitingClearQueue {
                    request_id: request.id.clone(),
                };
                Ok((transaction, StopDirective::Send(request)))
            }
            ProcessState::Stopping => Err(StopTransactionError::AlreadyStopping { run_id }),
            ProcessState::Exited | ProcessState::Failed | ProcessState::Quarantined => {
                Err(StopTransactionError::TerminalRun { run_id, process })
            }
        }
    }

    #[must_use]
    pub const fn phase(&self) -> &StopPhase {
        &self.phase
    }

    #[must_use]
    pub fn recovered(&self) -> Option<&ClearQueueResult> {
        self.recovered.as_ref()
    }

    #[must_use]
    pub const fn rpc_deadline(&self) -> Instant {
        self.rpc_deadline
    }

    pub fn on_response(
        &mut self,
        response: &RpcResponse,
        store: &mut RuntimeStore,
    ) -> Result<StopDirective, StopTransactionError> {
        match self.phase.clone() {
            StopPhase::AwaitingClearQueue { request_id } => {
                require_response_id(response, &request_id)?;
                if response.outcome() != RpcResponseOutcome::Accepted {
                    return self.escalate(StopEscalationReason::ClearQueueRejected, store);
                }
                let recovered = match response.clear_queue_result(self.limits) {
                    Ok(recovered) => recovered,
                    Err(_) => {
                        return self
                            .escalate(StopEscalationReason::ClearQueueInvalidResponse, store);
                    }
                };
                self.recovered = Some(recovered);

                if self.agent_settled {
                    return Ok(self.complete());
                }

                let activity = store
                    .get(self.run_id)
                    .ok_or(StopTransactionError::UnknownRun {
                        run_id: self.run_id,
                    })?
                    .activity_state();
                match activity {
                    ActivityState::Compacting => {
                        self.escalate(StopEscalationReason::CompactionHasNoRpcAbort, store)
                    }
                    ActivityState::Idle => Ok(self.complete()),
                    ActivityState::Working | ActivityState::WaitingForInput => {
                        store.apply(self.run_id, RunMutation::AbortRequested)?;
                        Ok(self.make_abort_request())
                    }
                    ActivityState::Aborting => Ok(self.make_abort_request()),
                }
            }
            StopPhase::AwaitingAbort { request_id } => {
                require_response_id(response, &request_id)?;
                if response.outcome() != RpcResponseOutcome::Accepted {
                    return self.escalate(StopEscalationReason::AbortRejected, store);
                }
                if self.agent_settled {
                    Ok(self.complete())
                } else {
                    self.phase = StopPhase::WaitingForAgentSettled;
                    Ok(StopDirective::WaitForAgentSettled)
                }
            }
            StopPhase::WaitingForAgentSettled
            | StopPhase::Complete
            | StopPhase::EscalationRequired => Ok(StopDirective::None),
        }
    }

    /// Records the authoritative Pi `agent_settled` semantic boundary.
    ///
    /// If queue clearing is still in flight, Stop waits for that response so it
    /// can recover queued user input before completing. Otherwise settlement
    /// completes the normal RPC stop path immediately.
    pub fn on_agent_settled(&mut self) -> StopDirective {
        self.agent_settled = true;
        match self.phase {
            StopPhase::AwaitingAbort { .. } | StopPhase::WaitingForAgentSettled => self.complete(),
            _ => StopDirective::None,
        }
    }

    pub fn on_deadline(
        &mut self,
        now: Instant,
        store: &mut RuntimeStore,
    ) -> Result<StopDirective, StopTransactionError> {
        if now < self.rpc_deadline
            || matches!(
                self.phase,
                StopPhase::Complete | StopPhase::EscalationRequired
            )
        {
            return Ok(StopDirective::None);
        }
        self.escalate(StopEscalationReason::RpcDeadlineExpired, store)
    }

    fn make_abort_request(&mut self) -> StopDirective {
        let request = RpcRequest::new(RpcCommand::Abort);
        self.phase = StopPhase::AwaitingAbort {
            request_id: request.id.clone(),
        };
        StopDirective::Send(request)
    }

    fn complete(&mut self) -> StopDirective {
        self.phase = StopPhase::Complete;
        StopDirective::Complete {
            recovered: self.recovered.clone().unwrap_or_default(),
        }
    }

    fn escalate(
        &mut self,
        reason: StopEscalationReason,
        store: &mut RuntimeStore,
    ) -> Result<StopDirective, StopTransactionError> {
        if !matches!(self.phase, StopPhase::EscalationRequired) {
            let process = store
                .get(self.run_id)
                .ok_or(StopTransactionError::UnknownRun {
                    run_id: self.run_id,
                })?
                .process_state();
            if matches!(process, ProcessState::Starting | ProcessState::Ready) {
                store.apply(self.run_id, RunMutation::BeginStop)?;
            }
            self.phase = StopPhase::EscalationRequired;
        }
        Ok(StopDirective::TerminateProcess {
            reason,
            recovered: self.recovered.clone(),
            termination_deadline: self.termination_deadline,
        })
    }
}

fn require_response_id(
    response: &RpcResponse,
    expected: &RequestId,
) -> Result<(), StopTransactionError> {
    match response.id.as_deref() {
        Some(actual) if actual == expected.as_str() => Ok(()),
        actual => Err(StopTransactionError::UnexpectedResponseId {
            expected: expected.clone(),
            actual: actual.map(str::to_owned),
        }),
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum StopTransactionError {
    #[error("run {run_id} is not registered")]
    UnknownRun { run_id: RunId },
    #[error("run {run_id} is already stopping")]
    AlreadyStopping { run_id: RunId },
    #[error("run {run_id} is already terminal in state {process:?}")]
    TerminalRun {
        run_id: RunId,
        process: ProcessState,
    },
    #[error("Stop response id {actual:?} did not match expected request {expected}")]
    UnexpectedResponseId {
        expected: RequestId,
        actual: Option<String>,
    },
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::ProjectId;
    use crate::launch::ProjectTrustPolicy;
    use crate::runtime::{ExecutionIsolation, RunRecord};

    fn store_with_activity(run_id: RunId, working: bool, compacting: bool) -> RuntimeStore {
        let mut store = RuntimeStore::new(RuntimeLimits::default());
        store
            .register(
                RunRecord::starting(
                    run_id,
                    ProjectId::new(),
                    PathBuf::from("project"),
                    ExecutionIsolation::LocalCheckout,
                    ProjectTrustPolicy::Ignore,
                )
                .expect("local run"),
            )
            .expect("register");
        store
            .apply(run_id, RunMutation::ProcessReady)
            .expect("ready");
        if working {
            store
                .apply(run_id, RunMutation::AgentStarted)
                .expect("working");
        }
        if compacting {
            store
                .apply(run_id, RunMutation::CompactionStarted)
                .expect("compacting");
        }
        store
    }

    fn success(id: &RequestId, command: &str, data: Option<serde_json::Value>) -> RpcResponse {
        RpcResponse {
            id: Some(id.as_str().to_owned()),
            command: command.to_owned(),
            success: true,
            data,
            error: None,
            extra: BTreeMap::new(),
        }
    }

    fn rejected(id: &RequestId, command: &str) -> RpcResponse {
        RpcResponse {
            id: Some(id.as_str().to_owned()),
            command: command.to_owned(),
            success: false,
            data: None,
            error: Some("rejected".to_owned()),
            extra: BTreeMap::new(),
        }
    }

    fn sent_request(directive: StopDirective) -> RpcRequest {
        let StopDirective::Send(request) = directive else {
            panic!("expected request directive");
        };
        request
    }

    #[test]
    fn normal_stop_clears_queue_before_abort_and_keeps_process_reusable() {
        let run_id = RunId::new();
        let now = Instant::now();
        let mut store = store_with_activity(run_id, true, false);
        let (mut stop, first) =
            StopTransaction::begin(run_id, now, RuntimeLimits::default(), &mut store)
                .expect("begin stop");
        let clear = sent_request(first);
        assert!(matches!(clear.command, RpcCommand::ClearQueue));

        let abort = sent_request(
            stop.on_response(
                &success(
                    &clear.id,
                    "clear_queue",
                    Some(json!({"steering":["fix this"],"followUp":["then test"]})),
                ),
                &mut store,
            )
            .expect("clear queue response"),
        );
        assert!(matches!(abort.command, RpcCommand::Abort));
        assert_eq!(
            store.get(run_id).expect("run").activity_state(),
            ActivityState::Aborting
        );

        assert_eq!(
            stop.on_response(&success(&abort.id, "abort", None), &mut store)
                .expect("abort accepted"),
            StopDirective::WaitForAgentSettled
        );
        store
            .apply(run_id, RunMutation::AgentSettled)
            .expect("settled");
        assert_eq!(
            stop.on_agent_settled(),
            StopDirective::Complete {
                recovered: ClearQueueResult {
                    steering: vec!["fix this".to_owned()],
                    follow_up: vec!["then test".to_owned()],
                }
            }
        );
        assert_eq!(
            store.get(run_id).expect("run").process_state(),
            ProcessState::Ready
        );
        assert_eq!(
            store.get(run_id).expect("run").activity_state(),
            ActivityState::Idle
        );
    }

    #[test]
    fn agent_settling_during_clear_queue_still_recovers_text_but_skips_abort() {
        let run_id = RunId::new();
        let mut store = store_with_activity(run_id, true, false);
        let (mut stop, first) =
            StopTransaction::begin(run_id, Instant::now(), RuntimeLimits::default(), &mut store)
                .expect("begin");
        let clear = sent_request(first);
        store
            .apply(run_id, RunMutation::AgentSettled)
            .expect("settled during clear");
        assert_eq!(stop.on_agent_settled(), StopDirective::None);

        assert_eq!(
            stop.on_response(
                &success(
                    &clear.id,
                    "clear_queue",
                    Some(json!({"steering":[],"followUp":["preserve"]})),
                ),
                &mut store,
            )
            .expect("clear response"),
            StopDirective::Complete {
                recovered: ClearQueueResult {
                    steering: Vec::new(),
                    follow_up: vec!["preserve".to_owned()],
                }
            }
        );
    }

    #[test]
    fn active_compaction_escalates_after_queue_recovery_instead_of_fake_abort() {
        let run_id = RunId::new();
        let mut store = store_with_activity(run_id, false, true);
        let (mut stop, first) =
            StopTransaction::begin(run_id, Instant::now(), RuntimeLimits::default(), &mut store)
                .expect("begin");
        let clear = sent_request(first);
        let directive = stop
            .on_response(
                &success(
                    &clear.id,
                    "clear_queue",
                    Some(json!({"steering":[],"followUp":[]})),
                ),
                &mut store,
            )
            .expect("clear response");

        assert!(matches!(
            directive,
            StopDirective::TerminateProcess {
                reason: StopEscalationReason::CompactionHasNoRpcAbort,
                ..
            }
        ));
        assert_eq!(
            store.get(run_id).expect("run").process_state(),
            ProcessState::Stopping
        );
    }

    #[test]
    fn clear_queue_or_abort_rejection_escalates_to_exact_process_termination_path() {
        let run_id = RunId::new();
        let mut store = store_with_activity(run_id, true, false);
        let (mut stop, first) =
            StopTransaction::begin(run_id, Instant::now(), RuntimeLimits::default(), &mut store)
                .expect("begin");
        let clear = sent_request(first);
        assert!(matches!(
            stop.on_response(&rejected(&clear.id, "clear_queue"), &mut store)
                .expect("escalate"),
            StopDirective::TerminateProcess {
                reason: StopEscalationReason::ClearQueueRejected,
                recovered: None,
                ..
            }
        ));
        assert_eq!(
            store.get(run_id).expect("run").process_state(),
            ProcessState::Stopping
        );
    }

    #[test]
    fn stop_deadline_never_optimistically_marks_run_idle() {
        let run_id = RunId::new();
        let now = Instant::now();
        let limits = RuntimeLimits {
            stop_abort_deadline_ms: 10,
            ..RuntimeLimits::default()
        };
        let mut store = store_with_activity(run_id, true, false);
        let (mut stop, _) = StopTransaction::begin(run_id, now, limits, &mut store).expect("begin");

        assert_eq!(
            stop.on_deadline(now + Duration::from_millis(9), &mut store)
                .expect("before deadline"),
            StopDirective::None
        );
        assert!(matches!(
            stop.on_deadline(now + Duration::from_millis(10), &mut store)
                .expect("deadline"),
            StopDirective::TerminateProcess {
                reason: StopEscalationReason::RpcDeadlineExpired,
                ..
            }
        ));
        assert_eq!(
            store.get(run_id).expect("run").process_state(),
            ProcessState::Stopping
        );
    }
}
