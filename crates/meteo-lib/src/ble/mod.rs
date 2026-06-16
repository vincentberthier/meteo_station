//! BLE telemetry support: the self-describing wire frame pushed over the
//! on-chip BLE Notify characteristic. (The RN4871 external-module parser was
//! removed in the ESP32-H2 port — on-chip BLE replaces it.)

pub mod frame;
