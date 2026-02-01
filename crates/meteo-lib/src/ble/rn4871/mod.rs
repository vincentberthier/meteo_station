//! RN4871 BLE module protocol handling.
//!
//! Provides typed command definitions, status event types, parsers for UART
//! messages, and `defmt` formatting for on-target logging.

pub mod command;
#[cfg(feature = "defmt")]
mod format;
pub(crate) mod parser;
pub(crate) mod response;
pub mod status_event;
pub mod status_parser;

pub use command::Command;
pub use status_event::StatusEvent;
pub use status_parser::parse as parse_status_event;
