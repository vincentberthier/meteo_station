//! `defmt::Format` implementation for [`Response`].
//!
//! Provides human-readable formatting for RTT logging on the target. MAC
//! addresses are displayed as `XX:XX:XX:XX:XX:XX` and byte slices are shown
//! as ASCII text.

use super::response::Response;

/// Formats a MAC address from a flat ASCII hex byte slice (e.g. `b"AABBCCDDEEFF"`)
/// into colon-separated form (`AA:BB:CC:DD:EE:FF`).
///
/// If the input is not exactly 12 bytes, formats it as-is.
fn format_mac_address(address: &[u8], f: defmt::Formatter<'_>) {
    if address.len() == 12 {
        for (i, chunk) in address.chunks(2).enumerate() {
            if i > 0 {
                defmt::write!(f, ":");
            }
            if let (Some(&hi), Some(&lo)) = (chunk.first(), chunk.get(1)) {
                defmt::write!(f, "{=char}{=char}", hi as char, lo as char);
            }
        }
    } else {
        // Fallback: write each byte as a char
        for &b in address {
            defmt::write!(f, "{=char}", b as char);
        }
    }
}

/// Writes a byte slice as ASCII characters.
fn format_ascii(data: &[u8], f: defmt::Formatter<'_>) {
    for &b in data {
        defmt::write!(f, "{=char}", b as char);
    }
}

impl defmt::Format for Response<'_> {
    fn format(&self, f: defmt::Formatter<'_>) {
        match self {
            Self::Aok => defmt::write!(f, "AOK"),
            Self::Err => defmt::write!(f, "ERR"),
            Self::Cmd => defmt::write!(f, "CMD>"),
            Self::End => defmt::write!(f, "END"),
            Self::Reboot => defmt::write!(f, "REBOOT"),
            Self::Connect {
                address_type,
                address,
            } => {
                let kind = if *address_type == 0 {
                    "public"
                } else {
                    "random"
                };
                defmt::write!(f, "Connect({=str} ", kind);
                format_mac_address(address, f);
                defmt::write!(f, ")");
            }
            Self::Disconnect => defmt::write!(f, "Disconnect"),
            Self::ConnParam(params) => {
                defmt::write!(f, "ConnParam(");
                format_ascii(params, f);
                defmt::write!(f, ")");
            }
            Self::StreamOpen => defmt::write!(f, "StreamOpen"),
            Self::Data(data) => {
                defmt::write!(f, "Data(");
                format_ascii(data, f);
                defmt::write!(f, ")");
            }
        }
    }
}
