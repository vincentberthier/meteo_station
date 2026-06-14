//! RN4871 ASCII-protocol BLE driver.
//!
//! Drives a Microchip RN4870/71 module via UART using the ASCII command
//! interface in "No-Prompt" mode (no `CMD>` terminator — responses key off
//! `AOK`/`ERR`).

// Suppress false positives from defmt macro expansion (only active when defmt feature is on).
#![cfg_attr(
    feature = "defmt",
    expect(
        clippy::missing_asserts_for_indexing,
        reason = "false positives from defmt macro expansion"
    )
)]

use core::fmt;
use core::str;

use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;
use embedded_io_async::{Read, Write};
use heapless::Vec as HVec;

use super::{CHAR_UUID, SERVICE_UUID};

// ── UUIDs as 32-char uppercase hex (no dashes) ───────────────────────────────

/// Format a `u128` as 32 uppercase hex bytes (ASCII) into `buf`.
///
/// `buf` must have capacity for at least 32 bytes.  Each push is infallible
/// because the buffer is sized to 36 and we emit exactly 32 bytes.
fn uuid_to_hex(uuid: u128, buf: &mut HVec<u8, 36>) {
    // Emit nibbles MSB-first: 128 bits / 4 bits per nibble = 32 nibbles.
    // Shifting u128 right by up to 124 and masking with 0xF is safe.
    // Cast to u8 is safe because we masked to [0, 15].
    let mut shift = 124_u32;
    loop {
        let nibble = ((uuid >> shift) & 0xF_u128) as u8;
        let ascii = nibble_to_hex(nibble);
        // buf has capacity 36; we push exactly 32 bytes — push never fails here.
        let _push_ok = buf.push(ascii);
        if shift == 0_u32 {
            break;
        }
        shift = shift.saturating_sub(4_u32);
    }
}

/// Convert a nibble (0–15) to its uppercase ASCII hex character.
const fn nibble_to_hex(n: u8) -> u8 {
    if n < 10_u8 {
        b'0'.saturating_add(n)
    } else {
        // n is in [10, 15]; n - 10 is in [0, 5]
        b'A'.saturating_add(n.saturating_sub(10_u8))
    }
}

// ── Line classification ───────────────────────────────────────────────────────

/// Internal classification of a line read from the RN4871.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    Aok,
    Err,
    Event(Event),
    Data,
}

/// Classify a raw line (without trailing `\r\n`).
fn classify(line: &[u8]) -> Line {
    if line == b"AOK" {
        Line::Aok
    } else if line == b"ERR" {
        Line::Err
    } else if line.len() >= 2_usize && line.first() == Some(&b'%') && line.last() == Some(&b'%') {
        let inner = &line[1_usize..line.len().saturating_sub(1_usize)];
        let event = if inner == b"REBOOT" {
            Event::Reboot
        } else if inner.starts_with(b"CONNECT") {
            Event::Connect
        } else if inner == b"DISCONNECT" {
            Event::Disconnect
        } else if inner == b"STREAM_OPEN" {
            Event::StreamOpen
        } else {
            Event::Other
        };
        Line::Event(event)
    } else {
        Line::Data
    }
}

// ── Public types ──────────────────────────────────────────────────────────────

/// Asynchronous events emitted by the RN4871 module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Event {
    /// Module finished booting (`%REBOOT%`).
    Reboot,
    /// A central connected (`%CONNECT,...%`).
    Connect,
    /// The central disconnected (`%DISCONNECT%`).
    Disconnect,
    /// Transparent UART stream opened (`%STREAM_OPEN%`).
    StreamOpen,
    /// Any other `%...%` event not explicitly handled.
    Other,
}

/// Errors produced by the RN4871 driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error<E> {
    /// A UART read or write failed.
    Io(E),
    /// The module responded with `ERR`, or a GPIO operation failed.
    Command,
    /// The operation timed out (reserved for callers that add timeouts).
    Timeout,
    /// The module returned a malformed or unexpected response.
    BadResponse,
    /// No characteristic value handle could be found in the `LS` listing.
    NoHandle,
    /// The module's firmware version is not supported.
    UnsupportedFirmware,
}

