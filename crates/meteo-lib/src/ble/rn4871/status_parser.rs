//! Parser for RN4871 `%...%` status events.
//!
//! Parses the inner content of `%`-delimited status messages into typed
//! [`StatusEvent`] values.

use super::status_event::StatusEvent;

/// Parses the inner content of a `%...%` status message.
///
/// The input should be the bytes between the `%` delimiters (not including the
/// delimiters themselves). For example, for `%REBOOT%`, pass `b"REBOOT"`.
///
/// Unrecognized events are returned as [`StatusEvent::Unknown`].
#[must_use]
pub fn parse(inner: &[u8]) -> StatusEvent<'_> {
    match inner {
        b"REBOOT" => StatusEvent::Reboot,
        b"DISCONNECT" => StatusEvent::Disconnect,
        b"STREAM_OPEN" => StatusEvent::StreamOpen,
        _ if inner.starts_with(b"CONN_PARAM,") => {
            StatusEvent::ConnParam(&inner[b"CONN_PARAM,".len()..])
        }
        _ if inner.starts_with(b"CONNECT,") => parse_connect_event(inner),
        _ => StatusEvent::Unknown(inner),
    }
}

/// Attempts to parse a `CONNECT,<type>,<address>` status event.
/// Returns `Unknown` if the format doesn't match.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "comma_pos < rest.len() so +1 won't overflow"
)]
fn parse_connect_event(inner: &[u8]) -> StatusEvent<'_> {
    // Expected format: CONNECT,<addr_type>,<address>
    let Some(rest) = inner.strip_prefix(b"CONNECT,") else {
        return StatusEvent::Unknown(inner);
    };

    // Find the comma separating address_type from address
    let Some(comma_pos) = rest.iter().position(|&b| b == b',') else {
        return StatusEvent::Unknown(inner);
    };

    let type_byte = &rest[..comma_pos];
    let address = &rest[comma_pos + 1..];

    // Address type must be a single ASCII digit (0 or 1)
    if type_byte.len() != 1 || address.is_empty() {
        return StatusEvent::Unknown(inner);
    }

    let address_type = match type_byte[0] {
        b'0' => 0_u8,
        b'1' => 1_u8,
        _ => return StatusEvent::Unknown(inner),
    };

    StatusEvent::Connect {
        address_type,
        address,
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

    // --- Known status events ---

    #[test]
    fn parse_reboot_event() -> TestResult {
        // Given
        let inner = b"REBOOT";

        // When
        let event = parse(inner);

        // Then
        assert_eq!(event, StatusEvent::Reboot, "expected Reboot event");
        Ok(())
    }

    #[test]
    fn parse_disconnect_event() -> TestResult {
        // Given
        let inner = b"DISCONNECT";

        // When
        let event = parse(inner);

        // Then
        assert_eq!(event, StatusEvent::Disconnect, "expected Disconnect event");
        Ok(())
    }

    #[test]
    fn parse_stream_open_event() -> TestResult {
        // Given
        let inner = b"STREAM_OPEN";

        // When
        let event = parse(inner);

        // Then
        assert_eq!(event, StatusEvent::StreamOpen, "expected StreamOpen event");
        Ok(())
    }

    #[test]
    fn parse_connect_public_address() -> TestResult {
        // Given
        let inner = b"CONNECT,0,AABBCCDDEEFF";

        // When
        let event = parse(inner);

        // Then
        assert_eq!(
            event,
            StatusEvent::Connect {
                address_type: 0,
                address: b"AABBCCDDEEFF",
            },
            "expected Connect with public address"
        );
        Ok(())
    }

    #[test]
    fn parse_connect_random_address() -> TestResult {
        // Given
        let inner = b"CONNECT,1,112233445566";

        // When
        let event = parse(inner);

        // Then
        assert_eq!(
            event,
            StatusEvent::Connect {
                address_type: 1,
                address: b"112233445566",
            },
            "expected Connect with random address"
        );
        Ok(())
    }

    #[test]
    fn parse_conn_param_event() -> TestResult {
        // Given
        let inner = b"CONN_PARAM,0006,0000,01F4";

        // When
        let event = parse(inner);

        // Then
        assert_eq!(
            event,
            StatusEvent::ConnParam(b"0006,0000,01F4"),
            "expected ConnParam with parameters"
        );
        Ok(())
    }

    #[test]
    fn parse_conn_param_different_values() -> TestResult {
        // Given
        let inner = b"CONN_PARAM,0018,0000,01F4";

        // When
        let event = parse(inner);

        // Then
        assert_eq!(
            event,
            StatusEvent::ConnParam(b"0018,0000,01F4"),
            "expected ConnParam with different interval"
        );
        Ok(())
    }

    // --- Unknown / malformed events ---

    #[test]
    fn parse_unknown_event_returns_unknown() -> TestResult {
        // Given
        let inner = b"UNKNOWN_EVENT";

        // When
        let event = parse(inner);

        // Then
        assert_eq!(
            event,
            StatusEvent::Unknown(b"UNKNOWN_EVENT"),
            "expected Unknown for unrecognized event"
        );
        Ok(())
    }

    #[test]
    fn parse_connect_missing_address_returns_unknown() -> TestResult {
        // Given — malformed: no address after type
        let inner = b"CONNECT,0,";

        // When
        let event = parse(inner);

        // Then
        assert_eq!(
            event,
            StatusEvent::Unknown(b"CONNECT,0,"),
            "expected Unknown for malformed CONNECT with empty address"
        );
        Ok(())
    }

    #[test]
    fn parse_connect_missing_comma_returns_unknown() -> TestResult {
        // Given — malformed: no comma after CONNECT prefix
        let inner = b"CONNECT";

        // When
        let event = parse(inner);

        // Then
        assert_eq!(
            event,
            StatusEvent::Unknown(b"CONNECT"),
            "expected Unknown for CONNECT without parameters"
        );
        Ok(())
    }

    #[test]
    fn parse_connect_invalid_type_returns_unknown() -> TestResult {
        // Given — address type is not 0 or 1
        let inner = b"CONNECT,2,AABBCCDDEEFF";

        // When
        let event = parse(inner);

        // Then
        assert_eq!(
            event,
            StatusEvent::Unknown(b"CONNECT,2,AABBCCDDEEFF"),
            "expected Unknown for invalid address type"
        );
        Ok(())
    }

    #[test]
    fn parse_empty_input_returns_unknown() -> TestResult {
        // Given
        let inner = b"";

        // When
        let event = parse(inner);

        // Then
        assert_eq!(
            event,
            StatusEvent::Unknown(b""),
            "expected Unknown for empty input"
        );
        Ok(())
    }
}
// grcov exclude stop
