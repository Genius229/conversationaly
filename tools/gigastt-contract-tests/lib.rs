//! Headless harness: compile the original source, never a test-only copy.
#[path = "../../frontend/src-tauri/src/audio/post_transcription/finalization.rs"]
pub mod finalization;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/artifacts.rs"]
pub mod artifacts;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/audio_prepare.rs"]
pub mod audio_prepare;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/gigastt_client.rs"]
pub mod gigastt_client;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/gigastt_sidecar.rs"]
pub mod gigastt_sidecar;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/importer.rs"]
pub mod importer;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/job_state.rs"]
pub mod job_state;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/meeting_identity.rs"]
pub mod meeting_identity;
#[cfg(feature = "gigastt-test-fixtures")]
#[path = "../../frontend/src-tauri/src/audio/post_transcription/model_install.rs"]
pub mod model_install;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/preview.rs"]
pub mod preview;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/service.rs"]
pub mod service;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/types.rs"]
pub mod types;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/wav.rs"]
pub mod wav;

pub use gigastt_client::{GigasttClient, GigasttClientError, SubmitJobOptions};
pub use gigastt_sidecar::*;
pub use types::*;

pub mod audio {
    pub mod post_transcription {
        pub use crate::finalization;
        pub use crate::audio_prepare;
        pub use crate::gigastt_client;
        pub use crate::gigastt_sidecar;
        pub use crate::gigastt_sidecar::*;
        pub use crate::importer;
        pub use crate::importer::{map_result, replace_transcript, ImportError, ImportedRow};
        pub use crate::job_state;
        #[cfg(feature = "gigastt-test-fixtures")]
        pub use crate::model_install;
        pub use crate::preview;
        pub use crate::service::{
            PostTranscriptionError, PostTranscriptionRequest, PostTranscriptionResult,
            PostTranscriptionService,
        };
        pub use crate::types;
        pub use crate::types::*;
        pub use crate::wav;
        pub use crate::{GigasttClient, GigasttClientError, SubmitJobOptions};
    }
}
