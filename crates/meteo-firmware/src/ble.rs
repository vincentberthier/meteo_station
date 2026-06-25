//! BLE peripheral stack — extended connectable broadcast + reserved config service.
//!
//! Brings up the on-chip ESP32-H2 BLE radio via esp-radio and trouble-host.
//! Advertises as `MeteoStation` with an extended connectable undirected
//! advertisement carrying the 38-byte v5 telemetry frame as
//! Manufacturer-Specific Data (company 0xFFFF), refreshed at 1 Hz.
//!
//! A reserved config service exposes a PIN-gated writable location
//! characteristic; on a successful write the validated Location is signalled
//! to the config task (see `crate::config`), which flushes it to NVS and
//! republishes it.  No flash I/O happens in this task.
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
use embassy_time::{Duration, Ticker};
use esp_radio::ble::controller::BleConnector;
use meteo_lib::Telemetry;
use trouble_host::advertise::{
    AdStructure, Advertisement, AdvertisementParameters, AdvertisementSet, BR_EDR_NOT_SUPPORTED,
    LE_GENERAL_DISCOVERABLE,
};
use trouble_host::att::AttErrorCode;
use trouble_host::attribute::{AttributeTable, Characteristic, CharacteristicProp, Service};
use trouble_host::connection::{Connection, RequestedConnParams};
use trouble_host::gatt::{GattConnectionEvent, GattEvent};
use trouble_host::prelude::{
    AttributeServer, DefaultPacketPool, ExternalController, GapConfig, Host, HostResources,
    PeripheralConfig, Runner, Uuid, appearance,
};
use trouble_host::{Address, Stack};

/// Fixed BLE static-random address for the weather station, in the byte order
/// `Address::random` expects: little-endian (LSB first), i.e. the on-air order.
/// `BlueZ`/`bluetoothctl` display addresses MSB-first, so these bytes reversed give
/// the human-readable `F0:CA:FE:00:00:01` used by `scripts/ble_soak.sh` and
/// `scripts/ble_notify_check.sh`. The MSB (`0xF0`, last byte here) has its top two
/// bits set → a valid static-random address per the BLE spec. Keep in sync with the
/// scripts' `DEVICE` default.
const STATION_ADDR: [u8; 6] = [0x01, 0x00, 0x00, 0xFE, 0xCA, 0xF0];
const STATION_NAME: &str = "MeteoStation";

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 2;

// ---------------------------------------------------------------------------
// Attribute table sizing
//
// GAP_SERVICE_ATTRIBUTE_COUNT = 6  (gap-service, device-name decl+value,
//                                   appearance decl+value, gatt-service)
// Reserved config service:
//   1  primary-service attribute
//   1  location characteristic declaration
//   1  location characteristic value
// ─────────────────────────────────────
//  9  total
//
// CCCD_MAX: no Notify/Indicate characteristics — keep 1 slot for mandatory
//           GATT bookkeeping.
// ---------------------------------------------------------------------------
const ATT_MAX: usize = 9;
const CCCD_MAX: usize = 1;

/// Reserved config service UUID: 7e700010-b1df-42a1-bb5f-6a1028c793b0
///
/// BLE transmits 128-bit UUIDs LSB-first; bytes below are in wire order.
const RESERVED_SERVICE_UUID: Uuid = Uuid::new_long([
    0xb0, 0x93, 0xc7, 0x28, 0x10, 0x6a, 0x5f, 0xbb, 0xa1, 0x42, 0xdf, 0xb1, 0x10, 0x00, 0x70, 0x7e,
]);

/// Location characteristic UUID: 7e700011-b1df-42a1-bb5f-6a1028c793b0
const LOCATION_UUID: Uuid = Uuid::new_long([
    0xb0, 0x93, 0xc7, 0x28, 0x10, 0x6a, 0x5f, 0xbb, 0xa1, 0x42, 0xdf, 0xb1, 0x11, 0x00, 0x70, 0x7e,
]);

/// Manufacturer-specific data company identifier (0xFFFF = test / unregistered).
const COMPANY_ID: u16 = 0xFFFF;

