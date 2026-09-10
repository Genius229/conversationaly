use app_lib::audio::post_transcription::{
    importer::{map_result, replace_transcript, ImportError},
    GigasttResult, Segment, WordInfo,
};
use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};
use tokio_util::sync::CancellationToken;

async fn migrated_pool() -> SqlitePool {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await
        .unwrap();
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let migrations_dir = if manifest_dir.ends_with("gigastt-contract-tests") {
        manifest_dir.join("../../frontend/src-tauri/migrations")
    } else {
        manifest_dir.join("migrations")
    };
    sqlx::migrate::Migrator::new(migrations_dir)
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    pool
}

async fn insert_meeting(pool: &SqlitePool, meeting_id: &str) {
    sqlx::query(
        "INSERT INTO meetings (id, title, created_at, updated_at) VALUES (?, 'Test', ?, ?)",
    )
    .bind(meeting_id)
    .bind("2026-09-10T00:00:00Z")
    .bind("2026-09-10T00:00:00Z")
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_draft(pool: &SqlitePool, meeting_id: &str) {
    sqlx::query(
        "INSERT INTO transcripts
         (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker)
         VALUES ('draft-row', ?, 'draft survives', '2026-09-10T00:00:00Z', 0.0, 1.0, 1.0, 'mic')",
    )
    .bind(meeting_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO meeting_transcript_metadata
         (meeting_id, provenance, result_metadata, updated_at)
         VALUES (?, 'draft_live', '{\"draft\":true}', '2026-09-10T00:00:00Z')",
    )
    .bind(meeting_id)
    .execute(pool)
    .await
    .unwrap();
}

fn valid_result() -> GigasttResult {
    GigasttResult {
        text: "Первый второй".to_string(),
        duration: 3.5,
        confidence: Some(0.91),
        words: vec![
            WordInfo {
                word: "Первый".to_string(),
                start: 0.25,
                end: 1.0,
                confidence: Some(0.9),
                speaker: Some(0),
            },
            WordInfo {
                word: "второй".to_string(),
                start: 2.0,
                end: 3.25,
                confidence: Some(0.8),
                speaker: Some(1),
            },
        ],
        segments: Some(vec![
            Segment {
                start: 0.25,
                end: 1.0,
                text: "Первый".to_string(),
                words: vec![WordInfo {
                    word: "Первый".to_string(),
                    start: 0.25,
                    end: 1.0,
                    confidence: Some(0.9),
                    speaker: Some(0),
                }],
                speaker: Some(0),
            },
            Segment {
                start: 2.0,
                end: 3.25,
                text: "второй".to_string(),
                words: vec![WordInfo {
                    word: "второй".to_string(),
                    start: 2.0,
                    end: 3.25,
                    confidence: Some(0.8),
                    speaker: Some(1),
                }],
                speaker: None,
            },
        ]),
    }
}

#[test]
fn maps_segment_seconds_speakers_and_stable_ids() {
    let result = valid_result();
    let first = map_result("meeting-a", &result).unwrap();
    let repeated = map_result("meeting-a", &result).unwrap();

    assert_eq!(first.len(), 2);
    assert_eq!(first[0].audio_start_time, 0.25);
    assert_eq!(first[0].audio_end_time, 1.0);
    assert_eq!(first[0].duration, 0.75);
    assert_eq!(first[0].speaker.as_deref(), Some("speaker_0"));
    assert_eq!(first[1].speaker.as_deref(), Some("speaker_1"));
    assert_eq!(
        first.iter().map(|row| &row.id).collect::<Vec<_>>(),
        repeated.iter().map(|row| &row.id).collect::<Vec<_>>()
    );
}

#[test]
fn preserves_zero_duration_source_timestamps() {
    let mut result = valid_result();
    result.words[0].end = result.words[0].start;
    let segment = &mut result.segments.as_mut().unwrap()[0];
    segment.end = segment.start;
    segment.words[0].end = segment.words[0].start;

    let rows = map_result("meeting-a", &result).unwrap();

    assert_eq!(rows[0].audio_start_time, 0.25);
    assert_eq!(rows[0].audio_end_time, 0.25);
    assert_eq!(rows[0].duration, 0.0);
}

#[test]
fn fallback_preserves_authoritative_text_when_word_tokens_are_raw() {
    let mut result = valid_result();
    result.segments = None;
    result.text = "25 рублей.".to_string();
    result.words = vec![
        WordInfo {
            word: "двадцать".to_string(),
            start: 0.25,
            end: 0.75,
            confidence: Some(0.9),
            speaker: Some(1),
        },
        WordInfo {
            word: "пять".to_string(),
            start: 0.8,
            end: 1.1,
            confidence: Some(0.9),
            speaker: Some(1),
        },
        WordInfo {
            word: "рублей".to_string(),
            start: 1.2,
            end: 2.0,
            confidence: Some(0.9),
            speaker: Some(1),
        },
    ];

    let rows = map_result("meeting-a", &result).unwrap();

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].transcript, "25 рублей.");
    assert_eq!(
        (rows[0].audio_start_time, rows[0].audio_end_time),
        (0.25, 2.0)
    );
    assert_eq!(rows[0].speaker.as_deref(), Some("speaker_1"));
}

