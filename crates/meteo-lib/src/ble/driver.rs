//! RN4871 BLE module async driver.
//!
//! Provides a high-level interface for communicating with the RN4871 over UART.
//! The driver is generic over a [`Uart`] trait to remain hardware-agnostic and
//! testable on the host.
//!
//! Hardware reset (`RST_N` pin) is managed externally by the firmware task.
//! After toggling reset, call [`Rn4871::wait_for_reboot`] to synchronize.
#![allow(
    clippy::missing_asserts_for_indexing,
    reason = "false positive from defmt::Format derive (conditional on defmt feature)"
)]

use core::error;
use core::fmt;
use core::future::Future;

use super::line_buffer::LineBuffer;
use super::rn4871::command::{Command, ResponseType};
use super::rn4871::{parser, response::Response};

/// Maximum size for the command wire-format buffer.
const CMD_BUF_SIZE: usize = 64;

/// UART buffer sizes.
const LINE_BUF_SIZE: usize = 256;
const RX_BUF_SIZE: usize = 64;

/// Errors returned by the RN4871 driver.
#[derive(Debug, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum Error<E> {
    /// The underlying UART returned an error.
    Uart(E),
    /// The module responded with `ERR` to a command.
    CommandFailed,
    /// Expected a specific response but got something else.
    UnexpectedResponse,
    /// The command's wire format exceeds the internal buffer size.
    CommandTooLong,
}

impl<E: fmt::Debug> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Uart(e) => write!(f, "UART error: {e:?}"),
            Self::CommandFailed => write!(f, "command failed (ERR)"),
            Self::UnexpectedResponse => write!(f, "unexpected response"),
            Self::CommandTooLong => write!(f, "command too long for buffer"),
        }
    }
}

impl<E: fmt::Debug> error::Error for Error<E> {}

/// Async UART interface for the RN4871 driver.
///
/// This trait abstracts the UART read/write operations so the driver can be
/// tested on the host without real hardware. The firmware provides an adapter
/// that wraps Embassy's `UartRx::read_until_idle` and `UartTx::write`.
pub trait Uart {
    /// The error type for UART operations.
    type Error;

    /// Writes all bytes to the UART TX.
    fn write(&mut self, data: &[u8]) -> impl Future<Output = Result<(), Self::Error>>;

    /// Reads available bytes into `buf`, returning the number of bytes read.
    ///
    /// This should behave like `read_until_idle`: return once data is available
    /// (possibly less than `buf.len()`), rather than waiting for the buffer to
    /// be completely filled.
    fn read(&mut self, buf: &mut [u8]) -> impl Future<Output = Result<usize, Self::Error>>;
}

/// Simplified response kind for internal matching (no borrowed data).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResponseKind {
    Aok,
    Err,
    Cmd,
    End,
    Data,
}

impl ResponseKind {
    /// Classifies a [`Response`] into a [`ResponseKind`].
    const fn from_response(response: &Response<'_>) -> Self {
        match response {
            Response::Aok => Self::Aok,
            Response::Err => Self::Err,
            Response::Cmd => Self::Cmd,
            Response::End => Self::End,
            Response::Data(_) => Self::Data,
        }
    }
}

/// High-level async driver for the RN4871 BLE module.
///
/// Generic over a [`Uart`] implementation for hardware abstraction.
/// The driver manages line framing internally and provides methods for
/// command mode operations.
pub struct Rn4871<U> {
    uart: U,
    line_buf: LineBuffer<LINE_BUF_SIZE>,
}

impl<U: Uart> Rn4871<U> {
    /// Creates a new driver wrapping the given UART.
    ///
    /// The module should already be powered on. Call [`wait_for_reboot`](Self::wait_for_reboot)
    /// after hardware reset to synchronize.
    #[must_use]
    pub const fn new(uart: U) -> Self {
        Self {
            uart,
            line_buf: LineBuffer::new(),
        }
    }

    /// Returns a mutable reference to the underlying UART.
    ///
    /// Useful for direct UART access after configuration is complete
    /// (e.g. for a monitoring loop).
    pub const fn uart_mut(&mut self) -> &mut U {
        &mut self.uart
    }

