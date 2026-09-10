#[path = "../../../frontend/src-tauri/src/audio/decoder/codec.rs"]
mod codec;
pub use codec::{
    decode_audio_file, decode_audio_file_with_progress, DecodedAudio, ProgressCallback,
};
