//! Post-recording GigaSTT integration.
//!
//! The module intentionally has no dependency on the live transcription
//! pipeline.  The client and DTOs are small enough to be exercised against a
//! fake HTTP server before the sidecar lifecycle is wired in.

pub mod gigastt_client;
pub mod gigastt_sidecar;
pub mod importer;
pub mod tauri_adapter;
pub mod types;
pub mod wav;
pub mod audio_prepare;
pub mod native_audio;

pub use gigastt_client::{GigasttClient, GigasttClientError, SubmitJobOptions};
pub use gigastt_sidecar::{
    DiagnosticKind, DiagnosticStream, GigasttSidecar, GigasttSidecarConfig,
    SidecarDiagnostic, SidecarError, SidecarFailure, SidecarFailureKind, SidecarSnapshot, SidecarStatus,
};
pub use importer::{map_result, replace_transcript, ImportError, ImportedRow};
pub use types::{
    GigasttJobStatus, GigasttResult, JobStatusResponse, JobSubmitResponse, PostTranscriptionState,
    ReadinessResponse, ReadinessStatus, Segment, TranscribeResponse, WordInfo,
};