    /// Waits for the module to send `%REBOOT%` after a hardware reset.
    ///
    /// Unlike other responses, `%REBOOT%` is not necessarily followed by
    /// `\r\n`, so this method scans the raw buffer content rather than
    /// relying on line framing.
    ///
    /// # Errors
    ///
    /// Returns `Error::Uart` if a UART read fails.
    pub async fn wait_for_reboot(&mut self) -> Result<(), Error<U::Error>> {
        const MARKER: &[u8] = b"%REBOOT%";
        loop {
            let data = self.line_buf.as_bytes();
            if data.windows(MARKER.len()).any(|window| window == MARKER) {
                self.line_buf.clear();
                return Ok(());
            }
            self.read_more().await?;
        }
    }

    /// Enters command mode by sending `$$$` and waiting for the `CMD>` prompt.
    ///
    /// The `CMD>` prompt is not followed by `\r\n`, so this method scans
    /// the raw buffer content rather than relying on line framing.
    ///
    /// # Errors
    ///
    /// Returns `Error::Uart` if UART I/O fails.
    pub async fn enter_command_mode(&mut self) -> Result<(), Error<U::Error>> {
        // RN4871 requires no line ending after $$$
        self.uart.write(b"$$$").await.map_err(Error::Uart)?;
        self.wait_for_marker(b"CMD>").await
    }

    /// Exits command mode by sending `---\r` and waiting for `END`.
    ///
    /// The `END` response may not be followed by `\r\n`, so this method
    /// scans the raw buffer content.
    ///
    /// # Errors
    ///
    /// Returns `Error::Uart` if UART I/O fails.
    pub async fn exit_command_mode(&mut self) -> Result<(), Error<U::Error>> {
        self.uart.write(b"---\r").await.map_err(Error::Uart)?;
        self.wait_for_marker(b"END").await
    }

    /// Sends a factory reset command (`SF,1`) and waits for the module to reboot.
    ///
    /// Must be called while in command mode. After this returns, the module
    /// has rebooted and is back in data mode (not command mode).
    ///
    /// # Errors
    ///
    /// Returns `Error::CommandTooLong` if the internal buffer is too small,
    /// or `Error::Uart` on I/O failure.
    #[cfg(feature = "factory-reset")]
    pub async fn factory_reset(&mut self) -> Result<(), Error<U::Error>> {
        let mut cmd_buf = [0_u8; CMD_BUF_SIZE];
        let n = Command::FactoryReset
            .write_to(&mut cmd_buf)
            .ok_or(Error::CommandTooLong)?;
        self.uart.write(&cmd_buf[..n]).await.map_err(Error::Uart)?;
        self.uart.write(b"\r").await.map_err(Error::Uart)?;
        self.line_buf.clear();
        self.wait_for_marker(b"Reboot").await
    }

    /// Executes a command that expects `AOK` (e.g. `SetName`, `SetFeatures`).
    ///
    /// # Errors
    ///
    /// Returns `Error::CommandTooLong` if the command exceeds the internal
    /// buffer, `Error::CommandFailed` on `ERR`, `Error::Uart` on I/O failure,
    /// or `Error::UnexpectedResponse` for unexpected responses.
    pub async fn execute(&mut self, command: Command<'_>) -> Result<(), Error<U::Error>> {
        debug_assert_eq!(
            command.response_type(),
            ResponseType::Aok,
            "execute() requires a command that expects AOK"
        );
        let mut cmd_buf = [0_u8; CMD_BUF_SIZE];
        let n = command
            .write_to(&mut cmd_buf)
            .ok_or(Error::CommandTooLong)?;
        self.send_command(&cmd_buf[..n]).await
    }

