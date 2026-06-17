//! Shared I2C0 bus: a single `&'static` async mutex over the esp-hal I2c, with
//! per-sensor `I2cDevice` handles. Each `embedded-hal-async` transaction locks
//! the bus for its duration and releases — whole transactions interleave on the
//! wire (standard multi-device I2C), so the BMP388 and MLX90614 tasks share GPIO10/11.

use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use esp_hal::Async;
use esp_hal::i2c::master::I2c;
use static_cell::StaticCell;

/// Concrete shared-bus I2C handle handed to each sensor task.
pub type SharedI2c = I2cDevice<'static, CriticalSectionRawMutex, I2c<'static, Async>>;

/// Backing storage for the one shared I2C0 bus mutex.
pub static I2C_BUS: StaticCell<Mutex<CriticalSectionRawMutex, I2c<'static, Async>>> =
    StaticCell::new();
