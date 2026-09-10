//! Atomic result persistence and disposable processing-file cleanup.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use super::types::GigasttResult;

#[derive(Debug)]
pub(crate) struct ArtifactError;

pub(crate) async fn persist_result(
    prepared_path: &Path,
    result: &GigasttResult,
) -> Result<PathBuf, ArtifactError> {
    let parent = prepared_path.parent().ok_or(ArtifactError)?.to_path_buf();
    let target = parent.join("gigastt-result.json");
    let bytes = serde_json::to_vec(result).map_err(|_| ArtifactError)?;
    let output = target.clone();
    tokio::task::spawn_blocking(move || -> Result<(), ArtifactError> {
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|_| ArtifactError)?;
        temporary.write_all(&bytes).map_err(|_| ArtifactError)?;
        temporary.flush().map_err(|_| ArtifactError)?;
        temporary.as_file().sync_all().map_err(|_| ArtifactError)?;
        temporary.persist(&output).map_err(|_| ArtifactError)?;
        Ok(())
    })
    .await
    .map_err(|_| ArtifactError)??;
    Ok(target)
}

pub(crate) async fn cleanup_file(path: &Path) -> bool {
    match tokio::fs::remove_file(path).await {
        Ok(()) => false,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}
