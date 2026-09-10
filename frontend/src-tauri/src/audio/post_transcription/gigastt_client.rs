//! Small, strict HTTP client for the GigaSTT 2.18 asynchronous jobs API.

use std::time::Duration;

use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;

use super::types::{
    GigasttJobStatus, JobStatusResponse, JobSubmitResponse, ReadinessResponse, ReadinessStatus,
    TranscribeResponse,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_JSON_BYTES: usize = 16 * 1024 * 1024;
const MAX_ERROR_BODY_BYTES: usize = 64 * 1024;

/// Request-level options forwarded as `/v1/jobs` query parameters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SubmitJobOptions {
    pub format: String,
    pub word_timestamps: bool,
    pub segments: bool,
    pub punctuation: bool,
    pub itn: bool,
    pub vad: bool,
    pub diarization: bool,
}

impl Default for SubmitJobOptions {
    fn default() -> Self {
        Self {
            format: "json".to_string(),
            word_timestamps: true,
            segments: true,
            punctuation: true,
            itn: true,
            vad: true,
            diarization: false,
        }
    }
}

/// Errors returned by the typed client.  HTTP error bodies are capped before
/// decoding so an unhealthy sidecar cannot make the app allocate unbounded
/// memory.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum GigasttClientError {
    #[error("invalid GigaSTT base URL: {0}")]
    InvalidBaseUrl(String),
    #[error("invalid GigaSTT job id")]
    InvalidJobId,
    #[error("GigaSTT request failed: {0}")]
    Transport(String),
    #[error("GigaSTT returned HTTP {status}: {message}")]
    Http {
        status: u16,
        code: Option<String>,
        message: String,
        retry_after_ms: Option<u64>,
    },
    #[error("invalid GigaSTT response: {0}")]
    InvalidResponse(String),
    #[error("GigaSTT response body exceeds {limit} bytes")]
    BodyTooLarge { limit: usize },
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    retry_after_ms: Option<u64>,
}

/// HTTP client for one loopback GigaSTT sidecar.
#[derive(Clone, Debug)]
pub struct GigasttClient {
    http: Client,
    base_url: String,
    max_json_bytes: usize,
}

impl GigasttClient {
    /// Construct a client with the default 30-second request timeout.
    pub fn new(base_url: impl Into<String>) -> Result<Self, GigasttClientError> {
        Self::with_timeout(base_url, DEFAULT_TIMEOUT)
    }

