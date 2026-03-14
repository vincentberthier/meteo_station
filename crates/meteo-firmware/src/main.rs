#![no_std]
#![no_main]
#![expect(
    clippy::missing_asserts_for_indexing,
    reason = "false positives from defmt macro expansion"
)]
#![expect(
    clippy::absolute_paths,
    reason = "false positives from bind_interrupts! macro expansion"
)]
#![expect(
    clippy::future_not_send,
    reason = "Embassy executor is single-threaded, Spawner is !Send by design"
)]
#![expect(
    clippy::expect_used,
    reason = "firmware main: no recovery from failed spawn or peripheral init"
)]

mod ble;
mod bmp;
mod leds;

use defmt::*;
use embassy_executor::Spawner;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::i2c::{Config as I2cConfig, I2c};
use embassy_stm32::usart::{BufferedUart, Config as UsartConfig};
use embassy_stm32::{Config, bind_interrupts, peripherals};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Timer};
use meteo_lib::bmp388::Reading;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    I2C1_EV => embassy_stm32::i2c::EventInterruptHandler<peripherals::I2C1>;
    I2C1_ER => embassy_stm32::i2c::ErrorInterruptHandler<peripherals::I2C1>;
});

bind_interrupts!(struct UsartIrqs {
    USART2 => embassy_stm32::usart::BufferedInterruptHandler<peripherals::USART2>;
});

/// Channel for passing sensor readings from the barometer task to the BLE task.
static SENSOR_CHANNEL: Channel<ThreadModeRawMutex, Reading, 1> = Channel::new();

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    info!("Starting Nucleo H753ZI Weather Station");
    let p = embassy_stm32::init(Config::default());

    // User LEDs: LD1 (green) = PB0, LD2 (yellow) = PE1, LD3 (red) = PB14
    let led_green = Output::new(p.PB0, Level::Low, Speed::Low);
    let led_yellow = Output::new(p.PE1, Level::Low, Speed::Low);
    spawner
        .spawn(leds::blink_led_green(led_green))
        .expect("blink_led_green already spawned");
    spawner
        .spawn(leds::blink_led_yellow(led_yellow))
        .expect("blink_led_yellow already spawned");

    // External LED on breadboard: D49 (CN8 pin 14) = PG2
    let led_external = Output::new(p.PG2, Level::Low, Speed::Low);
    spawner
        .spawn(leds::blink_led_external(led_external))
        .expect("blink_led_external already spawned");

    // I2C1 for BMP388 on ZIO connector CN7:
    //   D15 (pin 2) = I2C_A_SCL = PB8
    //   D14 (pin 4) = I2C_A_SDA = PB9
    let mut i2c_config = I2cConfig::default();
    i2c_config.scl_pullup = true;
    i2c_config.sda_pullup = true;
    let i2c = I2c::new(
        p.I2C1, p.PB8, p.PB9, Irqs, p.DMA1_CH0, p.DMA1_CH1, i2c_config,
    );
    spawner
        .spawn(bmp::read_barometer(i2c, &SENSOR_CHANNEL))
        .expect("read_barometer already spawned");

    // USART2 for RN4871 BLE module (buffered, interrupt-driven):
    //   D53 (pin 6)  = USART_B_TX  = PD5
    //   D52 (pin 4)  = USART_B_RX  = PD6
    // RST_N on D24 (CN7 pin 17) = PA4
    let usart_config = UsartConfig::default(); // 115200 8N1
    static TX_BUF: StaticCell<[u8; 256]> = StaticCell::new();
    static RX_BUF: StaticCell<[u8; 256]> = StaticCell::new();
    let tx_buf = TX_BUF.init([0_u8; 256]);
    let rx_buf = RX_BUF.init([0_u8; 256]);
    let ble_uart = BufferedUart::new(
        p.USART2,
        p.PD6,
        p.PD5,
        tx_buf,
        rx_buf,
        UsartIrqs,
        usart_config,
    )
    .expect("USART2 configuration failed");
    let ble_rst_n = Output::new(p.PA4, Level::High, Speed::Low);
    spawner
        .spawn(ble::ble_task(ble_uart, ble_rst_n, &SENSOR_CHANNEL))
        .expect("ble_task already spawned");

    info!("Init complete!");

    loop {
        Timer::after(Duration::from_millis(5_000)).await;
    }
}
