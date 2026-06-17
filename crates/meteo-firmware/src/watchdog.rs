use core::sync::atomic::{AtomicU32, Ordering};

use defmt::{trace, warn};
use embassy_time::{Duration, Timer};
use esp_hal::rtc_cntl::{Rtc, RwdtStage, RwdtStageAction};
use esp_hal::time::Duration as HalDuration;

use crate::ble::ADV_BEAT;

/// Bumped by the BMP388 sampler task after each successful sensor read.
pub static BMP_BEAT: AtomicU32 = AtomicU32::new(0);

/// Bumped by the BLE notify loop after each successful telemetry notification.
pub static BLE_BEAT: AtomicU32 = AtomicU32::new(0);

/// Bumped by the aggregator after each 1 Hz publish to `TELEMETRY`.
///
/// This is a task-liveness signal: it proves the aggregator loop is cycling and
/// publishing frames, not that any particular sensor has new data.
pub static AGG_BEAT: AtomicU32 = AtomicU32::new(0);

/// RWDT heartbeat supervisor.
///
/// Wakes every 2 s (the watchdog poll cadence — this Timer is the intentional
/// circuit-breaker mechanism of the hardware watchdog, NOT a synchronisation
/// sleep).  It checks that every supervised task advanced its heartbeat counter
/// since the previous poll, and only then feeds the RWDT.  If any task stalls,
/// the RWDT fires after its 8 s timeout and resets the whole chip.
///
/// Three independent gates must all be satisfied before the RWDT is fed:
///
/// 1. **BMP sampler** (`BMP_BEAT`): the BMP388 read loop is cycling (the I2C
///    sensor is responsive and the task has not stalled).
/// 2. **Aggregator publish** (`AGG_BEAT`): the aggregator task is cycling and
///    publishing frames at 1 Hz to `TELEMETRY`.
/// 3. **BLE liveness** (`ADV_BEAT || BLE_BEAT`): the BLE stack is running —
///    satisfied when EITHER the advertise loop is cycling OR notifications are
///    flowing (an idle-but-advertising device is never falsely reset).
///
/// The MLX90614 sky-IR sensor has no dedicated beat: a failed MLX read is a
/// graceful degradation (sets `sky_temp_c = None`) rather than a fatal stall,
/// and does not warrant a system reset.
#[embassy_executor::task]
pub async fn supervise(mut rtc: Rtc<'static>) {
    // Stage-0 timeout must comfortably exceed the longest legitimate gap
    // between supervisor polls (2 s poll cadence → 8 s gives 4 poll-misses
    // before the RWDT fires).
    rtc.rwdt
        .set_timeout(RwdtStage::Stage0, HalDuration::from_secs(8));
    rtc.rwdt
        .set_stage_action(RwdtStage::Stage0, RwdtStageAction::ResetSystem);
    rtc.rwdt.enable();

    let (mut last_bmp, mut last_ble, mut last_adv, mut last_agg) = (0_u32, 0_u32, 0_u32, 0_u32);
    loop {
        // Watchdog poll cadence: intentional periodic timer acting as the
        // liveness-check interval for the hardware RWDT circuit-breaker.
        Timer::after(Duration::from_secs(2)).await;

        let bmp = BMP_BEAT.load(Ordering::Relaxed);
        let ble = BLE_BEAT.load(Ordering::Relaxed);
        let adv = ADV_BEAT.load(Ordering::Relaxed);
        let agg = AGG_BEAT.load(Ordering::Relaxed);

        let sampler_alive = bmp != last_bmp;
        let agg_alive = agg != last_agg;
        // Advertising-only is still healthy: central may simply not be present.
        let ble_alive = adv != last_adv || ble != last_ble;

        if sampler_alive && agg_alive && ble_alive {
            rtc.rwdt.feed();
            trace!(
                "rwdt fed (bmp={=u32} agg={=u32} adv={=u32} ble={=u32})",
                bmp, agg, adv, ble
            );
        } else {
            warn!(
                "rwdt withheld — sampler_alive={=bool} agg_alive={=bool} ble_alive={=bool}",
                sampler_alive, agg_alive, ble_alive
            );
        }
        (last_bmp, last_ble, last_adv, last_agg) = (bmp, ble, adv, agg);
    }
}
