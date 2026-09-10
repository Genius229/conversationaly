use anyhow::{anyhow, Result};
use log::{info, warn};
use std::path::Path;
use std::process::{Command, Stdio};

use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

use super::super::ffmpeg::find_existing_ffmpeg_path;

/// Extensions which keep their existing direct FFmpeg path because Symphonia
/// does not provide the required demuxer or codec support.
const FFMPEG_ONLY_EXTENSIONS: &[&str] = &["mkv", "webm", "wma"];

/// Progress callback for long-running operations.
pub type ProgressCallback = Box<dyn Fn(u32, &str) + Send>;

/// Decoded audio in its source sample rate and channel layout.
#[derive(Debug, Clone)]
pub struct DecodedAudio {
    /// Raw audio samples, interleaved when more than one channel is present.
    pub samples: Vec<f32>,
    /// Source sample rate.
    pub sample_rate: u32,
    /// Source channel count.
    pub channels: u16,
    /// Duration derived from decoded frames.
    pub duration_seconds: f64,
}

/// Decode an audio file to source-rate/source-channel floating-point samples.
pub fn decode_audio_file(path: &Path) -> Result<DecodedAudio> {
    decode_audio_file_with_progress(path, None)
}

/// Decode with an optional callback while retaining callback ownership for a
/// possible one-shot FFmpeg fallback.
pub fn decode_audio_file_with_progress(
    path: &Path,
    progress_callback: Option<ProgressCallback>,
) -> Result<DecodedAudio> {
    info!("Decoding audio file");
    let callback = progress_callback.as_ref();

    decode_once_with_fallback(
        needs_ffmpeg_conversion(path),
        || decode_with_symphonia(path, callback),
        || decode_via_ffmpeg(path, callback),
    )
}

fn decode_once_with_fallback(
    force_ffmpeg: bool,
    symphonia_decode: impl FnOnce() -> Result<DecodedAudio>,
    ffmpeg_decode: impl FnOnce() -> Result<DecodedAudio>,
) -> Result<DecodedAudio> {
    if force_ffmpeg {
        return ffmpeg_decode().map_err(|error| anyhow!("FFmpeg audio decode failed: {error}"));
    }

    match symphonia_decode() {
        Ok(decoded) => Ok(decoded),
        Err(primary_error) => {
            warn!("Built-in audio decode failed; trying one FFmpeg fallback");
            ffmpeg_decode().map_err(|fallback_error| {
                anyhow!(
                    "Audio decode failed with both decoders (built-in: {primary_error}; FFmpeg: {fallback_error})"
                )
            })
        }
    }
}

fn decode_via_ffmpeg(
    input_path: &Path,
    progress_callback: Option<&ProgressCallback>,
) -> Result<DecodedAudio> {
    let temporary_wav = convert_to_wav_with_ffmpeg(input_path, progress_callback)?;
    decode_with_symphonia(temporary_wav.as_ref(), progress_callback)
        .map_err(|error| anyhow!("Converted WAV could not be decoded: {error}"))
}

