#![no_std]

pub mod ble;
pub mod sensors;
pub mod utils;

// Re-export commonly used items
pub use ble::frame::{FRAME_LEN, FRAME_VERSION, FrameError, Telemetry};
pub use sensors::bmp388;
pub use utils::trunc2;
