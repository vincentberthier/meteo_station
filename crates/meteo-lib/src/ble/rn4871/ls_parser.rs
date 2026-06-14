//! Parser for `LS` (List Services) multi-line output.
//!
//! Extracts characteristic handles from the indented lines of the LS response.
//! Service UUID lines (no leading whitespace) and the `END` terminator are
//! ignored.

use super::super::encoding;

/// A parsed characteristic from an LS output line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharacteristicInfo {
    /// The 128-bit UUID as raw bytes.
    pub uuid_bytes: [u8; 16],
    /// The characteristic handle.
    pub handle: u16,
    /// The characteristic property bitmap (`0x02` read, `0x10` notify, …).
    ///
    /// Defaults to `0` when the LS line omits the property field. The RN4871
    /// lists a read+notify characteristic as two lines sharing one UUID — the
    /// value line (`0x02`) and the CCCD descriptor line (`0x10`) — so this
    /// field is needed to tell the value handle from the CCCD handle.
    pub properties: u8,
}

/// Parse a single line from LS output.
///
/// Characteristic lines are indented (start with whitespace) and have the
/// format: `<32hex UUID>,<handle hex>[,<props>[,<config>]]`.
///
/// Returns `None` for service UUID lines (no indentation), `END`, or
/// malformed input.
#[must_use]
pub fn parse_characteristic_line(line: &[u8]) -> Option<CharacteristicInfo> {
    // Characteristic lines start with whitespace; service UUID lines don't.
    if line.is_empty() || !line[0].is_ascii_whitespace() {
        return None;
    }

    // Strip leading whitespace
    let trimmed = strip_leading_whitespace(line);
    if trimmed.is_empty() {
        return None;
    }

    // Split on commas — need at least 2 fields (uuid, handle)
    let first_comma = trimmed.iter().position(|&b| b == b',')?;
    let uuid_hex = &trimmed[..first_comma];
    let rest = &trimmed[first_comma..];

    // Parse UUID (first field: 32 hex chars)
    let uuid_bytes = encoding::parse_uuid128(uuid_hex)?;

    // Parse handle (second field: up to 4 hex chars)
    // rest starts with ',', skip it, then find next comma or end
    let after_comma = &rest[1..];
    let handle_end = after_comma
        .iter()
        .position(|&b| b == b',')
        .unwrap_or(after_comma.len());
    let handle_hex = &after_comma[..handle_end];
    let handle = encoding::parse_hex_u16(handle_hex)?;

    // Parse the optional property field (third field). Defaults to 0 when absent.
    let properties = parse_property_field(&after_comma[handle_end..]);

    Some(CharacteristicInfo {
        uuid_bytes,
        handle,
        properties,
    })
}

/// Parse the property field that follows the handle in an LS line.
///
/// `rest` begins at the comma after the handle (or is empty when the line has
/// no property field). Returns `0` when the field is absent or unparseable.
fn parse_property_field(rest: &[u8]) -> u8 {
    if rest.first() != Some(&b',') {
        return 0;
    }
    let after_comma = &rest[1..];
    let end = after_comma
        .iter()
        .position(|&b| b == b',')
        .unwrap_or(after_comma.len());
    encoding::parse_hex_u16(&after_comma[..end])
        .and_then(|v| u8::try_from(v).ok())
        .unwrap_or(0)
}

