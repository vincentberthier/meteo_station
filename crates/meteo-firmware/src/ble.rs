#![expect(
    clippy::missing_asserts_for_indexing,
    reason = "false positives from defmt macro expansion"
)]

use core::str;

use defmt::*;
use embassy_stm32::gpio::Output;
use embassy_stm32::usart::{self, BufferedUart};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use meteo_lib::ble::gatt::{
    self, F32_SIZE, GattHandles, METEO_SERVICE_UUID, PRESSURE_CHAR_UUID, PROP_READ_NOTIFY,
    TEMPERATURE_CHAR_UUID,
};
use meteo_lib::ble::{
    Command, LineBuffer, Rn4871, StatusEvent, Uart, encode_f32, parse_status_event,
};
use meteo_lib::bmp388::Reading;

/// Adapter wrapping Embassy's [`BufferedUart`] to implement the BLE [`Uart`] trait.
pub struct EmbassyUart {
    inner: BufferedUart<'static>,
}

impl EmbassyUart {
    pub const fn new(uart: BufferedUart<'static>) -> Self {
        Self { inner: uart }
    }
}

impl Uart for EmbassyUart {
    type Error = usart::Error;

    async fn write(&mut self, data: &[u8]) -> Result<(), Self::Error> {
        use embedded_io_async::Write as _;
        self.inner.write_all(data).await
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        use embedded_io_async::Read as _;
        self.inner.read(buf).await
    }
}

