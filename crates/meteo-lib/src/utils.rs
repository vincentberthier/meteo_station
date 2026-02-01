/// Truncates a floating-point value to 2 decimal places.
///
/// # Arguments
///
/// * `v` - The value to truncate
///
/// # Returns
///
/// The value truncated to 2 decimal places
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "truncation is the intended behavior"
)]
#[expect(
    clippy::cast_precision_loss,
    reason = "acceptable for display-only truncation"
)]
pub fn trunc2(v: f32) -> f32 {
    let scaled = v * 100.0;
    let scaled_i = scaled as i32;
    scaled_i as f32 / 100.0
}

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(test)]
mod tests {
    extern crate std;
    use std::boxed::Box;
    use std::error;

    use test_log::test;

    type TestResult = std::result::Result<(), Box<dyn error::Error>>;

    use super::*;

    #[test]
    fn trunc2_truncates_positive_values() -> TestResult {
        // Given
        let value = 25.456;

        // When
        let result = trunc2(value);

        // Then
        assert!(
            (result - 25.45).abs() < f32::EPSILON,
            "Expected 25.45, got {result}"
        );

        Ok(())
    }

    #[test]
    fn trunc2_truncates_negative_values() -> TestResult {
        // Given
        let value = -12.789;

        // When
        let result = trunc2(value);

        // Then
        assert!(
            (result - (-12.78)).abs() < f32::EPSILON,
            "Expected -12.78, got {result}"
        );

        Ok(())
    }

    #[test]
    fn trunc2_handles_zero() -> TestResult {
        // Given
        let value = 0.0;

        // When
        let result = trunc2(value);

        // Then
        assert!(
            (result - 0.0).abs() < f32::EPSILON,
            "Expected 0.0, got {result}"
        );

        Ok(())
    }

    #[test]
    fn trunc2_rounds_down_not_nearest() -> TestResult {
        // Given
        let value = 1.999;

        // When
        let result = trunc2(value);

        // Then
        assert!(
            (result - 1.99).abs() < f32::EPSILON,
            "Expected 1.99 (truncation, not rounding), got {result}"
        );

        Ok(())
    }

    #[test]
    fn trunc2_handles_pressure_values() -> TestResult {
        // Given
        let pressure = 101_325.67;

        // When
        let result = trunc2(pressure);

        // Then
        assert!(
            (result - 101_325.67).abs() < 0.01,
            "Expected 101325.67, got {result}"
        );

        Ok(())
    }

    #[test]
    fn trunc2_handles_temperature_values() -> TestResult {
        // Given
        let temperature = 23.456_78;

        // When
        let result = trunc2(temperature);

        // Then
        assert!(
            (result - 23.45).abs() < 0.01,
            "Expected 23.45, got {result}"
        );

        Ok(())
    }
}
// grcov exclude stop
