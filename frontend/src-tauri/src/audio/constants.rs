/// Supported audio file extensions for import and retranscription.
///
/// Container extensions do not guarantee codec support. The shared decoder
/// retries unsupported or failed Symphonia decoding through bundled FFmpeg.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp4", "m4a", "wav", "mp3", "flac", "ogg", "aac", "mkv", "webm", "wma",
];
