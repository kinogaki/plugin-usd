// Derived from OpenUSD (Copyright 2016 Pixar), licensed under the Tomorrow
// Open Source Technology License 1.0, and rewritten in Rust; see NOTICE.
//! Number text as OpenUSD writes it.
//!
//! Doubles and floats print as `TfStringify` does through double-conversion's
//! `ToShortest` (decimal_in_shortest_low -6, high 15, no flags): the
//! shortest digits that round-trip, decimal notation for decimal exponents
//! -6 to 14, else `1e21` / `1.5e-07`-free exponent form (`1.5e-7`), and
//! `inf`, `-inf`, `nan`, `-0`. Halves print as C's `%g` of their value, as
//! `GfHalf`'s stream operator does.

use std::io::Write;

use crate::usd::sdf::Half;

/// Append `TfStringify(double)` of `value`.
pub fn write_f64(out: &mut Vec<u8>, value: f64) {
    if value.is_nan() {
        out.extend_from_slice(b"nan");
        return;
    }
    if value.is_infinite() {
        out.extend_from_slice(if value < 0.0 { b"-inf" } else { b"inf" });
        return;
    }
    if value == 0.0 {
        out.extend_from_slice(if value.is_sign_negative() {
            b"-0"
        } else {
            b"0"
        });
        return;
    }
    if value.fract() == 0.0 && value.abs() < 1e15 {
        // Integral values below 1e15 print as integers: skip the digit search.
        write_integer(out, value as i64);
        return;
    }
    let mut buffer = [0u8; 40];
    let length = {
        let mut cursor = &mut buffer[..];
        // `{:e}` prints the shortest round-trip digits: "1.2345e-7".
        write!(cursor, "{value:e}").expect("40 bytes hold any f64");
        40 - cursor.len()
    };
    layout(out, &buffer[..length], value, false);
}

/// Append `TfStringify(float)` of `value`: the float's own shortest digits.
pub fn write_f32(out: &mut Vec<u8>, value: f32) {
    if value.is_nan() {
        out.extend_from_slice(b"nan");
        return;
    }
    if value.is_infinite() {
        out.extend_from_slice(if value < 0.0 { b"-inf" } else { b"inf" });
        return;
    }
    if value == 0.0 {
        out.extend_from_slice(if value.is_sign_negative() {
            b"-0"
        } else {
            b"0"
        });
        return;
    }
    if value.fract() == 0.0 && value.abs() < 1e7 {
        write_integer(out, value as i64);
        return;
    }
    let mut buffer = [0u8; 32];
    let length = {
        let mut cursor = &mut buffer[..];
        write!(cursor, "{value:e}").expect("32 bytes hold any f32");
        32 - cursor.len()
    };
    layout(out, &buffer[..length], f64::from(value), true);
}

/// Lay out `-d.ddde-x` (Rust's shortest exponent form) as double-conversion
/// does. `value` is the number printed (`single`: a float's own value).
fn layout(out: &mut Vec<u8>, scientific: &[u8], value: f64, single: bool) {
    let (negative, body) = match scientific.split_first() {
        Some((b'-', rest)) => (true, rest),
        _ => (false, scientific),
    };
    let e_at = body.iter().position(|&b| b == b'e').unwrap_or(body.len());
    let mantissa = &body[..e_at];
    let exponent: i32 = std::str::from_utf8(&body[e_at + 1..])
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(0);
    let mut digits = [0u8; 24];
    let mut count = 0;
    for &byte in mantissa {
        if byte != b'.' {
            digits[count] = byte;
            count += 1;
        }
    }
    let count = half_even(&mut digits, count, exponent, value.abs(), single);
    let digits = &digits[..count];
    if negative {
        out.push(b'-');
    }
    if (-6..15).contains(&exponent) {
        let point = exponent + 1;
        if point <= 0 {
            out.extend_from_slice(b"0.");
            out.extend(std::iter::repeat_n(b'0', (-point) as usize));
            out.extend_from_slice(digits);
        } else if point as usize >= digits.len() {
            out.extend_from_slice(digits);
            out.extend(std::iter::repeat_n(b'0', point as usize - digits.len()));
        } else {
            out.extend_from_slice(&digits[..point as usize]);
            out.push(b'.');
            out.extend_from_slice(&digits[point as usize..]);
        }
        return;
    }
    out.push(digits[0]);
    if digits.len() > 1 {
        out.push(b'.');
        out.extend_from_slice(&digits[1..]);
    }
    out.push(b'e');
    if exponent < 0 {
        out.push(b'-');
    }
    write_unsigned(out, u64::from(exponent.unsigned_abs()));
}

