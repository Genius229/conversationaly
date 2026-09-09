use std::net::SocketAddr;

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
}

impl FakeResponse {
    fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.to_string(),
            content_type: "application/json",
        }
    }

    fn empty(status: u16) -> Self {
        Self {
            status,
            body: String::new(),
            content_type: "application/json",
        }
    }
}

/// Start a tiny one-shot HTTP/1.1 responder.  The test intentionally parses
/// only request boundaries: the client contract is exercised through status,
/// query path, and JSON response validation, not through a second HTTP stack.
async fn fake_server(responses: Vec<FakeResponse>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind fake server");
    let addr = listener.local_addr().expect("local addr");
    let task = tokio::spawn(async move {
        for response in responses {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = Vec::new();
            let mut buf = [0_u8; 1024];
            loop {
                let read = stream.read(&mut buf).await.expect("read request");
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let reason = match response.status {
                200 => "OK",
                202 => "Accepted",
                204 => "No Content",
                400 => "Bad Request",
                404 => "Not Found",
                409 => "Conflict",
                429 => "Too Many Requests",
                500 => "Internal Server Error",
                503 => "Service Unavailable",
                _ => "Test",
            };
            let payload = response.body.as_bytes();
            let headers = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.status,
                reason,
                response.content_type,
                payload.len()
            );
            stream
                .write_all(headers.as_bytes())
                .await
                .expect("write headers");
            stream.write_all(payload).await.expect("write body");
        }
    });
    (addr, task)
}

fn client(addr: SocketAddr) -> GigasttClient {
    GigasttClient::new(format!("http://{addr}")).expect("client")
}

#[tokio::test]
async fn ready_parses_ready_and_not_ready_payloads() {
    let (addr, task) = fake_server(vec![
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
    let (addr, task) = fake_server(vec![
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
            r#"{"job_id":"job_123","status":"done","processed_seconds":10.0,"percent":100}"#,
        ),
        FakeResponse::json(
            200,
            r#"{"text":"Привет, мир!","words":[{"word":"Привет","start":0.0,"end":0.5,"confidence":0.9}],"duration":1.0,"segments":[{"start":0.0,"end":1.0,"text":"Привет, мир!","words":[]}] }"#,
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
    assert_eq!(
        c.get_job("job_123").await.expect("done").status,
        GigasttJobStatus::Done
    );
    let result = c.get_result("job_123").await.expect("result");
    assert_eq!(result.text, "Привет, мир!");
    assert_eq!(result.words[0].start, 0.0);
    task.await.expect("server task");
}

#[tokio::test]
async fn failed_job_and_cancel_are_reported() {
    let (addr, task) = fake_server(vec![
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
}

#[tokio::test]
async fn malformed_json_and_oversized_http_body_fail_closed() {
    let (addr, task) = fake_server(vec![
        FakeResponse::json(200, "{not-json"),
        FakeResponse {
            status: 500,
            body: "x".repeat(64 * 1024 + 1),
            content_type: "text/plain",
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
        Err(GigasttClientError::BodyTooLarge { limit: 64 * 1024 })
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
