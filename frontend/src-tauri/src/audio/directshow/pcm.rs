const SAMPLES_PER_CALLBACK: usize = 480;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmDecodeError {
    NonFinite,
    Truncated,
    CallbackPanicked,
}

/// Incrementally decodes little-endian f32 samples without retaining more than
/// one 10 ms callback frame and one incomplete sample.
pub struct PcmDecoder {
    partial: Vec<u8>,
    frame: Vec<f32>,
}

impl PcmDecoder {
    pub fn new() -> Self {
        Self {
            partial: Vec::with_capacity(4),
            frame: Vec::with_capacity(SAMPLES_PER_CALLBACK),
        }
    }

    pub fn push(
        &mut self,
        bytes: &[u8],
        emit: &mut impl FnMut(&[f32]) -> Result<(), ()>,
    ) -> Result<u64, PcmDecodeError> {
        let mut delivered = 0_u64;
        for &byte in bytes {
            self.partial.push(byte);
            if self.partial.len() != 4 {
                continue;
            }
            let sample = f32::from_le_bytes([
                self.partial[0],
                self.partial[1],
                self.partial[2],
                self.partial[3],
            ]);
            self.partial.clear();
            if !sample.is_finite() {
                return Err(PcmDecodeError::NonFinite);
            }
            self.frame.push(sample);
            if self.frame.len() == SAMPLES_PER_CALLBACK {
                emit(&self.frame).map_err(|_| PcmDecodeError::CallbackPanicked)?;
                delivered += self.frame.len() as u64;
                self.frame.clear();
            }
        }
        Ok(delivered)
    }

    pub fn finish(
        &mut self,
        emit: &mut impl FnMut(&[f32]) -> Result<(), ()>,
    ) -> Result<u64, PcmDecodeError> {
        if !self.partial.is_empty() {
            return Err(PcmDecodeError::Truncated);
        }
        if self.frame.is_empty() {
            return Ok(0);
        }
        emit(&self.frame).map_err(|_| PcmDecodeError::CallbackPanicked)?;
        let delivered = self.frame.len() as u64;
        self.frame.clear();
        Ok(delivered)
    }
}

impl Default for PcmDecoder {
    fn default() -> Self {
        Self::new()
    }
}
