use pi_wizard_core::RunId;
use pi_wizard_core::runtime::{ProcessState, RuntimeManagerHandle};

/// Terminates one app-internal Pi run without changing lifecycle of any
/// user-owned worker run. This is shared by internal workflow services only.
pub(crate) async fn terminate_internal_run(
    manager: &RuntimeManagerHandle,
    run_id: RunId,
) -> Result<(), String> {
    let hydration = manager.hydrate().await.map_err(|error| error.to_string())?;
    let Some(run) = hydration.runs.iter().find(|run| run.run.id() == run_id) else {
        return Ok(());
    };
    if run.run.process_state().is_terminal() {
        return Ok(());
    }
    if run.run.process_state() == ProcessState::Starting {
        let closed = manager
            .close_run(run_id)
            .await
            .map_err(|error| error.to_string())?;
        if closed.quarantined || !closed.process_terminated {
            return Err(format!(
                "internal run {run_id} could not be confirmed terminated during startup"
            ));
        }
        return Ok(());
    }
    if run.run.process_state() == ProcessState::Ready
        && run.run.activity_state() != pi_wizard_core::runtime::ActivityState::Idle
    {
        let stopped = manager
            .stop_run(run_id)
            .await
            .map_err(|error| error.to_string())?;
        if stopped.quarantined {
            return Err(format!(
                "internal run {run_id} Stop left process termination uncertain"
            ));
        }
        if stopped.process_terminated {
            return Ok(());
        }
    }

    let hydration = manager.hydrate().await.map_err(|error| error.to_string())?;
    let Some(run) = hydration.runs.iter().find(|run| run.run.id() == run_id) else {
        return Ok(());
    };
    if run.run.process_state().is_terminal() {
        return Ok(());
    }
    if run.run.process_state() == ProcessState::Ready
        && run.run.activity_state() == pi_wizard_core::runtime::ActivityState::Idle
    {
        let closed = manager
            .close_run(run_id)
            .await
            .map_err(|error| error.to_string())?;
        if closed.quarantined || !closed.process_terminated {
            return Err(format!(
                "internal run {run_id} Close could not confirm process termination"
            ));
        }
        return Ok(());
    }

    Err(format!(
        "internal run {run_id} could not be terminated from process state {:?}",
        run.run.process_state()
    ))
}
