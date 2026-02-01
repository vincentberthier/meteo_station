//! RN4871 command response parser.
//!
//! Parses individual lines received from the RN4871 UART into typed
//! [`Response`] values. Status events (`%...%`) are handled separately by
//! [`status_parser`](super::status_parser).

use super::response::Response;

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

/// Parses a single line received from the RN4871 UART.
///
/// The input should be a complete line as received from the module. Trailing
/// CR/LF characters are stripped before parsing.
///
/// # Command responses
///
/// Bare text like `AOK`, `ERR`, `CMD>`, `END` is matched against known command
/// response patterns.
///
/// # Fallback
///
/// Anything unrecognized is returned as [`Response::Data`], which covers
/// intermediate lines of multi-line responses (e.g. output from `LS`, `D`, `V`
/// commands).
///
/// # Note
///
/// Status events (`%...%` delimited) are **not** parsed here. They are
/// extracted from the raw buffer by [`LineBuffer::process_status_event`] and
/// parsed by [`status_parser::parse`](super::status_parser::parse).
#[must_use]
pub fn parse(line: &[u8]) -> Response<'_> {
    let trimmed = strip_line_endings(line);

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
    fn parse_status_event_syntax_returns_data() -> TestResult {
        // Given — %...% lines are no longer parsed here; they pass through as Data
        let line = b"%REBOOT%";

        // When
        let response = parse(line);

        // Then
        assert_eq!(
            response,
            Response::Data(b"%REBOOT%"),
            "status events are not parsed by the command response parser"
        );

        Ok(())
    }
}
// grcov exclude stop
