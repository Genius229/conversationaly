//! Real production codec, FFmpeg discovery and import validation, without the
//! desktop's GUI/audio-device stack. No copied codec or stub processing path.
#[path = "../../frontend/src-tauri/src/audio/constants.rs"]
pub mod constants;
pub mod decoder;
#[path = "../../frontend/src-tauri/src/audio/ffmpeg.rs"]
pub mod ffmpeg;
#[path = "../../frontend/src-tauri/src/audio/import_validation.rs"]
pub mod import_validation;
