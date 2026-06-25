#![no_std]

pub mod aggregate;
pub mod battery;
pub mod ble;
pub mod i2c_scan;
pub mod sensors;
pub mod utils;

// Re-export commonly used items
pub use aggregate::{Aggregator, SensorReading};
pub use battery::battery_pct_from_mv;
pub use ble::frame::{Diagnostics, FRAME_LEN, FRAME_VERSION, FrameError, Telemetry};
pub use ble::location::{
    AUTH_WRITE_LEN, Location, LocationError, LocationWriteError, parse_authorized_write,
};
pub use sensors::{bme280, bmp388, ina219, mlx90614, veml7700, weather_meter};
pub use utils::trunc2;
