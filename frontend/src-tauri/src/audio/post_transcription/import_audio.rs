//! Native import archive preparation before a GigaSTT job is accepted.
//!
//! The source is opened read-only and copied through a temporary file.  The
//! final `audio.<ext>` name is published only after the byte count and file
//! flush succeed, so retry discovery cannot mistake a partial copy for audio.

use std::{
    fs::{self, File, OpenOptions},
    future::Future,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use sqlx::{Acquire, SqlitePool};
use thiserror::Error;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const SUPPORTED_EXTENSIONS: &[&str] = &[
    "mp4", "m4a", "wav", "mp3", "flac", "ogg", "aac", "mkv", "webm", "wma",
];
const MAX_FILE_SIZE_BYTES: u64 = 20 * 1024 * 1024 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub struct ImportArchive {
    pub meeting_id: String,
    pub run_id: String,
    pub meeting_dir: PathBuf,
    pub audio_path: PathBuf,
    retain_on_drop: bool,
}

impl Drop for ImportArchive {
    fn drop(&mut self) {
        if !self.retain_on_drop {
            let _ = fs::remove_dir_all(&self.meeting_dir);
        }
    }
}

#[derive(Default)]
struct ImportWorkState {
    closing: bool,
    active: usize,
}

struct ImportWorkInner {
    state: Mutex<ImportWorkState>,
    shutdown: CancellationToken,
    drained: Notify,
}

impl Default for ImportWorkInner {
    fn default() -> Self {
        Self {
            state: Mutex::new(ImportWorkState::default()),
            shutdown: CancellationToken::new(),
            drained: Notify::new(),
        }
    }
}

/// Owns pre-accept validation/copy/database work across webview teardown.
#[derive(Clone, Default)]
pub struct ImportWorkRegistry {
    inner: Arc<ImportWorkInner>,
}

struct ImportWorkLeaseInner {
    registry: Arc<ImportWorkInner>,
}

/// Cloneable RAII proof that application shutdown must await this import.
#[derive(Clone)]
pub struct ImportWorkLease {
    inner: Arc<ImportWorkLeaseInner>,
}

impl ImportWorkRegistry {
    pub fn acquire(&self) -> Result<ImportWorkLease, ImportAudioError> {
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closing {
            return Err(ImportAudioError::ShuttingDown);
        }
        state.active = state
            .active
            .checked_add(1)
            .expect("import work count overflow");
        drop(state);
        Ok(ImportWorkLease {
            inner: Arc::new(ImportWorkLeaseInner {
                registry: self.inner.clone(),
            }),
        })
    }

    /// Spawn work whose lease is held by the runtime task, not by the IPC
    /// waiter. Dropping the returned handle detaches only the waiter; close
    /// still cancels and drains the task through its lease.
    pub fn spawn_owned<T, F, Fut>(
        &self,
        work: F,
    ) -> Result<tokio::task::JoinHandle<T>, ImportAudioError>
    where
        T: Send + 'static,
        F: FnOnce(ImportWorkLease) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
    {
        let lease = self.acquire()?;
        Ok(tokio::spawn(async move { work(lease).await }))
    }

    /// Reject new work, cooperatively cancel current work, then await all
    /// command/worker lease clones before database and sidecar shutdown.
    pub async fn close(&self) {
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.closing = true;
        }
        self.inner.shutdown.cancel();
        loop {
            let drained = self.inner.drained.notified();
            tokio::pin!(drained);
            // Register before checking the mutex-protected condition so the
            // final Drop cannot broadcast in the check-to-await gap.
            drained.as_mut().enable();
            if self
                .inner
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .active
                == 0
            {
                return;
            }
            drained.await;
        }
    }
}

impl ImportWorkLease {
    pub fn cancellation_token(&self) -> CancellationToken {
        self.inner.registry.shutdown.clone()
    }

    pub fn check_active(&self) -> Result<(), ImportAudioError> {
        if self.inner.registry.shutdown.is_cancelled() {
            Err(ImportAudioError::Cancelled)
        } else {
            Ok(())
        }
    }
}

impl Drop for ImportWorkLeaseInner {
    fn drop(&mut self) {
        let mut state = self
            .registry
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        state.active = state.active.saturating_sub(1);
        let drained = state.active == 0;
        drop(state);
        if drained {
            self.registry.drained.notify_waiters();
        }
    }
}