/// Strip leading ASCII whitespace from a byte slice.
fn strip_leading_whitespace(data: &[u8]) -> &[u8] {
    let start = data
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(data.len());
    &data[start..]
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

    #[test]
    fn parse_3_field_characteristic_line() -> TestResult {
        // Given — typical LS output: "  <uuid>,<handle>,<props>"
        let line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A01,0072,12";

        // When
        let result = parse_characteristic_line(line);

        // Then
        let info = result.expect("should parse characteristic line");
        assert_eq!(
            info.uuid_bytes,
            [
                0xA4, 0xE6, 0x4B, 0x8B, 0x8D, 0xB3, 0x4E, 0x08, 0xA7, 0xD5, 0x7D, 0x3C, 0x3F, 0x2E,
                0x1A, 0x01,
            ],
            "UUID should match"
        );
        assert_eq!(info.handle, 0x0072, "handle should be 0x0072");
        assert_eq!(info.properties, 0x12, "properties should be 0x12");
        Ok(())
    }

    #[test]
    fn parse_4_field_characteristic_line() -> TestResult {
        // Given — LS output with config value: "  <uuid>,<handle>,<props>,<config>"
        let line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A02,0075,12,0100";

        // When
        let result = parse_characteristic_line(line);

        // Then
        let info = result.expect("should parse 4-field characteristic line");
        assert_eq!(info.handle, 0x0075, "handle should be 0x0075");
        assert_eq!(info.properties, 0x12, "properties should be 0x12");
        Ok(())
    }

    #[test]
    fn parse_value_line_reports_read_property() -> TestResult {
        // Given — the RN4871 value line of a read+notify characteristic
        let line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A01,0072,02";

        // When
        let info = parse_characteristic_line(line).expect("should parse value line");

        // Then
        assert_eq!(info.handle, 0x0072, "value handle should be 0x0072");
        assert_eq!(
            info.properties, 0x02,
            "value line carries the read property"
        );
        Ok(())
    }

    #[test]
    fn parse_cccd_line_reports_notify_property() -> TestResult {
        // Given — the RN4871 CCCD descriptor line (handle = value + 1)
        let line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A01,0073,10,0";

        // When
        let info = parse_characteristic_line(line).expect("should parse CCCD line");

        // Then
        assert_eq!(info.handle, 0x0073, "CCCD handle should be 0x0073");
        assert_eq!(
            info.properties, 0x10,
            "CCCD line carries the notify property"
        );
        Ok(())
    }

    #[test]
    fn parse_two_field_line_has_zero_properties() -> TestResult {
        // Given — a line with no property field
        let line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A01,0072";

        // When
        let info = parse_characteristic_line(line).expect("should parse 2-field line");

        // Then
        assert_eq!(info.handle, 0x0072, "handle should be 0x0072");
        assert_eq!(info.properties, 0, "missing property field defaults to 0");
        Ok(())
    }

    #[test]
    fn service_uuid_line_returns_none() -> TestResult {
        // Given — service UUID line (no leading whitespace)
        let line = b"A4E64B8B8DB34E08A7D57D3C3F2E1A00";

        // When
        let result = parse_characteristic_line(line);

        // Then
        assert!(result.is_none(), "service UUID lines should return None");
        Ok(())
    }

    #[test]
    fn end_line_returns_none() -> TestResult {
        // Given
        let line = b"END";

        // When
        let result = parse_characteristic_line(line);

        // Then
        assert!(result.is_none(), "END should return None");
        Ok(())
    }

    #[test]
    fn empty_line_returns_none() -> TestResult {
        // Given
        let line = b"";

        // When
        let result = parse_characteristic_line(line);

        // Then
        assert!(result.is_none(), "empty input should return None");
        Ok(())
    }

    #[test]
    fn malformed_hex_returns_none() -> TestResult {
        // Given — invalid hex in UUID
        let line = b"  ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ,0072,12";

        // When
        let result = parse_characteristic_line(line);

        // Then
        assert!(result.is_none(), "bad hex UUID should return None");
        Ok(())
    }

    #[test]
    fn malformed_handle_returns_none() -> TestResult {
        // Given — invalid hex in handle
        let line = b"  A4E64B8B8DB34E08A7D57D3C3F2E1A01,ZZZZ,12";

        // When
        let result = parse_characteristic_line(line);

        // Then
        assert!(result.is_none(), "bad hex handle should return None");
        Ok(())
    }

    #[test]
    fn whitespace_only_returns_none() -> TestResult {
        // Given
        let line = b"   ";

        // When
        let result = parse_characteristic_line(line);

        // Then
        assert!(result.is_none(), "whitespace-only should return None");
        Ok(())
    }
}
// grcov exclude stop
