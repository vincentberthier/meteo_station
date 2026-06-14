//! Registry-driven ratatui rendering.
use meteo_lib::ble::registry::{SENSORS, SensorDescriptor};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::symbols;
use ratatui::text::Line;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph};

use crate::app::{App, ConnectionStatus, SensorState};

/// Render the full frame: a status line, then one row per registered sensor.
pub fn render(frame: &mut Frame, app: &App) {
    let mut constraints = vec![Constraint::Length(1_u16)];
    constraints.extend(SENSORS.iter().map(|_| Constraint::Fill(1_u16)));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(frame.area());

    render_status(frame, chunks[0], app);
    for (i, desc) in SENSORS.iter().enumerate() {
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "i < SENSORS.len() ≤ usize::MAX, so i + 1 cannot overflow"
        )]
        if let (Some(sensor_state), Some(&area)) = (app.sensors.get(i), chunks.get(i + 1)) {
            render_sensor(frame, area, desc, sensor_state);
        }
    }
}

/// Top status line: connection indicator (`● Connected` / `○ Scanning…`).
fn render_status(frame: &mut Frame, area: Rect, app: &App) {
    let (text, color) = match app.status {
        ConnectionStatus::Connected => ("● Connected", Color::Green),
        ConnectionStatus::Scanning => ("○ Scanning…", Color::Yellow),
    };
    let line = Line::from(text).style(Style::default().fg(color));
    frame.render_widget(Paragraph::new(line), area);
}

/// One sensor row: left readout (current value + min/max/avg) and right chart.
fn render_sensor(
    frame: &mut Frame,
    area: Rect,
    desc: &SensorDescriptor,
    sensor_state: &SensorState,
) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(24_u16), Constraint::Min(0_u16)])
        .split(area);
    assert!(
        cols.len() > 1,
        "horizontal split must produce at least 2 columns"
    );

    let prec = usize::from(desc.precision);
    let cur = sensor_state
        .latest()
        .map_or_else(|| "—".to_string(), |v| format!("{v:.prec$} {}", desc.unit));
    let stats_text = match (sensor_state.min(), sensor_state.max(), sensor_state.avg()) {
        (Some(lo), Some(hi), Some(avg)) => {
            format!("min {lo:.prec$}  max {hi:.prec$}  avg {avg:.prec$}")
        }
        _ => "no data".to_string(),
    };
    let readout = Paragraph::new(vec![Line::from(cur), Line::from(stats_text)])
        .block(Block::default().title(desc.name).borders(Borders::ALL));
    frame.render_widget(readout, cols[0]);

    let points = sensor_state.points();
    let bounds = y_bounds(sensor_state);
    let datasets = vec![
        Dataset::default()
            .marker(symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::default().fg(Color::Cyan))
            .data(&points),
    ];
    let x_max = x_axis_max(points.len());
    let chart = Chart::new(datasets)
        .block(Block::default().title("history").borders(Borders::ALL))
        .x_axis(Axis::default().bounds([0.0_f64, x_max]))
        .y_axis(Axis::default().bounds(bounds).labels(vec![
            format!("{:.prec$}", bounds[0]),
            format!("{:.prec$}", bounds[1]),
        ]));
    frame.render_widget(chart, cols[1]);
}

/// y-axis bounds for a sensor's chart, padded; pure & tested.
fn y_bounds(sensor_state: &SensorState) -> [f64; 2] {
    match (sensor_state.min(), sensor_state.max()) {
        (Some(lo), Some(hi)) => {
            let (lo, hi) = (f64::from(lo), f64::from(hi));
            let span = hi - lo;
            if span <= f64::EPSILON {
                [lo - 1.0_f64, hi + 1.0_f64]
            } else {
                let pad = span * 0.05_f64;
                [lo - pad, hi + pad]
            }
        }
        _ => [0.0_f64, 1.0_f64],
    }
}

/// x-axis upper bound from the sample count; pure & tested.
#[expect(
    clippy::cast_precision_loss,
    reason = "len ≤ HISTORY_CAPACITY = 600, exact in f64"
)]
fn x_axis_max(len: usize) -> f64 {
    len.max(1) as f64
}

// grcov exclude start
#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SensorState;

    #[test]
    fn y_bounds_pads_range() {
        // Given
        let mut sensor_state = SensorState::default();
        sensor_state.push(10.0_f32);
        sensor_state.push(20.0_f32);

        // When
        let bounds = y_bounds(&sensor_state);

        // Then
        assert!(
            bounds[0] < 10.0_f64,
            "lower bound {lower} should be less than 10.0",
            lower = bounds[0]
        );
        assert!(
            bounds[1] > 20.0_f64,
            "upper bound {upper} should be greater than 20.0",
            upper = bounds[1]
        );
    }

    #[test]
    fn y_bounds_single_point() {
        // Given
        let mut sensor_state = SensorState::default();
        sensor_state.push(15.0_f32);

        // When
        let bounds = y_bounds(&sensor_state);

        // Then
        assert_eq!(
            bounds,
            [14.0_f64, 16.0_f64],
            "single-point bounds should be [14.0, 16.0], got {bounds:?}"
        );
    }

    #[test]
    fn y_bounds_two_equal_values() {
        // Given
        let mut sensor_state = SensorState::default();
        sensor_state.push(15.0_f32);
        sensor_state.push(15.0_f32);

        // When
        let bounds = y_bounds(&sensor_state);

        // Then
        assert_eq!(
            bounds,
            [14.0_f64, 16.0_f64],
            "equal-values bounds should be [14.0, 16.0], got {bounds:?}"
        );
    }

    #[test]
    fn y_bounds_empty_is_unit_range() {
        // Given
        let sensor_state = SensorState::default();

        // When
        let bounds = y_bounds(&sensor_state);

        // Then
        assert_eq!(
            bounds,
            [0.0_f64, 1.0_f64],
            "empty-state bounds should be [0.0, 1.0], got {bounds:?}"
        );
    }

    #[test]
    fn x_axis_max_is_at_least_one() {
        // Given / When / Then
        assert_eq!(x_axis_max(0), 1.0_f64, "x_axis_max(0) should return 1.0");
        assert_eq!(x_axis_max(5), 5.0_f64, "x_axis_max(5) should return 5.0");
    }
}
// grcov exclude stop
