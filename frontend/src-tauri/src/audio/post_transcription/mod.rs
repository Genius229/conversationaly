//! Post-recording GigaSTT integration.
//!
//! The module intentionally has no dependency on the live transcription
//! pipeline.  The client and DTOs are small enough to be exercised against a
//! fake HTTP server before the sidecar lifecycle is wired in.

mod artifacts;
pub mod audio_prepare;
pub mod commands;
pub mod gigastt_client;
pub mod gigastt_sidecar;
pub mod importer;
pub mod job_state;
pub mod finalization;
mod meeting_identity;
pub mod model_commands;
pub mod model_install;
pub mod native_audio;
pub mod native_exports;
pub mod preview;
pub mod service;
pub mod settings;
pub mod tauri_adapter;
pub mod types;
pub mod wav;

pub use gigastt_client::{GigasttClient, GigasttClientError, SubmitJobOptions};
pub use gigastt_sidecar::{
    DiagnosticKind, DiagnosticStream, GigasttSidecar, GigasttSidecarConfig, SidecarDiagnostic,
    SidecarError, SidecarFailure, SidecarFailureKind, SidecarSnapshot, SidecarStatus,
};
pub use importer::{map_result, replace_transcript, ImportError, ImportedRow};
pub use service::{
    PostTranscriptionError, PostTranscriptionRequest, PostTranscriptionResult,
    PostTranscriptionService,
};
pub use types::{
    GigasttJobStatus, GigasttResult, JobStatusResponse, JobSubmitResponse, PostTranscriptionState,
    ReadinessResponse, ReadinessStatus, Segment, TranscribeResponse, WordInfo,
};
