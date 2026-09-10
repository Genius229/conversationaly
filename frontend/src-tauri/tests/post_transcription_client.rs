use std::{
    collections::{BTreeSet, HashMap},
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use app_lib::audio::post_transcription::{
    GigasttClient, GigasttClientError, GigasttJobStatus, PostTranscriptionState, ReadinessStatus,
    SubmitJobOptions,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct FakeResponse {
    status: u16,
    body: String,
    content_type: &'static str,
    headers: Vec<(&'static str, String)>,
}

impl FakeResponse {
    fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.to_string(),
            content_type: "application/json",
            headers: Vec::new(),
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            body: String::new(),
            content_type: "application/json",
            headers: Vec::new(),
        }
    }

    fn with_header(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.headers.push((name, value.into()));
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordedRequest {
    method: String,
    target: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

/// Start a tiny HTTP/1.1 responder which records the request line, headers, and
/// fixed-length body so the client wire contract can be asserted directly.
struct ServerTask(tokio::task::JoinHandle<()>);

impl Future for ServerTask {
    type Output = Result<(), tokio::task::JoinError>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

impl Drop for ServerTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn fake_server(
    responses: Vec<FakeResponse>,
) -> (SocketAddr, Arc<Mutex<Vec<RecordedRequest>>>, ServerTask) {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind fake server");
    let addr = listener.local_addr().expect("local addr");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), async move {
            for response in responses {
                let (mut stream, _) = listener.accept().await.expect("accept");
                let mut request = Vec::new();
                let mut buf = [0_u8; 1024];
                let header_end = loop {
                    let read = stream.read(&mut buf).await.expect("read request");
                    assert_ne!(read, 0, "connection closed before request headers");
                    request.extend_from_slice(&buf[..read]);
                    if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                        break index + 4;
                    }
                };
                let header_text = std::str::from_utf8(&request[..header_end])
                    .expect("request headers are utf-8");
                let mut lines = header_text.split("\r\n");
                let request_line = lines.next().expect("request line");
                let mut request_parts = request_line.split_whitespace();
                let method = request_parts.next().expect("request method").to_string();
                let target = request_parts.next().expect("request target").to_string();
                let headers: HashMap<String, String> = lines
                    .filter_map(|line| line.split_once(':'))
                    .map(|(name, value)| {
                        (name.trim().to_ascii_lowercase(), value.trim().to_string())
                    })
                    .collect();
                let content_length = headers
                    .get("content-length")
                    .map(|value| value.parse::<usize>().expect("numeric content-length"))
                    .unwrap_or(0);
                while request.len() < header_end + content_length {
                    let read = stream.read(&mut buf).await.expect("read request body");
                    assert_ne!(read, 0, "connection closed before request body");
                    request.extend_from_slice(&buf[..read]);
                }
                recorded.lock().expect("request recorder").push(RecordedRequest {
                    method,
                    target,
                    headers,
                    body: request[header_end..header_end + content_length].to_vec(),
                });

                let reason = match response.status {
                    200 => "OK",
                    202 => "Accepted",
                    204 => "No Content",
                    302 => "Found",
                    400 => "Bad Request",
                    404 => "Not Found",
                    409 => "Conflict",
                    429 => "Too Many Requests",
                    500 => "Internal Server Error",
                    503 => "Service Unavailable",
                    _ => "Test",
                };
                let payload = response.body.as_bytes();
                let mut response_headers = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    response.status,
                    reason,
                    response.content_type,
                    payload.len()
                );
                for (name, value) in response.headers {
                    response_headers.push_str(&format!("{name}: {value}\r\n"));
                }
                response_headers.push_str("\r\n");
                stream
                    .write_all(response_headers.as_bytes())
                    .await
                    .expect("write headers");
                stream.write_all(payload).await.expect("write body");
            }
        })
        .await
        .expect("fake server deadline");
    });
    (addr, requests, ServerTask(task))
}

