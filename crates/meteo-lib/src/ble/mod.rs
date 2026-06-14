//! BLE module: shared constants and codec for the `MeteoStation` BLE service.

pub mod frame;
pub mod rn4871;
pub mod sample;

pub use sample::{SensorSample, apply_sample};

/// Schema version embedded in every wire frame.
pub const SCHEMA_VERSION: u8 = 1;

/// 128-bit UUID for the `MeteoStation` GATT service.
pub const SERVICE_UUID: u128 = 0x7E9A_0001_B5A3_4F6E_9C11_2D4E_6F8A_0B1C;

/// 128-bit UUID for the `MeteoStation` measurement characteristic.
pub const CHAR_UUID: u128 = 0x7E9A_0002_B5A3_4F6E_9C11_2D4E_6F8A_0B1C;

/// Advertised BLE device name.
pub const DEVICE_NAME: &str = "MeteoStation";
