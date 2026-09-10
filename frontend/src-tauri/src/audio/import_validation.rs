//! Pure import validation shared by the desktop commands and codec contract tests.

use super::constants::AUDIO_EXTENSIONS;
use super::decoder::decode_audio_file;
use anyhow::{anyhow, Result};
use log::{debug, warn};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Maximum file size: 20GB (prevents OOM and excessive processing time)
const MAX_FILE_SIZE_BYTES: u64 = 20 * 1024 * 1024 * 1024; // 20GB

/// Information about a selected audio file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioFileInfo {
    pub path: String,
    pub filename: String,
    pub duration_seconds: f64,
    pub size_bytes: u64,
    pub format: String,
}

/// Validate an audio file and return its info using metadata-only approach
/// Falls back to full decode if metadata is unavailable
pub fn validate_audio_file(path: &Path) -> Result<AudioFileInfo> {
    // Check file exists
    if !path.exists() {
        return Err(anyhow!("File does not exist: {}", path.display()));
    }

    // Check extension
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if !AUDIO_EXTENSIONS.contains(&extension.as_str()) {
        return Err(anyhow!(
            "Unsupported format: .{}. Supported: {}",
            extension,
            AUDIO_EXTENSIONS.join(", ")
        ));
    }

    // Get file size
    let metadata = std::fs::metadata(path).map_err(|e| anyhow!("Cannot read file: {}", e))?;
    let size_bytes = metadata.len();

    // Check file size limit
    if size_bytes > MAX_FILE_SIZE_BYTES {
        return Err(anyhow!(
            "File too large: {:.2}GB. Maximum supported size is {}GB",
            size_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            MAX_FILE_SIZE_BYTES / (1024 * 1024 * 1024)
        ));
    }

    // Get filename without extension for title
    let filename = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Imported Audio")
        .to_string();

    // Try fast metadata-only validation first
    let duration_seconds = match extract_duration_from_metadata(path) {
        Ok(duration) => {
            debug!("Got duration from metadata: {:.2}s (fast path)", duration);
            duration
        }
        Err(e) => {
            // Fallback to full decode if metadata unavailable
            warn!(
                "Metadata extraction failed: {}, falling back to full decode",
                e
            );
            let decoded = decode_audio_file(path)?;
            decoded.duration_seconds
        }
    };

    Ok(AudioFileInfo {
        path: path.to_string_lossy().to_string(),
        filename,
        duration_seconds,
        size_bytes,
        format: extension.to_uppercase(),
    })
}

/// Extract duration from audio file metadata without full decode
/// Returns error if metadata is unavailable, triggering fallback to full decode
pub(super) fn extract_duration_from_metadata(path: &Path) -> Result<f64> {
    use symphonia::core::codecs::CodecParameters;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    // Open the file
    let file =
        std::fs::File::open(path).map_err(|e| anyhow!("Failed to open audio file: {}", e))?;

    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    // Set up format hint based on file extension
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    // Probe the file format (lightweight operation). 0.6 returns the
    // FormatReader directly and takes the options by value.
    let format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| anyhow!("Failed to probe audio format: {}", e))?;

    // Find the first audio track
    let track = format
        .tracks()
        .iter()
        .find(|t| {
            t.codec_params
                .as_ref()
                .is_some_and(CodecParameters::is_audio)
        })
        .ok_or_else(|| anyhow!("No audio track found in file"))?;

    let Some(CodecParameters::Audio(audio_params)) = track.codec_params.as_ref() else {
        return Err(anyhow!("No audio track found in file"));
    };

    // Extract duration from metadata
    let sample_rate = audio_params
        .sample_rate
        .ok_or_else(|| anyhow!("Unknown sample rate"))?;

    // n_frames moved off codec_params onto the Track itself in 0.6.
    let n_frames = track
        .num_frames
        .ok_or_else(|| anyhow!("Frame count not available in metadata"))?;

    let duration_seconds = n_frames as f64 / sample_rate as f64;

    debug!(
        "Extracted metadata: {}Hz, {} frames, {:.2}s",
        sample_rate, n_frames, duration_seconds
    );

    Ok(duration_seconds)
}
