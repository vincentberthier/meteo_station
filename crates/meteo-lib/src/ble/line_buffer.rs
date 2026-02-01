//! Fixed-size line buffer for UART byte stream framing.
//!
//! Accumulates bytes from a UART and extracts complete lines delimited by
//! `\r` or `\n`. Designed for the RN4871 BLE module which terminates responses
//! with `\r\n` and wraps status events in `%` delimiters.

/// Fixed-size buffer that accumulates UART bytes and extracts complete lines.
///
/// Lines are delimited by `\r` or `\n`. Multiple consecutive line-ending
/// characters (e.g. `\r\n`) are collapsed — empty lines between them are
/// not emitted.
///
/// When the buffer is full, the current content is returned as-is to prevent
/// data loss (even if no line ending was seen).
pub struct LineBuffer<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> Default for LineBuffer<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> LineBuffer<N> {
    /// Creates a new empty line buffer.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buf: [0_u8; N],
            len: 0,
        }
    }

    /// Feeds incoming bytes into the buffer.
    ///
    /// After calling this, use [`for_each_line`](Self::for_each_line) to
    /// process all complete lines.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "self.len < N guard prevents overflow"
    )]
    pub fn push_bytes(&mut self, data: &[u8]) {
        for &b in data {
            if self.len < N {
                self.buf[self.len] = b;
                self.len += 1;
            } else {
                // Buffer full — caller should drain first.
                // Drop the byte to avoid overwriting.
                break;
            }
        }
    }

    /// Processes all complete lines currently in the buffer.
    ///
    /// Calls `f` once for each complete line found (delimited by `\r` or `\n`).
    /// If the buffer is full and no line ending is found, the entire buffer
    /// content is passed to `f` to prevent data loss.
    ///
    /// Line-ending characters are NOT included in the slices passed to `f`.
    /// Empty lines (from consecutive `\r\n`) are skipped automatically.
    ///
    /// Consumed data is removed from the buffer. Any incomplete trailing data
    /// is compacted to the front for the next call.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "index arithmetic is bounded by self.len"
    )]
    pub fn for_each_line<F: FnMut(&[u8])>(&mut self, mut f: F) {
        loop {
            // Skip leading line-ending characters
            let start = self.skip_line_endings(0);
            if start >= self.len {
                self.len = 0;
                return;
            }

            // Find the next line-ending character
            let mut i = start;
            let mut found = false;
            while i < self.len {
                if self.buf[i] == b'\r' || self.buf[i] == b'\n' {
                    found = true;
                    break;
                }
                i += 1;
            }

            if found {
                // Complete line: buf[start..i]
                f(&self.buf[start..i]);

                // Compact: shift remaining data to front
                let remaining_len = self.len - i;
                self.buf.copy_within(i..self.len, 0);
                self.len = remaining_len;
            } else if self.len >= N {
                // Buffer full, no line ending — flush as-is
                f(&self.buf[start..self.len]);
                self.len = 0;
                return;
            } else {
                // Incomplete line — compact leading line endings and keep
                if start > 0 {
                    self.buf.copy_within(start..self.len, 0);
                    self.len -= start;
                }
                return;
            }
        }
    }

    /// Returns the position past any leading line-ending characters from `from`.
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "pos increments while pos < self.len"
    )]
    const fn skip_line_endings(&self, from: usize) -> usize {
        let mut pos = from;
        while pos < self.len && (self.buf[pos] == b'\r' || self.buf[pos] == b'\n') {
            pos += 1;
        }
        pos
    }
}

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(test)]
mod tests {
    extern crate std;

    use core::{error, result};

    use std::boxed::Box;
    use std::vec;
    use std::vec::Vec;
    use test_log::test;

    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    /// Helper: collects all lines from a `for_each_line` call into a Vec.
    fn collect_lines(buf: &mut LineBuffer<128>) -> Vec<Vec<u8>> {
        let mut lines = vec![];
        buf.for_each_line(|line| lines.push(line.to_vec()));
        lines
    }

    /// Helper: collects lines from a small buffer.
    fn collect_lines_small(buf: &mut LineBuffer<8>) -> Vec<Vec<u8>> {
        let mut lines = vec![];
        buf.for_each_line(|line| lines.push(line.to_vec()));
        lines
    }

    /// Helper: collects lines from a tiny buffer.
    fn collect_lines_tiny(buf: &mut LineBuffer<4>) -> Vec<Vec<u8>> {
        let mut lines = vec![];
        buf.for_each_line(|line| lines.push(line.to_vec()));
        lines
    }

    #[test]
    fn empty_buffer_yields_no_lines() -> TestResult {
        // Given
        let mut buf = LineBuffer::<128>::new();

        // When
        let lines = collect_lines(&mut buf);

        // Then
        assert!(lines.is_empty(), "empty buffer should yield no lines");
        Ok(())
    }

    #[test]
    fn incomplete_line_yields_nothing() -> TestResult {
        // Given
        let mut buf = LineBuffer::<128>::new();
        buf.push_bytes(b"hello");

        // When
        let lines = collect_lines(&mut buf);

        // Then
        assert!(lines.is_empty(), "incomplete line should yield nothing");
        Ok(())
    }