/// Compile-time PIN gating the location write.
/// Written as "000911" by the app (leading zeros are UI convention); only the
/// numeric value `911` matters here. App-level gate — not a cryptographic
/// secret; travels in cleartext (no BLE pairing configured).
const CONFIG_PIN: u32 = 911;

/// `RawMutex` type used for the GATT attribute table.
///
/// trouble-host 0.6 depends on `embassy-sync 0.7`, while our workspace targets
/// `embassy-sync 0.8`.  `esp_sync::RawMutex` bridges both versions, so it is
/// used here instead of `CriticalSectionRawMutex`.
type TableMutex = esp_sync::RawMutex;

/// Concrete controller type, fixed here so the BLE task is `'static`-spawnable.
pub type Controller = ExternalController<BleConnector<'static>, 20>;

/// Bumped every advertise-loop iteration; proves the GAP loop is cycling even
/// with no central connected (read by the RWDT supervisor in `watchdog.rs`).
pub static ADV_BEAT: AtomicU32 = AtomicU32::new(0);

/// Latest-wins signal: sensor tasks publish here after each reading; the
/// broadcast loop encodes the value into the manufacturer-data advertisement.
///
/// `Signal` is latest-wins: a second `signal()` before `wait()` is consumed
/// replaces the first value — the desired behaviour for a 1 Hz sensor feed.
pub static TELEMETRY: Signal<CriticalSectionRawMutex, Telemetry> = Signal::new();

/// Convenience alias for the concrete `AttributeServer` type.
type MeteoServer<'stack> =
    AttributeServer<'stack, TableMutex, DefaultPacketPool, ATT_MAX, CCCD_MAX, CONNECTIONS_MAX>;

/// Entry point for the BLE task.
pub async fn run(controller: Controller) {
    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX, 1> =
        HostResources::new();

    let stack: Stack<'_, Controller, DefaultPacketPool> =
        trouble_host::new(controller, &mut resources)
            .set_random_address(Address::random(STATION_ADDR));

    let Host {
        mut peripheral,
        runner,
        ..
    } = stack.build();

    // Storage buffer for the location characteristic value (AUTH_WRITE_LEN = 10 bytes).
    let mut location_storage = [0_u8; meteo_lib::ble::location::AUTH_WRITE_LEN];

    // Build the attribute table (GAP + GATT mandatory services + reserved config service).
    let mut table: AttributeTable<'_, TableMutex, ATT_MAX> = AttributeTable::new();

    // GAP + GATT mandatory services (device name + appearance).
    GapConfig::Peripheral(PeripheralConfig {
        name: STATION_NAME,
        appearance: &appearance::sensor::GENERIC_SENSOR,
    })
    .build(&mut table)
    .expect("GAP config");

    // Reserved config service with one Read+Write location characteristic.
    let location_char: Characteristic<[u8; meteo_lib::ble::location::AUTH_WRITE_LEN]> = {
        let mut svc = table.add_service(Service::new(RESERVED_SERVICE_UUID));
        let ch = svc
            .add_characteristic(
                LOCATION_UUID,
                [CharacteristicProp::Read, CharacteristicProp::Write],
                [0_u8; meteo_lib::ble::location::AUTH_WRITE_LEN],
                &mut location_storage,
            )
            .build();
        svc.build();
        ch
    };

    // Wrap the table in an AttributeServer.
    let server: MeteoServer<'_> = AttributeServer::new(table);

    join(
        ble_runner(runner),
        advertise_loop(&stack, &mut peripheral, &server, &location_char),
    )
    .await;
}

async fn ble_runner(mut runner: Runner<'_, Controller, DefaultPacketPool>) {
    let result: Result<(), _> = runner.run().await;
    result.expect("BLE runner exited");
}

/// Encodes a v5 advertisement payload into `buf` and returns the byte count written.
///
/// Layout: Flags (3 B) + `CompleteLocalName` "`MeteoStation`" (14 B) +
/// `ManufacturerSpecificData` 0xFFFF + 38-byte frame (42 B) = 59 B total.
/// `buf` must be at least 64 bytes.
fn encode_adv(buf: &mut [u8], frame: &[u8; meteo_lib::FRAME_LEN]) -> usize {
    AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::CompleteLocalName(STATION_NAME.as_bytes()),
            AdStructure::ManufacturerSpecificData {
                company_identifier: COMPANY_ID,
                payload: frame,
            },
        ],
        buf,
    )
    .expect("adv encode (64 B fits Flags+name+38-byte frame)")
}

