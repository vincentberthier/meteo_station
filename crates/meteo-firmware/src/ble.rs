//! BLE supervisor task for the RN4871 module.
//!
//! Manages the GAP-only link lifecycle: bring-up (reset → provision → advertise),
//! connection tracking, and UART-error recovery. No GATT, no sensor data.

use defmt::{Debug2Format, error, info, warn};
use embassy_futures::select::{Either, select};
use embassy_stm32::gpio::Output;
use embassy_stm32::usart::BufferedUart;
use embassy_time::{Delay, Duration, Ticker};
use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;
use embedded_io_async::{Read, Write};
use meteo_lib::ble::rn4871::{Error, Event, Rn4871};

// ---------------------------------------------------------------------------
// Bring-up helpers
// ---------------------------------------------------------------------------

/// Try once to bring the module from cold state to advertising.
///
/// This first calls `reset()` + `enter_command_mode()` as a comms/health probe
/// to verify the UART link and log the firmware version **before** writing any
/// NVM. `provision()` then performs its own factory-reset + re-enter sequence
/// as a clean-slate baseline for NVM writes. The apparent double-reset is
/// intentional: the first pair is a probe, the second is the NVM slate.
async fn bring_up_once<U, R, D, E>(dev: &mut Rn4871<U, R, D>) -> Result<(), Error<E>>
where
    U: Read<Error = E> + Write<Error = E>,
    R: OutputPin,
    D: DelayNs,
    E: core::fmt::Debug,
{
    dev.reset().await?;
    dev.enter_command_mode().await?;
    match dev.firmware_version().await {
        Ok((maj, min)) => info!("RN4871 firmware: {}.{}", maj, min),
        Err(e) => warn!("RN4871 version unreadable: {:?}", Debug2Format(&e)),
    }
    dev.provision().await?;
    dev.start_advertising().await?;
    Ok(())
}

/// Retry `bring_up_once` until it succeeds.
async fn bring_up(dev: &mut Rn4871<BufferedUart<'static>, Output<'static>, Delay>) {
    loop {
        match bring_up_once(dev).await {
            Ok(()) => {
                info!("BLE: advertising started");
                return;
            }
            Err(e) => error!("BLE bring-up failed: {:?}, retrying", Debug2Format(&e)),
        }
    }
}

/// Recovery path: pulse `RST_N` to wedge-recover the module, then re-run `bring_up`.
async fn recover(dev: &mut Rn4871<BufferedUart<'static>, Output<'static>, Delay>) {
    warn!("BLE: RST_N wedge recovery");
    dev.reset().await.ok();
    bring_up(dev).await;
}

// ---------------------------------------------------------------------------
// Embassy task
// ---------------------------------------------------------------------------

/// BLE supervisor task.
///
/// Owns the `BufferedUart` and `RST_N` `Output` pin for the RN4871 module.
/// Brings up the link at startup, then loops on connection events, re-arming
/// advertising after every disconnect.
///
/// The 30-second `Ticker` is defence-in-depth: it re-arms advertising when
/// disconnected, catching any silent advertising timeout the module may apply.
/// It is NOT the primary synchronization mechanism — the real disconnect signal
/// is `next_event()`, which directly observes the module's `%DISCONNECT%` frame.
#[embassy_executor::task]
pub async fn ble_task(uart: BufferedUart<'static>, reset: Output<'static>) {
    let mut dev = Rn4871::new(uart, reset, Delay);
    bring_up(&mut dev).await;

    let mut connected = false;
    // Defence-in-depth: re-arm advertising every 30 s while disconnected.
    // This is a fallback against silent advertising timeout, not the primary
    // connection-state machine — the real signal is next_event().
    let mut keepalive = Ticker::every(Duration::from_secs(30));

    loop {
        match select(keepalive.next(), dev.next_event()).await {
            Either::First(()) => {
                if !connected {
                    info!("BLE: keepalive re-arm advertising");
                    dev.restart_advertising().await.ok();
                }
            }
            Either::Second(Ok(Event::Connect)) => {
                info!("BLE: connected");
                connected = true;
            }
            Either::Second(Ok(Event::Disconnect)) => {
                info!("BLE: disconnected, re-advertising");
                connected = false;
                dev.restart_advertising().await.ok();
            }
            Either::Second(Ok(_)) => {
                info!("BLE: event");
            }
            Either::Second(Err(_)) => {
                error!("BLE: UART error, recovering");
                recover(&mut dev).await;
                connected = false;
            }
        }
    }
}
