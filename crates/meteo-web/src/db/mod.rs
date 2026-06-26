//! SQLite storage layer for the `MeteoStation` web dashboard.
//!
//! All types and functions in this module are SSR-only: the database lives on
//! the server and is never accessed from the wasm/hydrate bundle.
//!
//! ## Design notes
//!
//! - The `samples` table stores one row per minute (the finest granularity that
//!   the BLE → aggregator pipeline produces).
//! - [`query_history`] re-buckets on the fly at query time via
//!   `GROUP BY bucket_ts / bucket_secs`, so the client can request any coarser
//!   resolution without a second write path.
//! - Per-minute `*_avg` columns are re-averaged as a **sample-count-weighted**
//!   mean: `SUM(field_avg * sample_count) / SUM(...)`, preserving statistical
//!   accuracy across unequal-length buckets.
//! - Power in watts is **not** stored in the table: the raw millivolt and
//!   milliampere averages are stored and `meteo_chart::power_w` is applied in
//!   Rust after reading them back, to keep the schema stable.
//! - Wind direction is stored only as `wind_dir_avg` (degrees) per minute.
//!   Re-aggregation at coarser granularity uses a simple arithmetic mean:
//!   direction is display-only and the per-minute resolution already smooths it.
//!
//! [`query_history`]: DbHandle::query_history

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::Context as _;
use chrono::NaiveDate;
use meteo_chart::power_w;
use rusqlite::{Connection, params};
use tokio::task::spawn_blocking;

use crate::types::{HistoryRow, Metric, MetricStat, TracePoint};

/// One persisted minute bucket — one `Option<f64>` per `samples` column plus a count.
///
/// Field order and names mirror `schema.sql` 1-to-1 so the INSERT is
/// positional and auditable against the schema definition.
#[derive(Debug, Clone, PartialEq)]
pub struct BucketRow {
    /// Unix epoch second for this minute bucket (floored to the minute).
    pub bucket_ts: i64,
    /// Air temperature minimum in °C.
    pub temp_min: Option<f64>,
    /// Air temperature maximum in °C.
    pub temp_max: Option<f64>,
    /// Air temperature weighted average in °C.
    pub temp_avg: Option<f64>,
    /// Pressure minimum in hPa.
    pub pressure_min: Option<f64>,
    /// Pressure maximum in hPa.
    pub pressure_max: Option<f64>,
    /// Pressure weighted average in hPa.
    pub pressure_avg: Option<f64>,
    /// Humidity minimum in %.
    pub humidity_min: Option<f64>,
    /// Humidity maximum in %.
    pub humidity_max: Option<f64>,
    /// Humidity weighted average in %.
    pub humidity_avg: Option<f64>,
    /// Sky temperature minimum in °C.
    pub sky_min: Option<f64>,
    /// Sky temperature maximum in °C.
    pub sky_max: Option<f64>,
    /// Sky temperature weighted average in °C.
    pub sky_avg: Option<f64>,
    /// Luminosity minimum in lux.
    pub lux_min: Option<f64>,
    /// Luminosity maximum in lux.
    pub lux_max: Option<f64>,
    /// Luminosity weighted average in lux.
    pub lux_avg: Option<f64>,
    /// Wind speed minimum in m/s.
    pub wind_min: Option<f64>,
    /// Wind speed maximum (gust) in m/s.
    pub wind_max: Option<f64>,
    /// Wind speed weighted average in m/s.
    pub wind_avg: Option<f64>,
    /// Wind direction arithmetic mean in degrees (0–360).
    pub wind_dir_avg: Option<f64>,
    /// Rain rate weighted average in mm/h.
    pub rain_avg: Option<f64>,
    /// Rain rate maximum in mm/h.
    pub rain_max: Option<f64>,
    /// Battery `SoC` weighted average in percent.
    pub battery_avg: Option<f64>,
    /// Solar-panel voltage weighted average in millivolts.
    pub solar_mv_avg: Option<f64>,
    /// Solar-panel current weighted average in milliamps.
    pub solar_ma_avg: Option<f64>,
    /// Battery bus voltage weighted average in millivolts.
    pub batt_mv_avg: Option<f64>,
    /// Load current weighted average in milliamps.
    pub load_ma_avg: Option<f64>,
    /// Number of raw samples (BLE frames) that went into this minute bucket.
    pub sample_count: i64,
}

