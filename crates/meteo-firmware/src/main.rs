#![no_std]
#![no_main]
#![expect(
    clippy::expect_used,
    reason = "firmware init: no recovery from a failed peripheral init or spawn"
)]
#![expect(
    clippy::missing_asserts_for_indexing,
    reason = "false positives from defmt macro expansion"
)]
#![expect(
    clippy::future_not_send,
    reason = "Embassy/esp-rtos executor is single-threaded; Spawner and task futures are !Send by design"
)]

mod aggregator;
mod ble;
mod bmp;
mod bus;
mod watchdog;

use defmt::info;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_radio::ble::Config as BleConfig;
use esp_radio::ble::controller::BleConnector;
use {esp_backtrace as _, esp_println as _};

// The ESP-IDF second-stage bootloader (espflash v4) needs this app descriptor in
// the image to boot it.
esp_bootloader_esp_idf::esp_app_desc!();

/// BMP388 I2C address (SDO tied high). The shared bus also leaves 0x76 free for a
/// future BME280.
const BMP388_ADDR: u8 = 0x77;

/// Thin `'static`-spawnable wrapper for the BLE task.
#[embassy_executor::task]
async fn ble_task(controller: ble::Controller) {
    ble::run(controller).await;
}

#[esp_rtos::main]
async fn main(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // Heap must be initialised before esp_rtos::start and before BleConnector::new.
    // 72 KiB is sufficient for the trouble-host BLE stack; ESP32-H2 has 320 KiB SRAM.
    esp_alloc::heap_allocator!(size: 72 * 1024);

    info!("Starting ESP32-H2 Weather Station");

    // Start the esp-rtos scheduler: it owns timer group 0 (the embassy time driver)
    // and the FROM_CPU0 software interrupt that drives the thread-mode executor.
    // IMPORTANT (per esp-radio docs): the scheduler must be started BEFORE the radio.
    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_int = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    // RWDT supervisor: create the RTC handle and spawn the watchdog task.
    // Rtc::new takes LPWR; the supervisor feeds the RWDT while all tasks beat.
    let rtc = Rtc::new(peripherals.LPWR);
    spawner.spawn(watchdog::supervise(rtc).expect("watchdog already spawned"));

    // BLE: create the HCI connector and wrap it in an ExternalController for trouble-host.
    // BleConnector::new must be called AFTER esp_rtos::start (radio requires the scheduler).
    let connector =
        BleConnector::new(peripherals.BT, BleConfig::default()).expect("BLE controller init");
    let controller: ble::Controller = trouble_host::prelude::ExternalController::new(connector);
    spawner.spawn(ble_task(controller).expect("ble_task already spawned"));

    // BMP388 on I2C0: SDA = GPIO10 (J3/4), SCL = GPIO11 (J3/5). External 4.7 kΩ
    // pull-ups to 3V3 on the bus.
    let i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    )
    .expect("I2C0 init")
    .with_sda(peripherals.GPIO10)
    .with_scl(peripherals.GPIO11)
    .into_async();
    spawner.spawn(bmp::read_barometer(i2c, BMP388_ADDR).expect("read_barometer already spawned"));

    // Status LED on GPIO8 (the external LED + the onboard WS2812 share this line).
    // Driven as a plain GPIO: the external LED blinks; the WS2812 stays dark since a
    // plain toggle is not a valid addressable-LED data stream. See the note in the
    // workspace Cargo.toml for the colour-capable alternative.
    let mut led = Output::new(peripherals.GPIO8, Level::Low, OutputConfig::default());

    info!("Init complete!");

    // Liveness indicator: a steady blink so a stalled executor shows as a frozen LED.
    loop {
        led.toggle();
        Timer::after(Duration::from_millis(500)).await;
    }
}
