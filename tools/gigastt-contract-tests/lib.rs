//! Headless harness: compile the original source, never a test-only copy.
#[path = "../../frontend/src-tauri/src/audio/post_transcription/gigastt_client.rs"]
pub mod gigastt_client;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/gigastt_sidecar.rs"]
pub mod gigastt_sidecar;
#[path = "../../frontend/src-tauri/src/audio/post_transcription/types.rs"]
pub mod types;

pub use gigastt_client::{GigasttClient, GigasttClientError, SubmitJobOptions};
pub use gigastt_sidecar::*;
pub use types::*;

pub mod audio {
    pub mod post_transcription {
        pub use crate::gigastt_client;
        pub use crate::gigastt_sidecar;
        pub use crate::gigastt_sidecar::*;
        pub use crate::types;
        pub use crate::types::*;
        pub use crate::{GigasttClient, GigasttClientError, SubmitJobOptions};
    }
}
