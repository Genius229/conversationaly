use app_lib::audio::post_transcription::finalization::verified_audio;

#[test]
fn only_successful_save_of_nonempty_audio_in_this_meeting_authorizes_auto_stt() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("audio.mp4");
    std::fs::write(&old, b"saved audio").unwrap();
    let valid: Result<Option<String>, &str> = Ok(Some(old.to_string_lossy().into_owned()));
    assert_eq!(
        verified_audio(Some(dir.path()), &valid),
        Some(old.canonicalize().unwrap())
    );
    assert_eq!(
        verified_audio(
            Some(dir.path()),
            &Err::<Option<String>, _>("finalize failed")
        ),
        None
    );
    assert_eq!(verified_audio(Some(dir.path()), &Ok::<_, &str>(None)), None);
    let foreign = tempfile::tempdir().unwrap();
    assert_eq!(verified_audio(Some(foreign.path()), &valid), None);
    std::fs::write(&old, []).unwrap();
    assert_eq!(verified_audio(Some(dir.path()), &valid), None);
}
