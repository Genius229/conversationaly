//! Optional Windows microphone fallback. Does not replace working WASAPI.
pub mod device;
mod capture;
mod pcm;
mod windows_job;

pub use capture::{CaptureError, CaptureLimits, DirectShowCapture, StopReport};
pub const OUTPUT_RATE: u32 = 48_000;
pub const OUTPUT_CHANNELS: u16 = 1;
