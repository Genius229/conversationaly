//! Framework-independent post-recording GigaSTT coordinator.

use std::{
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use sqlx::SqlitePool;
use thiserror::Error;
use tokio::time::{sleep, timeout, Instant};
use tokio_util::sync::CancellationToken;

use super::{
    artifacts::{cleanup_file, persist_result},
    audio_prepare::{prepare_audio, AudioPrepareError, PreparedAudio},
    gigastt_client::{GigasttClient, GigasttClientError, SubmitJobOptions},
    gigastt_sidecar::{GigasttSidecar, SidecarError},
    importer::{replace_transcript, ImportError},
    meeting_identity::{self, MeetingIdentityError},
    types::{GigasttJobStatus, PostTranscriptionState},
};

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(500);
const DEFAULT_JOB_DEADLINE: Duration = Duration::from_secs(24 * 60 * 60);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(35);

#[derive(Debug, Clone)]
pub struct PostTranscriptionRequest {
    pub meeting_id: String,
    pub audio_path: PathBuf,
    pub meeting_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PostTranscriptionResult {
    pub meeting_id: String,
    pub row_count: usize,
    pub duration_seconds: f64,
    /// The database commit succeeded, but one or more disposable artifacts
    /// could not be removed and should be retried by a later cleanup pass.
    pub cleanup_pending: bool,
}

#[derive(Debug, Error)]
pub enum PostTranscriptionError {
    #[error("meeting does not exist")]
    MeetingNotFound,
    #[error("meeting folder does not match the stored meeting")]
    MeetingFolderMismatch,
    #[error("post-transcription was cancelled")]
    Cancelled,
    #[error("GigaSTT job was cancelled")]
    RemoteCancelled,
    #[error("GigaSTT job failed")]
    JobFailed,
    #[error("GigaSTT job exceeded its processing deadline")]
    PollDeadline,
    #[error("meeting lookup failed")]
    MeetingLookup(#[source] sqlx::Error),
    #[error("audio preparation failed")]
    AudioPreparation(#[source] AudioPrepareError),
    #[error("GigaSTT sidecar is unavailable")]
    Sidecar(#[source] SidecarError),
    #[error("GigaSTT request failed during {stage}")]
    Client {
        stage: &'static str,
        #[source]
        source: GigasttClientError,
    },
    #[error("GigaSTT request timed out during {stage}")]
    RequestTimeout { stage: &'static str },
    #[error("GigaSTT submission outcome is unknown")]
    SubmissionUnknown(#[source] GigasttClientError),
    #[error("GigaSTT result could not be saved")]
    ResultPersistence,
    #[error("transcript replacement failed")]
    Import(#[source] ImportError),
}

impl PostTranscriptionError {
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled | Self::RemoteCancelled)
    }
}

#[derive(Clone)]
pub struct PostTranscriptionService {
    pool: SqlitePool,
    sidecar: GigasttSidecar,
    poll_interval: Duration,
    job_deadline: Duration,
    request_timeout: Duration,
}

impl PostTranscriptionService {
    pub fn new(pool: SqlitePool, sidecar: GigasttSidecar) -> Self {
        Self {
            pool,
            sidecar,
            poll_interval: DEFAULT_POLL_INTERVAL,
            job_deadline: DEFAULT_JOB_DEADLINE,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }

    /// Set finite orchestration limits. This is primarily useful for compact
    /// deployments and deterministic contract tests; zero values are raised to
    /// one millisecond so no operation becomes accidentally unbounded.
    pub fn with_limits(
        mut self,
        poll_interval: Duration,
        job_deadline: Duration,
        request_timeout: Duration,
    ) -> Self {
        let minimum = Duration::from_millis(1);
        self.poll_interval = poll_interval.max(minimum);
        self.job_deadline = job_deadline.max(minimum);
        self.request_timeout = request_timeout.max(minimum);
        self
    }

    pub async fn run<D, F>(
        &self,
        request: PostTranscriptionRequest,
        decoder: Arc<D>,
        cancel: CancellationToken,
        emit: F,
    ) -> Result<PostTranscriptionResult, PostTranscriptionError>
    where
        D: Fn(&Path) -> Result<Vec<f32>, String> + Send + Sync + 'static,
        F: Fn(PostTranscriptionState) + Send + Sync,
    {
        emit(PostTranscriptionState::PreparingAudio);
        let result = self.run_pipeline(&request, decoder, &cancel, &emit).await;
        if let Err(error) = &result {
            if error.is_cancelled() {
                emit(PostTranscriptionState::Cancelled);
            } else {
                emit(PostTranscriptionState::Failed {
                    message: error.to_string(),
                });
            }
        }
        result
    }

    async fn run_pipeline<D, F>(
        &self,
        request: &PostTranscriptionRequest,
        decoder: Arc<D>,
        cancel: &CancellationToken,
        emit: &F,
    ) -> Result<PostTranscriptionResult, PostTranscriptionError>
    where
        D: Fn(&Path) -> Result<Vec<f32>, String> + Send + Sync + 'static,
        F: Fn(PostTranscriptionState) + Send + Sync,
    {
        meeting_identity::verify(&self.pool, &request.meeting_id, &request.meeting_dir)
            .await
            .map_err(|error| match error {
                MeetingIdentityError::NotFound => PostTranscriptionError::MeetingNotFound,
                MeetingIdentityError::Mismatch => PostTranscriptionError::MeetingFolderMismatch,
                MeetingIdentityError::Database(source) => {
                    PostTranscriptionError::MeetingLookup(source)
                }
            })?;
        self.check_cancelled(cancel)?;
        let prepared = prepare_audio(
            request.audio_path.clone(),
            request.meeting_dir.clone(),
            decoder,
            cancel.clone(),
        )
        .await
        .map_err(|error| match error {
            AudioPrepareError::Cancelled => PostTranscriptionError::Cancelled,
            other => PostTranscriptionError::AudioPreparation(other),
        })?;

        emit(PostTranscriptionState::Starting);
        let client = tokio::select! {
            ready = self.sidecar.ensure_ready() => ready.map_err(PostTranscriptionError::Sidecar)?,
            _ = cancel.cancelled() => {
                let _ = self.sidecar.shutdown().await;
                return Err(PostTranscriptionError::Cancelled);
            }
        };
        self.check_cancelled(cancel)?;
        self.sidecar
            .set_busy(true)
            .await
            .map_err(PostTranscriptionError::Sidecar)?;

        let result = self
            .run_owned_job(request, prepared, client, cancel, emit)
            .await;
        // set_busy(false) cannot resurrect a lifecycle failure.
        let _ = self.sidecar.set_busy(false).await;
        result
    }

    async fn run_owned_job<F>(
        &self,
        request: &PostTranscriptionRequest,
        prepared: PreparedAudio,
        client: GigasttClient,
        cancel: &CancellationToken,
        emit: &F,
    ) -> Result<PostTranscriptionResult, PostTranscriptionError>
    where
        F: Fn(PostTranscriptionState) + Send + Sync,
    {
        emit(PostTranscriptionState::Transcribing { percent: 0 });
        if cancel.is_cancelled() {
            return Err(PostTranscriptionError::Cancelled);
        }
        // Do not race submission against cancellation. A request already on the
        // wire must resolve within its bound before its accepted job can be
        // cancelled by ID. A timeout/transport loss has no trustworthy ID, so
        // stop the owned server rather than silently abandoning work.
        let submission = timeout(
            self.request_timeout,
            client.submit_file(&prepared.path, &SubmitJobOptions::default()),
        )
        .await;
        let submitted = match submission {
            Ok(Ok(value)) => value,
            Ok(Err(source)) => {
                let unknown = matches!(
                    source,
                    GigasttClientError::Transport(_)
                        | GigasttClientError::InvalidResponse(_)
                        | GigasttClientError::BodyTooLarge { .. }
                );
                if unknown {
                    let _ = self.sidecar.shutdown().await;
                    return Err(PostTranscriptionError::SubmissionUnknown(source));
                }
                return Err(PostTranscriptionError::Client {
                    stage: "submission",
                    source,
                });
            }
            Err(_) => {
                let _ = self.sidecar.shutdown().await;
                return Err(PostTranscriptionError::RequestTimeout {
                    stage: "submission",
                });
            }
        };
        let job_id = submitted.job_id;
        if cancel.is_cancelled() {
            self.cancel_job(&client, &job_id).await;
            return Err(PostTranscriptionError::Cancelled);
        }

        let deadline = Instant::now() + self.job_deadline;
        loop {
            if cancel.is_cancelled() {
                self.cancel_job(&client, &job_id).await;
                return Err(PostTranscriptionError::Cancelled);
            }
            if Instant::now() >= deadline {
                self.cancel_job(&client, &job_id).await;
                return Err(PostTranscriptionError::PollDeadline);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let status = match self
                .bounded_for(
                    "status",
                    self.request_timeout.min(remaining),
                    client.get_job(&job_id),
                )
                .await
            {
                Ok(value) => value,
                Err(PostTranscriptionError::RequestTimeout { .. })
                    if Instant::now() >= deadline =>
                {
                    self.cancel_job(&client, &job_id).await;
                    return Err(PostTranscriptionError::PollDeadline);
                }
                Err(error) => {
                    self.cancel_job(&client, &job_id).await;
                    return Err(error);
                }
            };
            emit(PostTranscriptionState::Transcribing {
                percent: status.percent,
            });
            match status.status {
                GigasttJobStatus::Done => break,
                GigasttJobStatus::Failed => {
                    self.cancel_job(&client, &job_id).await;
                    return Err(PostTranscriptionError::JobFailed);
                }
                GigasttJobStatus::Cancelled => {
                    self.cancel_job(&client, &job_id).await;
                    return Err(PostTranscriptionError::RemoteCancelled);
                }
                GigasttJobStatus::Queued | GigasttJobStatus::Processing => {}
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            tokio::select! {
                _ = cancel.cancelled() => {
                    self.cancel_job(&client, &job_id).await;
                    return Err(PostTranscriptionError::Cancelled);
                }
                _ = sleep(self.poll_interval.min(remaining)) => {}
            }
        }

        if cancel.is_cancelled() {
            self.cancel_job(&client, &job_id).await;
            return Err(PostTranscriptionError::Cancelled);
        }
        emit(PostTranscriptionState::Finalizing);
        let result = match self.bounded("result", client.get_result(&job_id)).await {
            Ok(value) => value,
            Err(error) => {
                self.cancel_job(&client, &job_id).await;
                return Err(error);
            }
        };
        if cancel.is_cancelled() {
            self.cancel_job(&client, &job_id).await;
            return Err(PostTranscriptionError::Cancelled);
        }

        let result_path = match persist_result(&prepared.path, &result).await {
            Ok(path) => path,
            Err(_) => {
                self.cancel_job(&client, &job_id).await;
                return Err(PostTranscriptionError::ResultPersistence);
            }
        };
        if cancel.is_cancelled() {
            self.cancel_job(&client, &job_id).await;
            return Err(PostTranscriptionError::Cancelled);
        }
        let row_count =
            match replace_transcript(&self.pool, &request.meeting_id, &result, cancel).await {
                Ok(count) => count,
                Err(ImportError::Cancelled) => {
                    self.cancel_job(&client, &job_id).await;
                    return Err(PostTranscriptionError::Cancelled);
                }
                Err(error) => {
                    self.cancel_job(&client, &job_id).await;
                    return Err(PostTranscriptionError::Import(error));
                }
            };

        // Commit is the cancellation checkpoint. From here the authoritative
        // transcript exists, so late cancellation cannot turn success into a
        // retry/failure or cause a second destructive import.
        let cleanup_pending = cleanup_file(&prepared.path).await | cleanup_file(&result_path).await;
        emit(PostTranscriptionState::Ready);
        Ok(PostTranscriptionResult {
            meeting_id: request.meeting_id.clone(),
            row_count,
            duration_seconds: result.duration,
            cleanup_pending,
        })
    }

    fn check_cancelled(&self, cancel: &CancellationToken) -> Result<(), PostTranscriptionError> {
        if cancel.is_cancelled() {
            Err(PostTranscriptionError::Cancelled)
        } else {
            Ok(())
        }
    }

    async fn bounded<T>(
        &self,
        stage: &'static str,
        request: impl Future<Output = Result<T, GigasttClientError>>,
    ) -> Result<T, PostTranscriptionError> {
        self.bounded_for(stage, self.request_timeout, request).await
    }

    async fn bounded_for<T>(
        &self,
        stage: &'static str,
        limit: Duration,
        request: impl Future<Output = Result<T, GigasttClientError>>,
    ) -> Result<T, PostTranscriptionError> {
        timeout(limit, request)
            .await
            .map_err(|_| PostTranscriptionError::RequestTimeout { stage })?
            .map_err(|source| PostTranscriptionError::Client { stage, source })
    }

    async fn cancel_job(&self, client: &GigasttClient, job_id: &str) {
        if self
            .bounded("cancellation", client.cancel_job(job_id))
            .await
            .is_err()
        {
            let _ = self.sidecar.shutdown().await;
        }
    }
}