    /// Construct a client with an explicit timeout (useful for bounded polling
    /// and deterministic tests).
    pub fn with_timeout(
        base_url: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, GigasttClientError> {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let url = reqwest::Url::parse(&base_url).map_err(|_| {
            GigasttClientError::InvalidBaseUrl("expected a loopback HTTP origin".into())
        })?;
        if url.scheme() != "http"
            || url.host_str() != Some("127.0.0.1")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
            || timeout.is_zero()
        {
            return Err(GigasttClientError::InvalidBaseUrl(
                "expected a loopback HTTP origin and nonzero timeout".into(),
            ));
        }
        let http = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(timeout)
            .build()
            .map_err(|e| GigasttClientError::Transport(e.to_string()))?;
        Ok(Self {
            http,
            base_url,
            max_json_bytes: DEFAULT_MAX_JSON_BYTES,
        })
    }

    /// Override the JSON response cap.  Intended for tests and future model
    /// variants with unusually large result payloads.
    pub fn with_max_json_bytes(mut self, limit: usize) -> Self {
        self.max_json_bytes = limit.max(1);
        self
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// Poll `/ready`.  GigaSTT uses HTTP 503 for a valid `not_ready` payload;
    /// that state is returned as `Ok` so callers can retry without parsing an
    /// error string.
    pub async fn ready(&self) -> Result<ReadinessResponse, GigasttClientError> {
        let response = self
            .http
            .get(self.url("/ready"))
            .send()
            .await
            .map_err(|e| GigasttClientError::Transport(e.to_string()))?;
        let status = response.status();
        let body = self.read_body(response, self.max_json_bytes).await?;
        if status != StatusCode::OK && status != StatusCode::SERVICE_UNAVAILABLE {
            return Err(http_error(status, &body));
        }
        let value: ReadinessResponse = parse_json(&body)?;
        validate_readiness(&value)?;
        if (status == StatusCode::OK && value.status == ReadinessStatus::Ready)
            || (status == StatusCode::SERVICE_UNAVAILABLE
                && value.status == ReadinessStatus::NotReady)
        {
            Ok(value)
        } else {
            Err(http_error(status, &body))
        }
    }

    /// Submit a complete audio file to `/v1/jobs`.
    pub async fn submit_job(
        &self,
        audio: &[u8],
        options: &SubmitJobOptions,
    ) -> Result<JobSubmitResponse, GigasttClientError> {
        let response = self
            .http
            .post(self.url("/v1/jobs"))
            .query(options)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(audio.to_vec())
            .send()
            .await
            .map_err(|e| GigasttClientError::Transport(e.to_string()))?;
        let status = response.status();
        let body = self.read_body(response, self.max_json_bytes).await?;
        if status != StatusCode::ACCEPTED {
            return Err(http_error(status, &body));
        }
        let value: JobSubmitResponse = parse_json(&body)?;
        validate_submit(&value)
    }

    /// Submit a prepared WAV without buffering a second full audio copy.
    pub async fn submit_file(
        &self,
        path: &std::path::Path,
        options: &SubmitJobOptions,
    ) -> Result<JobSubmitResponse, GigasttClientError> {
        let read_error = |_| GigasttClientError::Transport("prepared audio cannot be read".into());
        let file = tokio::fs::File::open(path).await.map_err(read_error)?;
        let metadata = file.metadata().await.map_err(read_error)?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(GigasttClientError::Transport(
                "prepared audio is empty or not a file".into(),
            ));
        }
        let response = self
            .http
            .post(self.url("/v1/jobs"))
            .query(options)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header(reqwest::header::CONTENT_LENGTH, metadata.len())
            .body(reqwest::Body::wrap_stream(
                tokio_util::io::ReaderStream::new(file),
            ))
            .send()
            .await
            .map_err(|e| GigasttClientError::Transport(e.to_string()))?;
        let status = response.status();
        let body = self.read_body(response, self.max_json_bytes).await?;
        if status != StatusCode::ACCEPTED {
            return Err(http_error(status, &body));
        }
        validate_submit(&parse_json(&body)?)
    }

    /// Fetch queue/progress state for one job.
    pub async fn get_job(&self, job_id: &str) -> Result<JobStatusResponse, GigasttClientError> {
        validate_job_id(job_id)?;
        let response = self
            .http
            .get(self.url(&format!("/v1/jobs/{job_id}")))
            .send()
            .await
            .map_err(|e| GigasttClientError::Transport(e.to_string()))?;
        let status = response.status();
        let body = self.read_body(response, self.max_json_bytes).await?;
        if status != StatusCode::OK {
            return Err(http_error(status, &body));
        }
        let value: JobStatusResponse = parse_json(&body)?;
        if value.job_id != job_id {
            return Err(GigasttClientError::InvalidResponse(
                "job id mismatch".into(),
            ));
        }
        validate_job_status(&value)
    }

    /// Cancel a queued/processing job.  GigaSTT answers with HTTP 204 and an
    /// empty body on success.
    pub async fn cancel_job(&self, job_id: &str) -> Result<(), GigasttClientError> {
        validate_job_id(job_id)?;
        let response = self
            .http
            .delete(self.url(&format!("/v1/jobs/{job_id}")))
            .send()
            .await
            .map_err(|e| GigasttClientError::Transport(e.to_string()))?;
        let status = response.status();
        let body = self.read_body(response, MAX_ERROR_BODY_BYTES).await?;
        if status == StatusCode::NO_CONTENT {
            Ok(())
        } else {
            Err(http_error(status, &body))
        }
    }

    /// Fetch the finished JSON transcription result.
    pub async fn get_result(&self, job_id: &str) -> Result<TranscribeResponse, GigasttClientError> {
        validate_job_id(job_id)?;
        let response = self
            .http
            .get(self.url(&format!("/v1/jobs/{job_id}/result")))
            .send()
            .await
            .map_err(|e| GigasttClientError::Transport(e.to_string()))?;
        let status = response.status();
        let body = self.read_body(response, self.max_json_bytes).await?;
        if status != StatusCode::OK {
            return Err(http_error(status, &body));
        }
        let value: TranscribeResponse = parse_json(&body)?;
        validate_result(&value)
    }

    async fn read_body(
        &self,
        response: reqwest::Response,
        limit: usize,
    ) -> Result<Vec<u8>, GigasttClientError> {
        let limit = if response.status().is_success() {
            limit
        } else {
            limit.min(MAX_ERROR_BODY_BYTES)
        };
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| GigasttClientError::Transport(e.to_string()))?;
            if body.len().saturating_add(chunk.len()) > limit {
                return Err(GigasttClientError::BodyTooLarge { limit });
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }
}

fn parse_json<T: DeserializeOwned>(body: &[u8]) -> Result<T, GigasttClientError> {
    serde_json::from_slice(body).map_err(|e| {
        GigasttClientError::InvalidResponse(format!(
            "invalid JSON schema at line {}, column {}",
            e.line(),
            e.column()
        ))
    })
}

fn validate_job_id(job_id: &str) -> Result<(), GigasttClientError> {
    if job_id.is_empty()
        || !job_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(GigasttClientError::InvalidJobId);
    }
    Ok(())
}

