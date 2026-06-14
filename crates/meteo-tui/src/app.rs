//! UI state and pure update logic for the TUI.
// Types here are defined for use in client.rs, ui.rs, and main.rs; they will
// be consumed in subsequent substeps of the implementation plan.
#![expect(
    dead_code,
    reason = "app types are used by client.rs, ui.rs, and main.rs added in later substeps"
)]
use std::collections::VecDeque;

use meteo_lib::ble::registry::SENSORS;

/// Max readings retained per sensor (≈ 10 min at the ~1 Hz cadence).
pub const HISTORY_CAPACITY: usize = 600;

/// Connection state shown in the status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    /// Not connected: initial scan or post-disconnect rescan.
    Scanning,
    /// Connected and receiving notifications.
    Connected,
}

/// Message from the BLE client task to the UI loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClientEvent {
    /// Link established and subscribed.
    Connected,
    /// Link lost; client is rescanning. History is kept.
    Disconnected,
    /// A new raw-wire reading for the sensor at registry `index`.
    Reading { index: usize, raw: f32 },
}

/// Rolling display-value history for one sensor (post-transform values).
#[derive(Debug, Default, Clone)]
pub struct SensorState {
    values: VecDeque<f32>,
}

impl SensorState {
    /// Append a display value, evicting the oldest beyond `HISTORY_CAPACITY`.
    pub fn push(&mut self, value: f32) {
        if self.values.len() == HISTORY_CAPACITY {
            self.values.pop_front();
        }
        self.values.push_back(value);
    }

    #[must_use]
    pub fn latest(&self) -> Option<f32> {
        self.values.back().copied()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    #[must_use]
    pub fn min(&self) -> Option<f32> {
        self.values.iter().copied().reduce(f32::min)
    }

    #[must_use]
    pub fn max(&self) -> Option<f32> {
        self.values.iter().copied().reduce(f32::max)
    }

    /// Mean of retained values (`None` when empty). `u16::try_from` keeps the
    /// divisor cast lossless (len ≤ `HISTORY_CAPACITY` < `u16::MAX`), avoiding
    /// a `cast_precision_loss` warning.
    #[must_use]
    pub fn avg(&self) -> Option<f32> {
        let n = u16::try_from(self.values.len()).ok()?;
        if n == 0 {
            return None;
        }
        Some(self.values.iter().sum::<f32>() / f32::from(n))
    }

    /// `(x, y)` points for a ratatui `Dataset` (x = sample index).
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "sample index ≤ HISTORY_CAPACITY, exact in f64"
    )]
    pub fn points(&self) -> Vec<(f64, f64)> {
        self.values
            .iter()
            .enumerate()
            .map(|(i, &v)| (i as f64, f64::from(v)))
            .collect()
    }
}

/// Top-level UI state: per-sensor history parallel to `SENSORS`, plus status.
pub struct App {
    pub sensors: Vec<SensorState>,
    pub status: ConnectionStatus,
    pub should_quit: bool,
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sensors: SENSORS.iter().map(|_| SensorState::default()).collect(),
            status: ConnectionStatus::Scanning,
            should_quit: false,
        }
    }

    /// Apply one client event. Unknown / out-of-range sensor indices are
    /// ignored (mirrors the firmware's "unknown characteristic — ignore").
    pub fn apply(&mut self, event: ClientEvent) {
        match event {
            ClientEvent::Connected => self.status = ConnectionStatus::Connected,
            ClientEvent::Disconnected => self.status = ConnectionStatus::Scanning,
            ClientEvent::Reading { index, raw } => {
                if let (Some(desc), Some(state)) = (SENSORS.get(index), self.sensors.get_mut(index))
                {
                    state.push(desc.display_value(raw));
                }
            }
        }
    }
}

