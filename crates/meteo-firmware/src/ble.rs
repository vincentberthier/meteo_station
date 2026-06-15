//! BLE task: owns the RN4871 driver, provisions at boot, and supervises the link.

#![expect(
    clippy::missing_asserts_for_indexing,
    reason = "false positives from defmt macro expansion"
)]

use defmt::{Debug2Format, error, info, warn};
use embassy_futures::select::{Either, select};
use embassy_stm32::gpio::Output;
use embassy_stm32::usart::BufferedUart;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::{Channel, TrySendError};
use embassy_time::Delay;
use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;
use embedded_io_async::{Read, Write};
use meteo_lib::ble::frame::Frame;
use meteo_lib::ble::rn4871::{Event, Rn4871};
use meteo_lib::ble::{SensorSample, apply_sample};

/// Channel for sensor samples from the measurement tasks to the BLE task.
pub static SENSOR_CHANNEL: Channel<ThreadModeRawMutex, SensorSample, 8> = Channel::new();

/// Publish a sample to [`SENSOR_CHANNEL`] without ever blocking the caller.
///
/// Telemetry is latest-wins: if the BLE consumer is slow or wedged and the
/// channel is full, the oldest unconsumed sample is dropped to make room for
/// the new one. A blocking `send().await` would instead back-pressure the
/// measurement task and freeze acquisition whenever the BLE link stalls — which
/// is never what we want for a periodic sensor feed.
pub fn publish_sample(sample: SensorSample) {
    let mut pending = sample;
    loop {
        match SENSOR_CHANNEL.try_send(pending) {
            Ok(()) => return,
            Err(TrySendError::Full(returned)) => {
                // Channel full ⇒ non-empty, so this receive always frees a slot;
                // the retry then succeeds. Drops the oldest, keeps the newest.
                let _dropped = SENSOR_CHANNEL.try_receive();
                pending = returned;
            }
        }
    }
}

/// Attempt a full bring-up sequence once (no retry).
///
/// Returns `Ok(())` once advertising has started, or an error on any failure.
async fn bring_up_once<U, R, D, E>(
    dev: &mut Rn4871<U, R, D>,
) -> Result<(), meteo_lib::ble::rn4871::Error<E>>
where
    U: Read<Error = E> + Write<Error = E>,
    R: OutputPin,
    D: DelayNs,
    E: core::fmt::Debug,
{
    dev.reset().await?;
    dev.enter_command_mode().await?;

    match dev.firmware_version().await {
        Ok((major, minor)) => {
            info!("RN4871 firmware: {}.{}", major, minor);
        }
        Err(e) => {
            warn!(
                "Could not read RN4871 firmware version: {:?}",
                Debug2Format(&e)
            );
            // Soft-gate: log and continue — version check is informational only.
        }
    }

    dev.provision().await?;
    dev.discover_char_handle().await?;
    dev.start_advertising().await?;
    Ok(())
}

/// Loop until the module is advertising, retrying from reset on each failure.
async fn bring_up<U, R, D, E>(dev: &mut Rn4871<U, R, D>)
where
    U: Read<Error = E> + Write<Error = E>,
    R: OutputPin,
    D: DelayNs,
    E: core::fmt::Debug,
{
    loop {
        match bring_up_once(dev).await {
            Ok(()) => {
                info!("BLE: advertising started");
                return;
            }
            Err(e) => {
                error!("BLE bring-up failed: {:?}, retrying", Debug2Format(&e));
                // Each retry restarts with a fresh reset pulse (already in bring_up_once).
            }
        }
    }
}

/// Hard-reset the module then bring it back up (deadlock circuit-breaker).
async fn recover<U, R, D, E>(dev: &mut Rn4871<U, R, D>)
where
    U: Read<Error = E> + Write<Error = E>,
    R: OutputPin,
    D: DelayNs,
    E: core::fmt::Debug,
{
    warn!("BLE: entering recovery (RST_N wedge reset)");
    dev.reset().await.ok();
    bring_up(dev).await;
}

/// BLE supervisor task.
///
/// Owns the UART and `RST_N` pin, provisions the RN4871 at boot, then
/// pushes a frame for every sensor sample received on [`SENSOR_CHANNEL`].
/// Reconnects after disconnect and uses [`recover`] on UART errors.
#[embassy_executor::task]
pub async fn ble_task(uart: BufferedUart<'static>, reset: Output<'static>) {
    let mut dev = Rn4871::new(uart, reset, Delay);
    bring_up(&mut dev).await;

    let mut frame = Frame::default();
    loop {
        match select(SENSOR_CHANNEL.receive(), dev.next_event()).await {
            Either::First(sample) => {
                apply_sample(&mut frame, sample);
                if dev.push_frame(&frame.encode()).await.is_err() {
                    recover(&mut dev).await;
                }
            }
            Either::Second(Ok(Event::Disconnect)) => {
                info!("BLE: disconnected, restarting advertising");
                dev.start_advertising().await.ok();
            }
            Either::Second(Ok(_)) => {
                // Connect / Reboot / StreamOpen / Other — log only.
                info!("BLE: event received");
            }
            Either::Second(Err(_)) => {
                error!("BLE: UART error on next_event, recovering");
                recover(&mut dev).await;
            }
        }
    }
}
