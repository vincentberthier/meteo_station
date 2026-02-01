//! RN4871 BLE module response parser.
//!
//! Parses individual lines received from the RN4871 UART into typed responses.
//! The caller is responsible for line-level framing (reading until CR or `%...%`
//! boundaries). This module only handles the parsing of complete lines.

/// Default status message delimiter used by the RN4871.
const STATUS_DELIMITER: u8 = b'%';

/// A parsed response from the RN4871 BLE module.
///
/// Each variant corresponds to a specific message type documented in the
/// RN4870/71 User Guide (DS50002466C). The `Data` variant captures any
/// unrecognized content, including intermediate lines of multi-line responses.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Response<'a> {
    /// Command acknowledged successfully (`AOK`).
    Aok,
    /// Command failed (`ERR`).
    Err,
    /// Command mode prompt (`CMD>` or `CMD`). Module is ready for commands.
    Cmd,
    /// Exited command mode (`END`). Module returned to data mode.
    End,
    /// Module has rebooted (`%REBOOT%`).
    Reboot,
    /// BLE connection established.
    /// `address_type`: 0 = public, 1 = random.
    /// `address`: ASCII hex MAC address (e.g. `b"AABBCCDDEEFF"`).
    Connect { address_type: u8, address: &'a [u8] },
    /// BLE connection lost (`%DISCONNECT%`).
    Disconnect,
    /// UART Transparent data pipe established (`%STREAM_OPEN%`).
    StreamOpen,
    /// Unrecognized or intermediate data. Captures multi-line response content
    /// (e.g. individual lines from `LS`, `D`, `V` commands) as well as any
    /// unknown messages.
    Data(&'a [u8]),
}

/// Strips trailing CR (`\r`) and LF (`\n`) characters from a byte slice.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "end > 0 guard prevents underflow"
)]
fn strip_line_endings(input: &[u8]) -> &[u8] {
    let mut end = input.len();
    while end > 0 && (input[end - 1] == b'\r' || input[end - 1] == b'\n') {
        end -= 1;
    }
    &input[..end]
}

/// Parses the inner content of a `%...%` status message.
fn parse_status_event(inner: &[u8]) -> Response<'_> {
    match inner {
        b"REBOOT" => Response::Reboot,
        b"DISCONNECT" => Response::Disconnect,
        b"STREAM_OPEN" => Response::StreamOpen,
        _ => parse_connect_event(inner),
    }
}

/// Attempts to parse a `CONNECT,<type>,<address>` status event.
/// Returns `Data` if the format doesn't match.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "comma_pos < rest.len() so +1 won't overflow"
)]
fn parse_connect_event(inner: &[u8]) -> Response<'_> {
    // Expected format: CONNECT,<addr_type>,<address>
    let Some(rest) = inner.strip_prefix(b"CONNECT,") else {
        return Response::Data(inner);
    };

    // Find the comma separating address_type from address
    let Some(comma_pos) = rest.iter().position(|&b| b == b',') else {
        return Response::Data(inner);
    };

    let type_byte = &rest[..comma_pos];
    let address = &rest[comma_pos + 1..];

    // Address type must be a single ASCII digit (0 or 1)
    if type_byte.len() != 1 || address.is_empty() {
        return Response::Data(inner);
    }

    let address_type = match type_byte[0] {
        b'0' => 0_u8,
        b'1' => 1_u8,
        _ => return Response::Data(inner),
    };

    Response::Connect {
        address_type,
        address,
    }
}

