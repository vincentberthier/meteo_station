#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::i2c::Master;
use embassy_stm32::i2c::{Config as I2cConfig, I2c};
use embassy_stm32::mode::Async;
use embassy_stm32::usart::{BufferedUart, Config as UsartConfig};
use embassy_stm32::{bind_interrupts, peripherals};
use embassy_time::{Duration, Timer};
use meteo_lib::ble::{Command, LineBuffer, Rn4871, Uart as BleUart, parse_status_event};
use meteo_lib::bmp388::Bmp388;
use meteo_lib::trunc2;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    I2C1_EV => embassy_stm32::i2c::EventInterruptHandler<peripherals::I2C1>;
    I2C1_ER => embassy_stm32::i2c::ErrorInterruptHandler<peripherals::I2C1>;
});

bind_interrupts!(struct UsartIrqs {
    USART2 => embassy_stm32::usart::BufferedInterruptHandler<peripherals::USART2>;
});

/// Adapter wrapping Embassy's [`BufferedUart`] to implement [`BleUart`].
struct EmbassyUart {
    inner: BufferedUart<'static>,
}

impl BleUart for EmbassyUart {
    type Error = embassy_stm32::usart::Error;

    async fn write(&mut self, data: &[u8]) -> Result<(), Self::Error> {
        use embedded_io_async::Write as _;
        self.inner.write_all(data).await
    }

    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        use embedded_io_async::Read as _;
        self.inner.read(buf).await
    }
}

#[embassy_executor::task]
async fn blink_led_green(mut led: Output<'static>) {
    loop {
        led.set_high();
        Timer::after(Duration::from_millis(500)).await;
        led.set_low();
        Timer::after(Duration::from_millis(100)).await;
    }
}

#[embassy_executor::task]
async fn blink_led_yellow(mut led: Output<'static>) {
    loop {
        led.set_high();
        Timer::after(Duration::from_millis(200)).await;
        led.set_low();
        Timer::after(Duration::from_millis(200)).await;
    }
}

#[embassy_executor::task]
async fn blink_led_external(mut led: Output<'static>) {
    loop {
        led.set_high();
        Timer::after(Duration::from_millis(1000)).await;
        led.set_low();
        Timer::after(Duration::from_millis(1000)).await;
    }
}

#[embassy_executor::task]
async fn read_barometer(i2c: I2c<'static, Async, Master>) {
    const BMP388_ADDR: u8 = 0x77;

    debug!("Setting up barometer");
    Timer::after(Duration::from_millis(100)).await;

    let mut sensor = match Bmp388::new(i2c, BMP388_ADDR).await {
        Ok(s) => {
            info!("BMP388 initialized successfully!");
            s
        }
        Err(e) => {
            error!("Failed to initialize BMP388: {:?}", Debug2Format(&e));
            return;
        }
    };

    loop {
        match sensor.read().await {
            Ok(reading) => {
                info!(
                    "Temperature: {}°C, Pressure: {} Pa ({} hPa)",
                    trunc2(reading.temperature),
                    trunc2(reading.pressure),
                    trunc2(reading.pressure_hpa())
                );
            }
            Err(e) => {
                warn!("Failed to read sensor: {:?}", Debug2Format(&e));
            }
        }
        Timer::after(Duration::from_secs(1)).await;
    }
}

/// Extracts and logs all `%...%` status events from the line buffer.
fn process_status_events(line_buf: &mut LineBuffer<256>) {
    while line_buf.process_status_event(|event| {
        let status = parse_status_event(event);
        debug!("BLE status: {:?}", status);
    }) {}
}

