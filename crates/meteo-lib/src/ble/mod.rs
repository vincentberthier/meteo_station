pub mod driver;
pub mod encoding;
pub mod gatt;
pub mod line_buffer;
pub mod registry;
pub mod rn4871;

pub use driver::{Error, Rn4871, Uart};
pub use encoding::{bytes_to_hex, decode_f32, encode_f32};
pub use gatt::{GattHandles, METEO_SERVICE_UUID, PRESSURE_CHAR_UUID, TEMPERATURE_CHAR_UUID};
pub use line_buffer::LineBuffer;
pub use registry::{SENSORS, SensorDescriptor, index_for_uuid, pa_to_hpa};
pub use rn4871::{Command, StatusEvent, parse_status_event};
