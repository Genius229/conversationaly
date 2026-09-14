//! Same-endpoint WASAPI compatibility and parameter-only diagnostics.
//! No microphone samples or transcript text are logged here.

use super::capture_negotiation::{
    negotiate, os_error_code, retryable_format_error, CaptureAlternatives, CaptureSpec,
    MAX_ATTEMPTS,
};
use anyhow::{Context, Result};
use cpal::traits::DeviceTrait;
use cpal::{Device, SampleFormat, SupportedStreamConfig, SupportedStreamConfigRange};
use log::{info, warn};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

static NEXT_CAPTURE_ID: AtomicU64 = AtomicU64::new(1);
const MAX_RANGES: usize = 64;

fn candidates_for_range(
    range: SupportedStreamConfigRange,
    preferred_rate: u32,
) -> Vec<SupportedStreamConfig> {
    [
        preferred_rate,
        48000,
        44100,
        16000,
        8000,
        range.min_sample_rate(),
        range.max_sample_rate(),
    ]
    .into_iter()
    .filter_map(|rate| range.try_with_sample_rate(rate))
    .filter(|config| spec(config).format != "Unsupported")
    .collect()
}

pub(super) fn spec(config: &SupportedStreamConfig) -> CaptureSpec {
    CaptureSpec {
        rate: config.sample_rate(),
        channels: config.channels(),
        format: match config.sample_format() {
            SampleFormat::F32 => "F32",
            SampleFormat::F64 => "F64",
            SampleFormat::I8 => "I8",
            SampleFormat::I16 => "I16",
            SampleFormat::I24 => "I24",
            SampleFormat::I32 => "I32",
            SampleFormat::U8 => "U8",
            SampleFormat::U16 => "U16",
            SampleFormat::U24 => "U24",
            SampleFormat::U32 => "U32",
            _ => "Unsupported",
        },
    }
}

fn error_details(error: &anyhow::Error) -> (String, Option<i32>) {
    match error.downcast_ref::<cpal::Error>() {
        Some(error) => (
            format!("{:?}", error.kind()),
            os_error_code(&error.to_string()),
        ),
        None => ("NonCpalError".into(), None),
    }
}

pub(super) fn error_log(error: &anyhow::Error) -> String {
    let (kind, code) = error_details(error);
    let hresult = code.map_or_else(
        || "none".to_string(),
        |code| format!("0x{:08X}", code as u32),
    );
    format!("kind={kind} os_code={code:?} hresult={hresult} error={error:#}")
}