async fn advertise_loop(
    stack: &Stack<'_, Controller, DefaultPacketPool>,
    peripheral: &mut trouble_host::peripheral::Peripheral<'_, Controller, DefaultPacketPool>,
    server: &MeteoServer<'_>,
    location_char: &Characteristic<[u8; meteo_lib::ble::location::AUTH_WRITE_LEN]>,
) {
    // 64 bytes is sufficient: Flags(3) + CompleteLocalName(14) + MfgData(4+38=42) = 59.
    let mut adv_buf = [0_u8; 64];

    loop {
        // Encode the current telemetry into the advertisement buffer.
        let frame = TELEMETRY
            .try_take()
            .unwrap_or_else(Telemetry::empty)
            .encode();
        let adv_len = encode_adv(&mut adv_buf, &frame);

        // Initialise handles from a temporary sets array; the temporary is dropped
        // at the end of this statement, releasing the immutable borrow on adv_buf.
        let mut handles = AdvertisementSet::handles(&[AdvertisementSet {
            params: AdvertisementParameters::default(),
            data: Advertisement::ExtConnectableNonscannableUndirected {
                adv_data: &adv_buf[..adv_len],
            },
        }]);

        // Start extended connectable advertising. A second temporary sets array is
        // passed here; it is also dropped when advertise_ext returns, freeing adv_buf
        // for the update loop below.
        let Ok(advertiser) = peripheral
            .advertise_ext(
                &[AdvertisementSet {
                    params: AdvertisementParameters::default(),
                    data: Advertisement::ExtConnectableNonscannableUndirected {
                        adv_data: &adv_buf[..adv_len],
                    },
                }],
                &mut handles,
            )
            .await
        else {
            warn!("BLE: advertise_ext() failed, retrying");
            continue;
        };

        ADV_BEAT.fetch_add(1, Ordering::Relaxed);
        crate::watchdog::BLE_BEAT.fetch_add(1, Ordering::Relaxed);
        info!(
            "BLE: broadcasting (beat={})",
            ADV_BEAT.load(Ordering::Relaxed)
        );

        // Wait for a central to connect, refreshing the manufacturer-data payload
        // at 1 Hz from TELEMETRY. Each refresh bumps ADV_BEAT and BLE_BEAT so the
        // RWDT supervisor sees the BLE stack is alive even with no central.
        let raw_conn = loop {
            // `wait()` yields until a new telemetry frame arrives (~1 Hz).
            let telem = TELEMETRY.wait().await;
            let upd_frame = telem.encode();
            let upd_len = encode_adv(&mut adv_buf, &upd_frame);

            let update_sets = [AdvertisementSet {
                params: AdvertisementParameters::default(),
                data: Advertisement::ExtConnectableNonscannableUndirected {
                    adv_data: &adv_buf[..upd_len],
                },
            }];
            if let Err(e) = peripheral
                .update_adv_data_ext(&update_sets, &mut handles)
                .await
            {
                warn!(
                    "BLE: update_adv_data_ext failed: {:?}",
                    defmt::Debug2Format(&e)
                );
            }

            ADV_BEAT.fetch_add(1, Ordering::Relaxed);
            crate::watchdog::BLE_BEAT.fetch_add(1, Ordering::Relaxed);

            if let Some(conn) = peripheral.try_accept() {
                break conn;
            }
            // update_sets dropped here; adv_buf immutable borrow released.
        };

        // Drop the advertiser to stop extended advertising before handing off to
        // the connection handler.
        drop(advertiser);

        serve_connection(stack, server, location_char, raw_conn).await;
        info!("BLE: resuming broadcast");
    }
}