/// Parses a single line received from the RN4871 UART.
///
/// The input should be a complete line as received from the module. Trailing
/// CR/LF characters are stripped before parsing.
///
/// # Status events
///
/// Lines wrapped in `%` delimiters (e.g. `%REBOOT%`, `%CONNECT,0,AABB...%`)
/// are parsed as status events. These can appear in both command mode and data
/// mode.
///
/// # Command responses
///
/// Bare text like `AOK`, `ERR`, `CMD>`, `END` is matched against known command
/// response patterns.
///
/// # Fallback
///
/// Anything unrecognized is returned as [`Response::Data`], which also covers
/// intermediate lines of multi-line responses (e.g. output from `LS`, `D`, `V`
/// commands).
#[must_use]
#[expect(
    clippy::arithmetic_side_effects,
    reason = "len >= 2 guard prevents underflow"
)]
pub fn parse(line: &[u8]) -> Response<'_> {
    let trimmed = strip_line_endings(line);

    if trimmed.is_empty() {
        return Response::Data(trimmed);
    }

    // Check for status events: %...%
    if trimmed[0] == STATUS_DELIMITER
        && trimmed.len() >= 2
        && trimmed[trimmed.len() - 1] == STATUS_DELIMITER
    {
        let inner = &trimmed[1..trimmed.len() - 1];
        return parse_status_event(inner);
    }

    // Check for command responses
    match trimmed {
        b"AOK" => Response::Aok,
        b"ERR" => Response::Err,
        b"CMD>" | b"CMD> " | b"CMD" => Response::Cmd,
        b"END" => Response::End,
        _ => Response::Data(trimmed),
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

    // --- Command response tests ---

    #[test]
    fn parse_aok_response() -> TestResult {
        // Given
        let line = b"AOK\r";

        // When
        let response = parse(line);

        // Then
        assert_eq!(response, Response::Aok, "expected Aok response");

        Ok(())
    }

    #[test]
    fn parse_aok_without_cr() -> TestResult {
        // Given
        let line = b"AOK";

        // When
        let response = parse(line);

        // Then
        assert_eq!(
            response,
            Response::Aok,
            "expected Aok even without trailing CR"
        );

        Ok(())
    }

    #[test]
    fn parse_err_response() -> TestResult {
        // Given
        let line = b"ERR\r";

        // When
        let response = parse(line);

        // Then
        assert_eq!(response, Response::Err, "expected Err response");

        Ok(())
    }

    #[test]
    fn parse_cmd_prompt() -> TestResult {
        // Given
        let line = b"CMD>";

        // When
        let response = parse(line);

        // Then
        assert_eq!(response, Response::Cmd, "expected Cmd response");

        Ok(())
    }

    #[test]
    fn parse_cmd_prompt_with_trailing_space() -> TestResult {
        // Given
        let line = b"CMD> ";

        // When
        let response = parse(line);

        // Then
        assert_eq!(
            response,
            Response::Cmd,
            "expected Cmd response with trailing space"
        );

        Ok(())
    }

    #[test]
    fn parse_cmd_no_prompt() -> TestResult {
        // Given — when No Prompt bit (0x4000) is set in SR
        let line = b"CMD";

        // When
        let response = parse(line);

        // Then
        assert_eq!(
            response,
            Response::Cmd,
            "expected Cmd for no-prompt variant"
        );

        Ok(())
    }

    #[test]
    fn parse_end_response() -> TestResult {
        // Given
        let line = b"END\r";

        // When
        let response = parse(line);

        // Then
        assert_eq!(response, Response::End, "expected End response");

        Ok(())
    }

    // --- Status event tests ---

    #[test]
    fn parse_reboot_event() -> TestResult {
        // Given
        let line = b"%REBOOT%";

        // When
        let response = parse(line);

        // Then
        assert_eq!(response, Response::Reboot, "expected Reboot event");

        Ok(())
    }

    #[test]
    fn parse_disconnect_event() -> TestResult {
        // Given
        let line = b"%DISCONNECT%";

        // When
        let response = parse(line);

        // Then
        assert_eq!(response, Response::Disconnect, "expected Disconnect event");

        Ok(())
    }

    #[test]
    fn parse_stream_open_event() -> TestResult {
        // Given
        let line = b"%STREAM_OPEN%";

        // When
        let response = parse(line);

        // Then
        assert_eq!(response, Response::StreamOpen, "expected StreamOpen event");

        Ok(())
    }

    #[test]
    fn parse_connect_public_address() -> TestResult {
        // Given
        let line = b"%CONNECT,0,AABBCCDDEEFF%";

        // When
        let response = parse(line);

        // Then
        assert_eq!(
            response,
            Response::Connect {
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
        let line = b"%CONNECT,1,112233445566%";

        // When
        let response = parse(line);

        // Then
        assert_eq!(
            response,
            Response::Connect {
                address_type: 1,
                address: b"112233445566",
            },
            "expected Connect with random address"
        );

        Ok(())
    }

    // --- Data / fallback tests ---

    #[test]
    fn parse_unknown_text_returns_data() -> TestResult {
        // Given — version string from V command
        let line = b"RN4871 V1.40\r";

        // When
        let response = parse(line);

        // Then
        assert_eq!(
            response,
            Response::Data(b"RN4871 V1.40"),
            "expected Data with version string"
        );

        Ok(())
    }

    #[test]
    fn parse_empty_input_returns_data() -> TestResult {
        // Given
        let line = b"";

        // When
        let response = parse(line);

        // Then
        assert_eq!(
            response,
            Response::Data(b""),
            "expected Data for empty input"
        );

        Ok(())
    }

    #[test]
    fn parse_only_cr_returns_data() -> TestResult {
        // Given
        let line = b"\r";

        // When
        let response = parse(line);

        // Then
        assert_eq!(response, Response::Data(b""), "expected Data for bare CR");

        Ok(())
    }

    #[test]
    fn parse_unknown_status_event_returns_data() -> TestResult {
        // Given
        let line = b"%UNKNOWN_EVENT%";

        // When
        let response = parse(line);

        // Then
        assert_eq!(
            response,
            Response::Data(b"UNKNOWN_EVENT"),
            "expected Data for unknown status event"
        );

        Ok(())
    }

    #[test]
    fn parse_connect_missing_address_returns_data() -> TestResult {
        // Given — malformed: no address after type
        let line = b"%CONNECT,0,%";

        // When
        let response = parse(line);

        // Then
        assert_eq!(
            response,
            Response::Data(b"CONNECT,0,"),
            "expected Data for malformed CONNECT with empty address"
        );

        Ok(())
    }

    #[test]
    fn parse_connect_missing_comma_returns_data() -> TestResult {
        // Given — malformed: no comma after CONNECT
        let line = b"%CONNECT%";

        // When
        let response = parse(line);

        // Then
        assert_eq!(
            response,
            Response::Data(b"CONNECT"),
            "expected Data for CONNECT without parameters"
        );

        Ok(())
    }

    #[test]
    fn parse_connect_invalid_type_returns_data() -> TestResult {
        // Given — address type is not 0 or 1
        let line = b"%CONNECT,2,AABBCCDDEEFF%";

        // When
        let response = parse(line);

        // Then
        assert_eq!(
            response,
            Response::Data(b"CONNECT,2,AABBCCDDEEFF"),
            "expected Data for invalid address type"
        );

        Ok(())
    }

    #[test]
    fn parse_single_percent_returns_data() -> TestResult {
        // Given
        let line = b"%";

        // When
        let response = parse(line);

        // Then
        assert_eq!(
            response,
            Response::Data(b"%"),
            "expected Data for lone percent sign"
        );

        Ok(())
    }

    #[test]
    fn parse_reboot_with_trailing_cr() -> TestResult {
        // Given — status event followed by CR (common from UART)
        let line = b"%REBOOT%\r";

        // When
        let response = parse(line);

        // Then — trailing CR is stripped, then %REBOOT% is parsed normally
        assert_eq!(
            response,
            Response::Reboot,
            "trailing CR should be stripped before parsing"
        );

        Ok(())
    }
}
// grcov exclude stop