impl<E: fmt::Debug> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "UART I/O error: {e:?}"),
            Self::Command => write!(f, "module returned ERR"),
            Self::Timeout => write!(f, "operation timed out"),
            Self::BadResponse => write!(f, "malformed response from module"),
            Self::NoHandle => write!(f, "characteristic handle not found"),
            Self::UnsupportedFirmware => write!(f, "unsupported firmware version"),
        }
    }
}

#[expect(
    clippy::absolute_paths,
    reason = "core::error::Error path avoids use import that could shadow other Error types"
)]
impl<E: fmt::Debug> core::error::Error for Error<E> {}

// ── Driver ────────────────────────────────────────────────────────────────────

/// Hardware-agnostic async driver for the Microchip RN4870/71 BLE module.
///
/// `U` — UART (must implement `Read + Write` from `embedded_io_async`).
/// `R` — `RST_N` GPIO pin (active-low reset, `OutputPin`).
/// `D` — Delay provider (`DelayNs`).
pub struct Rn4871<U, R, D> {
    uart: U,
    reset: R,
    delay: D,
    char_handle: Option<u16>,
    events: heapless::Deque<Event, 4>,
}

impl<U, R, D, E> Rn4871<U, R, D>
where
    U: Read<Error = E> + Write<Error = E>,
    R: OutputPin,
    D: DelayNs,
{
    /// Construct a new driver, taking ownership of the UART, reset pin, and delay.
    pub const fn new(uart: U, reset: R, delay: D) -> Self {
        Self {
            uart,
            reset,
            delay,
            char_handle: None,
            events: heapless::Deque::new(),
        }
    }

    // ── Low-level I/O ─────────────────────────────────────────────────────────

    /// Write all bytes to the UART.
    ///
    /// # Errors
    ///
    /// Propagates UART I/O errors as [`Error::Io`].
    async fn write_all(&mut self, data: &[u8]) -> Result<(), Error<E>> {
        embedded_io_async::Write::write_all(&mut self.uart, data)
            .await
            .map_err(Error::Io)
    }

    /// Read exactly one byte from the UART.
    ///
    /// # Errors
    ///
    /// Propagates UART I/O errors as [`Error::Io`].
    async fn read_byte(&mut self) -> Result<u8, Error<E>> {
        let mut buf = [0_u8; 1];
        // Loop until we get a byte (Read::read may return 0 on some impls)
        loop {
            let n = Read::read(&mut self.uart, &mut buf)
                .await
                .map_err(Error::Io)?;
            if n > 0_usize {
                return Ok(buf[0_usize]);
            }
        }
    }

    /// Read one line from the UART into `buf`, stripping the trailing `\r`.
    ///
    /// Reads one byte at a time (cancel-safe).  Stops at `\n` or when `buf`
    /// reaches capacity.  The `\n` byte itself is not stored.
    ///
    /// # Errors
    ///
    /// Propagates UART I/O errors as [`Error::Io`].
    async fn read_line(&mut self, buf: &mut HVec<u8, 64>) -> Result<(), Error<E>> {
        buf.clear();
        loop {
            let byte = self.read_byte().await?;
            if byte == b'\n' {
                // Strip trailing \r if present
                if buf.last() == Some(&b'\r') {
                    buf.pop();
                }
                return Ok(());
            }
            // If full, treat as complete line (don't overflow)
            if buf.is_full() {
                return Ok(());
            }
            // push cannot fail: we just verified !is_full()
            let _push_ok = buf.push(byte);
        }
    }

    // ── Public API ────────────────────────────────────────────────────────────

    /// Pulse `RST_N` low for the datasheet minimum hold time, then wait for
    /// `%REBOOT%` (the real ready signal, not a fixed settle delay).
    ///
    /// GPIO pin errors are silenced (`pin.set_low().ok()`) because `Output`
    /// on Embassy STM32 uses `Infallible`, and adding a second generic error
    /// parameter for an infallible operation would complicate the API needlessly.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] on UART failure.
    pub async fn reset(&mut self) -> Result<(), Error<E>> {
        // Pull RST_N low; pin errors are infallible on this target
        self.reset.set_low().ok();
        // Minimum reset-pulse hold per datasheet (hardware timing minimum, not a guess)
        self.delay.delay_ms(2_u32).await;
        self.reset.set_high().ok();

        // Wait for the actual reboot signal from the module
        let mut line = HVec::<u8, 64>::new();
        loop {
            self.read_line(&mut line).await?;
            if classify(&line) == Line::Event(Event::Reboot) {
                return Ok(());
            }
        }
    }

    /// Enter command mode via the `$$$` sequence with mandatory guard delays.
    ///
    /// The 100 ms guards before and after `$$$` are required by the datasheet
    /// (the module must see silence around the escape sequence).  Command mode
    /// is confirmed by issuing `V` and accepting a version response — there is
    /// no `CMD>` prompt in No-Prompt mode.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] on UART failure, or [`Error::BadResponse`] if the
    /// version query returns an unparseable line.
    pub async fn enter_command_mode(&mut self) -> Result<(), Error<E>> {
        // Pre-guard silence (datasheet requirement)
        self.delay.delay_ms(100_u32).await;
        self.write_all(b"$$$").await?;
        // Post-guard silence (datasheet requirement)
        self.delay.delay_ms(100_u32).await;

        // Confirm command mode — read and discard version line
        let mut scratch = [0_u8; 2];
        let _n = self.query(b"V", &mut scratch).await?;
        Ok(())
    }

    /// Send `cmd\r`, routing `%...%` events to the internal queue.
    ///
    /// Returns `Ok(())` on `AOK`, or `Err(Error::Command)` on `ERR`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] on UART failure, or [`Error::Command`] when the
    /// module responds with `ERR`.
    pub async fn command(&mut self, cmd: &[u8]) -> Result<(), Error<E>> {
        self.write_all(cmd).await?;
        self.write_all(b"\r").await?;
        let mut line = HVec::<u8, 64>::new();
        loop {
            self.read_line(&mut line).await?;
            match classify(&line) {
                Line::Aok => return Ok(()),
                Line::Err => return Err(Error::Command),
                Line::Event(e) => {
                    // Buffer event; keep waiting for AOK/ERR
                    let _push_ok = self.events.push_back(e);
                }
                Line::Data => {
                    // Echoes or blank lines — ignore
                }
            }
        }
    }

    /// Send `cmd\r`, capture the first non-event data line into `out`.
    ///
    /// Returns the number of bytes written into `out`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] on UART failure.
    pub async fn query(&mut self, cmd: &[u8], out: &mut [u8]) -> Result<usize, Error<E>> {
        self.write_all(cmd).await?;
        self.write_all(b"\r").await?;
        let mut line = HVec::<u8, 64>::new();
        loop {
            self.read_line(&mut line).await?;
            match classify(&line) {
                Line::Event(e) => {
                    let _push_ok = self.events.push_back(e);
                }
                Line::Data | Line::Aok | Line::Err => {
                    // First data-ish line is the response
                    let n = line.len().min(out.len());
                    out[..n].copy_from_slice(&line[..n]);
                    return Ok(n);
                }
            }
        }
    }

    /// Query the firmware version.
    ///
    /// Returns `(major, minor)` parsed from the version line (e.g.
    /// `"RN4871 V1.40 ..."` → `(1, 40)`).
    ///
    /// # Errors
    ///
    /// Returns [`Error::BadResponse`] if the line cannot be parsed, or
    /// [`Error::Io`] on UART failure.
    pub async fn firmware_version(&mut self) -> Result<(u8, u8), Error<E>> {
        let mut buf = [0_u8; 64];
        let n = self.query(b"V", &mut buf).await?;
        let line = &buf[..n];

        // Find 'V' byte followed by "major.minor"
        // Format: b"RN4871 V1.40 ..."
        let v_pos = line
            .iter()
            .position(|&b| b == b'V')
            .ok_or(Error::BadResponse)?;

        // Version string starts immediately after 'V'
        let after_v = line
            .get(v_pos.saturating_add(1_usize)..)
            .ok_or(Error::BadResponse)?;

        // Find the '.' separator
        let dot_pos = after_v
            .iter()
            .position(|&b| b == b'.')
            .ok_or(Error::BadResponse)?;

        let major_bytes = after_v.get(..dot_pos).ok_or(Error::BadResponse)?;
        let major_str = str::from_utf8(major_bytes).map_err(|_utf8_err| Error::BadResponse)?;
        let major = major_str
            .parse::<u8>()
            .map_err(|_parse_err| Error::BadResponse)?;

        // Minor: bytes after '.' until non-digit
        let after_dot = after_v
            .get(dot_pos.saturating_add(1_usize)..)
            .ok_or(Error::BadResponse)?;
        let minor_end = after_dot
            .iter()
            .position(|&b| !b.is_ascii_digit())
            .unwrap_or(after_dot.len());
        let minor_bytes = after_dot.get(..minor_end).ok_or(Error::BadResponse)?;
        let minor_str = str::from_utf8(minor_bytes).map_err(|_utf8_err| Error::BadResponse)?;
        let minor = minor_str
            .parse::<u8>()
            .map_err(|_parse_err| Error::BadResponse)?;

        Ok((major, minor))
    }

    /// Provision the module with the `MeteoStation` service and characteristic.
    ///
    /// Issues the provisioning command sequence in order, triggers a reboot,
    /// re-enters command mode, and discovers the characteristic handle.
    ///
    /// Note: this implementation always writes the full provisioning sequence
    /// (no verify-and-skip optimisation).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] on UART failure, [`Error::Command`] if any
    /// command is rejected, or [`Error::NoHandle`] if the characteristic
    /// handle cannot be discovered after provisioning.
    pub async fn provision(&mut self) -> Result<(), Error<E>> {
        self.command(b"SN,MeteoStation").await?;
        self.command(b"SS,80").await?;
        self.command(b"PZ").await?;

        // Format SERVICE_UUID as 32 uppercase hex chars
        let mut svc_uuid_buf = HVec::<u8, 36>::new();
        uuid_to_hex(SERVICE_UUID, &mut svc_uuid_buf);

        // Build "PS,<uuid32>" command
        let mut set_service_cmd = HVec::<u8, 40>::new();
        let _ext_ok = set_service_cmd.extend_from_slice(b"PS,");
        let _ext_ok2 = set_service_cmd.extend_from_slice(&svc_uuid_buf);
        self.command(&set_service_cmd).await?;

        // Format CHAR_UUID as 32 uppercase hex chars
        let mut char_uuid_buf = HVec::<u8, 36>::new();
        uuid_to_hex(CHAR_UUID, &mut char_uuid_buf);

        // Build "PC,<uuid32>,10,14" command
        // Property byte 0x10 = notify; size 0x14 = 20 decimal bytes
        let mut add_char_cmd = HVec::<u8, 48>::new();
        let _ext_ok3 = add_char_cmd.extend_from_slice(b"PC,");
        let _ext_ok4 = add_char_cmd.extend_from_slice(&char_uuid_buf);
        let _ext_ok5 = add_char_cmd.extend_from_slice(b",10,14");
        self.command(&add_char_cmd).await?;

        self.command(b"SR,4000").await?;
        self.command(b"WR").await?;

        // Trigger reboot
        self.command(b"R,1").await?;

        // Wait for reboot event
        let mut line = HVec::<u8, 64>::new();
        loop {
            self.read_line(&mut line).await?;
            if classify(&line) == Line::Event(Event::Reboot) {
                break;
            }
        }

        // Re-enter command mode
        self.enter_command_mode().await?;

        // Discover characteristic handle
        self.discover_char_handle().await?;

        Ok(())
    }

    /// Issue `LS` and parse the output to find the value handle for `CHAR_UUID`.
    ///
    /// Stores the handle in `self.char_handle` and also returns it.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoHandle`] if the characteristic UUID is not listed,
    /// [`Error::BadResponse`] if a handle field cannot be parsed, or
    /// [`Error::Io`] on UART failure.
    pub async fn discover_char_handle(&mut self) -> Result<u16, Error<E>> {
        self.write_all(b"LS").await?;
        self.write_all(b"\r").await?;

        // Format CHAR_UUID as 32 uppercase hex chars for comparison
        let mut char_uuid_buf = HVec::<u8, 36>::new();
        uuid_to_hex(CHAR_UUID, &mut char_uuid_buf);

        let mut line = HVec::<u8, 64>::new();
        loop {
            self.read_line(&mut line).await?;

            // Stop at END marker
            if &*line == b"END" {
                return Err(Error::NoHandle);
            }

            // Route events, skip non-data lines
            match classify(&line) {
                Line::Event(e) => {
                    let _push_ok = self.events.push_back(e);
                    continue;
                }
                Line::Aok | Line::Err => {
                    continue;
                }
                Line::Data => {}
            }

            // Trim leading whitespace
            let trimmed = trim_leading(line.as_slice());

            // Find first comma
            let Some(comma1) = trimmed.iter().position(|&b| b == b',') else {
                continue;
            };

            let uuid_field = &trimmed[..comma1];

            // Compare case-insensitively with our CHAR_UUID
            if !uuid_field.eq_ignore_ascii_case(&char_uuid_buf) {
                continue;
            }

            // Get second field (handle)
            let after_comma1 = trimmed.get(comma1.saturating_add(1_usize)..).unwrap_or(&[]);
            let comma2 = after_comma1.iter().position(|&b| b == b',');
            let handle_bytes =
                comma2.map_or(after_comma1, |pos| after_comma1.get(..pos).unwrap_or(&[]));

            let handle_str =
                str::from_utf8(handle_bytes).map_err(|_utf8_err| Error::BadResponse)?;
            let handle = u16::from_str_radix(handle_str.trim(), 16_u32)
                .map_err(|_parse_err| Error::BadResponse)?;

            self.char_handle = Some(handle);
            return Ok(handle);
        }
    }

    /// Start BLE advertising by sending the `A` command.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] on UART failure, or [`Error::Command`] if the
    /// module rejects the command.
    pub async fn start_advertising(&mut self) -> Result<(), Error<E>> {
        self.command(b"A").await
    }

    /// Encode `frame` bytes as hex and write them to the characteristic via `SHW`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NoHandle`] if `discover_char_handle` has not been
    /// called successfully, [`Error::Io`] on UART failure, or
    /// [`Error::Command`] if the module rejects the write.
    pub async fn push_frame(&mut self, frame: &[u8]) -> Result<(), Error<E>> {
        let handle = self.char_handle.ok_or(Error::NoHandle)?;

        // "SHW," + 4 hex chars for handle + "," + 2*frame.len() hex chars.
        // Frame is at most FRAME_LEN=17 bytes → 34 hex chars.
        // Total: 4 + 4 + 1 + 34 = 43; capacity 64 gives comfortable headroom.
        let mut cmd = HVec::<u8, 64>::new();
        let _ext_ok = cmd.extend_from_slice(b"SHW,");

        // Append handle as 4 uppercase hex digits
        push_handle_hex(&mut cmd, handle);

        let _push_ok = cmd.push(b',');

        // Frame bytes as 2 uppercase hex chars each
        for &byte in frame {
            let hi = byte >> 4_u8;
            let lo = byte & 0xF_u8;
            let _push_hi = cmd.push(nibble_to_hex(hi));
            let _push_lo = cmd.push(nibble_to_hex(lo));
        }

        self.command(&cmd).await
    }

    /// Return the next buffered event, or read one line from the UART and
    /// classify it as an event.
    ///
    /// # Errors
    ///
    /// Returns [`Error::BadResponse`] if the line is not an event, or
    /// [`Error::Io`] on UART failure.
    pub async fn next_event(&mut self) -> Result<Event, Error<E>> {
        if let Some(e) = self.events.pop_front() {
            return Ok(e);
        }
        let mut line = HVec::<u8, 64>::new();
        self.read_line(&mut line).await?;
        match classify(&line) {
            Line::Event(e) => Ok(e),
            Line::Aok | Line::Err | Line::Data => Err(Error::BadResponse),
        }
    }
}

