#![no_std]
#![no_main]

mod ble;
mod bluetooth;
mod pressure;

use bt_hci::controller::ExternalController;
use cyw43_pio::{PioSpi, RM2_CLOCK_DIVIDER};
use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::{
    bind_interrupts,
    gpio::{Level, Output},
    i2c::{Config as I2cConfig, I2c, InterruptHandler as I2cInterruptHandler},
    peripherals::{DMA_CH0, PIO0},
    pio::{InterruptHandler as PioInterruptHandler, Pio},
};
use embassy_time::{Duration, Timer};
use static_cell::StaticCell;
use {defmt_rtt as _, embassy_rp as _, panic_probe as _};

use crate::pressure::read_barometer;

bind_interrupts!(struct BleIrqs {
    PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
});

bind_interrupts!(struct I2cIrqs {
    I2C0_IRQ => I2cInterruptHandler<embassy_rp::peripherals::I2C0>;
});

#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn blink_led(mut led: Output<'static>) {
    loop {
        led.set_high();
        Timer::after(Duration::from_millis(500)).await;
        led.set_low();
        Timer::after(Duration::from_millis(100)).await;
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    info!("Starting Pioc 2 W Weather Station");
    let p = embassy_rp::init(Default::default());

    // Blinking led
    let led = Output::new(p.PIN_1, Level::Low);
    spawner.spawn(unwrap!(blink_led(led)));

    // // Bluetooth
    // let (fw, clm, btfw) = {
    //     let fw = include_bytes!("../cyw43-firmware/43439A0.bin");
    //     let clm = include_bytes!("../cyw43-firmware/43439A0_clm.bin");
    //     let btfw = include_bytes!("../cyw43-firmware/43439A0_btfw.bin");
    //     (fw, clm, btfw)
    // };

    // let pwr = Output::new(p.PIN_23, Level::Low);
    // let cs = Output::new(p.PIN_25, Level::High);
    // let mut pio = Pio::new(p.PIO0, BleIrqs);
    // let spi = PioSpi::new(
    //     &mut pio.common,
    //     pio.sm0,
    //     RM2_CLOCK_DIVIDER,
    //     pio.irq0,
    //     cs,
    //     p.PIN_24,
    //     p.PIN_29,
    //     p.DMA_CH0,
    // );

    // static STATE: StaticCell<cyw43::State> = StaticCell::new();
    // let state = STATE.init(cyw43::State::new());
    // let (_net_device, bt_device, mut control, runner) =
    //     cyw43::new_with_bluetooth(state, pwr, spi, fw, btfw).await;
    // // spawner.spawn(unwrap!(cyw43_task(runner)));
    // // control.init(clm).await;

    // // let controller: ExternalController<_, 10> = ExternalController::new(bt_device);

    // // Pressure sensor
    // let i2c = I2c::new_async(
    //     p.I2C0,
    //     p.PIN_5, // SCL
    //     p.PIN_4, // SDA
    //     I2cIrqs,
    //     I2cConfig::default(),
    // );

    info!("Init!");

    // spawner.spawn(unwrap!(read_barometer(i2c)));
    // spawner.spawn(unwrap!(ble::ble_task(controller, spawner)));

    loop {
        info!("Bing!");
        Timer::after(Duration::from_millis(5_000)).await;
    }
}
