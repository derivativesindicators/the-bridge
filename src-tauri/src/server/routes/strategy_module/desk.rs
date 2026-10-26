//! The Bridge 1.0.10: the Strategy desk routes (every strategy's open
//! positions and today's P&L in one mode, and the margin they block).

use super::{err, read_failed, Ctx, Q};
use crate::server::envelope::json_response;
use crate::server::middleware::User;
use crate::state::AppState;
use crate::strategy::desk::{margin_positions, DeskStrategy};
use crate::strategy::dispatch::RunMode;
use axum::{extract::{Query, State}, http::StatusCode, response::Response};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

/// Most legs priced one by one for the "sum of individual margins" figure.
const MAX_SINGLE_LEGS: usize = 30;
/// Most strategies priced as their own basket.
const MAX_STRATEGY_BASKETS: usize = 20;

fn mode_of(q: &HashMap<String, String>) -> Result<RunMode, Box<Response>> {
    match q.get("mode").map(String::as_str).unwrap_or("sandbox") {
        "" => Ok(RunMode::Sandbox),
        m => RunMode::parse(m).ok_or_else(|| {
            Box::new(err("mode must be sandbox or live", StatusCode::BAD_REQUEST))
        }),
    }
}

async fn desk_payload(ctx: &Arc<AppState>, user: &str, mode: RunMode) -> Result<Value, Response> {
    let u = user.to_string();
    let listed = ctx
        .blocking(move |c| c.strategy.store.list_strategies(&u, None, None))
        .await
        .map_err(|e| read_failed(format_args!("the strategy list"), e))?;
    let strategies = listed
        .iter()
        .filter_map(|s| {
            Some(DeskStrategy {
                id: s["id"].as_i64()?,
                name: s["name"].as_str().unwrap_or("").to_string(),
                status: s["status"].as_str().unwrap_or("").to_string(),
                product: s["product"].as_str().unwrap_or("").to_string(),
                run_mode: s["current_run_id"]
                    .as_i64()
                    .and_then(|rid| ctx.strategy.store.get_run(rid).ok().flatten())
                    .filter(|r| r.stopped_at.is_none())
                    .map(|r| r.mode),
            })
        })
        .collect();
    let any_live = listed.iter().any(|s| s["live_enabled"].as_bool() == Some(true) || s["live_enabled"].as_i64() == Some(1));
    let mut payload = ctx
        .strategy
        .desk(strategies, mode)
        .await
        .map_err(|e| read_failed(format_args!("the strategy desk"), e))?;
    payload["data"]["any_live_enabled"] = json!(any_live);
    Ok(payload)
}

/// GET /strategy/api/desk?mode=sandbox|live
pub async fn desk(State(ctx): Ctx, User(u): User, Query(q): Q) -> Response {
    let mode = match mode_of(&q) {
        Ok(m) => m,
        Err(r) => return *r,
    };
    match desk_payload(&ctx, &u.username, mode).await {
        Ok(p) => json_response(StatusCode::OK, p),
        Err(r) => r,
    }
}

/// The broker's margin for `rows`: `Ok(total)` or `Err(message)`.
async fn margin_of(ctx: &AppState, rows: &[Value]) -> Result<f64, String> {
    let r = crate::services::market_data_service::margin(ctx, rows).await;
    if r.status == 200 && r.body["status"] == "success" {
        Ok(r.body["data"]["total_margin_required"].as_f64().unwrap_or(0.0))
    } else {
        Err(r.body["message"].as_str().unwrap_or("Margin is not available").to_string())
    }
}

/// Every open position of the desk once: a contract shared by strategies is
/// one broker row repeated under each, so it is counted once.
fn unique_open(strategies: &[Value]) -> Vec<Value> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for s in strategies {
        for p in s["positions"].as_array().into_iter().flatten() {
            if p["source"] == "broker/shared" {
                let key = format!("{}|{}|{}", p["symbol"], p["exchange"], p["product"]);
                if !seen.insert(key) {
                    continue;
                }
            }
            out.push(p.clone());
        }
    }
    out
}

/// GET /strategy/api/desk/margin?mode=sandbox|live
///
/// The combined basket margin of every open position (hedges get the lower
/// combined figure), the sum of each leg's own margin, and each strategy's
/// own basket. Always the broker's calculator, so in Sandbox it is an
/// estimate of what the same positions would block live.
pub async fn desk_margin(State(ctx): Ctx, User(u): User, Query(q): Q) -> Response {
    let mode = match mode_of(&q) {
        Ok(m) => m,
        Err(r) => return *r,
    };
    let payload = match desk_payload(&ctx, &u.username, mode).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let strategies = payload["data"]["strategies"].as_array().cloned().unwrap_or_default();
    let all = margin_positions(&unique_open(&strategies));
    let estimate = mode == RunMode::Sandbox;
    if all.is_empty() {
        return json_response(StatusCode::OK, json!({"status": "success", "data": {
            "mode": mode.as_str(), "estimate": estimate, "available": true,
            "combined": 0.0, "sum_of_legs": 0.0, "legs": 0, "per_strategy": {},
        }}));
    }
    let combined = margin_of(&ctx, &all).await;
    let (combined, message) = match combined {
        Ok(v) => (Some(v), None),
        Err(m) => (None, Some(m)),
    };
    let mut sum_of_legs = None;
    let mut per_strategy = serde_json::Map::new();
    if combined.is_some() {
        if all.len() <= MAX_SINGLE_LEGS {
            let mut sum = 0.0;
            let mut ok = true;
            for leg in &all {
                match margin_of(&ctx, std::slice::from_ref(leg)).await {
                    Ok(v) => sum += v,
                    Err(_) => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                sum_of_legs = Some(sum);
            }
        }
        for s in strategies.iter().take(MAX_STRATEGY_BASKETS) {
            let rows = margin_positions(s["positions"].as_array().map(Vec::as_slice).unwrap_or(&[]));
            if rows.is_empty() {
                continue;
            }
            if let Ok(v) = margin_of(&ctx, &rows).await {
                per_strategy.insert(s["id"].to_string(), json!(v));
            }
        }
    }
    json_response(StatusCode::OK, json!({"status": "success", "data": {
        "mode": mode.as_str(), "estimate": estimate,
        "available": combined.is_some(), "message": message,
        "combined": combined, "sum_of_legs": sum_of_legs, "legs": all.len(),
        "per_strategy": per_strategy,
    }}))
}
