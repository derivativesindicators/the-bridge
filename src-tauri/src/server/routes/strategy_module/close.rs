//! The Bridge 1.0.11: "Close positions" (one strategy) and "Close all open
//! positions" (every strategy, sandbox and live) on Strategy Positions.
//!
//! Neither stops or disables anything: the strategy keeps running and keeps
//! taking signals. Each open leg exits at MARKET for its OWN recorded
//! quantity through the engine's per-leg exit path
//! (`StrategyModule::close_open_legs` -> `engine/exits.rs`), in the run's own
//! mode (strategy sandbox book or broker). Nothing here reads or closes the
//! broker's net position, so a contract another strategy also holds is never
//! touched beyond this strategy's own quantity.

use super::{record, resolve, store_error, Ctx};
use crate::server::envelope::json_response;
use crate::server::middleware::User;
use crate::state::AppState;
use crate::strategy::store::{EventFields, StrategyRow};
use axum::{extract::{Path, State}, http::StatusCode, response::Response};
use serde_json::json;
use serde_json::Value;

/// Close one strategy's open legs; the strategy stays running.
pub(super) async fn close_positions_row(ctx: &AppState, user: &str, row: &StrategyRow) -> Value {
    let Some(run_id) = row.current_run_id else {
        return json!({"id": row.id, "name": row.name, "ok": true, "closed": 0, "exits": [],
            "message": "Not running: no open positions to close"});
    };
    let mode = ctx.strategy.store.get_run(run_id).ok().flatten().map(|r| r.mode);
    let r = ctx.strategy.close_open_legs(run_id, user).await;
    if !r.ok && r.exits.is_empty() {
        // Refused before any exit (no session, not active): say so in the log.
        record(ctx, row.id, user, "manual_close",
            &format!("Close positions refused: {}", r.error.clone().unwrap_or_default()),
            EventFields { run_id: Some(run_id), severity: Some("warn"), ..Default::default() }).await;
    }
    let closed = r.exits.iter().filter(|e| e["ok"] == true).count();
    json!({
        "id": row.id, "name": row.name, "mode": mode, "ok": r.ok, "closed": closed,
        "exits": r.exits, "error": r.error, "still_running": true,
        "message": if r.ok {
            if closed == 0 { "No open positions".to_string() }
            else { format!("Closed {} position(s); the strategy keeps running", closed) }
        } else { r.error.clone().unwrap_or_else(|| "Close refused".into()) },
    })
}

/// POST /strategy/api/strategies/{sid}/close_positions
pub async fn close_positions(State(ctx): Ctx, User(u): User, Path(sid): Path<String>) -> Response {
    let row = match resolve(&ctx, &u.username, &sid) {
        Ok(r) => r,
        Err(r) => return *r,
    };
    let result = close_positions_row(&ctx, &u.username, &row).await;
    let code = if result["ok"] == true { StatusCode::OK } else { StatusCode::CONFLICT };
    let mut body = json!({"status": if code == StatusCode::OK { "success" } else { "error" }});
    if let (Some(b), Some(r)) = (body.as_object_mut(), result.as_object()) {
        b.extend(r.clone());
    }
    json_response(code, body)
}

/// The strategies "Close all" acts on: every one with a current run, of
/// either mode. `rows` is the strategy list.
pub fn close_all_targets(rows: &[Value]) -> Vec<i64> {
    rows.iter()
        .filter(|s| s["current_run_id"].as_i64().is_some())
        .filter_map(|s| s["id"].as_i64())
        .collect()
}

/// POST /strategy/api/desk/close_all: close every open position of every
/// running strategy, sandbox and live alike; every strategy keeps running.
/// One result per strategy that held something (or failed).
pub async fn desk_close_all(State(ctx): Ctx, User(u): User) -> Response {
    let rows = match ctx.strategy.store.list_strategies(&u.username, None, None) {
        Ok(r) => r,
        Err(e) => return store_error(e),
    };
    let mut results = Vec::new();
    for id in close_all_targets(&rows) {
        let r = match resolve(&ctx, &u.username, &id.to_string()) {
            Ok(row) => close_positions_row(&ctx, &u.username, &row).await,
            Err(_) => json!({"id": id, "ok": false, "error": "The strategy could not be read"}),
        };
        if r["ok"] != true || r["closed"].as_u64().unwrap_or(0) > 0 {
            results.push(r);
        }
    }
    let failed = results.iter().filter(|r| r["ok"] != true).count();
    json_response(StatusCode::OK, json!({
        "status": "success",
        "results": results,
        "closed_strategies": results.len() - failed,
        "failed": failed,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_all_targets_every_running_strategy_of_any_mode() {
        let rows = vec![
            json!({"id": 1, "current_run_id": 10}),
            json!({"id": 2, "current_run_id": null}),
            json!({"id": 3, "current_run_id": 11}),
        ];
        assert_eq!(close_all_targets(&rows), vec![1, 3]);
    }
}
