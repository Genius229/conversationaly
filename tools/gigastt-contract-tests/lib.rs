//! Headless harness: compile the original source, never a test-only copy.
#[path = "../../frontend/src-tauri/src/audio/post_transcription/audio_prepare.rs"]
pub mod audio_prepare;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/wav.rs"]
pub mod wav;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/gigastt_client.rs"]
pub mod gigastt_client;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/gigastt_sidecar.rs"]
pub mod gigastt_sidecar;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/types.rs"]
pub mod types;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/importer.rs"]
pub mod importer;

pub use gigastt_client::{GigasttClient, GigasttClientError, SubmitJobOptions};
pub use gigastt_sidecar::*;
pub use types::*;

pub mod audio {
    pub mod post_transcription {
        pub use crate::audio_prepare;
        pub use crate::wav;
        pub use crate::gigastt_client;
        pub use crate::gigastt_sidecar;
        pub use crate::gigastt_sidecar::*;
        pub use crate::types;
        pub use crate::types::*;
        pub use crate::importer;
        pub use crate::importer::{map_result, replace_transcript, ImportError, ImportedRow};
        pub use crate::{GigasttClient, GigasttClientError, SubmitJobOptions};
    }
}