fn client(addr: SocketAddr) -> GigasttClient {
    GigasttClient::new(format!("http://{addr}")).expect("client")
}

#[tokio::test]
async fn ready_parses_ready_and_not_ready_payloads() {
    let (addr, _requests, task) = fake_server(vec![
        FakeResponse::json(
            200,
            r#"{"status":"ready","pool_available":1,"pool_total":1}"#,
        ),
        FakeResponse::json(503, r#"{"status":"not_ready","reason":"initializing"}"#),
    ])
    .await;
    let c = client(addr);

    let ready = c.ready().await.expect("ready response");
    assert_eq!(ready.status, ReadinessStatus::Ready);
    assert_eq!(ready.pool_available, Some(1));
    let not_ready = c.ready().await.expect("not-ready is a valid poll result");
    assert_eq!(not_ready.status, ReadinessStatus::NotReady);
    assert_eq!(not_ready.reason.as_deref(), Some("initializing"));
    task.await.expect("server task");
}

#[tokio::test]
async fn submit_progress_and_result_are_typed() {
    let (addr, requests, task) = fake_server(vec![
        FakeResponse::json(
            202,
            r#"{"job_id":"job_123","status":"queued","created_at":1712700000.5}"#,
        ),
        FakeResponse::json(
            200,
            r#"{"job_id":"job_123","status":"processing","processed_seconds":4.0,"percent":42}"#,
        ),
        FakeResponse::json(
            200,
            r#"{"job_id":"job_123","status":"done","processed_seconds":10.0,"percent":0}"#,
        ),
        FakeResponse::json(
            200,
            r#"{"text":"Привет, мир!","words":[{"word":"Привет","start":0.0,"end":0.5,"confidence":0.9}],"duration":1.0,"segments":[{"start":0.0,"end":0.5,"text":"Привет","words":[{"word":"Привет","start":0.0,"end":0.5,"confidence":0.9}]}] }"#,
        ),
    ])
    .await;
    let c = client(addr);

    let submitted = c
        .submit_job(b"RIFF fake wav", &SubmitJobOptions::default())
        .await
        .expect("submit");
    assert_eq!(submitted.job_id, "job_123");
    assert_eq!(submitted.status, GigasttJobStatus::Queued);
    assert_eq!(c.get_job("job_123").await.expect("progress").percent, 42);
    let done = c.get_job("job_123").await.expect("done");
    assert_eq!(done.status, GigasttJobStatus::Done);
    assert_eq!(
        done.percent, 0,
        "terminal status, not percent, drives completion"
    );
    let result = c.get_result("job_123").await.expect("result");
    assert_eq!(result.text, "Привет, мир!");
    assert_eq!(result.words[0].start, 0.0);
    task.await.expect("server task");

    let requests = requests.lock().expect("request recorder");
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].method, "POST");
    let (submit_path, query) = requests[0].target.split_once('?').expect("submit query");
    assert_eq!(submit_path, "/v1/jobs");
    assert_eq!(
        query.split('&').collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "diarization=false",
            "format=json",
            "itn=true",
            "punctuation=true",
            "segments=true",
            "vad=true",
            "word_timestamps=true",
        ])
    );
    assert_eq!(
        requests[0].headers.get("content-type").map(String::as_str),
        Some("application/octet-stream")
    );
    assert_eq!(requests[0].body, b"RIFF fake wav");
    assert_eq!(requests[1].method, "GET");
    assert_eq!(requests[1].target, "/v1/jobs/job_123");
    assert_eq!(requests[2].method, "GET");
    assert_eq!(requests[2].target, "/v1/jobs/job_123");
    assert_eq!(requests[3].method, "GET");
    assert_eq!(requests[3].target, "/v1/jobs/job_123/result");
}

