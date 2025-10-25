#![no_std]
#![no_main]

mod pressure;

use defmt::*;
use embassy_executor::Spawner;
use embassy_rp::{
    bind_interrupts,
    gpio::{Level, Output},
    i2c::I2c,
};
use embassy_time::{Duration, Timer};
use {defmt_rtt as _, embassy_rp as _, panic_probe as _};

use crate::pressure::read_barometer;

bind_interrupts!(struct Irqs {
    I2C0_IRQ => embassy_rp::i2c::InterruptHandler<embassy_rp::peripherals::I2C0>;
});

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
    let p = embassy_rp::init(Default::default());

    let led = Output::new(p.PIN_1, Level::Low);

    let i2c = I2c::new_async(
        p.I2C0,
        p.PIN_5, // SCL
        p.PIN_4, // SDA
        Irqs,
        embassy_rp::i2c::Config::default(),
    );

    info!("Init!");

    spawner.spawn(blink_led(led)).ok();
    spawner.spawn(read_barometer(i2c)).ok();

    loop {
        info!("Bing!");
        Timer::after(Duration::from_millis(5_000)).await;
    }
}