fn needs_ffmpeg_conversion(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| FFMPEG_ONLY_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn temporary_wav() -> Result<tempfile::TempPath> {
    tempfile::Builder::new()
        .prefix(".conversationaly_decode_")
        .suffix(".wav")
        .tempfile()
        .map(tempfile::NamedTempFile::into_temp_path)
        .map_err(|error| anyhow!("Could not create a temporary audio file: {error}"))
}

fn configure_ffmpeg_command(command: &mut Command, input_path: &Path, output_path: &Path) {
    command
        .arg("-nostdin")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-protocol_whitelist")
        .arg("file")
        .arg("-i")
        .arg(input_path)
        .arg("-map")
        .arg("0:a:0")
        .arg("-vn")
        .arg("-c:a")
        .arg("pcm_s16le")
        // Deliberately omit -ac and -ar. This shared decoder returns the source
        // channel layout and rate; to_whisper_format owns mono/16 kHz conversion.
        .arg("-y")
        .arg(output_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
}

fn convert_to_wav_with_ffmpeg(
    input_path: &Path,
    progress_callback: Option<&ProgressCallback>,
) -> Result<tempfile::TempPath> {
    let ffmpeg_path = find_existing_ffmpeg_path()
        .ok_or_else(|| anyhow!("FFmpeg is not available to decode this audio format"))?;
    let temp_path = temporary_wav()?;

    if let Some(callback) = progress_callback {
        callback(0, "Preparing audio with FFmpeg...");
    }

    let mut command = Command::new(ffmpeg_path);
    configure_ffmpeg_command(&mut command, input_path, temp_path.as_ref());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let status = command
        .status()
        .map_err(|error| anyhow!("Could not run FFmpeg: {error}"))?;

    if !status.success() {
        let status = status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "terminated".to_string());
        return Err(anyhow!(
            "FFmpeg could not convert the audio (status {status})"
        ));
    }

    let output_size = std::fs::metadata(&temp_path)
        .map_err(|error| anyhow!("FFmpeg did not create its temporary audio output: {error}"))?
        .len();
    if output_size == 0 {
        return Err(anyhow!("FFmpeg produced an empty audio output"));
    }

    if let Some(callback) = progress_callback {
        callback(5, "Audio format prepared");
    }
    info!("FFmpeg audio conversion completed");
    Ok(temp_path)
}

fn decode_with_symphonia(
    path: &Path,
    progress_callback: Option<&ProgressCallback>,
) -> Result<DecodedAudio> {
    let file = std::fs::File::open(path)
        .map_err(|error| anyhow!("Could not open the audio input: {error}"))?;
    let media_source = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }

    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            media_source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|error| anyhow!("Could not probe the audio container: {error}"))?;
    let track = format
        .tracks()
        .iter()
        .find(|track| {
            track
                .codec_params
                .as_ref()
                .is_some_and(CodecParameters::is_audio)
        })
        .ok_or_else(|| anyhow!("The file contains no audio track"))?;
    let track_id = track.id;
    let track_num_frames = track.num_frames;
    let Some(CodecParameters::Audio(audio_params)) = track.codec_params.as_ref() else {
        return Err(anyhow!("The file contains no audio track"));
    };
    let audio_params = audio_params.clone();
    let sample_rate = audio_params
        .sample_rate
        .ok_or_else(|| anyhow!("The audio track has no sample rate"))?;
    let mut channels = audio_params
        .channels
        .as_ref()
        .map(|channels| channels.count() as u16)
        .unwrap_or(1);
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&audio_params, &AudioDecoderOptions::default())
        .map_err(|error| anyhow!("The audio codec is not supported: {error}"))?;
    let expected_samples = expected_sample_count(track_num_frames, channels);
    let mut last_progress = 0_u32;
    let mut all_samples = Vec::new();
    let mut chunk = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(error) => return Err(anyhow!("Could not read a complete audio packet: {error}")),
        };
        if packet.track_id != track_id {
            continue;
        }

        let decoded = decoder
            .decode(&packet)
            .map_err(|error| anyhow!("Could not decode a complete audio packet: {error}"))?;
        let actual_channels = decoded.spec().channels().count() as u16;
        if actual_channels == 0 {
            return Err(anyhow!("Decoded audio reported zero channels"));
        }
        channels = actual_channels;
        decoded.copy_to_vec_interleaved(&mut chunk);
        all_samples.extend_from_slice(&chunk);

        if let (Some(callback), Some(expected)) = (progress_callback, expected_samples) {
            if expected > 0 {
                let current_progress =
                    ((all_samples.len() as f64 / expected as f64) * 100.0) as u32;
                if current_progress >= last_progress + 10 && current_progress <= 100 {
                    last_progress = current_progress;
                    callback(
                        current_progress,
                        &format!("Decoding audio: {current_progress}%"),
                    );
                }
            }
        }
    }

    if all_samples.is_empty() {
        return Err(anyhow!("No audio samples were decoded"));
    }
    if all_samples.len() % channels as usize != 0 {
        return Err(anyhow!("Decoded audio ended with an incomplete frame"));
    }

    let total_frames = all_samples.len() / channels as usize;
    let duration_seconds = total_frames as f64 / sample_rate as f64;
    if let Some(callback) = progress_callback {
        callback(100, "Decoding complete");
    }
    info!(
        "Decoded {} samples ({:.2}s) at {}Hz with {} channel(s)",
        all_samples.len(),
        duration_seconds,
        sample_rate,
        channels
    );

    Ok(DecodedAudio {
        samples: all_samples,
        sample_rate,
        channels,
        duration_seconds,
    })
}

