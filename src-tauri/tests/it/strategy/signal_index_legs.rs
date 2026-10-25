//! The Bridge 1.0.6: signal strategies on an index, with legs whose
//! contract (future, or ATM-relative option) is chosen when the alert
//! arrives. Driven end to end through the webhook pipeline in sandbox.

use super::*;
use openalgo_desktop_lib::strategy::validate::validate_strategy_config;

const ATM_PE: &str = "NIFTY13OCT2624500PE";

fn option_leg(id: i64, side: &str, position: &str, option_type: &str) -> Value {
    json!({
        "id": id, "side": side, "segment": "options", "expiry": "weekly", "lots": 1,
        "position": position, "option_type": option_type, "strike_mode": "atm",
        "atm_offset": "ATM", "sl_pts": 20, "trail": {"x": 0, "y": 0}, "risk_unit": "points",
    })
}

fn index_signal(legs: Value) -> Value {
    config(
        "Index signal",
        legs,
        json!({"strategy_kind": "signal", "universe_tab": "weekly_monthly", "product": "MIS",
               "strategy_type": "intraday", "entry_time": "00:00", "exit_time": "23:59"}),
    )
}

async fn hook(t: &T, token: &str, body: Value) -> openalgo_desktop_lib::strategy::webhook::WebhookOutcome {
    t.m.handle_webhook(token, body.to_string().as_bytes(), Some("10.0.0.1"), Some("TradingView"))
        .await
}

async fn fill_last_entry(t: &T, run: i64) {
    let o = t
        .orders(run)
        .into_iter()
        .filter(|o| o.kind == "entry")
        .last()
        .unwrap();
    t.frame(o.broker_order_id.as_deref().unwrap(), "complete", o.qty, 100.0)
        .await;
}

#[test]
fn index_legs_chosen_at_signal_time_validate() {
    let t = t();
    let g = t.symbols.snapshot();
    let ok = validate_strategy_config(
        &index_signal(json!([option_leg(1, "long", "B", "CE"), option_leg(2, "short", "B", "PE")])),
        &g,
    )
    .unwrap();
    assert_eq!(ok["legs"][0]["side"], "long");
    assert_eq!(ok["legs"][1]["option_type"], "PE");
    // A contract-choosing leg fires on one side only.
    let both = validate_strategy_config(&index_signal(json!([option_leg(1, "both", "B", "CE")])), &g);
    assert!(both.unwrap_err().contains("side"));
    let mut fut = option_leg(1, "long", "B", "CE");
    fut["segment"] = json!("futures");
    fut.as_object_mut().unwrap().retain(|k, _| !["option_type", "strike_mode", "atm_offset"].contains(&k.as_str()));
    fut["expiry"] = json!("monthly");
    assert!(validate_strategy_config(&index_signal(json!([fut])), &g).is_ok());
}

#[tokio::test]
async fn long_buys_the_call_short_flips_to_the_put_and_exit_closes_it() {
    let t = t();
    let (_, token) = t.make_with_token(index_signal(json!([
        option_leg(1, "long", "B", "CE"),
        option_leg(2, "short", "B", "PE"),
    ])));
    let a = hook(&t, &token, json!({"action": "long_entry"})).await;
    assert_eq!(a.result, "ok", "{}", a.message);
    let run = a.run_id.unwrap();
    let p = t.gw.placed();
    assert_eq!((p[0].symbol.as_str(), p[0].action.as_str()), (ATM_CE, "BUY"));
    assert_eq!(p[0].exchange, "NFO");
    fill_last_entry(&t, run).await;

    // A repeated long alert opens nothing more.
    let again = hook(&t, &token, json!({"action": "long_entry"})).await;
    assert_eq!(again.result, "ok");
    assert_eq!(t.gw.placed().len(), 1);

    // short_entry: close the call, then buy the put.
    let b = hook(&t, &token, json!({"action": "short_entry"})).await;
    assert_eq!(b.result, "ok", "{}", b.message);
    let p = t.gw.placed();
    assert_eq!((p[1].symbol.as_str(), p[1].action.as_str()), (ATM_CE, "SELL"));
    assert_eq!((p[2].symbol.as_str(), p[2].action.as_str()), (ATM_PE, "BUY"));
    fill_last_entry(&t, run).await;

    // short_exit closes the put that was opened, not a re-resolved strike.
    let c = hook(&t, &token, json!({"action": "short_exit"})).await;
    assert_eq!(c.result, "ok", "{}", c.message);
    let p = t.gw.placed();
    assert_eq!((p[3].symbol.as_str(), p[3].action.as_str()), (ATM_PE, "SELL"));
    assert_eq!(p.len(), 4);
}

#[tokio::test]
async fn selling_the_put_on_long_holds_a_short_and_exits_by_buying() {
    let t = t();
    let (_, token) = t.make_with_token(index_signal(json!([option_leg(1, "long", "S", "PE")])));
    let a = hook(&t, &token, json!({"action": "long_entry"})).await;
    assert_eq!(a.result, "ok", "{}", a.message);
    fill_last_entry(&t, a.run_id.unwrap()).await;
    let c = hook(&t, &token, json!({"action": "long_exit"})).await;
    assert_eq!(c.result, "ok", "{}", c.message);
    assert_eq!(t.gw.actions(), vec!["SELL", "BUY"]);
    assert!(t.gw.placed().iter().all(|o| o.symbol == ATM_PE));
}

#[tokio::test]
async fn a_leg_id_for_the_other_side_is_refused() {
    let t = t();
    let (_, token) = t.make_with_token(index_signal(json!([option_leg(1, "long", "B", "CE")])));
    let o = hook(&t, &token, json!({"action": "short_entry", "leg_id": 1})).await;
    assert_ne!(o.result, "ok");
    assert!(t.gw.placed().is_empty());
}