// ── Free helpers ──────────────────────────────────────────────────────────────

/// Append a `u16` value handle as 4 uppercase ASCII hex digits to `cmd`.
///
/// Nibble extraction uses right-shifts and bit-masks on `u16`.
fn push_handle_hex(cmd: &mut HVec<u8, 64>, handle: u16) {
    let n3 = ((handle >> 12_u16) & 0xF_u16) as u8;
    let n2 = ((handle >> 8_u16) & 0xF_u16) as u8;
    let n1 = ((handle >> 4_u16) & 0xF_u16) as u8;
    let n0 = (handle & 0xF_u16) as u8;
    let _p3 = cmd.push(nibble_to_hex(n3));
    let _p2 = cmd.push(nibble_to_hex(n2));
    let _p1 = cmd.push(nibble_to_hex(n1));
    let _p0 = cmd.push(nibble_to_hex(n0));
}

/// Trim leading ASCII whitespace from a byte slice.
fn trim_leading(s: &[u8]) -> &[u8] {
    let start = s
        .iter()
        .position(|&b| b != b' ' && b != b'\t')
        .unwrap_or(s.len());
    &s[start..]
}

// ── Tests ─────────────────────────────────────────────────────────────────────

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(test)]
mod tests {
    extern crate std;

    use core::convert::Infallible;
    use core::str;
    use std::boxed::Box;
    use std::collections::VecDeque;
    use std::error;
    use std::result;
    use std::vec::Vec;

