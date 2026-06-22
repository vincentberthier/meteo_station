#![no_std]

pub mod aggregate;
pub mod ble;
pub mod sensors;
pub mod utils;

// Re-export commonly used items
pub use aggregate::{Aggregator, SensorReading};
pub use ble::frame::{Diagnostics, FRAME_LEN, FRAME_VERSION, FrameError, Telemetry};
pub use sensors::{bme280, bmp388, mlx90614, veml7700};
pub use utils::trunc2;
