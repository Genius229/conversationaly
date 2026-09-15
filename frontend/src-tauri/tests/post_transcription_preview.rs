use app_lib::audio::post_transcription::{
    preview::discard_audio, types::PostTranscriptionSettings,
};

#[test]
fn desktop_profile_defaults_to_post_processing_without_live_model() {
    let settings: PostTranscriptionSettings = serde_json::from_str("{}").unwrap();
    assert!(settings.auto_transcribe);
    assert!(!settings.live_preview);
    assert!(settings.vad_enabled);
}

#[test]
fn old_settings_without_vad_keep_gigastt_vad_enabled() {
    let settings: PostTranscriptionSettings =
        serde_json::from_str(r#"{"auto_transcribe":false,"live_preview":true}"#).unwrap();

    assert!(!settings.auto_transcribe);
    assert!(settings.live_preview);
    assert!(settings.vad_enabled);
}

#[test]
fn explicit_gigastt_vad_modes_survive_serialization_roundtrip() {
    for vad_enabled in [false, true] {
        let settings = PostTranscriptionSettings {
            auto_transcribe: false,
            live_preview: true,
            vad_enabled,
        };

        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&json).unwrap()["vad_enabled"],
            vad_enabled
        );
        assert_eq!(
            serde_json::from_str::<PostTranscriptionSettings>(&json).unwrap(),
            settings
        );
    }
}

#[tokio::test]
async fn disabled_preview_drains_every_chunk_and_joins_on_close() {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Vec<f32>>();
    let mut task = discard_audio(rx);
    for _ in 0..1000 {
        tx.send(vec![0.0; 1600]).unwrap();
    }
    drop(tx);
    let completed = tokio::time::timeout(std::time::Duration::from_millis(500), &mut task)
        .await
        .is_ok();
    if !completed {
        task.abort();
        let _ = task.await;
    }
    assert!(
        completed,
        "disabled preview must consume chunks until sender closes"
    );
}
