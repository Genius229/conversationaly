use std::fs;
use std::io::Write;
use std::net::TcpListener as StdTcpListener;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use app_lib::audio::post_transcription::{
    DiagnosticKind, GigasttSidecar, GigasttSidecarConfig, SidecarError, SidecarStatus,
    SubmitJobOptions, PINNED_GIGASTT_VERSION,
};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn fake_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake-gigastt"))
}

fn test_config(temp: &TempDir, port: Option<u16>) -> GigasttSidecarConfig {
    GigasttSidecarConfig::new(fake_binary(), temp.path().join("models"), port)
        .with_startup_timeout(Duration::from_millis(650))
        .with_poll_interval(Duration::from_millis(15))
        .with_request_timeout(Duration::from_millis(80))
        .with_shutdown_timeouts(Duration::from_millis(120), Duration::from_millis(250))
}

fn model_dir(temp: &TempDir) -> PathBuf {
    temp.path().join("models")
}

fn control(temp: &TempDir, name: &str, value: &str) {
    let dir = model_dir(temp);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(name), value).unwrap();
}

fn launch_count(temp: &TempDir) -> usize {
    fs::read_to_string(model_dir(temp).join("fake-launch-count"))
        .ok()
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0)
}

fn pids(temp: &TempDir) -> Vec<u32> {
    fs::read_to_string(model_dir(temp).join("fake-pids"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.parse().ok())
        .collect()
}

fn fixture_diagnostics(temp: &TempDir) -> String {
    [
        "fake-failures",
        "fake-transport-diagnostics",
        "fake-bind-errors",
    ]
    .into_iter()
    .filter_map(|name| {
        fs::read_to_string(model_dir(temp).join(name))
            .ok()
            .map(|value| format!("{name}:\n{value}"))
    })
    .collect::<Vec<_>>()
    .join("\n")
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    // SAFETY: signal 0 performs existence/permission checking only.
    unsafe { kill(pid as i32, 0) == 0 }
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

async fn wait_status(manager: &GigasttSidecar, wanted: SidecarStatus) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if manager.status().await == wanted {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {wanted:?}"));
}

fn reserve_port() -> u16 {
    StdTcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn starts_with_verified_v221_flags_on_the_same_random_loopback_port() {
    assert_eq!(PINNED_GIGASTT_VERSION, "2.21.0");
    let temp = TempDir::new().unwrap();
    control(&temp, "fake-ready-delay-ms", "80");
    let manager = GigasttSidecar::new(test_config(&temp, None)).unwrap();
    let mut states = manager.subscribe();

    let start = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.ensure_ready().await })
    };
    let mut observed = vec![states.borrow().status];
    while !observed.contains(&SidecarStatus::Ready) {
        states.changed().await.unwrap();
        observed.push(states.borrow().status);
    }
    let client = start.await.unwrap().unwrap();
    assert!(observed.contains(&SidecarStatus::Starting));
    assert_eq!(client.ready().await.unwrap().pool_total, Some(1));

    let snapshot = manager.snapshot().await;
    let port = snapshot.port.expect("chosen port");
    assert_ne!(port, 0);
    let raw = fs::read_to_string(model_dir(&temp).join("fake-args")).unwrap();
    let args: Vec<&str> = raw.trim().split('\u{1f}').collect();
    let root = model_dir(&temp);
    let expected = vec![
        "--offline".to_string(),
        "serve".to_string(),
        "--host".to_string(),
        "127.0.0.1".to_string(),
        "--port".to_string(),
        port.to_string(),
        "--model-dir".to_string(),
        root.display().to_string(),
        "--model-variant".to_string(),
        "rnnt".to_string(),
        "--punctuation".to_string(),
        "on".to_string(),
        "--punct-model-dir".to_string(),
        root.join("punct").display().to_string(),
        "--itn".to_string(),
        "on".to_string(),
        "--vad".to_string(),
        "--vad-model-dir".to_string(),
        root.join("vad").display().to_string(),
        "--pool-size".to_string(),
        "1".to_string(),
        "--pool-min-size".to_string(),
        "1".to_string(),
        "--batch-pool-size".to_string(),
        "0".to_string(),
        "--file-window-concurrency".to_string(),
        "1".to_string(),
        "--enable-jobs".to_string(),
        "--shutdown-drain-secs".to_string(),
        "1".to_string(),
    ];
    assert_eq!(
        args,
        expected.iter().map(String::as_str).collect::<Vec<_>>()
    );

    manager.set_busy(true).await.unwrap();
    assert_eq!(manager.status().await, SidecarStatus::Busy);
    manager.set_busy(false).await.unwrap();
    assert_eq!(manager.status().await, SidecarStatus::Ready);
    manager.shutdown().await.unwrap();
    assert_eq!(manager.status().await, SidecarStatus::NotInstalled);
}