#[derive(Debug, Error)]
pub enum ImportAudioError {
    #[error("application is shutting down")]
    ShuttingDown,
    #[error("audio import cancelled during application shutdown")]
    Cancelled,
    #[error("import title is empty")]
    EmptyTitle,
    #[error("import audio duration is invalid")]
    InvalidDuration,
    #[error("source audio does not exist")]
    MissingSource,
    #[error("source audio is not a regular file")]
    SourceNotFile,
    #[error("unsupported audio format: .{0}")]
    UnsupportedFormat(String),
    #[error("source audio exceeds the 20GB import limit")]
    SourceTooLarge,
    #[error("could not {stage} imported audio: {source}")]
    Io {
        stage: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("imported audio copy was incomplete")]
    IncompleteCopy,
    #[error("meeting archive path cannot be stored in the database")]
    NonUtf8ArchivePath,
    #[error("could not encode import metadata")]
    Metadata(#[source] serde_json::Error),
    #[error("could not persist imported meeting")]
    Database(#[source] sqlx::Error),
    #[error("imported meeting commit outcome is unknown")]
    CommitOutcomeUnknown(#[source] sqlx::Error),
}

/// Build a unique meeting archive without modifying the selected source file.
pub fn create_import_archive(
    base_dir: &Path,
    title: &str,
    source: &Path,
    duration_seconds: f64,
    cancel: &CancellationToken,
) -> Result<ImportArchive, ImportAudioError> {
    check_cancelled(cancel)?;
    let title = title.trim();
    if title.is_empty() {
        return Err(ImportAudioError::EmptyTitle);
    }
    if !duration_seconds.is_finite() || duration_seconds <= 0.0 {
        return Err(ImportAudioError::InvalidDuration);
    }
    let source_metadata = match fs::metadata(source) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(ImportAudioError::MissingSource)
        }
        Err(source) => {
            return Err(ImportAudioError::Io {
                stage: "inspect source",
                source,
            })
        }
    };
    if !source_metadata.is_file() {
        return Err(ImportAudioError::SourceNotFile);
    }
    if source_metadata.len() > MAX_FILE_SIZE_BYTES {
        return Err(ImportAudioError::SourceTooLarge);
    }
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if !SUPPORTED_EXTENSIONS.contains(&extension.as_str()) {
        return Err(ImportAudioError::UnsupportedFormat(extension));
    }

    check_cancelled(cancel)?;
    fs::create_dir_all(base_dir).map_err(|source| ImportAudioError::Io {
        stage: "create recordings folder",
        source,
    })?;
    let meeting_id = format!("meeting-{}", Uuid::new_v4());
    let run_id = Uuid::new_v4().to_string();
    let timestamp = chrono::Utc::now().format("%Y-%m-%d_%H-%M-%S");
    let folder_name = format!(
        "{}_{}_{}",
        archive_title(title),
        timestamp,
        &meeting_id["meeting-".len()..]
    );
    let meeting_dir = base_dir.join(folder_name);
    fs::create_dir(&meeting_dir).map_err(|source| ImportAudioError::Io {
        stage: "create unique meeting archive",
        source,
    })?;

    let result = copy_into_archive(
        &meeting_dir,
        source,
        source_metadata.len(),
        &extension,
        cancel,
    )
    .and_then(|audio_path| {
        write_archive_metadata(
            &meeting_dir,
            &meeting_id,
            title,
            &extension,
            duration_seconds,
            cancel,
        )?;
        Ok(audio_path)
    })
    .and_then(|audio_path| {
        let meeting_dir = meeting_dir
            .canonicalize()
            .map_err(|source| ImportAudioError::Io {
                stage: "resolve meeting archive",
                source,
            })?;
        let audio_path = audio_path
            .canonicalize()
            .map_err(|source| ImportAudioError::Io {
                stage: "resolve archived audio",
                source,
            })?;
        Ok((meeting_dir, audio_path))
    });
    match result {
        Ok((meeting_dir, audio_path)) => Ok(ImportArchive {
            meeting_id,
            run_id,
            meeting_dir,
            audio_path,
            retain_on_drop: false,
        }),
        Err(error) => {
            let _ = fs::remove_dir_all(&meeting_dir);
            Err(error)
        }
    }
}

fn copy_into_archive(
    meeting_dir: &Path,
    source: &Path,
    source_len: u64,
    extension: &str,
    cancel: &CancellationToken,
) -> Result<PathBuf, ImportAudioError> {
    let audio_filename = format!("audio.{extension}");
    let audio_path = meeting_dir.join(&audio_filename);
    let staged_audio = meeting_dir.join(format!(".{audio_filename}.copying"));
    let mut input = File::open(source).map_err(|source| ImportAudioError::Io {
        stage: "open source",
        source,
    })?;
    atomic_publish_copy(&mut input, &staged_audio, &audio_path, source_len, cancel)?;
    let current_source_len = input
        .metadata()
        .map_err(|source| ImportAudioError::Io {
            stage: "verify source",
            source,
        })?
        .len();
    let archived_len = fs::metadata(&audio_path)
        .map_err(|source| ImportAudioError::Io {
            stage: "verify archive copy",
            source,
        })?
        .len();
    if archived_len != source_len || current_source_len != source_len {
        return Err(ImportAudioError::IncompleteCopy);
    }

    Ok(audio_path)
}

fn write_archive_metadata(
    meeting_dir: &Path,
    meeting_id: &str,
    title: &str,
    extension: &str,
    duration_seconds: f64,
    cancel: &CancellationToken,
) -> Result<(), ImportAudioError> {
    check_cancelled(cancel)?;
    let audio_filename = format!("audio.{extension}");
    let metadata = serde_json::to_vec_pretty(&serde_json::json!({
        "version": "1.0",
        "meeting_id": meeting_id,
        "meeting_name": title,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "duration_seconds": duration_seconds,
        "audio_file": audio_filename,
        "transcript_file": "transcripts.json",
        "status": "completed",
        "source": "import",
        "transcript_authority": "pending_gigastt"
    }))
    .map_err(ImportAudioError::Metadata)?;
    let staged_metadata = meeting_dir.join(".metadata.json.copying");
    let metadata_path = meeting_dir.join("metadata.json");
    let mut file = create_new(&staged_metadata, "stage import metadata")?;
    file.write_all(&metadata)
        .and_then(|_| file.flush())
        .and_then(|_| file.sync_all())
        .map_err(|source| ImportAudioError::Io {
            stage: "write import metadata",
            source,
        })?;
    drop(file);
    check_cancelled(cancel)?;
    fs::rename(staged_metadata, metadata_path).map_err(|source| ImportAudioError::Io {
        stage: "publish import metadata",
        source,
    })?;
    Ok(())
}

fn atomic_publish_copy<R: Read>(
    input: &mut R,
    staged: &Path,
    published: &Path,
    expected_bytes: u64,
    cancel: &CancellationToken,
) -> Result<u64, ImportAudioError> {
    check_cancelled(cancel)?;
    let mut output = create_new(staged, "stage archive copy")?;
    let result = (|| {
        let mut copied = 0_u64;
        let mut buffer = vec![0_u8; 1024 * 1024];
        loop {
            check_cancelled(cancel)?;
            let read = input
                .read(&mut buffer)
                .map_err(|source| ImportAudioError::Io {
                    stage: "read source during archive copy",
                    source,
                })?;
            if read == 0 {
                break;
            }
            check_cancelled(cancel)?;
            output
                .write_all(&buffer[..read])
                .map_err(|source| ImportAudioError::Io {
                    stage: "write archive copy",
                    source,
                })?;
            copied = copied
                .checked_add(read as u64)
                .ok_or(ImportAudioError::IncompleteCopy)?;
        }
        if copied != expected_bytes {
            return Err(ImportAudioError::IncompleteCopy);
        }
        check_cancelled(cancel)?;
        output
            .flush()
            .and_then(|_| output.sync_all())
            .map_err(|source| ImportAudioError::Io {
                stage: "flush archive copy",
                source,
            })?;
        check_cancelled(cancel)?;
        Ok(copied)
    })();
    drop(output);
    let copied = match result {
        Ok(copied) => copied,
        Err(error) => {
            let _ = fs::remove_file(staged);
            return Err(error);
        }
    };
    if let Err(source) = fs::rename(staged, published) {
        let _ = fs::remove_file(staged);
        return Err(ImportAudioError::Io {
            stage: "publish archive copy",
            source,
        });
    }
    Ok(copied)
}

fn check_cancelled(cancel: &CancellationToken) -> Result<(), ImportAudioError> {
    if cancel.is_cancelled() {
        Err(ImportAudioError::Cancelled)
    } else {
        Ok(())
    }
}

fn create_new(path: &Path, stage: &'static str) -> Result<File, ImportAudioError> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| ImportAudioError::Io { stage, source })
}

fn archive_title(title: &str) -> String {
    let sanitized: String = title
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            value if value.is_control() => '_',
            value => value,
        })
        .take(80)
        .collect();
    sanitized.trim().to_string()
}

