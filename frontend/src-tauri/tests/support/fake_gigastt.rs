//! Process-level fake for the GigaSTT sidecar lifecycle contract tests.
//!
//! Test controls are files in `--model-dir`; the production manager passes no
//! test-only arguments or environment variables to the child.

use std::collections::HashMap;
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

fn control_text(model_dir: &Path, name: &str) -> Option<String> {
    fs::read_to_string(model_dir.join(name)).ok()
}

struct Request {
    method: String,
    target: String,
    body: Vec<u8>,
}

/// Read the complete fixed-length request. Prepared meeting WAVs are normally
/// larger than the first socket buffer; stopping at the headers would let
/// client regressions pass while the fake silently discarded most audio.
fn read_request(stream: &mut std::net::TcpStream) -> Request {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set fake request timeout");
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut buffer).expect("read fake request");
        assert_ne!(count, 0, "connection closed before request headers");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end]).expect("request headers are utf-8");
    let mut lines = headers.split("\r\n");
    let mut request_line = lines.next().expect("request line").split_whitespace();
    let method = request_line.next().expect("request method").to_string();
    let target = request_line.next().expect("request target").to_string();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let content_length = headers
        .get("content-length")
        .map(|value| value.parse::<usize>().expect("numeric content length"))
        .unwrap_or(0);
    while bytes.len() < header_end + content_length {
        let count = stream.read(&mut buffer).expect("read fake request body");
        assert_ne!(count, 0, "connection closed before request body");
        bytes.extend_from_slice(&buffer[..count]);
    }
    Request {
        method,
        target,
        body: bytes[header_end..header_end + content_length].to_vec(),
    }
}

fn job_status(model_dir: &Path, poll: usize) -> (&'static str, u8) {
    let configured = control_text(model_dir, "fake-job-statuses")
        .unwrap_or_else(|| "processing:50,done:0".to_string());
    let value = configured
        .split(',')
        .nth(poll)
        .or_else(|| configured.split(',').next_back())
        .unwrap_or("done:0")
        .trim();
    let (status, percent) = value.split_once(':').unwrap_or((value, "0"));
    let status = match status {
        "queued" => "queued",
        "processing" => "processing",
        "done" => "done",
        "failed" => "failed",
        "cancelled" => "cancelled",
        other => panic!("unsupported fake job status {other}"),
    };
    (status, percent.parse().expect("fake percent"))
}

fn response(stream: &mut impl Write, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Test",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    // Readiness futures are cancelled at the supervisor deadline. On Windows
    // that can reset this socket while the fake is replying; a disconnected
    // probe must not crash the fake and masquerade as a sidecar process exit.
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
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
    let mut job_polls = 0_usize;

    loop {
        if exit_after.is_some_and(|limit| started.elapsed() >= limit) {
            std::process::exit(42);
        }

        match listener.accept() {
            Ok((mut stream, _)) => {
                let request = read_request(&mut stream);
                append(
                    &model_dir.join("fake-requests"),
                    &format!(
                        "{} {} {}",
                        request.method,
                        request.target,
                        request.body.len()
                    ),
                );
                let is_job = request.method == "POST" && request.target.starts_with("/v1/jobs?");
                let status_delay = if request.method == "GET" && request.target == "/v1/jobs/job_1"
                {
                    control_text(&model_dir, "fake-status-response-delay-ms")
                        .and_then(|value| value.trim().parse().ok())
                        .map(Duration::from_millis)
                } else {
                    None
                };
                thread::sleep(status_delay.unwrap_or(if is_job {
                    job_response_delay
                } else {
                    response_delay
                }));
                let ready = !never_ready && started.elapsed() >= ready_delay;
                let (status, body) = if is_job {
                    append(
                        &model_dir.join("fake-upload-lengths"),
                        &request.body.len().to_string(),
                    );
                    let status = control_text(&model_dir, "fake-submit-http-status")
                        .and_then(|value| value.trim().parse().ok())
                        .unwrap_or(202);
                    let body =
                        control_text(&model_dir, "fake-submit-response").unwrap_or_else(|| {
                            r#"{"job_id":"job_1","status":"queued","created_at":1}"#.to_string()
                        });
                    (status, body)
                } else if request.method == "GET" && request.target == "/v1/jobs/job_1" {
                    let (job_status, percent) = job_status(&model_dir, job_polls);
                    job_polls += 1;
                    (
                        200,
                        format!(
                            r#"{{"job_id":"job_1","status":"{job_status}","processed_seconds":1.0,"percent":{percent},"error":"PRIVATE_TRANSCRIPT_PAYLOAD"}}"#
                        ),
                    )
                } else if request.method == "GET" && request.target == "/v1/jobs/job_1/result" {
                    let status = control_text(&model_dir, "fake-result-http-status")
                        .and_then(|value| value.trim().parse().ok())
                        .unwrap_or(200);
                    let body = control_text(&model_dir, "fake-result.json").unwrap_or_else(|| {
                        r#"{"text":"Финальный текст","words":[{"word":"Финальный","start":0.0,"end":0.5,"confidence":0.95},{"word":"текст","start":0.5,"end":1.0,"confidence":0.94}],"duration":1.0,"segments":[{"start":0.0,"end":1.0,"text":"Финальный текст","words":[{"word":"Финальный","start":0.0,"end":0.5,"confidence":0.95},{"word":"текст","start":0.5,"end":1.0,"confidence":0.94}]}]}"#.to_string()
                    });
                    (status, body)
                } else if request.method == "DELETE" && request.target == "/v1/jobs/job_1" {
                    append(&model_dir.join("fake-cancelled-jobs"), "job_1");
                    let status = control_text(&model_dir, "fake-cancel-http-status")
                        .and_then(|value| value.trim().parse().ok())
                        .unwrap_or(204);
                    (status, String::new())
                } else if ready {
                    (
                        200,
                        r#"{"status":"ready","pool_available":1,"pool_total":1}"#.to_string(),
                    )
                } else {
                    (
                        503,
                        r#"{"status":"not_ready","reason":"initializing"}"#.to_string(),
                    )
                };
                response(&mut stream, status, &body);
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => panic!("fake accept failed: {error}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::response;
    use std::io::{Error, ErrorKind, Result, Write};

    struct DisconnectedClient;

    impl Write for DisconnectedClient {
        fn write(&mut self, _buffer: &[u8]) -> Result<usize> {
            Err(Error::new(ErrorKind::BrokenPipe, "client disconnected"))
        }

        fn flush(&mut self) -> Result<()> {
            Err(Error::new(ErrorKind::BrokenPipe, "client disconnected"))
        }
    }

    #[test]
    fn a_disconnected_probe_does_not_terminate_the_fake_server() {
        response(&mut DisconnectedClient, 503, r#"{"status":"not_ready"}"#);
    }
}
