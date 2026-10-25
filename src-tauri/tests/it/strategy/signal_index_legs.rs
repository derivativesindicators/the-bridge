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

async fn hook(
    t: &T,
    token: &str,
    body: Value,
) -> openalgo_desktop_lib::strategy::webhook::WebhookOutcome {
    t.m.handle_webhook(
        token,
        body.to_string().as_bytes(),
        Some("10.0.0.1"),
        Some("TradingView"),
    )
    .await
}

async fn fill_last_entry(t: &T, run: i64) {
    let o = t
        .orders(run)
        .into_iter()
        .filter(|o| o.kind == "entry")
        .last()
        .unwrap();
    t.frame(
        o.broker_order_id.as_deref().unwrap(),
        "complete",
        o.qty,
        100.0,
    )
    .await;
}

#[test]
fn index_legs_chosen_at_signal_time_validate() {
    let t = t();
    let g = t.symbols.snapshot();
    let ok = validate_strategy_config(
        &index_signal(json!([
            option_leg(1, "long", "B", "CE"),
            option_leg(2, "short", "B", "PE")
        ])),
        &g,
    )
    .unwrap();
    assert_eq!(ok["legs"][0]["side"], "long");
    assert_eq!(ok["legs"][1]["option_type"], "PE");
    // A contract-choosing leg fires on one side only.
    let both =
        validate_strategy_config(&index_signal(json!([option_leg(1, "both", "B", "CE")])), &g);
    assert!(both.unwrap_err().contains("side"));
    let mut fut = option_leg(1, "long", "B", "CE");
    fut["segment"] = json!("futures");
    fut.as_object_mut()
        .unwrap()
        .retain(|k, _| !["option_type", "strike_mode", "atm_offset"].contains(&k.as_str()));
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
    assert_eq!(
        (p[0].symbol.as_str(), p[0].action.as_str()),
        (ATM_CE, "BUY")
    );
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
    assert_eq!(
        (p[1].symbol.as_str(), p[1].action.as_str()),
        (ATM_CE, "SELL")
    );
    assert_eq!(
        (p[2].symbol.as_str(), p[2].action.as_str()),
        (ATM_PE, "BUY")
    );
    fill_last_entry(&t, run).await;

    // short_exit closes the put that was opened, not a re-resolved strike.
    let c = hook(&t, &token, json!({"action": "short_exit"})).await;
    assert_eq!(c.result, "ok", "{}", c.message);
    let p = t.gw.placed();
    assert_eq!(
        (p[3].symbol.as_str(), p[3].action.as_str()),
        (ATM_PE, "SELL")
    );
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

// ---- The Bridge 1.0.7: strike chosen by premium at alert time ----

fn premium_leg(id: i64, side: &str, option_type: &str, rule: &str, premium: f64) -> Value {
    json!({
        "id": id, "side": side, "segment": "options", "expiry": "weekly", "lots": 1,
        "position": "B", "option_type": option_type, "strike_mode": "premium",
        "premium_rule": rule, "premium": premium, "trail": {"x": 0, "y": 0}, "risk_unit": "points",
    })
}

/// CE premiums on the 13 OCT chain: 0.4 x (25000 - strike), so 24750 is
/// exactly 100, 24700 is 120, 24800 is 80.
fn price_calls(t: &T) {
    let mut l = t.gw.ltps.lock();
    for i in 0..=20 {
        let k = 24000 + 50 * i;
        l.insert(
            (format!("NIFTY13OCT26{}CE", k), "NFO".into()),
            0.4 * (25000 - k) as f64,
        );
    }
}

#[test]
fn premium_legs_validate_only_on_signal_legs() {
    let t = t();
    let g = t.symbols.snapshot();
    let ok = validate_strategy_config(
        &index_signal(json!([premium_leg(1, "long", "CE", "GTE", 100.0)])),
        &g,
    )
    .unwrap();
    assert_eq!(ok["legs"][0]["strike_mode"], "premium");
    assert_eq!(ok["legs"][0]["premium_rule"], "gte");
    assert_eq!(ok["legs"][0]["premium"], 100.0);
    let mut missing = premium_leg(1, "long", "CE", "gte", 100.0);
    missing.as_object_mut().unwrap().remove("premium");
    assert!(
        validate_strategy_config(&index_signal(json!([missing])), &g)
            .unwrap_err()
            .contains("premium")
    );
    let zero = premium_leg(1, "long", "CE", "gte", 0.0);
    assert!(validate_strategy_config(&index_signal(json!([zero])), &g).is_err());
    let bad_rule = premium_leg(1, "long", "CE", "near", 100.0);
    assert!(
        validate_strategy_config(&index_signal(json!([bad_rule])), &g)
            .unwrap_err()
            .contains("premium_rule")
    );
    let mut with_offset = premium_leg(1, "long", "CE", "gte", 100.0);
    with_offset["atm_offset"] = json!("ATM");
    assert!(validate_strategy_config(&index_signal(json!([with_offset])), &g).is_err());
    let mut stray = option_leg(1, "long", "B", "CE");
    stray["premium"] = json!(100);
    assert!(validate_strategy_config(&index_signal(json!([stray])), &g).is_err());
}

#[tokio::test]
async fn premium_rules_pick_the_strike_from_live_quotes() {
    for (rule, x, want) in [
        ("gte", 100.0, "NIFTY13OCT2624750CE"),
        ("gte", 110.0, "NIFTY13OCT2624700CE"),
        ("lte", 90.0, "NIFTY13OCT2624800CE"),
        ("closest", 95.0, "NIFTY13OCT2624750CE"),
    ] {
        let t = t();
        price_calls(&t);
        let (_, token) =
            t.make_with_token(index_signal(json!([premium_leg(1, "long", "CE", rule, x)])));
        let a = hook(&t, &token, json!({"action": "long_entry"})).await;
        assert_eq!(a.result, "ok", "{} {}: {}", rule, x, a.message);
        let p = t.gw.placed();
        assert_eq!(p.len(), 1);
        assert_eq!(
            (p[0].symbol.as_str(), p[0].action.as_str()),
            (want, "BUY"),
            "{} {}",
            rule,
            x
        );
        // The exit closes that exact contract.
        fill_last_entry(&t, a.run_id.unwrap()).await;
        let c = hook(&t, &token, json!({"action": "long_exit"})).await;
        assert_eq!(c.result, "ok", "{}", c.message);
        assert_eq!(t.gw.placed()[1].symbol, want);
    }
}

#[tokio::test]
async fn no_quotes_or_no_qualifying_strike_places_nothing() {
    // No option quotes at all.
    let t = t();
    let (_, token) = t.make_with_token(index_signal(json!([premium_leg(
        1, "long", "CE", "gte", 100.0
    )])));
    let a = hook(&t, &token, json!({"action": "long_entry"})).await;
    assert_ne!(a.result, "ok");
    assert!(a.message.contains("Nothing was placed"), "{}", a.message);
    assert!(t.gw.placed().is_empty());

    // Quotes, but nothing as dear as asked.
    let t2 = super::support::t();
    price_calls(&t2);
    let (_, token) = t2.make_with_token(index_signal(json!([premium_leg(
        1, "long", "CE", "gte", 5000.0
    )])));
    let a = hook(&t2, &token, json!({"action": "long_entry"})).await;
    assert_ne!(a.result, "ok");
    assert!(a.message.contains("≥ ₹5000"), "{}", a.message);
    assert!(t2.gw.placed().is_empty());
}

// ---- The Bridge 1.0.7: plain-text alert messages ----

async fn text(
    t: &T,
    token: &str,
    body: &str,
) -> openalgo_desktop_lib::strategy::webhook::WebhookOutcome {
    t.m.handle_webhook(
        token,
        body.as_bytes(),
        Some("10.0.0.1"),
        Some("TradingView"),
    )
    .await
}

async fn fill_open_orders(t: &T, run: i64) {
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

const ATM_CE_13: &str = "NIFTY13OCT2624500CE";

/// One strategy, four legs: Long buys the call and sells the put, Short
/// buys the put and sells the call (the backtest's spread, at ATM here).
fn four_legs() -> Value {
    index_signal(json!([
        option_leg(1, "long", "B", "CE"),
        option_leg(2, "long", "S", "PE"),
        option_leg(3, "short", "B", "PE"),
        option_leg(4, "short", "S", "CE"),
    ]))
}

#[tokio::test]
async fn arkos_messages_drive_four_legs_in_tradingview_order() {
    let t = t();
    let (_, token) = t.make_with_token(four_legs());
    let a = text(&t, &token, "BUY").await;
    assert_eq!(a.result, "ok", "{}", a.message);
    let run = a.run_id.unwrap();
    let p = t.gw.placed();
    assert_eq!(p.len(), 2);
    assert_eq!(
        (p[0].symbol.as_str(), p[0].action.as_str()),
        (ATM_CE_13, "BUY")
    );
    assert_eq!(
        (p[1].symbol.as_str(), p[1].action.as_str()),
        (ATM_PE, "SELL")
    );
    fill_open_orders(&t, run).await;

    // Same bar as a Supertrend flip: Pine sends SELL before CLOSE LONG.
    let b = text(&t, &token, "SELL").await;
    assert_eq!(b.result, "ok", "{}", b.message);
    let p = t.gw.placed();
    assert_eq!(p.len(), 6, "close CE, close PE, buy PE, sell CE");
    assert_eq!(
        (p[2].symbol.as_str(), p[2].action.as_str()),
        (ATM_CE_13, "SELL")
    );
    assert_eq!(
        (p[3].symbol.as_str(), p[3].action.as_str()),
        (ATM_PE, "BUY")
    );
    assert_eq!(
        (p[4].symbol.as_str(), p[4].action.as_str()),
        (ATM_PE, "BUY")
    );
    assert_eq!(
        (p[5].symbol.as_str(), p[5].action.as_str()),
        (ATM_CE_13, "SELL")
    );
    fill_open_orders(&t, run).await;

    // The late CLOSE LONG is a harmless no-op.
    let c = text(&t, &token, "CLOSE LONG: Normal Exit").await;
    assert_eq!(c.result, "ok", "{}", c.message);
    assert_eq!(t.gw.placed().len(), 6);

    let d = text(&t, &token, "close short: Trailing Profit\n").await;
    assert_eq!(d.result, "ok", "{}", d.message);
    let p = t.gw.placed();
    assert_eq!(p.len(), 8);
    assert_eq!(
        (p[6].symbol.as_str(), p[6].action.as_str()),
        (ATM_PE, "SELL")
    );
    assert_eq!(
        (p[7].symbol.as_str(), p[7].action.as_str()),
        (ATM_CE_13, "BUY")
    );
}

#[tokio::test]
async fn unmapped_text_is_refused_and_custom_rules_apply() {
    let t = t();
    let (_, token) = t.make_with_token(four_legs());
    let a = text(&t, &token, "LONG ENTRY").await;
    assert_eq!(a.result, "rejected_payload");
    assert!(t.gw.placed().is_empty());

    let mut cfg = four_legs();
    cfg["name"] = json!("Custom messages");
    cfg["webhook_messages"] = json!([
        {"match": "LONG ENTRY", "action": "long_entry"},
        {"match": "LONG EXIT", "action": "long_exit", "prefix": true},
    ]);
    let (_, token) = t.make_with_token(cfg);
    let b = text(&t, &token, "long entry").await;
    assert_eq!(b.result, "ok", "{}", b.message);
    assert_eq!(t.gw.placed().len(), 2);
    // BUY is not one of this strategy's messages.
    let c = text(&t, &token, "BUY").await;
    assert_eq!(c.result, "rejected_payload");
    // JSON still works alongside.
    fill_open_orders(&t, b.run_id.unwrap()).await;
    let d = hook(&t, &token, json!({"action": "long_exit"})).await;
    assert_eq!(d.result, "ok", "{}", d.message);
}

#[test]
fn bad_message_rules_are_refused() {
    let t = t();
    let g = t.symbols.snapshot();
    let mut cfg = four_legs();
    cfg["webhook_messages"] = json!([{"match": "BUY", "action": "buy"}]);
    assert!(validate_strategy_config(&cfg, &g)
        .unwrap_err()
        .contains("webhook_messages"));
}
