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
mod anemometer;
mod ble;
mod bme;
mod bmp;
mod bus;
mod ina;
mod mlx;
mod rain;
mod vane;
mod veml;
mod watchdog;

use defmt::info;
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_executor::Spawner;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rtc_cntl::Rtc;
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_radio::ble::Config as BleConfig;
use esp_radio::ble::controller::BleConnector;
use {esp_backtrace as _, esp_println as _};

use crate::bus::{I2C_BUS, SharedI2c};

// The ESP-IDF second-stage bootloader (espflash v4) needs this app descriptor in
// the image to boot it.
esp_bootloader_esp_idf::esp_app_desc!();

/// BMP388 I2C address (SDO tied high). 0x76 is the BME280 on the same bus.
const BMP388_ADDR: u8 = 0x77;

/// MLX90614 I2C address (factory default; not remapped in EEPROM).
const MLX90614_ADDR: u8 = 0x5A;

/// BME280 I2C address (SDO → GND). 0x77 is the BMP388.
const BME280_ADDR: u8 = 0x76;

/// VEML7700 fixed I2C address (not configurable).
const VEML7700_ADDR: u8 = 0x10;

/// PV-side INA219 address (U6, A0/A1 → GND): solar panel voltage + harvest current.
const INA_PV_ADDR: u8 = 0x40;

/// Battery-side INA219 address (U7, A0 → VS): battery voltage + load current.
const INA_BATT_ADDR: u8 = 0x41;

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

    // BMP388 + MLX90614 + BME280 + VEML7700 on I2C0: SDA = GPIO10 (J3/4), SCL = GPIO11
    // (J3/5). External 4.7 kΩ pull-ups to 3V3 on the bus. The shared async mutex lets
    // each sensor task hold the bus for one transaction at a time.
    let mut i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    )
    .expect("I2C0 init")
    .with_sda(peripherals.GPIO10)
    .with_scl(peripherals.GPIO11)
    .into_async();

    // One-shot I2C bus scan at boot: enumerate every device that ACKs so the log
    // shows what is physically wired before the sensor tasks take the bus. Expected
    // today: 0x10 VEML7700, 0x40 INA219 (PV), 0x41 INA219 (batt), 0x5A MLX90614,
    // 0x76 BME280, 0x77 BMP388.
    let found = meteo_lib::i2c_scan::scan(&mut i2c).await;
    info!("I2C scan: {} device(s) responding", found.len());
    for &addr in &found {
        info!("  I2C device @ {=u8:#04x}", addr);
    }

    let bus: &'static Mutex<_, _> = I2C_BUS.init(Mutex::new(i2c));

    // Aggregator owns TELEMETRY; spawn it before the sensor tasks so the channel drains.
    spawner.spawn(aggregator::run().expect("aggregator already spawned"));
    let bmp_i2c: SharedI2c = I2cDevice::new(bus);
    spawner
        .spawn(bmp::read_barometer(bmp_i2c, BMP388_ADDR).expect("read_barometer already spawned"));
    let mlx_i2c: SharedI2c = I2cDevice::new(bus);
    spawner.spawn(mlx::read_sky(mlx_i2c, MLX90614_ADDR).expect("read_sky already spawned"));
    let bme_i2c: SharedI2c = I2cDevice::new(bus);
    spawner.spawn(bme::read_humidity(bme_i2c, BME280_ADDR).expect("read_humidity already spawned"));
    let veml_i2c: SharedI2c = I2cDevice::new(bus);
    spawner.spawn(
        veml::read_luminosity(veml_i2c, VEML7700_ADDR).expect("read_luminosity already spawned"),
    );

    // Two INA219 power monitors on the same I2C0 bus: U6 @ 0x40 on the PV feed
    // (panel V + harvest I), U7 @ 0x41 on the battery feed (battery V + load I).
    // Both degrade gracefully and bump no watchdog beat.
    let ina_pv_i2c: SharedI2c = I2cDevice::new(bus);
    spawner.spawn(
        ina::read_power(ina_pv_i2c, INA_PV_ADDR, ina::Rail::Solar).expect("ina pv already spawned"),
    );
    let ina_batt_i2c: SharedI2c = I2cDevice::new(bus);
    spawner.spawn(
        ina::read_power(ina_batt_i2c, INA_BATT_ADDR, ina::Rail::Battery)
            .expect("ina batt already spawned"),
    );

    // Weather meter (SparkFun SEN-15901): anemometer on GPIO22 (J3/9), rain gauge
    // on GPIO12 (J3/7), wind vane on GPIO1 (J1/4, ADC1). The reed switches use
    // internal pull-ups; all three tasks degrade gracefully and bump no watchdog
    // beat (calm/dry/disconnected reads are legitimate, not stalls).
    let anemometer_in = Input::new(
        peripherals.GPIO22,
        InputConfig::default().with_pull(Pull::Up),
    );
    spawner.spawn(
        anemometer::read_wind_speed(anemometer_in).expect("read_wind_speed already spawned"),
    );
    let rain_in = Input::new(
        peripherals.GPIO12,
        InputConfig::default().with_pull(Pull::Up),
    );
    spawner.spawn(rain::read_rain(rain_in).expect("read_rain already spawned"));
    spawner.spawn(
        vane::read_wind_dir(peripherals.ADC1, peripherals.GPIO1)
            .expect("read_wind_dir already spawned"),
    );

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
