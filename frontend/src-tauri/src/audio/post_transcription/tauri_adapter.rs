//! Tauri composition root only; the process supervisor has no Tauri dependency.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tauri::{AppHandle, Manager, Runtime, State};
use tokio::sync::Mutex;

use super::gigastt_sidecar::{GigasttSidecar, GigasttSidecarConfig, SidecarStatus};

pub use super::gigastt_sidecar::PINNED_GIGASTT_VERSION as GIGASTT_VERSION;
pub use super::model_install::PINNED_MODEL_VERSION as GIGASTT_MODEL_LAYOUT_VERSION;

pub fn model_directory<R: Runtime>(app: &AppHandle<R>) -> Result<std::path::PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|_| "GigaSTT app data directory unavailable")?
        .join("models")
        .join("gigastt")
        // GigaSTT 2.21 uses the same pinned eight-file model contract as 2.18.
        // Reuse the already verified directory instead of forcing a redownload.
        .join(GIGASTT_MODEL_LAYOUT_VERSION))
}

#[derive(Default)]
pub struct GigasttSidecarState {
    manager: Mutex<Option<Arc<GigasttSidecar>>>,
    closing: AtomicBool,
}

impl GigasttSidecarState {
    /// Lazy construction does not download models or launch a process.
    pub async fn manager<R: Runtime>(
        &self,
        app: &AppHandle<R>,
    ) -> Result<Arc<GigasttSidecar>, String> {
        let mut slot = self.manager.lock().await;
        if self.closing.load(Ordering::Acquire) {
            return Err("GigaSTT application is shutting down".into());
        }
        if let Some(manager) = slot.as_ref() {
            return Ok(manager.clone());
        }
        let resources = app
            .path()
            .resource_dir()
            .map_err(|_| "GigaSTT resource directory unavailable")?;
        let binary_name = if cfg!(windows) {
            "gigastt.exe"
        } else {
            "gigastt"
        };
        // Resources keep the executable and its native DLLs together. Never search PATH.
        let binary = resources.join("gigastt").join(binary_name);
        let models = model_directory(app)?;
        let manager = Arc::new(
            GigasttSidecar::new(GigasttSidecarConfig::new(binary, models, None))
                .map_err(|e| e.to_string())?,
        );
        *slot = Some(manager.clone());
        Ok(manager)
    }

    pub async fn shutdown(&self) -> Result<(), String> {
        let manager = self.manager.lock().await.clone();
        if let Some(manager) = manager {
            manager.shutdown().await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn close(&self) -> Result<(), String> {
        self.closing.store(true, Ordering::Release);
        let manager = self.manager.lock().await.clone();
        if let Some(manager) = manager {
            manager.close().await.map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[tauri::command]
pub async fn gigastt_sidecar_status<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, GigasttSidecarState>,
) -> Result<SidecarStatus, String> {
    Ok(state.manager(&app).await?.status().await)
}

#[tauri::command]
pub async fn gigastt_start_sidecar<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, GigasttSidecarState>,
) -> Result<SidecarStatus, String> {
    let _guard = crate::audio::retranscription::RetranscriptionGuard::acquire()?;
    super::model_commands::require_pinned_models(&app).await?;
    let manager = state.manager(&app).await?;
    manager.ensure_ready().await.map_err(|e| e.to_string())?;
    Ok(manager.status().await)
}

#[tauri::command]
pub async fn gigastt_stop_sidecar(state: State<'_, GigasttSidecarState>) -> Result<(), String> {
    state.shutdown().await
}
