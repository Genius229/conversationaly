//! Native job ownership, progress events and manual/retry commands.
use super::{
    import_audio::{
        create_import_archive, discard_unpersisted_archive, persist_imported_meeting,
        ImportAudioError, ImportWorkLease, ImportWorkRegistry,
    },
    job_state,
    service::{PostTranscriptionRequest, PostTranscriptionService},
    tauri_adapter::GigasttSidecarState,
    PostTranscriptionState,
};
use futures_util::FutureExt;
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::{sync::oneshot, task::JoinHandle};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Serialize)]
pub struct JobSnapshot {
    pub meeting_id: String,
    pub run_id: String,
    #[serde(flatten)]
    pub progress: PostTranscriptionState,
    pub cleanup_pending: bool,
}

struct ActiveJob {
    snapshot: JobSnapshot,
    cancel: CancellationToken,
    handle: JoinHandle<()>,
}

#[derive(Default)]
pub struct PostTranscriptionJobs {
    active: Mutex<Option<ActiveJob>>,
    finalized: Mutex<Option<(PathBuf, Option<PathBuf>)>>,
    imports: ImportWorkRegistry,
    closing: AtomicBool,
}

impl PostTranscriptionJobs {
    pub fn record_finalization(&self, folder: Option<PathBuf>, saved_audio: Option<PathBuf>) {
        *self.finalized.lock().unwrap_or_else(|e| e.into_inner()) =
            folder.map(|path| (path, saved_audio));
    }
    fn update(&self, snapshot: JobSnapshot) {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(job) = active
            .as_mut()
            .filter(|j| j.snapshot.run_id == snapshot.run_id)
        {
            job.snapshot = snapshot;
        }
    }

    pub async fn close(&self) {
        self.closing.store(true, Ordering::Release);
        // Pre-accept import work can outlive its webview invocation. Cancel and
        // join its RAII leases before the composition root closes SQLite or the
        // sidecar. A partial archive is removed by the copy owner.
        self.imports.close().await;
        let active = self.active.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some(mut job) = active {
            job.cancel.cancel();
            if tokio::time::timeout(Duration::from_secs(10), &mut job.handle)
                .await
                .is_err()
            {
                job.handle.abort();
                let _ = job.handle.await;
                // Durable 'running' state is recovered as interrupted on restart.
            }
        }
    }
}

/// Called after the frontend has persisted the meeting ID. Only a successful
/// native Stop finalization can authorize automatic transcription.
#[tauri::command]
pub async fn gigastt_finalize_saved_meeting<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
) -> Result<Option<JobSnapshot>, String> {
    let settings = super::settings::load(&app);
    if settings
        .as_ref()
        .is_ok_and(|settings| !settings.auto_transcribe)
    {
        return Ok(None);
    }
    let jobs = app
        .try_state::<PostTranscriptionJobs>()
        .ok_or("transcription runtime unavailable")?;
    let state = app
        .try_state::<crate::state::AppState>()
        .ok_or("database unavailable")?;
    let pool = state.db_manager.pool().clone();
    if job_state::get_job(&pool, &meeting_id).await?.is_some() {
        return gigastt_get_job_state(app.clone(), meeting_id).await;
    }
    let folder: Option<Option<String>> =
        sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id=?")
            .bind(&meeting_id)
            .fetch_optional(&pool)
            .await
            .map_err(|_| "could not read saved meeting")?;
    let folder = folder.flatten().map(PathBuf::from);
    let finalized = {
        let mut proof = jobs.finalized.lock().unwrap_or_else(|e| e.into_inner());
        if proof
            .as_ref()
            .is_some_and(|(path, _)| Some(path) == folder.as_ref())
        {
            proof.take().and_then(|(_, saved_audio)| saved_audio)
        } else {
            None
        }
    };
    // Reserve final authority before model verification or other slow startup
    // work. A summary request during that work must not see a legacy draft.
    let auto_run = uuid::Uuid::new_v4().to_string();
    job_state::begin_job(&pool, &meeting_id, &auto_run, true).await?;
    let result = if let Err(error) = settings {
        // Unknown preferences must not silently allow automatic draft/API summary.
        Err(error)
    } else if let Some(audio) = finalized {
        launch_meeting(
            app.clone(),
            meeting_id.clone(),
            Some(audio),
            Some(auto_run.clone()),
            None,
        )
        .await
    } else {
        Err("recording audio was not successfully finalized; automatic GigaSTT skipped".into())
    };
    match result {
        Ok(snapshot) => Ok(Some(snapshot)),
        Err(error) => {
            // Conditional run_id prevents a late failure from closing a retry.
            let _ = job_state::finish_job(
                &pool,
                &meeting_id,
                &auto_run,
                "failed",
                Some("auto_start_failed"),
            )
            .await;
            Err(error)
        }
    }
}