    /// Executes a query command and returns the first data line.
    ///
    /// Returns the number of bytes written to `response_buf`.
    ///
    /// # Errors
    ///
    /// Returns `Error::CommandTooLong` if the command exceeds the internal
    /// buffer, `Error::Uart` on I/O failure, `Error::CommandFailed` on `ERR`,
    /// or `Error::UnexpectedResponse` for unexpected responses.
    pub async fn query(
        &mut self,
        command: Command<'_>,
        response_buf: &mut [u8],
    ) -> Result<usize, Error<U::Error>> {
        debug_assert_eq!(
            command.response_type(),
            ResponseType::SingleLine,
            "query() requires a command that expects SingleLine"
        );
        let mut cmd_buf = [0_u8; CMD_BUF_SIZE];
        let n = command
            .write_to(&mut cmd_buf)
            .ok_or(Error::CommandTooLong)?;
        self.send_query(&cmd_buf[..n], response_buf).await
    }

    /// Executes a multi-line query command and calls `f` for each data line.
    ///
    /// # Errors
    ///
    /// Returns `Error::CommandTooLong` if the command exceeds the internal
    /// buffer, `Error::Uart` on I/O failure, `Error::CommandFailed` on `ERR`,
    /// or `Error::UnexpectedResponse` for unexpected responses.
    pub async fn query_multiline<F: FnMut(&[u8])>(
        &mut self,
        command: Command<'_>,
        f: F,
    ) -> Result<(), Error<U::Error>> {
        debug_assert_eq!(
            command.response_type(),
            ResponseType::MultiLine,
            "query_multiline() requires a command that expects MultiLine"
        );
        let mut cmd_buf = [0_u8; CMD_BUF_SIZE];
        let n = command
            .write_to(&mut cmd_buf)
            .ok_or(Error::CommandTooLong)?;
        self.send_multiline_query(&cmd_buf[..n], f).await
    }

    /// Sends a raw command and waits for `AOK`.
    ///
    /// The command should NOT include the trailing `\r` — it is appended
    /// automatically. Prefer [`execute`](Self::execute) with a typed
    /// [`Command`] instead.
    ///
    /// # Errors
    ///
    /// Returns `Error::CommandFailed` if the module responds with `ERR`,
    /// `Error::Uart` on I/O failure, or `Error::UnexpectedResponse` for
    /// other unexpected responses.
    async fn send_command(&mut self, cmd: &[u8]) -> Result<(), Error<U::Error>> {
        self.uart.write(cmd).await.map_err(Error::Uart)?;
        self.uart.write(b"\r").await.map_err(Error::Uart)?;
        self.wait_for(ResponseKind::Aok).await?;
        // The module re-sends the CMD> prompt after each response.
        // Consume it so it doesn't pollute the next command's buffer.
        self.wait_for_marker(b"CMD>").await
    }

    /// Sends a raw query command and returns the first data line.
    ///
    /// Prefer [`query`](Self::query) with a typed [`Command`] instead.
    async fn send_query(&mut self, cmd: &[u8], buf: &mut [u8]) -> Result<usize, Error<U::Error>> {
        self.uart.write(cmd).await.map_err(Error::Uart)?;
        self.uart.write(b"\r").await.map_err(Error::Uart)?;

        loop {
            let mut result: Option<Result<usize, Error<U::Error>>> = None;

            self.ensure_line_available().await?;
            self.line_buf.process_line(|line| {
                let response = parser::parse(line);
                match response {
                    Response::Data(data) => {
                        // Skip echo of the command itself
                        if data == cmd {
                            return;
                        }
                        let n = data.len().min(buf.len());
                        buf[..n].copy_from_slice(&data[..n]);
                        result = Some(Ok(n));
                    }
                    Response::Err => {
                        result = Some(Result::Err(Error::CommandFailed));
                    }
                    _ => {
                        result = Some(Result::Err(Error::UnexpectedResponse));
                    }
                }
            });

            if let Some(r) = result {
                // The module re-sends the CMD> prompt after each response.
                // Consume it so it doesn't pollute the next command's buffer.
                if r.is_ok() {
                    self.wait_for_marker(b"CMD>").await?;
                }
                return r;
            }
        }
    }

