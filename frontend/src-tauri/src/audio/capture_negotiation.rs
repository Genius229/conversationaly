//! Bounded capture format negotiation, independent of the platform/CPAL API.
//! The caller owns ONE endpoint; this helper never discovers another device.

pub const MAX_ATTEMPTS: usize = 12;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct CaptureSpec {
    pub rate: u32,
    pub channels: u16,
    pub format: &'static str,
}

#[derive(Debug)]
pub struct NegotiationFailure<E> {
    pub error: E,
    pub attempts: usize,
}

pub struct CaptureAlternatives<C> {
    pub refreshed: Option<C>,
    pub supported: Vec<C>,
}

impl<C> From<Vec<C>> for CaptureAlternatives<C> {
    fn from(supported: Vec<C>) -> Self {
        Self {
            refreshed: None,
            supported,
        }
    }
}

pub fn os_error_code(message: &str) -> Option<i32> {
    message
        .rsplit_once("(os error ")?
        .1
        .strip_suffix(')')?
        .parse()
        .ok()
}

pub fn retryable_format_error(kind: &str, os_code: Option<i32>) -> bool {
    matches!(kind, "UnsupportedConfig" | "InvalidInput")
        || (kind == "BackendError" && os_code == Some(0x80070057_u32 as i32))
}

pub fn negotiate<C, T, E>(
    default: C,
    spec: impl Fn(&C) -> CaptureSpec,
    mut attempt: impl FnMut(&C, usize) -> Result<T, E>,
    alternatives: impl FnOnce() -> CaptureAlternatives<C>,
    retryable: impl Fn(&E) -> bool,
) -> Result<(T, C), NegotiationFailure<E>> {
    let mut error = match attempt(&default, 1) {
        Ok(stream) => return Ok((stream, default)),
        Err(error) => error,
    };
    if !retryable(&error) {
        return Err(NegotiationFailure { error, attempts: 1 });
    }
    let preferred = spec(&default);
    let alternatives = alternatives();
    let refreshed_spec = alternatives.refreshed.as_ref().map(&spec);
    let mut candidates = alternatives.supported;
    if let Some(refreshed) = alternatives.refreshed {
        candidates.push(refreshed);
    }
    candidates.retain(|c| {
        let s = spec(c);
        s.rate > 0 && s.channels > 0 && s != preferred
    });
    candidates.sort_by_key(|c| {
        let s = spec(c);
        (
            Some(s) != refreshed_spec,
            s.channels != preferred.channels,
            s.rate != preferred.rate,
            format_rank(s.format),
            s.rate.abs_diff(preferred.rate),
            s,
        )
    });
    candidates.dedup_by(|a, b| spec(a) == spec(b));
    let mut attempts = 1;
    for candidate in candidates.into_iter().take(MAX_ATTEMPTS - 1) {
        attempts += 1;
        match attempt(&candidate, attempts) {
            Ok(stream) => return Ok((stream, candidate)),
            Err(next) => error = next,
        }
        if !retryable(&error) {
            break;
        }
    }
    Err(NegotiationFailure { error, attempts })
}

fn format_rank(format: &str) -> u8 {
    match format {
        "I16" => 0,
        "I32" => 1,
        "I24" => 2,
        "F32" => 3,
        "F64" => 4,
        "U8" => 5,
        _ => 6,
    }
}