/// BLE task: configures the RN4871 as "MeteoStation" and monitors messages.
#[embassy_executor::task]
async fn ble_task(uart: BufferedUart<'static>, mut rst_n: Output<'static>) {
    let adapter = EmbassyUart { inner: uart };
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

    // Query and log firmware version
    match ble.query(Command::GetFirmwareVersion, &mut buf).await {
        Ok(n) => debug!(
            "BLE firmware: {}",
            core::str::from_utf8(&buf[..n]).unwrap_or("?")
        ),
        Err(e) => warn!("BLE: failed to query version: {:?}", Debug2Format(&e)),
    }

    // Query and log device name
    match ble.query(Command::GetDeviceName, &mut buf).await {
        Ok(n) => debug!(
            "BLE device name: {}",
            core::str::from_utf8(&buf[..n]).unwrap_or("?")
        ),
        Err(e) => warn!("BLE: failed to query name: {:?}", Debug2Format(&e)),
    }

    // Dump full device configuration
    if let Err(e) = ble
        .query_multiline(Command::DumpConfig, |line| {
            debug!("BLE config: {}", core::str::from_utf8(line).unwrap_or("?"));
        })
        .await
    {
        warn!("BLE: failed to query config: {:?}", Debug2Format(&e));
    }

    // Set device name to MeteoStation
    if let Err(e) = ble.execute(Command::SetName("MeteoStation")).await {
        warn!("BLE: failed to set name: {:?}", Debug2Format(&e));
    }

    // Set features: enable auto-advertise on power up (bit 0x2000)
    if let Err(e) = ble.execute(Command::SetFeatures(0x2000)).await {
        warn!("BLE: failed to set features: {:?}", Debug2Format(&e));
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
    let uart = ble.uart_mut();
    loop {
        match uart.read(&mut rx_buf).await {
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

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    info!("Starting Nucleo H753ZI Weather Station");
    let p = embassy_stm32::init(Default::default());

    // User LEDs: LD1 (green) = PB0, LD2 (yellow) = PE1, LD3 (red) = PB14
    let led_green = Output::new(p.PB0, Level::Low, Speed::Low);
    let led_yellow = Output::new(p.PE1, Level::Low, Speed::Low);
    spawner.spawn(blink_led_green(led_green)).unwrap();
    spawner.spawn(blink_led_yellow(led_yellow)).unwrap();

    // External LED on breadboard: D49 (CN8 pin 14) = PG2
    let led_external = Output::new(p.PG2, Level::Low, Speed::Low);
    spawner.spawn(blink_led_external(led_external)).unwrap();

    // I2C1 for BMP388 on ZIO connector CN7:
    //   D15 (pin 2) = I2C_A_SCL = PB8
    //   D14 (pin 4) = I2C_A_SDA = PB9
    let mut i2c_config = I2cConfig::default();
    i2c_config.scl_pullup = true;
    i2c_config.sda_pullup = true;
    let i2c = I2c::new(
        p.I2C1, p.PB8, p.PB9, Irqs, p.DMA1_CH0, p.DMA1_CH1, i2c_config,
    );
    spawner.spawn(read_barometer(i2c)).unwrap();

    // USART2 for RN4871 BLE module (buffered, interrupt-driven):
    //   D53 (pin 6)  = USART_B_TX  = PD5
    //   D52 (pin 4)  = USART_B_RX  = PD6
    // RST_N on D24 (CN7 pin 17) = PA4
    let usart_config = UsartConfig::default(); // 115200 8N1
    static TX_BUF: StaticCell<[u8; 256]> = StaticCell::new();
    static RX_BUF: StaticCell<[u8; 256]> = StaticCell::new();
    let tx_buf = TX_BUF.init([0_u8; 256]);
    let rx_buf = RX_BUF.init([0_u8; 256]);
    let uart = BufferedUart::new(
        p.USART2,
        p.PD6,
        p.PD5,
        tx_buf,
        rx_buf,
        UsartIrqs,
        usart_config,
    )
    .unwrap();
    let ble_rst_n = Output::new(p.PA4, Level::High, Speed::Low);
    spawner.spawn(ble_task(uart, ble_rst_n)).unwrap();

    info!("Init complete!");

    loop {
        Timer::after(Duration::from_millis(5_000)).await;
    }
}
