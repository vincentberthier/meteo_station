//! RN4871 BLE module protocol handling.
//!
//! Provides typed response definitions, a parser for UART messages, and
//! `defmt` formatting for on-target logging.

#[cfg(feature = "defmt")]
mod format;
mod parser;
mod response;

pub use parser::parse;
pub use response::Response;
