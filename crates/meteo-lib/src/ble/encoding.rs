//! Hex encoding/decoding helpers for BLE data formatting.
//!
//! Single source of truth for encoding f32 values as little-endian bytes and
//! converting between binary data and hex strings. Used by both command
//! serialization and GATT data encoding.

/// Encode an f32 as 4 bytes in little-endian order (BLE wire format).
#[expect(clippy::little_endian_bytes, reason = "BLE wire format is LE")]
#[must_use]
pub const fn encode_f32(value: f32) -> [u8; 4] {
    value.to_le_bytes()
}

/// Decode 4 little-endian bytes into an f32.
#[expect(clippy::little_endian_bytes, reason = "BLE wire format is LE")]
#[must_use]
pub const fn decode_f32(bytes: &[u8; 4]) -> f32 {
    f32::from_le_bytes(*bytes)
}

/// Hex digit lookup table.
const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

/// Write a byte slice as uppercase hex into `buf`.
///
/// Returns the number of hex chars written (2 * `data.len()`), or `None` if
/// `buf` is too small.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "pos increments by 2 within bounds checked by initial length test"
)]
#[must_use]
pub fn bytes_to_hex(data: &[u8], buf: &mut [u8]) -> Option<usize> {
    let needed = data.len().checked_mul(2)?;
    if buf.len() < needed {
        return None;
    }
    let mut pos = 0_usize;
    for &byte in data {
        buf[pos] = HEX_DIGITS[(byte >> 4_i32) as usize];
        buf[pos + 1] = HEX_DIGITS[(byte & 0x0F) as usize];
        pos += 2;
    }
    Some(needed)
}

/// Write a u8 as exactly 2 uppercase hex digits.
///
/// Returns 2 on success, or `None` if `buf` is too small.
#[must_use]
pub fn u8_to_hex(val: u8, buf: &mut [u8]) -> Option<usize> {
    bytes_to_hex(&[val], buf)
}

/// Write a u16 as exactly 4 uppercase hex digits (zero-padded).
///
/// Returns 4 on success, or `None` if `buf` is too small.
#[expect(clippy::big_endian_bytes, reason = "big-endian for hex display order")]
#[must_use]
pub fn u16_to_hex(val: u16, buf: &mut [u8]) -> Option<usize> {
    let be_bytes = val.to_be_bytes();
    bytes_to_hex(&be_bytes, buf)
}

/// Parse up to 4 hex chars as a u16.
///
/// Returns `None` on invalid hex or empty input.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "shift by 4 and OR with nibble are safe for u16 accumulation"
)]
#[must_use]
pub fn parse_hex_u16(bytes: &[u8]) -> Option<u16> {
    if bytes.is_empty() || bytes.len() > 4 {
        return None;
    }
    let mut result = 0_u16;
    for &b in bytes {
        let nibble = match b {
            b'0'..=b'9' => b - b'0',
            b'A'..=b'F' => b - b'A' + 10,
            b'a'..=b'f' => b - b'a' + 10,
            _ => return None,
        };
        result = (result << 4_i32) | u16::from(nibble);
    }
    Some(result)
}

/// Parse 32 hex chars as a 128-bit UUID byte array.
///
/// Returns `None` on invalid hex or wrong length.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "i increments by 2 within bounds, nibble shifts are safe"
)]
#[expect(
    clippy::integer_division,
    reason = "i / 2 maps pairs of hex chars to byte index"
)]
#[must_use]
pub fn parse_uuid128(hex: &[u8]) -> Option<[u8; 16]> {
    if hex.len() != 32 {
        return None;
    }
    let mut uuid = [0_u8; 16];
    let mut i = 0_usize;
    while i < 32 {
        let hi = hex_nibble(hex[i])?;
        let lo = hex_nibble(hex[i + 1])?;
        uuid[i / 2] = (hi << 4_i32) | lo;
        i += 2;
    }
    Some(uuid)
}

/// Convert a single ASCII hex character to its nibble value.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "match arms guarantee b is within safe range for subtraction"
)]
const fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(test)]
mod tests {
    extern crate std;