    #[test]
    fn line_terminated_by_cr() -> TestResult {
        // Given
        let mut buf = LineBuffer::<128>::new();
        buf.push_bytes(b"AOK\r");

        // When
        let lines = collect_lines(&mut buf);

        // Then
        assert_eq!(lines, vec![b"AOK"], "should extract line before CR");
        Ok(())
    }

    #[test]
    fn line_terminated_by_lf() -> TestResult {
        // Given
        let mut buf = LineBuffer::<128>::new();
        buf.push_bytes(b"AOK\n");

        // When
        let lines = collect_lines(&mut buf);

        // Then
        assert_eq!(lines, vec![b"AOK"], "should extract line before LF");
        Ok(())
    }

    #[test]
    fn line_terminated_by_crlf() -> TestResult {
        // Given
        let mut buf = LineBuffer::<128>::new();
        buf.push_bytes(b"AOK\r\n");

        // When
        let lines = collect_lines(&mut buf);

        // Then
        assert_eq!(lines, vec![b"AOK"], "CRLF should produce one line");
        Ok(())
    }

    #[test]
    fn multiple_lines() -> TestResult {
        // Given
        let mut buf = LineBuffer::<128>::new();
        buf.push_bytes(b"AOK\r\nCMD> \r\n");

        // When
        let lines = collect_lines(&mut buf);

        // Then
        assert_eq!(
            lines,
            vec![b"AOK".to_vec(), b"CMD> ".to_vec()],
            "should extract two lines"
        );
        Ok(())
    }

    #[test]
    fn status_event_as_line() -> TestResult {
        // Given
        let mut buf = LineBuffer::<128>::new();
        buf.push_bytes(b"%REBOOT%\r\n");

        // When
        let lines = collect_lines(&mut buf);

        // Then
        assert_eq!(
            lines,
            vec![b"%REBOOT%"],
            "status event should be returned as a line"
        );
        Ok(())
    }

    #[test]
    fn incremental_push() -> TestResult {
        // Given
        let mut buf = LineBuffer::<128>::new();
        buf.push_bytes(b"AO");
        assert!(
            collect_lines(&mut buf).is_empty(),
            "partial data should yield nothing"
        );
        buf.push_bytes(b"K\r");

        // When
        let lines = collect_lines(&mut buf);

        // Then
        assert_eq!(lines, vec![b"AOK"], "incremental push should work");
        Ok(())
    }

    #[test]
    fn full_buffer_flushes_as_line() -> TestResult {
        // Given
        let mut buf = LineBuffer::<8>::new();
        buf.push_bytes(b"12345678");

        // When
        let lines = collect_lines_small(&mut buf);

        // Then
        assert_eq!(lines, vec![b"12345678"], "full buffer should flush as line");
        Ok(())
    }

    #[test]
    fn overflow_bytes_are_dropped() -> TestResult {
        // Given
        let mut buf = LineBuffer::<4>::new();
        buf.push_bytes(b"123456"); // 4 fit, 2 dropped

        // When
        let lines = collect_lines_tiny(&mut buf);

        // Then
        assert_eq!(lines, vec![b"1234"], "only first 4 bytes should be kept");
        Ok(())
    }

    #[test]
    fn sequential_drain_and_refill() -> TestResult {
        // Given
        let mut buf = LineBuffer::<128>::new();
        buf.push_bytes(b"first\r\n");
        let first = collect_lines(&mut buf);
        buf.push_bytes(b"second\r\n");

        // When
        let second = collect_lines(&mut buf);

        // Then
        assert_eq!(first, vec![b"first"], "first drain");
        assert_eq!(second, vec![b"second"], "second drain after refill");
        Ok(())
    }

    #[test]
    fn only_line_endings_yields_nothing() -> TestResult {
        // Given
        let mut buf = LineBuffer::<128>::new();
        buf.push_bytes(b"\r\n\r\n");

        // When
        let lines = collect_lines(&mut buf);

        // Then
        assert!(lines.is_empty(), "only line endings should yield nothing");
        Ok(())
    }

    #[test]
    fn blank_lines_between_content_are_skipped() -> TestResult {
        // Given
        let mut buf = LineBuffer::<128>::new();
        buf.push_bytes(b"line1\r\n\r\nline2\r\n");

        // When
        let lines = collect_lines(&mut buf);

        // Then
        assert_eq!(
            lines,
            vec![b"line1".to_vec(), b"line2".to_vec()],
            "blank lines should be skipped"
        );
        Ok(())
    }

    #[test]
    fn incomplete_data_preserved_across_pushes() -> TestResult {
        // Given
        let mut buf = LineBuffer::<128>::new();
        buf.push_bytes(b"hel");
        let empty = collect_lines(&mut buf);
        buf.push_bytes(b"lo\rworld\r");

        // When
        let lines = collect_lines(&mut buf);

        // Then
        assert!(empty.is_empty(), "first push incomplete");
        assert_eq!(
            lines,
            vec![b"hello".to_vec(), b"world".to_vec()],
            "data should be preserved across pushes"
        );
        Ok(())
    }
}
// grcov exclude stop
