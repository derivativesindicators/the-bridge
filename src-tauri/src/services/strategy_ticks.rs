//! The strategy tick path's health. A tick the reader misses because the
//! market-data channel overran it is a price strategy risk management never
//! saw. The web counts its shed ticks only as a diagnostic; here the counts
//! are on the Health page (`/health/api/current`, `strategy_ticks`) and in
//! the system report, and a new miss raises a health alert (OBS-01 style).

use crate::db::sqlite::monitor::{self as store, AlertRow};
use crate::error::Result;
use crate::state::AppState;
use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::Value;

/// The alert a missed tick raises; the trader resolves it.
pub const ALERT: &str = "strategy_ticks_missed";

/// Prices read, decisions combined and ticks missed since start, and the
/// strategies whose exits wait to be acted on.
pub fn json(ctx: &AppState) -> Value {
    ctx.strategy.feed.health(ctx.strategy.clock.now())
}

/// Raise the alert when more ticks were missed since the last one.
pub fn alert(conn: &Connection, ctx: &AppState, now: DateTime<Utc>) -> Result<()> {
    let Some(missed) = ctx.strategy.feed.ticks_missed_unreported() else {
        return Ok(());
    };
    store::raise_alert(
        conn,
        &AlertRow {
            alert_type: ALERT.into(),
            severity: "warn".into(),
            metric_name: ALERT.into(),
            metric_value: missed as f64,
            threshold_value: 0.0,
            message: format!(
                "Strategy risk management missed {} prices since The Bridge started: the market data came faster than it was read. Stops and targets act on the next price. If this keeps happening, hold fewer symbols or restart The Bridge, and report it.",
                missed
            ),
            ..Default::default()
        },
        now,
    )
}
