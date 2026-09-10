//! Adapter to the recorder's existing decoding/resampling implementation.
use super::audio_prepare::{prepare_audio, AudioPrepareError, PreparedAudio};
use std::{path::PathBuf, sync::Arc};
use tokio_util::sync::CancellationToken;

pub async fn prepare_meeting_audio(
    input: PathBuf,
    meeting_dir: PathBuf,
    cancel: CancellationToken,
) -> Result<PreparedAudio, AudioPrepareError> {
    prepare_audio(
        input,
        meeting_dir,
        Arc::new(|path: &std::path::Path| {
            crate::audio::decoder::decode_audio_file(path)
                .map(|decoded| decoded.to_whisper_format())
                .map_err(|_| "audio decoding failed".into())
        }),
        cancel,
    )
    .await
}
