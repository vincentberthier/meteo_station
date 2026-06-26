//! One-shot I2C bus scanner: probe every standard 7-bit address and report which
//! devices acknowledge. Run once at boot to confirm what is physically wired
//! before the sensor tasks claim the bus.

use embedded_hal_async::i2c::I2c;
use heapless::Vec;

/// Lowest probed 7-bit address (0x00..=0x07 are reserved).
const FIRST_ADDR: u8 = 0x08;
/// Highest probed 7-bit address (0x78..=0x7F are reserved).
const LAST_ADDR: u8 = 0x77;

/// Capacity of the returned list: the full `0x08..=0x77` probe range.
pub const MAX_DEVICES: usize = (LAST_ADDR - FIRST_ADDR) as usize + 1;

/// Probe `0x08..=0x77` and return every address that acknowledges a 1-byte read.
///
/// Each probe issues START + address + a single read byte; a device that ACKs its
/// address is recorded as present, and addresses that NAK (no device) are skipped.
/// This drives real bus traffic, so call it once at boot before the per-sensor
/// tasks take the bus mutex — never concurrently with them.
pub async fn scan<I>(i2c: &mut I) -> Vec<u8, MAX_DEVICES>
where
    I: I2c,
{
    let mut found = Vec::new();
    for addr in FIRST_ADDR..=LAST_ADDR {
        let mut buf = [0_u8; 1];
        if i2c.read(addr, &mut buf).await.is_ok() {
            // Capacity equals the probe range, so this push can never overflow;
            // `.ok()` discards the infallible `Result` without tripping must-use.
            found.push(addr).ok();
        }
    }
    found
}
