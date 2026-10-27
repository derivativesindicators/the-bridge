//! The Bridge 1.0.12 regressions: a carried signal run that has gone flat
//! must finish when the next day's stale-run stop rolls it (it used to stay
//! "stop requested" forever), and the next signal opens a new run.

use super::*;
use openalgo_desktop_lib::strategy::checkpoint::write_once;
use openalgo_desktop_lib::strategy::recovery::recover_run;

fn leg(id: i64, side: &str, position: &str, option_type: &str) -> Value {
    json!({
        "id": id, "side": side, "segment": "options", "expiry": "weekly", "lots": 1,
        "position": position, "option_type": option_type, "strike_mode": "atm",
        "atm_offset": "ATM", "trail": {"x": 0, "y": 0}, "risk_unit": "points",
    })
}

fn strat() -> Value {
    config(
        "Stuck stop",
        json!([leg(1, "long", "S", "PE"), leg(2, "short", "S", "CE")]),
        json!({"strategy_kind": "signal", "universe_tab": "weekly_monthly", "product": "NRML",
               "strategy_type": "positional", "signal_options": {"carry_overnight": true}}),
    )
}

async fn send(t: &T, token: &str, body: &str) -> openalgo_desktop_lib::strategy::webhook::WebhookOutcome {
    t.m.webhook.reset();
    t.m.handle_webhook(token, body.as_bytes(), Some("10.0.0.1"), Some("TradingView")).await
}

async fn fill_all(t: &T, run: i64) {
    for o in t.orders(run) {
        if o.status == "open" || o.status == "pending" {
            t.frame(o.broker_order_id.as_deref().unwrap(), "complete", o.qty, 100.0).await;
        }
    }
}

async fn carried_then_flat(t: &T, token: &str, restart: bool) -> i64 {
    let a = send(t, token, "BUY").await;
    assert_eq!(a.result, "ok", "{}", a.message);
    let run = a.run_id.unwrap();
    fill_all(t, run).await;
    write_once(&t.m, Some(false));
    if restart {
        t.m.state.clear(run);
    }
    t.clock.set(ist(2026, 10, 8, 9, 20));
    if restart {
        assert!(recover_run(&t.m, run).await.ok);
    }
    for _ in 0..2 {
        t.gw.reject_next("Symbol NIFTY13OCT2624500PE not found on NFO");
        let x = send(t, token, "CLOSE LONG: Normal Exit").await;
        eprintln!("rejected exit: {} {}", x.result, x.message);
        let y = send(t, token, "BUY").await;
        eprintln!("buy while held: {} {}", y.result, y.message);
    }
    let b = send(t, token, "CLOSE LONG: Normal Exit").await;
    assert_eq!(b.result, "ok", "{}", b.message);
    assert_eq!(b.run_id, Some(run));
    fill_all(t, run).await;
    t.clock.set(ist(2026, 10, 8, 12, 0));
    run
}

async fn check(restart: bool) {
    let t = t();
    let (sid, token) = t.make_with_token(strat());
    let run = carried_then_flat(&t, &token, restart).await;
    // CLOSE SHORT with no open short leg on a stale, flat run.
    let c = send(&t, &token, "CLOSE SHORT: Normal Exit").await;
    let r = t.run(run);
    assert!(
        r.stopped_at.is_some(),
        "flat carried run must finish, not stay stop-requested: {:?} / {} {}",
        r.stop_requested_reason, c.result, c.message
    );
    let d = send(&t, &token, "BUY").await;
    assert_eq!(d.result, "ok", "{}", d.message);
    assert_ne!(d.run_id, Some(run), "a new run opens");
    let _ = sid;
}

#[tokio::test]
async fn flat_carried_run_finishes_on_stale_roll() {
    check(false).await;
}

#[tokio::test]
async fn flat_carried_run_finishes_on_stale_roll_after_restart() {
    check(true).await;
}

/// A signal-entry claim that was never released (no order row behind it)
/// must not hold a requested stop pending forever once every leg is flat.
#[tokio::test]
async fn leaked_entry_claim_does_not_block_a_stop_on_a_flat_run() {
    let t = t();
    let (_sid, token) = t.make_with_token(strat());
    let a = send(&t, &token, "BUY").await;
    let run = a.run_id.unwrap();
    fill_all(&t, run).await;
    let b = send(&t, &token, "CLOSE LONG: Normal Exit").await;
    assert_eq!(b.result, "ok", "{}", b.message);
    fill_all(&t, run).await;
    // Leg 2 never opened; simulate an entry decision that leaked its claim.
    assert!(matches!(
        t.m.state.claim_signal_entry(run, 2, "S"),
        Some(openalgo_desktop_lib::strategy::state::EntryDecision::Claimed(_))
    ));
    let user = t.row(_sid).user_id;
    let out = t.m.stop_run(run, &user, "manual").await;
    assert!(out.ok && !out.stop_pending, "{:?}", out.error);
    assert!(t.run(run).stopped_at.is_some());
    // The next signal starts a new run.
    let d = send(&t, &token, "BUY").await;
    assert_eq!(d.result, "ok", "{}", d.message);
    assert_ne!(d.run_id, Some(run));
}