#[tokio::test]
async fn incomplete_header_does_not_block_job_status_and_delete() {
    let temp = TempDir::new().unwrap();
    let manager = GigasttSidecar::new(test_config(&temp, None)).unwrap();
    let client = manager.ensure_ready().await.unwrap();
    let port = manager.snapshot().await.port.unwrap();

    let mut abandoned = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    abandoned
        .write_all(b"GET /v1/jobs/job_1 HTTP/1.1\r\nHost:")
        .unwrap();
    // Let the server accept the deliberately incomplete request first. A
    // serial header reader would now head-of-line block both calls below.
    tokio::time::sleep(Duration::from_millis(30)).await;

    let started = Instant::now();
    let status = client.get_job("job_1").await.unwrap();
    assert_eq!(
        status.status,
        app_lib::audio::post_transcription::GigasttJobStatus::Processing
    );
    client.cancel_job("job_1").await.unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "ready requests were head-of-line blocked by an incomplete header"
    );
    assert!(model_dir(&temp).join("fake-cancelled-jobs").exists());

    drop(abandoned);
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn concurrent_ensure_ready_calls_spawn_exactly_one_owned_process() {
    let temp = TempDir::new().unwrap();
    control(&temp, "fake-ready-delay-ms", "100");
    let manager = GigasttSidecar::new(test_config(&temp, Some(reserve_port()))).unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..12 {
        let manager = manager.clone();
        tasks.spawn(async move { manager.ensure_ready().await });
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap().unwrap();
    }
    assert_eq!(launch_count(&temp), 1);
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn readiness_timeout_fails_and_reaps_the_child() {
    let temp = TempDir::new().unwrap();
    control(&temp, "fake-never-ready", "");
    let manager = GigasttSidecar::new(test_config(&temp, None)).unwrap();

    let result = manager.ensure_ready().await;
    assert!(
        matches!(result, Err(SidecarError::StartupTimeout { .. })),
        "fixture diagnostics:\n{}",
        fixture_diagnostics(&temp)
    );
    assert_eq!(
        manager.status().await,
        SidecarStatus::Failed,
        "fixture diagnostics:\n{}",
        fixture_diagnostics(&temp)
    );
    let children = pids(&temp);
    assert_eq!(
        children.len(),
        1,
        "fixture diagnostics:\n{}",
        fixture_diagnostics(&temp)
    );
    #[cfg(unix)]
    wait_until(|| !process_exists(children[0]), "timed-out child reap").await;
}

#[tokio::test]
async fn shutdown_cancels_an_in_progress_start_and_reaps_the_child() {
    let temp = TempDir::new().unwrap();
    control(&temp, "fake-never-ready", "");
    let manager = GigasttSidecar::new(test_config(&temp, None)).unwrap();
    let start = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.ensure_ready().await })
    };
    wait_status(&manager, SidecarStatus::Starting).await;
    wait_until(|| launch_count(&temp) == 1, "child launch").await;

    manager.shutdown().await.unwrap();
    assert!(matches!(start.await.unwrap(), Err(SidecarError::Cancelled)));
    assert_eq!(manager.status().await, SidecarStatus::NotInstalled);
    #[cfg(unix)]
    wait_until(
        || pids(&temp).iter().all(|pid| !process_exists(*pid)),
        "cancelled child reap",
    )
    .await;
}