/// Only called for Windows; normal devices succeed before any enumeration.
/// `device` remains the identical resolved CPAL endpoint for every attempt.
pub(super) fn open<T>(
    device: &Device,
    default: SupportedStreamConfig,
    loopback: bool,
    mut build: impl FnMut(&SupportedStreamConfig) -> Result<T>,
) -> Result<(T, SupportedStreamConfig)> {
    let id = NEXT_CAPTURE_ID.fetch_add(1, Ordering::Relaxed);
    let started = Instant::now();
    // Do not log the persistent endpoint ID or arbitrary structured metadata.
    let description = device.description().map(|d| {
        (
            d.name().chars().take(160).collect::<String>(),
            d.device_type(),
            d.interface_type(),
        )
    });
    info!("capture_open id={id} version={} backend=WASAPI cpal=0.18.1 loopback={loopback} max_attempts={MAX_ATTEMPTS} name_type_interface={description:?} default={default:?}",
        env!("CARGO_PKG_VERSION"));
    let preferred_rate = default.sample_rate();
    let result = negotiate(
        default,
        spec,
        |config, attempt| {
            info!("capture_attempt id={id} attempt={attempt} spec={:?} buffer=Default shared=true event_callback=true stage=build", spec(config));
            let result = build(config);
            match &result {
                Ok(_) => info!("capture_attempt id={id} attempt={attempt} stage=build result=ok elapsed_ms={}", started.elapsed().as_millis()),
                Err(error) => warn!("capture_attempt id={id} attempt={attempt} stage=build result=error elapsed_ms={} {}", started.elapsed().as_millis(), error_log(error)),
            }
            result
        },
        || {
            let mut configs = Vec::new();
            // Bluetooth may switch profiles when opening capture. Refresh only
            // this endpoint, without falling back to the system default device.
            let refreshed = if loopback {
                device.default_output_config()
            } else {
                device.default_input_config()
            };
            let refreshed = match refreshed {
                Ok(config) => {
                    info!("capture_default_refresh id={id} config={config:?}");
                    Some(config)
                }
                Err(error) => {
                    warn!(
                        "capture_default_refresh id={id} kind={:?} error={error}",
                        error.kind()
                    );
                    None
                }
            };
            // Output endpoints expose loopback through build_input_stream, but
            // advertise their render formats via supported_output_configs.
            let ranges = if loopback {
                device
                    .supported_output_configs()
                    .map(|r| r.take(MAX_RANGES + 1).collect::<Vec<_>>())
            } else {
                device
                    .supported_input_configs()
                    .map(|r| r.take(MAX_RANGES + 1).collect::<Vec<_>>())
            };
            let ranges = match ranges {
                Ok(ranges) => ranges,
                Err(error) => {
                    warn!(
                        "capture_formats id={id} enumeration_failed kind={:?} error={error}",
                        error.kind()
                    );
                    return CaptureAlternatives {
                        refreshed,
                        supported: configs,
                    };
                }
            };
            info!(
                "capture_formats id={id} reported_ranges={} range_limit={MAX_RANGES}",
                ranges.len()
            );
            for range in ranges.into_iter().take(MAX_RANGES) {
                info!("capture_formats id={id} supported={range:?}");
                configs.extend(candidates_for_range(range, preferred_rate));
            }
            CaptureAlternatives {
                refreshed,
                supported: configs,
            }
        },
        |error| {
            let (kind, code) = error_details(error);
            retryable_format_error(&kind, code)
        },
    );
    match result {
        Ok((stream, selected)) => {
            info!(
                "capture_selected id={id} spec={:?} elapsed_ms={}",
                spec(&selected),
                started.elapsed().as_millis()
            );
            Ok((stream, selected))
        }
        Err(failure) => {
            warn!(
                "capture_failed id={id} attempts={} elapsed_ms={} {}",
                failure.attempts,
                started.elapsed().as_millis(),
                error_log(&failure.error)
            );
            Err(failure.error).with_context(|| {
                format!(
                    "WASAPI capture failed after {} format attempt(s) on the selected endpoint",
                    failure.attempts
                )
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpal_error_chain_preserves_localized_hresult_and_stage() {
        let error = anyhow::Error::new(cpal::Error::with_message(
            cpal::ErrorKind::BackendError,
            "Failed to initialize audio client: Параметр задан неверно. (os error -2147024809)",
        ))
        .context("stream build");
        let details = error_details(&error);
        assert!(retryable_format_error(&details.0, details.1));
        let line = error_log(&error);
        assert!(line.contains("0x80070057"));
        assert!(line.contains("stream build"));
        assert!(line.contains("Failed to initialize audio client"));
    }

    #[test]
    fn pcm_candidate_retains_its_actual_rate_and_channels() {
        let config = SupportedStreamConfig::new(
            1,
            16000,
            cpal::SupportedBufferSize::Unknown,
            SampleFormat::I16,
        );
        assert_eq!(
            spec(&config),
            CaptureSpec {
                rate: 16000,
                channels: 1,
                format: "I16"
            }
        );
        assert_eq!(config.config().buffer_size, cpal::BufferSize::Default);
    }

    #[test]
    fn narrowband_range_never_synthesizes_unsupported_rates() {
        let range = SupportedStreamConfigRange::new(
            1,
            16000,
            16000,
            cpal::SupportedBufferSize::Unknown,
            SampleFormat::I16,
        );
        let candidates = candidates_for_range(range, 48000);
        assert!(!candidates.is_empty());
        assert!(candidates.iter().all(|c| c.sample_rate() == 16000
            && c.channels() == 1
            && c.sample_format() == SampleFormat::I16));
    }
}