/// Ties between two shortest candidates go to the even last digit, as
/// double-conversion's `ToShortest` rounds them (Rust's formatter rounds them
/// up): when the printed digits end odd and the value lies exactly halfway to
/// the neighbor below (or above) that also reads back as the value, print the
/// even neighbor. The digit count after the change.
fn half_even(
    digits: &mut [u8; 24],
    count: usize,
    exponent: i32,
    value: f64,
    single: bool,
) -> usize {
    if count == 0 || count > 19 || (digits[count - 1] - b'0').is_multiple_of(2) {
        return count;
    }
    let Ok(m) = std::str::from_utf8(&digits[..count])
        .unwrap_or("")
        .parse::<u64>()
    else {
        return count;
    };
    let e10 = exponent - (count as i32 - 1);
    if !may_be_halfway(value, e10) {
        return count;
    }
    for candidate in [m.wrapping_sub(1), m + 1] {
        if candidate == 0 || candidate.to_string().len() != count {
            continue;
        }
        let text = format!("{candidate}e{e10}");
        let reads_back = if single {
            text.parse::<f32>()
                .is_ok_and(|parsed| f64::from(parsed) == value)
        } else {
            text.parse::<f64>().is_ok_and(|parsed| parsed == value)
        };
        if reads_back && exactly_halfway(value, m + candidate, e10) {
            let replaced = candidate.to_string();
            digits[..count].copy_from_slice(replaced.as_bytes());
            let mut kept = count;
            while kept > 1 && digits[kept - 1] == b'0' {
                kept -= 1;
            }
            return kept;
        }
    }
    count
}

/// A cheap filter for [`exactly_halfway`]: a value halfway between two
/// multiples of 10^e10 (e10 < 0) has no binary digits below 2^(e10 - 1).
fn may_be_halfway(value: f64, e10: i32) -> bool {
    if e10 >= 0 {
        return true;
    }
    let bits = value.to_bits();
    let raw_exponent = ((bits >> 52) & 0x7FF) as i32;
    let fraction = bits & ((1 << 52) - 1);
    let (mantissa, exponent2) = if raw_exponent == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1 << 52), raw_exponent - 1075)
    };
    if mantissa == 0 {
        return false;
    }
    exponent2 + mantissa.trailing_zeros() as i32 + 1 >= e10
}

/// Whether `2 * value == sum * 10^e10` exactly (`value` positive, finite).
fn exactly_halfway(value: f64, sum: u64, e10: i32) -> bool {
    let bits = value.to_bits();
    let raw_exponent = ((bits >> 52) & 0x7FF) as i32;
    let fraction = bits & ((1 << 52) - 1);
    let (mantissa, exponent2) = if raw_exponent == 0 {
        (fraction, -1074)
    } else {
        (fraction | (1 << 52), raw_exponent - 1075)
    };
    // mantissa * 2^(exponent2 + 1) == sum * 5^e10 * 2^e10
    let mut left = Big::from(mantissa);
    let mut right = Big::from(sum);
    let pow2 = exponent2 + 1 - e10;
    if e10 >= 0 {
        right.mul_pow5(e10 as u32);
    } else {
        left.mul_pow5((-e10) as u32);
    }
    if pow2 >= 0 {
        left.shl(pow2 as u32);
    } else {
        right.shl((-pow2) as u32);
    }
    left.trimmed() == right.trimmed()
}

