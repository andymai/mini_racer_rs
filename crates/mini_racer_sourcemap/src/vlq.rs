const CONTINUE: u8 = 0b10_0000;
// Five bits a digit: seven hold the 32 bits ECMA-426 allows. Its grammar
// allows any number of digits, but a run of zeros past these only makes a
// map longer.
const DIGITS: usize = 7;

const fn digit(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte.wrapping_sub(b'A')),
        b'a'..=b'z' => Some(byte.wrapping_sub(b'a').wrapping_add(26)),
        b'0'..=b'9' => Some(byte.wrapping_sub(b'0').wrapping_add(52)),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// The base64 VLQ that starts `input`, as ECMA-426 section 6 reads it, and
/// how many bytes it takes; `None` for a byte outside base64, a VLQ cut
/// short or longer than seven digits, or a value past 32 bits.
pub fn decode(input: &[u8]) -> Option<(i32, usize)> {
    let mut unsigned: u64 = 0;
    for (at, &byte) in input.iter().take(DIGITS).enumerate() {
        let digit = digit(byte)?;
        let shift = u32::try_from(at).ok()?.checked_mul(5)?;
        unsigned |= u64::from(digit & !CONTINUE).checked_shl(shift)?;
        if unsigned >> 32 != 0 {
            return None;
        }
        if digit & CONTINUE == 0 {
            return Some((signed(unsigned), at.saturating_add(1)));
        }
    }
    None
}

// The low bit is the sign, and ECMA-426 reads a negative zero as -2^31.
fn signed(unsigned: u64) -> i32 {
    let magnitude = i64::try_from(unsigned >> 1).unwrap_or(i64::MAX);
    let value = match (unsigned & 1 == 1, magnitude) {
        (true, 0) => i64::from(i32::MIN),
        (true, magnitude) => magnitude.saturating_neg(),
        (false, magnitude) => magnitude,
    };
    i32::try_from(value).unwrap_or(i32::MAX)
}

#[cfg(test)]
pub mod tests {
    #![allow(
        clippy::arithmetic_side_effects,
        clippy::cast_possible_truncation,
        clippy::indexing_slicing,
        reason = "test helpers"
    )]

    use super::decode;

    const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    /// Encodes `value` as a source map generator writes it.
    pub fn encode(value: i64) -> String {
        let mut unsigned = if value < 0 {
            (value.unsigned_abs() << 1) | 1
        } else {
            value.unsigned_abs() << 1
        };
        let mut out = String::new();
        loop {
            let digit = (unsigned & 0b1_1111) as u8;
            unsigned >>= 5;
            let digit = if unsigned == 0 {
                digit
            } else {
                digit | 0b10_0000
            };
            out.push(char::from(BASE64[usize::from(digit)]));
            if unsigned == 0 {
                return out;
            }
        }
    }

    #[test]
    fn reads_the_spec_examples() {
        assert_eq!(decode(b"iB"), Some((17, 2)));
        assert_eq!(decode(b"V"), Some((-10, 1)));
        assert_eq!(encode(17), "iB");
        assert_eq!(encode(-10), "V");
    }

    #[test]
    fn reads_single_digits_and_their_signs() {
        assert_eq!(decode(b"A"), Some((0, 1)));
        assert_eq!(decode(b"C"), Some((1, 1)));
        assert_eq!(decode(b"D"), Some((-1, 1)));
        assert_eq!(decode(b"e"), Some((15, 1)));
        assert_eq!(decode(b"f"), Some((-15, 1)));
        assert_eq!(decode(b"gB"), Some((16, 2)));
        assert_eq!(decode(b"hB"), Some((-16, 2)));
    }

    #[test]
    fn stops_at_the_end_of_one_value() {
        assert_eq!(decode(b"iBAAC"), Some((17, 2)));
        assert_eq!(decode(b"D,"), Some((-1, 1)));
    }

    #[test]
    fn round_trips_to_the_32_bit_limits() {
        for value in [
            0,
            1,
            -1,
            31,
            -31,
            32,
            1_000,
            -1_000,
            123_456_789,
            i64::from(i32::MAX),
            -i64::from(i32::MAX),
        ] {
            let text = encode(value);
            assert_eq!(
                decode(text.as_bytes()),
                Some((i32::try_from(value).unwrap(), text.len())),
                "{value} as {text}"
            );
        }
    }

    // ECMA-426 reads the sign bit alone, a negative zero, as -2^31.
    #[test]
    fn reads_a_negative_zero_as_the_least_value() {
        assert_eq!(decode(b"B"), Some((i32::MIN, 1)));
        assert_eq!(decode(b"hA"), Some((i32::MIN, 2)));
    }

    #[test]
    fn reads_up_to_seven_digits() {
        assert_eq!(decode(b"ggggggA"), Some((0, 7)));
        assert_eq!(decode(b"iggggggA"), None);
        assert_eq!(decode(b"igggggA"), Some((1, 7)));
        let long = format!("{}A", "g".repeat(10_000_000));
        let started = std::time::Instant::now();
        assert_eq!(decode(long.as_bytes()), None);
        assert!(started.elapsed() < std::time::Duration::from_millis(50));
    }

    #[test]
    fn refuses_values_past_32_bits() {
        // 2^31 as a magnitude needs 33 bits with its sign.
        assert_eq!(decode(encode(1 << 31).as_bytes()), None);
        assert_eq!(decode(encode(-(1 << 31) - 1).as_bytes()), None);
        assert_eq!(decode(b"ggggggE"), None);
    }

    #[test]
    fn refuses_cut_short_or_foreign_input() {
        assert_eq!(decode(b""), None);
        assert_eq!(decode(b"g"), None);
        assert_eq!(decode(b"gg"), None);
        assert_eq!(decode(b"!"), None);
        assert_eq!(decode(b"g="), None);
        assert_eq!(decode(b"-"), None);
    }
}
