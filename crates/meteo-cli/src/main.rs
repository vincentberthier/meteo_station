//! BLE client CLI for the `MeteoStation` weather station.
//!
//! Scans for a BLE peripheral named `MeteoStation`, connects, discovers the
//! custom GATT service, reads initial values, and subscribes to notifications.
#![expect(
    clippy::print_stdout,
    reason = "CLI tool: stdout is the user interface"
)]

use std::error::Error;
use std::time::Duration;

use btleplug::api::{Central, CharPropFlags, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::stream::StreamExt as _;
use meteo_lib::ble::encoding::decode_f32;
use meteo_lib::ble::gatt::{METEO_SERVICE_UUID, PRESSURE_CHAR_UUID, TEMPERATURE_CHAR_UUID};
use tokio::time;
use uuid::Uuid;

/// Convert a 16-byte UUID array to a `uuid::Uuid`.
const fn uuid_from_bytes(bytes: &[u8; 16]) -> Uuid {
    Uuid::from_bytes(*bytes)
}

/// Decode an f32 from a BLE characteristic value (4 bytes LE).
fn decode_reading(data: &[u8]) -> Option<f32> {
    let bytes: &[u8; 4] = data.first_chunk()?;
    Some(decode_f32(bytes))
}

/// Find the first BLE adapter.
async fn get_adapter(manager: &Manager) -> Result<Adapter, Box<dyn Error>> {
    let adapters = manager.adapters().await?;
    adapters
        .into_iter()
        .next()
        .ok_or_else(|| "no BLE adapters found".into())
}

/// Scan for a peripheral named `MeteoStation`.
async fn find_meteo_station(adapter: &Adapter) -> Result<Peripheral, Box<dyn Error>> {
    adapter.start_scan(ScanFilter::default()).await?;
    time::sleep(Duration::from_secs(5)).await;

    let peripherals = adapter.peripherals().await?;
    for p in peripherals {
        if let Some(props) = p.properties().await?
            && let Some(name) = &props.local_name
            && name.contains("MeteoStation")
        {
            return Ok(p);
        }
    }
    Err("MeteoStation not found — is the device powered on and advertising?".into())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let _service_uuid = uuid_from_bytes(&METEO_SERVICE_UUID);
    let temp_uuid = uuid_from_bytes(&TEMPERATURE_CHAR_UUID);
    let pres_uuid = uuid_from_bytes(&PRESSURE_CHAR_UUID);

    println!("Scanning for MeteoStation...");

    let manager = Manager::new().await?;
    let adapter = get_adapter(&manager).await?;
    let device = find_meteo_station(&adapter).await?;

    println!(
        "Found MeteoStation: {:?}",
        device
            .properties()
            .await?
            .and_then(|p| p.local_name)
            .unwrap_or_default()
    );

    println!("Connecting...");
    device.connect().await?;
    println!("Connected. Discovering services...");
    device.discover_services().await?;

    let chars = device.characteristics();

    let temp_char = chars
        .iter()
        .find(|c| c.uuid == temp_uuid)
        .ok_or("temperature characteristic not found")?;
    let pres_char = chars
        .iter()
        .find(|c| c.uuid == pres_uuid)
        .ok_or("pressure characteristic not found")?;

    // Read initial values
    if let Ok(data) = device.read(temp_char).await
        && let Some(t) = decode_reading(&data)
    {
        println!("Temperature: {t:.2}°C");
    }
    if let Ok(data) = device.read(pres_char).await
        && let Some(p) = decode_reading(&data)
    {
        println!("Pressure: {p:.1} Pa ({:.2} hPa)", p / 100.0);
    }

    // Subscribe to notifications
    if temp_char.properties.contains(CharPropFlags::NOTIFY) {
        device.subscribe(temp_char).await?;
        println!("Subscribed to temperature notifications");
    }
    if pres_char.properties.contains(CharPropFlags::NOTIFY) {
        device.subscribe(pres_char).await?;
        println!("Subscribed to pressure notifications");
    }

    println!("Listening for notifications (Ctrl+C to stop)...\n");

    let mut events = device.notifications().await?;
    while let Some(notification) = events.next().await {
        if let Some(val) = decode_reading(&notification.value) {
            if notification.uuid == temp_uuid {
                println!("Temperature: {val:.2}°C");
            } else if notification.uuid == pres_uuid {
                println!("Pressure: {val:.1} Pa ({:.2} hPa)", val / 100.0);
            } else {
                // Unknown characteristic — ignore
            }
        }
    }

    println!("Disconnecting...");
    device.disconnect().await?;
    Ok(())
}

// grcov exclude start
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_from_bytes_matches_expected_string() {
        // Given
        let uuid = uuid_from_bytes(&METEO_SERVICE_UUID);

        // When
        let s = uuid.to_string();

        // Then
        assert_eq!(
            s, "a4e64b8b-8db3-4e08-a7d5-7d3c3f2e1a00",
            "UUID byte order should match expected string"
        );
    }

    #[test]
    fn decode_sensor_reading_round_trip() {
        // Given
        use meteo_lib::ble::encoding::encode_f32;
        let temp = 23.45_f32;
        let pressure = 101_325.0_f32;

        // When
        let temp_bytes = encode_f32(temp);
        let pres_bytes = encode_f32(pressure);
        let decoded_temp = decode_reading(&temp_bytes);
        let decoded_pres = decode_reading(&pres_bytes);

        // Then
        assert_eq!(decoded_temp, Some(temp), "temperature round-trip");
        assert_eq!(decoded_pres, Some(pressure), "pressure round-trip");
    }

    #[test]
    fn decode_reading_too_short_returns_none() {
        // Given
        let data = [0x01, 0x02];

        // When
        let result = decode_reading(&data);

        // Then
        assert!(result.is_none(), "too-short data should return None");
    }
}
// grcov exclude stop
