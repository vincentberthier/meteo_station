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

/// RWDT heartbeat supervisor.
///
/// Wakes every 2 s (the watchdog poll cadence — this Timer is the intentional
/// circuit-breaker mechanism of the hardware watchdog, NOT a synchronisation
/// sleep).  It checks that every supervised task advanced its heartbeat counter
/// since the previous poll, and only then feeds the RWDT.  If any task stalls,
/// the RWDT fires after its 8 s timeout and resets the whole chip.
///
/// BLE liveness is satisfied when EITHER the advertise loop is cycling OR
/// notifications are flowing, so an idle-but-advertising device is never
/// falsely reset.
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

    let (mut last_bmp, mut last_ble, mut last_adv) = (0_u32, 0_u32, 0_u32);
    loop {
        // Watchdog poll cadence: intentional periodic timer acting as the
        // liveness-check interval for the hardware RWDT circuit-breaker.
        Timer::after(Duration::from_secs(2)).await;

        let bmp = BMP_BEAT.load(Ordering::Relaxed);
        let ble = BLE_BEAT.load(Ordering::Relaxed);
        let adv = ADV_BEAT.load(Ordering::Relaxed);

        let sampler_alive = bmp != last_bmp;
        // Advertising-only is still healthy: central may simply not be present.
        let ble_alive = adv != last_adv || ble != last_ble;

        if sampler_alive && ble_alive {
            rtc.rwdt.feed();
            trace!("rwdt fed (bmp={=u32} adv={=u32} ble={=u32})", bmp, adv, ble);
        } else {
            warn!(
                "rwdt withheld — sampler_alive={=bool} ble_alive={=bool}",
                sampler_alive, ble_alive
            );
        }
        (last_bmp, last_ble, last_adv) = (bmp, ble, adv);
    }
}
