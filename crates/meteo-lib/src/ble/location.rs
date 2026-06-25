// The defmt::Format derive macro expands to code that indexes internal slices
// without preceding asserts; this triggers a false-positive lint across the file.
// The same suppression is applied in aggregate.rs and frame.rs for the same reason.
// Real slice indexing in from_wire / parse_authorized_write is guarded by the
// preceding length checks, so the bounds are statically guaranteed.
#![allow(
    clippy::missing_asserts_for_indexing,
    reason = "defmt::Format macro expansion triggers this lint as a false positive; \
              explicit length checks guard all other indexing"
)]

//! Coarse (~1 km) station-location wire type, shared by the BLE config write,
//! flash persistence, and the broadcast frame.

/// Length of the coarse location wire blob: lat i16 + lon i16 + alt i16, all LE.
pub const LOCATION_WIRE_LEN: usize = 6;

/// Coarse station location. Resolution is ~1.1 km (lat/lon 0.01°), 1 m altitude —
/// the station never holds a finer fix.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Location {
    /// Latitude in degrees (range −90 to +90, coarse 0.01° steps).
    pub latitude_deg: f32,
    /// Longitude in degrees (range −180 to +180, coarse 0.01° steps).
    pub longitude_deg: f32,
    /// Altitude in metres (coarse 1 m steps).
    pub altitude_m: f32,
}

/// Errors from [`Location::from_wire`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum LocationError {
    /// The byte slice was not exactly [`LOCATION_WIRE_LEN`] bytes long.
    WrongLength(usize),
    /// A field contained the `i16::MIN` sentinel, or lat/lon was out of geographic range.
    OutOfRange,
}

impl core::fmt::Display for LocationError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WrongLength(n) => write!(
                f,
                "wrong location wire length: expected {LOCATION_WIRE_LEN}, got {n}"
            ),
            Self::OutOfRange => write!(f, "location field sentinel or out of geographic range"),
        }
    }
}

impl core::error::Error for LocationError {}

impl Location {
    /// Parse the 6-byte coarse wire form (lat, lon, alt as i16 LE; lat/lon × 100, alt × 1).
    ///
    /// Rejects:
    /// - wrong length (not exactly [`LOCATION_WIRE_LEN`] bytes),
    /// - the `i16::MIN` sentinel in any field,
    /// - lat outside −90° to +90° (wire value outside −9000 to 9000),
    /// - lon outside −180° to +180° (wire value outside −18000 to 18000).
    ///
    /// # Errors
    ///
    /// Returns [`LocationError::WrongLength`] if `bytes.len() != LOCATION_WIRE_LEN`.
    /// Returns [`LocationError::OutOfRange`] if any field is the sentinel or out of range.
    pub fn from_wire(bytes: &[u8]) -> Result<Self, LocationError> {
        if bytes.len() != LOCATION_WIRE_LEN {
            return Err(LocationError::WrongLength(bytes.len()));
        }
        let lat = i16::from_le_bytes([bytes[0], bytes[1]]);
        let lon = i16::from_le_bytes([bytes[2], bytes[3]]);
        let alt = i16::from_le_bytes([bytes[4], bytes[5]]);
        if lat == i16::MIN || lon == i16::MIN || alt == i16::MIN {
            return Err(LocationError::OutOfRange);
        }
        if !(-9000..=9000).contains(&lat) || !(-18000..=18000).contains(&lon) {
            return Err(LocationError::OutOfRange);
        }
        Ok(Self {
            latitude_deg: f32::from(lat) / 100.0,
            longitude_deg: f32::from(lon) / 100.0,
            altitude_m: f32::from(alt),
        })
    }

    /// Serialize to the 6-byte coarse wire form (for flash storage and frame embedding).
    ///
    /// Each field is rounded to the coarse LSB and clamped away from the `i16::MIN`
    /// sentinel before encoding.
    #[must_use]
    pub fn to_wire(&self) -> [u8; LOCATION_WIRE_LEN] {
        let lat = clamp_i16(self.latitude_deg * 100.0);
        let lon = clamp_i16(self.longitude_deg * 100.0);
        let alt = clamp_i16(self.altitude_m);
        let mut b = [0_u8; LOCATION_WIRE_LEN];
        b[0..2].copy_from_slice(&lat.to_le_bytes());
        b[2..4].copy_from_slice(&lon.to_le_bytes());
        b[4..6].copy_from_slice(&alt.to_le_bytes());
        b
    }
}

