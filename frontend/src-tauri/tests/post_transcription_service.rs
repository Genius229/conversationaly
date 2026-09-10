use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use app_lib::audio::post_transcription::{
    GigasttSidecar, GigasttSidecarConfig, PostTranscriptionError, PostTranscriptionRequest,
    PostTranscriptionService, PostTranscriptionState, SidecarStatus,
};
use sqlx::SqlitePool;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

fn fake_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake-gigastt"))
}

async fn migrated_pool() -> SqlitePool {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let migrations_dir = manifest_dir.join("../../frontend/src-tauri/migrations");
    sqlx::migrate::Migrator::new(migrations_dir)
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    pool
}

fn write_control(model_dir: &Path, name: &str, value: &str) {
    fs::create_dir_all(model_dir).unwrap();
    fs::write(model_dir.join(name), value).unwrap();
}

async fn insert_meeting(pool: &SqlitePool, id: &str, folder: &Path) {
    let canonical = folder.canonicalize().unwrap();
    sqlx::query(
        "INSERT INTO meetings (id, title, created_at, updated_at, folder_path)
         VALUES (?, 'Service test', '2026-09-10T00:00:00Z', '2026-09-10T00:00:00Z', ?)",
    )
    .bind(id)
    .bind(canonical.to_string_lossy().as_ref())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO transcripts
         (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration)
         VALUES ('draft-row', ?, 'draft survives', '2026-09-10T00:00:00Z', 0, 1, 1)",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO meeting_transcript_metadata
         (meeting_id, provenance, result_metadata, updated_at)
         VALUES (?, 'draft_live', '{\"draft\":true}', '2026-09-10T00:00:00Z')",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
}

struct Fixture {
    _temp: TempDir,
    meeting_dir: PathBuf,
    audio_path: PathBuf,
    model_dir: PathBuf,
    pool: SqlitePool,
    sidecar: GigasttSidecar,
}

impl Fixture {
    async fn new(with_meeting: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let meeting_dir = temp.path().join("meeting");
        fs::create_dir_all(&meeting_dir).unwrap();
        let audio_path = meeting_dir.join("audio.mp4");
        fs::write(&audio_path, b"immutable original recording").unwrap();
        let model_dir = temp.path().join("models");
        let pool = migrated_pool().await;
        if with_meeting {
            insert_meeting(&pool, "meeting-a", &meeting_dir).await;
        }
        let config = GigasttSidecarConfig::new(fake_binary(), &model_dir, None)
            .with_startup_timeout(Duration::from_millis(700))
            .with_poll_interval(Duration::from_millis(10))
            .with_request_timeout(Duration::from_millis(100))
            .with_shutdown_timeouts(Duration::from_millis(100), Duration::from_millis(200));
        let sidecar = GigasttSidecar::new(config).unwrap();
        Self {
            _temp: temp,
            meeting_dir,
            audio_path,
            model_dir,
            pool,
            sidecar,
        }
    }

    fn request(&self) -> PostTranscriptionRequest {
        PostTranscriptionRequest {
            meeting_id: "meeting-a".to_string(),
            audio_path: self.audio_path.clone(),
            meeting_dir: self.meeting_dir.clone(),
        }
    }

    fn service(&self) -> PostTranscriptionService {
        PostTranscriptionService::new(self.pool.clone(), self.sidecar.clone()).with_limits(
            Duration::from_millis(10),
            Duration::from_millis(350),
            Duration::from_millis(250),
        )
    }
}

fn decode_samples(_: &Path) -> Result<Vec<f32>, String> {
    Ok(vec![0.25; 5_000])
}

type TestDecoder = fn(&Path) -> Result<Vec<f32>, String>;

fn decoder() -> Arc<TestDecoder> {
    Arc::new(decode_samples)
}

fn state_sink() -> (
    Arc<Mutex<Vec<PostTranscriptionState>>>,
    impl Fn(PostTranscriptionState) + Send + Sync,
) {
    let states = Arc::new(Mutex::new(Vec::new()));
    let output = states.clone();
    (states, move |state| output.lock().unwrap().push(state))
}