fn emit<R: Runtime>(app: &AppHandle<R>, snapshot: JobSnapshot) {
    if let Some(jobs) = app.try_state::<PostTranscriptionJobs>() {
        jobs.update(snapshot.clone());
    }
    let _ = app.emit("gigastt-transcription-progress", snapshot);
}

/// Returns immediately after reserving/persisting a job. The application owns
/// the spawned task even if the webview reloads or its invocation is dropped.
#[tauri::command]
pub async fn gigastt_transcribe_meeting<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
) -> Result<JobSnapshot, String> {
    launch_meeting(app, meeting_id, None, None, None).await
}

struct PreparedLaunch {
    guard: crate::audio::retranscription::RetranscriptionGuard,
    sidecar: Arc<super::GigasttSidecar>,
}

/// Import audio directly into a durable meeting and run the accepted Russian
/// GigaSTT profile. Copying finishes before acceptance, while transcription is
/// owned by [`PostTranscriptionJobs`] and survives webview navigation/closure.
#[tauri::command]
pub async fn gigastt_import_audio<R: Runtime>(
    app: AppHandle<R>,
    source_path: String,
    title: String,
) -> Result<JobSnapshot, String> {
    let imports = app
        .try_state::<PostTranscriptionJobs>()
        .ok_or("transcription runtime unavailable")?
        .imports
        .clone();
    let task = imports
        .spawn_owned(move |lease| run_owned_import(app, source_path, title, lease))
        .map_err(|error| error.to_string())?;
    task.await
        .map_err(|_| "native audio import worker failed".to_string())?
}

