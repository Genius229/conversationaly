use super::types::PostTranscriptionSettings;
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

pub fn load<R: Runtime>(app: &AppHandle<R>) -> Result<PostTranscriptionSettings, String> {
    let store = app
        .store("gigastt-settings.json")
        .map_err(|_| "GigaSTT settings unavailable")?;
    match store.get("settings") {
        Some(value) => serde_json::from_value(value).map_err(|_| "invalid GigaSTT settings".into()),
        None => Ok(PostTranscriptionSettings::default()),
    }
}

#[tauri::command]
pub async fn gigastt_get_settings<R: Runtime>(
    app: AppHandle<R>,
) -> Result<PostTranscriptionSettings, String> {
    load(&app)
}

#[tauri::command]
pub async fn gigastt_save_settings<R: Runtime>(
    app: AppHandle<R>,
    settings: PostTranscriptionSettings,
) -> Result<(), String> {
    let store = app
        .store("gigastt-settings.json")
        .map_err(|_| "GigaSTT settings unavailable")?;
    store.set(
        "settings",
        serde_json::to_value(settings).map_err(|_| "invalid GigaSTT settings")?,
    );
    store
        .save()
        .map_err(|_| "could not save GigaSTT settings".into())
}
