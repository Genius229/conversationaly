//! Database/filesystem identity check before touching meeting audio.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

#[derive(Debug)]
pub(crate) enum MeetingIdentityError {
    NotFound,
    Mismatch,
    Database(sqlx::Error),
}

pub(crate) async fn verify(
    pool: &SqlitePool,
    meeting_id: &str,
    supplied: &Path,
) -> Result<(), MeetingIdentityError> {
    let stored: Option<Option<String>> =
        sqlx::query_scalar("SELECT folder_path FROM meetings WHERE id = ?")
            .bind(meeting_id)
            .fetch_optional(pool)
            .await
            .map_err(MeetingIdentityError::Database)?;
    let stored = stored.ok_or(MeetingIdentityError::NotFound)?;
    let stored = stored
        .filter(|path| !path.trim().is_empty())
        .ok_or(MeetingIdentityError::Mismatch)?;
    let supplied = supplied.to_path_buf();
    let matches = tokio::task::spawn_blocking(move || {
        let expected = PathBuf::from(stored).canonicalize().ok()?;
        let supplied = supplied.canonicalize().ok()?;
        Some(expected == supplied)
    })
    .await
    .ok()
    .flatten()
    .unwrap_or(false);
    if matches {
        Ok(())
    } else {
        Err(MeetingIdentityError::Mismatch)
    }
}