/// Query parameters for [`DbHandle::query_history`].
pub struct HistoryQuery {
    /// Inclusive start unix timestamp (seconds).
    pub from_ts: i64,
    /// Exclusive end unix timestamp (seconds).
    pub to_ts: i64,
    /// Re-aggregation window width in seconds (e.g. 3600 for hourly buckets).
    /// Must be greater than zero.
    pub bucket_secs: i64,
}

/// Shared handle to the SQLite samples database.
///
/// The inner [`rusqlite::Connection`] is guarded by a `std::sync::Mutex`.
/// All blocking I/O is offloaded via `tokio::task::spawn_blocking`, keeping
/// the async runtime threads unblocked. Only one writer runs at a time (one
/// minute bucket per minute), so lock contention is negligible.
#[derive(Clone)]
pub struct DbHandle {
    conn: Arc<Mutex<Connection>>,
}

impl DbHandle {
    /// Open (or create) the `samples` database at `path`.
    ///
    /// Enables WAL journal mode for concurrent read access, then executes
    /// the embedded `schema.sql` migration (idempotent `CREATE TABLE IF NOT EXISTS`).
    /// Pass `Path::new(":memory:")` for an in-process in-memory database (useful
    /// in tests).
    ///
    /// # Errors
    ///
    /// Returns an error if the SQLite file cannot be opened or the schema SQL fails.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path).context("open SQLite database")?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .context("enable WAL journal mode")?;
        conn.execute_batch(include_str!("schema.sql"))
            .context("apply schema.sql migration")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Persist one minute-bucket row (INSERT OR REPLACE).
    ///
    /// The blocking rusqlite call is offloaded via `spawn_blocking`.
    ///
    /// # Errors
    ///
    /// Returns an error if the database write fails or the worker thread panics.
    pub async fn store_bucket(&self, row: BucketRow) -> anyhow::Result<()> {
        let conn = Arc::clone(&self.conn);
        spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|_e| anyhow::anyhow!("db mutex poisoned"))?;
            store_bucket_impl(&guard, &row)
        })
        .await?
    }

    /// Query aggregated history between `q.from_ts` (inclusive) and `q.to_ts`
    /// (exclusive), re-bucketed into `q.bucket_secs`-second windows.
    ///
    /// Within each window the per-minute averages are combined as a
    /// sample-count-weighted mean; min/max collapse via `MIN`/`MAX`. Power in
    /// watts is computed in Rust from the raw millivolt/milliamp averages.
    ///
    /// The blocking rusqlite call is offloaded via `spawn_blocking`.
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails or the worker thread panics.
    pub async fn query_history(&self, q: HistoryQuery) -> anyhow::Result<Vec<HistoryRow>> {
        let conn = Arc::clone(&self.conn);
        spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|_e| anyhow::anyhow!("db mutex poisoned"))?;
            history_impl(&guard, &q)
        })
        .await?
    }

    /// Query a single metric's trace for a given UTC calendar date.
    ///
    /// Returns one [`TracePoint`] per stored minute bucket where the metric is
    /// not `NULL`. `x` = seconds elapsed since midnight UTC on `date`;
    /// `y` = the metric's stored average value (or watts for Solar/Load).
    ///
    /// The blocking rusqlite call is offloaded via `spawn_blocking`.
    ///
    /// # Errors
    ///
    /// Returns an error if the database query fails or the worker thread panics.
    pub async fn query_comparison(
        &self,
        date: NaiveDate,
        metric: Metric,
    ) -> anyhow::Result<Vec<TracePoint>> {
        let conn = Arc::clone(&self.conn);
        spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|_e| anyhow::anyhow!("db mutex poisoned"))?;
            comparison_impl(&guard, date, metric)
        })
        .await?
    }
}

// ---------------------------------------------------------------------------
// Pure synchronous helpers (pub(crate) so tests in this module can use them)
// ---------------------------------------------------------------------------