#[tokio::test]
async fn failed_job_and_cancel_are_reported() {
    let (addr, requests, task) = fake_server(vec![
        FakeResponse::json(
            200,
            r#"{"job_id":"job_fail","status":"failed","processed_seconds":2.0,"percent":20,"error":"decoder failed"}"#,
        ),
        FakeResponse::empty(204),
    ])
    .await;
    let c = client(addr);
    let failed = c.get_job("job_fail").await.expect("failed status");
    assert_eq!(failed.status, GigasttJobStatus::Failed);
    assert_eq!(failed.error.as_deref(), Some("decoder failed"));
    c.cancel_job("job_fail").await.expect("cancel");
    task.await.expect("server task");
    let requests = requests.lock().expect("request recorder");
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].target, "/v1/jobs/job_fail");
    assert_eq!(requests[1].method, "DELETE");
    assert_eq!(requests[1].target, "/v1/jobs/job_fail");
}

#[tokio::test]
async fn malformed_json_and_oversized_http_body_fail_closed() {
    let (addr, _requests, task) = fake_server(vec![
        FakeResponse::json(200, "{not-json"),
        FakeResponse {
            status: 500,
            body: "x".repeat(64 * 1024 + 1),
            content_type: "text/plain",
            headers: Vec::new(),
        },
    ])
    .await;
    let c = client(addr);
    assert!(matches!(
        c.ready().await,
        Err(GigasttClientError::InvalidResponse(_))
    ));
    assert!(matches!(
        c.cancel_job("job_123").await,
        Err(GigasttClientError::BodyTooLarge { limit }) if limit == 64 * 1024
    ));
    task.await.expect("server task");
}

#[test]
fn post_transcription_state_covers_required_stages() {
    let states = [
        PostTranscriptionState::PreparingAudio,
        PostTranscriptionState::Starting,
        PostTranscriptionState::Transcribing { percent: 46 },
        PostTranscriptionState::Finalizing,
        PostTranscriptionState::Ready,
        PostTranscriptionState::Failed {
            message: "boom".into(),
        },
        PostTranscriptionState::Cancelled,
    ];
    assert_eq!(states.len(), 7);
    assert!(matches!(
        PostTranscriptionState::Transcribing { percent: 46 },
        PostTranscriptionState::Transcribing { percent: 46 }
    ));
}

#[test]
fn rejects_non_loopback_and_ambiguous_urls() {
    for url in [
        "http://example.com",
        "https://127.0.0.1:1234",
        "http://localhost:1234",
        "http://user:password@127.0.0.1",
        "http://127.0.0.1/path",
        "http://127.0.0.1?x=1",
        "http://127.0.0.1#fragment",
    ] {
        assert!(GigasttClient::new(url).is_err(), "accepted {url}");
    }
}

#[tokio::test]
async fn readiness_http_errors_are_typed_and_redacted() {
    let (addr, _requests, task) = fake_server(vec![FakeResponse::json(
        500,
        r#"{"error":"private transcript content","code":"inference_failed"}"#,
    )])
    .await;
    let result = client(addr).ready().await;
    task.await.unwrap();
    assert!(matches!(
        result,
        Err(GigasttClientError::Http { status: 500, .. })
    ));
    assert!(!result
        .unwrap_err()
        .to_string()
        .contains("private transcript"));
}

