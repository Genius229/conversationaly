//! Structural shutdown guards supplement the Windows fake-process tests.
//!
//! The production lifecycle tests prove process behavior. These checks keep
//! every application path routed through the same drain-before-state-close
//! contract even though the full Tauri application cannot link in headless CI.

const STREAM: &str = include_str!("../src/audio/stream.rs");
const MANAGER: &str = include_str!("../src/audio/recording_manager.rs");
const COMMANDS: &str = include_str!("../src/audio/recording_commands.rs");
const LIB: &str = include_str!("../src/lib.rs");

fn function_body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("missing {signature}"));
    let brace = start + source[start..].find('{').expect("function opening brace");
    let mut depth = 0usize;
    for (offset, byte) in source.as_bytes()[brace..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return &source[brace + 1..brace + offset];
                }
            }
            _ => {}
        }
    }
    panic!("unterminated {signature}")
}

#[test]
fn normal_stop_paths_share_the_directshow_aware_capture_stop() {
    for name in [
        "pub async fn stop_streams_only",
        "pub async fn stop_streams_and_force_flush",
        "pub async fn stop_recording",
        "pub async fn cleanup_without_save",
    ] {
        let body = function_body(MANAGER, name);
        assert!(
            body.contains("self.stop_capture_sources().await"),
            "{name} bypasses the drain-before-state-close helper"
        );
    }

    let helper = function_body(MANAGER, "async fn stop_capture_sources");
    let drain = helper.find("stop_streams_and_drain().await").unwrap();
    let state_close = helper.find("state.stop_recording()").unwrap();
    assert!(
        drain < state_close,
        "DirectShow PCM must drain before state closes"
    );
}

#[test]
fn reconnect_rejects_active_directshow_and_restarts_cpal_without_fallback() {
    let body = function_body(MANAGER, "pub async fn attempt_device_reconnect");
    let guard = body.find("has_directshow_microphone()").unwrap();
    let first_await = body.find(".await").unwrap();
    assert!(guard < first_await, "active DirectShow must fail before any await");
    assert!(body.contains("stop and start recording again"));
    assert_eq!(body.matches("stop_streams()?;").count(), 2);
    assert_eq!(body.matches("start_streams_without_fallback").count(), 2);
    assert!(!body.contains("stop_streams_and_drain"));
    assert!(!body.contains("state.stop_recording()"));
}

#[test]
fn reconnect_capture_creation_explicitly_disables_directshow() {
    let reconnect = function_body(STREAM, "pub(super) async fn start_streams_without_fallback");
    assert!(reconnect.contains("start_streams_inner"));
    assert!(reconnect.contains("false"));

    let creation = function_body(STREAM, "async fn create_cpal_stream");
    assert!(creation.contains("directshow_startup_allowed"));
    assert!(creation.contains("allow_directshow"));
}

#[test]
fn exit_cleanup_is_selective_and_runs_before_database_shutdown() {
    let cleanup = function_body(COMMANDS, "pub async fn cleanup_on_exit");
    assert!(cleanup.contains("has_directshow_microphone()"));
    assert!(cleanup.contains("manager_guard.take()"));
    assert!(cleanup.contains("cleanup_external_capture_on_exit().await"));

    let exit = function_body(LIB, "pub fn run");
    let drain = exit
        .find("recording_commands::cleanup_on_exit().await")
        .unwrap();
    let database = exit.find("db_manager.cleanup().await").unwrap();
    assert!(
        drain < database,
        "external capture must drain before database cleanup"
    );
}

#[test]
fn stream_manager_keeps_sync_stop_and_adds_explicit_async_drain() {
    assert!(STREAM.contains("pub fn stop(self) -> Result<()>"));
    assert!(STREAM.contains("pub async fn stop_streams_and_drain"));
    assert!(STREAM.contains("directshow.request_stop();"));
    assert!(STREAM.contains("directshow.finish().await"));
}