/// Convert an `Option<f64>` weighted-average back to `Option<u16>` for use with
/// [`meteo_chart::power_w`].
///
/// The value is rounded to the nearest integer and clamped to `[0, u16::MAX]`
/// before the cast so both `cast_possible_truncation` and `cast_sign_loss` are
/// guarded.
#[expect(
    clippy::single_option_map,
    reason = "named helper consolidates the clamped-cast and the #[expect] attribute at one site"
)]
fn f64_to_u16_opt(v: Option<f64>) -> Option<u16> {
    v.map(|x| {
        // Round first to eliminate any sub-integer residue, then clamp to [0,
        // 65535.0] so the subsequent `as u16` is always in-range.
        let clamped = x.round().clamp(0.0, f64::from(u16::MAX));
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "value is rounded then clamped to [0.0, u16::MAX] before cast; no truncation or sign loss possible"
        )]
        {
            clamped as u16
        }
    })
}

/// Persist a [`BucketRow`] into the `samples` table (synchronous body).
///
/// Called by [`DbHandle::store_bucket`] (wrapped in `spawn_blocking`) and
/// directly by tests.
///
/// # Errors
///
/// Returns an error if the INSERT fails.
pub(crate) fn store_bucket_impl(conn: &Connection, row: &BucketRow) -> anyhow::Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO samples (
            bucket_ts,
            temp_min,     temp_max,     temp_avg,
            pressure_min, pressure_max, pressure_avg,
            humidity_min, humidity_max, humidity_avg,
            sky_min,      sky_max,      sky_avg,
            lux_min,      lux_max,      lux_avg,
            wind_min,     wind_max,     wind_avg,
            wind_dir_avg,
            rain_avg,     rain_max,
            battery_avg,
            solar_mv_avg, solar_ma_avg, batt_mv_avg, load_ma_avg,
            sample_count
        ) VALUES (
            ?1,
            ?2,  ?3,  ?4,
            ?5,  ?6,  ?7,
            ?8,  ?9,  ?10,
            ?11, ?12, ?13,
            ?14, ?15, ?16,
            ?17, ?18, ?19,
            ?20,
            ?21, ?22,
            ?23,
            ?24, ?25, ?26, ?27,
            ?28
        )",
        params![
            row.bucket_ts,
            row.temp_min,
            row.temp_max,
            row.temp_avg,
            row.pressure_min,
            row.pressure_max,
            row.pressure_avg,
            row.humidity_min,
            row.humidity_max,
            row.humidity_avg,
            row.sky_min,
            row.sky_max,
            row.sky_avg,
            row.lux_min,
            row.lux_max,
            row.lux_avg,
            row.wind_min,
            row.wind_max,
            row.wind_avg,
            row.wind_dir_avg,
            row.rain_avg,
            row.rain_max,
            row.battery_avg,
            row.solar_mv_avg,
            row.solar_ma_avg,
            row.batt_mv_avg,
            row.load_ma_avg,
            row.sample_count,
        ],
    )
    .context("INSERT OR REPLACE into samples")?;
    Ok(())
}