/// A little-endian unsigned big integer, for exact tie tests.
struct Big(Vec<u32>);

impl Big {
    fn from(value: u64) -> Big {
        Big(vec![value as u32, (value >> 32) as u32])
    }

    fn mul_small(&mut self, factor: u32) {
        let mut carry = 0u64;
        for limb in &mut self.0 {
            let product = u64::from(*limb) * u64::from(factor) + carry;
            *limb = product as u32;
            carry = product >> 32;
        }
        if carry > 0 {
            self.0.push(carry as u32);
        }
    }

    fn mul_pow5(&mut self, mut power: u32) {
        while power >= 13 {
            self.mul_small(1_220_703_125);
            power -= 13;
        }
        if power > 0 {
            self.mul_small(5u32.pow(power));
        }
    }

    fn shl(&mut self, bits: u32) {
        let (words, rest) = ((bits / 32) as usize, bits % 32);
        if rest > 0 {
            let mut carry = 0u32;
            for limb in &mut self.0 {
                let next = *limb >> (32 - rest);
                *limb = (*limb << rest) | carry;
                carry = next;
            }
            if carry > 0 {
                self.0.push(carry);
            }
        }
        self.0.splice(0..0, std::iter::repeat_n(0, words));
    }

    fn trimmed(mut self) -> Vec<u32> {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
        self.0
    }
}

/// Append a signed integer.
pub fn write_integer(out: &mut Vec<u8>, value: i64) {
    if value < 0 {
        out.push(b'-');
    }
    write_unsigned(out, value.unsigned_abs());
}

/// Append an unsigned integer.
pub fn write_unsigned(out: &mut Vec<u8>, value: u64) {
    let mut digits = [0u8; 20];
    let mut at = digits.len();
    let mut rest = value;
    loop {
        at -= 1;
        digits[at] = b'0' + (rest % 10) as u8;
        rest /= 10;
        if rest == 0 {
            break;
        }
    }
    out.extend_from_slice(&digits[at..]);
}

