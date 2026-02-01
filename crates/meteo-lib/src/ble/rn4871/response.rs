//! RN4871 command response type definitions.
//!
//! Defines the [`Response`] enum representing command-mode responses from the
//! RN4871 BLE module. These are protocol-level responses to commands and are
//! internal to the driver — callers interact with [`StatusEvent`](super::status_event::StatusEvent)
//! for unsolicited events.

/// A parsed command response from the RN4871 BLE module.
///
/// These variants represent protocol mechanics: acknowledgments, errors, mode
/// transitions, and unrecognized data lines. Status events (`%...%`) are
/// handled separately by [`StatusEvent`](super::status_event::StatusEvent).
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
    /// Unrecognized or intermediate data. Captures multi-line response content
    /// (e.g. individual lines from `LS`, `D`, `V` commands) as well as any
    /// unknown messages.
    Data(&'a [u8]),
}
