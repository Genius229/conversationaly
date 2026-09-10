use std::{fs, path::Path};

use app_lib::audio::post_transcription::{
    import_audio::{create_import_archive, persist_imported_meeting},
    job_state::{get_job, summary_text},
};
use sqlx::SqlitePool;
use tokio_util::sync::CancellationToken;

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
    let migrations_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../frontend/src-tauri/migrations");
    sqlx::migrate::Migrator::new(migrations_dir)
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    pool
}

#[test]
fn archive_copy_is_atomic_unique_and_does_not_modify_source() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("recordings");
    let source = temp.path().join("source.WAV");
    let original = b"source audio stays immutable";
    fs::write(&source, original).unwrap();

    let first = create_import_archive(
        &base,
        "Review / call",
        &source,
        12.5,
        &CancellationToken::new(),
    )
    .unwrap();
    let second = create_import_archive(
        &base,
        "Review / call",
        &source,
        12.5,
        &CancellationToken::new(),
    )
    .unwrap();

    assert_ne!(first.meeting_dir, second.meeting_dir);
    assert_eq!(first.audio_path.file_name().unwrap(), "audio.wav");
    assert_eq!(fs::read(&first.audio_path).unwrap(), original);
    assert_eq!(fs::read(&second.audio_path).unwrap(), original);
    assert_eq!(fs::read(&source).unwrap(), original);
    assert!(!first.meeting_dir.join(".audio.wav.copying").exists());

    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(first.meeting_dir.join("metadata.json")).unwrap())
            .unwrap();
    assert_eq!(metadata["meeting_id"], first.meeting_id);
    assert_eq!(metadata["audio_file"], "audio.wav");
    assert_eq!(metadata["duration_seconds"], 12.5);
    assert_eq!(metadata["source"], "import");
    assert_eq!(metadata["transcript_authority"], "pending_gigastt");
}

#[test]
fn invalid_source_never_leaves_an_archive_folder() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("recordings");
    let unsupported = temp.path().join("notes.txt");
    fs::write(&unsupported, b"not audio").unwrap();

    let error = create_import_archive(&base, "Notes", &unsupported, 1.0, &CancellationToken::new())
        .unwrap_err();

    assert!(error.to_string().contains("unsupported audio format"));
    assert!(!base.exists());
}

#[test]
fn unreadable_source_never_publishes_a_partial_archive() {
    let temp = tempfile::tempdir().unwrap();
    let base = temp.path().join("recordings");
    let directory_named_wav = temp.path().join("directory.wav");
    fs::create_dir(&directory_named_wav).unwrap();

    let error = create_import_archive(
        &base,
        "Broken source",
        &directory_named_wav,
        1.0,
        &CancellationToken::new(),
    )
    .unwrap_err();

    assert!(error.to_string().contains("regular file"));
    assert!(!base.exists());
}

#[test]
fn dropping_unpersisted_archive_cleans_completed_copy_after_invoke_cancellation() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.wav");
    fs::write(&source, b"complete but not accepted").unwrap();
    let meeting_dir = {
        let archive = create_import_archive(
            temp.path(),
            "Cancelled invoke",
            &source,
            1.0,
            &CancellationToken::new(),
        )
        .unwrap();
        assert!(archive.audio_path.is_file());
        archive.meeting_dir.clone()
    };

    assert!(!meeting_dir.exists());
}

#[tokio::test]
async fn persisted_import_reserves_final_authority_with_readable_audio() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.mp3");
    fs::write(&source, b"archived bytes").unwrap();
    let mut archive = create_import_archive(
        temp.path(),
        "Imported meeting",
        &source,
        3.0,
        &CancellationToken::new(),
    )
    .unwrap();
    let pool = migrated_pool().await;

    persist_imported_meeting(&pool, "Imported meeting", &mut archive)
        .await
        .unwrap();

    let row: (String, String, Option<String>) = sqlx::query_as(
        "SELECT title, folder_path, (SELECT status FROM meeting_post_transcription_jobs WHERE meeting_id=meetings.id) FROM meetings WHERE id=?",
    )
    .bind(&archive.meeting_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, "Imported meeting");
    assert_eq!(
        Path::new(&row.1).canonicalize().unwrap(),
        archive.meeting_dir
    );
    assert_eq!(row.2.as_deref(), Some("running"));
    let job = get_job(&pool, &archive.meeting_id).await.unwrap().unwrap();
    assert_eq!(job.run_id, archive.run_id);
    assert!(job.requires_final);
    assert!(
        summary_text(&pool, &archive.meeting_id, "legacy draft", true)
            .await
            .is_err()
    );
    assert_eq!(fs::read(&archive.audio_path).unwrap(), b"archived bytes");
}
