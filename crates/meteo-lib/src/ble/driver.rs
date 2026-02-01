//! RN4871 BLE module async driver.
//!
//! Provides a high-level interface for communicating with the RN4871 over UART.
//! The driver is generic over a [`Uart`] trait to remain hardware-agnostic and
//! testable on the host.
//!
//! Hardware reset (`RST_N` pin) is managed externally by the firmware task.
//! After toggling reset, call [`Rn4871::wait_for_reboot`] to synchronize.

use core::error;
use core::fmt;
use core::future::Future;

use super::line_buffer::LineBuffer;
use super::rn4871::{self, Response};

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
}

impl<E: fmt::Debug> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Uart(e) => write!(f, "UART error: {e:?}"),
            Self::CommandFailed => write!(f, "command failed (ERR)"),
            Self::UnexpectedResponse => write!(f, "unexpected response"),
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
    Reboot,
    Connect,
    Disconnect,
    StreamOpen,
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
            Response::Reboot => Self::Reboot,
            Response::Connect { .. } => Self::Connect,
            Response::Disconnect => Self::Disconnect,
            Response::StreamOpen => Self::StreamOpen,
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

    /// Waits for the module to send `%REBOOT%` after a hardware reset.
    ///
    /// # Errors
    ///
    /// Returns `Error::Uart` if a UART read fails, or `Error::UnexpectedResponse`
    /// if a non-`Data` response other than `Reboot` is received.
    pub async fn wait_for_reboot(&mut self) -> Result<(), Error<U::Error>> {
        self.wait_for(ResponseKind::Reboot).await
    }

    /// Enters command mode by sending `$$$` and waiting for the `CMD>` prompt.
    ///
    /// # Errors
    ///
    /// Returns `Error::Uart` if UART I/O fails, or `Error::UnexpectedResponse`
    /// if the module doesn't respond with `CMD>`.
    pub async fn enter_command_mode(&mut self) -> Result<(), Error<U::Error>> {
        // RN4871 requires no line ending after $$$
        self.uart.write(b"$$$").await.map_err(Error::Uart)?;
        self.wait_for(ResponseKind::Cmd).await
    }

    /// Exits command mode by sending `---\r` and waiting for `END`.
    ///
    /// # Errors
    ///
    /// Returns `Error::Uart` if UART I/O fails, or `Error::UnexpectedResponse`
    /// if the module doesn't respond with `END`.
    pub async fn exit_command_mode(&mut self) -> Result<(), Error<U::Error>> {
        self.uart.write(b"---\r").await.map_err(Error::Uart)?;
        self.wait_for(ResponseKind::End).await
    }

    /// Sends a command and waits for `AOK`.
    ///
    /// The command should NOT include the trailing `\r` — it is appended
    /// automatically.
    ///
    /// # Errors
    ///
    /// Returns `Error::CommandFailed` if the module responds with `ERR`,
    /// `Error::Uart` on I/O failure, or `Error::UnexpectedResponse` for
    /// other unexpected responses.
    pub async fn send_command(&mut self, cmd: &[u8]) -> Result<(), Error<U::Error>> {
        self.uart.write(cmd).await.map_err(Error::Uart)?;
        self.uart.write(b"\r").await.map_err(Error::Uart)?;
        self.wait_for(ResponseKind::Aok).await
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
                let response = rn4871::parse(line);
                kind = Some(ResponseKind::from_response(&response));
            });

            if let Some(k) = kind {
                return Ok(k);
            }

            // No complete line yet — read more bytes from UART
            let mut rx_buf = [0_u8; RX_BUF_SIZE];
            let n = self.uart.read(&mut rx_buf).await.map_err(Error::Uart)?;
            self.line_buf.push_bytes(&rx_buf[..n]);
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
    async fn send_command_appends_cr() -> TestResult {
        // Given
        let mock = MockUart::new(&[b"AOK\r\n"]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.send_command(b"SN,MeteoStation").await;

        // Then
        assert!(result.is_ok(), "should succeed on AOK");
        assert_eq!(
            driver.uart.written_bytes(),
            b"SN,MeteoStation\r",
            "should append CR to command"
        );
        Ok(())
    }

    #[tokio::test]
    async fn send_command_returns_error_on_err_response() -> TestResult {
        // Given
        let mock = MockUart::new(&[b"ERR\r\n"]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.send_command(b"INVALID").await;

        // Then
        assert_eq!(
            result,
            Result::Err(Error::CommandFailed),
            "should return CommandFailed on ERR"
        );
        Ok(())
    }

    #[tokio::test]
    async fn send_command_skips_echo_before_aok() -> TestResult {
        // Given: module echoes the command before responding
        let mock = MockUart::new(&[b"SN,MeteoStation\r\nAOK\r\n"]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.send_command(b"SN,MeteoStation").await;

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
    async fn unexpected_response_returns_error() -> TestResult {
        // Given: expecting Reboot but get Cmd
        let mock = MockUart::new(&[b"CMD> \r\n"]);
        let mut driver = Rn4871::new(mock);

        // When
        let result = driver.wait_for_reboot().await;

        // Then
        assert_eq!(
            result,
            Result::Err(Error::UnexpectedResponse),
            "should return UnexpectedResponse"
        );
        Ok(())
    }

    #[tokio::test]
    async fn multiple_commands_in_sequence() -> TestResult {
        // Given: enter command mode, send a command, exit
        let mock = MockUart::new(&[b"CMD> \r\n", b"AOK\r\n", b"END\r\n"]);
        let mut driver = Rn4871::new(mock);

        // When
        driver.enter_command_mode().await?;
        driver.send_command(b"SN,Test").await?;
        driver.exit_command_mode().await?;

        // Then
        assert_eq!(
            driver.uart.written_bytes(),
            b"$$$SN,Test\r---\r",
            "should send all commands in sequence"
        );
        Ok(())
    }
}
// grcov exclude stop