async fn run_owned_import<R: Runtime>(
    app: AppHandle<R>,
    source_path: String,
    title: String,
    lease: ImportWorkLease,
) -> Result<JobSnapshot, String> {
    let guard = acquire_job_ownership(&app).await?;
    let state = app
        .try_state::<crate::state::AppState>()
        .ok_or("database unavailable")?;
    let pool = state.db_manager.pool().clone();
    let source = PathBuf::from(source_path);
    let validation_source = source.clone();
    let validation_lease = lease.clone();
    let source_info = tokio::task::spawn_blocking(move || {
        validation_lease
            .check_active()
            .map_err(anyhow::Error::new)?;
        crate::audio::import::validate_audio_file(&validation_source)
    })
    .await
    .map_err(|_| "audio validation worker failed".to_string())?
    .map_err(|error| error.to_string())?;
    lease.check_active().map_err(|error| error.to_string())?;
    // Fail before creating an archive if the accepted GigaSTT runtime is not
    // available. There is no fallback to the legacy import inference engine.
    let sidecar = prepare_sidecar(&app).await?;
    lease.check_active().map_err(|error| error.to_string())?;
    let base_dir = crate::audio::recording_preferences::get_default_recordings_folder();
    let copy_cancel = lease.cancellation_token();

    // `block_in_place` makes this potentially long copy non-detachable: task
    // cancellation is not observed until the atomic archive copy has either
    // completed or cleaned its staging folder.
    let mut archive = tokio::task::block_in_place(|| {
        create_import_archive(
            &base_dir,
            &title,
            &source,
            source_info.duration_seconds,
            &copy_cancel,
        )
    })
    .map_err(|error| {
        format!(
            "{error}; no GigaSTT retry meeting was created because archived audio is unavailable"
        )
    })?;
    if let Err(error) = lease.check_active() {
        let _ = discard_unpersisted_archive(&archive);
        return Err(error.to_string());
    }
    if let Err(error) = persist_imported_meeting(&pool, &title, &mut archive).await {
        if matches!(&error, ImportAudioError::CommitOutcomeUnknown(_)) {
            return Err(format!(
                "{error}; archived audio for meeting {} was retained at {} for recovery because the database outcome is unknown",
                archive.meeting_id,
                archive.meeting_dir.display()
            ));
        }
        let cleanup = discard_unpersisted_archive(&archive);
        return Err(match cleanup {
            Ok(()) => format!("{error}; no imported meeting was persisted"),
            Err(cleanup_error) => format!(
                "{error}; no imported meeting was persisted and archive cleanup failed: {cleanup_error}"
            ),
        });
    }

    if let Err(error) = lease.check_active() {
        let _ = job_state::finish_job(
            &pool,
            &archive.meeting_id,
            &archive.run_id,
            "cancelled",
            Some("app_closing"),
        )
        .await;
        return Err(format!(
            "{error}; imported meeting {} and archived audio were retained for retry",
            archive.meeting_id
        ));
    }

    let meeting_id = archive.meeting_id.clone();
    let run_id = archive.run_id.clone();
    let result = launch_meeting(
        app,
        archive.meeting_id.clone(),
        Some(archive.audio_path.clone()),
        Some(archive.run_id.clone()),
        Some(PreparedLaunch { guard, sidecar }),
    )
    .await;
    match result {
        Ok(snapshot) => Ok(snapshot),
        Err(error) => {
            let status = if lease.check_active().is_err() {
                "cancelled"
            } else {
                "failed"
            };
            let code = if status == "cancelled" {
                "app_closing"
            } else {
                "import_start_failed"
            };
            let _ = job_state::finish_job(&pool, &meeting_id, &run_id, status, Some(code)).await;
            Err(format!(
                "{error}; imported meeting {meeting_id} and archived audio were retained for retry"
            ))
        }
    }
}

