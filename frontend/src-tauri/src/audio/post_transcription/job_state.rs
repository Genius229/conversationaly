//! Durable processing state and canonical summary authority (no UI dependency).
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

/// Called inside the alternative provider's transcript replacement transaction.
pub async fn clear_for_alternative(
    connection: &mut sqlx::SqliteConnection,
    meeting: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM meeting_transcript_metadata WHERE meeting_id=?")
        .bind(meeting)
        .execute(&mut *connection)
        .await?;
    sqlx::query("DELETE FROM meeting_post_transcription_jobs WHERE meeting_id=?")
        .bind(meeting)
        .execute(connection)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct PersistedJob {
    pub meeting_id: String,
    pub run_id: String,
    pub status: String,
    pub requires_final: bool,
    pub error_code: Option<String>,
}

pub async fn begin_job(
    pool: &SqlitePool,
    meeting: &str,
    run: &str,
    requires_final: bool,
) -> Result<(), String> {
    if meeting.is_empty() || run.is_empty() {
        return Err("invalid transcription job identity".into());
    }
    let result = sqlx::query("INSERT INTO meeting_post_transcription_jobs (meeting_id,run_id,status,requires_final,error_code,updated_at) VALUES (?,?,'running',?,NULL,?) ON CONFLICT(meeting_id) DO UPDATE SET run_id=excluded.run_id,status='running',requires_final=excluded.requires_final,error_code=NULL,updated_at=excluded.updated_at WHERE meeting_post_transcription_jobs.status != 'running'")
        .bind(meeting).bind(run).bind(requires_final).bind(chrono::Utc::now().to_rfc3339())
        .execute(pool).await.map_err(|_| "could not persist transcription job")?;
    if result.rows_affected() == 0 {
        return Err("meeting already has a pending transcription".into());
    }
    Ok(())
}
pub async fn finish_job(
    pool: &SqlitePool,
    meeting: &str,
    run: &str,
    status: &str,
    code: Option<&str>,
) -> Result<(), String> {
    if !["ready", "failed", "cancelled"].contains(&status) {
        return Err("invalid terminal transcription status".into());
    }
    let result = sqlx::query("UPDATE meeting_post_transcription_jobs SET status=?,error_code=?,updated_at=? WHERE meeting_id=? AND run_id=? AND status='running'")
        .bind(status).bind(code).bind(chrono::Utc::now().to_rfc3339()).bind(meeting).bind(run)
        .execute(pool).await.map_err(|_| "could not persist transcription outcome")?;
    if result.rows_affected() != 1 {
        return Err("stale transcription outcome".into());
    }
    Ok(())
}
/// Called once after DB startup, before any new native jobs can start.
pub async fn recover_interrupted(pool: &SqlitePool) -> Result<u64, String> {
    sqlx::query("UPDATE meeting_post_transcription_jobs SET status='interrupted',error_code='app_interrupted',updated_at=? WHERE status='running'")
        .bind(chrono::Utc::now().to_rfc3339()).execute(pool).await.map(|r|r.rows_affected())
        .map_err(|_| "could not recover transcription jobs".into())
}
pub async fn get_job(pool: &SqlitePool, meeting: &str) -> Result<Option<PersistedJob>, String> {
    sqlx::query_as("SELECT meeting_id,run_id,status,requires_final,error_code FROM meeting_post_transcription_jobs WHERE meeting_id=?")
        .bind(meeting).fetch_optional(pool).await.map_err(|_| "could not read transcription state".into())
}
/// Backend authority gate; the frontend's supplied text is not authoritative
/// for a GigaSTT-final meeting. No process/chunk rows may be created before this.
pub async fn summary_text(
    pool: &SqlitePool,
    meeting: &str,
    provided: &str,
    allow_draft: bool,
) -> Result<String, String> {
    // Read authority and optional speaker rows from one consistent snapshot.
    let mut tx = pool
        .begin()
        .await
        .map_err(|_| "could not read transcript authority")?;
    let job: Option<PersistedJob> = sqlx::query_as("SELECT meeting_id,run_id,status,requires_final,error_code FROM meeting_post_transcription_jobs WHERE meeting_id=?")
        .bind(meeting).fetch_optional(&mut *tx).await.map_err(|_| "could not read transcription state")?;
    if let Some(job) = &job {
        if job.status == "running" {
            return Err("GigaSTT final transcription is still running".into());
        }
        if job.requires_final && job.status != "ready" && !allow_draft {
            return Err("GigaSTT final transcript unavailable; retry or explicitly summarize the current transcript".into());
        }
    }
    let metadata: Option<Option<String>> = sqlx::query_scalar("SELECT result_metadata FROM meeting_transcript_metadata WHERE meeting_id=? AND provenance='final_gigastt'")
        .bind(meeting).fetch_optional(&mut *tx).await.map_err(|_| "could not read transcript authority")?;
    if let Some(metadata) = metadata {
        let value: serde_json::Value = serde_json::from_str(metadata.as_deref().unwrap_or(""))
            .map_err(|_| "invalid canonical transcript metadata")?;
        let mut text = value
            .get("text")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .ok_or("canonical GigaSTT text is missing")?;
        let annotations: Vec<(Option<f64>,Option<f64>,String,String)> = sqlx::query_as(
            "SELECT t.audio_start_time,t.audio_end_time,COALESCE(s.name,t.speaker),t.transcript FROM transcripts t LEFT JOIN speaker_names s ON s.meeting_id=t.meeting_id AND s.speaker=t.speaker WHERE t.meeting_id=? AND t.speaker IS NOT NULL ORDER BY t.audio_start_time,t.rowid"
        ).bind(meeting).fetch_all(&mut *tx).await.map_err(|_| "could not read speaker annotations")?;
        if !annotations.is_empty() {
            text.push_str("\n\nSpeaker annotations (recording-relative seconds; canonical transcript above is authoritative):\n");
            for (start, end, speaker, segment) in annotations {
                let start = start.unwrap_or(0.0);
                let speaker = speaker.replace(['\r', '\n'], " ");
                text.push_str(&format!(
                    "[{start:.2}-{:.2}] {speaker}: {segment}\n",
                    end.unwrap_or(start)
                ));
            }
        }
        tx.commit()
            .await
            .map_err(|_| "could not finish transcript read")?;
        return Ok(text);
    }
    if job.as_ref().is_some_and(|j| j.status == "ready") {
        return Err("canonical GigaSTT text is missing".into());
    }
    tx.commit()
        .await
        .map_err(|_| "could not finish transcript read")?;
    Ok(provided.to_owned())
}