    use core::{error, result};

    use std::boxed::Box;
    use test_log::test;

    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    // --- f32 encode/decode round-trip ---

    #[test]
    fn encode_decode_f32_round_trip() -> TestResult {
        // Given
        let value = 23.45_f32;

        // When
        let encoded = encode_f32(value);
        let decoded = decode_f32(&encoded);

        // Then
        assert_eq!(decoded, value, "round-trip should preserve value");
        Ok(())
    }

    #[test]
    fn encode_decode_negative_temperature() -> TestResult {
        // Given
        let value = -15.7_f32;

        // When
        let encoded = encode_f32(value);
        let decoded = decode_f32(&encoded);

        // Then
        assert_eq!(decoded, value, "negative temperature round-trip");
        Ok(())
    }

    #[test]
    fn encode_decode_high_pressure() -> TestResult {
        // Given
        let value = 110_000.0_f32;

        // When
        let encoded = encode_f32(value);
        let decoded = decode_f32(&encoded);

        // Then
        assert_eq!(decoded, value, "high pressure round-trip");
        Ok(())
    }

    #[test]
    fn encode_f32_zero() -> TestResult {
        // Given
        let value = 0.0_f32;

        // When
        let encoded = encode_f32(value);

        // Then
        assert_eq!(encoded, [0, 0, 0, 0], "zero should encode as all zeros");
        Ok(())
    }

    // --- bytes_to_hex ---

    #[test]
    fn bytes_to_hex_known_values() -> TestResult {
        // Given
        let data = [0xA4, 0xE6, 0x4B];
        let mut buf = [0_u8; 6];

        // When
        let n = bytes_to_hex(&data, &mut buf).expect("buffer large enough");

        // Then
        assert_eq!(n, 6, "should write 6 hex chars");
        assert_eq!(&buf[..n], b"A4E64B", "should produce uppercase hex");
        Ok(())
    }

    #[test]
    fn bytes_to_hex_empty() -> TestResult {
        // Given
        let data: &[u8] = &[];
        let mut buf = [0_u8; 4];

        // When
        let n = bytes_to_hex(data, &mut buf).expect("buffer large enough");

        // Then
        assert_eq!(n, 0, "empty input should produce zero hex chars");
        Ok(())
    }

    #[test]
    fn bytes_to_hex_buffer_too_small() -> TestResult {
        // Given
        let data = [0xFF, 0x00];
        let mut buf = [0_u8; 3]; // needs 4

        // When
        let result = bytes_to_hex(&data, &mut buf);

        // Then
        assert!(result.is_none(), "should return None for small buffer");
        Ok(())
    }

    // --- u8_to_hex ---

    #[test]
    fn u8_to_hex_zero_padded() -> TestResult {
        // Given
        let mut buf = [0_u8; 4];

        // When
        let n = u8_to_hex(0x05, &mut buf).expect("buffer large enough");

        // Then
        assert_eq!(&buf[..n], b"05", "should zero-pad to 2 digits");
        Ok(())
    }

    #[test]
    fn u8_to_hex_max() -> TestResult {
        // Given
        let mut buf = [0_u8; 4];

        // When
        let n = u8_to_hex(0xFF, &mut buf).expect("buffer large enough");

        // Then
        assert_eq!(&buf[..n], b"FF", "should produce FF");
        Ok(())
    }

    // --- u16_to_hex ---

    #[test]
    fn u16_to_hex_zero_padded() -> TestResult {
        // Given
        let mut buf = [0_u8; 8];

        // When
        let n = u16_to_hex(0x001A, &mut buf).expect("buffer large enough");

        // Then
        assert_eq!(&buf[..n], b"001A", "should zero-pad to 4 digits");
        Ok(())
    }

    #[test]
    fn u16_to_hex_full() -> TestResult {
        // Given
        let mut buf = [0_u8; 8];

        // When
        let n = u16_to_hex(0xABCD, &mut buf).expect("buffer large enough");

        // Then
        assert_eq!(&buf[..n], b"ABCD", "should produce ABCD");
        Ok(())
    }

