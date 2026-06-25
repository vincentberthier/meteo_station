//! Battery state-of-charge estimation for a single-cell (`1S`) `LiPo`.
//!
//! The only battery signal the hardware exposes is the resting bus voltage read
//! by the battery-side INA219 (U7 @ 0x41). There is no coulomb counter, so the
//! charge level is estimated from a voltage→`SoC` discharge curve.
//!
//! The curve below is a representative `1S` `LiPo` open-circuit table (resting
//! voltage, no load). Under load the cell sags, so the estimate reads low while
//! the station is drawing current — acceptable for a coarse battery gauge.

/// Voltage→`SoC` breakpoints for a `1S` `LiPo`, ascending by millivolts.
///
/// `(resting_mv, percent)`. Values between breakpoints are linearly
/// interpolated; values outside the range clamp to 0 % / 100 %.
const CURVE: [(u16, u8); 21] = [
    (3270, 0),
    (3610, 5),
    (3690, 10),
    (3710, 15),
    (3730, 20),
    (3750, 25),
    (3770, 30),
    (3790, 35),
    (3800, 40),
    (3820, 45),
    (3840, 50),
    (3850, 55),
    (3870, 60),
    (3910, 65),
    (3950, 70),
    (3980, 75),
    (4020, 80),
    (4080, 85),
    (4110, 90),
    (4150, 95),
    (4200, 100),
];

/// Estimates 1S-LiPo state of charge (0–100 %) from a resting bus voltage in mV.
///
/// Below the lowest breakpoint returns 0; above the highest returns 100;
/// in between, linearly interpolates the [`CURVE`] table.
#[must_use]
pub fn battery_pct_from_mv(mv: u16) -> u8 {
    let first = CURVE[0];
    let last = CURVE[CURVE.len().saturating_sub(1)];

    if mv <= first.0 {
        return first.1;
    }
    if mv >= last.0 {
        return last.1;
    }

    // Find the bracketing segment [lo, hi] with lo.0 < mv <= hi.0.
    let mut lo = first;
    for &hi in CURVE.iter().skip(1) {
        if mv <= hi.0 {
            // Linear interpolation within [lo.0, hi.0]. span_mv > 0 since the
            // table is strictly ascending in voltage; saturating/checked ops keep
            // the arithmetic_side_effects lint satisfied.
            let span_mv = u32::from(hi.0.saturating_sub(lo.0));
            let span_pct = u32::from(hi.1.saturating_sub(lo.1));
            let offset_mv = u32::from(mv.saturating_sub(lo.0));
            let interp = offset_mv
                .saturating_mul(span_pct)
                .checked_div(span_mv)
                .unwrap_or(0);
            let pct = u32::from(lo.1).saturating_add(interp);
            // pct is bounded by hi.1 ≤ 100, so the cast cannot truncate.
            #[expect(
                clippy::cast_possible_truncation,
                reason = "pct ≤ 100 by construction (bounded by hi.1)"
            )]
            return pct as u8;
        }
        lo = hi;
    }

    last.1
}

// grcov exclude start
#[cfg(test)]
mod tests {
    use test_log::test;

    use super::*;

    #[test]
    fn full_charge_reads_100() {
        // Given / When / Then — at and above the top breakpoint
        assert_eq!(battery_pct_from_mv(4200), 100);
        assert_eq!(battery_pct_from_mv(4300), 100, "above range clamps to 100");
    }

    #[test]
    fn empty_reads_0() {
        // Given / When / Then — at and below the bottom breakpoint
        assert_eq!(battery_pct_from_mv(3270), 0);
        assert_eq!(battery_pct_from_mv(3000), 0, "below range clamps to 0");
    }

    #[test]
    fn breakpoint_is_exact() {
        // Given / When / Then — a tabulated breakpoint returns its exact percent
        assert_eq!(battery_pct_from_mv(3840), 50);
        assert_eq!(battery_pct_from_mv(3950), 70);
    }

    #[test]
    fn interpolates_between_breakpoints() {
        // Given — midway between (3840,50) and (3850,55): mv=3845
        // When
        let pct = battery_pct_from_mv(3845);

        // Then — halfway → 52 (50 + 5*5/10 = 52.5 truncated)
        assert_eq!(pct, 52);
    }

    #[test]
    fn monotonic_non_decreasing() {
        // Given / When / Then — SoC never decreases as voltage rises
        let mut prev = 0;
        for mv in (3200_u16..=4250).step_by(5) {
            let pct = battery_pct_from_mv(mv);
            assert!(pct >= prev, "SoC dropped: mv={mv} gave {pct} after {prev}");
            assert!(pct <= 100, "SoC over 100: mv={mv} gave {pct}");
            prev = pct;
        }
    }
}
// grcov exclude stop