/// Commit the imported meeting and its canonical-summary gate together.
pub async fn persist_imported_meeting(
    pool: &SqlitePool,
    title: &str,
    archive: &mut ImportArchive,
) -> Result<(), ImportAudioError> {
    let expected_dir =
        archive
            .meeting_dir
            .canonicalize()
            .map_err(|source| ImportAudioError::Io {
                stage: "verify meeting archive",
                source,
            })?;
    let audio_path = archive
        .audio_path
        .canonicalize()
        .map_err(|source| ImportAudioError::Io {
            stage: "verify archived audio",
            source,
        })?;
    if audio_path.parent() != Some(expected_dir.as_path()) || !audio_path.is_file() {
        return Err(ImportAudioError::IncompleteCopy);
    }
    let stored_dir = expected_dir
        .to_str()
        .ok_or(ImportAudioError::NonUtf8ArchivePath)?;

    let now = chrono::Utc::now().to_rfc3339();
    let mut connection = pool.acquire().await.map_err(ImportAudioError::Database)?;
    let mut transaction = connection
        .begin()
        .await
        .map_err(ImportAudioError::Database)?;
    sqlx::query(
        "INSERT INTO meetings (id,title,created_at,updated_at,folder_path) VALUES (?,?,?,?,?)",
    )
    .bind(&archive.meeting_id)
    .bind(title.trim())
    .bind(&now)
    .bind(&now)
    .bind(stored_dir)
    .execute(&mut *transaction)
    .await
    .map_err(ImportAudioError::Database)?;
    sqlx::query(
        "INSERT INTO meeting_post_transcription_jobs (meeting_id,run_id,status,requires_final,error_code,updated_at) VALUES (?,?,'running',1,NULL,?)",
    )
    .bind(&archive.meeting_id)
    .bind(&archive.run_id)
    .bind(&now)
    .execute(&mut *transaction)
    .await
    .map_err(ImportAudioError::Database)?;
    // From this point an error, panic, or lost IPC waiter cannot prove that
    // SQLite did not commit. Preserve the archive conservatively.
    archive.retain_on_drop = true;
    transaction
        .commit()
        .await
        .map_err(ImportAudioError::CommitOutcomeUnknown)?;
    Ok(())
}