async fn launch_meeting<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
    finalized_audio: Option<PathBuf>,
    pending_run: Option<String>,
    prepared: Option<PreparedLaunch>,
) -> Result<JobSnapshot, String> {
    let jobs = app
        .try_state::<PostTranscriptionJobs>()
        .ok_or("transcription runtime unavailable")?;
    if jobs.closing.load(Ordering::Acquire) {
        return Err("application is shutting down".into());
    }
    let (guard, prepared_sidecar) = match prepared {
        Some(prepared) => (prepared.guard, Some(prepared.sidecar)),
        None => (acquire_job_ownership(&app).await?, None),
    };
    let state = app
        .try_state::<crate::state::AppState>()
        .ok_or("database unavailable")?;
    let pool = state.db_manager.pool().clone();
    let folder: Option<Option<String>> =
        sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id=?")
            .bind(&meeting_id)
            .fetch_optional(&pool)
            .await
            .map_err(|_| "could not find meeting audio")?;
    let meeting_dir = PathBuf::from(
        folder
            .flatten()
            .ok_or("meeting has no saved audio folder")?,
    );
    let folder_for_scan = meeting_dir.clone();
    let audio_path = match finalized_audio {
        Some(path) => path,
        None => tokio::task::spawn_blocking(move || {
            crate::audio::retranscription::find_audio_file(&folder_for_scan)
        })
        .await
        .map_err(|_| "audio lookup worker failed")?
        .map_err(|_| "meeting audio is unavailable")?,
    };
    let sidecar = match prepared_sidecar {
        Some(sidecar) => sidecar,
        None => prepare_sidecar(&app).await?,
    };
    let was_reserved = pending_run.is_some();
    let run_id = pending_run.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    // Acquiring the shared guard and joining its previous owner proves that
    // any persisted running row here is an interrupted task, not a live run.
    if !was_reserved {
        if let Some(old) = job_state::get_job(&pool, &meeting_id)
            .await?
            .filter(|j| j.status == "running")
        {
            job_state::finish_job(
                &pool,
                &meeting_id,
                &old.run_id,
                "failed",
                Some("app_interrupted"),
            )
            .await?;
        }
        job_state::begin_job(&pool, &meeting_id, &run_id, true).await?;
    }
    let snapshot = JobSnapshot {
        meeting_id: meeting_id.clone(),
        run_id: run_id.clone(),
        progress: PostTranscriptionState::PreparingAudio,
        cleanup_pending: false,
    };
    let initial = snapshot.clone();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let app_for_task = app.clone();
    let task_pool = pool.clone();
    let (start, started) = oneshot::channel();
    let registered = {
        let mut active = jobs.active.lock().unwrap_or_else(|e| e.into_inner());
        if jobs.closing.load(Ordering::Acquire) {
            false
        } else {
            let handle = tokio::spawn(async move {
                let _guard = guard;
                if started.await.is_err() {
                    return;
                }
                let service = PostTranscriptionService::new(task_pool.clone(), (*sidecar).clone());
                let progress_app = app_for_task.clone();
                let progress_base = initial.clone();
                let sink = move |progress: PostTranscriptionState| {
                    // Terminal events are emitted only after the durable status
                    // below; otherwise UI could race the backend summary gate.
                    if !matches!(
                        progress,
                        PostTranscriptionState::Ready
                            | PostTranscriptionState::Failed { .. }
                            | PostTranscriptionState::Cancelled
                    ) {
                        emit(
                            &progress_app,
                            JobSnapshot {
                                progress,
                                ..progress_base.clone()
                            },
                        );
                    }
                };
                let request = PostTranscriptionRequest {
                    meeting_id: meeting_id.clone(),
                    audio_path,
                    meeting_dir,
                };
                let decoder = Arc::new(|path: &std::path::Path| {
                    crate::audio::decoder::decode_audio_file(path)
                        .map(|audio| audio.to_whisper_format())
                        .map_err(|_| "audio decoder failed".to_string())
                });
                let result =
                    std::panic::AssertUnwindSafe(service.run(request, decoder, token, sink))
                        .catch_unwind()
                        .await;
                let (status, code, progress, cleanup_pending) = match result {
                    Ok(Ok(mut result)) => {
                        result.cleanup_pending |=
                            super::native_exports::mirror_transcript(&task_pool, &meeting_id)
                                .await
                                .is_err();
                        (
                            "ready",
                            if result.cleanup_pending {
                                Some("cleanup_pending")
                            } else {
                                None
                            },
                            PostTranscriptionState::Ready,
                            result.cleanup_pending,
                        )
                    }
                    Ok(Err(error)) if error.is_cancelled() => {
                        ("cancelled", None, PostTranscriptionState::Cancelled, false)
                    }
                    Ok(Err(error)) => {
                        log::warn!("GigaSTT run {} failed: {}", run_id, error);
                        ("failed", Some("transcription_failed"), PostTranscriptionState::Failed {message:format!("GigaSTT failed; recording and previous transcript preserved (run {run_id})")}, false)
                    }
                    Err(_) => {
                        let _ = sidecar.shutdown().await;
                        (
                            "failed",
                            Some("worker_panic"),
                            PostTranscriptionState::Failed {
                                message: format!("GigaSTT worker failed (run {run_id})"),
                            },
                            false,
                        )
                    }
                };
                let progress = match job_state::finish_job(&task_pool, &meeting_id, &run_id, status, code).await {
                Ok(()) => progress,
                Err(_) => PostTranscriptionState::Failed {message:format!("Could not persist GigaSTT completion state (run {run_id}); retry or inspect the saved transcript")},
            };
                // Ready/retry listeners may immediately launch another transcript
                // operation (e.g. optional speaker labeling). Release the shared
                // mutation guard before announcing terminal completion.
                drop(_guard);
                emit(
                    &app_for_task,
                    JobSnapshot {
                        progress,
                        cleanup_pending,
                        ..initial
                    },
                );
            });
            *active = Some(ActiveJob {
                snapshot: snapshot.clone(),
                cancel,
                handle,
            });
            true
        }
    };
    if !registered {
        job_state::finish_job(
            &pool,
            &snapshot.meeting_id,
            &snapshot.run_id,
            "cancelled",
            Some("app_closing"),
        )
        .await?;
        return Err("application is shutting down".into());
    }
    let _ = start.send(());
    Ok(snapshot)
}

