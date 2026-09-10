//! Process-level fake for the GigaSTT sidecar lifecycle contract tests.
//!
//! Test controls are files in `--model-dir`; the production manager passes no
//! test-only arguments or environment variables to the child.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
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

#[derive(Debug)]
struct StagedIoError {
    stage: &'static str,
}

impl std::fmt::Display for StagedIoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.stage)
    }
}

impl std::error::Error for StagedIoError {}

fn staged_io_error(kind: io::ErrorKind, stage: &'static str) -> io::Error {
    io::Error::new(kind, StagedIoError { stage })
}

fn error_stage(error: &io::Error) -> &'static str {
    error
        .get_ref()
        .and_then(|source| source.downcast_ref::<StagedIoError>())
        .map_or("transport", |error| error.stage)
}

fn is_expected_peer_disconnect(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::NotConnected
            | io::ErrorKind::TimedOut
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::WouldBlock
            | io::ErrorKind::WriteZero
    )
}

fn is_expected_accept_disconnect(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset
    )
}

fn record_transport(model_dir: &Path, outcome: &str, stage: &'static str, kind: io::ErrorKind) {
    append(
        &model_dir.join("fake-transport-diagnostics"),
        &format!("outcome={outcome} stage={stage} kind={kind:?}"),
    );
}

fn record_fixture_failure(model_dir: &Path, stage: &'static str, kind: io::ErrorKind) {
    record_transport(model_dir, "failure", stage, kind);
    append(
        &model_dir.join("fake-failures"),
        &format!("stage={stage} kind={kind:?}"),
    );
}

fn fail_fixture(model_dir: &Path, stage: &'static str, kind: io::ErrorKind) -> ! {
    record_fixture_failure(model_dir, stage, kind);
    panic!("fake fixture failure: stage={stage} kind={kind:?}");
}

fn handle_peer_error(model_dir: &Path, error: io::Error) {
    let stage = error_stage(&error);
    if !is_expected_peer_disconnect(error.kind()) {
        fail_fixture(model_dir, stage, error.kind());
    }
    record_transport(model_dir, "disconnect", stage, error.kind());
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
fn read_request(stream: &mut impl Read) -> std::io::Result<Request> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 4096];
    let header_end = loop {
        let count = read_request_bytes(stream, &mut buffer, "request_headers")?;
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
    };
    let headers = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| staged_io_error(io::ErrorKind::InvalidData, "request_headers_utf8"))?;
    let mut lines = headers.split("\r\n");
    let mut request_line = lines
        .next()
        .ok_or_else(|| staged_io_error(io::ErrorKind::InvalidData, "request_line"))?
        .split_whitespace();
    let method = request_line
        .next()
        .ok_or_else(|| staged_io_error(io::ErrorKind::InvalidData, "request_line"))?
        .to_string();
    let target = request_line
        .next()
        .ok_or_else(|| staged_io_error(io::ErrorKind::InvalidData, "request_line"))?
        .to_string();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let content_length = headers
        .get("content-length")
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| staged_io_error(io::ErrorKind::InvalidData, "content_length"))
        })
        .transpose()?
        .unwrap_or(0);
    let body_end = header_end
        .checked_add(content_length)
        .ok_or_else(|| staged_io_error(io::ErrorKind::InvalidData, "content_length"))?;
    while bytes.len() < body_end {
        let count = read_request_bytes(stream, &mut buffer, "request_body")?;
        bytes.extend_from_slice(&buffer[..count]);
    }
    Ok(Request {
        method,
        target,
        body: bytes[header_end..body_end].to_vec(),
    })
}