/// Query and re-aggregate history buckets (synchronous body).
///
/// Re-buckets stored per-minute rows into `q.bucket_secs`-second windows using
/// integer division `bucket_ts / q.bucket_secs`. Within each window:
/// - `MIN` / `MAX` collapse scalar fields as expected.
/// - Averages are re-combined as a **sample-count-weighted** mean:
///   `SUM(field_avg * sample_count) / SUM(sample_count for non-NULL rows)`.
///   Rows where the field is NULL contribute nothing to either sum.
/// - Power watts are computed in Rust after fetching raw mv/ma averages.
///
/// Called by [`DbHandle::query_history`] (wrapped in `spawn_blocking`) and
/// directly by tests.
///
/// # Errors
///
/// Returns an error if the SQL statement fails to prepare or execute.
#[expect(
    clippy::too_many_lines,
    reason = "the body mirrors the flat schema column layout; splitting would obscure field correspondence"
)]
#[expect(
    clippy::similar_names,
    reason = "solar_mv_avg / solar_ma_avg and batt_mv_avg / load_ma_avg differ only by the domain suffix"
)]
pub(crate) fn history_impl(conn: &Connection, q: &HistoryQuery) -> anyhow::Result<Vec<HistoryRow>> {
    // Each weighted-avg expression uses the CASE to exclude NULL rows from the
    // denominator.  When all rows in a window have NULL for a field, both the
    // numerator (SUM of NULLs = NULL in SQLite) and denominator (SUM of 0s = 0)
    // produce NULL / 0 = NULL — the correct sentinel for "no data".
    const SQL: &str = "
SELECT
    MIN(bucket_ts) AS ts,
    MIN(temp_min)  AS temp_min,
    MAX(temp_max)  AS temp_max,
    SUM(temp_avg * sample_count)
        / SUM(CASE WHEN temp_avg IS NOT NULL THEN sample_count ELSE 0 END)
        AS temp_avg,
    MIN(pressure_min) AS pressure_min,
    MAX(pressure_max) AS pressure_max,
    SUM(pressure_avg * sample_count)
        / SUM(CASE WHEN pressure_avg IS NOT NULL THEN sample_count ELSE 0 END)
        AS pressure_avg,
    MIN(humidity_min) AS humidity_min,
    MAX(humidity_max) AS humidity_max,
    SUM(humidity_avg * sample_count)
        / SUM(CASE WHEN humidity_avg IS NOT NULL THEN sample_count ELSE 0 END)
        AS humidity_avg,
    MIN(sky_min) AS sky_min,
    MAX(sky_max) AS sky_max,
    SUM(sky_avg * sample_count)
        / SUM(CASE WHEN sky_avg IS NOT NULL THEN sample_count ELSE 0 END)
        AS sky_avg,
    MIN(lux_min) AS lux_min,
    MAX(lux_max) AS lux_max,
    SUM(lux_avg * sample_count)
        / SUM(CASE WHEN lux_avg IS NOT NULL THEN sample_count ELSE 0 END)
        AS lux_avg,
    MIN(wind_min) AS wind_min,
    MAX(wind_max) AS wind_max,
    SUM(wind_avg * sample_count)
        / SUM(CASE WHEN wind_avg IS NOT NULL THEN sample_count ELSE 0 END)
        AS wind_avg,
    SUM(wind_dir_avg * sample_count)
        / SUM(CASE WHEN wind_dir_avg IS NOT NULL THEN sample_count ELSE 0 END)
        AS wind_dir_avg,
    SUM(rain_avg * sample_count)
        / SUM(CASE WHEN rain_avg IS NOT NULL THEN sample_count ELSE 0 END)
        AS rain_avg,
    MAX(rain_max) AS rain_max,
    SUM(battery_avg * sample_count)
        / SUM(CASE WHEN battery_avg IS NOT NULL THEN sample_count ELSE 0 END)
        AS battery_avg,
    SUM(solar_mv_avg * sample_count)
        / SUM(CASE WHEN solar_mv_avg IS NOT NULL THEN sample_count ELSE 0 END)
        AS solar_mv_avg,
    SUM(solar_ma_avg * sample_count)
        / SUM(CASE WHEN solar_ma_avg IS NOT NULL THEN sample_count ELSE 0 END)
        AS solar_ma_avg,
    SUM(batt_mv_avg * sample_count)
        / SUM(CASE WHEN batt_mv_avg IS NOT NULL THEN sample_count ELSE 0 END)
        AS batt_mv_avg,
    SUM(load_ma_avg * sample_count)
        / SUM(CASE WHEN load_ma_avg IS NOT NULL THEN sample_count ELSE 0 END)
        AS load_ma_avg
FROM samples
WHERE bucket_ts >= ?1 AND bucket_ts < ?2
GROUP BY bucket_ts / ?3
ORDER BY ts
";
    let mut stmt = conn.prepare(SQL).context("prepare history query")?;

    stmt.query_map(params![q.from_ts, q.to_ts, q.bucket_secs], |row| {
        // Column indices mirror the SELECT clause above (0-indexed).
        let ts: i64 = row.get(0)?;
        let temp_min: Option<f64> = row.get(1)?;
        let temp_max: Option<f64> = row.get(2)?;
        let temp_avg: Option<f64> = row.get(3)?;
        let pressure_min: Option<f64> = row.get(4)?;
        let pressure_max: Option<f64> = row.get(5)?;
        let pressure_avg: Option<f64> = row.get(6)?;
        let humidity_min: Option<f64> = row.get(7)?;
        let humidity_max: Option<f64> = row.get(8)?;
        let humidity_avg: Option<f64> = row.get(9)?;
        let sky_min: Option<f64> = row.get(10)?;
        let sky_max: Option<f64> = row.get(11)?;
        let sky_avg: Option<f64> = row.get(12)?;
        let lux_min: Option<f64> = row.get(13)?;
        let lux_max: Option<f64> = row.get(14)?;
        let lux_avg: Option<f64> = row.get(15)?;
        let wind_min: Option<f64> = row.get(16)?;
        let wind_max: Option<f64> = row.get(17)?;
        let wind_avg: Option<f64> = row.get(18)?;
        let wind_dir_avg: Option<f64> = row.get(19)?;
        let rain_avg: Option<f64> = row.get(20)?;
        let rain_max: Option<f64> = row.get(21)?;
        let battery_avg: Option<f64> = row.get(22)?;
        let solar_mv_avg: Option<f64> = row.get(23)?;
        let solar_ma_avg: Option<f64> = row.get(24)?;
        let batt_mv_avg: Option<f64> = row.get(25)?;
        let load_ma_avg: Option<f64> = row.get(26)?;

        Ok(HistoryRow {
            ts,
            temp: MetricStat {
                min: temp_min,
                max: temp_max,
                avg: temp_avg,
            },
            pressure: MetricStat {
                min: pressure_min,
                max: pressure_max,
                avg: pressure_avg,
            },
            humidity: MetricStat {
                min: humidity_min,
                max: humidity_max,
                avg: humidity_avg,
            },
            sky: MetricStat {
                min: sky_min,
                max: sky_max,
                avg: sky_avg,
            },
            lux: MetricStat {
                min: lux_min,
                max: lux_max,
                avg: lux_avg,
            },
            wind: MetricStat {
                min: wind_min,
                max: wind_max,
                avg: wind_avg,
            },
            wind_dir_avg,
            rain: MetricStat {
                min: None, // rain_min is not stored in the schema (min unused)
                max: rain_max,
                avg: rain_avg,
            },
            battery_avg,
            solar_w_avg: power_w(f64_to_u16_opt(solar_mv_avg), f64_to_u16_opt(solar_ma_avg)),
            load_w_avg: power_w(f64_to_u16_opt(batt_mv_avg), f64_to_u16_opt(load_ma_avg)),
        })
    })
    .context("execute history query")?
    .map(|r| r.context("read history row"))
    .collect()
}