async fn acquire_job_ownership<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<crate::audio::retranscription::RetranscriptionGuard, String> {
    let jobs = app
        .try_state::<PostTranscriptionJobs>()
        .ok_or("transcription runtime unavailable")?;
    if jobs.closing.load(Ordering::Acquire) {
        return Err("application is shutting down".into());
    }
    let guard = crate::audio::retranscription::RetranscriptionGuard::acquire()?;
    if crate::audio::recording_commands::is_recording_now() {
        return Err("stop recording before re-transcribing".into());
    }
    // The shared guard proves the previous task has released its transcript
    // ownership. Await its retained JoinHandle rather than detaching it.
    let previous = jobs.active.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(previous) = previous {
        let _ = previous.handle.await;
    }
    Ok(guard)
}

async fn prepare_sidecar<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<Arc<super::GigasttSidecar>, String> {
    let sidecar_state = app
        .try_state::<GigasttSidecarState>()
        .ok_or("GigaSTT runtime unavailable")?;
    super::model_commands::require_pinned_models(app).await?;
    let sidecar = sidecar_state.manager(app).await?;
    // Retry is explicit. Reset a failed lifecycle without an endless auto-restart.
    if matches!(
        sidecar.status().await,
        super::SidecarStatus::Failed | super::SidecarStatus::NotInstalled
    ) {
        sidecar.shutdown().await.map_err(|e| e.to_string())?;
    }
    Ok(sidecar)
}

#[tauri::command]
pub async fn gigastt_cancel_transcription<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
) -> Result<bool, String> {
    let jobs = app
        .try_state::<PostTranscriptionJobs>()
        .ok_or("transcription runtime unavailable")?;
    let active = jobs.active.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(job) = active
        .as_ref()
        .filter(|j| j.snapshot.meeting_id == meeting_id && !j.handle.is_finished())
    {
        job.cancel.cancel();
        return Ok(true);
    }
    Ok(false)
}

#[tauri::command]
pub async fn gigastt_get_job_state<R: Runtime>(
    app: AppHandle<R>,
    meeting_id: String,
) -> Result<Option<JobSnapshot>, String> {
    let memory = if let Some(jobs) = app.try_state::<PostTranscriptionJobs>() {
        let active = jobs.active.lock().unwrap_or_else(|e| e.into_inner());
        active
            .as_ref()
            .filter(|j| j.snapshot.meeting_id == meeting_id)
            .map(|j| (j.snapshot.clone(), j.handle.is_finished()))
    } else {
        None
    };
    if let Some((snapshot, false)) = &memory {
        return Ok(Some(snapshot.clone()));
    }
    let state = app
        .try_state::<crate::state::AppState>()
        .ok_or("database unavailable")?;
    let persisted = job_state::get_job(state.db_manager.pool(), &meeting_id).await?;
    // A successful legacy retranscription can clear GigaSTT authority. Never
    // resurrect its old terminal UI snapshot after that committed replacement.
    if let (Some((snapshot, _)), Some(job)) = (&memory, &persisted) {
        if snapshot.run_id == job.run_id {
            return Ok(Some(snapshot.clone()));
        }
    }
    Ok(persisted.map(|job| JobSnapshot {
        meeting_id: job.meeting_id,
        run_id: job.run_id,
        cleanup_pending: job.error_code.as_deref() == Some("cleanup_pending"),
        progress: match job.status.as_str() {
            "running" => PostTranscriptionState::PreparingAudio,
            "ready" => PostTranscriptionState::Ready,
            "cancelled" => PostTranscriptionState::Cancelled,
            _ => PostTranscriptionState::Failed {
                message: "GigaSTT did not finish; retry or explicitly use the current transcript"
                    .into(),
            },
        },
    }))
}
