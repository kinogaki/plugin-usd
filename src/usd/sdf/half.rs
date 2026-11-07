// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! IEEE 754 half-precision floats (`GfHalf`), stored as their bits.

use std::fmt;

/// A 16-bit float.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Half(pub u16);

impl Half {
    /// The half nearest `value` (round to nearest, ties to even).
    pub fn from_f32(value: f32) -> Half {
        let bits = value.to_bits();
        let sign = ((bits >> 16) & 0x8000) as u16;
        let exponent = ((bits >> 23) & 0xFF) as i32;
        let mantissa = bits & 0x007F_FFFF;
        if exponent == 0xFF {
            // Infinity, or NaN with a quiet bit kept.
            let nan = if mantissa != 0 {
                0x0200 | (mantissa >> 13) as u16
            } else {
                0
            };
            return Half(sign | 0x7C00 | nan);
        }
        let unbiased = exponent - 127;
        if unbiased > 15 {
            return Half(sign | 0x7C00);
        }
        if unbiased >= -14 {
            // Normal: round the 23-bit mantissa to 10 bits.
            let half_exponent = ((unbiased + 15) as u32) << 10;
            let mut result = half_exponent | (mantissa >> 13);
            let rest = mantissa & 0x1FFF;
            if rest > 0x1000 || (rest == 0x1000 && result & 1 == 1) {
                result += 1; // may carry into the exponent, up to infinity
            }
            return Half(sign | result as u16);
        }
        if unbiased < -25 {
            return Half(sign);
        }
        // Subnormal: shift the full significand into place, rounding.
        let significand = mantissa | 0x0080_0000;
        let shift = (-unbiased - 14 + 13) as u32;
        let mut result = significand >> shift;
        let rest = significand & ((1 << shift) - 1);
        let halfway = 1 << (shift - 1);
        if rest > halfway || (rest == halfway && result & 1 == 1) {
            result += 1;
        }
        Half(sign | result as u16)
    }

    /// The exact f32 value.
    pub fn to_f32(self) -> f32 {
        let bits = u32::from(self.0);
        let sign = (bits & 0x8000) << 16;
        let exponent = (bits >> 10) & 0x1F;
        let mantissa = bits & 0x03FF;
        let value = match exponent {
            0 if mantissa == 0 => sign,
            0 => {
                // Subnormal: normalize.
                let mut e = 127 - 15 + 1;
                let mut m = mantissa;
                while m & 0x0400 == 0 {
                    m <<= 1;
                    e -= 1;
                }
                sign | (e << 23) | ((m & 0x03FF) << 13)
            }
            0x1F => sign | 0x7F80_0000 | (mantissa << 13),
            _ => sign | ((exponent + 127 - 15) << 23) | (mantissa << 13),
        };
        f32::from_bits(value)
    }

    /// The f64 value.
    pub fn to_f64(self) -> f64 {
        f64::from(self.to_f32())
    }
}

impl From<f32> for Half {
    fn from(value: f32) -> Half {
        Half::from_f32(value)
    }
}

impl fmt::Debug for Half {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}h", self.to_f32())
    }
}

#[cfg(test)]
mod tests {
    use super::Half;

    #[test]
    fn round_trips_every_half() {
        for bits in 0..=u16::MAX {
            let half = Half(bits);
            let value = half.to_f32();
            if value.is_nan() {
                assert!(Half::from_f32(value).to_f32().is_nan());
            } else {
                assert_eq!(Half::from_f32(value), half, "{bits:#06x}");
            }
        }
    }

    #[test]
    fn rounds_to_nearest_even() {
        assert_eq!(Half::from_f32(1.0).0, 0x3C00);
        assert_eq!(Half::from_f32(65504.0).0, 0x7BFF);
        assert_eq!(Half::from_f32(65520.0).0, 0x7C00);
        assert_eq!(Half::from_f32(1.0 + 1.0 / 2048.0).0, 0x3C00);
        assert_eq!(Half::from_f32(1.0 + 3.0 / 2048.0).0, 0x3C02);
        assert_eq!(Half::from_f32(5.96e-8).0, 0x0001);
        assert_eq!(Half::from_f32(-0.0).0, 0x8000);
    }
}