// grcov exclude start
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_appends_and_reports_latest() {
        // Given
        let mut state = SensorState::default();

        // When
        state.push(1.0);
        state.push(2.0);
        state.push(3.0);

        // Then
        assert_eq!(
            state.latest(),
            Some(3.0),
            "latest should be the last pushed value"
        );
        assert_eq!(
            state.len(),
            3,
            "len should equal the number of pushed values"
        );
    }

    #[test]
    fn push_evicts_oldest_at_capacity() {
        // Given
        let mut state = SensorState::default();

        // When
        for i in 0..=(HISTORY_CAPACITY + 4) {
            state.push(i as f32);
        }

        // Then
        assert_eq!(
            state.len(),
            HISTORY_CAPACITY,
            "len should be capped at HISTORY_CAPACITY"
        );
        assert_eq!(
            state.latest(),
            Some((HISTORY_CAPACITY + 4) as f32),
            "latest should be the final pushed value"
        );
    }

    #[test]
    fn min_max_avg_over_values() {
        // Given
        let mut state = SensorState::default();

        // When
        state.push(10.0);
        state.push(20.0);
        state.push(30.0);

        // Then
        assert_eq!(state.min(), Some(10.0), "min should be 10.0");
        assert_eq!(state.max(), Some(30.0), "max should be 30.0");
        let avg = state.avg().expect("avg should be Some for non-empty state");
        assert!(
            (avg - 20.0).abs() < 1e-3,
            "avg should be approximately 20.0, got {avg}"
        );
    }

    #[test]
    fn stats_none_when_empty() {
        // Given
        let state = SensorState::default();

        // Then
        assert!(state.min().is_none(), "min should be None for empty state");
        assert!(state.max().is_none(), "max should be None for empty state");
        assert!(state.avg().is_none(), "avg should be None for empty state");
        assert!(
            state.latest().is_none(),
            "latest should be None for empty state"
        );
        assert!(state.is_empty(), "is_empty should be true for empty state");
    }

    #[test]
    fn apply_reading_transforms_pressure() {
        // Given
        let mut app = App::new();

        // When
        app.apply(ClientEvent::Reading {
            index: 1,
            raw: 101_325.0,
        });

        // Then
        let latest = app.sensors[1]
            .latest()
            .expect("pressure sensor should have a value after apply");
        assert!(
            (latest - 1013.25).abs() < 1e-3,
            "pressure should be converted from Pa to hPa, got {latest}"
        );
    }

    #[test]
    fn apply_connected_sets_status() {
        // Given
        let mut app = App::new();

        // When
        app.apply(ClientEvent::Connected);

        // Then
        assert_eq!(
            app.status,
            ConnectionStatus::Connected,
            "status should be Connected after applying Connected event"
        );
    }

    #[test]
    fn apply_disconnected_keeps_history() {
        // Given
        let mut app = App::new();
        app.apply(ClientEvent::Reading {
            index: 0,
            raw: 20.0,
        });

        // When
        app.apply(ClientEvent::Disconnected);

        // Then
        assert_eq!(
            app.status,
            ConnectionStatus::Scanning,
            "status should be Scanning after disconnect"
        );
        assert_eq!(
            app.sensors[0].len(),
            1,
            "history should be preserved after disconnect"
        );
    }

    #[test]
    fn apply_reading_out_of_range_index_ignored() {
        // Given
        let mut app = App::new();

        // When
        app.apply(ClientEvent::Reading {
            index: 99,
            raw: 1.0,
        });

        // Then
        assert!(
            app.sensors.iter().all(|s| s.is_empty()),
            "all sensor histories should remain empty when index is out of range"
        );
    }

    #[test]
    fn app_sensor_count_matches_registry() {
        // Given / When
        let app = App::new();

        // Then
        assert_eq!(
            app.sensors.len(),
            SENSORS.len(),
            "sensor count should match registry length"
        );
    }

    #[test]
    fn points_maps_index_and_value() {
        // Given
        let mut state = SensorState::default();

        // When
        state.push(10.0);
        state.push(20.0);

        // Then
        assert_eq!(
            state.points(),
            vec![(0.0, 10.0), (1.0, 20.0)],
            "points should map index and value correctly"
        );
    }
}
// grcov exclude stop
