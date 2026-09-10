use audio_codec_contract_tests::{decoder, ffmpeg, import_validation};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use symphonia::core::codecs::{audio::AudioDecoderOptions, CodecParameters};
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;

// Both cases inspect the process-wide conversion temp prefix. Do not mistake
// another test's live RAII file for a leak under the default parallel runner.
static TEMP_INSPECTION: Mutex<()> = Mutex::new(());

fn hash(path: &Path) -> Vec<u8> {
    Sha256::digest(fs::read(path).unwrap()).to_vec()
}

fn conversion_temps() -> BTreeSet<PathBuf> {
    fs::read_dir(std::env::temp_dir())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".conversationaly_decode_")
        })
        .map(|entry| entry.path())
        .collect()
}

fn bundled_ffmpeg() -> PathBuf {
    let path =
        ffmpeg::find_existing_ffmpeg_path().expect("Real FFmpeg is required for codec acceptance");
    if let Some(expected) = std::env::var_os("CODEC_EXPECTED_FFMPEG") {
        assert_eq!(
            path.canonicalize().unwrap(),
            PathBuf::from(expected).canonicalize().unwrap()
        );
    }
    path
}

fn fixture(path: &Path, codec: &str) {
    let status = Command::new(bundled_ffmpeg())
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=1.2",
            "-ac",
            "2",
            "-c:a",
            codec,
            "-y",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "Could not generate real {codec} fixture");
}

fn native_registry_rejects_codec(path: &Path) {
    let mut hint = Hint::new();
    hint.with_extension(path.extension().unwrap().to_str().unwrap());
    let stream =
        MediaSourceStream::new(Box::new(fs::File::open(path).unwrap()), Default::default());
    let format = symphonia::default::get_probe()
        .probe(&hint, stream, Default::default(), Default::default())
        .unwrap();
    let track = format
        .tracks()
        .iter()
        .find(|track| {
            track
                .codec_params
                .as_ref()
                .is_some_and(CodecParameters::is_audio)
        })
        .unwrap();
    let Some(CodecParameters::Audio(parameters)) = track.codec_params.as_ref() else {
        panic!("Missing audio parameters")
    };
    assert!(
        symphonia::default::get_codecs()
            .make_audio_decoder(parameters, &AudioDecoderOptions::default())
            .is_err(),
        "Fixture must reproduce an unsupported native codec, not silently take the native path"
    );
}

#[test]
fn real_import_validation_and_decode_cover_opus_alac_aac_and_pcm() {
    let _guard = TEMP_INSPECTION.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("Telegram аудио с пробелами");
    fs::create_dir(&folder).unwrap();
    for (name, codec, needs_fallback) in [
        ("voice.ogg", "libopus", true),
        ("lossless.m4a", "alac", true),
        ("aac.m4a", "aac", false),
        ("recording.mp4", "aac", false),
        ("pcm.wav", "pcm_s16le", false),
    ] {
        let source = folder.join(name);
        fixture(&source, codec);
        if needs_fallback {
            native_registry_rejects_codec(&source);
        }
        let before = hash(&source);
        let temps = conversion_temps();
        let permissions = fs::metadata(&source).unwrap().permissions();
        let mut read_only = permissions.clone();
        read_only.set_readonly(true);
        fs::set_permissions(&source, read_only).unwrap();
        // Match Windows imports: archive paths are canonical extended-length paths.
        let canonical = source.canonicalize().unwrap();
        let info = import_validation::validate_audio_file(&canonical)
            .unwrap_or_else(|error| panic!("Validation {name}: {error}"));
        let decoded = decoder::decode_audio_file(&canonical)
            .unwrap_or_else(|error| panic!("Decode {name}: {error}"));
        assert!(
            (info.duration_seconds - 1.2).abs() < 0.1,
            "Metadata duration {name}: {}",
            info.duration_seconds
        );
        assert!(
            (decoded.duration_seconds - 1.2).abs() < 0.1,
            "Decoded duration {name}: {}",
            decoded.duration_seconds
        );
        assert_eq!(decoded.sample_rate, 48000);
        assert_eq!(decoded.channels, 2);
        assert!(decoded.samples.iter().all(|sample| sample.is_finite()));
        assert!(decoded.samples.iter().any(|sample| sample.abs() > 0.01));
        assert_eq!(hash(&source), before, "Original changed: {name}");
        assert_eq!(conversion_temps(), temps, "Temporary WAV leaked: {name}");
        assert!(!fs::read_dir(&folder)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".conversationaly_decode_")));
        fs::set_permissions(&source, permissions).unwrap();
    }
}

#[test]
fn invalid_media_fails_without_source_mutation_or_temporary_leaks() {
    let _guard = TEMP_INSPECTION.lock().unwrap();
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("bad.m4a");
    fs::write(&source, b"PRIVATE_MEDIA_CONTENT_NOT_FOR_LOGS").unwrap();
    let before = hash(&source);
    let temps = conversion_temps();
    let error = decoder::decode_audio_file(&source).unwrap_err().to_string();
    assert!(!error.contains("PRIVATE_MEDIA_CONTENT"));
    assert!(import_validation::validate_audio_file(&source).is_err());
    assert_eq!(hash(&source), before);
    assert_eq!(conversion_temps(), temps);
}
