//! Keep the existing meeting-folder transcript exports in step with SQLite.
use sqlx::SqlitePool;
use std::{io::Write, path::PathBuf};

pub async fn mirror_transcript(pool: &SqlitePool, meeting_id: &str) -> Result<(), String> {
    let (folder, title): (Option<String>, String) =
        sqlx::query_as("SELECT folder_path,title FROM meetings WHERE id=?")
            .bind(meeting_id)
            .fetch_one(pool)
            .await
            .map_err(|_| "could not read export location")?;
    let folder = PathBuf::from(folder.ok_or("missing export location")?);
    let rows: Vec<crate::database::models::Transcript> =
        sqlx::query_as("SELECT * FROM transcripts WHERE meeting_id=? ORDER BY audio_start_time,rowid")
            .bind(meeting_id)
            .fetch_all(pool)
            .await
            .map_err(|_| "could not read transcript for export")?;
    let segments: Vec<crate::api::TranscriptSegment> = rows
        .into_iter()
        .map(|row| crate::api::TranscriptSegment {
            id: row.id,
            text: row.transcript,
            timestamp: row.timestamp,
            audio_start_time: row.audio_start_time,
            audio_end_time: row.audio_end_time,
            duration: row.duration,
            speaker: row.speaker,
        })
        .collect();
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        crate::audio::common::write_transcripts_json(&folder, &segments)
            .map_err(|_| "could not update transcripts.json")?;
        crate::audio::common::write_transcript_md(
            &folder,
            Some(&title),
            &crate::audio::common::markdown_segments(&segments),
        )
        .map_err(|_| "could not update transcript.md")?;
        let path = folder.join("metadata.json");
        if path.is_file() {
            let mut metadata: serde_json::Value = serde_json::from_slice(
                &std::fs::read(&path).map_err(|_| "could not read recording metadata")?,
            )
            .map_err(|_| "invalid recording metadata")?;
            let object = metadata
                .as_object_mut()
                .ok_or("invalid recording metadata")?;
            object.remove("detected_summary_language");
            object.insert("transcript_authority".into(), "final_gigastt".into());
            object.insert(
                "gigastt_version".into(),
                super::gigastt_sidecar::PINNED_GIGASTT_VERSION.into(),
            );
            // Audio identity/duration/status are deliberately unchanged.
            let bytes =
                serde_json::to_vec_pretty(&metadata).map_err(|_| "could not encode metadata")?;
            let mut temp =
                tempfile::NamedTempFile::new_in(&folder).map_err(|_| "could not stage metadata")?;
            temp.write_all(&bytes)
                .and_then(|_| temp.as_file().sync_all())
                .map_err(|_| "could not write metadata")?;
            temp.persist(path)
                .map_err(|_| "could not replace metadata")?;
        }
        Ok(())
    })
    .await
    .map_err(|_| "export worker failed")?
}
