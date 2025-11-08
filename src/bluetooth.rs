use bt_hci::uuid::{characteristic, descriptors};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::mutex::Mutex;
use trouble_host::prelude::*;

pub static SENSOR_DATA: Mutex<ThreadModeRawMutex, SensorReading> = Mutex::new(SensorReading::new());

// GATT Server definition
#[gatt_server]
pub struct Server {
    pub pressure_service: PressureService,
}

// Pressure service
#[gatt_service(uuid = service::ENVIRONMENTAL_SENSING)]
pub struct PressureService {
    /// Pressure
    #[descriptor(uuid = descriptors::VALID_RANGE, read, value = [0x2C, 0x01, 0xEA, 0x04])]
    #[descriptor(uuid = descriptors::MEASUREMENT_DESCRIPTION, name = "pressure", read, value = "Atmospheric Pressure")]
    #[characteristic(uuid = characteristic::PRESSURE, read, notify, value = 1000_u16)]
    pub pressure: u16,
    #[characteristic(uuid = "019a64d7-7e6b-7331-ac9a-277494e2220f", write, read, notify)]
    pub status: bool,
}

#[derive(Clone, Copy)]
pub struct SensorReading {
    pub temperature: f32,
    pub pressure: f32,
}

impl SensorReading {
    pub const fn new() -> Self {
        Self {
            temperature: 0.0,
            pressure: 0.0,
        }
    }
}
