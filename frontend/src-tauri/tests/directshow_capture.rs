use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use app_lib::audio::directshow::{CaptureError, CaptureLimits, DirectShowCapture};
use serde_json::{json, Value};
use tempfile::TempDir;

struct FakeFfmpeg {
    _temp: TempDir,
    path: PathBuf,
}

impl FakeFfmpeg {
    fn new(overrides: Value) -> Self {
        let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("dshow-fixtures");
        fs::create_dir_all(&fixture_root).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("capture-")
            .tempdir_in(fixture_root)
            .unwrap();
        let source = PathBuf::from(env!("CARGO_BIN_EXE_fake-ffmpeg-capture"));
        let file_name = source.file_name().unwrap();
        let path = temp.path().join(file_name);
        fs::copy(&source, &path).unwrap();
        wait_until_executable_copy_is_ready(&path);
        let mut config = json!({
            "enumeration_stderr": concat!(
                "[dshow @ 00000001] DirectShow audio devices\n",
                "[dshow @ 00000001]  \"Lifecycle Microphone\" (audio)\n",
                "[dshow @ 00000001]    Alternative name \"@device_cm_test_audio\"\n"
            ),
            "initial_value": 0.25,
            "initial_samples": 480,
            "wait_for_quit": true,
            "exit_code": 0
        });
        merge(&mut config, overrides);
        fs::write(
            path.with_extension("json"),
            serde_json::to_vec(&config).unwrap(),
        )
        .unwrap();
        Self { _temp: temp, path }
    }

    fn path(&self) -> PathBuf {
        self.path.clone()
    }

    fn marker(&self, extension: &str) -> String {
        fs::read_to_string(self.path.with_extension(extension)).unwrap_or_default()
    }
}

fn wait_until_executable_copy_is_ready(path: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    loop {
        match std::process::Command::new(path)
            .arg("--fixture-probe")
            .status()
        {
            Ok(status) if status.success() => return,
            Err(error)
                if error.kind() == std::io::ErrorKind::ExecutableFileBusy
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::yield_now();
            }
            Ok(status) => panic!("fake executable probe failed with {status}"),
            Err(error) => panic!("fake executable probe failed: {error}"),
        }
    }
}

fn merge(target: &mut Value, overrides: Value) {
    let target = target.as_object_mut().unwrap();
    for (key, value) in overrides.as_object().unwrap() {
        target.insert(key.clone(), value.clone());
    }
}

fn limits() -> CaptureLimits {
    CaptureLimits {
        enumeration_timeout: Duration::from_millis(300),
        first_pcm_timeout: Duration::from_millis(300),
        quit_grace: Duration::from_millis(150),
        reap_timeout: Duration::from_millis(500),
    }
}

fn values(storage: &Arc<Mutex<Vec<f32>>>) -> Vec<f32> {
    storage.lock().unwrap().clone()
}

fn pids(fake: &FakeFfmpeg, kind: &str) -> Vec<u32> {
    fake.marker("pids")
        .lines()
        .filter_map(|line| line.split_once(','))
        .filter(|(recorded_kind, _)| *recorded_kind == kind)
        .filter_map(|(_, pid)| pid.parse().ok())
        .collect()
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // SAFETY: signal 0 only checks whether the process still exists.
    unsafe { kill(pid as i32, 0) == 0 }
}

#[cfg(target_os = "windows")]
fn process_exists(pid: u32) -> bool {
    use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
    else {
        return false;
    };
    let mut code = 0;
    let active =
        unsafe { GetExitCodeProcess(process, &mut code) }.is_ok() && code == STILL_ACTIVE.0 as u32;
    let _ = unsafe { CloseHandle(process) };
    active
}

async fn wait_until(mut condition: impl FnMut() -> bool, label: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {label}"));
}

fn assert_capture_token_was_an_atomic_argument(fake: &FakeFfmpeg) {
    let arguments = fake.marker("args");
    let capture_line = arguments
        .lines()
        .find(|line| line.contains("audio=@device_cm_test_audio"))
        .expect("capture invocation");
    assert!(capture_line
        .split('\u{1f}')
        .any(|arg| arg == "audio=@device_cm_test_audio"));
}

