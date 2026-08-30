use pi_wizard_core::RunId;
use pi_wizard_core::rpc::{RpcCommand, RpcRequest};
use pi_wizard_core::runtime::RuntimeManagerHandle;

pub(crate) async fn last_assistant_text(
    manager: &RuntimeManagerHandle,
    run_id: RunId,
) -> Result<Option<String>, String> {
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
    let data = completion
        .response
        .data
        .as_ref()
        .ok_or_else(|| "Pi returned no data for get_last_assistant_text".to_owned())?;
    let text = data
        .get("text")
        .ok_or_else(|| "Pi returned no text field for get_last_assistant_text".to_owned())?;
    match text {
        serde_json::Value::Null => Ok(None),
        serde_json::Value::String(text) => Ok(Some(text.clone())),
        _ => Err("Pi returned a non-string text field for get_last_assistant_text".to_owned()),
    }
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
