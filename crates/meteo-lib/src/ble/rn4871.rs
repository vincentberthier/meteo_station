//! RN4871 BLE module driver.
//!
//! Minimal link driver: no GATT, no services. Communicates with the module
//! over UART using the RN4871 ASCII command protocol (115200 8N1).
//!
//! # Protocol overview
//!
//! - Commands are sent as ASCII ending with `\r` (CR only, no LF).
//! - Success reply: `AOK\r\n`; failure reply: `ERR\r\n` (matched case-insensitively).
//! - Status events are `%<name>%` frames with **no** trailing newline.
//! - Command-mode prompt `CMD> ` has **no** trailing newline.
//! - With the `SR,4000` "No-Prompt" feature bit set the `CMD> ` prompt is suppressed.

// The defmt::Format proc-macro emits internal slice-indexing code (inside
// defmt::unreachable!) that triggers missing_asserts_for_indexing. This is
// entirely within the generated code, not our own indexing logic.
#![allow(
    clippy::missing_asserts_for_indexing,
    reason = "defmt::Format derive macro uses internal slice indexing — not our code"
)]

use core::error::Error as StdError;
use core::fmt;

use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;
use embedded_io_async::{Read as AsyncRead, Write as AsyncWrite};
use heapless::{Deque, Vec};

/// Events emitted by the RN4871 module as `%…%`-delimited status frames.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Event {
    /// `%REBOOT%` — module has finished rebooting.
    Reboot,
    /// `%CONNECT,<0-1>,<addr>%` — a central has connected.
    Connect,
    /// `%DISCONNECT%` — the connection has been dropped.
    Disconnect,
    /// `%STREAM_OPEN%` — Transparent UART pipe is ready.
    StreamOpen,
    /// Any other `%…%` event not specifically matched.
    Other,
}

/// Errors returned by [`Rn4871`] methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error<E> {
    /// A UART read or write failed.
    Io(E),
    /// The module replied `ERR` (matched case-insensitively).
    Command,
    /// Reserved: the caller added a timeout as a deadlock circuit-breaker.
    Timeout,
    /// Malformed or unexpected module response (e.g. unparseable version string).
    BadResponse,
}

impl<E: fmt::Display> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "UART I/O error: {e}"),
            Self::Command => f.write_str("RN4871 returned ERR"),
            Self::Timeout => f.write_str("RN4871 operation timed out"),
            Self::BadResponse => f.write_str("RN4871 response was malformed"),
        }
    }
}

impl<E: fmt::Display + fmt::Debug> StdError for Error<E> {}

// ---------------------------------------------------------------------------
// Internal line classifier
// ---------------------------------------------------------------------------

/// Internal representation of a parsed line / frame from the module.
#[derive(PartialEq, Eq, Debug)]
enum Line {
    Aok,
    Err,
    Event(Event),
    Prompt,
    Data,
}

/// Classify a raw byte slice (stripped of trailing `\r\n`) into a [`Line`].
///
/// Matching rules (in order):
/// 1. `AOK` (case-insensitive) → [`Line::Aok`]
/// 2. `ERR` (case-insensitive) → [`Line::Err`]
/// 3. Starts and ends with `%`, length ≥ 2 → [`Line::Event`]
/// 4. Starts with `CMD>` → [`Line::Prompt`]
/// 5. Everything else → [`Line::Data`]
fn classify(line: &[u8]) -> Line {
    if line.eq_ignore_ascii_case(b"AOK") {
        Line::Aok
    } else if line.eq_ignore_ascii_case(b"ERR") {
        Line::Err
    } else if line.len() >= 2 && line.first() == Some(&b'%') && line.last() == Some(&b'%') {
        let inner = &line[1..line.len().saturating_sub(1)];
        Line::Event(if inner == b"REBOOT" {
            Event::Reboot
        } else if inner.starts_with(b"CONNECT") {
            Event::Connect
        } else if inner.starts_with(b"DISCONNECT") {
            Event::Disconnect
        } else if inner == b"STREAM_OPEN" {
            Event::StreamOpen
        } else {
            Event::Other
        })
    } else if line.starts_with(b"CMD>") {
        Line::Prompt
    } else {
        Line::Data
    }
}

