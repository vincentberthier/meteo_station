//! BLE peripheral stack — advertising + GATT telemetry service.
//!
//! Brings up the on-chip ESP32-H2 BLE radio via esp-radio and trouble-host,
//! advertises as `MeteoStation` (connectable, undirected), exposes a custom
//! 128-bit GATT service with one Notify characteristic, pushes sensor telemetry
//! at 1 Hz, and re-advertises immediately after every disconnect.
//!
//! The `derive` feature of trouble-host 0.6 requires `trouble-host-macros 0.4`
//! which is not available in the crates.io registry used by this workspace.
//! The GATT server is therefore built manually via `AttributeTable` /
//! `AttributeServer` / `Characteristic` — the same primitives the derive macro
//! would emit.
//!
//! The attribute table uses `esp_sync::RawMutex` (not our workspace's
//! `embassy_sync 0.8` `CriticalSectionRawMutex`) because trouble-host 0.6
//! depends on `embassy-sync 0.7`.  `esp_sync::RawMutex` implements the
//! `RawMutex` trait for both 0.7 and 0.8, so it bridges the two versions.

#![expect(
    clippy::expect_used,
    reason = "BLE task: no recovery path from controller or host errors"
)]

use core::sync::atomic::{AtomicU32, Ordering};

use defmt::{info, warn};
use embassy_futures::join::join;
use embassy_futures::select::select;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use esp_radio::ble::controller::BleConnector;
use meteo_lib::Telemetry;
use trouble_host::advertise::{
    AdStructure, Advertisement, AdvertisementParameters, BR_EDR_NOT_SUPPORTED,
    LE_GENERAL_DISCOVERABLE,
};
use trouble_host::attribute::{AttributeTable, Characteristic, CharacteristicProp, Service};
use trouble_host::gatt::GattConnectionEvent;
use trouble_host::prelude::{
    AttributeServer, DefaultPacketPool, ExternalController, GapConfig, Host, HostResources,
    PeripheralConfig, Runner, Uuid, appearance,
};
use trouble_host::{Address, Stack};

/// Fixed BLE static-random address for the weather station (top two MSB bits
/// set → random-static per BLE spec). Keep in sync with `scripts/ble_soak.sh`.
const STATION_ADDR: [u8; 6] = [0xF0, 0xCA, 0xFE, 0x00, 0x00, 0x01];
const STATION_NAME: &str = "MeteoStation";

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 2;

// ---------------------------------------------------------------------------
// Attribute table sizing
//
// GAP_SERVICE_ATTRIBUTE_COUNT = 6  (device-name decl+value, appearance decl+value,
//                                   GAP service handle, GATT service handle)
// MeteoService:
//   1  primary-service attribute
//   2  telemetry characteristic (declaration + value)
//   1  CCCD descriptor          (for Notify)
// ───────────────────────────────────
// 10  total
//
// CCCD_MAX: one CCCD slot per notifiable characteristic per connection = 1.
// CONN_MAX: matches CONNECTIONS_MAX = 1.
// ---------------------------------------------------------------------------
const ATT_MAX: usize = 10;
const CCCD_MAX: usize = 1;

/// Telemetry service UUID: 7e700001-b1df-42a1-bb5f-6a1028c793b0
///
/// BLE transmits 128-bit UUIDs LSB-first; the bytes below are the UUID octets
/// in wire (little-endian) order.
const SERVICE_UUID: Uuid = Uuid::new_long([
    0xb0, 0x93, 0xc7, 0x28, 0x10, 0x6a, 0x5f, 0xbb, 0xa1, 0x42, 0xdf, 0xb1, 0x01, 0x00, 0x70, 0x7e,
]);

/// Telemetry characteristic UUID: 7e700002-b1df-42a1-bb5f-6a1028c793b0
const TELEMETRY_UUID: Uuid = Uuid::new_long([
    0xb0, 0x93, 0xc7, 0x28, 0x10, 0x6a, 0x5f, 0xbb, 0xa1, 0x42, 0xdf, 0xb1, 0x02, 0x00, 0x70, 0x7e,
]);

/// Service UUID bytes used in the 128-bit Service UUIDs AD structure.
const SERVICE_UUID_LE: [u8; 16] = [
    0xb0, 0x93, 0xc7, 0x28, 0x10, 0x6a, 0x5f, 0xbb, 0xa1, 0x42, 0xdf, 0xb1, 0x01, 0x00, 0x70, 0x7e,
];

/// `RawMutex` type used for the GATT attribute table.
///
/// trouble-host 0.6 depends on `embassy-sync 0.7`, while our workspace targets
/// `embassy-sync 0.8`.  `esp_sync::RawMutex` bridges both versions, so it is
/// used here instead of `CriticalSectionRawMutex`.
type TableMutex = esp_sync::RawMutex;

/// Concrete controller type, fixed here so the BLE task is `'static`-spawnable.
pub type Controller = ExternalController<BleConnector<'static>, 20>;