#[tokio::test]
async fn start_waits_for_real_pcm_and_graceful_stop_reaps_the_owned_child() {
    let fake = FakeFfmpeg::new(json!({ "initial_delay_ms": 80 }));
    let received = Arc::new(Mutex::new(Vec::new()));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let started = tokio::time::Instant::now();
    let capture = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        {
            let received = received.clone();
            move |pcm| received.lock().unwrap().extend_from_slice(pcm)
        },
        {
            let errors = errors.clone();
            move |error| errors.lock().unwrap().push(error)
        },
        limits(),
    )
    .await
    .unwrap();

    assert!(started.elapsed() >= Duration::from_millis(60));
    assert_eq!(values(&received), vec![0.25; 480]);
    assert_capture_token_was_an_atomic_argument(&fake);
    let report = capture.stop().await.unwrap();
    assert!(report.graceful);
    assert!(!report.force_killed);
    assert_eq!(report.samples_delivered, 480);
    assert!(fake.marker("events").contains("quit"));
    assert!(errors.lock().unwrap().is_empty());
}

#[tokio::test]
async fn first_pcm_timeout_fails_start_and_reaps_the_capture_process() {
    let fake = FakeFfmpeg::new(json!({
        "initial_samples": 0,
        "wait_for_quit": true
    }));
    let mut short = limits();
    short.first_pcm_timeout = Duration::from_millis(70);

    let result = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        |_| {},
        |_| {},
        short,
    )
    .await;

    assert!(matches!(result, Err(CaptureError::FirstPcmTimeout)));
    let captures = pids(&fake, "capture");
    assert_eq!(captures.len(), 1);
    wait_until(
        || captures.iter().all(|pid| !process_exists(*pid)),
        "first-PCM timed-out child reap",
    )
    .await;
}

#[tokio::test]
async fn cancelling_a_pending_start_aborts_and_reaps_the_owned_process() {
    let fake = FakeFfmpeg::new(json!({ "initial_delay_ms": 10_000 }));
    let path = fake.path();
    let start = tokio::spawn(async move {
        DirectShowCapture::start_with_limits(
            path,
            "Lifecycle Microphone".to_string(),
            |_| {},
            |_| {},
            limits(),
        )
        .await
    });
    wait_until(|| !pids(&fake, "capture").is_empty(), "capture spawn").await;

    start.abort();
    match start.await {
        Err(error) => assert!(error.is_cancelled()),
        Ok(_) => panic!("aborted start task completed normally"),
    }
    let captures = pids(&fake, "capture");
    wait_until(
        || captures.iter().all(|pid| !process_exists(*pid)),
        "cancelled pending-start child reap",
    )
    .await;
}

#[tokio::test]
async fn enumeration_timeout_kills_and_reaps_only_its_owned_process() {
    let fake = FakeFfmpeg::new(json!({ "enumeration_delay_ms": 10_000 }));
    let mut short = limits();
    short.enumeration_timeout = Duration::from_millis(70);

    let result = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        |_| {},
        |_| {},
        short,
    )
    .await;

    assert!(matches!(result, Err(CaptureError::EnumerationTimeout)));
    assert!(pids(&fake, "capture").is_empty());
    let enumerations = pids(&fake, "enumeration");
    assert_eq!(enumerations.len(), 1);
    wait_until(
        || enumerations.iter().all(|pid| !process_exists(*pid)),
        "enumeration child reap",
    )
    .await;
}

#[tokio::test]
async fn empty_capture_exit_is_not_reported_as_ready() {
    let fake = FakeFfmpeg::new(json!({
        "initial_samples": 0,
        "wait_for_quit": false
    }));
    let errors = Arc::new(Mutex::new(Vec::new()));

    let result = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        |_| {},
        {
            let errors = errors.clone();
            move |error| errors.lock().unwrap().push(error)
        },
        limits(),
    )
    .await;

    assert!(matches!(result, Err(CaptureError::StartupExited)));
    assert!(errors.lock().unwrap().is_empty());
}

#[tokio::test]
async fn arbitrarily_fragmented_pcm_is_delivered_in_480_sample_frames_and_an_eof_tail() {
    let fake = FakeFfmpeg::new(json!({
        "initial_samples": 725,
        "byte_fragments": [1, 2, 3, 5, 7]
    }));
    let callbacks = Arc::new(Mutex::new(Vec::<Vec<f32>>::new()));
    let capture = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        {
            let callbacks = callbacks.clone();
            move |pcm| callbacks.lock().unwrap().push(pcm.to_vec())
        },
        |_| {},
        limits(),
    )
    .await
    .unwrap();

    let report = capture.stop().await.unwrap();
    let callbacks = callbacks.lock().unwrap();
    assert_eq!(
        callbacks.iter().map(Vec::len).collect::<Vec<_>>(),
        [480, 245]
    );
    assert!(callbacks.iter().flatten().all(|sample| *sample == 0.25));
    assert_eq!(report.samples_delivered, 725);
}