#[tokio::test]
async fn one_unexpected_exit_restarts_then_a_second_exit_stops() {
    let temp = TempDir::new().unwrap();
    control(&temp, "fake-exit-after-ms", "140,140");
    let manager = GigasttSidecar::new(test_config(&temp, None)).unwrap();
    manager.ensure_ready().await.unwrap();

    wait_until(|| launch_count(&temp) == 2, "one automatic restart").await;
    wait_status(&manager, SidecarStatus::Ready).await;
    wait_status(&manager, SidecarStatus::Failed).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(launch_count(&temp), 2, "must not loop after second exit");
    let snapshot = manager.snapshot().await;
    assert_eq!(snapshot.restart_count, 1);
    assert!(snapshot.failure.is_some());
}

#[tokio::test]
async fn bounded_diagnostics_never_retain_child_output_payloads() {
    let temp = TempDir::new().unwrap();
    let config = test_config(&temp, None).with_diagnostic_limits(3, 32);
    let manager = GigasttSidecar::new(config).unwrap();
    manager.ensure_ready().await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let diagnostics = manager.diagnostics().await;
    assert!(!diagnostics.is_empty());
    assert!(diagnostics.len() <= 3);
    assert!(diagnostics
        .iter()
        .any(|entry| entry.kind == DiagnosticKind::ChildOutput));
    let rendered = format!("{diagnostics:?}");
    assert!(!rendered.contains("PRIVATE_TRANSCRIPT_PAYLOAD"));
    assert!(!rendered.contains("transcript="));
    assert!(!rendered.contains(r#"\"text\""#));
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_existing_ready_server_on_the_explicit_port_is_not_adopted() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let external = tokio::spawn(async move {
        for _ in 0..8 {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await;
            let body = r#"{"status":"ready","pool_available":1,"pool_total":1}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        }
    });
    let temp = TempDir::new().unwrap();
    control(&temp, "fake-bind-failure-delay-ms", "400");
    let manager = GigasttSidecar::new(test_config(&temp, Some(port))).unwrap();

    let result = manager.ensure_ready().await;
    assert!(matches!(result, Err(SidecarError::PortUnavailable { port: value }) if value == port));
    assert_eq!(manager.status().await, SidecarStatus::Failed);
    assert_eq!(
        launch_count(&temp),
        0,
        "an occupied port must be rejected before spawn"
    );
    external.abort();
}

#[tokio::test]
async fn startup_deadline_is_not_extended_by_a_slow_readiness_response() {
    let temp = TempDir::new().unwrap();
    control(&temp, "fake-response-delay-ms", "500");
    let config = GigasttSidecarConfig::new(fake_binary(), model_dir(&temp), None)
        .with_startup_timeout(Duration::from_millis(100))
        .with_poll_interval(Duration::from_millis(10))
        .with_request_timeout(Duration::from_millis(700))
        .with_shutdown_timeouts(Duration::from_millis(100), Duration::from_millis(200));
    let manager = GigasttSidecar::new(config).unwrap();

    let started = Instant::now();
    assert!(matches!(
        manager.ensure_ready().await,
        Err(SidecarError::StartupTimeout { .. })
    ));
    assert!(
        started.elapsed() < Duration::from_millis(300),
        "readiness request overran the manager deadline: {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn readiness_probe_timeout_does_not_shorten_the_returned_job_client_timeout() {
    let temp = TempDir::new().unwrap();
    control(&temp, "fake-job-response-delay-ms", "120");
    let config = GigasttSidecarConfig::new(fake_binary(), model_dir(&temp), None)
        .with_startup_timeout(Duration::from_millis(500))
        .with_poll_interval(Duration::from_millis(10))
        .with_request_timeout(Duration::from_millis(30))
        .with_shutdown_timeouts(Duration::from_millis(100), Duration::from_millis(200));
    let manager = GigasttSidecar::new(config).unwrap();

    let client = manager.ensure_ready().await.unwrap();
    let submitted = client
        .submit_job(b"RIFF fake", &SubmitJobOptions::default())
        .await
        .expect("job client must retain its normal request timeout");
    assert_eq!(submitted.job_id, "job_1");
    manager.shutdown().await.unwrap();
}

#[tokio::test]
async fn busy_updates_cannot_resurrect_a_failed_supervisor_state() {
    let temp = TempDir::new().unwrap();
    let manager = GigasttSidecar::new(test_config(&temp, None)).unwrap();
    manager.ensure_ready().await.unwrap();
    manager.set_busy(true).await.unwrap();
    // Arm both crashes only after the Ready -> Busy state under test exists.
    // A Windows cold spawn can legitimately consume the old 100 ms window.
    control(&temp, "fake-exit-after-ms", "100,100");

    let updater = {
        let manager = manager.clone();
        tokio::spawn(async move {
            for _ in 0..200 {
                let _ = manager.set_busy(false).await;
                let _ = manager.set_busy(true).await;
                tokio::task::yield_now().await;
            }
        })
    };
    wait_status(&manager, SidecarStatus::Failed).await;
    updater.await.unwrap();
    assert_eq!(manager.status().await, SidecarStatus::Failed);
    assert!(manager.set_busy(false).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn overlapping_shutdowns_keep_new_startup_cancelled_until_reaping_finishes() {
    let temp = TempDir::new().unwrap();
    control(&temp, "fake-ignore-term", "");
    let manager = GigasttSidecar::new(test_config(&temp, Some(reserve_port()))).unwrap();
    manager.ensure_ready().await.unwrap();

    let first = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.shutdown().await })
    };
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if manager
                .diagnostics()
                .await
                .iter()
                .any(|entry| entry.kind == DiagnosticKind::GracefulShutdown)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("graceful shutdown request");
    let second = {
        let manager = manager.clone();
        tokio::spawn(async move { manager.shutdown().await })
    };

    assert!(matches!(
        manager.ensure_ready().await,
        Err(SidecarError::Cancelled)
    ));
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
}

#[tokio::test]
async fn close_is_terminal_while_shutdown_allows_an_explicit_restart() {
    let temp = TempDir::new().unwrap();
    let manager = GigasttSidecar::new(test_config(&temp, None)).unwrap();
    manager.ensure_ready().await.unwrap();
    manager.shutdown().await.unwrap();
    manager.ensure_ready().await.unwrap();
    assert_eq!(launch_count(&temp), 2);

    manager.close().await.unwrap();
    assert!(matches!(
        manager.ensure_ready().await,
        Err(SidecarError::Closed)
    ));
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(launch_count(&temp), 2);
}

#[cfg(unix)]
#[tokio::test]
async fn shutdown_is_bounded_when_graceful_sigterm_is_ignored() {
    let temp = TempDir::new().unwrap();
    control(&temp, "fake-ignore-term", "");
    let manager = GigasttSidecar::new(test_config(&temp, None)).unwrap();
    manager.ensure_ready().await.unwrap();
    let child_pid = pids(&temp)[0];

    let started = Instant::now();
    manager.shutdown().await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    wait_until(|| !process_exists(child_pid), "forced child reap").await;
}

#[cfg(unix)]
#[tokio::test]
async fn dropping_the_last_manager_handle_kills_the_owned_process() {
    let temp = TempDir::new().unwrap();
    let manager = GigasttSidecar::new(test_config(&temp, None)).unwrap();
    manager.ensure_ready().await.unwrap();
    let child_pid = pids(&temp)[0];
    assert!(process_exists(child_pid));

    drop(manager);
    wait_until(|| !process_exists(child_pid), "drop kill backup").await;
}

#[tokio::test]
async fn missing_binary_remains_not_installed() {
    let temp = TempDir::new().unwrap();
    let config =
        GigasttSidecarConfig::new(temp.path().join("missing-gigastt"), model_dir(&temp), None);
    let manager = GigasttSidecar::new(config).unwrap();
    assert!(matches!(
        manager.ensure_ready().await,
        Err(SidecarError::BinaryNotFound { .. })
    ));
    assert_eq!(manager.status().await, SidecarStatus::NotInstalled);
}