    use embedded_hal::digital::{ErrorType as PinErrorType, OutputPin};
    use embedded_hal_async::delay::DelayNs;
    use embedded_io_async::{ErrorType as UartErrorType, Read, Write};
    use test_log::test;

    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    // ── Fakes ─────────────────────────────────────────────────────────────────

    /// Fake UART backed by byte queues.
    struct FakeUart {
        rx: VecDeque<u8>,
        tx: Vec<u8>,
    }

    impl FakeUart {
        fn new(rx_data: &[u8]) -> Self {
            Self {
                rx: rx_data.iter().copied().collect(),
                tx: Vec::new(),
            }
        }
    }

    impl UartErrorType for FakeUart {
        type Error = Infallible;
    }

    impl Read for FakeUart {
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Infallible> {
            if buf.is_empty() || self.rx.is_empty() {
                return Ok(0_usize);
            }
            buf[0_usize] = self.rx.pop_front().unwrap_or_default();
            Ok(1_usize)
        }
    }

    impl Write for FakeUart {
        async fn write(&mut self, buf: &[u8]) -> Result<usize, Infallible> {
            self.tx.extend_from_slice(buf);
            Ok(buf.len())
        }

        async fn flush(&mut self) -> Result<(), Infallible> {
            Ok(())
        }
    }