    #[test]
    fn u16_to_hex_zero() -> TestResult {
        // Given
        let mut buf = [0_u8; 8];

        // When
        let n = u16_to_hex(0x0000, &mut buf).expect("buffer large enough");

        // Then
        assert_eq!(&buf[..n], b"0000", "zero should pad to 4 digits");
        Ok(())
    }

    // --- parse_hex_u16 ---

    #[test]
    fn parse_hex_u16_valid_4_chars() -> TestResult {
        // Given
        let input = b"0072";

        // When
        let result = parse_hex_u16(input);

        // Then
        assert_eq!(result, Some(0x0072), "should parse 0072 as 0x0072");
        Ok(())
    }

    #[test]
    fn parse_hex_u16_lowercase() -> TestResult {
        // Given
        let input = b"abcd";

        // When
        let result = parse_hex_u16(input);

        // Then
        assert_eq!(result, Some(0xABCD), "should accept lowercase hex");
        Ok(())
    }

    #[test]
    fn parse_hex_u16_single_char() -> TestResult {
        // Given
        let input = b"F";

        // When
        let result = parse_hex_u16(input);

        // Then
        assert_eq!(result, Some(0xF), "should parse single char");
        Ok(())
    }

    #[test]
    fn parse_hex_u16_empty_returns_none() -> TestResult {
        // Given
        let input = b"";

        // When
        let result = parse_hex_u16(input);

        // Then
        assert!(result.is_none(), "empty input should return None");
        Ok(())
    }

    #[test]
    fn parse_hex_u16_too_long_returns_none() -> TestResult {
        // Given
        let input = b"12345";

        // When
        let result = parse_hex_u16(input);

        // Then
        assert!(result.is_none(), "5 chars should return None");
        Ok(())
    }

    #[test]
    fn parse_hex_u16_invalid_char_returns_none() -> TestResult {
        // Given
        let input = b"00GG";

        // When
        let result = parse_hex_u16(input);

        // Then
        assert!(result.is_none(), "invalid hex char should return None");
        Ok(())
    }

    // --- parse_uuid128 ---

    #[test]
    fn parse_uuid128_valid() -> TestResult {
        // Given
        let hex = b"A4E64B8B8DB34E08A7D57D3C3F2E1A00";

        // When
        let result = parse_uuid128(hex);

        // Then
        let expected = [
            0xA4, 0xE6, 0x4B, 0x8B, 0x8D, 0xB3, 0x4E, 0x08, 0xA7, 0xD5, 0x7D, 0x3C, 0x3F, 0x2E,
            0x1A, 0x00,
        ];
        assert_eq!(result, Some(expected), "should parse 32 hex chars as UUID");
        Ok(())
    }

    #[test]
    fn parse_uuid128_wrong_length() -> TestResult {
        // Given
        let hex = b"A4E64B8B8DB34E08A7D57D3C3F2E1A"; // 30 chars

        // When
        let result = parse_uuid128(hex);

        // Then
        assert!(result.is_none(), "wrong length should return None");
        Ok(())
    }

    #[test]
    fn parse_uuid128_invalid_hex() -> TestResult {
        // Given
        let hex = b"A4E64B8B8DB34E08A7D57D3C3F2EXXXX";

        // When
        let result = parse_uuid128(hex);

        // Then
        assert!(result.is_none(), "invalid hex should return None");
        Ok(())
    }

    #[test]
    fn parse_uuid128_lowercase() -> TestResult {
        // Given
        let hex = b"a4e64b8b8db34e08a7d57d3c3f2e1a01";

        // When
        let result = parse_uuid128(hex);

        // Then
        let expected = [
            0xA4, 0xE6, 0x4B, 0x8B, 0x8D, 0xB3, 0x4E, 0x08, 0xA7, 0xD5, 0x7D, 0x3C, 0x3F, 0x2E,
            0x1A, 0x01,
        ];
        assert_eq!(result, Some(expected), "should accept lowercase hex");
        Ok(())
    }
}
// grcov exclude stop