/// Round `v` to nearest integer and clamp to `[i16::MIN + 1, i16::MAX]`, keeping
/// `i16::MIN` free as a sentinel (mirrors `scale_loc_i16` in `frame.rs`).
fn clamp_i16(v: f32) -> i16 {
    let r = libm::roundf(v);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "clamped to i16 range before cast"
    )]
    {
        r.max(f32::from(i16::MIN) + 1.0).min(f32::from(i16::MAX)) as i16
    }
}

/// Length of the PIN-gated GATT write payload: PIN (u32 LE) + [`LOCATION_WIRE_LEN`].
pub const AUTH_WRITE_LEN: usize = 4 + LOCATION_WIRE_LEN; // 10

/// Errors from [`parse_authorized_write`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum LocationWriteError {
    /// Payload was not exactly [`AUTH_WRITE_LEN`] bytes.
    WrongLength(usize),
    /// The leading PIN (bytes 0–3, u32 LE) did not match the expected value.
    BadPin,
    /// The location portion (bytes 4–9) was invalid.
    Location(LocationError),
}

impl core::fmt::Display for LocationWriteError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WrongLength(n) => write!(
                f,
                "wrong authorized write length: expected {AUTH_WRITE_LEN}, got {n}"
            ),
            Self::BadPin => write!(f, "PIN mismatch in authorized write"),
            Self::Location(e) => write!(f, "location parse error: {e}"),
        }
    }
}

impl core::error::Error for LocationWriteError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Location(e) => Some(e),
            Self::WrongLength(_) | Self::BadPin => None,
        }
    }
}

/// Parse a PIN-gated location write.
///
/// Layout: bytes 0–3 = PIN (u32 LE), bytes 4–9 = coarse location wire form.
/// The PIN is checked **before** the location is parsed.
///
/// # Errors
///
/// - [`LocationWriteError::WrongLength`] if `bytes.len() != AUTH_WRITE_LEN`.
/// - [`LocationWriteError::BadPin`] if the leading u32 LE does not equal `expected_pin`.
/// - [`LocationWriteError::Location`] if [`Location::from_wire`] fails on bytes 4–9.
pub fn parse_authorized_write(
    bytes: &[u8],
    expected_pin: u32,
) -> Result<Location, LocationWriteError> {
    if bytes.len() != AUTH_WRITE_LEN {
        return Err(LocationWriteError::WrongLength(bytes.len()));
    }
    let pin = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if pin != expected_pin {
        return Err(LocationWriteError::BadPin);
    }
    Location::from_wire(&bytes[4..]).map_err(LocationWriteError::Location)
}

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(test)]
mod tests {
    extern crate alloc;

    use alloc::boxed::Box;
    use core::{error::Error, result};

    use test_log::test;

    use super::*;

    type TestResult = result::Result<(), Box<dyn Error>>;

    /// PIN used for authorized-write tests.
    const TEST_PIN: u32 = 911;

    /// Paris coordinates used across multiple tests.
    const PARIS_LAT: f32 = 48.85;
    const PARIS_LON: f32 = 2.35;
    const PARIS_ALT: f32 = 35.0;

    /// Build the 10-byte authorized write payload: [PIN LE][location wire].
    fn make_auth_payload(pin: u32, loc: &Location) -> [u8; AUTH_WRITE_LEN] {
        let mut buf = [0_u8; AUTH_WRITE_LEN];
        buf[0..4].copy_from_slice(&pin.to_le_bytes());
        buf[4..10].copy_from_slice(&loc.to_wire());
        buf
    }

    /// Paris test location.
    fn paris() -> Location {
        Location {
            latitude_deg: PARIS_LAT,
            longitude_deg: PARIS_LON,
            altitude_m: PARIS_ALT,
        }
    }