fn read_request_bytes(
    stream: &mut impl Read,
    buffer: &mut [u8],
    stage: &'static str,
) -> io::Result<usize> {
    loop {
        match stream.read(buffer) {
            Ok(0) => return Err(staged_io_error(io::ErrorKind::UnexpectedEof, stage)),
            Ok(count) => return Ok(count),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(staged_io_error(error.kind(), stage)),
        }
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

fn response(stream: &mut impl Write, status: u16, body: &str) -> io::Result<()> {
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
    stream
        .write_all(response.as_bytes())
        .map_err(|error| staged_io_error(error.kind(), "response_write"))?;
    stream
        .flush()
        .map_err(|error| staged_io_error(error.kind(), "response_flush"))
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
            record_fixture_failure(&model_dir, "listener_bind", error.kind());
            append(
                &model_dir.join("fake-bind-errors"),
                &format!("{launch}:{:?}", error.kind()),
            );
            if let Some(delay) = per_launch_millis(&model_dir, "fake-bind-failure-delay-ms", launch)
            {
                thread::sleep(Duration::from_millis(delay));
            }
            std::process::exit(73);
        }
    };
    if let Err(error) = listener.set_nonblocking(true) {
        fail_fixture(&model_dir, "listener_nonblocking", error.kind());
    }

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
                if let Err(error) = stream.set_read_timeout(Some(Duration::from_secs(2))) {
                    fail_fixture(&model_dir, "request_timeout", error.kind());
                }
                let request = match read_request(&mut stream) {
                    Ok(request) => request,
                    Err(error) => {
                        handle_peer_error(&model_dir, error);
                        continue;
                    }
                };
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
                if let Err(error) = response(&mut stream, status, &body) {
                    handle_peer_error(&model_dir, error);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if is_expected_accept_disconnect(error.kind()) => {
                record_transport(&model_dir, "disconnect", "listener_accept", error.kind());
            }
            Err(error) => fail_fixture(&model_dir, "listener_accept", error.kind()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        error_stage, handle_peer_error, is_expected_accept_disconnect, is_expected_peer_disconnect,
        read_request, response, staged_io_error,
    };
    use std::fs;
    use std::io::{Cursor, Error, ErrorKind, Read, Result, Write};

    struct ResetReader;

    impl Read for ResetReader {
        fn read(&mut self, _buffer: &mut [u8]) -> Result<usize> {
            Err(Error::new(ErrorKind::ConnectionReset, "private os detail"))
        }
    }

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
    fn a_disconnected_response_is_an_expected_transport_error() {
        let error =
            response(&mut DisconnectedClient, 503, r#"{"status":"not_ready"}"#).unwrap_err();

        assert_eq!(error.kind(), ErrorKind::BrokenPipe);
        assert_eq!(error_stage(&error), "response_write");
        assert!(is_expected_peer_disconnect(error.kind()));
    }

    #[test]
    fn header_eof_is_reported_as_a_transport_disconnect() {
        let mut input = Cursor::new(b"GET /ready HTTP/1.1\r\nHost: localhost\r\n");

        let error = match read_request(&mut input) {
            Ok(_) => panic!("incomplete headers were accepted"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), ErrorKind::UnexpectedEof);
        assert_eq!(error_stage(&error), "request_headers");
        assert!(is_expected_peer_disconnect(error.kind()));
    }

    #[test]
    fn body_eof_is_reported_as_a_transport_disconnect() {
        let mut input =
            Cursor::new(b"POST /v1/jobs HTTP/1.1\r\nContent-Length: 9\r\n\r\nRIFF".as_slice());

        let error = match read_request(&mut input) {
            Ok(_) => panic!("incomplete body was accepted"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), ErrorKind::UnexpectedEof);
        assert_eq!(error_stage(&error), "request_body");
        assert!(is_expected_peer_disconnect(error.kind()));
    }

    #[test]
    fn connection_reset_is_reported_without_leaking_os_details() {
        let error = match read_request(&mut ResetReader) {
            Ok(_) => panic!("reset connection was accepted"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), ErrorKind::ConnectionReset);
        assert_eq!(error_stage(&error), "request_headers");
        assert!(is_expected_peer_disconnect(error.kind()));
        assert!(!error.to_string().contains("private os detail"));
    }

    #[test]
    fn complete_request_body_is_preserved() {
        let body = vec![b'a'; 8193];
        let mut bytes = format!(
            "POST /v1/jobs?language=ru HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        bytes.extend_from_slice(&body);
        let mut input = Cursor::new(bytes);

        let request = read_request(&mut input).unwrap();

        assert_eq!(request.method, "POST");
        assert_eq!(request.target, "/v1/jobs?language=ru");
        assert_eq!(request.body, body);
    }

    #[test]
    fn malformed_request_line_remains_a_fixture_failure() {
        let mut input = Cursor::new(b"GET\r\n\r\n");

        let error = match read_request(&mut input) {
            Ok(_) => panic!("malformed request line was accepted"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert_eq!(error_stage(&error), "request_line");
        assert!(!is_expected_peer_disconnect(error.kind()));
    }

    #[test]
    fn only_peer_disconnect_errors_are_continuable() {
        for kind in [
            ErrorKind::BrokenPipe,
            ErrorKind::ConnectionAborted,
            ErrorKind::ConnectionReset,
            ErrorKind::NotConnected,
            ErrorKind::TimedOut,
            ErrorKind::UnexpectedEof,
            ErrorKind::WouldBlock,
            ErrorKind::WriteZero,
        ] {
            assert!(is_expected_peer_disconnect(kind), "{kind:?}");
        }
        for kind in [
            ErrorKind::AddrInUse,
            ErrorKind::InvalidData,
            ErrorKind::PermissionDenied,
        ] {
            assert!(!is_expected_peer_disconnect(kind), "{kind:?}");
        }
    }

    #[test]
    fn accept_only_continues_for_windows_queued_peer_disconnects() {
        assert!(is_expected_accept_disconnect(ErrorKind::ConnectionReset));
        assert!(is_expected_accept_disconnect(ErrorKind::ConnectionAborted));
        assert!(!is_expected_accept_disconnect(ErrorKind::WouldBlock));
        assert!(!is_expected_accept_disconnect(ErrorKind::PermissionDenied));
    }

    #[test]
    fn expected_disconnect_diagnostic_contains_only_stage_and_kind() {
        let temp = tempfile::tempdir().unwrap();

        handle_peer_error(
            temp.path(),
            staged_io_error(ErrorKind::ConnectionReset, "request_headers"),
        );

        assert_eq!(
            fs::read_to_string(temp.path().join("fake-transport-diagnostics")).unwrap(),
            "outcome=disconnect stage=request_headers kind=ConnectionReset\n"
        );
        assert!(!temp.path().join("fake-failures").exists());
    }

    #[test]
    fn malformed_http_records_a_sanitized_fixture_failure() {
        let temp = tempfile::tempdir().unwrap();

        let failure = std::panic::catch_unwind(|| {
            handle_peer_error(
                temp.path(),
                staged_io_error(ErrorKind::InvalidData, "request_line"),
            );
        });

        assert!(failure.is_err());
        assert_eq!(
            fs::read_to_string(temp.path().join("fake-failures")).unwrap(),
            "stage=request_line kind=InvalidData\n"
        );
    }
}
