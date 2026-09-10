use app_lib::audio::post_transcription::audio_prepare::{prepare_audio, AudioPrepareError};
use app_lib::audio::post_transcription::wav::write_pcm16_wav;
use std::{fs, io};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn preparation_uses_decoder_and_meeting_processing_folder() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("audio.mp4");
    fs::write(&input, b"original").unwrap();
    let result = prepare_audio(
        input.clone(),
        dir.path().into(),
        Arc::new(|_: &Path| Ok(vec![0.0; 16000])),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(
        result.path,
        dir.path().join(".processing/gigastt-input.wav")
    );
    assert_eq!(result.duration_seconds, 1.0);
    assert_eq!(fs::metadata(result.path).unwrap().len(), 32044);
    assert_eq!(fs::read(input).unwrap(), b"original");
}

#[tokio::test]
async fn pre_cancel_does_not_decode_or_mutate_original() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("audio.mp4");
    fs::write(&input, b"original").unwrap();
    let called = Arc::new(AtomicBool::new(false));
    let was_called = called.clone();
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = prepare_audio(
        input.clone(),
        dir.path().into(),
        Arc::new(move |_: &Path| {
            was_called.store(true, Ordering::SeqCst);
            Ok(vec![0.0])
        }),
        cancel,
    )
    .await;
    assert!(matches!(result, Err(AudioPrepareError::Cancelled)));
    assert!(!called.load(Ordering::SeqCst));
    assert_eq!(fs::read(input).unwrap(), b"original");
}

#[tokio::test]
async fn rejects_an_audio_file_outside_the_meeting() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let input = other.path().join("audio.wav");
    fs::write(&input, b"other meeting").unwrap();
    let result = prepare_audio(
        input.clone(),
        dir.path().into(),
        Arc::new(|_: &Path| Ok(vec![0.0])),
        CancellationToken::new(),
    )
    .await;
    assert!(matches!(result, Err(AudioPrepareError::InvalidLocation)));
    assert_eq!(fs::read(input).unwrap(), b"other meeting");
}

#[test]
fn writes_pcm16_mono_16khz_and_preserves_original() {
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("audio.mp4");
    fs::write(&original, b"immutable original recording").unwrap();
    let path = dir.path().join("gigastt-input.wav");
    write_pcm16_wav(&path, &[-1.0, 0.0, 1.0, 0.5, -0.5]).unwrap();
    let bytes = fs::read(&path).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[8..16], b"WAVEfmt ");
    assert_eq!(u16::from_le_bytes(bytes[20..22].try_into().unwrap()), 1);
    assert_eq!(u16::from_le_bytes(bytes[22..24].try_into().unwrap()), 1);
    assert_eq!(u32::from_le_bytes(bytes[24..28].try_into().unwrap()), 16000);
    assert_eq!(u16::from_le_bytes(bytes[34..36].try_into().unwrap()), 16);
    assert_eq!(&bytes[36..40], b"data");
    assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 10);
    let samples: Vec<_> = bytes[44..]
        .as_chunks::<2>()
        .0
        .iter()
        .copied()
        .map(i16::from_le_bytes)
        .collect();
    assert_eq!(samples, [-32768, 0, 32767, 16384, -16384]);
    assert_eq!(fs::read(original).unwrap(), b"immutable original recording");
    assert_eq!(bytes.len(), 54);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
}

#[test]
fn rejects_invalid_audio_without_replacing_previous_preparation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gigastt-input.wav");
    fs::write(&path, b"previous preparation").unwrap();
    for samples in [vec![], vec![f32::NAN], vec![f32::INFINITY]] {
        assert_eq!(
            write_pcm16_wav(&path, &samples).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(fs::read(&path).unwrap(), b"previous preparation");
    }
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[test]
fn clips_finite_resampler_overshoot_and_atomically_replaces_temp_audio() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gigastt-input.wav");
    fs::write(&path, b"old temporary wav").unwrap();
    write_pcm16_wav(&path, &[-1.2, 1.2]).unwrap();
    assert_eq!(&fs::read(&path).unwrap()[44..], &[0, 128, 255, 127]);
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn cancellation_after_decode_cannot_produce_input_for_a_job() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("audio.wav");
    fs::write(&input, b"original").unwrap();
    let token = CancellationToken::new();
    let cancel = token.clone();
    let result = prepare_audio(
        input.clone(),
        dir.path().into(),
        Arc::new(move |_: &Path| {
            cancel.cancel();
            Ok(vec![0.0; 16])
        }),
        token,
    )
    .await;
    assert!(matches!(result, Err(AudioPrepareError::Cancelled)));
    assert!(!dir.path().join(".processing/gigastt-input.wav").exists());
    assert_eq!(fs::read(input).unwrap(), b"original");
}

#[tokio::test]
async fn decode_failure_preserves_prior_temp_and_redacts_details() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("audio.wav");
    fs::write(&input, b"original").unwrap();
    let processing = dir.path().join(".processing");
    fs::create_dir(&processing).unwrap();
    let temp = processing.join("gigastt-input.wav");
    fs::write(&temp, b"old temp").unwrap();
    let result = prepare_audio(
        input,
        dir.path().into(),
        Arc::new(|_: &Path| Err("private details".into())),
        CancellationToken::new(),
    )
    .await;
    assert!(matches!(result, Err(AudioPrepareError::DecodeFailed)));
    assert_eq!(fs::read(temp).unwrap(), b"old temp");
}

#[cfg(unix)]
#[tokio::test]
async fn processing_symlink_cannot_redirect_temp_writes() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let input = dir.path().join("audio.wav");
    fs::write(&input, b"original").unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join(".processing")).unwrap();
    let result = prepare_audio(
        input,
        dir.path().into(),
        Arc::new(|_: &Path| Ok(vec![0.0])),
        CancellationToken::new(),
    )
    .await;
    assert!(matches!(result, Err(AudioPrepareError::InvalidLocation)));
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}