async fn draft(pool: &SqlitePool) -> (String, String) {
    let text: String = sqlx::query_scalar(
        "SELECT transcript FROM transcripts WHERE meeting_id = 'meeting-a' ORDER BY id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let provenance: String = sqlx::query_scalar(
        "SELECT provenance FROM meeting_transcript_metadata WHERE meeting_id = 'meeting-a'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    (text, provenance)
}

#[tokio::test]
async fn happy_pipeline_emits_order_imports_and_cleans_temporary_files() {
    let fixture = Fixture::new(true).await;
    write_control(
        &fixture.model_dir,
        "fake-job-statuses",
        "processing:41,done:0",
    );
    let (states, emit) = state_sink();

    let result = fixture
        .service()
        .run(fixture.request(), decoder(), CancellationToken::new(), emit)
        .await
        .unwrap();

    assert_eq!(result.meeting_id, "meeting-a");
    assert_eq!(result.row_count, 1);
    assert_eq!(result.duration_seconds, 1.0);
    assert!(!result.cleanup_pending);
    assert_eq!(
        *states.lock().unwrap(),
        vec![
            PostTranscriptionState::PreparingAudio,
            PostTranscriptionState::Starting,
            PostTranscriptionState::Transcribing { percent: 0 },
            PostTranscriptionState::Transcribing { percent: 41 },
            PostTranscriptionState::Transcribing { percent: 0 },
            PostTranscriptionState::Finalizing,
            PostTranscriptionState::Ready,
        ]
    );
    assert_eq!(draft(&fixture.pool).await.0, "Финальный текст");
    assert_eq!(draft(&fixture.pool).await.1, "final_gigastt");
    assert!(!fixture
        .meeting_dir
        .join(".processing/gigastt-input.wav")
        .exists());
    assert!(!fixture
        .meeting_dir
        .join(".processing/gigastt-result.json")
        .exists());
    assert_eq!(
        fs::read(&fixture.audio_path).unwrap(),
        b"immutable original recording"
    );
    assert_eq!(
        fs::read_to_string(fixture.model_dir.join("fake-upload-lengths"))
            .unwrap()
            .trim(),
        "10044",
        "the process fake must consume the complete >4 KiB WAV"
    );
    assert_eq!(fixture.sidecar.status().await, SidecarStatus::Ready);
    fixture.sidecar.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_after_commit_does_not_change_success_into_failure() {
    let fixture = Fixture::new(true).await;
    write_control(&fixture.model_dir, "fake-job-statuses", "done:0");
    let cancel = CancellationToken::new();
    let cancel_on_ready = cancel.clone();

    let result = fixture
        .service()
        .run(fixture.request(), decoder(), cancel, move |state| {
            if state == PostTranscriptionState::Ready {
                cancel_on_ready.cancel();
            }
        })
        .await
        .unwrap();

    assert_eq!(result.row_count, 1);
    assert_eq!(draft(&fixture.pool).await.1, "final_gigastt");
    assert!(!fixture.model_dir.join("fake-cancelled-jobs").exists());
    fixture.sidecar.shutdown().await.unwrap();
}

#[tokio::test]
async fn failed_job_cancels_remote_and_preserves_draft_without_leaking_content() {
    let fixture = Fixture::new(true).await;
    write_control(&fixture.model_dir, "fake-job-statuses", "failed:37");
    let (states, emit) = state_sink();

    let error = fixture
        .service()
        .run(fixture.request(), decoder(), CancellationToken::new(), emit)
        .await
        .unwrap_err();

    assert!(matches!(error, PostTranscriptionError::JobFailed));
    assert!(!error.to_string().contains("PRIVATE_TRANSCRIPT_PAYLOAD"));
    assert_eq!(
        draft(&fixture.pool).await,
        ("draft survives".into(), "draft_live".into())
    );
    assert!(fixture
        .meeting_dir
        .join(".processing/gigastt-input.wav")
        .exists());
    assert!(!fixture
        .meeting_dir
        .join(".processing/gigastt-result.json")
        .exists());
    assert_eq!(
        fs::read_to_string(fixture.model_dir.join("fake-cancelled-jobs"))
            .unwrap()
            .trim(),
        "job_1"
    );
    assert!(matches!(
        states.lock().unwrap().last(),
        Some(PostTranscriptionState::Failed { .. })
    ));
    fixture.sidecar.shutdown().await.unwrap();
}

#[tokio::test]
async fn local_cancellation_cancels_known_job_and_preserves_draft() {
    let fixture = Fixture::new(true).await;
    write_control(
        &fixture.model_dir,
        "fake-job-statuses",
        "processing:19,done:0",
    );
    let cancel = CancellationToken::new();
    let cancel_from_sink = cancel.clone();
    let states = Arc::new(Mutex::new(Vec::new()));
    let output = states.clone();

    let error = fixture
        .service()
        .run(fixture.request(), decoder(), cancel, move |state| {
            if state == (PostTranscriptionState::Transcribing { percent: 19 }) {
                cancel_from_sink.cancel();
            }
            output.lock().unwrap().push(state);
        })
        .await
        .unwrap_err();

    assert!(error.is_cancelled());
    assert!(matches!(error, PostTranscriptionError::Cancelled));
    assert_eq!(
        draft(&fixture.pool).await,
        ("draft survives".into(), "draft_live".into())
    );
    assert!(fixture.model_dir.join("fake-cancelled-jobs").exists());
    assert_eq!(
        states.lock().unwrap().last(),
        Some(&PostTranscriptionState::Cancelled)
    );
    fixture.sidecar.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_during_submission_waits_for_job_id_then_cancels_it() {
    let fixture = Fixture::new(true).await;
    write_control(&fixture.model_dir, "fake-job-response-await-release", "");
    let cancel = CancellationToken::new();
    let cancel_when_upload_arrives = cancel.clone();
    let model_dir = fixture.model_dir.clone();
    let trigger = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !model_dir.join("fake-job-request-arrived").exists() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            cancel_when_upload_arrives.cancel();
            write_control(&model_dir, "fake-job-response-release", "");
        })
        .await
        .unwrap();
    });

    let service = PostTranscriptionService::new(fixture.pool.clone(), fixture.sidecar.clone())
        .with_limits(
            Duration::from_millis(10),
            Duration::from_secs(5),
            Duration::from_secs(5),
        );
    let result = service
        .run(fixture.request(), decoder(), cancel, |_| {})
        .await;
    trigger.await.unwrap();
    let error = result.unwrap_err();

    assert!(
        matches!(error, PostTranscriptionError::Cancelled),
        "expected Cancelled after the submitted job id arrived, got {error:?}"
    );
    let requests = fs::read_to_string(fixture.model_dir.join("fake-requests")).unwrap();
    let lines: Vec<&str> = requests.lines().collect();
    let submit = lines
        .iter()
        .position(|line| line.starts_with("POST /v1/jobs?"))
        .unwrap();
    let cancel_request = lines
        .iter()
        .position(|line| line == &"DELETE /v1/jobs/job_1 0")
        .unwrap();
    assert!(submit < cancel_request);
    assert_eq!(draft(&fixture.pool).await.0, "draft survives");
    fixture.sidecar.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_before_submission_never_enqueues_a_remote_job() {
    let fixture = Fixture::new(true).await;
    let cancel = CancellationToken::new();
    let cancel_from_sink = cancel.clone();

    let error = fixture
        .service()
        .run(fixture.request(), decoder(), cancel, move |state| {
            if state == (PostTranscriptionState::Transcribing { percent: 0 }) {
                cancel_from_sink.cancel();
            }
        })
        .await
        .unwrap_err();

    assert!(matches!(error, PostTranscriptionError::Cancelled));
    let requests = fs::read_to_string(fixture.model_dir.join("fake-requests")).unwrap();
    assert!(!requests.lines().any(|line| line.starts_with("POST ")));
    assert_eq!(draft(&fixture.pool).await.0, "draft survives");
    fixture.sidecar.shutdown().await.unwrap();
}

