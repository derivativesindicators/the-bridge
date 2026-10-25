//! The Bridge 1.0.8: overnight carry, the expiry-day roll (both reopen
//! modes), entry deferral, the CAS exit, restart recovery, and same-bar
//! CLOSE LONG + SELL with carry on and off. Sandbox, through the webhook.

use super::*;
use openalgo_desktop_lib::brokers::types::Candle;
use openalgo_desktop_lib::strategy::checkpoint::write_once;
use openalgo_desktop_lib::strategy::recovery::recover_run;

const CE_13: &str = "NIFTY13OCT2624500CE";
const PE_13: &str = "NIFTY13OCT2624500PE";
const CE_27: &str = "NIFTY27OCT2624500CE";

fn leg(id: i64, side: &str, position: &str, option_type: &str) -> Value {
    json!({
        "id": id, "side": side, "segment": "options", "expiry": "weekly", "lots": 1,
        "position": position, "option_type": option_type, "strike_mode": "atm",
        "atm_offset": "ATM", "trail": {"x": 0, "y": 0}, "risk_unit": "points",
    })
}

/// A positional NRML index signal strategy with these options.
fn strat(legs: Value, opts: Value) -> Value {
    config(
        "Signal options",
        legs,
        json!({"strategy_kind": "signal", "universe_tab": "weekly_monthly", "product": "NRML",
               "strategy_type": "positional", "signal_options": opts}),
    )
}

async fn send(
    t: &T,
    token: &str,
    body: &str,
) -> openalgo_desktop_lib::strategy::webhook::WebhookOutcome {
    t.m.webhook.reset();
    t.m.handle_webhook(
        token,
        body.as_bytes(),
        Some("10.0.0.1"),
        Some("TradingView"),
    )
    .await
}

async fn fill_all(t: &T, run: i64) {
    for o in t.orders(run) {
        if o.status == "open" || o.status == "pending" {
            t.frame(
                o.broker_order_id.as_deref().unwrap(),
                "complete",
                o.qty,
                100.0,
            )
            .await;
        }
    }
}

fn placed(t: &T) -> Vec<(String, String)> {
    t.gw.placed()
        .into_iter()
        .map(|o| (o.symbol, o.action))
        .collect()
}

fn p(sym: &str, action: &str) -> (String, String) {
    (sym.to_string(), action.to_string())
}

#[tokio::test]
async fn carry_keeps_the_position_into_the_next_day_and_survives_a_restart() {
    let t = t();
    let (sid, token) = t.make_with_token(strat(
        json!([leg(1, "long", "B", "CE")]),
        json!({"carry_overnight": true}),
    ));
    let a = send(&t, &token, "BUY").await;
    assert_eq!(a.result, "ok", "{}", a.message);
    let run = a.run_id.unwrap();
    fill_all(&t, run).await;
    write_once(&t.m, Some(false));
    // Restart overnight.
    t.m.state.clear(run);
    t.clock.set(ist(2026, 10, 8, 9, 20));
    assert!(recover_run(&t.m, run).await.ok);
    let b = send(&t, &token, "BUY").await;
    assert_eq!(b.result, "ok", "{}", b.message);
    assert_eq!(b.run_id, Some(run), "the carried run continues");
    assert_eq!(
        placed(&t),
        vec![p(CE_13, "BUY")],
        "nothing closed or reopened"
    );
    assert!(t.event_kinds(sid).contains(&"position_carried".to_string()));
    // The exit alert still closes it.
    let c = send(&t, &token, "CLOSE LONG: Normal Exit").await;
    assert_eq!(c.result, "ok", "{}", c.message);
    assert_eq!(placed(&t).last().unwrap(), &p(CE_13, "SELL"));
}

#[tokio::test]
async fn without_carry_a_new_day_starts_a_new_run() {
    let t = t();
    let (_, token) = t.make_with_token(strat(json!([leg(1, "long", "B", "CE")]), Value::Null));
    let a = send(&t, &token, "BUY").await;
    let run = a.run_id.unwrap();
    fill_all(&t, run).await;
    t.clock.set(ist(2026, 10, 8, 9, 20));
    let b = send(&t, &token, "BUY").await;
    assert_ne!(b.run_id, Some(run), "{}", b.message);
}