    /// Sends a raw multi-line query command and calls `f` for each data line.
    ///
    /// Prefer [`query_multiline`](Self::query_multiline) with a typed
    /// [`Command`] instead.
    async fn send_multiline_query<F: FnMut(&[u8])>(
        &mut self,
        cmd: &[u8],
        mut f: F,
    ) -> Result<(), Error<U::Error>> {
        self.uart.write(cmd).await.map_err(Error::Uart)?;
        self.uart.write(b"\r").await.map_err(Error::Uart)?;

        loop {
            // Drain all complete lines before checking for the CMD> marker.
            // This ensures we process data lines that arrived in the same
            // chunk as CMD>.
            loop {
                let mut had_line = false;
                let mut result: Option<Result<(), Error<U::Error>>> = None;
                self.line_buf.process_line(|line| {
                    had_line = true;
                    let response = parser::parse(line);
                    match response {
                        Response::Data(line_data) => {
                            if line_data != cmd {
                                f(line_data);
                            }
                        }
                        Response::Err => {
                            result = Some(Result::Err(Error::CommandFailed));
                        }
                        _ => {
                            result = Some(Result::Err(Error::UnexpectedResponse));
                        }
                    }
                });

                if let Some(r) = result {
                    return r;
                }
                if !had_line {
                    break;
                }
            }

            // No more complete lines — check for CMD> marker in remaining data
            let data = self.line_buf.as_bytes();
            if data.len() >= 4 && data.windows(4).any(|w| w == b"CMD>") {
                self.line_buf.clear();
                return Ok(());
            }

            self.read_more().await?;
        }
    }

    /// Reads lines until the expected response kind is received.
    ///
    /// `Data` responses are silently skipped (they represent intermediate
    /// multi-line output or echo). Any other unexpected response kind causes
    /// an `UnexpectedResponse` error. `Err` responses cause `CommandFailed`.
    async fn wait_for(&mut self, expected: ResponseKind) -> Result<(), Error<U::Error>> {
        loop {
            let kind = self.read_response_kind().await?;
            if kind == expected {
                return Ok(());
            }
            match kind {
                ResponseKind::Data => {}
                ResponseKind::Err => return Result::Err(Error::CommandFailed),
                _ => return Result::Err(Error::UnexpectedResponse),
            }
        }
    }

    /// Reads bytes from UART and returns the kind of the next parsed response.
    async fn read_response_kind(&mut self) -> Result<ResponseKind, Error<U::Error>> {
        loop {
            // Try to extract a single line from buffered data
            let mut kind = None;
            self.line_buf.process_line(|line| {
                let response = parser::parse(line);
                kind = Some(ResponseKind::from_response(&response));
            });

            if let Some(k) = kind {
                return Ok(k);
            }

            // No complete line yet — read more bytes from UART
            self.read_more().await?;
        }
    }

    /// Ensures at least one complete line is available in the line buffer.
    ///
    /// Reads from UART until the line buffer contains a complete line.
    async fn ensure_line_available(&mut self) -> Result<(), Error<U::Error>> {
        while !self.line_buf.has_complete_line() {
            self.read_more().await?;
        }
        Ok(())
    }

    /// Reads a chunk of bytes from the UART into the line buffer.
    async fn read_more(&mut self) -> Result<(), Error<U::Error>> {
        let mut rx_buf = [0_u8; RX_BUF_SIZE];
        let n = self.uart.read(&mut rx_buf).await.map_err(Error::Uart)?;
        self.line_buf.push_bytes(&rx_buf[..n]);
        Ok(())
    }

    /// Waits for a specific byte sequence to appear in the raw buffer.
    ///
    /// Unlike [`wait_for`](Self::wait_for), this does not rely on line framing.
    /// It scans the raw buffer content using a sliding window. This is needed
    /// for RN4871 prompts like `CMD>` and `END` which are not followed by
    /// `\r\n`.
    ///
    /// After the marker is found, all buffered data is discarded.
    async fn wait_for_marker(&mut self, marker: &[u8]) -> Result<(), Error<U::Error>> {
        loop {
            let data = self.line_buf.as_bytes();
            if data.len() >= marker.len() && data.windows(marker.len()).any(|w| w == marker) {
                self.line_buf.clear();
                return Ok(());
            }
            self.read_more().await?;
        }
    }
}

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(test)]
mod tests {
    extern crate std;

    use core::{error, result};

    use std::boxed::Box;
    use std::collections::VecDeque;
    use std::vec;
    use std::vec::Vec;