/// Query one metric's trace for a UTC calendar date (synchronous body).
///
/// Dispatches to [`comparison_single`] for scalar metrics or
/// [`comparison_power`] for Solar/Load (which require two columns to compute
/// watts).
///
/// Called by [`DbHandle::query_comparison`] (wrapped in `spawn_blocking`) and
/// directly by tests.
///
/// # Errors
///
/// Returns an error if the underlying query fails.
pub(crate) fn comparison_impl(
    conn: &Connection,
    date: NaiveDate,
    metric: Metric,
) -> anyhow::Result<Vec<TracePoint>> {
    let midnight = date
        .and_hms_opt(0, 0, 0)
        .context("invalid midnight from NaiveDate")?;
    let day_start = midnight.and_utc().timestamp();
    let day_end = day_start.saturating_add(86_400_i64);

    match metric {
        Metric::Solar => comparison_power(conn, day_start, day_end, "solar_mv_avg", "solar_ma_avg"),
        Metric::Load => comparison_power(conn, day_start, day_end, "batt_mv_avg", "load_ma_avg"),
        Metric::AirTemp => comparison_single(conn, day_start, day_end, "temp_avg"),
        Metric::Pressure => comparison_single(conn, day_start, day_end, "pressure_avg"),
        Metric::Humidity => comparison_single(conn, day_start, day_end, "humidity_avg"),
        Metric::SkyTemp => comparison_single(conn, day_start, day_end, "sky_avg"),
        Metric::Lux => comparison_single(conn, day_start, day_end, "lux_avg"),
        Metric::Wind => comparison_single(conn, day_start, day_end, "wind_avg"),
        Metric::Rain => comparison_single(conn, day_start, day_end, "rain_avg"),
        Metric::Battery => comparison_single(conn, day_start, day_end, "battery_avg"),
    }
}

