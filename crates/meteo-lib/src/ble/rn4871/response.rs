//! RN4871 response type definitions.
//!
//! Defines the [`Response`] enum representing all known message types from the
//! RN4871 BLE module, as documented in the RN4870/71 User Guide (DS50002466C).

/// A parsed response from the RN4871 BLE module.
///
/// Each variant corresponds to a specific message type. The `Data` variant
/// captures any unrecognized content, including intermediate lines of
/// multi-line responses.
#[derive(Debug, PartialEq, Eq)]
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
    /// Connection parameters updated (`%CONN_PARAM,...%`).
    /// Raw parameter bytes (e.g. `b"0006,0000,01F4"`).
    ConnParam(&'a [u8]),
    /// UART Transparent data pipe established (`%STREAM_OPEN%`).
    StreamOpen,
    /// Unrecognized or intermediate data. Captures multi-line response content
    /// (e.g. individual lines from `LS`, `D`, `V` commands) as well as any
    /// unknown messages.
    Data(&'a [u8]),
}