#[tokio::test]
async fn nonfinite_pcm_fails_start_without_delivering_or_calling_runtime_error() {
    let fake = FakeFfmpeg::new(json!({ "invalid_pcm": "nonfinite" }));
    let received = Arc::new(Mutex::new(Vec::<f32>::new()));
    let errors = Arc::new(Mutex::new(Vec::new()));

    let result = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        {
            let received = received.clone();
            move |pcm| received.lock().unwrap().extend_from_slice(pcm)
        },
        {
            let errors = errors.clone();
            move |error| errors.lock().unwrap().push(error)
        },
        limits(),
    )
    .await;

    assert!(matches!(result, Err(CaptureError::NonFinitePcm)));
    assert!(received.lock().unwrap().is_empty());
    assert!(errors.lock().unwrap().is_empty());
}

#[tokio::test]
async fn truncated_pcm_at_eof_is_rejected() {
    let fake = FakeFfmpeg::new(json!({
        "invalid_pcm": "truncated",
        "wait_for_quit": false
    }));

    let result = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        |_| {},
        |_| {},
        limits(),
    )
    .await;

    assert!(matches!(result, Err(CaptureError::TruncatedPcm)));
}

#[tokio::test]
async fn stderr_flood_is_continuously_drained_without_exposing_private_payloads() {
    let fake = FakeFfmpeg::new(json!({ "stderr_bytes": 2 * 1024 * 1024 }));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let capture = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        |_| {},
        {
            let errors = errors.clone();
            move |error| errors.lock().unwrap().push(error)
        },
        limits(),
    )
    .await
    .unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;

    capture.stop().await.unwrap();
    let rendered = format!("{:?}", errors.lock().unwrap().as_slice());
    assert!(!rendered.contains("PRIVATE_DEVICE_MODEL_ID"));
    assert!(!rendered.contains("PRIVATE_TRANSCRIPT_PAYLOAD"));
    assert!(errors.lock().unwrap().is_empty());
}

#[tokio::test]
async fn graceful_quit_drains_the_final_aligned_pcm_tail_before_finish() {
    let fake = FakeFfmpeg::new(json!({
        "tail_value": -0.5,
        "tail_samples": 240
    }));
    let received = Arc::new(Mutex::new(Vec::new()));
    let capture = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        {
            let received = received.clone();
            move |pcm| received.lock().unwrap().extend_from_slice(pcm)
        },
        |_| {},
        limits(),
    )
    .await
    .unwrap();

    capture.request_stop();
    let report = capture.finish().await.unwrap();
    let received = values(&received);
    assert_eq!(&received[..480], vec![0.25; 480]);
    assert_eq!(&received[480..], vec![-0.5; 240]);
    assert_eq!(report.samples_delivered, 720);
    assert!(report.graceful);
}

#[tokio::test]
async fn ignored_quit_is_force_killed_and_reaped_after_the_grace_deadline() {
    let fake = FakeFfmpeg::new(json!({ "ignore_quit": true }));
    let mut short = limits();
    short.quit_grace = Duration::from_millis(60);
    let capture = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        |_| {},
        |_| {},
        short,
    )
    .await
    .unwrap();
    let captures = pids(&fake, "capture");

    let report = capture.stop().await.unwrap();
    assert!(report.force_killed);
    assert!(!report.graceful);
    wait_until(
        || captures.iter().all(|pid| !process_exists(*pid)),
        "quit-ignoring child reap",
    )
    .await;
}

#[tokio::test]
async fn unexpected_exit_after_readiness_calls_the_sanitized_runtime_error_once() {
    let fake = FakeFfmpeg::new(json!({
        "wait_for_quit": false,
        "exit_code": 17,
        "stderr_bytes": 512
    }));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let capture = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        |_| {},
        {
            let errors = errors.clone();
            move |error| errors.lock().unwrap().push(error)
        },
        limits(),
    )
    .await
    .unwrap();

    wait_until(
        || errors.lock().unwrap().len() == 1,
        "unexpected-exit callback",
    )
    .await;
    assert!(matches!(
        capture.finish().await,
        Err(CaptureError::RuntimeExited)
    ));
    let errors = errors.lock().unwrap();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("exited unexpectedly"));
    assert!(!errors[0].contains("Lifecycle Microphone"));
    assert!(!errors[0].contains("PRIVATE_"));
}

#[tokio::test]
async fn drop_requests_emergency_cleanup_without_blocking() {
    let fake = FakeFfmpeg::new(json!({ "ignore_quit": true }));
    let capture = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        |_| {},
        |_| {},
        limits(),
    )
    .await
    .unwrap();
    let captures = pids(&fake, "capture");

    let dropped = std::time::Instant::now();
    drop(capture);
    assert!(dropped.elapsed() < Duration::from_millis(30));
    wait_until(
        || captures.iter().all(|pid| !process_exists(*pid)),
        "dropped capture child reap",
    )
    .await;
}