// ---------------------------------------------------------------------------
// Private comparison helpers
// ---------------------------------------------------------------------------

/// Convert `ts - day_start` (seconds elapsed in day) to `f64`.
///
/// Values are bounded to `[0, 86400)` seconds; `i32::try_from` is lossless for
/// this range and `f64::from(i32)` is lossless, avoiding `cast_precision_loss`.
/// `saturating_sub` avoids the `arithmetic_side_effects` lint for overflow.
fn secs_in_day_f64(ts: i64, day_start: i64) -> f64 {
    let diff = ts.saturating_sub(day_start);
    // diff ∈ [0, 86400); i32 holds it exactly; f64::from(i32) is exact.
    f64::from(i32::try_from(diff).unwrap_or(86_400_i32))
}

/// Query a single scalar metric column from the `samples` table.
///
/// Rows where the column is NULL are excluded (the `AND col IS NOT NULL`
/// predicate ensures `y` is always a valid `f64`).
///
/// `col` is always one of a fixed set of column names resolved from the
/// [`Metric`] enum; it is never user-controlled input.
fn comparison_single(
    conn: &Connection,
    day_start: i64,
    day_end: i64,
    col: &str,
) -> anyhow::Result<Vec<TracePoint>> {
    let sql = format!(
        "SELECT bucket_ts, {col} \
         FROM samples \
         WHERE bucket_ts >= ?1 AND bucket_ts < ?2 AND {col} IS NOT NULL \
         ORDER BY bucket_ts"
    );
    let mut stmt = conn
        .prepare(&sql)
        .context("prepare comparison single query")?;
    stmt.query_map(params![day_start, day_end], |row| {
        let ts: i64 = row.get(0)?;
        let y: f64 = row.get(1)?;
        Ok((ts, y))
    })
    .context("execute comparison single query")?
    .map(|r| -> anyhow::Result<TracePoint> {
        let (ts, y) = r.context("read comparison single row")?;
        Ok(TracePoint {
            x: secs_in_day_f64(ts, day_start),
            y,
        })
    })
    .collect()
}

/// Query two millivolt/milliampere columns, compute watts, and build a trace.
///
/// Rows where `power_w` returns `None` (either column is NULL) are silently
/// skipped; rusqlite errors are propagated as `Err`.
///
/// `mv_col` / `ma_col` are always hardcoded column names from the [`Metric`]
/// match — never user-controlled input.
#[expect(
    clippy::similar_names,
    reason = "mv_col / ma_col differ only by the domain suffix; renaming would obscure the millivolt / milliamp distinction"
)]
fn comparison_power(
    conn: &Connection,
    day_start: i64,
    day_end: i64,
    mv_col: &str,
    ma_col: &str,
) -> anyhow::Result<Vec<TracePoint>> {
    let sql = format!(
        "SELECT bucket_ts, {mv_col}, {ma_col} \
         FROM samples \
         WHERE bucket_ts >= ?1 AND bucket_ts < ?2 \
         ORDER BY bucket_ts"
    );
    let mut stmt = conn
        .prepare(&sql)
        .context("prepare comparison power query")?;
    stmt.query_map(params![day_start, day_end], |row| {
        let ts: i64 = row.get(0)?;
        let mv: Option<f64> = row.get(1)?;
        let ma: Option<f64> = row.get(2)?;
        Ok((ts, mv, ma))
    })
    .context("execute comparison power query")?
    .filter_map(|r| match r.context("read comparison power row") {
        Err(e) => Some(Err(e)),
        Ok((ts, mv, ma)) => power_w(f64_to_u16_opt(mv), f64_to_u16_opt(ma)).map(|w| {
            Ok(TracePoint {
                x: secs_in_day_f64(ts, day_start),
                y: w,
            })
        }),
    })
    .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(test)]
mod tests {
    use core::{error, result};

    use test_log::test;

    use super::{BucketRow, HistoryQuery, comparison_impl, history_impl, store_bucket_impl};
    use crate::types::Metric;

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    /// Open an in-memory SQLite database with the schema applied.
    fn make_db() -> anyhow::Result<rusqlite::Connection> {
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch(include_str!("schema.sql"))?;
        Ok(conn)
    }

