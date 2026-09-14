#[path = "../src/audio/directshow/device.rs"]
mod device;
use device::*;

fn listing(name: &str, media: &str, moniker: &str) -> String {
    format!("[dshow @ 1234] \"{name}\" ({media})\r\n[dshow @ 1234]   Alternative name \"{moniker}\"\r\n")
}

#[test]
fn selects_exact_unique_unicode_audio_not_similarly_named_camera() {
    let data = listing("Микрофон", "video", "camera")
        + &listing("Микрофон USB", "audio", "wrong")
        + &listing("Микрофон", "audio", "@device_cm_{abc}\\wave_0");
    assert_eq!(
        select_device(data.as_bytes(), "Микрофон").unwrap(),
        "@device_cm_{abc}\\wave_0"
    );
    assert_eq!(
        select_device(data.as_bytes(), "Микр"),
        Err(DeviceSelectionError::Missing)
    );
}

#[test]
fn duplicates_are_ambiguous_even_if_monikers_match() {
    let entry = listing("Mic", "audio", "m");
    assert!(matches!(
        select_device((entry.clone() + &entry).as_bytes(), "Mic"),
        Err(DeviceSelectionError::Ambiguous { matches: 2 })
    ));
}

#[test]
fn mixed_media_audio_pin_is_eligible() {
    assert_eq!(
        select_device(listing("Webcam", "video, audio", "m").as_bytes(), "Webcam").unwrap(),
        "m"
    );
}

#[test]
fn names_with_quotes_and_metacharacters_stay_single_arguments() {
    let name = "Микрофон \"USB\" & ; %COMSPEC%";
    let moniker = "@device_{x}\\wave_\"0\";&";
    assert_eq!(
        select_device(listing(name, "audio", moniker).as_bytes(), name).unwrap(),
        moniker
    );
    let args = capture_args(moniker);
    let input = args.iter().position(|arg| arg == "-i").unwrap();
    assert_eq!(
        args[input + 1],
        std::ffi::OsString::from(format!("audio={moniker}"))
    );
    assert!(!args.iter().any(|a| a == "-nostdin"));
    assert!(!args.iter().any(|a| a == "-sample_rate" || a == "-channels"));
    assert_eq!(args.last().unwrap(), "pipe:1");
}

#[test]
fn malformed_missing_or_orphan_monikers_fail_closed() {
    for data in [
        "[dshow @ 1] \"Mic\" (audio)\n",
        "[dshow @ 1]   Alternative name \"m\"\n",
        "[dshow @ 1] \"Mic\" (audio)\nother line\n[dshow @ 1]   Alternative name \"m\"\n",
        "[dshow @ 1] \"Mic\" (audio)\n[dshow @ 1]   Alternative name \"\"\n",
        "[dshow @ 1] \"Mic\" (audio)\n[dshow @ 1]   Alternative name \"m:video=x\"\n",
        "[dshow @ 1] \"Mic\" (audio)\n[dshow @ 1]   Alternative name \"truncated\n",
    ] {
        assert!(
            select_device(data.as_bytes(), "Mic").is_err(),
            "must reject malformed enumeration"
        );
    }
}

#[test]
fn ignores_normal_ffmpeg_diagnostics_without_accepting_unknown_media() {
    let data = "FFmpeg banner\n".to_owned()
        + &listing("Mic", "audio", "m")
        + "Error opening input file dummy.\n";
    assert_eq!(select_device(data.as_bytes(), "Mic").unwrap(), "m");
    assert!(select_device(listing("Mic", "notaudio", "m").as_bytes(), "Mic").is_err());
}

#[test]
fn bounded_strict_text_and_no_private_values_in_errors() {
    assert_eq!(
        select_device(&[255], "Mic"),
        Err(DeviceSelectionError::InvalidUtf8)
    );
    assert!(matches!(
        select_device(&vec![b'a'; ENUMERATION_BYTE_LIMIT + 1], "Mic"),
        Err(DeviceSelectionError::OutputTooLarge { .. })
    ));
    assert!(select_device(listing("Mic\0", "audio", "secret").as_bytes(), "Mic").is_err());
    let error = select_device(
        listing("Private", "video", "SECRET_MONIKER").as_bytes(),
        "Private",
    )
    .unwrap_err();
    assert!(
        !error.to_string().contains("Private") && !error.to_string().contains("SECRET_MONIKER")
    );
}

#[test]
fn enumeration_is_noninteractive_capture_output_is_48k_mono_float() {
    let args = enumeration_args();
    assert!(args.iter().any(|a| a == "-nostdin"));
    assert!(args.windows(2).any(|a| a == ["-list_devices", "true"]));
    let args = capture_args("m");
    for pair in [
        ["-ac", "1"],
        ["-ar", "48000"],
        ["-c:a", "pcm_f32le"],
        ["-f", "f32le"],
    ] {
        assert!(args.windows(2).any(|a| a == pair));
    }
}

#[test]
fn unrelated_none_or_unknown_filters_do_not_hide_selected_microphone() {
    let data = listing("Odd filter", "none", "odd")
        + &listing("Unknown filter", "unknown", "unknown")
        + &listing("Mic", "audio", "m");
    assert_eq!(select_device(data.as_bytes(), "Mic").unwrap(), "m");
    assert_eq!(
        select_device(data.as_bytes(), "Odd filter"),
        Err(DeviceSelectionError::Missing)
    );
}

#[test]
fn alternative_name_must_come_from_same_directshow_context() {
    let data = b"[dshow @ 1] \"Mic\" (audio)\n[dshow @ 2]   Alternative name \"wrong\"\n";
    assert_eq!(
        select_device(data, "Mic"),
        Err(DeviceSelectionError::Malformed)
    );
}
