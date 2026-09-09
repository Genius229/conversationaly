//! Wire/domain types for the GigaSTT asynchronous jobs API.
//!
//! These types deliberately model only the fields used by Conversationaly.
//! GigaSTT may add fields over time; serde therefore ignores additive fields,
//! while the client performs explicit validation of required values.

use serde::{Deserialize, Serialize};

/// State shown while a finalized recording is being replaced by GigaSTT.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PostTranscriptionState {
    PreparingAudio,
    Starting,
    Transcribing { percent: u8 },
    Finalizing,
    Ready,
    Failed { message: String },
    Cancelled,
}

/// `/ready` status.  A `not_ready` response is expected while the sidecar is
/// loading a model or when its single inference slot is occupied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessStatus {
    Ready,
    NotReady,
}

/// Typed response from `GET /ready`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReadinessResponse {
    pub status: ReadinessStatus,
    #[serde(default)]
    pub pool_available: Option<usize>,
    #[serde(default)]
    pub pool_total: Option<usize>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Async job lifecycle as returned by `GET /v1/jobs/{id}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GigasttJobStatus {
    Queued,
    Processing,
    Done,
    Failed,
    Cancelled,
}

/// Response returned by `POST /v1/jobs` (HTTP 202).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobSubmitResponse {
    pub job_id: String,
    pub status: GigasttJobStatus,
    pub created_at: f64,
}

/// Progress/status response returned by `GET /v1/jobs/{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobStatusResponse {
    pub job_id: String,
    pub status: GigasttJobStatus,
    pub processed_seconds: f64,
    pub percent: u8,
    #[serde(default)]
    pub error: Option<String>,
}

/// Per-word timing from a GigaSTT transcription result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WordInfo {
    pub word: String,
    pub start: f64,
    pub end: f64,
    #[serde(default)]
    pub confidence: Option<f32>,
    #[serde(default)]
    pub speaker: Option<u32>,
}

/// Natural segment grouped from word timings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub words: Vec<WordInfo>,
    #[serde(default)]
    pub speaker: Option<u32>,
}

/// JSON response returned by `GET /v1/jobs/{id}/result`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscribeResponse {
    pub text: String,
    pub words: Vec<WordInfo>,
    pub duration: f64,
    #[serde(default)]
    pub segments: Option<Vec<Segment>>,
    #[serde(default)]
    pub confidence: Option<f32>,
}

/// Alias used by the post-transcription service when it stores a fetched job
/// result.  Keeping this name makes the domain intent explicit without
/// introducing a second wire representation.
pub type GigasttResult = TranscribeResponse;