#[test]
fn rejects_empty_and_mismatched_results_without_echoing_text() {
    let empty = GigasttResult {
        text: "  ".to_string(),
        duration: 0.0,
        confidence: None,
        words: vec![],
        segments: None,
    };
    assert!(matches!(
        map_result("meeting-a", &empty),
        Err(ImportError::InvalidResult { .. })
    ));

    let mut malformed = valid_result();
    malformed.segments.as_mut().unwrap()[1].end = 4.0;
    let error = map_result("meeting-a", &malformed).unwrap_err();
    assert!(matches!(error, ImportError::InvalidResult { .. }));
    assert!(!error.to_string().contains("второй"));

    let mut incomplete_segment = valid_result();
    incomplete_segment.segments.as_mut().unwrap()[0]
        .words
        .clear();
    assert!(matches!(
        map_result("meeting-a", &incomplete_segment),
        Err(ImportError::InvalidResult { .. })
    ));
}

#[tokio::test]
async fn atomically_replaces_draft_and_records_result_metadata() {
    let pool = migrated_pool().await;
    insert_meeting(&pool, "meeting-a").await;
    insert_draft(&pool, "meeting-a").await;

    let inserted = replace_transcript(
        &pool,
        "meeting-a",
        &valid_result(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(inserted, 2);
    let rows = sqlx::query(
        "SELECT transcript, audio_start_time, audio_end_time, speaker
         FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time",
    )
    .bind("meeting-a")
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].get::<String, _>("transcript"), "Первый");
    assert_eq!(rows[1].get::<String, _>("speaker"), "speaker_1");

    let (provenance, metadata): (String, String) = sqlx::query_as(
        "SELECT provenance, result_metadata FROM meeting_transcript_metadata WHERE meeting_id = ?",
    )
    .bind("meeting-a")
    .fetch_one(&pool)
    .await
    .unwrap();
    let metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
    assert_eq!(provenance, "final_gigastt");
    assert!((metadata["confidence"].as_f64().unwrap() - 0.91).abs() < 0.000_001);
    assert!((metadata["words"][0]["confidence"].as_f64().unwrap() - 0.9).abs() < 0.000_001);
    assert_eq!(metadata["words"][1]["speaker"], 1);
}

#[tokio::test]
async fn metadata_preserves_canonical_text_when_segments_differ() {
    let pool = migrated_pool().await;
    insert_meeting(&pool, "meeting-a").await;
    let mut result = valid_result();
    result.text = "25 рублей.".to_string();

    replace_transcript(&pool, "meeting-a", &result, &CancellationToken::new())
        .await
        .unwrap();

    let metadata: String = sqlx::query_scalar(
        "SELECT result_metadata FROM meeting_transcript_metadata WHERE meeting_id = ?",
    )
    .bind("meeting-a")
    .fetch_one(&pool)
    .await
    .unwrap();
    let metadata: serde_json::Value = serde_json::from_str(&metadata).unwrap();
    assert_eq!(metadata["text"], "25 рублей.");
}

