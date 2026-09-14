use app_lib::audio::capture_negotiation::{
    negotiate, os_error_code, retryable_format_error, CaptureAlternatives, CaptureSpec,
    MAX_ATTEMPTS,
};
use std::cell::Cell;

fn spec(rate: u32, channels: u16, format: &'static str) -> CaptureSpec {
    CaptureSpec {
        rate,
        channels,
        format,
    }
}

#[test]
fn working_default_does_not_enumerate_or_retry() {
    let default = spec(48000, 2, "F32");
    let result = negotiate(
        default,
        |s| *s,
        |_, attempt| Ok::<_, &str>(attempt),
        || panic!("working devices must retain their original fast path"),
        |_| false,
    )
    .unwrap();
    assert_eq!(result, (1, default));
}

#[test]
fn rejected_bluetooth_float_uses_supported_pcm_on_the_same_endpoint() {
    let default = spec(16000, 1, "F32");
    let pcm = spec(16000, 1, "I16");
    let mut attempts = Vec::new();
    let (stream, selected) = negotiate(
        default,
        |s| *s,
        |s, _| {
            attempts.push(*s);
            if *s == pcm {
                Ok("same endpoint stream")
            } else {
                Err("format")
            }
        },
        || vec![spec(48000, 2, "F32"), spec(48000, 1, "I16"), default, pcm].into(),
        |e| *e == "format",
    )
    .unwrap();
    assert_eq!(attempts, vec![default, pcm]);
    assert_eq!(selected, pcm);
    assert_eq!(stream, "same endpoint stream");
}

#[test]
fn terminal_errors_stop_without_enumeration() {
    let result = negotiate(
        spec(16000, 1, "F32"),
        |s| *s,
        |_, _| Err::<(), _>("access denied"),
        || panic!("must not retry permissions"),
        |_| false,
    );
    assert_eq!(result.unwrap_err().attempts, 1);
}

#[test]
fn terminal_error_during_fallback_stops_remaining_attempts() {
    let calls = Cell::new(0);
    let result = negotiate(
        spec(16000, 1, "F32"),
        |s| *s,
        |_, _| {
            calls.set(calls.get() + 1);
            Err::<(), _>(if calls.get() == 1 {
                "format"
            } else {
                "disconnected"
            })
        },
        || vec![spec(16000, 1, "I16"), spec(48000, 1, "F32")].into(),
        |e| *e == "format",
    );
    let failure = result.unwrap_err();
    assert_eq!(failure.attempts, 2);
    assert_eq!(failure.error, "disconnected");
}

#[test]
fn candidates_are_deduplicated_and_attempts_are_bounded() {
    let default = spec(16000, 1, "F32");
    let mut seen = Vec::new();
    let result = negotiate(
        default,
        |s| *s,
        |s, _| {
            seen.push(*s);
            Err::<(), _>("format")
        },
        || {
            (8000..49000)
                .step_by(1000)
                .flat_map(|r| [default, spec(r, 1, "I16"), spec(r, 1, "I16")])
                .collect::<Vec<_>>()
                .into()
        },
        |_| true,
    );
    assert_eq!(result.unwrap_err().attempts, MAX_ATTEMPTS);
    assert_eq!(seen.len(), MAX_ATTEMPTS);
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len());
}

#[test]
fn missing_alternatives_retains_initial_error() {
    let failure = negotiate(
        spec(16000, 1, "F32"),
        |s| *s,
        |_, _| Err::<(), _>("original WASAPI error"),
        || Vec::new().into(),
        |_| true,
    )
    .unwrap_err();
    assert_eq!(failure.error, "original WASAPI error");
    assert_eq!(failure.attempts, 1);
}

#[test]
fn localized_windows_errors_keep_numeric_hresult() {
    let error = "Failed to initialize audio client: Параметр задан неверно. (os error -2147024809)";
    assert_eq!(os_error_code(error), Some(0x80070057_u32 as i32));
    assert!(retryable_format_error("BackendError", os_error_code(error)));
    assert!(!retryable_format_error(
        "BackendError",
        Some(0x80070005_u32 as i32)
    ));
    assert!(!retryable_format_error("BackendError", None));
    assert!(!retryable_format_error("DeviceBusy", None));
    assert!(!retryable_format_error("PermissionDenied", None));
    assert!(retryable_format_error("UnsupportedConfig", None));
    assert!(retryable_format_error("InvalidInput", None));
    assert_eq!(os_error_code("no numeric error"), None);
    assert_eq!(os_error_code("os error not-a-number)"), None);
}

#[test]
fn changed_bluetooth_default_is_tried_before_old_profile_alternatives() {
    let default = spec(16000, 1, "F32");
    let refreshed = spec(48000, 2, "F32");
    let mut attempts = Vec::new();
    let result = negotiate(
        default,
        |s| *s,
        |s, _| {
            attempts.push(*s);
            if *s == refreshed {
                Ok(())
            } else {
                Err("format")
            }
        },
        || CaptureAlternatives {
            refreshed: Some(refreshed),
            supported: (8000..40000)
                .step_by(1000)
                .map(|r| spec(r, 1, "I16"))
                .collect(),
        },
        |_| true,
    )
    .unwrap();
    assert_eq!(result.1, refreshed);
    assert_eq!(attempts, vec![default, refreshed]);
}

#[test]
fn unchanged_refresh_is_not_a_duplicate_attempt() {
    let default = spec(16000, 1, "F32");
    let pcm = spec(16000, 1, "I16");
    let mut calls = Vec::new();
    let result = negotiate(
        default,
        |s| *s,
        |s, _| {
            calls.push(*s);
            if *s == pcm {
                Ok(())
            } else {
                Err("format")
            }
        },
        || CaptureAlternatives {
            refreshed: Some(default),
            supported: vec![default, pcm, pcm],
        },
        |_| true,
    )
    .unwrap();
    assert_eq!(result.1, pcm);
    assert_eq!(calls, vec![default, pcm]);
}

#[test]
fn invalid_advertised_dimensions_are_not_attempted() {
    let default = spec(16000, 1, "F32");
    let error = negotiate(
        default,
        |s| *s,
        |_, attempt| {
            assert_eq!(attempt, 1);
            Err::<(), _>("format")
        },
        || vec![spec(0, 1, "I16"), spec(16000, 0, "I16")].into(),
        |_| true,
    )
    .unwrap_err();
    assert_eq!(error.attempts, 1);
}
