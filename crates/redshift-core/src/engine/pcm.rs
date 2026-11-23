//! Turning the mixer's 32-bit float samples into what a device wants: integers of a given width.
//!
//! The mixer works in `f32`, which holds 24 bits of every sample exactly. Files of 16 or 24 bits
//! therefore pass through float unchanged, and going back to an integer of the same (or a larger)
//! width gives the original samples back, bit for bit. Only going to a *smaller* width than the file
//! loses information, and then a little noise (dither) is added so the rounding doesn't leave a
//! harsh distortion behind.

/// How one sample sits in the bytes sent to the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PcmFormat {
    /// 24 significant bits in a 32-bit container, most significant bits first (what WASAPI calls
    /// "24 valid bits in 32"). Most USB DACs and sound cards accept this.
    Int24In32,
    /// Three bytes per sample.
    Int24Packed,
    /// Full 32-bit integers (only the top 24 bits can differ from silence, since the mixer is `f32`).
    Int32,
    /// 16-bit integers.
    Int16,
}

impl PcmFormat {
    /// Bytes taken by one sample.
    pub fn container_bytes(self) -> usize {
        match self {
            PcmFormat::Int24In32 | PcmFormat::Int32 => 4,
            PcmFormat::Int24Packed => 3,
            PcmFormat::Int16 => 2,
        }
    }
    /// Bits that carry the sound.
    pub fn valid_bits(self) -> u8 {
        match self {
            PcmFormat::Int24In32 | PcmFormat::Int24Packed => 24,
            PcmFormat::Int32 => 32,
            PcmFormat::Int16 => 16,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            PcmFormat::Int24In32 => "24-bit (32-bit container)",
            PcmFormat::Int24Packed => "24-bit",
            PcmFormat::Int32 => "32-bit",
            PcmFormat::Int16 => "16-bit",
        }
    }
}

/// A tiny deterministic noise source for dither (xorshift); quality is plenty for ±1 LSB noise.
pub struct Dither(u32);

impl Dither {
    pub fn new() -> Self {
        Self(0x9E37_79B9)
    }
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }
    /// Triangular noise between -1 and +1 (in units of the target's least significant bit).
    fn tpdf(&mut self) -> f32 {
        let a = (self.next() >> 8) as f32 / (1u32 << 24) as f32;
        let b = (self.next() >> 8) as f32 / (1u32 << 24) as f32;
        a - b
    }
}

impl Default for Dither {
    fn default() -> Self {
        Self::new()
    }
}

/// Rounds a float sample (-1.0..1.0) to an integer with `bits` bits, saturating at the ends.
fn quantise(x: f32, bits: u32, dither: Option<&mut Dither>) -> i32 {
    let scale = (1u64 << (bits - 1)) as f64;
    let max = scale - 1.0;
    let noise = dither.map_or(0.0, |d| d.tpdf() as f64);
    let v = (x as f64 * scale + noise).round();
    v.clamp(-scale, max) as i32
}

/// Writes `samples` (interleaved, any channel count) into `out` in `format`, little endian.
/// `dither` adds noise only when given (use it when the format is narrower than the source).
pub fn pack(samples: &[f32], format: PcmFormat, mut dither: Option<&mut Dither>, out: &mut Vec<u8>) {
    out.reserve(samples.len() * format.container_bytes());
    for &s in samples {
        match format {
            PcmFormat::Int16 => {
                let v = quantise(s, 16, dither.as_deref_mut()) as i16;
                out.extend_from_slice(&v.to_le_bytes());
            }
            PcmFormat::Int24Packed => {
                let v = quantise(s, 24, dither.as_deref_mut());
                out.extend_from_slice(&v.to_le_bytes()[..3]);
            }
            PcmFormat::Int24In32 => {
                let v = quantise(s, 24, dither.as_deref_mut()) << 8;
                out.extend_from_slice(&v.to_le_bytes());
            }
            PcmFormat::Int32 => {
                // The float only has 24 bits to give, so scale the 24-bit value up.
                let v = quantise(s, 24, dither.as_deref_mut()) << 8;
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unpack24(bytes: &[u8]) -> Vec<i32> {
        bytes.chunks_exact(3).map(|b| (i32::from_le_bytes([0, b[0], b[1], b[2]])) >> 8).collect()
    }

    /// A 16-bit or 24-bit file survives the trip through f32 and back, bit for bit.
    #[test]
    fn sixteen_and_twenty_four_bit_samples_round_trip_exactly() {
        let ints24: Vec<i32> = vec![0, 1, -1, 8_388_607, -8_388_608, 123_456, -654_321, 4_194_304, -2_097_153];
        let floats: Vec<f32> = ints24.iter().map(|&v| v as f32 / 8_388_608.0).collect();
        let mut out = Vec::new();
        pack(&floats, PcmFormat::Int24Packed, None, &mut out);
        assert_eq!(unpack24(&out), ints24);
        out.clear();
        pack(&floats, PcmFormat::Int24In32, None, &mut out);
        let back: Vec<i32> = out.chunks_exact(4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) >> 8).collect();
        assert_eq!(back, ints24);

        // 16-bit file samples, as the decoder hands them over (value / 32768), come back exactly.
        let ints16: Vec<i16> = vec![0, 1, -1, 32_767, -32_768, 12_345, -23_456];
        let floats16: Vec<f32> = ints16.iter().map(|&v| v as f32 / 32_768.0).collect();
        out.clear();
        pack(&floats16, PcmFormat::Int16, None, &mut out);
        let back16: Vec<i16> = out.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        assert_eq!(back16, ints16);
        out.clear();
        pack(&floats16, PcmFormat::Int24In32, None, &mut out);
        let wide: Vec<i32> = out.chunks_exact(4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]) >> 8).collect();
        assert_eq!(wide, ints16.iter().map(|&v| v as i32 * 256).collect::<Vec<_>>());
    }

    #[test]
    fn out_of_range_samples_saturate_instead_of_wrapping() {
        let mut out = Vec::new();
        pack(&[2.0, -2.0, 1.0, -1.0], PcmFormat::Int16, None, &mut out);
        let v: Vec<i16> = out.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        assert_eq!(v, vec![32_767, -32_768, 32_767, -32_768]);
    }

    #[test]
    fn dither_stays_within_one_step_and_averages_out() {
        let mut d = Dither::new();
        let x = 0.25_f32 + 0.3 / 32_768.0; // between two 16-bit steps
        let samples = vec![x; 20_000];
        let mut out = Vec::new();
        pack(&samples, PcmFormat::Int16, Some(&mut d), &mut out);
        let v: Vec<i32> = out.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as i32).collect();
        let exact = x as f64 * 32_768.0;
        assert!(v.iter().all(|&s| (s as f64 - exact).abs() <= 2.0));
        let mean = v.iter().map(|&s| s as f64).sum::<f64>() / v.len() as f64;
        assert!((mean - exact).abs() < 0.05, "mean {mean} vs {exact}");
    }

    #[test]
    fn container_sizes_and_bits() {
        assert_eq!((PcmFormat::Int24In32.container_bytes(), PcmFormat::Int24In32.valid_bits()), (4, 24));
        assert_eq!((PcmFormat::Int24Packed.container_bytes(), PcmFormat::Int24Packed.valid_bits()), (3, 24));
        assert_eq!((PcmFormat::Int16.container_bytes(), PcmFormat::Int16.valid_bits()), (2, 16));
    }
}
