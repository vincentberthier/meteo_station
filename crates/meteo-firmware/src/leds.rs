use embassy_stm32::gpio::Output;
use embassy_time::{Duration, Timer};

#[embassy_executor::task]
pub async fn blink_led_green(mut led: Output<'static>) {
    loop {
        led.set_high();
        Timer::after(Duration::from_millis(500)).await;
        led.set_low();
        Timer::after(Duration::from_millis(100)).await;
    }
}

#[embassy_executor::task]
pub async fn blink_led_yellow(mut led: Output<'static>) {
    loop {
        led.set_high();
        Timer::after(Duration::from_millis(200)).await;
        led.set_low();
        Timer::after(Duration::from_millis(200)).await;
    }
}

#[embassy_executor::task]
pub async fn blink_led_external(mut led: Output<'static>) {
    loop {
        led.set_high();
        Timer::after(Duration::from_millis(1000)).await;
        led.set_low();
        Timer::after(Duration::from_millis(1000)).await;
    }
}
