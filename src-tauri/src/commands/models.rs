use pi_wizard_core::model_catalog::{CustomModelProfile, ModelCatalogSnapshot};
use serde::Deserialize;

use crate::DesktopRuntime;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SaveCustomModelRequest {
    pub(crate) provider: String,
    pub(crate) model: String,
    #[serde(default)]
    pub(crate) name: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeleteCustomModelRequest {
    pub(crate) provider: String,
    pub(crate) model: String,
}

#[tauri::command]
pub(crate) async fn runtime_model_catalog(
    runtime: tauri::State<'_, DesktopRuntime>,
) -> Result<ModelCatalogSnapshot, String> {
    Ok(runtime.models.lock().await.snapshot())
}

#[tauri::command]
pub(crate) async fn runtime_save_custom_model(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: SaveCustomModelRequest,
) -> Result<CustomModelProfile, String> {
    runtime
        .models
        .lock()
        .await
        .upsert(CustomModelProfile {
            provider: request.provider,
            model: request.model,
            name: request.name,
        })
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn runtime_delete_custom_model(
    runtime: tauri::State<'_, DesktopRuntime>,
    request: DeleteCustomModelRequest,
) -> Result<bool, String> {
    runtime
        .models
        .lock()
        .await
        .remove(&request.provider, &request.model)
        .map_err(|error| error.to_string())
}
