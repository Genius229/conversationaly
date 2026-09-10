use app_lib::audio::post_transcription::job_state::*;
use sqlx::{sqlite::SqlitePoolOptions, SqlitePool};

async fn db() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::raw_sql(
        "CREATE TABLE meetings(id TEXT PRIMARY KEY); INSERT INTO meetings VALUES ('meeting');",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/20260910000000_add_transcript_provenance.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/20260910000001_add_post_transcription_jobs.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql("CREATE TABLE transcripts(meeting_id TEXT,transcript TEXT,speaker TEXT,audio_start_time REAL,audio_end_time REAL); CREATE TABLE speaker_names(meeting_id TEXT,speaker TEXT,name TEXT);").execute(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn canonical_summary_preserves_database_speaker_annotations_without_trusting_client_text() {
    let pool = db().await;
    sqlx::query(
        "INSERT INTO meeting_transcript_metadata VALUES ('meeting','final_gigastt',?, 'now')",
    )
    .bind(r#"{"text":"25 рублей."}"#)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::raw_sql("INSERT INTO transcripts VALUES ('meeting','25 рублей.','speaker_0',0,2); INSERT INTO speaker_names VALUES ('meeting','speaker_0','Иван');").execute(&pool).await.unwrap();
    let text = summary_text(&pool, "meeting", "stale private draft", false)
        .await
        .unwrap();
    assert!(text.starts_with("25 рублей."));
    assert!(text.contains("Иван: 25 рублей."));
    assert!(!text.contains("stale private draft"));
    pool.close().await;
}

#[tokio::test]
async fn summary_waits_for_final_and_only_explicitly_uses_failed_draft() {
    let pool = db().await;
    assert_eq!(
        summary_text(&pool, "meeting", "draft", false)
            .await
            .unwrap(),
        "draft"
    );
    begin_job(&pool, "meeting", "run1", true).await.unwrap();
    assert!(summary_text(&pool, "meeting", "draft", false)
        .await
        .is_err());
    assert!(summary_text(&pool, "meeting", "draft", true).await.is_err());
    finish_job(&pool, "meeting", "run1", "failed", Some("job_failed"))
        .await
        .unwrap();
    assert!(summary_text(&pool, "meeting", "draft", false)
        .await
        .is_err());
    assert_eq!(
        summary_text(&pool, "meeting", "draft", true).await.unwrap(),
        "draft"
    );
    pool.close().await;
}

#[tokio::test]
async fn finished_gigastt_summary_uses_canonical_text_not_client_draft() {
    let pool = db().await;
    begin_job(&pool, "meeting", "run1", true).await.unwrap();
    sqlx::query(
        "INSERT INTO meeting_transcript_metadata VALUES ('meeting','final_gigastt',?, 'now')",
    )
    .bind(r#"{"text":"25 рублей."}"#)
    .execute(&pool)
    .await
    .unwrap();
    finish_job(&pool, "meeting", "run1", "ready", None)
        .await
        .unwrap();
    assert_eq!(
        summary_text(&pool, "meeting", "двадцать пять рублей", false)
            .await
            .unwrap(),
        "25 рублей."
    );
    pool.close().await;
}

#[tokio::test]
async fn stale_finish_cannot_close_retry_and_restart_blocks_auto_summary() {
    let pool = db().await;
    begin_job(&pool, "meeting", "old", true).await.unwrap();
    finish_job(&pool, "meeting", "old", "failed", None)
        .await
        .unwrap();
    begin_job(&pool, "meeting", "new", true).await.unwrap();
    assert!(finish_job(&pool, "meeting", "old", "ready", None)
        .await
        .is_err());
    assert_eq!(
        get_job(&pool, "meeting").await.unwrap().unwrap().status,
        "running"
    );
    assert_eq!(recover_interrupted(&pool).await.unwrap(), 1);
    assert_eq!(
        get_job(&pool, "meeting").await.unwrap().unwrap().status,
        "interrupted"
    );
    assert!(summary_text(&pool, "meeting", "draft", false)
        .await
        .is_err());
    pool.close().await;
}

#[tokio::test]
async fn duplicate_start_and_missing_or_corrupt_final_text_fail_closed() {
    let pool = db().await;
    begin_job(&pool, "meeting", "one", true).await.unwrap();
    assert!(begin_job(&pool, "meeting", "two", true).await.is_err());
    assert_eq!(
        get_job(&pool, "meeting").await.unwrap().unwrap().run_id,
        "one"
    );
    finish_job(&pool, "meeting", "one", "ready", None)
        .await
        .unwrap();
    assert!(summary_text(&pool, "meeting", "stale draft", false)
        .await
        .is_err());
    sqlx::query(
        "INSERT INTO meeting_transcript_metadata VALUES ('meeting','final_gigastt','{}','now')",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert!(summary_text(&pool, "meeting", "stale draft", true)
        .await
        .is_err());
    pool.close().await;
}

#[tokio::test]
async fn alternative_provider_clears_authority_only_with_successful_transaction() {
    let pool = db().await;
    begin_job(&pool, "meeting", "old", true).await.unwrap();
    finish_job(&pool, "meeting", "old", "ready", None)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO meeting_transcript_metadata VALUES ('meeting','final_gigastt',?, 'now')",
    )
    .bind(r#"{"text":"Old final"}"#)
    .execute(&pool)
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    clear_for_alternative(&mut tx, "meeting").await.unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(
        summary_text(&pool, "meeting", "new alternative", false)
            .await
            .unwrap(),
        "Old final"
    );
    let mut tx = pool.begin().await.unwrap();
    clear_for_alternative(&mut tx, "meeting").await.unwrap();
    tx.commit().await.unwrap();
    assert!(get_job(&pool, "meeting").await.unwrap().is_none());
    assert_eq!(
        summary_text(&pool, "meeting", "new alternative", false)
            .await
            .unwrap(),
        "new alternative"
    );
    pool.close().await;
}
