//! RN4871 unsolicited status event types.
//!
//! Defines the [`StatusEvent`] enum representing `%...%` delimited messages
//! from the RN4871 BLE module. These are asynchronous notifications that can
//! arrive at any time, independent of command/response flow.

/// An unsolicited status event from the RN4871 BLE module.
///
/// Status events are delimited by `%` characters (e.g. `%REBOOT%`,
/// `%CONNECT,1,AABBCCDDEEFF%`) and may arrive in both command mode and data
/// mode. Unlike command responses (`AOK`, `ERR`, etc.), these are not tied to
/// any specific command and represent external state changes.
#[derive(Debug, PartialEq, Eq)]
pub enum StatusEvent<'a> {
    /// Module has rebooted (`%REBOOT%`).
    Reboot,
    /// BLE connection established.
    /// `address_type`: 0 = public, 1 = random.
    /// `address`: ASCII hex MAC address (e.g. `b"AABBCCDDEEFF"`).
    Connect {
        /// Address type: 0 = public, 1 = random.
        address_type: u8,
        /// ASCII hex MAC address (e.g. `b"AABBCCDDEEFF"`).
        address: &'a [u8],
    },
    /// BLE connection lost (`%DISCONNECT%`).
    Disconnect,
    /// Connection parameters updated (`%CONN_PARAM,...%`).
    /// Raw parameter bytes (e.g. `b"0006,0000,01F4"`).
    ConnParam(&'a [u8]),
    /// UART Transparent data pipe established (`%STREAM_OPEN%`).
    StreamOpen,
    /// Unrecognized status event. Contains the inner content between `%`
    /// delimiters.
    Unknown(&'a [u8]),
}