    /// Return a [`BucketRow`] with all optional fields set to `None` and the
    /// given `bucket_ts` and `sample_count = 1`.
    fn empty_bucket(bucket_ts: i64) -> BucketRow {
        BucketRow {
            bucket_ts,
            temp_min: None,
            temp_max: None,
            temp_avg: None,
            pressure_min: None,
            pressure_max: None,
            pressure_avg: None,
            humidity_min: None,
            humidity_max: None,
            humidity_avg: None,
            sky_min: None,
            sky_max: None,
            sky_avg: None,
            lux_min: None,
            lux_max: None,
            lux_avg: None,
            wind_min: None,
            wind_max: None,
            wind_avg: None,
            wind_dir_avg: None,
            rain_avg: None,
            rain_max: None,
            battery_avg: None,
            solar_mv_avg: None,
            solar_ma_avg: None,
            batt_mv_avg: None,
            load_ma_avg: None,
            sample_count: 1,
        }
    }

    // -----------------------------------------------------------------------
    // Required tests (from substep spec)
    // -----------------------------------------------------------------------

    #[test]
    #[expect(clippy::unwrap_used, reason = "test: values asserted to be Some")]
    fn store_then_query_roundtrips_one_bucket() -> TestResult {
        // Given — one bucket with known temperature values
        let conn = make_db()?;
        let row = BucketRow {
            bucket_ts: 1_000,
            temp_min: Some(18.0),
            temp_max: Some(22.0),
            temp_avg: Some(20.0),
            sample_count: 5,
            ..empty_bucket(1_000)
        };

        // When — store and then query
        store_bucket_impl(&conn, &row)?;
        let q = HistoryQuery {
            from_ts: 0,
            to_ts: 2_000,
            bucket_secs: 60,
        };
        let result = history_impl(&conn, &q)?;

        // Then — exactly one row with the stored values
        assert_eq!(result.len(), 1);
        let hr = &result[0];
        assert_eq!(hr.ts, 1_000);
        let avg = hr.temp.avg.unwrap();
        assert!(
            (avg - 20.0).abs() < 1e-9,
            "temp.avg should be 20.0, got {avg}"
        );
        let min = hr.temp.min.unwrap();
        assert!(
            (min - 18.0).abs() < 1e-9,
            "temp.min should be 18.0, got {min}"
        );
        let max = hr.temp.max.unwrap();
        assert!(
            (max - 22.0).abs() < 1e-9,
            "temp.max should be 22.0, got {max}"
        );

        Ok(())
    }

