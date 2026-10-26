//! web: test/test_strategy_module_api.py, test_strategy_module_lifecycle_api.py

use super::*;
use axum::http::{Method, StatusCode};

fn create_body() -> Value {
    json!({
        "name": "Short straddle",
        "underlying": "NIFTY",
        "underlying_exchange": "NSE_INDEX",
        "strategy_type": "positional",
        "legs": [short_call_leg()],
    })
}

#[tokio::test]
async fn create_answers_201_and_shows_the_token_once() {
    let a = app();
    let (s, b) = a.post("/strategy/api/strategies", create_body()).await;
    assert_eq!(s, StatusCode::CREATED, "{}", b);
    assert_eq!(b["status"], "success");
    let token = b["webhook_token"].as_str().unwrap().to_string();
    assert!(token.starts_with("oaws_"));
    assert!(b["message"]
        .as_str()
        .unwrap()
        .contains("Copy the webhook token now"));
    let sid = b["data"]["id"].as_i64().unwrap();
    let (s, d) = a.get(&format!("/strategy/api/strategies/{}", sid)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(!d.to_string().contains(&token));
    let (_, list) = a.get("/strategy/api/strategies").await;
    assert_eq!(list["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn every_session_route_needs_a_signed_in_user() {
    let a = app();
    let mut r = a.req(Method::GET, "/strategy/api/strategies", None, false);
    r.headers_mut().remove(axum::http::header::COOKIE);
    let (s, b) = a.send(r).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(b["message"], "Not authenticated");
}

#[tokio::test]
async fn a_write_without_the_csrf_token_is_refused() {
    let a = app();
    let (s, _) = a
        .send(a.req(
            Method::POST,
            "/strategy/api/strategies",
            Some(create_body()),
            false,
        ))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "CSRF refusal");
    let mut r = a.req(
        Method::POST,
        "/strategy/api/strategies",
        Some(create_body()),
        true,
    );
    r.headers_mut()
        .insert("origin", "https://evil.example".parse().unwrap());
    let (s, _) = a.send(r).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_strategy_that_is_not_yours_answers_404_not_403() {
    let a = app();
    let (row, _) = a
        .ctx
        .strategy
        .store
        .create_strategy(
            "someone-else",
            &config("Theirs", json!([short_call_leg()]), json!({})),
        )
        .unwrap();
    for path in ["", "/runs", "/orders", "/events", "/checkpoints"] {
        let (s, b) = a
            .get(&format!("/strategy/api/strategies/{}{}", row.id, path))
            .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "{}", path);
        assert_eq!(b["message"], "Strategy not found");
    }
    let (s, _) = a.get("/strategy/api/strategies/999999").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn validation_errors_are_400_with_a_message() {
    let a = app();
    let mut body = create_body();
    body["legs"] = json!([]);
    let (s, b) = a.post("/strategy/api/strategies", body).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(
        b,
        json!({"status": "error", "message": "A strategy needs at least 1 leg"})
    );
}

#[tokio::test]
async fn a_patch_revalidates_the_merged_config_and_refuses_a_kind_change() {
    let a = app();
    let (_, b) = a.post("/strategy/api/strategies", create_body()).await;
    let sid = b["data"]["id"].as_i64().unwrap();
    let path = format!("/strategy/api/strategies/{}", sid);
    let (s, b) = a
        .send(a.req(
            Method::PATCH,
            &path,
            Some(json!({"overall_sl_mtm": 2500})),
            true,
        ))
        .await;
    assert_eq!(s, StatusCode::OK, "{}", b);
    assert_eq!(b["data"]["overall_sl_mtm"], 2500.0);
    let (s, b) = a
        .send(a.req(
            Method::PATCH,
            &path,
            Some(json!({"strategy_kind": "signal"})),
            true,
        ))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(b["message"]
        .as_str()
        .unwrap()
        .contains("cannot change between batch and signal"));
    let (s, _) = a
        .send(a.req(Method::PATCH, &path, Some(json!({})), true))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, b) = a
        .send(a.req(
            Method::PATCH,
            &path,
            Some(json!({"strategy_type": "intraday"})),
            true,
        ))
        .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "merged config re-validated: {}",
        b
    );
}

#[tokio::test]
async fn start_requires_a_mode() {
    let a = app();
    let (_, b) = a.post("/strategy/api/strategies", create_body()).await;
    let sid = b["data"]["id"].as_i64().unwrap();
    let (s, b) = a
        .post(
            &format!("/strategy/api/strategies/{}/start", sid),
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(b["message"], "mode must be one of: live, sandbox");
    let (s, _) = a
        .post(&format!("/strategy/api/strategies/{}/stop", sid), json!({}))
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_sandbox_run_starts_fills_through_the_sandbox_and_stops_flat() {
    let a = app();
    let (_, b) = a.post("/strategy/api/strategies", create_body()).await;
    let sid = b["data"]["id"].as_i64().unwrap();
    let (s, b) = a
        .post(
            &format!("/strategy/api/strategies/{}/start", sid),
            json!({"mode": "sandbox"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{}", b);
    let run = b["run_id"].as_i64().unwrap();
    assert_eq!(b["mode"], "sandbox");
    // The sandbox fills the MARKET entry and publishes order.update; the
    // strategy subscriber folds it into the leg.
    let m = a.ctx.strategy.clone();
    assert!(
        a.until(|| m
            .state
            .snapshot(run)
            .map(|s| s.legs["1"].entry_status == "complete")
            .unwrap_or(false))
            .await
    );
    assert_eq!(m.state.snapshot(run).unwrap().legs["1"].entry_avg, 100.0);
    let (s, b) = a
        .post(&format!("/strategy/api/strategies/{}/stop", sid), json!({}))
        .await;
    assert_eq!(s, StatusCode::OK, "{}", b);
    let store = a.ctx.strategy.store.clone();
    assert!(
        a.until(|| store.get_run(run).unwrap().unwrap().stopped_at.is_some())
            .await
    );
    let (_, orders) = a
        .get(&format!("/strategy/api/strategies/{}/orders", sid))
        .await;
    let rows = orders["data"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["action"], "BUY");
    assert_eq!(rows[1]["status"], "complete");
    // Nothing reached the live broker.
    assert!(!a.mock.calls().iter().any(|c| matches!(
        c,
        openalgo_desktop_lib::brokers::mock::MockCall::PlaceOrder(_)
    )));
    // The orderbook view reads the sandbox book for a sandbox run.
    let (s, ob) = a
        .get(&format!("/strategy/api/strategies/{}/orderbook", sid))
        .await;
    assert_eq!(s, StatusCode::OK, "{}", ob);
    assert_eq!(ob["data"]["orders"].as_array().unwrap().len(), 2);
    assert_eq!(ob["mode"], "analyze");
    let (_, cps) = a
        .get(&format!("/strategy/api/strategies/{}/checkpoints", sid))
        .await;
    assert_eq!(cps["run_id"], run);
    let (_, runs) = a
        .get(&format!("/strategy/api/strategies/{}/runs", sid))
        .await;
    assert_eq!(runs["data"][0]["stop_reason"], "manual");
}

#[tokio::test]
async fn the_kill_switch_locks_and_flattens() {
    let a = app();
    let (_, b) = a.post("/strategy/api/strategies", create_body()).await;
    let sid = b["data"]["id"].as_i64().unwrap();
    let (s, b) = a
        .post(
            &format!("/strategy/api/strategies/{}/kill_switch", sid),
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["webhook_locked"], true);
    assert_eq!(b["run_stopped"], false);
    let (s, b) = a
        .post(
            &format!("/strategy/api/strategies/{}/unlock_webhook", sid),
            json!({}),
        )
        .await;
    assert_eq!(
        (s, b["webhook_locked"].clone()),
        (StatusCode::OK, json!(false))
    );
}

#[tokio::test]
async fn rotate_returns_a_new_token_and_live_toggles() {
    let a = app();
    let (_, b) = a.post("/strategy/api/strategies", create_body()).await;
    let sid = b["data"]["id"].as_i64().unwrap();
    let old = b["webhook_token"].as_str().unwrap().to_string();
    let (s, b) = a
        .post(
            &format!("/strategy/api/strategies/{}/webhook/rotate", sid),
            json!({}),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_ne!(b["webhook_token"].as_str().unwrap(), old);
    let (s, _) = a
        .post(&format!("/strategy/api/strategies/{}/live", sid), json!({}))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, b) = a
        .post(
            &format!("/strategy/api/strategies/{}/live", sid),
            json!({"enabled": true}),
        )
        .await;
    assert_eq!(
        (s, b["live_enabled"].clone()),
        (StatusCode::OK, json!(true))
    );
}

#[tokio::test]
async fn the_events_query_is_validated_and_clamped() {
    let a = app();
    let (_, b) = a.post("/strategy/api/strategies", create_body()).await;
    let sid = b["data"]["id"].as_i64().unwrap();
    let (s, _) = a
        .get(&format!(
            "/strategy/api/strategies/{}/events?kind=bogus",
            sid
        ))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = a
        .get(&format!("/strategy/api/strategies/{}/events?limit=x", sid))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, b) = a
        .get(&format!("/strategy/api/strategies/{}/events?limit=-1", sid))
        .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(b["data"].as_array().unwrap().len(), 1, "clamped to 1");
}

#[tokio::test]
async fn the_public_webhook_needs_no_session_or_csrf() {
    let a = app();
    let (_, b) = a.post("/strategy/api/strategies", create_body()).await;
    let token = b["webhook_token"].as_str().unwrap().to_string();
    let (s, b) = a
        .webhook(&token, r#"{"action":"start","mode":"sandbox"}"#)
        .await;
    assert_eq!(s, StatusCode::OK, "{}", b);
    assert_eq!(b["result"], "ok");
    let (s, b) = a.webhook("oaws_unknown_but_long_enough_xx", "{}").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    assert_eq!(
        b,
        json!({"status": "error", "result": "rejected_token", "message": "Unknown or expired webhook token"})
    );
    let big = format!("{{\"x\":\"{}\"}}", "a".repeat(17000));
    let (s, _) = a.webhook(&token, &big).await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
}

// ------------------------------------------------- SM-02, SM-04

#[tokio::test]
async fn a_patch_to_a_taken_name_answers_409_and_saves_nothing() {
    let a = app();
    a.post("/strategy/api/strategies", create_body()).await;
    let mut other = create_body();
    other["name"] = json!("Iron fly");
    let (_, b) = a.post("/strategy/api/strategies", other).await;
    let sid = b["data"]["id"].as_i64().unwrap();
    let path = format!("/strategy/api/strategies/{}", sid);
    let (_, before) = a.get(&path).await;
    let (s, b) = a
        .send(a.req(
            Method::PATCH,
            &path,
            Some(json!({"name": "Short straddle", "overall_sl_mtm": 2500})),
            true,
        ))
        .await;
    assert_eq!(s, StatusCode::CONFLICT, "{}", b);
    assert_eq!(b["status"], "error");
    assert!(b["message"].as_str().unwrap().contains("already exists"));
    let (_, after) = a.get(&path).await;
    assert_eq!(after, before);
}

/// Break one local table, as a corrupt or unreadable database would.
fn break_table(a: &App, table: &str) {
    a.ctx
        .sqlite
        .conn()
        .unwrap()
        .execute_batch(&format!(
            "ALTER TABLE {table} RENAME TO {table}_unreadable;"
        ))
        .unwrap();
}

async fn assert_unavailable(a: &App, path: &str) {
    let (s, b) = a.get(path).await;
    assert!(
        !s.is_success(),
        "{} answered {} with {} instead of an error",
        path,
        s,
        b
    );
    assert_eq!(b["status"], "error", "{}: {}", path, b);
}

#[tokio::test]
async fn unreadable_order_records_answer_an_error_not_an_empty_book() {
    let a = app();
    let (_, b) = a.post("/strategy/api/strategies", create_body()).await;
    let sid = b["data"]["id"].as_i64().unwrap();
    let (s, b) = a
        .post(
            &format!("/strategy/api/strategies/{}/start", sid),
            json!({"mode": "sandbox"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{}", b);
    break_table(&a, "sm_strategy_order");
    for view in ["orders", "orderbook", "tradebook", "positions"] {
        assert_unavailable(&a, &format!("/strategy/api/strategies/{}/{}", sid, view)).await;
    }
    let (s, b) = a
        .api("/api/v1/strategy/orders", json!({"strategy_id": sid}))
        .await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{}", b);
    assert_eq!(b["status"], "error");
}

#[tokio::test]
async fn unreadable_run_records_never_read_as_never_run() {
    let a = app();
    let (_, b) = a.post("/strategy/api/strategies", create_body()).await;
    let sid = b["data"]["id"].as_i64().unwrap();
    break_table(&a, "sm_strategy_run");
    for view in ["runs", "checkpoints", "orderbook", "tradebook", "positions"] {
        assert_unavailable(&a, &format!("/strategy/api/strategies/{}/{}", sid, view)).await;
    }
    let (s, b) = a
        .api("/api/v1/strategy/runs", json!({"strategy_id": sid}))
        .await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{}", b);
}

#[tokio::test]
async fn an_unreadable_strategy_is_an_error_not_not_found() {
    let a = app();
    let (_, b) = a.post("/strategy/api/strategies", create_body()).await;
    let sid = b["data"]["id"].as_i64().unwrap();
    break_table(&a, "sm_strategy");
    let (s, b) = a.get("/strategy/api/strategies").await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{}", b);
    assert_eq!(b["status"], "error");
    // A stop or a kill switch must not be told the strategy does not exist.
    let (s, b) = a
        .post(&format!("/strategy/api/strategies/{}/stop", sid), json!({}))
        .await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{}", b);
    assert_ne!(b["message"], "Strategy not found");
}

fn restore_table(a: &App, table: &str) {
    a.ctx
        .sqlite
        .conn()
        .unwrap()
        .execute_batch(&format!(
            "ALTER TABLE {table}_unreadable RENAME TO {table};"
        ))
        .unwrap();
}

/// The stop and leg-close requests of the page and of `/api/v1`, each
/// answered 500 with `status: error` and never as "not active" or "not
/// found"; the page's carry the trader's text.
async fn stop_and_close_are_unavailable(a: &App, sid: i64) {
    use openalgo_desktop_lib::strategy::engine::STOP_READ_FAILED;
    for path in [
        format!("/strategy/api/strategies/{}/stop", sid),
        format!("/strategy/api/strategies/{}/legs/1/close", sid),
    ] {
        let (s, b) = a.post(&path, json!({})).await;
        assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{}: {}", path, b);
        assert_eq!(b["status"], "error");
        assert_ne!(b["message"], "Run is not active", "{}", path);
        assert_ne!(b["message"], "Strategy not found", "{}", path);
        if b["message"] != STOP_READ_FAILED {
            // The strategy itself unreadable: the route answers first (SM-04).
            assert!(b["message"].as_str().unwrap().contains("could not be read"));
        }
    }
    for (route, body) in [
        ("/api/v1/strategy/stop", json!({"strategy_id": sid})),
        (
            "/api/v1/strategy/close_leg",
            json!({"strategy_id": sid, "leg_id": 1}),
        ),
    ] {
        let (s, b) = a.api(route, body).await;
        assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR, "{}: {}", route, b);
        assert_eq!(b["status"], "error");
        assert_ne!(b["message"], "Run is not active", "{}", route);
        assert_ne!(b["message"], "Strategy not found", "{}", route);
    }
}

/// SM-04 follow-up (re-audit NEW-TR-06): a stop or a leg close that cannot
/// read its own run, or its strategy, answers an error (500, logged), never
/// "Run is not active" or "Strategy not found", through the page and
/// `/api/v1` alike. Nothing is closed: no exit order is written, and the run
/// stays open and managed.
#[tokio::test]
async fn a_stop_that_cannot_read_its_run_is_an_error_not_not_active() {
    let a = app();
    let (_, b) = a.post("/strategy/api/strategies", create_body()).await;
    let sid = b["data"]["id"].as_i64().unwrap();
    let (s, b) = a
        .post(
            &format!("/strategy/api/strategies/{}/start", sid),
            json!({"mode": "sandbox"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{}", b);
    let run = b["run_id"].as_i64().unwrap();
    let orders = a.ctx.strategy.store.list_orders(run).unwrap().len();
    for table in ["sm_strategy_run", "sm_strategy"] {
        break_table(&a, table);
        stop_and_close_are_unavailable(&a, sid).await;
        restore_table(&a, table);
    }
    // Nothing was closed: no exit order, the run still open and managed.
    assert_eq!(a.ctx.strategy.store.list_orders(run).unwrap().len(), orders);
    let open = a.ctx.strategy.store.get_run(run).unwrap().unwrap();
    assert!(open.stopped_at.is_none() && open.stop_requested_at.is_none());
    assert!(
        a.ctx.strategy.state.snapshot(run).is_some(),
        "still managed"
    );
}

#[tokio::test]
async fn a_stop_that_cannot_read_its_strategy_is_an_error_not_not_found() {
    use openalgo_desktop_lib::strategy::engine::STOP_READ_FAILED;
    let a = app();
    let (row, _) = a
        .ctx
        .strategy
        .store
        .create_strategy(USER, &config("Read", json!([short_call_leg()]), json!({})))
        .unwrap();
    let r = a
        .ctx
        .strategy
        .start_run(row.id, USER, "sandbox", "manual", None)
        .await;
    assert!(r.ok, "{:?}", r);
    let run = r.run_id.unwrap();
    break_table(&a, "sm_strategy");
    let stop = a.ctx.strategy.stop_run(run, USER, "manual").await;
    assert!(stop.read_failed, "{:?}", stop);
    assert_eq!(stop.error.as_deref(), Some(STOP_READ_FAILED));
    assert_eq!(stop.error_status(), 500);
    let close = a.ctx.strategy.close_leg(run, 1, USER).await;
    assert!(close.read_failed, "{:?}", close);
    assert_eq!(close.error.as_deref(), Some(STOP_READ_FAILED));
    restore_table(&a, "sm_strategy");
    assert!(
        a.ctx.strategy.state.snapshot(run).is_some(),
        "still managed"
    );
    // A strategy that is not the user's is still "not found", a 409.
    let other = a.ctx.strategy.stop_run(run, "someone-else", "manual").await;
    assert_eq!(other.error.as_deref(), Some("Strategy not found"));
    assert_eq!(other.error_status(), 409);
}

// ------------------------------------------------- SM-03, SM-05

#[tokio::test]
async fn the_page_reads_orders_with_their_run_mode_and_the_api_keeps_the_web_shape() {
    let a = app();
    let (_, b) = a.post("/strategy/api/strategies", create_body()).await;
    let sid = b["data"]["id"].as_i64().unwrap();
    let (s, b) = a
        .post(
            &format!("/strategy/api/strategies/{}/start", sid),
            json!({"mode": "sandbox"}),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{}", b);
    let (s, b) = a
        .get(&format!("/strategy/api/strategies/{}/orders", sid))
        .await;
    assert_eq!(s, StatusCode::OK, "{}", b);
    let rows = b["data"].as_array().unwrap();
    assert!(!rows.is_empty(), "{}", b);
    assert!(rows.iter().all(|o| o["mode"] == "sandbox"), "{}", b);
    // /api/v1 answers the web's order dict, field for field.
    let (s, b) = a
        .api("/api/v1/strategy/orders", json!({"strategy_id": sid}))
        .await;
    assert_eq!(s, StatusCode::OK, "{}", b);
    let rows = b["data"].as_array().unwrap();
    assert!(!rows.is_empty(), "{}", b);
    assert!(rows.iter().all(|o| o.get("mode").is_none()), "{}", b);
    // The page's lifetime totals come per mode from the server.
    let (s, b) = a.get(&format!("/strategy/api/strategies/{}", sid)).await;
    assert_eq!(s, StatusCode::OK, "{}", b);
    assert_eq!(
        b["data"]["realized_by_mode"],
        json!({"live": 0.0, "sandbox": 0.0})
    );
}

// The Bridge 1.0.11: "Close positions" and "Close all open positions".

#[tokio::test]
async fn close_positions_on_a_stopped_strategy_is_a_harmless_ok() {
    let a = app();
    let (_, b) = a.post("/strategy/api/strategies", create_body()).await;
    let sid = b["data"]["id"].as_i64().unwrap();
    let (s, b) = a
        .post(&format!("/strategy/api/strategies/{}/close_positions", sid), json!({}))
        .await;
    assert_eq!(s, StatusCode::OK, "{}", b);
    assert_eq!(b["closed"], 0);
    let (s, _) = a
        .post("/strategy/api/strategies/999999/close_positions", json!({}))
        .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn close_all_squares_off_each_running_strategy_by_its_own_quantity_and_keeps_it() {
    let a = app();
    let (_, b) = a.post("/strategy/api/strategies", create_body()).await;
    let sid = b["data"]["id"].as_i64().unwrap();
    let (s, b) = a
        .post(&format!("/strategy/api/strategies/{}/start", sid), json!({"mode": "sandbox"}))
        .await;
    assert_eq!(s, StatusCode::OK, "{}", b);
    let run = b["run_id"].as_i64().unwrap();
    let m = a.ctx.strategy.clone();
    assert!(
        a.until(|| m
            .state
            .snapshot(run)
            .map(|s| s.legs["1"].entry_status == "complete")
            .unwrap_or(false))
            .await
    );
    let (s, b) = a.post("/strategy/api/desk/close_all", json!({})).await;
    assert_eq!(s, StatusCode::OK, "{}", b);
    let results = b["results"].as_array().unwrap();
    assert_eq!(results.len(), 1, "{}", b);
    assert_eq!(results[0]["id"], sid);
    assert_eq!(results[0]["ok"], true, "{}", b);
    assert_eq!(results[0]["closed"], 1);
    let store = a.ctx.strategy.store.clone();
    // The exit is the leg's own fixed quantity, opposite the entry.
    let (_, orders) = a.get(&format!("/strategy/api/strategies/{}/orders", sid)).await;
    let rows = orders["data"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["action"], "BUY");
    assert_eq!(rows[1]["qty"], rows[0]["qty"]);
    let ev = store.list_events(sid, None, Some("manual_close"), None, 10).unwrap();
    assert_eq!(ev.len(), 1);
    // The strategy is not disabled: its webhook stays unlocked.
    assert!(!store.get_strategy(sid, USER).unwrap().unwrap().webhook_locked);
    // Nothing reached the live broker.
    assert!(!a.mock.calls().iter().any(|c| matches!(
        c,
        openalgo_desktop_lib::brokers::mock::MockCall::PlaceOrder(_)
    )));
}
