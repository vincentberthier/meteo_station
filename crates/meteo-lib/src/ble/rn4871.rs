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
    /// The command-mode prompt (`CMD> `), present when the module runs *with*
    /// prompts — i.e. before provisioning switches it to No-Prompt (`SR,4000`).
    /// Callers skip it.
    Prompt,
    Data,
}

/// Classify a raw line.  Status events (`%...%`) are delimiter-framed, not
/// newline-terminated, and the prompt (`CMD> `) carries no newline either; the
/// reader ([`Rn4871::read_line`]) hands both here as complete tokens.
fn classify(line: &[u8]) -> Line {
    // Status replies are matched case-insensitively: the RN4871 (V1.30) answers
    // errors as lowercase `Err`, not `ERR`. Treating `Err` as data made
    // `command()` loop forever waiting for an AOK/ERR that never came — exactly
    // how a rejected `SHW` wedged the BLE task.
    if line.eq_ignore_ascii_case(b"AOK") {
        Line::Aok
    } else if line.eq_ignore_ascii_case(b"ERR") {
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
    } else if line.starts_with(b"CMD>") {
        Line::Prompt
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

    /// Read one message from the UART into `buf`.
    ///
    /// Reads one byte at a time (cancel-safe) and returns on the first complete
    /// message, which is any of:
    /// - a newline-terminated line (`\n`; trailing `\r` stripped, `\n` dropped) —
    ///   command responses such as `AOK` / `RN4871 V1.30 ...`;
    /// - a delimiter-framed status event (`%...%`) — the RN4871 does **not**
    ///   newline-terminate these, so waiting for `\n` would block forever on a
    ///   `%REBOOT%` that has already fully arrived;
    /// - the command-mode prompt (`CMD> `), which also carries no newline;
    /// - a full buffer (defensive; avoids overflow).
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
            // A status event is complete once its closing `%` arrives: opening
            // `%` at index 0 and a `%` just pushed (len >= 2). These are never
            // newline-terminated, so this is the only way they ever return.
            if buf.len() >= 2_usize && buf.first() == Some(&b'%') && byte == b'%' {
                return Ok(());
            }
            // The prompt `CMD> ` has no newline; return it so it can be skipped
            // instead of stalling the reader in prompt mode.
            if buf.ends_with(b"CMD> ") {
                return Ok(());
            }
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
                Line::Prompt | Line::Data => {
                    // Prompt (`CMD> `), echoes, or blank lines — ignore
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
                Line::Prompt => {
                    // Skip the `CMD> ` prompt; the real response follows.
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

    /// Issue `R,1` (immediate reboot) and wait for the `%REBOOT%` event.
    ///
    /// `R,1` replies with the `%REBOOT%` *event*, not `AOK`, so it cannot go
    /// through [`command`](Self::command) (which would block forever waiting for
    /// an `AOK` that never arrives). It is written raw and the reboot event
    /// awaited here.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] on UART failure.
    async fn reboot(&mut self) -> Result<(), Error<E>> {
        self.write_all(b"R,1\r").await?;
        let mut line = HVec::<u8, 64>::new();
        loop {
            self.read_line(&mut line).await?;
            if classify(&line) == Line::Event(Event::Reboot) {
                return Ok(());
            }
        }
    }

    /// Full factory reset (`SF,2`): clear all NVM config to factory defaults.
    ///
    /// Like `R,1`, `SF,2` reboots immediately and answers with the `%REBOOT%`
    /// event (not `AOK`), so it is written raw and the event awaited. Used as the
    /// first provisioning step to guarantee a known-clean starting state.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] on UART failure.
    async fn factory_reset(&mut self) -> Result<(), Error<E>> {
        self.write_all(b"SF,2\r").await?;
        let mut line = HVec::<u8, 64>::new();
        loop {
            self.read_line(&mut line).await?;
            if classify(&line) == Line::Event(Event::Reboot) {
                return Ok(());
            }
        }
    }

    /// Provision the module with the `MeteoStation` service and characteristic.
    ///
    /// Sets the name, restricts the default services, clears any prior custom
    /// GATT table (`PZ`), defines the service + one notify characteristic,
    /// switches to No-Prompt mode (`SR,4000`), persists (`WR`), and reboots to
    /// activate. Leaves the module in command mode; the caller discovers the
    /// handle.
    ///
    /// Each command is sent via [`command`](Self::command), which discards any
    /// lines preceding the `AOK`/`Err` — including the version banner the module
    /// emits after a reboot — so the command stream re-synchronises on every
    /// step. Always writes the full sequence (no verify-and-skip optimisation).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] on UART failure or [`Error::Command`] if any
    /// command is rejected.
    pub async fn provision(&mut self) -> Result<(), Error<E>> {
        // Start from a known-clean state: a full factory reset clears any prior
        // NVM config (stale services, prompt/no-prompt, locked settings) that
        // would otherwise make set-commands behave unpredictably.
        self.factory_reset().await?;
        self.enter_command_mode().await?;

        self.command(b"SN,MeteoStation").await?;
        self.command(b"SS,80").await?;
        // Max RF output power for advertising and connection. Per the RN4870/71
        // user guide §2.4.16, `SGA,0`/`SGC,0` is the HIGHEST power (0 dBm); 5 is
        // lowest. The factory reset can leave power below max, giving a very weak
        // link (RSSI ~-99 even at <2 m) that drops the connection ~1×/s.
        self.command(b"SGA,0").await?;
        self.command(b"SGC,0").await?;
        // Request a long connection supervision timeout so the link survives a
        // lossy central (e.g. a deaf BLE adapter that drops many packets) instead
        // of dropping ~1×/s. ST,<min>,<max>,<latency>,<timeout>: interval ×1.25 ms,
        // timeout ×10 ms. 20–40 ms interval, 0 latency, 6 s supervision timeout.
        self.command(b"ST,0010,0020,0000,0258").await?;
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

        // No-Prompt mode (suppress `CMD> `) for clean MCU parsing post-reboot.
        self.command(b"SR,4000").await?;
        // No `WR`: RN4871 V1.30 rejects it (`Err`) and set-commands + service
        // definitions persist to NVM on their own; the reboot activates them.
        self.reboot().await?;
        self.enter_command_mode().await?;

        Ok(())
    }

    /// Issue `LS` and parse the output to find the value handle for `CHAR_UUID`.
    ///
    /// The RN4871 lists the notify characteristic under its UUID with the value
    /// handle first — the handle `SHW` writes to, which fans the write out to a
    /// subscribed central. A following same-UUID line carries the CCCD handle,
    /// which `SHW` rejects. This takes the first match (the value handle) and
    /// stores it in `self.char_handle`. Non-matching lines — including the
    /// version banner the module emits after a reboot — are skipped.
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
                Line::Prompt | Line::Aok | Line::Err => {
                    continue;
                }
                Line::Data => {}
            }

            // Trim leading indentation, then split the characteristic line into
            // its comma-separated fields. The RN4871 lists each entry as
            //   <UUID>,<handle>,<property>[,...]
            // For our notify characteristic the value handle line (the handle
            // `SHW` writes) is listed first under the UUID, with the CCCD handle
            // on a following same-UUID line. We take the first match: the value
            // handle.
            let trimmed = trim_leading(line.as_slice());
            let mut fields = trimmed.split(|&b| b == b',');

            let Some(uuid_field) = fields.next() else {
                continue;
            };
            // Compare case-insensitively with our CHAR_UUID
            if !uuid_field.eq_ignore_ascii_case(&char_uuid_buf) {
                continue;
            }

            let Some(handle_bytes) = fields.next() else {
                // UUID matched but the line is missing the handle field.
                continue;
            };

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
        // `A,<interval>` with a SINGLE parameter (no total-time) advertises
        // FOREVER at that interval. The bare `A` only fast-advertises for 30 s
        // then drops off, leaving the device dark while idle — which fails the
        // hard requirement that it advertise full-time whenever nothing is
        // connected. 0x00A0 = 160 ms interval.
        self.command(b"A,00A0").await
    }

    /// Re-start advertising fire-and-forget: write `A` without awaiting `AOK`.
    ///
    /// Used from the event loop after a disconnect. Awaiting the `A`
    /// acknowledgement there can block forever if a central reconnects before
    /// the `AOK` arrives (the module won't re-acknowledge once connecting). The
    /// stray `AOK` is harmlessly skipped by [`next_event`](Self::next_event).
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] on UART failure.
    pub async fn restart_advertising(&mut self) -> Result<(), Error<E>> {
        // Same single-parameter form as `start_advertising` so the device
        // resumes FULL-TIME advertising (no 30 s cutoff) after a disconnect.
        self.write_all(b"A,00A0\r").await
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
        loop {
            self.read_line(&mut line).await?;
            match classify(&line) {
                Line::Event(e) => return Ok(e),
                // Skip everything that isn't an event: prompts, stray command
                // acknowledgements (e.g. from a fire-and-forget re-advertise),
                // echoes, and blank/data lines. Keep waiting for a real event.
                Line::Prompt | Line::Aok | Line::Err | Line::Data => {}
            }
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
    async fn command_returns_err_on_lowercase_err() -> TestResult {
        // Given — the RN4871 V1.30 answers a rejected command with lowercase `Err`.
        let mut drv = make_driver(b"Err\r\n");

        // When
        let result = drv.command(b"SHW,0073,01AB").await;

        // Then — must classify as an error, not hang waiting for AOK/ERR.
        assert!(
            matches!(result, Err(Error::Command)),
            "expected Err(Command) from lowercase `Err` but got {result:?}"
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
    async fn next_event_skips_stray_ack_before_event() -> TestResult {
        // Given — a fire-and-forget re-advertise leaves a stray `AOK` in the
        // stream before the next real event. next_event must skip it, not error.
        let mut drv = make_driver(b"AOK\r\n%CONNECT,0,001122334455%\r\n");

        // When
        let event = drv.next_event().await;

        // Then — the AOK is skipped and the Connect event is returned.
        assert!(
            matches!(event, Ok(Event::Connect)),
            "expected Connect past the stray AOK but got {event:?}"
        );

        Ok(())
    }

    #[test(tokio::test)]
    async fn next_event_parses_event_without_trailing_newline() -> TestResult {
        // Given — the real RN4871 emits `%...%` events with NO trailing newline.
        let mut drv = make_driver(b"%REBOOT%");

        // When
        let event = drv.next_event().await;

        // Then — must not block waiting for a `\n` that never comes.
        assert!(
            matches!(event, Ok(Event::Reboot)),
            "expected Reboot event from un-terminated %REBOOT% but got {event:?}"
        );

        Ok(())
    }

    #[test(tokio::test)]
    async fn reset_completes_on_reboot_without_newline() -> TestResult {
        // Given — hardware sends `%REBOOT%` with no `\r\n` after a reset pulse.
        let mut drv = make_driver(b"%REBOOT%");

        // When
        let result = drv.reset().await;

        // Then — reset must detect the event token and return, not hang.
        assert!(
            result.is_ok(),
            "reset should complete on un-terminated %REBOOT%, got {result:?}"
        );

        Ok(())
    }

    #[test(tokio::test)]
    async fn command_skips_prompt_before_aok() -> TestResult {
        // Given — in prompt mode a leftover `CMD> ` (no newline) precedes AOK.
        let mut drv = make_driver(b"CMD> AOK\r\n");

        // When
        let result = drv.command(b"SN,MeteoStation").await;

        // Then
        assert!(
            result.is_ok(),
            "command should skip the CMD> prompt and accept AOK, got {result:?}"
        );

        Ok(())
    }

    #[test(tokio::test)]
    async fn firmware_version_skips_prompt() -> TestResult {
        // Given — the version response is preceded by a `CMD> ` prompt.
        let mut drv = make_driver(b"CMD> RN4871 V1.30 3/18/2018\r\n");

        // When
        let version = drv.firmware_version().await;

        // Then — the prompt must be skipped, not parsed as the version line.
        assert!(
            matches!(version, Ok((1_u8, 30_u8))),
            "expected (1, 30) past the prompt but got {version:?}"
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
    async fn discover_char_handle_picks_value_handle_first() -> TestResult {
        // Given — the real V1.30 hardware lists the notify characteristic under
        // its UUID across TWO lines: the value handle (0072) first, then the
        // CCCD handle (0073). `SHW` accepts the value handle and rejects the
        // CCCD handle, so discover must take the first match.
        let ls_fixture = b"7E9A0001B5A34F6E9C112D4E6F8A0B1C\r\n  7E9A0002B5A34F6E9C112D4E6F8A0B1C,0072,00\r\n  7E9A0002B5A34F6E9C112D4E6F8A0B1C,0073,10,0\r\nEND\r\n";
        let mut rx = Vec::new();
        rx.extend_from_slice(ls_fixture);
        rx.extend_from_slice(b"AOK\r\n");
        let mut drv = make_driver(&rx);

        // When
        let handle = drv.discover_char_handle().await;

        // Then — the value handle (first), never the CCCD handle.
        assert!(
            matches!(handle, Ok(0x0072_u16)),
            "expected value handle 0x0072 (not the CCCD 0x0073) but got {handle:?}"
        );

        // And SHW targets the value handle.
        let push = drv.push_frame(&[0x01_u8, 0xAB_u8]).await;
        assert!(push.is_ok(), "push_frame should succeed: {push:?}");
        assert!(
            drv.uart.tx.windows(8_usize).any(|w| w == b"SHW,0072"),
            "tx should contain SHW,0072 but tx was: {:?}",
            str::from_utf8(&drv.uart.tx)
        );

        Ok(())
    }

    #[test(tokio::test)]
    async fn discover_char_handle_skips_reboot_banner() -> TestResult {
        // Given — after a reboot the module emits a version banner that lands in
        // the LS read; it has no UUID/handle fields and must be skipped, not
        // misparsed as the characteristic.
        let ls_fixture = b"RN4871 V1.30 3/18/2018 (c)Microchip Technology Inc\r\n7E9A0001B5A34F6E9C112D4E6F8A0B1C\r\n  7E9A0002B5A34F6E9C112D4E6F8A0B1C,0072,00\r\nEND\r\n";
        let mut rx = Vec::new();
        rx.extend_from_slice(ls_fixture);
        rx.extend_from_slice(b"AOK\r\n");
        let mut drv = make_driver(&rx);

        // When
        let handle = drv.discover_char_handle().await;

        // Then — banner skipped, value handle found.
        assert!(
            matches!(handle, Ok(0x0072_u16)),
            "expected value handle 0x0072 past the banner but got {handle:?}"
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