/// Serves a single connected central: negotiates connection parameters,
/// handles GATT writes on the location characteristic, and bumps `BLE_BEAT`
/// at 1 Hz so the RWDT supervisor sees a live BLE stack during a connection.
async fn serve_connection(
    stack: &Stack<'_, Controller, DefaultPacketPool>,
    server: &MeteoServer<'_>,
    location_char: &Characteristic<[u8; meteo_lib::ble::location::AUTH_WRITE_LEN]>,
    raw_conn: Connection<'_, DefaultPacketPool>,
) {
    // Request a robust 8 s supervision timeout (vs the BlueZ default ~420 ms)
    // and a relaxed 80 ms connection interval. This only takes effect due to the
    // vendored trouble-host patch that routes peripheral parameter updates over
    // L2CAP signalling. Best-effort: if the central rejects, the link keeps its
    // own parameters.
    if let Err(e) = raw_conn
        .update_connection_params(stack, &RequestedConnParams::default())
        .await
    {
        warn!(
            "BLE: connection-params update request failed: {:?}",
            defmt::Debug2Format(&e)
        );
    }

    let Ok(conn) = raw_conn.with_attribute_server(server) else {
        warn!("BLE: with_attribute_server() failed");
        return;
    };

    info!("BLE: central connected (config channel)");

    // Drive GATT events and a 1 Hz heartbeat concurrently until disconnect.
    select(gatt_events(&conn, location_char), connection_heartbeat()).await;
}

/// Bumps `BLE_BEAT` at 1 Hz so the RWDT supervisor sees a live BLE stack
/// while a central is connected.  Never returns.
async fn connection_heartbeat() -> ! {
    let mut tick = Ticker::every(Duration::from_secs(1));
    loop {
        tick.next().await;
        crate::watchdog::BLE_BEAT.fetch_add(1, Ordering::Relaxed);
    }
}

/// Polls GATT connection events, returning when the connection is disconnected.
async fn gatt_events(
    conn: &trouble_host::gatt::GattConnection<'_, '_, DefaultPacketPool>,
    location_char: &Characteristic<[u8; meteo_lib::ble::location::AUTH_WRITE_LEN]>,
) {
    loop {
        match conn.next().await {
            GattConnectionEvent::Disconnected { reason } => {
                info!(
                    "BLE: disconnected (reason={:?})",
                    defmt::Debug2Format(&reason)
                );
                break;
            }
            GattConnectionEvent::ConnectionParamsUpdated {
                conn_interval,
                peripheral_latency,
                supervision_timeout,
            } => {
                info!(
                    "BLE: conn params: interval_us={=u64} latency={=u16} supervision_ms={=u64}",
                    conn_interval.as_micros(),
                    peripheral_latency,
                    supervision_timeout.as_millis()
                );
            }
            GattConnectionEvent::Gatt { event } => match event {
                GattEvent::Write(write) if write.handle() == location_char.handle => {
                    match meteo_lib::parse_authorized_write(write.data(), CONFIG_PIN) {
                        Ok(loc) => {
                            crate::config::LOCATION_WRITE.signal(loc);
                            info!("BLE: location write accepted");
                            if let Ok(reply) = write.accept() {
                                reply.send().await;
                            }
                        }
                        Err(e) => {
                            warn!("BLE: rejected location write: {:?}", e);
                            let code = match e {
                                meteo_lib::LocationWriteError::BadPin => {
                                    AttErrorCode::INSUFFICIENT_AUTHORISATION
                                }
                                meteo_lib::LocationWriteError::WrongLength(_)
                                | meteo_lib::LocationWriteError::Location(_) => {
                                    AttErrorCode::OUT_OF_RANGE
                                }
                            };
                            if let Ok(reply) = write.reject(code) {
                                reply.send().await;
                            }
                        }
                    }
                }
                other @ (GattEvent::Read(_)
                | GattEvent::Write(_)
                | GattEvent::Other(_)
                | GattEvent::NotAllowed(_)) => {
                    if let Ok(reply) = other.accept() {
                        reply.send().await;
                    }
                }
            },
            GattConnectionEvent::PhyUpdated { .. }
            | GattConnectionEvent::RequestConnectionParams(_)
            | GattConnectionEvent::DataLengthUpdated { .. } => {}
        }
    }
}