#[test]
fn carry_needs_positional_nrml_and_roll_must_be_before_1530() {
    let t = t();
    let g = t.symbols.snapshot();
    let v = |c: &Value| openalgo_desktop_lib::strategy::validate::validate_strategy_config(c, &g);
    let mut mis = strat(
        json!([leg(1, "long", "B", "CE")]),
        json!({"carry_overnight": true}),
    );
    mis["product"] = json!("MIS");
    assert!(v(&mis).unwrap_err().contains("NRML"));
    let late = strat(
        json!([leg(1, "long", "B", "CE")]),
        json!({"expiry_roll": true, "reopen": "immediate", "roll_time": "15:30"}),
    );
    assert!(v(&late).unwrap_err().contains("15:30"));
    let ok = v(&strat(
        json!([leg(1, "long", "B", "CE")]),
        json!({"cas_exit": true}),
    ))
    .unwrap();
    assert_eq!(ok["signal_options"]["session_end"], "15:40");
    assert_eq!(ok["signal_options"]["cas_start"], "15:15");
    assert_eq!(ok["signal_options"]["session_times"], "auto");
    assert_eq!(
        v(&strat(json!([leg(1, "long", "B", "CE")]), Value::Null)).unwrap()["signal_options"],
        Value::Null
    );
}

#[tokio::test]
async fn roll_immediately_closes_the_expiring_leg_and_reopens_next_expiry() {
    let t = t_at(ist(2026, 10, 13, 10, 0));
    let (sid, token) = t.make_with_token(strat(
        json!([leg(1, "long", "B", "CE")]),
        json!({"expiry_roll": true, "reopen": "immediate", "roll_time": "15:20"}),
    ));
    let a = send(&t, &token, "BUY").await;
    let run = a.run_id.unwrap();
    assert_eq!(placed(&t), vec![p(CE_13, "BUY")]);
    fill_all(&t, run).await;
    t.clock.set(ist(2026, 10, 13, 15, 19));
    t.m.run_signal_timers().await;
    assert_eq!(placed(&t).len(), 1, "not before the roll time");
    t.clock.set(ist(2026, 10, 13, 15, 20));
    t.m.run_signal_timers().await;
    assert_eq!(placed(&t)[1], p(CE_13, "SELL"));
    t.m.run_signal_timers().await;
    assert_eq!(
        placed(&t).len(),
        2,
        "rolled once; reopen waits for the close to fill"
    );
    fill_all(&t, run).await;
    t.clock.set(ist(2026, 10, 13, 15, 21));
    t.m.run_signal_timers().await;
    assert_eq!(placed(&t)[2], p(CE_27, "BUY"), "{:?}", t.event_kinds(sid));
    assert!(t.event_kinds(sid).contains(&"expiry_roll".to_string()));
}

#[tokio::test]
async fn roll_next_session_reopens_at_the_reopen_time_after_a_restart() {
    let t = t_at(ist(2026, 10, 13, 10, 0));
    let (sid, token) = t.make_with_token(strat(
        json!([leg(1, "long", "B", "CE")]),
        json!({"carry_overnight": true, "expiry_roll": true, "roll_time": "15:25", "reopen_time": "09:20"}),
    ));
    let run = send(&t, &token, "BUY").await.run_id.unwrap();
    fill_all(&t, run).await;
    t.clock.set(ist(2026, 10, 13, 15, 25));
    t.m.run_signal_timers().await;
    assert_eq!(placed(&t)[1], p(CE_13, "SELL"));
    fill_all(&t, run).await;
    t.m.run_signal_timers().await;
    assert_eq!(placed(&t).len(), 2, "not reopened the same day");
    write_once(&t.m, Some(false));
    t.m.state.clear(run);
    t.clock.set(ist(2026, 10, 14, 9, 19));
    let _ = recover_run(&t.m, run).await;
    t.m.run_signal_timers().await;
    assert_eq!(placed(&t).len(), 2, "not before the reopen time");
    t.clock.set(ist(2026, 10, 14, 9, 20));
    t.m.run_signal_timers().await;
    assert_eq!(placed(&t)[2], p(CE_27, "BUY"), "{:?}", t.event_kinds(sid));
}

#[tokio::test]
async fn expiry_day_entry_is_deferred_and_an_exit_cancels_it() {
    let t = t_at(ist(2026, 10, 13, 10, 0));
    let (sid, token) = t.make_with_token(strat(
        json!([leg(1, "long", "B", "CE")]),
        json!({"carry_overnight": true, "expiry_day_entry": "defer"}),
    ));
    let a = send(&t, &token, "BUY").await;
    assert_eq!(a.result, "ok", "{}", a.message);
    assert!(placed(&t).is_empty(), "deferred, nothing placed");
    assert!(t.event_kinds(sid).contains(&"entry_deferred".to_string()));
    t.clock.set(ist(2026, 10, 14, 9, 20));
    t.m.run_signal_timers().await;
    assert_eq!(placed(&t), vec![p(CE_27, "BUY")]);

    // A second strategy: the exit alert arrives before the reopen time.
    let t = t_at(ist(2026, 10, 13, 10, 0));
    let (_, token) = t.make_with_token(strat(
        json!([leg(1, "long", "B", "CE")]),
        json!({"carry_overnight": true, "expiry_day_entry": "defer"}),
    ));
    send(&t, &token, "BUY").await;
    send(&t, &token, "CLOSE LONG: Normal Exit").await;
    t.clock.set(ist(2026, 10, 14, 9, 20));
    t.m.run_signal_timers().await;
    assert!(placed(&t).is_empty(), "the exit dropped the deferred entry");
}