    #[test]
    #[expect(clippy::unwrap_used, reason = "test: values asserted to be Some")]
    fn query_history_reaggregates_to_coarser_buckets() -> TestResult {
        // Given — 10 one-minute rows; row 0 carries temp_max = 100
        let conn = make_db()?;
        for i in 0_i64..10 {
            #[expect(
                clippy::cast_precision_loss,
                reason = "i ∈ [0,9], exactly representable as f64"
            )]
            let v = i as f64;
            let row = BucketRow {
                bucket_ts: i * 60,
                temp_min: Some(v),
                temp_max: if i == 0 { Some(100.0) } else { Some(v) },
                temp_avg: Some(v),
                sample_count: 1,
                ..empty_bucket(i * 60)
            };
            store_bucket_impl(&conn, &row)?;
        }

        // When — query with bucket_secs = 600 (10-minute window)
        let q = HistoryQuery {
            from_ts: 0,
            to_ts: 600,
            bucket_secs: 600,
        };
        let result = history_impl(&conn, &q)?;

        // Then — exactly one re-aggregated row
        assert_eq!(result.len(), 1, "expected 1 coarse bucket");
        let hr = &result[0];

        // temp.max = MAX(100, 1, 2, …, 9) = 100
        let max = hr.temp.max.unwrap();
        assert!(
            (max - 100.0).abs() < 1e-9,
            "temp.max should be 100.0, got {max}"
        );

        // Weighted avg: each sample_count = 1, avgs = 0..9 → (0+1+…+9)/10 = 4.5
        let avg = hr.temp.avg.unwrap();
        assert!(
            (avg - 4.5).abs() < 1e-9,
            "temp.avg should be 4.5, got {avg}"
        );

        Ok(())
    }

    #[test]
    #[expect(clippy::unwrap_used, reason = "test: value asserted to be Some")]
    fn query_history_computes_power_watts() -> TestResult {
        // Given — solar: 5000 mV × 200 mA = 5 V × 0.2 A = 1.0 W
        let conn = make_db()?;
        let row = BucketRow {
            bucket_ts: 0,
            solar_mv_avg: Some(5_000.0),
            solar_ma_avg: Some(200.0),
            sample_count: 1,
            ..empty_bucket(0)
        };
        store_bucket_impl(&conn, &row)?;

        // When
        let q = HistoryQuery {
            from_ts: 0,
            to_ts: 100,
            bucket_secs: 60,
        };
        let result = history_impl(&conn, &q)?;

        // Then
        assert_eq!(result.len(), 1);
        let w = result[0].solar_w_avg.unwrap();
        assert!(
            (w - 1.0).abs() < 1e-9,
            "solar_w_avg should be 1.0 W (5 V × 0.2 A), got {w}"
        );

        Ok(())
    }

    #[test]
    fn query_history_empty_range_returns_empty() -> TestResult {
        // Given — empty database
        let conn = make_db()?;

        // When — query a non-empty time range that has no rows
        let q = HistoryQuery {
            from_ts: 0,
            to_ts: 100,
            bucket_secs: 60,
        };
        let result = history_impl(&conn, &q)?;

        // Then
        assert!(
            result.is_empty(),
            "expected empty result, got {}",
            result.len()
        );

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Additional coverage tests
    // -----------------------------------------------------------------------

    #[test]
    #[expect(clippy::unwrap_used, reason = "test: value asserted to be Some")]
    fn comparison_impl_returns_trace_for_air_temp() -> TestResult {
        // Given — two buckets on 2024-01-15 (unix day start = 1705276800)
        let day_start: i64 = 1_705_276_800; // 2024-01-15 00:00:00 UTC
        let conn = make_db()?;
        let row1 = BucketRow {
            bucket_ts: day_start + 60,
            temp_avg: Some(10.0),
            sample_count: 1,
            ..empty_bucket(day_start + 60)
        };
        let row2 = BucketRow {
            bucket_ts: day_start + 120,
            temp_avg: Some(12.0),
            sample_count: 1,
            ..empty_bucket(day_start + 120)
        };
        store_bucket_impl(&conn, &row1)?;
        store_bucket_impl(&conn, &row2)?;

        // When
        use chrono::NaiveDate;
        let date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let pts = comparison_impl(&conn, date, Metric::AirTemp)?;

        // Then — two points with correct x values
        assert_eq!(pts.len(), 2);
        assert!(
            (pts[0].x - 60.0).abs() < 1e-9,
            "x[0] should be 60 s, got {}",
            pts[0].x
        );
        assert!(
            (pts[0].y - 10.0).abs() < 1e-9,
            "y[0] should be 10.0 °C, got {}",
            pts[0].y
        );
        assert!(
            (pts[1].x - 120.0).abs() < 1e-9,
            "x[1] should be 120 s, got {}",
            pts[1].x
        );

        Ok(())
    }

    #[test]
    #[expect(clippy::unwrap_used, reason = "test: value asserted to be Some")]
    fn comparison_impl_solar_computes_watts() -> TestResult {
        // Given — one bucket: 10 V × 0.5 A = 5.0 W
        let day_start: i64 = 1_705_276_800; // 2024-01-15 00:00:00 UTC
        let conn = make_db()?;
        let row = BucketRow {
            bucket_ts: day_start + 300,
            solar_mv_avg: Some(10_000.0),
            solar_ma_avg: Some(500.0),
            sample_count: 1,
            ..empty_bucket(day_start + 300)
        };
        store_bucket_impl(&conn, &row)?;

        // When
        use chrono::NaiveDate;
        let date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();
        let pts = comparison_impl(&conn, date, Metric::Solar)?;

        // Then — one trace point with 5.0 W
        assert_eq!(pts.len(), 1);
        let w = pts[0].y;
        assert!(
            (w - 5.0).abs() < 1e-9,
            "solar trace point should be 5.0 W, got {w}"
        );

        Ok(())
    }
}
// grcov exclude stop