    use super::*;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    /// Mock UART that returns pre-loaded byte chunks on read and records writes.
    struct MockUart {
        /// Queued read responses: each entry is returned by one `read()` call.
        read_queue: VecDeque<Vec<u8>>,
        /// Collected write data.
        written: Vec<Vec<u8>>,
    }

    impl MockUart {
        fn new(reads: &[&[u8]]) -> Self {
            Self {
                read_queue: reads.iter().map(|r| r.to_vec()).collect(),
                written: vec![],
            }
        }

        /// Returns all written bytes concatenated.
        fn written_bytes(&self) -> Vec<u8> {
            self.written.iter().flatten().copied().collect()
        }
    }

    impl Uart for MockUart {
        type Error = core::convert::Infallible;

        async fn write(&mut self, data: &[u8]) -> Result<(), Self::Error> {
            self.written.push(data.to_vec());
            Ok(())
        }

        #[expect(
            clippy::panic,
            reason = "mock panics on underflow to fail tests clearly"
        )]
        async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
            let chunk = self
                .read_queue
                .pop_front()
                .expect("MockUart: no more data to read");
            let n = chunk.len().min(buf.len());
            buf[..n].copy_from_slice(&chunk[..n]);
            Ok(n)
        }
    }

    #[tokio::test]
    async fn wait_for_reboot_succeeds() -> TestResult {
        // Given
        let mock = MockUart::new(&[b"%REBOOT%\r\n"]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.wait_for_reboot().await;

        // Then
        assert!(result.is_ok(), "should succeed on %REBOOT%");
        Ok(())
    }

    #[tokio::test]
    async fn wait_for_reboot_skips_data_lines() -> TestResult {
        // Given: some garbage before the reboot message
        let mock = MockUart::new(&[b"RN4871 V1.40\r\n%REBOOT%\r\n"]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.wait_for_reboot().await;

        // Then
        assert!(result.is_ok(), "should skip Data and succeed on %REBOOT%");
        Ok(())
    }

    #[tokio::test]
    async fn enter_command_mode_sends_dollar_signs() -> TestResult {
        // Given
        let mock = MockUart::new(&[b"CMD> \r\n"]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.enter_command_mode().await;

        // Then
        assert!(result.is_ok(), "should succeed");
        assert_eq!(
            driver.uart.written_bytes(),
            b"$$$",
            "should send $$$ with no line ending"
        );
        Ok(())
    }

    #[tokio::test]
    async fn exit_command_mode_sends_triple_dash() -> TestResult {
        // Given
        let mock = MockUart::new(&[b"END\r\n"]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.exit_command_mode().await;

        // Then
        assert!(result.is_ok(), "should succeed");
        assert_eq!(driver.uart.written_bytes(), b"---\r", "should send ---\\r");
        Ok(())
    }

    #[tokio::test]
    async fn execute_set_name_sends_correct_bytes() -> TestResult {
        // Given: AOK followed by CMD> prompt (real RN4871 behavior)
        let mock = MockUart::new(&[b"AOK\r\nCMD> "]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.execute(Command::SetName("MeteoStation")).await;

        // Then
        assert!(result.is_ok(), "should succeed on AOK");
        assert_eq!(
            driver.uart.written_bytes(),
            b"SN,MeteoStation\r",
            "should send SN,MeteoStation with CR"
        );
        Ok(())
    }

    #[tokio::test]
    async fn execute_returns_error_on_err_response() -> TestResult {
        // Given
        let mock = MockUart::new(&[b"ERR\r\n"]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.execute(Command::SetFeatures(0x2000)).await;

        // Then
        assert_eq!(
            result,
            Result::Err(Error::CommandFailed),
            "should return CommandFailed on ERR"
        );
        Ok(())
    }

    #[tokio::test]
    async fn execute_skips_echo_before_aok() -> TestResult {
        // Given: module echoes the command before responding, then re-sends CMD>
        let mock = MockUart::new(&[b"SN,MeteoStation\r\nAOK\r\nCMD> "]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.execute(Command::SetName("MeteoStation")).await;

        // Then
        assert!(result.is_ok(), "should skip echo (Data) and succeed on AOK");
        Ok(())
    }

    #[tokio::test]
    async fn incremental_reads_across_chunks() -> TestResult {
        // Given: response arrives in two separate read chunks
        let mock = MockUart::new(&[b"%REBOOT", b"%\r\n"]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.wait_for_reboot().await;

        // Then
        assert!(result.is_ok(), "should accumulate across read boundaries");
        Ok(())
    }

    #[tokio::test]
    async fn wait_for_reboot_ignores_non_reboot_data() -> TestResult {
        // Given: noise before the reboot marker, without line endings
        let mock = MockUart::new(&[b"CMD> ", b"garbage", b"%REBOOT%"]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.wait_for_reboot().await;

        // Then
        assert!(
            result.is_ok(),
            "should succeed after ignoring non-REBOOT data"
        );
        Ok(())
    }

    #[tokio::test]
    async fn multiple_commands_in_sequence() -> TestResult {
        // Given: enter command mode, send a command, exit
        // CMD> and END have no \r\n (real RN4871 behavior)
        // After AOK, module re-sends CMD> prompt
        let mock = MockUart::new(&[b"CMD> ", b"AOK\r\nCMD> ", b"END"]);
        let mut driver = Rn4871::new(mock);

        // When
        driver.enter_command_mode().await?;
        driver.execute(Command::SetName("Test")).await?;
        driver.exit_command_mode().await?;

        // Then
        assert_eq!(
            driver.uart.written_bytes(),
            b"$$$SN,Test\r---\r",
            "should send all commands in sequence"
        );
        Ok(())
    }

    #[tokio::test]
    async fn query_firmware_version_returns_data_line() -> TestResult {
        // Given: V command returns firmware version, then CMD> prompt
        let mock = MockUart::new(&[b"RN4871 V1.40 7/9/2019\r\nCMD> "]);
        let mut driver = Rn4871::new(mock);
        let mut buf = [0_u8; 64];

        // When
        let n = driver.query(Command::GetFirmwareVersion, &mut buf).await?;

        // Then
        assert_eq!(
            &buf[..n],
            b"RN4871 V1.40 7/9/2019",
            "should return firmware version"
        );
        Ok(())
    }

    #[tokio::test]
    async fn query_device_name_skips_echo() -> TestResult {
        // Given: module echoes the command before the response, then CMD> prompt
        let mock = MockUart::new(&[b"GN\r\nRN4871-1234\r\nCMD> "]);
        let mut driver = Rn4871::new(mock);
        let mut buf = [0_u8; 64];

        // When
        let n = driver.query(Command::GetDeviceName, &mut buf).await?;

        // Then
        assert_eq!(
            &buf[..n],
            b"RN4871-1234",
            "should skip echo and return device name"
        );
        Ok(())
    }

    #[tokio::test]
    async fn enter_command_mode_succeeds_without_line_ending() -> TestResult {
        // Given: CMD> prompt without \r\n (real RN4871 behavior)
        let mock = MockUart::new(&[b"CMD>"]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.enter_command_mode().await;

        // Then
        assert!(result.is_ok(), "should succeed on CMD> without line ending");
        Ok(())
    }

    #[tokio::test]
    async fn enter_command_mode_with_trailing_space() -> TestResult {
        // Given: CMD> with trailing space (common RN4871 variant)
        let mock = MockUart::new(&[b"CMD> "]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.enter_command_mode().await;

        // Then
        assert!(result.is_ok(), "should succeed on CMD> with trailing space");
        Ok(())
    }

    #[tokio::test]
    async fn exit_command_mode_succeeds_without_line_ending() -> TestResult {
        // Given: END without \r\n (real RN4871 behavior)
        let mock = MockUart::new(&[b"END"]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.exit_command_mode().await;

        // Then
        assert!(result.is_ok(), "should succeed on END without line ending");
        assert_eq!(driver.uart.written_bytes(), b"---\r", "should send ---\\r");
        Ok(())
    }

    #[tokio::test]
    async fn enter_command_mode_across_chunks() -> TestResult {
        // Given: CMD> arrives split across two reads
        let mock = MockUart::new(&[b"CM", b"D>"]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.enter_command_mode().await;

        // Then
        assert!(result.is_ok(), "should accumulate CMD> across read chunks");
        Ok(())
    }

    #[tokio::test]
    async fn exit_command_mode_with_noise_before_end() -> TestResult {
        // Given: some trailing prompt noise before END
        let mock = MockUart::new(&[b"CMD> END"]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.exit_command_mode().await;

        // Then
        assert!(
            result.is_ok(),
            "should find END marker even with preceding data"
        );
        Ok(())
    }

    #[tokio::test]
    async fn sequential_queries_do_not_pollute_each_other() -> TestResult {
        // Given: two queries in sequence, each followed by CMD> prompt
        let mock = MockUart::new(&[b"RN4871 V1.40\r\nCMD> ", b"WeatherStation\r\nCMD> "]);
        let mut driver = Rn4871::new(mock);
        let mut buf = [0_u8; 64];

        // When
        let n1 = driver.query(Command::GetFirmwareVersion, &mut buf).await?;
        let v: Vec<u8> = buf[..n1].to_vec();

        let n2 = driver.query(Command::GetDeviceName, &mut buf).await?;

        // Then
        assert_eq!(v, b"RN4871 V1.40", "first query should return version");
        assert_eq!(
            &buf[..n2],
            b"WeatherStation",
            "second query should return clean name without CMD> prefix"
        );
        Ok(())
    }

    #[tokio::test]
    async fn query_returns_error_on_err() -> TestResult {
        // Given
        let mock = MockUart::new(&[b"ERR\r\n"]);
        let mut driver = Rn4871::new(mock);
        let mut buf = [0_u8; 64];

        // When
        let result = driver.query(Command::GetFirmwareVersion, &mut buf).await;

        // Then
        assert_eq!(
            result,
            Result::Err(Error::CommandFailed),
            "should return CommandFailed on ERR"
        );
        Ok(())
    }

    #[tokio::test]
    async fn query_multiline_collects_all_lines() -> TestResult {
        // Given: D command returns multiple config lines then CMD>
        let mock =
            MockUart::new(&[b"BTA=AABBCCDDEEFF\r\nName=MeteoStation\r\nConnected=no\r\nCMD> "]);
        let mut driver = Rn4871::new(mock);
        let mut lines: Vec<Vec<u8>> = vec![];

        // When
        driver
            .query_multiline(Command::DumpConfig, |line| lines.push(line.to_vec()))
            .await?;

        // Then
        assert_eq!(lines.len(), 3, "should collect all data lines");
        assert_eq!(lines[0], b"BTA=AABBCCDDEEFF");
        assert_eq!(lines[1], b"Name=MeteoStation");
        assert_eq!(lines[2], b"Connected=no");
        Ok(())
    }

    #[tokio::test]
    async fn query_multiline_skips_echo() -> TestResult {
        // Given: module echoes the command before response
        let mock = MockUart::new(&[b"D\r\nBTA=AABBCCDDEEFF\r\nCMD> "]);
        let mut driver = Rn4871::new(mock);
        let mut lines: Vec<Vec<u8>> = vec![];

        // When
        driver
            .query_multiline(Command::DumpConfig, |line| lines.push(line.to_vec()))
            .await?;

        // Then
        assert_eq!(lines.len(), 1, "should skip echo");
        assert_eq!(lines[0], b"BTA=AABBCCDDEEFF");
        Ok(())
    }

    #[tokio::test]
    async fn query_multiline_across_chunks() -> TestResult {
        // Given: response split across multiple reads
        let mock = MockUart::new(&[b"BTA=AABB\r\n", b"Name=Test\r\n", b"CMD> "]);
        let mut driver = Rn4871::new(mock);
        let mut lines: Vec<Vec<u8>> = vec![];

        // When
        driver
            .query_multiline(Command::DumpConfig, |line| lines.push(line.to_vec()))
            .await?;

        // Then
        assert_eq!(lines.len(), 2, "should collect lines across chunks");
        assert_eq!(lines[0], b"BTA=AABB");
        assert_eq!(lines[1], b"Name=Test");
        Ok(())
    }
}
// grcov exclude stop