    #[test]
    fn from_wire_roundtrips_to_wire() -> TestResult {
        // Given
        let loc = paris();

        // When
        let wire = loc.to_wire();
        let recovered = Location::from_wire(&wire)?;

        // Then — within one coarse LSB (0.01° lat/lon, 1 m alt)
        assert!(
            (recovered.latitude_deg - loc.latitude_deg).abs() <= 0.01,
            "lat roundtrip: got {}, expected {}",
            recovered.latitude_deg,
            loc.latitude_deg,
        );
        assert!(
            (recovered.longitude_deg - loc.longitude_deg).abs() <= 0.01,
            "lon roundtrip: got {}, expected {}",
            recovered.longitude_deg,
            loc.longitude_deg,
        );
        assert!(
            (recovered.altitude_m - loc.altitude_m).abs() <= 1.0,
            "alt roundtrip: got {}, expected {}",
            recovered.altitude_m,
            loc.altitude_m,
        );

        Ok(())
    }

    #[test]
    fn from_wire_rejects_wrong_length() {
        // Given
        let five_bytes = [0_u8; 5];

        // When / Then
        assert_eq!(
            Location::from_wire(&five_bytes),
            Err(LocationError::WrongLength(5))
        );
    }

    #[test]
    fn from_wire_rejects_sentinel() {
        // Given — lat = i16::MIN (the sentinel value)
        let mut bytes = [0_u8; LOCATION_WIRE_LEN];
        let sentinel = i16::MIN.to_le_bytes();
        bytes[0] = sentinel[0];
        bytes[1] = sentinel[1];

        // When / Then
        assert_eq!(Location::from_wire(&bytes), Err(LocationError::OutOfRange));
    }

    #[test]
    fn from_wire_rejects_out_of_range() {
        // Given — lat = 9001 (> 90.0°)
        let mut bytes = [0_u8; LOCATION_WIRE_LEN];
        let lat: i16 = 9001;
        bytes[0..2].copy_from_slice(&lat.to_le_bytes());

        // When / Then
        assert_eq!(Location::from_wire(&bytes), Err(LocationError::OutOfRange));
    }

    #[test]
    fn parse_authorized_write_accepts_correct_pin() -> TestResult {
        // Given
        let loc = paris();
        let payload = make_auth_payload(TEST_PIN, &loc);

        // When
        let result = parse_authorized_write(&payload, TEST_PIN)?;

        // Then — fields round-trip within coarse LSB
        assert!(
            (result.latitude_deg - loc.latitude_deg).abs() <= 0.01,
            "lat: got {}, expected {}",
            result.latitude_deg,
            loc.latitude_deg,
        );
        assert!(
            (result.longitude_deg - loc.longitude_deg).abs() <= 0.01,
            "lon: got {}, expected {}",
            result.longitude_deg,
            loc.longitude_deg,
        );
        assert!(
            (result.altitude_m - loc.altitude_m).abs() <= 1.0,
            "alt: got {}, expected {}",
            result.altitude_m,
            loc.altitude_m,
        );

        Ok(())
    }

    #[test]
    fn parse_authorized_write_rejects_bad_pin() {
        // Given — correct location but wrong PIN in payload (0 instead of TEST_PIN)
        let payload = make_auth_payload(0, &paris());

        // When / Then
        assert_eq!(
            parse_authorized_write(&payload, TEST_PIN),
            Err(LocationWriteError::BadPin)
        );
    }

    #[test]
    fn parse_authorized_write_rejects_wrong_length() {
        // Given — 9 bytes (one short of AUTH_WRITE_LEN = 10)
        let nine_bytes = [0_u8; 9];

        // When / Then
        assert_eq!(
            parse_authorized_write(&nine_bytes, 0),
            Err(LocationWriteError::WrongLength(9))
        );
    }

    #[test]
    fn parse_authorized_write_propagates_location_error() {
        // Given — correct PIN but lat = 9001 (> 90°)
        let mut payload = [0_u8; AUTH_WRITE_LEN];
        payload[0..4].copy_from_slice(&TEST_PIN.to_le_bytes());
        let lat: i16 = 9001;
        payload[4..6].copy_from_slice(&lat.to_le_bytes());

        // When / Then
        assert_eq!(
            parse_authorized_write(&payload, TEST_PIN),
            Err(LocationWriteError::Location(LocationError::OutOfRange))
        );
    }
}
// grcov exclude stop
