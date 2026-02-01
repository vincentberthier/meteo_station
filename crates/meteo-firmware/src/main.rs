#![no_std]
#![no_main]

use defmt::*;
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::i2c::Master;
use embassy_stm32::i2c::{Config as I2cConfig, I2c};
use embassy_stm32::mode::Async;
use embassy_stm32::usart::{Config as UsartConfig, UartRx};
use embassy_stm32::{bind_interrupts, peripherals};
use embassy_time::{Duration, Timer};
use meteo_lib::ble::{self, LineBuffer};
use meteo_lib::bmp388::Bmp388;
use meteo_lib::trunc2;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    I2C1_EV => embassy_stm32::i2c::EventInterruptHandler<peripherals::I2C1>;
    I2C1_ER => embassy_stm32::i2c::ErrorInterruptHandler<peripherals::I2C1>;
    USART2 => embassy_stm32::usart::InterruptHandler<peripherals::USART2>;
});

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

/// BLE UART debug task: reads bytes from the RN4871, frames lines, parses and logs them.
#[embassy_executor::task]
async fn ble_debug(mut rx: UartRx<'static, Async>) {
    let mut line_buf = LineBuffer::<256>::new();
    let mut rx_buf = [0_u8; 64];

    debug!("BLE debug task started, listening on USART2");

    loop {
        match rx.read_until_idle(&mut rx_buf).await {
            Ok(n) => {
                line_buf.push_bytes(&rx_buf[..n]);
                line_buf.for_each_line(|line| {
                    let response = ble::parse(line);
                    debug!("BLE: {:?}", response);
                });
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

    // USART2 for RN4871 BLE module on ZIO connector CN9:
    //   D53 (pin 6) = USART_B_TX = PD5
    //   D52 (pin 4) = USART_B_RX = PD6
    let usart_config = UsartConfig::default(); // 115200 8N1
    let uart_rx = UartRx::new(p.USART2, Irqs, p.PD6, p.DMA1_CH2, usart_config).unwrap();
    spawner.spawn(ble_debug(uart_rx)).unwrap();

    info!("Init complete!");

    loop {
        Timer::after(Duration::from_millis(5_000)).await;
    }
}
