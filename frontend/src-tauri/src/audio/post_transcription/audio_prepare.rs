//! Preparation boundary; production supplies the existing recording decoder.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, thiserror::Error)]
pub enum AudioPrepareError {
    #[error("audio preparation cancelled")]
    Cancelled,
    #[error("audio decoder failed")]
    DecodeFailed,
    #[error("invalid meeting audio location")]
    InvalidLocation,
    #[error("audio preparation I/O failure: {0}")]
    Io(#[from] std::io::Error),
    #[error("audio preparation worker failed")]
    WorkerFailed,
}

pub struct PreparedAudio {
    pub path: PathBuf,
    pub duration_seconds: f64,
}

pub async fn prepare_audio<D>(
    input: PathBuf,
    meeting_dir: PathBuf,
    decoder: Arc<D>,
    cancel: CancellationToken,
) -> Result<PreparedAudio, AudioPrepareError>
where
    D: Fn(&Path) -> Result<Vec<f32>, String> + Send + Sync + 'static,
{
    // The existing decoder is blocking and not cooperatively cancellable. Own
    // and await that worker; cancellation is checked before and after decode,
    // so a cancelled preparation can never submit a job or import a result.
    tokio::task::spawn_blocking(move || {
        if cancel.is_cancelled() {
            return Err(AudioPrepareError::Cancelled);
        }
        let root = meeting_dir.canonicalize()?;
        let input = input.canonicalize()?;
        if input.parent() != Some(root.as_path()) || !input.is_file() {
            return Err(AudioPrepareError::InvalidLocation);
        }
        let processing = root.join(".processing");
        match std::fs::symlink_metadata(&processing) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err(AudioPrepareError::InvalidLocation);
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
        let samples = decoder(&input).map_err(|_| AudioPrepareError::DecodeFailed)?;
        if cancel.is_cancelled() {
            return Err(AudioPrepareError::Cancelled);
        }
        std::fs::create_dir_all(&processing)?;
        let path = processing.join("gigastt-input.wav");
        super::wav::write_pcm16_wav(&path, &samples)?;
        if cancel.is_cancelled() {
            return Err(AudioPrepareError::Cancelled);
        }
        Ok(PreparedAudio {
            path,
            duration_seconds: samples.len() as f64 / 16000.0,
        })
    })
    .await
    .map_err(|_| AudioPrepareError::WorkerFailed)?
}
