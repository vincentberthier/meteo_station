//! Leptos server functions and DTO re-exports for the `MeteoStation` web dashboard.
// The leptos `#[server]` macro generates async trait impls for client stubs
// (compiled under `hydrate`). Those impls have no `.await` expressions,
// triggering `unused_async_trait_impl`. This is unavoidable macro-generated
// code; suppress the lint at the module level.
#![allow(
    clippy::unused_async_trait_impl,
    reason = "leptos #[server] macro generates async trait impls for its client stubs"
)]
//!
//! ## Compilation model
//!
//! Leptos `#[server]` functions are compiled **twice**:
//! - under `ssr`: the full async body executes on the server (accesses the DB,
//!   uses [`crate::state::AppState`] from leptos context, etc.).
//! - under `hydrate` (wasm32): the macro generates a lightweight client stub
//!   that serialises arguments and calls the server endpoint over HTTP; the
//!   body is never compiled for the wasm target.
//!
//! All DTO types referenced in function signatures (`HistoryRow`, `Metric`, …)
//! are in [`crate::types`], which is unconditional and compiles for both targets.
//!
//! ## SSE live endpoint
//!
//! [`sse`] is gated on `ssr` because axum's SSE types are not available in the
//! wasm/hydrate bundle.

#[cfg(feature = "ssr")]
pub mod sse;

pub use crate::types::{HistoryRow, LiveFrame, Metric, MetricStat, TracePoint};

use leptos::prelude::*;

/// Return aggregated history buckets between `from_ts` (inclusive) and `to_ts`
/// (exclusive) unix seconds, re-bucketed into `bucket_secs`-second windows.
///
/// Power fields (`solar_w_avg`, `load_w_avg`) are already converted to watts.
///
/// # Errors
///
/// Returns a [`ServerFnError`] if the database query fails.
#[server]
pub async fn get_history(
    from_ts: i64,
    to_ts: i64,
    bucket_secs: i64,
) -> Result<Vec<HistoryRow>, ServerFnError> {
    use crate::db::HistoryQuery;
    use crate::state::AppState;

    let state = expect_context::<AppState>();
    let q = HistoryQuery {
        from_ts,
        to_ts,
        bucket_secs,
    };
    state
        .db
        .query_history(q)
        .await
        .map_err(|e| ServerFnError::ServerError(e.to_string()))
}

/// Return a single metric's trace for the given UTC calendar date.
///
/// `date` must be formatted as `YYYY-MM-DD`. Returns one [`TracePoint`] per
/// stored per-minute bucket where the metric is not `NULL`; `x` = seconds
/// elapsed since midnight UTC on `date`; `y` = the metric average (or watts
/// for `Solar`/`Load`).
///
/// # Errors
///
/// Returns a [`ServerFnError`] if `date` cannot be parsed as a UTC date or if
/// the database query fails.
#[server]
pub async fn get_comparison_trace(
    date: String,
    metric: Metric,
) -> Result<Vec<TracePoint>, ServerFnError> {
    use chrono::NaiveDate;

    use crate::state::AppState;

    let naive = NaiveDate::parse_from_str(&date, "%Y-%m-%d").map_err(|e| -> ServerFnError {
        ServerFnError::ServerError(format!("invalid date '{date}': {e}"))
    })?;

    let state = expect_context::<AppState>();
    state
        .db
        .query_comparison(naive, metric)
        .await
        .map_err(|e| ServerFnError::ServerError(e.to_string()))
}

// ---------------------------------------------------------------------------
// Tests (ssr only — DbHandle requires tokio + rusqlite)
// ---------------------------------------------------------------------------

// grcov exclude start
#[expect(clippy::panic_in_result_fn, reason = "test module")]
#[cfg(all(test, feature = "ssr"))]
mod tests {
    use core::{error, result};
    use std::path::Path;

    use test_log::test;

    use crate::db::{BucketRow, DbHandle, HistoryQuery};

    type TestResult = result::Result<(), Box<dyn error::Error>>;

    // Helper: build an all-None BucketRow at the given timestamp.
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

    /// Verify the async `DbHandle::query_history` path works end-to-end and
    /// converts power columns to watts.
    ///
    /// An equivalent test exists in `db::tests` that exercises `history_impl`
    /// directly; this one focuses on the async API path (`store_bucket` +
    /// `query_history`) used by the server fn body.
    #[test]
    fn get_history_smoke() -> TestResult {
        let rt = tokio::runtime::Runtime::new()?;

        // Given — a `:memory:` database with one bucket: 5 V × 0.2 A = 1.0 W solar
        let db = DbHandle::open(Path::new(":memory:"))?;
        let row = BucketRow {
            bucket_ts: 0,
            solar_mv_avg: Some(5_000.0),
            solar_ma_avg: Some(200.0),
            sample_count: 1,
            ..empty_bucket(0)
        };

        rt.block_on(db.store_bucket(row))?;

        // When — use the async query path
        let q = HistoryQuery {
            from_ts: 0,
            to_ts: 100,
            bucket_secs: 60,
        };
        let rows = rt.block_on(db.query_history(q))?;

        // Then — one row with solar_w_avg = 1.0 W (5 V × 0.2 A)
        assert_eq!(rows.len(), 1, "expected exactly one HistoryRow");
        let w = rows[0].solar_w_avg.ok_or("solar_w_avg should be Some")?;
        assert!(
            (w - 1.0).abs() < 1e-9,
            "solar_w_avg should be 1.0 W (5 V × 0.2 A), got {w}"
        );
        // load_w_avg is None — batt_mv_avg and load_ma_avg were not seeded.
        assert!(
            rows[0].load_w_avg.is_none(),
            "load_w_avg should be None when both inputs are None"
        );

        Ok(())
    }
}
// grcov exclude stop