    /// Fake GPIO pin (no-op, infallible).
    struct FakePin;

    impl PinErrorType for FakePin {
        type Error = Infallible;
    }

    impl OutputPin for FakePin {
        fn set_high(&mut self) -> Result<(), Infallible> {
            Ok(())
        }

        fn set_low(&mut self) -> Result<(), Infallible> {
            Ok(())
        }
    }

    /// Fake delay that returns immediately (no wall-clock waits in tests).
    struct FakeDelay;

    impl DelayNs for FakeDelay {
        async fn delay_ns(&mut self, _ns: u32) {}
    }

    /// Convenience: build a driver with the given rx bytes pre-loaded.
    fn make_driver(rx_data: &[u8]) -> Rn4871<FakeUart, FakePin, FakeDelay> {
        Rn4871::new(FakeUart::new(rx_data), FakePin, FakeDelay)
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[test(tokio::test)]
    async fn command_returns_ok_on_aok() -> TestResult {
        // Given
        let mut drv = make_driver(b"AOK\r\n");

        // When
        let result = drv.command(b"SN,MeteoStation").await;

        // Then
        assert!(result.is_ok(), "expected Ok but got {result:?}");
        assert!(
            drv.uart.tx.starts_with(b"SN,MeteoStation\r"),
            "tx should contain the command with \\r"
        );

        Ok(())
    }

    #[test(tokio::test)]
    async fn command_returns_err_on_err() -> TestResult {
        // Given
        let mut drv = make_driver(b"ERR\r\n");

        // When
        let result = drv.command(b"SN,MeteoStation").await;

        // Then
        assert!(
            matches!(result, Err(Error::Command)),
            "expected Err(Command) but got {result:?}"
        );

        Ok(())
    }

    #[test(tokio::test)]
    async fn command_returns_err_after_event() -> TestResult {
        // Given — event arrives before ERR
        let mut drv = make_driver(b"%CONNECT,0,001122334455%\r\nERR\r\n");

        // When
        let result = drv.command(b"TEST").await;

        // Then — command returns Err(Command)
        assert!(
            matches!(result, Err(Error::Command)),
            "expected Err(Command) but got {result:?}"
        );

        // And the event was buffered
        let event = drv.next_event().await;
        assert!(
            matches!(event, Ok(Event::Connect)),
            "expected buffered Connect event but got {event:?}"
        );

        Ok(())
    }

    #[test(tokio::test)]
    async fn status_events_are_routed_while_awaiting_aok() -> TestResult {
        // Given — event arrives before AOK
        let mut drv = make_driver(b"%CONNECT,0,001122334455%\r\nAOK\r\n");

        // When
        let result = drv.command(b"TEST").await;

        // Then — command returns Ok
        assert!(result.is_ok(), "expected Ok but got {result:?}");

        // And the event was buffered
        let event = drv.next_event().await;
        assert!(
            matches!(event, Ok(Event::Connect)),
            "expected buffered Connect event but got {event:?}"
        );

        Ok(())
    }

    #[test(tokio::test)]
    async fn next_event_parses_disconnect() -> TestResult {
        // Given
        let mut drv = make_driver(b"%DISCONNECT%\r\n");

        // When
        let event = drv.next_event().await;

        // Then
        assert!(
            matches!(event, Ok(Event::Disconnect)),
            "expected Disconnect event but got {event:?}"
        );

        Ok(())
    }

    #[test(tokio::test)]
    async fn firmware_version_parses_major_minor() -> TestResult {
        // Given
        let mut drv = make_driver(b"RN4871 V1.40\r\n");

        // When
        let version = drv.firmware_version().await;

        // Then
        assert!(
            matches!(version, Ok((1_u8, 40_u8))),
            "expected (1, 40) but got {version:?}"
        );

        Ok(())
    }

    #[test(tokio::test)]
    async fn discover_char_handle_parses_ls() -> TestResult {
        // Given — representative LS fixture followed by AOK for the push_frame call
        let ls_fixture = b"7E9A0001B5A34F6E9C112D4E6F8A0B1C\r\n  7E9A0002B5A34F6E9C112D4E6F8A0B1C,0072,10\r\nEND\r\n";
        let mut rx = Vec::new();
        rx.extend_from_slice(ls_fixture);
        rx.extend_from_slice(b"AOK\r\n");
        let mut drv = make_driver(&rx);

        // When
        let handle = drv.discover_char_handle().await;

        // Then — handle is 0x0072
        assert!(
            matches!(handle, Ok(0x0072_u16)),
            "expected handle 0x0072 but got {handle:?}"
        );

        // And push_frame targets the right handle
        let push_result = drv.push_frame(&[0x01_u8, 0xAB_u8]).await;
        assert!(
            push_result.is_ok(),
            "push_frame should succeed: {push_result:?}"
        );
        assert!(
            drv.uart.tx.windows(8_usize).any(|w| w == b"SHW,0072"),
            "tx should contain SHW,0072 but tx was: {:?}",
            str::from_utf8(&drv.uart.tx)
        );

        Ok(())
    }

    #[test(tokio::test)]
    async fn discover_char_handle_missing_returns_no_handle() -> TestResult {
        // Given — LS block with only the service line and END (no char UUID)
        let ls_fixture = b"7E9A0001B5A34F6E9C112D4E6F8A0B1C\r\nEND\r\n";
        let mut drv = make_driver(ls_fixture);

        // When
        let result = drv.discover_char_handle().await;

        // Then
        assert!(
            matches!(result, Err(Error::NoHandle)),
            "expected NoHandle but got {result:?}"
        );

        Ok(())
    }

    #[test(tokio::test)]
    async fn push_frame_emits_shw_with_hex() -> TestResult {
        // Given — set handle via discover_char_handle, then preload AOK for push_frame
        let ls_fixture = b"7E9A0001B5A34F6E9C112D4E6F8A0B1C\r\n  7E9A0002B5A34F6E9C112D4E6F8A0B1C,0072,10\r\nEND\r\n";
        let mut rx = Vec::new();
        rx.extend_from_slice(ls_fixture);
        rx.extend_from_slice(b"AOK\r\n");
        let mut drv = make_driver(&rx);

        let _handle = drv.discover_char_handle().await?;

        // When
        let result = drv.push_frame(&[0x01_u8, 0xAB_u8]).await;

        // Then
        assert!(result.is_ok(), "push_frame should return Ok: {result:?}");
        let tx_str = str::from_utf8(&drv.uart.tx).unwrap_or("(invalid utf8)");
        assert!(
            drv.uart
                .tx
                .windows(14_usize)
                .any(|w| w == b"SHW,0072,01AB\r"),
            "tx should contain SHW,0072,01AB\\r but tx was: {tx_str}"
        );

        Ok(())
    }
}
// grcov exclude end