#[tokio::test]
async fn rejects_mismatched_job_and_nested_word_outside_segment() {
    let (addr, _requests, task) = fake_server(vec![
        FakeResponse::json(200, r#"{"job_id":"other","status":"done","processed_seconds":1,"percent":100}"#),
        FakeResponse::json(200, r#"{"text":"hi","duration":2,"words":[],"segments":[{"text":"hi","start":0,"end":1,"words":[{"word":"hi","start":0.8,"end":1.2,"confidence":0.9}]}]}"#),
    ]).await;
    let c = client(addr);
    let job = c.get_job("expected").await;
    let result = c.get_result("expected").await;
    task.await.unwrap();
    assert!(job.is_err());
    assert!(result.is_err());
}

#[tokio::test]
async fn rejects_nonempty_segment_without_words() {
    let (addr, _requests, task) = fake_server(vec![FakeResponse::json(
        200,
        r#"{"text":"hi","duration":1,"words":[],"segments":[{"text":"hi","start":0,"end":1,"words":[]}]}"#,
    )])
    .await;
    let result = client(addr).get_result("expected").await;
    task.await.unwrap();
    assert!(matches!(
        result,
        Err(GigasttClientError::InvalidResponse(_))
    ));
}

#[tokio::test]
async fn validates_optional_confidence_when_present() {
    let (addr, _requests, task) = fake_server(vec![FakeResponse::json(
        200,
        r#"{"text":"hi","duration":1,"words":[{"word":"hi","start":0,"end":1,"confidence":1.1}]}"#,
    )])
    .await;
    let result = client(addr).get_result("expected").await;
    task.await.unwrap();
    assert!(matches!(
        result,
        Err(GigasttClientError::InvalidResponse(_))
    ));
}

#[tokio::test]
async fn accepts_missing_optional_confidence() {
    let (addr, _requests, task) = fake_server(vec![FakeResponse::json(
        200,
        r#"{"text":"hi","duration":1,"words":[{"word":"hi","start":0,"end":1}]}"#,
    )])
    .await;
    let result = client(addr).get_result("expected").await;
    task.await.unwrap();
    assert_eq!(
        result.expect("optional confidence").words[0].confidence,
        None
    );
}

#[tokio::test]
async fn oversized_error_on_ready_obeys_error_cap() {
    let (addr, _requests, task) =
        fake_server(vec![FakeResponse::json(500, &"x".repeat(65537))]).await;
    let result = client(addr).ready().await;
    task.await.unwrap();
    assert!(matches!(
        result,
        Err(GigasttClientError::BodyTooLarge { limit: 65536 })
    ));
}

#[tokio::test]
async fn does_not_follow_redirects() {
    let redirect_target = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind redirect target");
    let target_addr = redirect_target.local_addr().expect("redirect target addr");
    let contacted = Arc::new(AtomicBool::new(false));
    let contacted_by_task = contacted.clone();
    let target_task = tokio::spawn(async move {
        if let Ok(Ok((mut stream, _))) =
            tokio::time::timeout(Duration::from_millis(250), redirect_target.accept()).await
        {
            contacted_by_task.store(true, Ordering::SeqCst);
            let response = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 54\r\nConnection: close\r\n\r\n{\"status\":\"ready\",\"pool_available\":1,\"pool_total\":1}";
            stream.write_all(response).await.expect("redirect response");
        }
    });
    let (addr, _requests, task) = fake_server(vec![
        FakeResponse::empty(302).with_header("Location", format!("http://{target_addr}/ready"))
    ])
    .await;

    let result = client(addr).ready().await;
    task.await.unwrap();
    target_task.await.unwrap();

    assert!(matches!(
        result,
        Err(GigasttClientError::Http { status: 302, .. })
    ));
    assert!(!contacted.load(Ordering::SeqCst));
}

#[tokio::test]
async fn request_timeout_bounds_a_stalled_response() {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind stalled server");
    let addr = listener.local_addr().expect("stalled server addr");
    let task = ServerTask(tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(10), async move {
            let (_stream, _) = listener.accept().await.expect("accept stalled request");
            tokio::time::sleep(Duration::from_millis(250)).await;
        })
        .await
        .expect("stalled server deadline");
    }));
    let client = GigasttClient::with_timeout(format!("http://{addr}"), Duration::from_millis(50))
        .expect("client");

    let started = Instant::now();
    let result = client.ready().await;
    let elapsed = started.elapsed();
    task.await.unwrap();

    assert!(matches!(result, Err(GigasttClientError::Transport(_))));
    assert!(elapsed < Duration::from_millis(200), "elapsed: {elapsed:?}");
}
