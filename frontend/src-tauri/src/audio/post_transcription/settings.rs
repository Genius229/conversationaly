use super::{
    settings_transaction::{save_with_rollback, SettingsStore},
    types::PostTranscriptionSettings,
};
use std::sync::{Mutex, MutexGuard};
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::{Store, StoreExt};

const SETTINGS_KEY: &str = "settings";
static SETTINGS_ACCESS: Mutex<()> = Mutex::new(());

fn lock_settings_access() -> MutexGuard<'static, ()> {
    SETTINGS_ACCESS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl<R: Runtime> SettingsStore for Store<R> {
    type Error = tauri_plugin_store::Error;

    fn get(&self, key: &str) -> Option<serde_json::Value> {
        Store::get(self, key)
    }

    fn set(&self, key: &str, value: serde_json::Value) {
        Store::set(self, key, value);
    }

    fn delete(&self, key: &str) {
        Store::delete(self, key);
    }

    fn save(&self) -> Result<(), Self::Error> {
        Store::save(self)
    }
}

pub fn load<R: Runtime>(app: &AppHandle<R>) -> Result<PostTranscriptionSettings, String> {
    let _access = lock_settings_access();
    let store = app
        .store("gigastt-settings.json")
        .map_err(|_| "GigaSTT settings unavailable")?;
    match store.get(SETTINGS_KEY) {
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

fn persist<R: Runtime>(
    app: &AppHandle<R>,
    settings: PostTranscriptionSettings,
) -> Result<(), String> {
    let next = serde_json::to_value(settings).map_err(|_| "invalid GigaSTT settings")?;
    let _access = lock_settings_access();
    let store = app
        .store("gigastt-settings.json")
        .map_err(|_| "GigaSTT settings unavailable")?;

    if let Err(error) = save_with_rollback(store.as_ref(), SETTINGS_KEY, next) {
        if error.rollback_error.is_some() {
            log::warn!("Could not restore rejected GigaSTT settings on disk");
        }
        // Preserve the original save failure as the command outcome. The
        // rollback attempt must never disguise a rejected user choice as saved.
        return Err("could not save GigaSTT settings".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn gigastt_save_settings<R: Runtime>(
    app: AppHandle<R>,
    settings: PostTranscriptionSettings,
) -> Result<(), String> {
    persist(&app, settings)
}