#[tokio::test]
async fn validation_or_cancellation_preserves_draft_and_metadata() {
    let pool = migrated_pool().await;
    insert_meeting(&pool, "meeting-a").await;
    insert_draft(&pool, "meeting-a").await;
    let mut invalid = valid_result();
    invalid.words[0].confidence = Some(1.5);

    assert!(matches!(
        replace_transcript(&pool, "meeting-a", &invalid, &CancellationToken::new()).await,
        Err(ImportError::InvalidResult { .. })
    ));
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        replace_transcript(&pool, "meeting-a", &valid_result(), &cancelled).await,
        Err(ImportError::Cancelled)
    ));

    let draft: String =
        sqlx::query_scalar("SELECT transcript FROM transcripts WHERE meeting_id = ?")
            .bind("meeting-a")
            .fetch_one(&pool)
            .await
            .unwrap();
    let provenance: String = sqlx::query_scalar(
        "SELECT provenance FROM meeting_transcript_metadata WHERE meeting_id = ?",
    )
    .bind("meeting-a")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(draft, "draft survives");
    assert_eq!(provenance, "draft_live");
}

#[tokio::test]
async fn sql_failure_mid_insert_rolls_back_rows_and_provenance() {
    let pool = migrated_pool().await;
    insert_meeting(&pool, "meeting-a").await;
    insert_draft(&pool, "meeting-a").await;
    sqlx::query(
        "CREATE TRIGGER reject_second_replacement BEFORE INSERT ON transcripts
         WHEN NEW.transcript = 'второй'
         BEGIN SELECT RAISE(ABORT, 'synthetic insert failure'); END",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert!(matches!(
        replace_transcript(
            &pool,
            "meeting-a",
            &valid_result(),
            &CancellationToken::new()
        )
        .await,
        Err(ImportError::Database(_))
    ));

    let rows: Vec<String> =
        sqlx::query_scalar("SELECT transcript FROM transcripts WHERE meeting_id = ? ORDER BY id")
            .bind("meeting-a")
            .fetch_all(&pool)
            .await
            .unwrap();
    let provenance: String = sqlx::query_scalar(
        "SELECT provenance FROM meeting_transcript_metadata WHERE meeting_id = ?",
    )
    .bind("meeting-a")
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(rows, vec!["draft survives"]);
    assert_eq!(provenance, "draft_live");
}

#[tokio::test]
async fn rejects_missing_meeting_and_metadata_obeys_foreign_key() {
    let pool = migrated_pool().await;
    assert!(matches!(
        replace_transcript(&pool, "missing", &valid_result(), &CancellationToken::new()).await,
        Err(ImportError::MeetingNotFound)
    ));

    insert_meeting(&pool, "meeting-a").await;
    replace_transcript(
        &pool,
        "meeting-a",
        &valid_result(),
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    sqlx::query("DELETE FROM meetings WHERE id = ?")
        .bind("meeting-a")
        .execute(&pool)
        .await
        .unwrap();
    let transcript_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM transcripts")
        .fetch_one(&pool)
        .await
        .unwrap();
    let metadata_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM meeting_transcript_metadata")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((transcript_count, metadata_count), (0, 0));
}

#[tokio::test]
async fn repeated_import_reuses_deterministic_row_ids() {
    let pool = migrated_pool().await;
    insert_meeting(&pool, "meeting-a").await;
    let result = valid_result();

    replace_transcript(&pool, "meeting-a", &result, &CancellationToken::new())
        .await
        .unwrap();
    let first: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time",
    )
    .bind("meeting-a")
    .fetch_all(&pool)
    .await
    .unwrap();
    replace_transcript(&pool, "meeting-a", &result, &CancellationToken::new())
        .await
        .unwrap();
    let second: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM transcripts WHERE meeting_id = ? ORDER BY audio_start_time",
    )
    .bind("meeting-a")
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(first, second);
}
