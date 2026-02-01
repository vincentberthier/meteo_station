#![expect(
    clippy::missing_asserts_for_indexing,
    reason = "false positives from defmt macro expansion"
)]

use core::str;

use defmt::*;
use embassy_stm32::gpio::Output;
use embassy_stm32::usart::{self, BufferedUart};
use embassy_time::{Duration, Timer};
use meteo_lib::ble::{Command, LineBuffer, Rn4871, Uart, parse_status_event};

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

/// Extracts and logs all `%...%` status events from the line buffer.
fn process_status_events(line_buf: &mut LineBuffer<256>) {
    while line_buf.process_status_event(|event| {
        let status = parse_status_event(event);
        debug!("BLE status: {:?}", status);
    }) {}
}

/// BLE task: configures the RN4871 as `MeteoStation` and monitors messages.
#[embassy_executor::task]
pub async fn ble_task(uart: BufferedUart<'static>, mut rst_n: Output<'static>) {
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

    // Exit command mode (module starts advertising automatically)
    if let Err(e) = ble.exit_command_mode().await {
        error!("BLE: failed to exit command mode: {:?}", Debug2Format(&e));
        return;
    }
    info!("BLE: configured as MeteoStation, now advertising");

    // Monitor incoming BLE messages using the driver's UART adapter.
    // Status events (%CONNECT%, %DISCONNECT%, etc.) may not be followed
    // by \r\n on this firmware version, so we scan the raw buffer for
    // %...% patterns in addition to line-framed responses.
    let mut line_buf = LineBuffer::<256>::new();
    let mut rx_buf = [0_u8; 64];
    let raw_uart = ble.uart_mut();
    loop {
        match raw_uart.read(&mut rx_buf).await {
            Ok(n) => {
                line_buf.push_bytes(&rx_buf[..n]);
                // Extract %...% status events from the raw buffer
                process_status_events(&mut line_buf);
                // Drain any remaining line-framed data to keep buffer clean
                line_buf.for_each_line(|_line| {});
            }
            Err(e) => {
                warn!("BLE UART read error: {:?}", Debug2Format(&e));
            }
        }
    }
}
