//! Validated, atomic import of a completed GigaSTT result.

use super::types::{GigasttResult, WordInfo};
use serde_json::json;
use sqlx::{Acquire, SqlitePool};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const TIMESTAMP_EPSILON_SECONDS: f64 = 0.001;

#[derive(Debug, Clone, PartialEq)]
pub struct ImportedRow {
    pub id: String,
    pub meeting_id: String,
    pub transcript: String,
    pub timestamp: String,
    pub audio_start_time: f64,
    pub audio_end_time: f64,
    pub duration: f64,
    pub speaker: Option<String>,
}

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("invalid GigaSTT result field: {field}")]
    InvalidResult { field: &'static str },
    #[error("meeting does not exist")]
    MeetingNotFound,
    #[error("transcript import cancelled")]
    Cancelled,
    #[error("result metadata could not be encoded")]
    Metadata(#[from] serde_json::Error),
    #[error("database operation failed")]
    Database(#[from] sqlx::Error),
}

/// Validate a complete result and map it to the existing `transcripts` schema.
pub fn map_result(
    meeting_id: &str,
    result: &GigasttResult,
) -> Result<Vec<ImportedRow>, ImportError> {
    validate_result(meeting_id, result)?;

    let encoded_result = serde_json::to_vec(result)?;
    let meeting_namespace = Uuid::new_v5(&Uuid::NAMESPACE_OID, meeting_id.as_bytes());
    let result_namespace = Uuid::new_v5(&meeting_namespace, &encoded_result);
    let timestamp = chrono::Utc::now().to_rfc3339();
    let rows = match result
        .segments
        .as_ref()
        .filter(|segments| !segments.is_empty())
    {
        Some(segments) => segments
            .iter()
            .enumerate()
            .map(|(index, segment)| ImportedRow {
                id: stable_row_id(&result_namespace, index),
                meeting_id: meeting_id.to_string(),
                transcript: segment.text.trim().to_string(),
                timestamp: timestamp.clone(),
                audio_start_time: segment.start,
                audio_end_time: segment.end,
                duration: segment.end - segment.start,
                speaker: segment
                    .speaker
                    .or_else(|| unanimous_speaker(&segment.words))
                    .map(speaker_label),
            })
            .collect(),
        None => {
            let first_word = result.words.first().expect("validated non-empty words");
            let last_word = result.words.last().expect("validated non-empty words");
            vec![ImportedRow {
                id: stable_row_id(&result_namespace, 0),
                meeting_id: meeting_id.to_string(),
                transcript: result.text.trim().to_string(),
                timestamp,
                audio_start_time: first_word.start,
                audio_end_time: last_word.end,
                duration: last_word.end - first_word.start,
                speaker: unanimous_speaker(&result.words).map(speaker_label),
            }]
        }
    };

    Ok(rows)
}

/// Replace the visible transcript and its authority metadata in one transaction.
pub async fn replace_transcript(
    pool: &SqlitePool,
    meeting_id: &str,
    result: &GigasttResult,
    cancel: &CancellationToken,
) -> Result<usize, ImportError> {
    let rows = map_result(meeting_id, result)?;
    let result_metadata = serde_json::to_string(&json!({
        "text": &result.text,
        "duration_seconds": result.duration,
        "confidence": result.confidence,
        "words": &result.words,
        "segments": &result.segments,
    }))?;

    check_cancelled(cancel)?;
    let mut connection = pool.acquire().await?;
    let mut transaction = connection.begin().await?;
    let meeting_exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM meetings WHERE id = ?")
        .bind(meeting_id)
        .fetch_optional(&mut *transaction)
        .await?;
    if meeting_exists.is_none() {
        return Err(ImportError::MeetingNotFound);
    }

    // This is the first destructive statement.  Everything below remains in
    // the same transaction, and every early return therefore restores the old
    // draft/final rows and metadata.
    check_cancelled(cancel)?;
    sqlx::query("DELETE FROM transcripts WHERE meeting_id = ?")
        .bind(meeting_id)
        .execute(&mut *transaction)
        .await?;

    for row in &rows {
        check_cancelled(cancel)?;
        sqlx::query(
            "INSERT INTO transcripts
             (id, meeting_id, transcript, timestamp, audio_start_time, audio_end_time, duration, speaker)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&row.id)
        .bind(&row.meeting_id)
        .bind(&row.transcript)
        .bind(&row.timestamp)
        .bind(row.audio_start_time)
        .bind(row.audio_end_time)
        .bind(row.duration)
        .bind(&row.speaker)
        .execute(&mut *transaction)
        .await?;
    }

    check_cancelled(cancel)?;
    sqlx::query(
        "INSERT INTO meeting_transcript_metadata
         (meeting_id, provenance, result_metadata, updated_at)
         VALUES (?, 'final_gigastt', ?, ?)
         ON CONFLICT(meeting_id) DO UPDATE SET
             provenance = excluded.provenance,
             result_metadata = excluded.result_metadata,
             updated_at = excluded.updated_at",
    )
    .bind(meeting_id)
    .bind(result_metadata)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&mut *transaction)
    .await?;

    check_cancelled(cancel)?;
    transaction.commit().await?;
    Ok(rows.len())
}

fn validate_result(meeting_id: &str, result: &GigasttResult) -> Result<(), ImportError> {
    if meeting_id.trim().is_empty() {
        return invalid("meeting_id");
    }
    if result.text.trim().is_empty() {
        return invalid("text");
    }
    if !result.duration.is_finite() || result.duration <= 0.0 {
        return invalid("duration");
    }
    validate_confidence(result.confidence, "confidence")?;
    validate_words(&result.words, result.duration, None)?;

    if let Some(segments) = result
        .segments
        .as_ref()
        .filter(|segments| !segments.is_empty())
    {
        let mut previous_start = 0.0;
        for (index, segment) in segments.iter().enumerate() {
            validate_interval(
                segment.start,
                segment.end,
                result.duration,
                "segment_timing",
            )?;
            if index > 0 && segment.start < previous_start {
                return invalid("segment_order");
            }
            if segment.text.trim().is_empty() {
                return invalid("segment_text");
            }
            if segment.words.is_empty() {
                return invalid("segment_words");
            }
            validate_words(
                &segment.words,
                result.duration,
                Some((segment.start, segment.end)),
            )?;
            previous_start = segment.start;
        }
    } else if result.words.is_empty() {
        return invalid("timings");
    }

    Ok(())
}

fn validate_words(
    words: &[WordInfo],
    result_duration: f64,
    segment_bounds: Option<(f64, f64)>,
) -> Result<(), ImportError> {
    let mut previous_start = 0.0;
    for (index, word) in words.iter().enumerate() {
        if word.word.trim().is_empty() {
            return invalid("word_text");
        }
        validate_interval(word.start, word.end, result_duration, "word_timing")?;
        if index > 0 && word.start < previous_start {
            return invalid("word_order");
        }
        if let Some((segment_start, segment_end)) = segment_bounds {
            if word.start + TIMESTAMP_EPSILON_SECONDS < segment_start
                || word.end > segment_end + TIMESTAMP_EPSILON_SECONDS
            {
                return invalid("word_segment_timing");
            }
        }
        validate_confidence(word.confidence, "word_confidence")?;
        previous_start = word.start;
    }
    Ok(())
}

fn validate_interval(
    start: f64,
    end: f64,
    result_duration: f64,
    field: &'static str,
) -> Result<(), ImportError> {
    if !start.is_finite()
        || !end.is_finite()
        || start < 0.0
        || end < start
        || end > result_duration + TIMESTAMP_EPSILON_SECONDS
    {
        return invalid(field);
    }
    Ok(())
}

fn validate_confidence(confidence: Option<f32>, field: &'static str) -> Result<(), ImportError> {
    if confidence.is_some_and(|value| !value.is_finite() || !(0.0..=1.0).contains(&value)) {
        return invalid(field);
    }
    Ok(())
}

fn invalid<T>(field: &'static str) -> Result<T, ImportError> {
    Err(ImportError::InvalidResult { field })
}

fn stable_row_id(result_namespace: &Uuid, index: usize) -> String {
    format!(
        "transcript-{}",
        Uuid::new_v5(result_namespace, &(index as u64).to_be_bytes())
    )
}

fn unanimous_speaker(words: &[WordInfo]) -> Option<u32> {
    let speaker = words.first()?.speaker?;
    words
        .iter()
        .all(|word| word.speaker == Some(speaker))
        .then_some(speaker)
}

fn speaker_label(speaker: u32) -> String {
    format!("speaker_{speaker}")
}

fn check_cancelled(cancel: &CancellationToken) -> Result<(), ImportError> {
    if cancel.is_cancelled() {
        Err(ImportError::Cancelled)
    } else {
        Ok(())
    }
}
