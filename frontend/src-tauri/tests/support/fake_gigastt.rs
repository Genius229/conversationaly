//! Process-level fake for the GigaSTT sidecar lifecycle contract tests.
//!
//! Test controls are files in `--model-dir`; the production manager passes no
//! test-only arguments or environment variables to the child.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

fn flag_value(args: &[String], flag: &str) -> String {
    let index = args
        .iter()
        .position(|arg| arg == flag)
        .unwrap_or_else(|| panic!("missing {flag}"));
    args.get(index + 1)
        .unwrap_or_else(|| panic!("missing value for {flag}"))
        .clone()
}

fn append(path: &Path, value: &str) {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open fake output");
    writeln!(file, "{value}").expect("write fake output");
    file.flush().expect("flush fake output");
}

fn launch_number(model_dir: &Path) -> usize {
    let path = model_dir.join("fake-launch-count");
    let previous = fs::read_to_string(&path)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let next = previous + 1;
    fs::write(path, next.to_string()).expect("write launch count");
    next
}

fn per_launch_millis(model_dir: &Path, name: &str, launch: usize) -> Option<u64> {
    let values = fs::read_to_string(model_dir.join(name)).ok()?;
    let value = values.split(',').nth(launch.saturating_sub(1))?.trim();
    match value {
        "" | "none" | "never" | "0" => None,
        value => value.parse().ok(),
    }
}

#[cfg(unix)]
fn ignore_sigterm_if_requested(model_dir: &Path) {
    if !model_dir.join("fake-ignore-term").exists() {
        return;
    }
    unsafe extern "C" {
        fn signal(signal: i32, handler: usize) -> usize;
    }
    const SIGTERM: i32 = 15;
    const SIG_IGN: usize = 1;
    // SAFETY: installs the platform-defined ignore disposition for SIGTERM.
    unsafe {
        signal(SIGTERM, SIG_IGN);
    }
}

#[cfg(not(unix))]
fn ignore_sigterm_if_requested(_model_dir: &Path) {}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let model_dir = PathBuf::from(flag_value(&args, "--model-dir"));
    fs::create_dir_all(&model_dir).expect("create model dir");
    let launch = launch_number(&model_dir);
    append(&model_dir.join("fake-args"), &args.join("\u{1f}"));
    append(
        &model_dir.join("fake-pids"),
        &std::process::id().to_string(),
    );

    // Deliberately hostile output proves the manager never retains child text.
    for _ in 0..8 {
        println!("transcript=PRIVATE_TRANSCRIPT_PAYLOAD");
        eprintln!(r#"{{"text":"PRIVATE_TRANSCRIPT_PAYLOAD"}}"#);
    }

    ignore_sigterm_if_requested(&model_dir);

    let host = flag_value(&args, "--host");
    let port: u16 = flag_value(&args, "--port").parse().expect("valid port");
    let listener = match TcpListener::bind((host.as_str(), port)) {
        Ok(listener) => listener,
        Err(error) => {
            append(
                &model_dir.join("fake-bind-errors"),
                &format!("{launch}:{error}"),
            );
            if let Some(delay) = per_launch_millis(&model_dir, "fake-bind-failure-delay-ms", launch)
            {
                thread::sleep(Duration::from_millis(delay));
            }
            std::process::exit(73);
        }
    };
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");

    let ready_delay = per_launch_millis(&model_dir, "fake-ready-delay-ms", launch)
        .map(Duration::from_millis)
        .unwrap_or_default();
    let never_ready = model_dir.join("fake-never-ready").exists();
    let exit_after =
        per_launch_millis(&model_dir, "fake-exit-after-ms", launch).map(Duration::from_millis);
    let response_delay = per_launch_millis(&model_dir, "fake-response-delay-ms", launch)
        .map(Duration::from_millis)
        .unwrap_or_default();
    let job_response_delay = per_launch_millis(&model_dir, "fake-job-response-delay-ms", launch)
        .map(Duration::from_millis)
        .unwrap_or_default();
    let started = Instant::now();

    loop {
        if exit_after.is_some_and(|limit| started.elapsed() >= limit) {
            std::process::exit(42);
        }

        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                let mut request = [0_u8; 4096];
                let count = stream.read(&mut request).unwrap_or_default();
                let is_job = request[..count].starts_with(b"POST /v1/jobs?");
                thread::sleep(if is_job {
                    job_response_delay
                } else {
                    response_delay
                });
                let ready = !never_ready && started.elapsed() >= ready_delay;
                let (status, reason, body) = if is_job {
                    (
                        202,
                        "Accepted",
                        r#"{"job_id":"job_1","status":"queued","created_at":1}"#,
                    )
                } else if ready {
                    (
                        200,
                        "OK",
                        r#"{"status":"ready","pool_available":1,"pool_total":1}"#,
                    )
                } else {
                    (
                        503,
                        "Service Unavailable",
                        r#"{"status":"not_ready","reason":"initializing"}"#,
                    )
                };
                let response = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("fake accept failed: {error}"),
        }
    }
}