// ---------------------------------------------------------------------------
// Driver struct
// ---------------------------------------------------------------------------

/// Driver for the Microchip RN4871 BLE module.
///
/// Communicates over a single UART interface. No GATT services are used; the
/// driver only manages GAP (advertising, connection events) and module
/// configuration.
///
/// # Type parameters
///
/// - `U` — UART peripheral implementing both [`AsyncRead`] and [`AsyncWrite`].
/// - `R` — GPIO output pin connected to the module's active-low `RST_N` pin.
/// - `D` — Async delay provider.
pub struct Rn4871<U, R, D> {
    uart: U,
    reset: R,
    delay: D,
    events: Deque<Event, 4>,
}

impl<U, R, D, E> Rn4871<U, R, D>
where
    U: AsyncRead<Error = E> + AsyncWrite<Error = E>,
    R: OutputPin,
    D: DelayNs,
{
    /// Creates a new driver instance wrapping the provided peripherals.
    pub const fn new(uart: U, reset: R, delay: D) -> Self {
        Self {
            uart,
            reset,
            delay,
            events: Deque::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Low-level UART helpers
    // -----------------------------------------------------------------------

    /// Write all bytes in `buf` to the UART. Maps UART errors to [`Error::Io`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the UART write fails.
    async fn write_all(&mut self, buf: &[u8]) -> Result<(), Error<E>> {
        self.uart.write_all(buf).await.map_err(Error::Io)
    }

    /// Read exactly one byte from the UART.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the UART read fails.
    async fn read_byte(&mut self) -> Result<u8, Error<E>> {
        let mut b = [0_u8; 1];
        self.uart.read(&mut b).await.map_err(Error::Io)?;
        Ok(b[0])
    }

    // -----------------------------------------------------------------------
    // Line reader
    // -----------------------------------------------------------------------

    /// Read one logical message from the UART into `buf`, which is cleared first.
    ///
    /// Returns when the FIRST of the following conditions is met:
    ///
    /// 1. A `\n` byte is read — the trailing `\r\n` is stripped.
    /// 2. The buffer currently starts with `%` and the just-pushed byte is
    ///    also `%` (closes a `%…%` event frame), with length ≥ 2.
    /// 3. The buffer ends with `CMD> ` (no newline follows).
    /// 4. The buffer reaches its capacity (64 bytes) — returned as-is.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if a UART read fails.
    async fn read_line(&mut self, buf: &mut Vec<u8, 64>) -> Result<(), Error<E>> {
        buf.clear();
        loop {
            let b = self.read_byte().await?;

            // Condition 1: newline — strip trailing \r and return.
            if b == b'\n' {
                if buf.last() == Some(&b'\r') {
                    buf.truncate(buf.len().saturating_sub(1));
                }
                return Ok(());
            }

            // Push byte; Vec::push returns Err when full but we check capacity next.
            buf.push(b).ok();

            // Condition 2: closed %…% event frame.
            if buf.len() >= 2 && buf.first() == Some(&b'%') && buf.last() == Some(&b'%') {
                return Ok(());
            }

            // Condition 3: ends with "CMD> ".
            if buf.ends_with(b"CMD> ") {
                return Ok(());
            }

            // Condition 4: buffer full.
            if buf.is_full() {
                return Ok(());
            }
        }
    }

    // -----------------------------------------------------------------------
    // Mid-level helpers
    // -----------------------------------------------------------------------

    /// Push an event to the internal queue, discarding it silently if full.
    fn push_event(&mut self, event: Event) {
        self.events.push_back(event).ok();
    }

    /// Send `cmd` followed by `\r`, then read and classify lines until `AOK`
    /// or `ERR`. Event lines are buffered; `Prompt` and `Data` lines are
    /// silently skipped.
    ///
    /// Returns `Ok(())` on `AOK`, `Err(Error::Command)` on `ERR`.
    ///
    /// # Errors
    ///
    /// - [`Error::Io`] if UART communication fails.
    /// - [`Error::Command`] if the module replies `ERR`.
    pub async fn command(&mut self, cmd: &[u8]) -> Result<(), Error<E>> {
        self.write_all(cmd).await?;
        self.write_all(b"\r").await?;
        let mut buf: Vec<u8, 64> = Vec::new();
        loop {
            self.read_line(&mut buf).await?;
            match classify(&buf) {
                Line::Aok => return Ok(()),
                Line::Err => return Err(Error::Command),
                Line::Event(e) => self.push_event(e),
                Line::Prompt | Line::Data => { /* skip */ }
            }
        }
    }

    /// Send `cmd` followed by `\r`, then read lines and return the byte count
    /// of the first `Data`, `Aok`, or `Err` line, copying bytes into `out`.
    ///
    /// # Semantics
    ///
    /// - `Event` lines are buffered into the internal event queue.
    /// - `Prompt` lines (`CMD> `) are skipped.
    /// - The **first** non-prompt, non-event line is returned as data.
    ///   `AOK` and `ERR` are also treated as data payloads here because some
    ///   commands (e.g. `V`) emit a response line **without** a following `AOK`.
    ///   A subsequent `CMD> ` prompt (if prompts are enabled) remains in the
    ///   stream and will be consumed and silently skipped by the next call.
    ///
    /// Returns the number of bytes written into `out`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if UART communication fails.
    pub async fn query(&mut self, cmd: &[u8], out: &mut [u8]) -> Result<usize, Error<E>> {
        self.write_all(cmd).await?;
        self.write_all(b"\r").await?;
        let mut buf: Vec<u8, 64> = Vec::new();
        loop {
            self.read_line(&mut buf).await?;
            match classify(&buf) {
                Line::Event(e) => self.push_event(e),
                Line::Prompt => { /* skip */ }
                Line::Aok | Line::Err | Line::Data => {
                    let n = buf.len().min(out.len());
                    out[..n].copy_from_slice(&buf[..n]);
                    return Ok(n);
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Public API
    // -----------------------------------------------------------------------

    /// Assert the hardware reset pin, wait for the `%REBOOT%` event.
    ///
    /// Pulses `RST_N` low for 2 ms (the datasheet recommends > 1 ms), then
    /// reads lines until `%REBOOT%` is received. No fixed post-reboot settle
    /// delay is inserted; the caller observes the real signal.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if UART communication fails while waiting for
    /// the reboot event.
    pub async fn reset(&mut self) -> Result<(), Error<E>> {
        // Pulse RST_N low for 2 ms (datasheet recommends > 1 ms).
        // OutputPin::set_low/set_high errors are ignored: the pin type's
        // error is constrained to Infallible in all realistic configurations.
        self.reset.set_low().ok();
        self.delay.delay_ms(2).await;
        self.reset.set_high().ok();
        self.wait_for_reboot().await
    }

    /// Wait until the module emits `%REBOOT%`, draining any preceding lines.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if UART communication fails.
    async fn wait_for_reboot(&mut self) -> Result<(), Error<E>> {
        let mut buf: Vec<u8, 64> = Vec::new();
        loop {
            self.read_line(&mut buf).await?;
            if classify(&buf) == Line::Event(Event::Reboot) {
                return Ok(());
            }
        }
    }

    /// Enter command mode by sending `$$$`.
    ///
    /// Inserts the 100 ms guard silence required before the first `$`, then
    /// sends `$$$`, waits 100 ms, and verifies command mode by issuing a `V`
    /// query and discarding the version response line.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if UART communication fails.
    pub async fn enter_command_mode(&mut self) -> Result<(), Error<E>> {
        self.delay.delay_ms(100).await;
        self.write_all(b"$$$").await?;
        self.delay.delay_ms(100).await;
        let mut buf = [0_u8; 64];
        let _n = self.query(b"V", &mut buf).await?;
        Ok(())
    }

    /// Parse the firmware version from the module.
    ///
    /// Issues the `V` command and parses `V<major>.<minor>` from the response.
    ///
    /// # Errors
    ///
    /// - [`Error::Io`] if UART communication fails.
    /// - [`Error::BadResponse`] if the version string cannot be parsed.
    pub async fn firmware_version(&mut self) -> Result<(u8, u8), Error<E>> {
        let mut buf = [0_u8; 64];
        let n = self.query(b"V", &mut buf).await?;
        parse_version(&buf[..n]).ok_or(Error::BadResponse)
    }

    /// Reboot the module by issuing `R,1\r`.
    ///
    /// The module replies with `%REBOOT%` (not `AOK`) after this command.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if UART communication fails.
    pub async fn reboot(&mut self) -> Result<(), Error<E>> {
        self.write_all(b"R,1\r").await?;
        self.wait_for_reboot().await
    }

    /// Perform a full factory reset (`SF,2\r`) and wait for `%REBOOT%`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if UART communication fails.
    pub async fn factory_reset(&mut self) -> Result<(), Error<E>> {
        self.write_all(b"SF,2\r").await?;
        self.wait_for_reboot().await
    }

    /// Provision the module for bare-GAP (no GATT) operation.
    ///
    /// Sequence:
    /// 1. Full factory reset (clears all NVM).
    /// 2. Enter command mode.
    /// 3. Apply configuration: device name, no services, NIIO auth, max TX
    ///    power, connection timing, advertising timing, No-Prompt.
    /// 4. Reboot to activate NVM settings.
    /// 5. Re-enter command mode (leaves module ready for further commands).
    ///
    /// # Errors
    ///
    /// - [`Error::Io`] if UART communication fails at any step.
    /// - [`Error::Command`] if the module replies `ERR` to any command.
    pub async fn provision(&mut self) -> Result<(), Error<E>> {
        self.factory_reset().await?;
        self.enter_command_mode().await?;
        self.command(b"SN,MeteoStation").await?;
        // Device Info (0x80) + Transparent UART (0x40). The module needs at
        // least one service to accept and hold a central's connection; with
        // SS,00 (no services) it advertises but the link never establishes
        // (central sees HCI 0x3e "Connection Failed to be Established" and the
        // module never emits %CONNECT). Confirmed on hardware 2026-06-15.
        self.command(b"SS,C0").await?;
        self.command(b"SA,2").await?;
        self.command(b"SGA,0").await?;
        self.command(b"SGC,0").await?;
        self.command(b"ST,0006,000C,0000,0258").await?;
        self.command(b"STA,0020,FFFF,0020").await?;
        self.command(b"SR,4000").await?;
        self.reboot().await?;
        self.enter_command_mode().await?;
        Ok(())
    }

    /// Start advertising with a 20 ms interval (value `0x0020`).
    ///
    /// Sends `A,0020\r` and awaits the `AOK` acknowledgement.
    ///
    /// # Errors
    ///
    /// - [`Error::Io`] if UART communication fails.
    /// - [`Error::Command`] if the module replies `ERR`.
    pub async fn start_advertising(&mut self) -> Result<(), Error<E>> {
        self.command(b"A,0020").await
    }

    /// Restart advertising: fire-and-forget `A,0020\r`.
    ///
    /// Does **not** read a response. This avoids a deadlock where a central
    /// reconnects before `AOK` is received; any stray `AOK` is silently
    /// consumed by the next [`next_event`](Self::next_event) call.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the UART write fails.
    pub async fn restart_advertising(&mut self) -> Result<(), Error<E>> {
        self.write_all(b"A,0020\r").await
    }

    /// Return the next [`Event`], blocking until one arrives.
    ///
    /// Drains the internal buffer first; if empty, reads lines from UART until
    /// a `%…%` event frame is received. `Prompt`, `Data`, and `Aok`/`Err`
    /// lines are silently skipped.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if UART communication fails.
    pub async fn next_event(&mut self) -> Result<Event, Error<E>> {
        if let Some(e) = self.events.pop_front() {
            return Ok(e);
        }
        let mut buf: Vec<u8, 64> = Vec::new();
        loop {
            self.read_line(&mut buf).await?;
            if let Line::Event(e) = classify(&buf) {
                return Ok(e);
            }
        }
    }

    /// Non-blocking drain of the internal event buffer.
    ///
    /// Returns `Some(event)` if one is buffered, `None` otherwise.
    pub fn take_buffered_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }
}

// ---------------------------------------------------------------------------
// Version parser (pure function — easy to test)
// ---------------------------------------------------------------------------

/// Parse `V<major>.<minor>` out of a module version string.
///
/// Scans left-to-right for a `V` byte followed by decimal digits, a `.`, and
/// more decimal digits. Returns `None` if the pattern is not found.
fn parse_version(s: &[u8]) -> Option<(u8, u8)> {
    let v_pos = s.iter().position(|&b| b == b'V')?;
    let rest = &s[v_pos.saturating_add(1)..];
    let dot = rest.iter().position(|&b| b == b'.')?;
    let major_bytes = &rest[..dot];
    let minor_start = dot.saturating_add(1);
    let minor_len = rest[minor_start..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    let minor_bytes = &rest[minor_start..minor_start.saturating_add(minor_len)];
    let major = parse_u8_decimal(major_bytes)?;
    let minor = parse_u8_decimal(minor_bytes)?;
    Some((major, minor))
}

/// Parse a decimal ASCII byte slice as a `u8`. Returns `None` on empty or
/// overflow.
fn parse_u8_decimal(digits: &[u8]) -> Option<u8> {
    if digits.is_empty() {
        return None;
    }
    let mut acc: u16 = 0;
    for &d in digits {
        if !d.is_ascii_digit() {
            return None;
        }
        acc = acc
            .checked_mul(10)?
            .checked_add(u16::from(d.wrapping_sub(b'0')))?;
        if acc > u16::from(u8::MAX) {
            return None;
        }
    }
    u8::try_from(acc).ok()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[expect(
    clippy::unnecessary_wraps,
    reason = "test module: TestResult is the project-standard return type even when ? is not used"
)]
#[cfg(test)]
mod tests {
    use core::{convert::Infallible, future::pending, result};

    use embedded_hal::digital::{ErrorType, OutputPin};
    use embedded_io_async::ErrorType as IoErrorType;
    use test_log::test;

    use super::*;

    /// Convenience alias: all driver calls use `Infallible` as the I/O error
    /// type in tests, so this covers both pure and async test functions.
    type TestResult = result::Result<(), Error<Infallible>>;

    // -----------------------------------------------------------------------
    // Test doubles
    // -----------------------------------------------------------------------

    /// Fake UART: fixed rx byte queue + captured tx bytes.
    struct FakeUart {
        rx: heapless::Deque<u8, 512>,
        pub tx: heapless::Vec<u8, 512>,
    }

    impl FakeUart {
        fn new(rx_bytes: &[u8]) -> Self {
            let mut rx = heapless::Deque::new();
            for &b in rx_bytes {
                rx.push_back(b).ok();
            }
            Self {
                rx,
                tx: heapless::Vec::new(),
            }
        }
    }

    impl IoErrorType for FakeUart {
        type Error = Infallible;
    }

    #[allow(
        clippy::unreachable,
        reason = "pending() never resolves; the unreachable!() after it is dead code"
    )]
    impl AsyncRead for FakeUart {
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
            if buf.is_empty() {
                return Ok(0);
            }
            // Serve bytes one at a time for exact framing tests.
            if let Some(b) = self.rx.pop_front() {
                buf[0] = b;
                Ok(1)
            } else {
                // Block forever if no bytes — tests must provide enough rx data.
                pending::<()>().await;
                unreachable!()
            }
        }
    }

    impl AsyncWrite for FakeUart {
        async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
            for &b in buf {
                self.tx.push(b).ok();
            }
            Ok(buf.len())
        }

        async fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    /// Fake GPIO pin (`OutputPin` with `Infallible` error).
    struct FakePin;

    impl ErrorType for FakePin {
        type Error = Infallible;
    }

    impl OutputPin for FakePin {
        fn set_low(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }

        fn set_high(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    /// Fake delay: all delay calls resolve instantly (no actual sleeping).
    struct FakeDelay;

    impl DelayNs for FakeDelay {
        async fn delay_ns(&mut self, _ns: u32) {
            // Instant no-op.
        }
    }

    /// Build a driver with the given rx byte stream.
    fn make_driver(rx: &[u8]) -> Rn4871<FakeUart, FakePin, FakeDelay> {
        Rn4871::new(FakeUart::new(rx), FakePin, FakeDelay)
    }

    // -----------------------------------------------------------------------
    // classify tests (pure)
    // -----------------------------------------------------------------------

    #[test]
    fn classify_recognises_aok_err_and_lowercase_err() -> TestResult {
        // Given / When / Then
        assert_eq!(classify(b"AOK"), Line::Aok, "AOK should classify as Aok");
        assert_eq!(
            classify(b"aok"),
            Line::Aok,
            "lowercase aok should classify as Aok"
        );
        assert_eq!(classify(b"ERR"), Line::Err, "ERR should classify as Err");
        assert_eq!(
            classify(b"Err"),
            Line::Err,
            "mixed-case Err should classify as Err"
        );
        assert_eq!(
            classify(b"err"),
            Line::Err,
            "lowercase err should classify as Err"
        );
        Ok(())
    }

    #[test]
    fn classify_recognises_events() -> TestResult {
        assert_eq!(
            classify(b"%REBOOT%"),
            Line::Event(Event::Reboot),
            "%REBOOT% should classify as Event::Reboot"
        );
        assert_eq!(
            classify(b"%CONNECT,0,AABBCCDDEE%"),
            Line::Event(Event::Connect),
            "%CONNECT,...% should classify as Event::Connect"
        );
        assert_eq!(
            classify(b"%DISCONNECT%"),
            Line::Event(Event::Disconnect),
            "%DISCONNECT% should classify as Event::Disconnect"
        );
        assert_eq!(
            classify(b"%STREAM_OPEN%"),
            Line::Event(Event::StreamOpen),
            "%STREAM_OPEN% should classify as Event::StreamOpen"
        );
        assert_eq!(
            classify(b"%UNKNOWN%"),
            Line::Event(Event::Other),
            "unknown event should classify as Event::Other"
        );
        Ok(())
    }

    #[test]
    fn classify_recognises_prompt() -> TestResult {
        assert_eq!(
            classify(b"CMD> "),
            Line::Prompt,
            "CMD>  should classify as Prompt"
        );
        Ok(())
    }

    // -----------------------------------------------------------------------
    // parse_version tests (pure)
    // -----------------------------------------------------------------------

    #[test]
    fn firmware_version_parses_major_minor() -> TestResult {
        // Given
        let version_str = b"RN4871 V1.30 10/10/2017 (c)Microchip Technology Inc";

        // When
        let result = parse_version(version_str);

        // Then
        assert_eq!(
            result,
            Some((1_u8, 30_u8)),
            "Version should parse to (1, 30)"
        );
        Ok(())
    }

    #[test]
    fn parse_version_returns_none_for_garbage() -> TestResult {
        assert!(
            parse_version(b"no version here").is_none(),
            "Should return None for invalid input"
        );
        Ok(())
    }

    // -----------------------------------------------------------------------
    // read_line tests
    // -----------------------------------------------------------------------

    #[test(tokio::test)]
    async fn read_line_returns_event_without_trailing_newline() -> TestResult {
        // Given: %DISCONNECT% with no \n
        let mut driver = make_driver(b"%DISCONNECT%");
        let mut buf: Vec<u8, 64> = Vec::new();

        // When
        driver.read_line(&mut buf).await?;

        // Then
        assert_eq!(
            buf.as_slice(),
            b"%DISCONNECT%",
            "read_line should return the event frame"
        );
        Ok(())
    }

    #[test(tokio::test)]
    async fn read_line_returns_prompt_without_newline() -> TestResult {
        // Given: CMD> with no \n
        let mut driver = make_driver(b"CMD> ");
        let mut buf: Vec<u8, 64> = Vec::new();

        // When
        driver.read_line(&mut buf).await?;

        // Then
        assert_eq!(
            buf.as_slice(),
            b"CMD> ",
            "read_line should return the prompt"
        );
        Ok(())
    }

    #[test(tokio::test)]
    async fn read_line_strips_crlf() -> TestResult {
        // Given: AOK\r\n
        let mut driver = make_driver(b"AOK\r\n");
        let mut buf: Vec<u8, 64> = Vec::new();

        // When
        driver.read_line(&mut buf).await?;

        // Then
        assert_eq!(
            buf.as_slice(),
            b"AOK",
            "read_line should strip trailing \\r\\n"
        );
        Ok(())
    }

    // -----------------------------------------------------------------------
    // command tests
    // -----------------------------------------------------------------------

    #[test(tokio::test)]
    async fn command_returns_ok_on_aok() -> TestResult {
        // Given
        let mut driver = make_driver(b"AOK\r\n");

        // When
        let result = driver.command(b"SN,Test").await;

        // Then
        assert!(result.is_ok(), "command should return Ok on AOK");
        assert!(
            driver.uart.tx.ends_with(b"SN,Test\r"),
            "command should send cmd\\r"
        );
        Ok(())
    }

    #[test(tokio::test)]
    async fn command_returns_err_on_err() -> TestResult {
        // Given
        let mut driver = make_driver(b"ERR\r\n");

        // When
        let result = driver.command(b"SN,Test").await;

        // Then
        assert_eq!(
            result,
            Err(Error::Command),
            "command should return Err(Error::Command) on ERR"
        );
        Ok(())
    }

    #[test(tokio::test)]
    async fn command_routes_events_while_awaiting_aok() -> TestResult {
        // Given: CONNECT event followed by AOK
        let mut driver = make_driver(b"%CONNECT,0,AABBCCDDEE%AOK\r\n");

        // When
        let result = driver.command(b"A,0020").await;

        // Then
        assert!(
            result.is_ok(),
            "command should return Ok after buffering event"
        );
        let event = driver.next_event().await?;
        assert_eq!(event, Event::Connect, "buffered event should be Connect");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // next_event / event routing
    // -----------------------------------------------------------------------

    #[test(tokio::test)]
    async fn next_event_parses_disconnect() -> TestResult {
        // Given
        let mut driver = make_driver(b"%DISCONNECT%");

        // When
        let event = driver.next_event().await?;

        // Then
        assert_eq!(
            event,
            Event::Disconnect,
            "next_event should return Disconnect"
        );
        Ok(())
    }

    // -----------------------------------------------------------------------
    // reset
    // -----------------------------------------------------------------------

    #[test(tokio::test)]
    async fn reset_completes_on_reboot() -> TestResult {
        // Given: module sends %REBOOT% after reset
        let mut driver = make_driver(b"%REBOOT%");

        // When
        let result = driver.reset().await;

        // Then
        assert!(result.is_ok(), "reset should complete on %REBOOT%");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // start_advertising / restart_advertising
    // -----------------------------------------------------------------------

    #[test(tokio::test)]
    async fn start_advertising_sends_a_with_interval() -> TestResult {
        // Given
        let mut driver = make_driver(b"AOK\r\n");

        // When
        let result = driver.start_advertising().await;

        // Then
        assert!(result.is_ok(), "start_advertising should return Ok");
        assert!(
            driver.uart.tx.ends_with(b"A,0020\r"),
            "start_advertising should send A,0020\\r"
        );
        Ok(())
    }

    #[test(tokio::test)]
    async fn restart_advertising_is_fire_and_forget() -> TestResult {
        // Given: empty rx queue (no response expected)
        let mut driver = make_driver(b"");

        // When
        let result = driver.restart_advertising().await;

        // Then
        assert!(
            result.is_ok(),
            "restart_advertising should return Ok without reading"
        );
        assert_eq!(
            driver.uart.tx.as_slice(),
            b"A,0020\r",
            "restart_advertising should write A,0020\\r"
        );
        Ok(())
    }

    // -----------------------------------------------------------------------
    // provision tests
    // -----------------------------------------------------------------------

    /// Build the exact rx byte stream that `provision` will consume.
    ///
    /// Sequence consumed by provision:
    /// 1. `factory_reset` → writes `SF,2\r`, reads `%REBOOT%`
    /// 2. `enter_command_mode` → writes `$$$`, then `V\r`, reads version line
    /// 3. `SN,MeteoStation\r` → `AOK\r\n`
    /// 4. `SS,C0\r` → `AOK\r\n`
    /// 5. `SA,2\r` → `AOK\r\n`
    /// 6. `SGA,0\r` → `AOK\r\n`
    /// 7. `SGC,0\r` → `AOK\r\n`
    /// 8. `ST,...\r` → `AOK\r\n`
    /// 9. `STA,...\r` → `AOK\r\n`
    /// 10. `SR,4000\r` → `AOK\r\n`
    /// 11. `reboot` → writes `R,1\r`, reads `%REBOOT%`
    /// 12. `enter_command_mode` → writes `$$$`, `V\r`, reads version line
    const PROVISION_RX: &[u8] =
        b"%REBOOT%RN4871 V1.30\r\nAOK\r\nAOK\r\nAOK\r\nAOK\r\nAOK\r\nAOK\r\nAOK\r\nAOK\r\n%REBOOT%RN4871 V1.30\r\n";

    #[test(tokio::test)]
    async fn provision_emits_expected_command_sequence() -> TestResult {
        // Given
        let mut driver = make_driver(PROVISION_RX);

        // When
        let result = driver.provision().await;

        // Then
        assert!(result.is_ok(), "provision should succeed");
        let tx = driver.uart.tx.as_slice();
        assert!(
            tx.windows(b"SN,MeteoStation\r".len())
                .any(|w| w == b"SN,MeteoStation\r"),
            "tx should contain SN,MeteoStation\\r"
        );
        assert!(
            tx.windows(b"SS,C0\r".len()).any(|w| w == b"SS,C0\r"),
            "tx should contain SS,C0\\r"
        );
        assert!(
            tx.windows(b"SA,2\r".len()).any(|w| w == b"SA,2\r"),
            "tx should contain SA,2\\r"
        );
        assert!(
            tx.windows(b"SGA,0\r".len()).any(|w| w == b"SGA,0\r"),
            "tx should contain SGA,0\\r"
        );
        assert!(
            tx.windows(b"SGC,0\r".len()).any(|w| w == b"SGC,0\r"),
            "tx should contain SGC,0\\r"
        );
        assert!(
            tx.windows(b"ST,0006,000C,0000,0258\r".len())
                .any(|w| w == b"ST,0006,000C,0000,0258\r"),
            "tx should contain ST,...\\r"
        );
        assert!(
            tx.windows(b"STA,0020,FFFF,0020\r".len())
                .any(|w| w == b"STA,0020,FFFF,0020\r"),
            "tx should contain STA,...\\r"
        );
        assert!(
            tx.windows(b"SR,4000\r".len()).any(|w| w == b"SR,4000\r"),
            "tx should contain SR,4000\\r"
        );
        assert!(
            tx.windows(b"SF,2\r".len()).any(|w| w == b"SF,2\r"),
            "tx should contain SF,2\\r"
        );
        assert!(
            tx.windows(b"R,1\r".len()).any(|w| w == b"R,1\r"),
            "tx should contain R,1\\r"
        );
        // Must NOT contain GATT commands.
        assert!(
            !tx.windows(4).any(|w| w == b"PS,"),
            "tx must not contain PS,"
        );
        assert!(
            !tx.windows(4).any(|w| w == b"PC,"),
            "tx must not contain PC,"
        );
        assert!(
            !tx.windows(3).any(|w| w == b"SHW"),
            "tx must not contain SHW"
        );
        Ok(())
    }

    #[test(tokio::test)]
    async fn provision_propagates_command_error() -> TestResult {
        // Given: SA,2 reply replaced with Err\r\n (3rd AOK → Err)
        // Sequence: %REBOOT%, version line, AOK (SN), AOK (SS), Err (SA,2)
        let rx = b"%REBOOT%RN4871 V1.30\r\nAOK\r\nAOK\r\nErr\r\n";
        let mut driver = make_driver(rx);

        // When
        let result = driver.provision().await;

        // Then
        assert_eq!(
            result,
            Err(Error::Command),
            "provision should propagate Error::Command when SA,2 fails"
        );
        let tx = driver.uart.tx.as_slice();
        assert!(
            !tx.windows(b"STA,".len()).any(|w| w == b"STA,"),
            "provision should not have reached STA, after early error"
        );
        Ok(())
    }
}
// grcov exclude stop