#[tokio::test]
async fn unknown_submission_outcome_stops_owned_sidecar_without_importing() {
    let fixture = Fixture::new(true).await;
    write_control(&fixture.model_dir, "fake-job-response-delay-ms", "250");
    let service = PostTranscriptionService::new(fixture.pool.clone(), fixture.sidecar.clone())
        .with_limits(
            Duration::from_millis(10),
            Duration::from_millis(500),
            Duration::from_millis(60),
        );

    let error = service
        .run(
            fixture.request(),
            decoder(),
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        PostTranscriptionError::RequestTimeout {
            stage: "submission"
        }
    ));
    assert_eq!(fixture.sidecar.status().await, SidecarStatus::NotInstalled);
    assert!(!fixture.model_dir.join("fake-cancelled-jobs").exists());
    assert_eq!(draft(&fixture.pool).await.0, "draft survives");
}

#[tokio::test]
async fn database_rollback_keeps_valid_result_json_and_original_draft() {
    let fixture = Fixture::new(true).await;
    write_control(&fixture.model_dir, "fake-job-statuses", "done:0");
    sqlx::query(
        "CREATE TRIGGER reject_final BEFORE INSERT ON transcripts
         WHEN NEW.transcript = 'Финальный текст'
         BEGIN SELECT RAISE(FAIL, 'injected database failure'); END",
    )
    .execute(&fixture.pool)
    .await
    .unwrap();

    let error = fixture
        .service()
        .run(
            fixture.request(),
            decoder(),
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();

    assert!(matches!(error, PostTranscriptionError::Import(_)));
    assert!(!error.to_string().contains("Финальный текст"));
    assert_eq!(
        draft(&fixture.pool).await,
        ("draft survives".into(), "draft_live".into())
    );
    let result_path = fixture.meeting_dir.join(".processing/gigastt-result.json");
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(result_path).unwrap()).unwrap();
    assert_eq!(saved["text"], "Финальный текст");
    fixture.sidecar.shutdown().await.unwrap();
}

