//! BLE peripheral stack — advertising only (substep 3, no GATT yet).
//!
//! Brings up the on-chip ESP32-H2 BLE radio via esp-radio and trouble-host,
//! advertises as `MeteoStation` (connectable, undirected), and re-advertises
//! immediately after every disconnect.

#![expect(
    clippy::expect_used,
    reason = "BLE task: no recovery path from controller or host errors"
)]

use core::sync::atomic::{AtomicU32, Ordering};

use defmt::{info, warn};
use embassy_futures::join::join;
use esp_radio::ble::controller::BleConnector;
use trouble_host::advertise::{
    AdStructure, Advertisement, AdvertisementParameters, BR_EDR_NOT_SUPPORTED,
    LE_GENERAL_DISCOVERABLE,
};
use trouble_host::connection::{Connection, ConnectionEvent};
use trouble_host::peripheral::{Advertiser, Peripheral};
use trouble_host::prelude::{DefaultPacketPool, ExternalController, Host, HostResources, Runner};
use trouble_host::{Address, Stack};

/// Fixed BLE static-random address for the weather station (top two MSB bits
/// set → random-static per BLE spec). Keep in sync with `scripts/ble_soak.sh`.
const STATION_ADDR: [u8; 6] = [0xF0, 0xCA, 0xFE, 0x00, 0x00, 0x01];
const STATION_NAME: &str = "MeteoStation";

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 2;

/// Concrete controller type, fixed here so the BLE task is `'static`-spawnable.
pub type Controller = ExternalController<BleConnector<'static>, 20>;

/// Bumped every advertise-loop iteration; proves the GAP loop is cycling even
/// with no central connected (read by the RWDT supervisor in substep 5).
pub static ADV_BEAT: AtomicU32 = AtomicU32::new(0);

/// Entry point for the BLE task.
pub async fn run(controller: Controller) {
    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> =
        HostResources::new();

    let stack: Stack<'_, Controller, DefaultPacketPool> =
        trouble_host::new(controller, &mut resources)
            .set_random_address(Address::random(STATION_ADDR));

    let Host {
        mut peripheral,
        runner,
        ..
    } = stack.build();

    join(ble_runner(runner), advertise_loop(&mut peripheral)).await;
}

async fn ble_runner(mut runner: Runner<'_, Controller, DefaultPacketPool>) {
    let result: Result<(), _> = runner.run().await;
    result.expect("BLE runner exited");
}

async fn advertise_loop(peripheral: &mut Peripheral<'_, Controller, DefaultPacketPool>) {
    // Build advertisement data once: Flags + Complete Local Name.
    let mut adv_buf = [0_u8; 31];
    let adv_len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteLocalName(STATION_NAME.as_bytes()),
        ],
        &mut adv_buf,
    )
    .expect("adv encode");

    let params = AdvertisementParameters::default();

    loop {
        ADV_BEAT.fetch_add(1, Ordering::Relaxed);
        info!(
            "BLE: starting advertisement (beat={})",
            ADV_BEAT.load(Ordering::Relaxed)
        );

        let advertiser: Advertiser<'_, Controller, DefaultPacketPool> = if let Ok(a) = peripheral
            .advertise(
                &params,
                Advertisement::ConnectableScannableUndirected {
                    adv_data: &adv_buf[..adv_len],
                    scan_data: &[],
                },
            )
            .await
        {
            a
        } else {
            warn!("BLE: advertise() failed, retrying");
            continue;
        };

        let conn: Connection<'_, DefaultPacketPool> = if let Ok(c) = advertiser.accept().await {
            c
        } else {
            warn!("BLE: accept() failed (timeout?), re-advertising");
            continue;
        };

        info!("BLE: central connected");

        // Hold the connection until it disconnects, then re-advertise.
        loop {
            if let ConnectionEvent::Disconnected { reason } = conn.next().await {
                info!(
                    "BLE: disconnected (reason={:?})",
                    defmt::Debug2Format(&reason)
                );
                break;
            }
            // Ignore other events (PhyUpdated, ConnectionParamsUpdated, etc.)
        }
    }
}