/// Used only before a meeting row has committed. Never remove retry audio.
pub fn discard_unpersisted_archive(archive: &ImportArchive) -> Result<(), io::Error> {
    if archive.retain_on_drop {
        Ok(())
    } else {
        fs::remove_dir_all(&archive.meeting_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::sync::{mpsc, Arc, Condvar, Mutex};
    use std::time::Duration;

    struct GatedReader {
        started: Option<mpsc::Sender<()>>,
        gate: Arc<(Mutex<bool>, Condvar)>,
    }

    impl Read for GatedReader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if let Some(started) = self.started.take() {
                let _ = started.send(());
                let (lock, signal) = &*self.gate;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = signal.wait(released).unwrap();
                }
                buffer[..4].copy_from_slice(b"data");
                return Ok(4);
            }
            Ok(0)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_cancels_partial_copy_and_waits_for_worker_owned_lease() {
        let registry = ImportWorkRegistry::default();
        let lease = registry.acquire().unwrap();
        let cancel = lease.cancellation_token();
        let worker_lease = lease.clone();
        drop(lease);
        let temp = tempfile::tempdir().unwrap();
        let staged = temp.path().join(".audio.wav.copying");
        let published = temp.path().join("audio.wav");
        let (started_tx, started_rx) = mpsc::channel();
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let worker_gate = gate.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let _lease = worker_lease;
            let mut reader = GatedReader {
                started: Some(started_tx),
                gate: worker_gate,
            };
            atomic_publish_copy(&mut reader, &staged, &published, 4, &cancel)
                .map(|_| (staged, published))
        });
        tokio::task::spawn_blocking(move || started_rx.recv().unwrap())
            .await
            .unwrap();

        let mut close = Box::pin(registry.close());
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut close)
            .await
            .is_err());
        assert!(registry.acquire().is_err());

        let (lock, signal) = &*gate;
        *lock.lock().unwrap() = true;
        signal.notify_all();
        close.await;
        let error = worker.await.unwrap().unwrap_err();
        assert!(matches!(error, ImportAudioError::Cancelled));
        assert!(!temp.path().join(".audio.wav.copying").exists());
        assert!(!temp.path().join("audio.wav").exists());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_invoke_waiter_at_commit_keeps_owned_task_and_archived_audio() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE meetings(id TEXT PRIMARY KEY,title TEXT NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,folder_path TEXT);\
             CREATE TABLE meeting_post_transcription_jobs(meeting_id TEXT PRIMARY KEY REFERENCES meetings(id),run_id TEXT NOT NULL,status TEXT NOT NULL,requires_final INTEGER NOT NULL,error_code TEXT,updated_at TEXT NOT NULL);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.wav");
        fs::write(&source, b"owned archived audio").unwrap();
        let base = temp.path().join("recordings");
        let registry = ImportWorkRegistry::default();
        let commit_release = Arc::new(tokio::sync::Notify::new());
        let task_commit_release = commit_release.clone();
        let release = Arc::new(tokio::sync::Notify::new());
        let task_release = release.clone();
        let task_pool = pool.clone();
        let (pre_commit_tx, pre_commit_rx) = tokio::sync::oneshot::channel();
        let (committed_tx, committed_rx) = tokio::sync::oneshot::channel();
        let waiter = registry
            .spawn_owned(move |lease| async move {
                let cancel = lease.cancellation_token();
                let mut archive =
                    create_import_archive(&base, "Owned", &source, 1.0, &cancel).unwrap();
                let _ = pre_commit_tx.send(());
                task_commit_release.notified().await;
                persist_imported_meeting(&task_pool, "Owned", &mut archive)
                    .await
                    .unwrap();
                let identity = (archive.meeting_id.clone(), archive.audio_path.clone());
                let _ = committed_tx.send(identity);
                // Deterministic commit boundary: the IPC waiter is gone, but
                // the registry-owned task and archive remain until released.
                task_release.notified().await;
            })
            .unwrap();
        pre_commit_rx.await.unwrap();
        drop(waiter);
        commit_release.notify_one();

        let (meeting_id, audio_path) = committed_rx.await.unwrap();
        assert!(audio_path.is_file());
        let stored: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meetings WHERE id=?")
            .bind(&meeting_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(stored, 1);
        let mut close = Box::pin(registry.close());
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut close)
            .await
            .is_err());
        release.notify_one();
        close.await;
        assert!(audio_path.is_file());
        pool.close().await;
    }

    #[tokio::test]
    async fn final_lease_release_wakes_every_concurrent_close_waiter() {
        let registry = ImportWorkRegistry::default();
        let lease = registry.acquire().unwrap();
        let first_registry = registry.clone();
        let second_registry = registry.clone();
        let mut first = Box::pin(first_registry.close());
        let mut second = Box::pin(second_registry.close());
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut first)
            .await
            .is_err());
        assert!(tokio::time::timeout(Duration::from_millis(10), &mut second)
            .await
            .is_err());

        drop(lease);

        tokio::time::timeout(Duration::from_millis(100), async {
            tokio::join!(first, second);
        })
        .await
        .expect("both close waiters must observe the final lease release");
    }

    #[tokio::test]
    async fn unknown_commit_outcome_conservatively_retains_archived_audio() {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::raw_sql(
            "PRAGMA foreign_keys=ON;\
             CREATE TABLE parents(id TEXT PRIMARY KEY);\
             CREATE TABLE meetings(\
               id TEXT PRIMARY KEY,title TEXT NOT NULL,created_at TEXT NOT NULL,updated_at TEXT NOT NULL,folder_path TEXT,\
               parent_id TEXT NOT NULL DEFAULT 'missing' REFERENCES parents(id) DEFERRABLE INITIALLY DEFERRED\
             );\
             CREATE TABLE meeting_post_transcription_jobs(meeting_id TEXT PRIMARY KEY REFERENCES meetings(id),run_id TEXT NOT NULL,status TEXT NOT NULL,requires_final INTEGER NOT NULL,error_code TEXT,updated_at TEXT NOT NULL);",
        )
        .execute(&pool)
        .await
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.wav");
        fs::write(&source, b"retain when commit is uncertain").unwrap();
        let mut archive = create_import_archive(
            &temp.path().join("recordings"),
            "Uncertain",
            &source,
            1.0,
            &CancellationToken::new(),
        )
        .unwrap();
        let audio_path = archive.audio_path.clone();

        let error = persist_imported_meeting(&pool, "Uncertain", &mut archive)
            .await
            .unwrap_err();
        assert!(matches!(error, ImportAudioError::CommitOutcomeUnknown(_)));
        drop(archive);

        assert!(audio_path.is_file());
        let meetings: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM meetings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(meetings, 0);
        pool.close().await;
    }
}