/// BLE task: configures the RN4871 with GATT services and streams sensor data.
#[embassy_executor::task]
#[allow(
    clippy::too_many_lines,
    reason = "hardware init sequence is inherently long"
)]
pub async fn ble_task(
    uart: BufferedUart<'static>,
    mut rst_n: Output<'static>,
    sensor_channel: &'static Channel<ThreadModeRawMutex, Reading, 1>,
) {
    let adapter = EmbassyUart::new(uart);
    let mut ble = Rn4871::new(adapter);
    let mut buf = [0_u8; 64];

    // Hardware reset to get a clean state
    debug!("BLE: resetting module...");
    rst_n.set_low();
    Timer::after(Duration::from_millis(5)).await;
    rst_n.set_high();

    // Wait for reboot message
    if let Err(e) = ble.wait_for_reboot().await {
        error!("BLE: failed to detect reboot: {:?}", Debug2Format(&e));
        return;
    }
    debug!("BLE: module rebooted");

    // Enter command mode
    debug!("BLE: sending $$$ to enter command mode...");
    if let Err(e) = ble.enter_command_mode().await {
        error!("BLE: failed to enter command mode: {:?}", Debug2Format(&e));
        return;
    }
    debug!("BLE: entered command mode");

    // Factory reset to clear any bad stored configuration
    #[cfg(feature = "factory-reset")]
    {
        debug!("BLE: factory reset...");
        if let Err(e) = ble.factory_reset().await {
            error!("BLE: factory reset failed: {:?}", Debug2Format(&e));
            return;
        }
        debug!("BLE: factory reset done, module rebooting");

        // Wait for reboot after factory reset
        if let Err(e) = ble.wait_for_reboot().await {
            error!(
                "BLE: failed to detect reboot after factory reset: {:?}",
                Debug2Format(&e)
            );
            return;
        }
        debug!("BLE: module rebooted after factory reset");

        // Re-enter command mode after factory reset reboot
        if let Err(e) = ble.enter_command_mode().await {
            error!(
                "BLE: failed to re-enter command mode: {:?}",
                Debug2Format(&e)
            );
            return;
        }
        debug!("BLE: re-entered command mode");
    }

    // Query and log firmware version
    match ble.query(Command::GetFirmwareVersion, &mut buf).await {
        Ok(n) => debug!("BLE firmware: {}", str::from_utf8(&buf[..n]).unwrap_or("?")),
        Err(e) => warn!("BLE: failed to query version: {:?}", Debug2Format(&e)),
    }

    // Query and log device name
    match ble.query(Command::GetDeviceName, &mut buf).await {
        Ok(n) => debug!(
            "BLE device name: {}",
            str::from_utf8(&buf[..n]).unwrap_or("?")
        ),
        Err(e) => warn!("BLE: failed to query name: {:?}", Debug2Format(&e)),
    }

    // Dump full device configuration
    if let Err(e) = ble
        .query_multiline(Command::DumpConfig, |line| {
            debug!("BLE config: {}", str::from_utf8(line).unwrap_or("?"));
        })
        .await
    {
        warn!("BLE: failed to query config: {:?}", Debug2Format(&e));
    }

    // Set device name to MeteoStation
    if let Err(e) = ble.execute(Command::SetName("MeteoStation")).await {
        warn!("BLE: failed to set name: {:?}", Debug2Format(&e));
    }

    // --- GATT service setup ---
    // Clear existing private services
    if let Err(e) = ble.execute(Command::ClearPrivateServices).await {
        warn!("BLE: failed to clear services: {:?}", Debug2Format(&e));
    }

    // Define MeteoStation service
    if let Err(e) = ble
        .execute(Command::DefineService(&METEO_SERVICE_UUID))
        .await
    {
        warn!("BLE: failed to define service: {:?}", Debug2Format(&e));
    }

    // Define temperature characteristic (read + notify, 4 bytes)
    if let Err(e) = ble
        .execute(Command::DefineCharacteristic {
            uuid: &TEMPERATURE_CHAR_UUID,
            properties: PROP_READ_NOTIFY,
            size: F32_SIZE,
        })
        .await
    {
        warn!(
            "BLE: failed to define temperature char: {:?}",
            Debug2Format(&e)
        );
    }

    // Define pressure characteristic (read + notify, 4 bytes)
    if let Err(e) = ble
        .execute(Command::DefineCharacteristic {
            uuid: &PRESSURE_CHAR_UUID,
            properties: PROP_READ_NOTIFY,
            size: F32_SIZE,
        })
        .await
    {
        warn!(
            "BLE: failed to define pressure char: {:?}",
            Debug2Format(&e)
        );
    }

    // Exit command mode to trigger NVM store, then reboot to activate
    if let Err(e) = ble.exit_command_mode().await {
        error!("BLE: failed to exit command mode: {:?}", Debug2Format(&e));
        return;
    }
    debug!("BLE: services defined, rebooting to activate");

    // Hardware reboot to activate NVM-stored services
    rst_n.set_low();
    Timer::after(Duration::from_millis(5)).await;
    rst_n.set_high();
    if let Err(e) = ble.wait_for_reboot().await {
        error!("BLE: failed to detect reboot: {:?}", Debug2Format(&e));
        return;
    }
    debug!("BLE: module rebooted with GATT services");

    // Re-enter command mode to discover handles via LS
    if let Err(e) = ble.enter_command_mode().await {
        error!(
            "BLE: failed to enter command mode for LS: {:?}",
            Debug2Format(&e)
        );
        return;
    }

    // Discover characteristic handles
    let mut handles = GattHandles::default();
    if let Err(e) = ble
        .query_multiline(Command::ListServices, |line| {
            gatt::collect_handles(line, &mut handles);
        })
        .await
    {
        warn!("BLE: failed to list services: {:?}", Debug2Format(&e));
    }

    match (handles.temperature, handles.pressure) {
        (Some(t), Some(p)) => {
            info!(
                "BLE: handles discovered: temperature={=u16:04X}, pressure={=u16:04X}",
                t, p
            );
        }
        _ => {
            warn!(
                "BLE: some handles missing: temperature={:?}, pressure={:?}",
                handles.temperature, handles.pressure
            );
        }
    }

    // Exit command mode — module starts advertising
    if let Err(e) = ble.exit_command_mode().await {
        error!("BLE: failed to exit command mode: {:?}", Debug2Format(&e));
        return;
    }
    info!("BLE: configured as MeteoStation with GATT services, now advertising");

    // --- Monitoring loop: borrow-safe structure ---
    let mut connected = false;
    let mut line_buf = LineBuffer::<256>::new();
    let mut rx_buf = [0_u8; 64];

    loop {
        // Phase 1: drain UART data with adaptive timeout.
        let timeout = if connected {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(5)
        };
        let uart_result =
            embassy_time::with_timeout(timeout, ble.uart_mut().read(&mut rx_buf)).await;
        // ble borrow released here — uart_result owns the Result, not the reference.
        match uart_result {
            Ok(Ok(n)) => {
                line_buf.push_bytes(&rx_buf[..n]);
                // Extract %...% status events
                while line_buf.process_status_event(|event| match parse_status_event(event) {
                    StatusEvent::Connect {
                        address_type,
                        address,
                    } => {
                        info!(
                            "BLE: connected (type={=u8}, addr={})",
                            address_type,
                            str::from_utf8(address).unwrap_or("?")
                        );
                        connected = true;
                    }
                    StatusEvent::Disconnect => {
                        info!("BLE: disconnected");
                        connected = false;
                    }
                    StatusEvent::WriteConfig { handle, data } => {
                        debug!(
                            "BLE: CCCD write handle={=u16:04X} data={}",
                            handle,
                            str::from_utf8(data).unwrap_or("?")
                        );
                    }
                    other => debug!("BLE: {:?}", other),
                }) {}
                // Drain remaining line-framed data
                line_buf.for_each_line(|_| {});
            }
            Ok(Err(e)) => warn!("BLE UART read error: {:?}", Debug2Format(&e)),
            Err(_) => {} // timeout — fall through to check sensor data
        }

        // Phase 2: push sensor data when connected.
        // ble is no longer borrowed here — safe to call driver methods.
        if connected
            && let Ok(reading) = sensor_channel.try_receive()
            && let (Some(t_handle), Some(p_handle)) = (handles.temperature, handles.pressure)
        {
            let t_bytes = encode_f32(reading.temperature);
            let p_bytes = encode_f32(reading.pressure);
            info!(
                "BLE: pushing sensor data (t={=f32}, p={=f32}) via SHW; entering command mode",
                reading.temperature, reading.pressure
            );
            if let Err(e) = ble.enter_command_mode().await {
                warn!("BLE: cmd mode failed: {:?}", Debug2Format(&e));
                continue;
            }
            if let Err(e) = ble
                .execute(Command::ServerWrite {
                    handle: t_handle,
                    data: &t_bytes,
                })
                .await
            {
                warn!("BLE: temp write failed: {:?}", Debug2Format(&e));
            } else {
                info!("BLE: temp SHW ok (handle={=u16:04X})", t_handle);
            }
            if let Err(e) = ble
                .execute(Command::ServerWrite {
                    handle: p_handle,
                    data: &p_bytes,
                })
                .await
            {
                warn!("BLE: pressure write failed: {:?}", Debug2Format(&e));
            } else {
                info!("BLE: pressure SHW ok (handle={=u16:04X})", p_handle);
            }
            if let Err(e) = ble.exit_command_mode().await {
                warn!("BLE: exit cmd mode failed: {:?}", Debug2Format(&e));
            } else {
                info!("BLE: exited command mode after SHW");
            }
        }
    }
}
