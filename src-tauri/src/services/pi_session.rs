use pi_wizard_core::RunId;
use pi_wizard_core::rpc::{RpcCommand, RpcRequest};
use pi_wizard_core::runtime::RuntimeManagerHandle;

pub(crate) async fn last_assistant_text(
    manager: &RuntimeManagerHandle,
    run_id: RunId,
) -> Result<String, String> {
    let completion = manager
        .request(run_id, RpcRequest::new(RpcCommand::GetLastAssistantText))
        .await
        .map_err(|error| error.to_string())?;
    if !completion.response.success {
        return Err(completion
            .response
            .error
            .unwrap_or_else(|| "Pi rejected get_last_assistant_text".to_owned()));
    }
    completion
        .response
        .data
        .as_ref()
        .and_then(|data| data.get("text"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| "Pi returned no last assistant text".to_owned())
}

pub(crate) async fn submit_text_prompt(
    manager: &RuntimeManagerHandle,
    run_id: RunId,
    prompt: &str,
) -> Result<(), String> {
    let completion = manager
        .request(
            run_id,
            RpcRequest::new(RpcCommand::Prompt {
                message: prompt.to_owned(),
                images: Vec::new(),
                streaming_behavior: None,
            }),
        )
        .await
        .map_err(|error| error.to_string())?;
    if completion.response.success {
        Ok(())
    } else {
        Err(completion
            .response
            .error
            .unwrap_or_else(|| "Pi rejected prompt".to_owned()))
    }
}
