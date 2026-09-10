//! Explicit model install/repair actions; normal startup stays offline.
use super::{
    model_install::{ModelAvailability, ModelInstallProgress, ModelInstaller},
    tauri_adapter::{model_directory, GigasttSidecarState},
};
use futures_util::FutureExt;
use serde::Serialize;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::{sync::oneshot, task::JoinHandle};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize)]
pub struct ModelDownloadState {
    state: String,
    progress: Option<ModelInstallProgress>,
    message: Option<String>,
}
impl Default for ModelDownloadState {
    fn default() -> Self {
        Self {
            state: "idle".into(),
            progress: None,
            message: None,
        }
    }
}
struct ModelJob {
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}
#[derive(Default)]
pub struct ModelJobs {
    job: Mutex<Option<ModelJob>>,
    snapshot: Mutex<ModelDownloadState>,
    closing: AtomicBool,
}
impl ModelJobs {
    pub async fn close(&self) {
        self.closing.store(true, Ordering::Release);
        let job = self.job.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut job) = job {
            job.cancel.cancel();
            if tokio::time::timeout(Duration::from_secs(10), &mut job.handle)
                .await
                .is_err()
            {
                job.handle.abort();
                let _ = job.handle.await;
            }
        }
    }
    fn update<R: Runtime>(&self, app: &AppHandle<R>, value: ModelDownloadState) {
        *self.snapshot.lock().unwrap_or_else(|e| e.into_inner()) = value.clone();
        let _ = app.emit("gigastt-model-state", value);
    }
}

#[tauri::command]
pub async fn gigastt_model_status<R: Runtime>(
    app: AppHandle<R>,
) -> Result<ModelAvailability, String> {
    ModelInstaller::new(model_directory(&app)?)
        .map_err(|e| e.to_string())?
        .verify()
        .await
        .map_err(|e| e.to_string())
}

pub async fn require_pinned_models<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let available = ModelInstaller::new(model_directory(app)?)
        .map_err(|e| e.to_string())?
        .verify()
        .await
        .map_err(|e| e.to_string())?;
    if !available.is_ready() {
        return Err("GigaSTT models are missing or corrupt; download or repair them first".into());
    }
    Ok(())
}

#[tauri::command]
pub async fn gigastt_model_download_state<R: Runtime>(
    app: AppHandle<R>,
) -> Result<ModelDownloadState, String> {
    let state = app
        .try_state::<ModelJobs>()
        .ok_or("model runtime unavailable")?;
    let snapshot = state
        .snapshot
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    Ok(snapshot)
}

#[tauri::command]
pub async fn gigastt_cancel_model_install<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    let state = app
        .try_state::<ModelJobs>()
        .ok_or("model runtime unavailable")?;
    let job = state.job.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(job) = job.as_ref().filter(|j| !j.handle.is_finished()) {
        job.cancel.cancel();
        return Ok(true);
    }
    Ok(false)
}

#[tauri::command]
pub async fn gigastt_install_models<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let guard = crate::audio::retranscription::RetranscriptionGuard::acquire()?;
    let state = app
        .try_state::<ModelJobs>()
        .ok_or("model runtime unavailable")?;
    if state.closing.load(Ordering::Acquire) {
        return Err("application is shutting down".into());
    }
    let previous = state.job.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(previous) = previous {
        let _ = previous.handle.await;
    }
    // Never replace a model file while an engine may hold it open on Windows.
    let sidecar = app
        .try_state::<GigasttSidecarState>()
        .ok_or("GigaSTT runtime unavailable")?;
    sidecar.shutdown().await?;
    let installer = ModelInstaller::new(model_directory(&app)?).map_err(|e| e.to_string())?;
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let task_app = app.clone();
    let (start, started) = oneshot::channel();
    {
        let mut job = state.job.lock().unwrap_or_else(|e| e.into_inner());
        if state.closing.load(Ordering::Acquire) {
            return Err("application is shutting down".into());
        }
        let handle = tokio::spawn(async move {
            let _guard = guard;
            if started.await.is_err() {
                return;
            }
            let progress_app = task_app.clone();
            let progress = move |progress: ModelInstallProgress| {
                if let Some(state) = progress_app.try_state::<ModelJobs>() {
                    state.update(
                        &progress_app,
                        ModelDownloadState {
                            state: "running".into(),
                            progress: Some(progress.clone()),
                            message: None,
                        },
                    );
                }
                let _ = progress_app.emit("gigastt-model-progress", progress);
            };
            let result = std::panic::AssertUnwindSafe(installer.install(token.clone(), progress))
                .catch_unwind()
                .await;
            let snapshot = match result {
                Ok(Ok(_)) => ModelDownloadState {
                    state: "ready".into(),
                    progress: None,
                    message: None,
                },
                Ok(Err(_)) if token.is_cancelled() => ModelDownloadState {
                    state: "cancelled".into(),
                    progress: None,
                    message: None,
                },
                Ok(Err(error)) => ModelDownloadState {
                    state: "failed".into(),
                    progress: None,
                    message: Some(error.to_string()),
                },
                Err(_) => ModelDownloadState {
                    state: "failed".into(),
                    progress: None,
                    message: Some("model installer worker failed".into()),
                },
            };
            drop(_guard);
            if let Some(state) = task_app.try_state::<ModelJobs>() {
                state.update(&task_app, snapshot);
            }
        });
        *job = Some(ModelJob { cancel, handle });
    }
    state.update(
        &app,
        ModelDownloadState {
            state: "running".into(),
            progress: None,
            message: None,
        },
    );
    let _ = start.send(());
    Ok(())
}