/// Append C's `%g` of a half's value: six significant digits, rounded half
/// to even from the exact decimal value, trailing zeros dropped, exponent
/// form (`1e-05`, `1.5e+06`) below 1e-4 and from 1e6.
pub fn write_half(out: &mut Vec<u8>, half: Half) {
    let bits = half.0;
    let negative = bits >> 15 != 0;
    let exponent_bits = i32::from((bits >> 10) & 0x1F);
    let fraction = u128::from(bits & 0x3FF);
    if exponent_bits == 0x1F {
        out.extend_from_slice(match (fraction != 0, negative) {
            (true, _) => b"nan".as_slice(),
            (false, true) => b"-inf",
            (false, false) => b"inf",
        });
        return;
    }
    if negative {
        out.push(b'-');
    }
    if exponent_bits == 0 && fraction == 0 {
        out.push(b'0');
        return;
    }
    // value = significand * 2^power exactly; as decimal digits * 10^scale.
    let significand = if exponent_bits == 0 {
        fraction
    } else {
        fraction | 0x400
    };
    let power = if exponent_bits == 0 {
        -24
    } else {
        exponent_bits - 25
    };
    let (number, scale) = if power >= 0 {
        (significand << power, 0)
    } else {
        (significand * 5u128.pow((-power) as u32), power)
    };
    let mut digits: Vec<u8> = number
        .to_string()
        .into_bytes()
        .into_iter()
        .map(|b| b - b'0')
        .collect();
    let mut exponent10 = digits.len() as i32 - 1 + scale;
    if digits.len() > 6 {
        let dropped = &digits[6..];
        let first = dropped[0];
        let rest_zero = dropped[1..].iter().all(|&d| d == 0);
        let up = first > 5 || (first == 5 && (!rest_zero || digits[5] % 2 == 1));
        digits.truncate(6);
        if up {
            let mut at = 6;
            loop {
                if at == 0 {
                    // Carried past the first digit: 9.99995 -> 10.0000.
                    digits.insert(0, 1);
                    digits.truncate(6);
                    exponent10 += 1;
                    break;
                }
                at -= 1;
                if digits[at] == 9 {
                    digits[at] = 0;
                } else {
                    digits[at] += 1;
                    break;
                }
            }
        }
    }
    while digits.len() > 1 && digits.last() == Some(&0) {
        digits.pop();
    }
    if !(-4..6).contains(&exponent10) {
        out.push(b'0' + digits[0]);
        if digits.len() > 1 {
            out.push(b'.');
            out.extend(digits[1..].iter().map(|d| b'0' + d));
        }
        out.push(b'e');
        out.push(if exponent10 < 0 { b'-' } else { b'+' });
        let magnitude = exponent10.unsigned_abs();
        if magnitude < 10 {
            out.push(b'0');
        }
        write_unsigned(out, u64::from(magnitude));
        return;
    }
    if exponent10 < 0 {
        out.extend_from_slice(b"0.");
        out.extend(std::iter::repeat_n(b'0', (-exponent10 - 1) as usize));
        out.extend(digits.iter().map(|d| b'0' + d));
        return;
    }
    let integer_digits = exponent10 as usize + 1;
    for at in 0..integer_digits {
        out.push(b'0' + digits.get(at).copied().unwrap_or(0));
    }
    if digits.len() > integer_digits {
        out.push(b'.');
        out.extend(digits[integer_digits..].iter().map(|d| b'0' + d));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn double(value: f64) -> String {
        let mut out = Vec::new();
        write_f64(&mut out, value);
        String::from_utf8(out).unwrap()
    }

    fn float(value: f32) -> String {
        let mut out = Vec::new();
        write_f32(&mut out, value);
        String::from_utf8(out).unwrap()
    }

    fn half(value: f32) -> String {
        let mut out = Vec::new();
        write_half(&mut out, Half::from_f32(value));
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn doubles_print_as_tf_stringify() {
        assert_eq!(double(0.1), "0.1");
        assert_eq!(double(-0.0), "-0");
        assert_eq!(double(1.0), "1");
        assert_eq!(double(1e14), "100000000000000");
        assert_eq!(double(1e15), "1e15");
        assert_eq!(double(1.5e15), "1.5e15");
        assert_eq!(double(1e-6), "0.000001");
        assert_eq!(double(1.25e-7), "1.25e-7");
        assert_eq!(double(123.456), "123.456");
        assert_eq!(double(-2.5e-300), "-2.5e-300");
        assert_eq!(double(f64::INFINITY), "inf");
        assert_eq!(double(f64::NAN), "nan");
        assert_eq!(double(0.30000000000000004), "0.30000000000000004");
    }

    #[test]
    fn floats_use_their_own_digits() {
        assert_eq!(float(0.1), "0.1");
        assert_eq!(float(1.0 / 3.0), "0.33333334");
        assert_eq!(float(1e20), "1e20");
        assert_eq!(float(16777216.0), "16777216");
        // Exactly halfway between two shortest candidates: the even one.
        assert_eq!(float(57.820_313), "57.820312");
        assert_eq!(float(149.140_63), "149.14062");
    }

    #[test]
    fn halves_print_as_percent_g() {
        assert_eq!(half(1.0), "1");
        assert_eq!(half(0.1), "0.0999756");
        assert_eq!(half(65504.0), "65504");
        assert_eq!(half(-2.5), "-2.5");
        assert_eq!(half(6.1035156e-5), "6.10352e-05");
        assert_eq!(half(5.96e-8), "5.96046e-08");
    }
}