#[tokio::test]
async fn repeated_start_stop_cycles_leave_no_capture_processes() {
    let fake = FakeFfmpeg::new(json!({}));
    for _ in 0..3 {
        DirectShowCapture::start_with_limits(
            fake.path(),
            "Lifecycle Microphone".to_string(),
            |_| {},
            |_| {},
            limits(),
        )
        .await
        .unwrap()
        .stop()
        .await
        .unwrap();
    }

    let captures = pids(&fake, "capture");
    assert_eq!(captures.len(), 3);
    wait_until(
        || captures.iter().all(|pid| !process_exists(*pid)),
        "repeated capture child reap",
    )
    .await;
}

#[tokio::test]
async fn an_expected_nonzero_enumeration_exit_still_uses_its_valid_listing() {
    let fake = FakeFfmpeg::new(json!({ "exit_code": 1 }));

    let capture = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        |_| {},
        |_| {},
        limits(),
    )
    .await
    .unwrap();

    capture.stop().await.unwrap();
    assert_capture_token_was_an_atomic_argument(&fake);
}

#[tokio::test]
async fn stopping_capture_never_terminates_an_unrelated_process() {
    use std::process::{Command, Stdio};

    let fake = FakeFfmpeg::new(json!({ "ignore_quit": true }));
    let mut unrelated = Command::new(fake.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_until(
        || process_exists(unrelated.id()),
        "unrelated process startup",
    )
    .await;

    let mut short = limits();
    short.quit_grace = Duration::from_millis(60);
    DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        |_| {},
        |_| {},
        short,
    )
    .await
    .unwrap()
    .stop()
    .await
    .unwrap();

    assert!(unrelated.try_wait().unwrap().is_none());
    unrelated.kill().unwrap();
    unrelated.wait().unwrap();
}

#[tokio::test]
async fn zero_time_limit_is_rejected_before_spawning_any_process() {
    let fake = FakeFfmpeg::new(json!({}));
    let mut invalid = limits();
    invalid.first_pcm_timeout = Duration::ZERO;

    let result = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        |_| {},
        |_| {},
        invalid,
    )
    .await;

    assert!(matches!(result, Err(CaptureError::InvalidLimits)));
    assert!(fake.marker("pids").is_empty());
}

#[tokio::test]
async fn invalid_pcm_after_readiness_reports_one_sanitized_runtime_error() {
    let fake = FakeFfmpeg::new(json!({ "invalid_pcm": "nonfinite_after_initial" }));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let capture = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        |_| {},
        {
            let errors = errors.clone();
            move |error| errors.lock().unwrap().push(error)
        },
        limits(),
    )
    .await
    .unwrap();

    wait_until(
        || errors.lock().unwrap().len() == 1,
        "invalid-PCM runtime callback",
    )
    .await;
    assert!(matches!(
        capture.finish().await,
        Err(CaptureError::NonFinitePcm)
    ));
    let errors = errors.lock().unwrap();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("non-finite PCM"));
    assert!(!errors[0].contains("Lifecycle Microphone"));
}

#[tokio::test]
async fn graceful_stop_honors_its_grace_when_stdout_closes_before_process_exit() {
    let fake = FakeFfmpeg::new(json!({
        "close_stdout_after_initial": true,
        "exit_delay_after_quit_ms": 150
    }));
    let mut patient = limits();
    patient.quit_grace = Duration::from_millis(500);
    let capture = DirectShowCapture::start_with_limits(
        fake.path(),
        "Lifecycle Microphone".to_string(),
        |_| {},
        |_| {},
        patient,
    )
    .await
    .unwrap();

    let started = std::time::Instant::now();
    let report = capture.stop().await.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(120));
    assert!(report.graceful);
    assert!(!report.force_killed);
    assert_eq!(report.samples_delivered, 480);
}

#[cfg(target_os = "windows")]
#[tokio::test]
async fn windows_job_kills_capture_when_its_owner_process_exits_after_readiness() {
    let fake = FakeFfmpeg::new(json!({}));
    let status = std::process::Command::new(fake.path())
        .arg("--job-owner-probe")
        .status()
        .unwrap();
    assert!(status.success());
    assert!(fake.marker("events").contains("owner-ready"));
    let captures = pids(&fake, "capture");
    assert_eq!(captures.len(), 1);
    wait_until(
        || captures.iter().all(|pid| !process_exists(*pid)),
        "job-contained child termination after owner exit",
    )
    .await;
}

#[allow(dead_code)]
fn _assert_path(_: &Path) {}