/// Bumped every advertise-loop iteration; proves the GAP loop is cycling even
/// with no central connected (read by the RWDT supervisor in substep 5).
pub static ADV_BEAT: AtomicU32 = AtomicU32::new(0);

/// Latest-wins signal: the BMP388 task publishes here after each reading; the
/// notify loop drains it and pushes the encoded frame to every connected central.
///
/// `Signal` is latest-wins: a second `signal()` before `wait()` is consumed
/// replaces the first value — the desired behaviour for a 1 Hz sensor feed.
pub static TELEMETRY: Signal<CriticalSectionRawMutex, Telemetry> = Signal::new();

/// Convenience alias for the concrete `AttributeServer` type.
type MeteoServer<'stack> =
    AttributeServer<'stack, TableMutex, DefaultPacketPool, ATT_MAX, CCCD_MAX, CONNECTIONS_MAX>;

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

    // Storage buffer for the 17-byte telemetry value; lives for the duration of `run`.
    let mut telemetry_storage = [0_u8; 17];

    // Build the attribute table (GAP + GATT mandatory services + MeteoService).
    let mut table: AttributeTable<'_, TableMutex, ATT_MAX> = AttributeTable::new();

    // GAP + GATT mandatory services (device name + appearance).
    GapConfig::Peripheral(PeripheralConfig {
        name: STATION_NAME,
        appearance: &appearance::sensor::GENERIC_SENSOR,
    })
    .build(&mut table)
    .expect("GAP config");

    // Custom telemetry service with one Notify+Read characteristic.
    let telemetry_char: Characteristic<[u8; 17]> = {
        let mut svc = table.add_service(Service::new(SERVICE_UUID));
        let ch = svc
            .add_characteristic(
                TELEMETRY_UUID,
                [CharacteristicProp::Read, CharacteristicProp::Notify],
                [0_u8; 17],
                &mut telemetry_storage,
            )
            .build();
        svc.build();
        ch
    };

    // Wrap the table in an AttributeServer.
    let server: MeteoServer<'_> = AttributeServer::new(table);

    join(
        ble_runner(runner),
        advertise_loop(&mut peripheral, &server, &telemetry_char),
    )
    .await;
}

async fn ble_runner(mut runner: Runner<'_, Controller, DefaultPacketPool>) {
    let result: Result<(), _> = runner.run().await;
    result.expect("BLE runner exited");
}

async fn advertise_loop(
    peripheral: &mut trouble_host::peripheral::Peripheral<'_, Controller, DefaultPacketPool>,
    server: &MeteoServer<'_>,
    telemetry_char: &Characteristic<[u8; 17]>,
) {
    // Build advertisement data once: Flags + Complete Local Name + 128-bit service UUID.
    let mut adv_buf = [0_u8; 31];
    let adv_len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteLocalName(STATION_NAME.as_bytes()),
            AdStructure::ServiceUuids128(&[SERVICE_UUID_LE]),
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

        let Ok(advertiser) = peripheral
            .advertise(
                &params,
                Advertisement::ConnectableScannableUndirected {
                    adv_data: &adv_buf[..adv_len],
                    scan_data: &[],
                },
            )
            .await
        else {
            warn!("BLE: advertise() failed, retrying");
            continue;
        };

        let Ok(raw_conn) = advertiser.accept().await else {
            warn!("BLE: accept() failed (timeout?), re-advertising");
            continue;
        };

        // Attach the GATT attribute server to this connection.
        let Ok(conn) = raw_conn.with_attribute_server(server) else {
            warn!("BLE: with_attribute_server() failed, re-advertising");
            continue;
        };

        info!("BLE: central connected");

        // Drive GATT events and telemetry notifications concurrently until disconnect.
        select(gatt_events(&conn), notify_loop(&conn, telemetry_char)).await;

        info!("BLE: disconnected, re-advertising");
    }
}

/// Polls GATT connection events, returning when the connection is disconnected.
async fn gatt_events(conn: &trouble_host::gatt::GattConnection<'_, '_, DefaultPacketPool>) {
    loop {
        if let GattConnectionEvent::Disconnected { reason } = conn.next().await {
            info!(
                "BLE: disconnected (reason={:?})",
                defmt::Debug2Format(&reason)
            );
            break;
        }
        // Ignore all other events (PhyUpdated, ConnectionParamsUpdated, GATT, …)
    }
}

/// Waits for new telemetry from the `TELEMETRY` signal and notifies the central.
///
/// Returns when `notify()` returns an error (connection lost).
/// Heartbeat bumps (substep 5) are deliberately omitted here.
async fn notify_loop(
    conn: &trouble_host::gatt::GattConnection<'_, '_, DefaultPacketPool>,
    telemetry_char: &Characteristic<[u8; 17]>,
) {
    loop {
        // Latest-wins: no backlog accumulates between 1 Hz samples.
        let telem = TELEMETRY.wait().await;
        let frame = telem.encode();

        // notify() is a no-op if the central has not enabled CCCD notifications.
        if telemetry_char.notify(conn, &frame).await.is_err() {
            break;
        }
    }
}