#[tokio::test]
async fn result_persistence_failure_cancels_known_job_and_keeps_draft() {
    let fixture = Fixture::new(true).await;
    write_control(&fixture.model_dir, "fake-job-statuses", "done:0");
    fs::create_dir_all(fixture.meeting_dir.join(".processing/gigastt-result.json")).unwrap();

    let error = fixture
        .service()
        .run(
            fixture.request(),
            decoder(),
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();

    assert!(matches!(error, PostTranscriptionError::ResultPersistence));
    assert_eq!(draft(&fixture.pool).await.0, "draft survives");
    assert!(
        fixture.model_dir.join("fake-cancelled-jobs").exists(),
        "a completed remote job remains owned until the local commit succeeds"
    );
    fixture.sidecar.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_meeting_is_rejected_before_decode_or_sidecar_start() {
    let fixture = Fixture::new(false).await;
    let decoded = Arc::new(AtomicBool::new(false));
    let called = decoded.clone();

    let error = fixture
        .service()
        .run(
            fixture.request(),
            Arc::new(move |_: &Path| {
                called.store(true, Ordering::SeqCst);
                Ok(vec![0.0; 100])
            }),
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();

    assert!(matches!(error, PostTranscriptionError::MeetingNotFound));
    assert!(!decoded.load(Ordering::SeqCst));
    assert!(!fixture.model_dir.join("fake-launch-count").exists());
}

#[tokio::test]
async fn foreign_meeting_folder_is_rejected_before_decode_or_sidecar_start() {
    let fixture = Fixture::new(true).await;
    let foreign = fixture._temp.path().join("foreign");
    fs::create_dir(&foreign).unwrap();
    let audio = foreign.join("audio.mp4");
    fs::write(&audio, b"foreign").unwrap();
    let mut request = fixture.request();
    request.meeting_dir = foreign;
    request.audio_path = audio;

    let error = fixture
        .service()
        .run(request, decoder(), CancellationToken::new(), |_| {})
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        PostTranscriptionError::MeetingFolderMismatch
    ));
    assert!(!fixture.model_dir.join("fake-launch-count").exists());
}

#[tokio::test]
async fn stalled_job_hits_deadline_cancels_remote_and_restores_ready() {
    let fixture = Fixture::new(true).await;
    write_control(&fixture.model_dir, "fake-job-statuses", "processing:9");
    let service = PostTranscriptionService::new(fixture.pool.clone(), fixture.sidecar.clone())
        .with_limits(
            Duration::from_millis(10),
            Duration::from_millis(55),
            Duration::from_millis(150),
        );

    let error = service
        .run(
            fixture.request(),
            decoder(),
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();

    assert!(matches!(error, PostTranscriptionError::PollDeadline));
    assert!(fixture.model_dir.join("fake-cancelled-jobs").exists());
    assert_eq!(draft(&fixture.pool).await.0, "draft survives");
    assert_eq!(fixture.sidecar.status().await, SidecarStatus::Ready);
    fixture.sidecar.shutdown().await.unwrap();
}

#[tokio::test]
async fn slow_status_request_is_clamped_to_remaining_job_deadline() {
    let fixture = Fixture::new(true).await;
    write_control(&fixture.model_dir, "fake-job-statuses", "done:0");
    write_control(&fixture.model_dir, "fake-status-response-delay-ms", "90");
    let service = PostTranscriptionService::new(fixture.pool.clone(), fixture.sidecar.clone())
        .with_limits(
            Duration::from_millis(10),
            Duration::from_millis(40),
            Duration::from_millis(150),
        );
    let error = service
        .run(
            fixture.request(),
            decoder(),
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();

    assert!(matches!(error, PostTranscriptionError::PollDeadline));
    assert!(fixture.model_dir.join("fake-cancelled-jobs").exists());
    assert_eq!(draft(&fixture.pool).await.0, "draft survives");
    fixture.sidecar.shutdown().await.unwrap();
}

#[tokio::test]
async fn remote_cancel_is_classified_as_cancellation() {
    let fixture = Fixture::new(true).await;
    write_control(&fixture.model_dir, "fake-job-statuses", "cancelled:0");

    let error = fixture
        .service()
        .run(
            fixture.request(),
            decoder(),
            CancellationToken::new(),
            |_| {},
        )
        .await
        .unwrap_err();

    assert!(matches!(error, PostTranscriptionError::RemoteCancelled));
    assert!(error.is_cancelled());
    fixture.sidecar.shutdown().await.unwrap();
}