fn expected_sample_count(frames: Option<u64>, channels: u16) -> Option<usize> {
    let frames = usize::try_from(frames?).ok()?;
    frames.checked_mul(usize::from(channels))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::ffi::OsString;
    use std::io::Write;
    use std::path::Path;
    use std::process::Command;

    fn decoded() -> DecodedAudio {
        DecodedAudio {
            samples: vec![0.25, -0.25],
            sample_rate: 48_000,
            channels: 1,
            duration_seconds: 2.0 / 48_000.0,
        }
    }

    #[test]
    fn native_success_does_not_start_ffmpeg() {
        let fallback_calls = Cell::new(0);

        let result = decode_once_with_fallback(
            false,
            || Ok(decoded()),
            || {
                fallback_calls.set(fallback_calls.get() + 1);
                Ok(decoded())
            },
        )
        .unwrap();

        assert_eq!(result.samples.len(), 2);
        assert_eq!(fallback_calls.get(), 0);
    }

    #[test]
    fn any_native_failure_gets_exactly_one_ffmpeg_attempt() {
        let fallback_calls = Cell::new(0);

        let result = decode_once_with_fallback(
            false,
            || Err(anyhow!("packet decode failed after partial output")),
            || {
                fallback_calls.set(fallback_calls.get() + 1);
                Ok(decoded())
            },
        )
        .unwrap();

        assert_eq!(result.samples.len(), 2);
        assert_eq!(fallback_calls.get(), 1);
    }

    #[test]
    fn forced_formats_skip_symphonia_and_still_run_ffmpeg_once() {
        let native_calls = Cell::new(0);
        let fallback_calls = Cell::new(0);

        decode_once_with_fallback(
            true,
            || {
                native_calls.set(native_calls.get() + 1);
                Ok(decoded())
            },
            || {
                fallback_calls.set(fallback_calls.get() + 1);
                Ok(decoded())
            },
        )
        .unwrap();

        assert_eq!(native_calls.get(), 0);
        assert_eq!(fallback_calls.get(), 1);
    }

    #[test]
    fn only_existing_ffmpeg_only_extensions_skip_the_native_attempt() {
        for path in ["video.mkv", "audio.webm", "audio.wma", "meeting.MKV"] {
            assert!(needs_ffmpeg_conversion(Path::new(path)), "{path}");
        }
        for path in [
            "audio.mp4",
            "audio.wav",
            "audio.mp3",
            "audio.flac",
            "audio.ogg",
            "audio.aac",
            "audio.m4a",
            "noext",
        ] {
            assert!(!needs_ffmpeg_conversion(Path::new(path)), "{path}");
        }
    }

    #[test]
    fn malformed_frame_metadata_cannot_overflow_the_progress_estimate() {
        assert_eq!(expected_sample_count(Some(u64::MAX), 2), None);
        assert_eq!(expected_sample_count(Some(48_000), 2), Some(96_000));
        assert_eq!(expected_sample_count(None, 2), None);
    }

    #[test]
    fn ffmpeg_command_preserves_source_channels_and_rate() {
        let mut command = Command::new("ffmpeg");
        configure_ffmpeg_command(
            &mut command,
            Path::new("read only/source audio.m4a"),
            Path::new("system temp/output audio.wav"),
        );
        let args: Vec<OsString> = command.get_args().map(Into::into).collect();

        assert_eq!(
            args,
            [
                "-nostdin",
                "-hide_banner",
                "-loglevel",
                "error",
                "-protocol_whitelist",
                "file",
                "-i",
                "read only/source audio.m4a",
                "-map",
                "0:a:0",
                "-vn",
                "-c:a",
                "pcm_s16le",
                "-y",
                "system temp/output audio.wav",
            ]
            .map(OsString::from),
        );
        assert!(!args.iter().any(|arg| arg == "-ac" || arg == "-ar"));
    }

    #[test]
    fn fallback_wav_lives_in_os_temp_and_is_removed_on_drop() {
        let temp_path = temporary_wav().unwrap();
        let path = temp_path.to_path_buf();

        assert!(path.starts_with(std::env::temp_dir()));
        assert!(path.exists());
        drop(temp_path);
        assert!(!path.exists());
    }

    #[test]
    fn truncated_wav_is_not_accepted_as_partial_audio() {
        let mut source = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        source
            .write_all(&[
                b'R', b'I', b'F', b'F', 40, 0, 0, 0, b'W', b'A', b'V', b'E', b'f', b'm', b't',
                b' ', 16, 0, 0, 0, 1, 0, 1, 0, 0x80, 0x3e, 0, 0, 0, 0x7d, 0, 0, 2, 0, 16, 0, b'd',
                b'a', b't', b'a', 4, 0, 0, 0, 1, 0,
            ])
            .unwrap();
        source.flush().unwrap();

        assert!(decode_with_symphonia(source.path(), None).is_err());
    }
}