fn validate_readiness(value: &ReadinessResponse) -> Result<ReadinessResponse, GigasttClientError> {
    if value.status == ReadinessStatus::Ready
        && (value.pool_available.is_none()
            || value.pool_total.is_none()
            || value.pool_total == Some(0))
    {
        return Err(GigasttClientError::InvalidResponse(
            "ready response has no pool fields".into(),
        ));
    }
    if let (Some(available), Some(total)) = (value.pool_available, value.pool_total) {
        if available > total {
            return Err(GigasttClientError::InvalidResponse(
                "pool_available exceeds pool_total".into(),
            ));
        }
    }
    Ok(value.clone())
}

fn validate_submit(value: &JobSubmitResponse) -> Result<JobSubmitResponse, GigasttClientError> {
    if value.job_id.is_empty() {
        return Err(GigasttClientError::InvalidResponse("empty job_id".into()));
    }
    validate_job_id(&value.job_id)?;
    if value.status != GigasttJobStatus::Queued {
        return Err(GigasttClientError::InvalidResponse(
            "submitted job status is not queued".into(),
        ));
    }
    if !value.created_at.is_finite() || value.created_at < 0.0 {
        return Err(GigasttClientError::InvalidResponse(
            "created_at must be a non-negative finite timestamp".into(),
        ));
    }
    Ok(value.clone())
}

fn validate_job_status(value: &JobStatusResponse) -> Result<JobStatusResponse, GigasttClientError> {
    if value.job_id.is_empty() {
        return Err(GigasttClientError::InvalidResponse("empty job_id".into()));
    }
    validate_job_id(&value.job_id)?;
    if !value.processed_seconds.is_finite() || value.processed_seconds < 0.0 {
        return Err(GigasttClientError::InvalidResponse(
            "processed_seconds must be a non-negative finite number".into(),
        ));
    }
    if value.percent > 100 {
        return Err(GigasttClientError::InvalidResponse(
            "percent must be between 0 and 100".into(),
        ));
    }
    if value.status == GigasttJobStatus::Failed && value.error.as_deref().unwrap_or("").is_empty() {
        return Err(GigasttClientError::InvalidResponse(
            "failed job has no error".into(),
        ));
    }
    Ok(value.clone())
}

fn validate_result(value: &TranscribeResponse) -> Result<TranscribeResponse, GigasttClientError> {
    if !value.duration.is_finite() || value.duration < 0.0 {
        return Err(GigasttClientError::InvalidResponse(
            "duration must be a non-negative finite number".into(),
        ));
    }
    validate_confidence(value.confidence)?;
    for word in &value.words {
        validate_confidence(word.confidence)?;
        validate_span(word.start, word.end, "word")?;
        if word.word.trim().is_empty() {
            return Err(GigasttClientError::InvalidResponse(
                "word text is empty".into(),
            ));
        }
    }
    if let Some(segments) = &value.segments {
        for segment in segments {
            validate_span(segment.start, segment.end, "segment")?;
            if segment.text.trim().is_empty() {
                return Err(GigasttClientError::InvalidResponse(
                    "empty segment text".into(),
                ));
            }
            if segment.words.is_empty() {
                return Err(GigasttClientError::InvalidResponse(
                    "non-empty segment has no words".into(),
                ));
            }
            for word in &segment.words {
                validate_span(word.start, word.end, "word")?;
                validate_confidence(word.confidence)?;
                if word.start < segment.start || word.end > segment.end {
                    return Err(GigasttClientError::InvalidResponse(
                        "word timestamps fall outside segment".into(),
                    ));
                }
                if word.word.trim().is_empty() {
                    return Err(GigasttClientError::InvalidResponse(
                        "empty word text".into(),
                    ));
                }
            }
        }
    }
    Ok(value.clone())
}

fn validate_confidence(confidence: Option<f32>) -> Result<(), GigasttClientError> {
    if confidence.is_some_and(|v| !v.is_finite() || !(0.0..=1.0).contains(&v)) {
        return Err(GigasttClientError::InvalidResponse(
            "invalid confidence".into(),
        ));
    }
    Ok(())
}

fn validate_span(start: f64, end: f64, label: &str) -> Result<(), GigasttClientError> {
    if !start.is_finite() || !end.is_finite() || start < 0.0 || end < start {
        return Err(GigasttClientError::InvalidResponse(format!(
            "{label} timestamps are invalid"
        )));
    }
    Ok(())
}

fn http_error(status: StatusCode, body: &[u8]) -> GigasttClientError {
    let parsed = serde_json::from_slice::<ErrorResponse>(body).ok();
    // Error bodies can include recognized text. Never forward them to logs/UI.
    let message = status
        .canonical_reason()
        .unwrap_or("HTTP error")
        .to_string();
    GigasttClientError::Http {
        status: status.as_u16(),
        code: parsed.as_ref().and_then(|e| e.code.clone()),
        message,
        retry_after_ms: parsed.and_then(|e| e.retry_after_ms),
    }
}