fn candle(at: chrono::DateTime<chrono::Utc>, c: f64) -> Candle {
    Candle {
        timestamp: at.timestamp(),
        open: c,
        high: c + 1.0,
        low: c - 1.0,
        close: c,
        volume: 0,
        oi: 0,
    }
}

#[tokio::test]
async fn cas_exit_closes_on_a_premium_supertrend_flip_in_the_closing_session() {
    let t = t_at(ist(2026, 10, 7, 14, 0));
    let (sid, token) = t.make_with_token(strat(
        json!([leg(1, "long", "B", "CE")]),
        json!({"cas_exit": true}),
    ));
    let run = send(&t, &token, "BUY").await.run_id.unwrap();
    fill_all(&t, run).await;
    // Rising premium on 5-minute candles until 15:15, then a collapse.
    let mut c = vec![];
    for i in 0..30u32 {
        let at = ist(2026, 10, 7, 13, 15) + chrono::Duration::minutes(5 * i as i64);
        let px = if at < ist(2026, 10, 7, 15, 15) {
            100.0 + i as f64 * 2.0
        } else {
            40.0
        };
        c.push(candle(at, px));
    }
    *t.gw.candles.lock() = c;
    t.clock.set(ist(2026, 10, 7, 15, 14));
    t.m.run_signal_timers().await;
    assert_eq!(
        placed(&t).len(),
        1,
        "before the spot's last bar ends, CAS is idle"
    );
    t.clock.set(ist(2026, 10, 7, 15, 26));
    t.m.run_signal_timers().await;
    assert_eq!(placed(&t)[1], p(CE_13, "SELL"), "{:?}", t.event_kinds(sid));
    assert!(t.event_kinds(sid).contains(&"cas_exit".to_string()));
}

#[tokio::test]
async fn cas_without_history_holds_and_says_so() {
    let t = t_at(ist(2026, 10, 7, 14, 0));
    let (sid, token) = t.make_with_token(strat(
        json!([leg(1, "long", "B", "CE")]),
        json!({"cas_exit": true}),
    ));
    let run = send(&t, &token, "BUY").await.run_id.unwrap();
    fill_all(&t, run).await;
    t.clock.set(ist(2026, 10, 7, 15, 20));
    t.m.run_signal_timers().await;
    t.m.run_signal_timers().await;
    assert_eq!(placed(&t).len(), 1);
    let n = t
        .event_kinds(sid)
        .iter()
        .filter(|k| *k == "cas_tracking_unavailable")
        .count();
    assert_eq!(n, 1, "said once");
}

fn four() -> Value {
    json!([
        leg(1, "long", "B", "CE"),
        leg(2, "long", "S", "PE"),
        leg(3, "short", "B", "PE"),
        leg(4, "short", "S", "CE")
    ])
}

/// CLOSE LONG + SELL on one bar, in both orders, with carry on and off:
/// the long legs are closed once and the short legs opened once.
#[tokio::test]
async fn same_bar_close_long_and_sell_in_either_order_with_carry_on_or_off() {
    for carry in [false, true] {
        for close_first in [true, false] {
            let t = t();
            let opts = if carry {
                json!({"carry_overnight": true})
            } else {
                Value::Null
            };
            let (_, token) = t.make_with_token(strat(four(), opts));
            let run = send(&t, &token, "BUY").await.run_id.unwrap();
            fill_all(&t, run).await;
            let order = if close_first {
                ["CLOSE LONG: Normal Exit", "SELL"]
            } else {
                ["SELL", "CLOSE LONG: Normal Exit"]
            };
            for m in order {
                let r = send(&t, &token, m).await;
                assert_eq!(r.result, "ok", "carry {} {}: {}", carry, m, r.message);
                fill_all(&t, run).await;
            }
            let mut got = placed(&t)[2..].to_vec();
            got.sort();
            let mut want = vec![
                p(CE_13, "SELL"),
                p(PE_13, "BUY"),
                p(PE_13, "BUY"),
                p(CE_13, "SELL"),
            ];
            want.sort();
            assert_eq!(got, want, "carry {} close_first {}", carry, close_first);
            assert_eq!(t.leg(run, 1).status, "closed");
            assert_eq!(t.leg(run, 3).status, "open");
            assert_eq!(t.leg(run, 4).status, "open");
        }
    }
}
