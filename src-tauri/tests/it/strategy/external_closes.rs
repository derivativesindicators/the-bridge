//! The Bridge 1.0.11: legs closed outside the strategy (expiry settlement,
//! MIS auto square-off, an app-wide close all) are closed without an order.

use super::*;
use openalgo_desktop_lib::strategy::dispatch::RunMode;
use openalgo_desktop_lib::strategy::recovery::recover_run;
use openalgo_desktop_lib::strategy::store::StrategyRow;

const FUT: &str = "RELIANCE27OCT26FUT";

fn fut_leg() -> Value {
    json!({"id": 1, "symbol": FUT, "exchange": "NFO", "side": "both",
           "qty": 2, "qty_mode": "lots", "segment": "futures", "expiry": "current"})
}

fn strategy(t: &T, name: &str) -> StrategyRow {
    let sid = t.make(signal_config(
        json!([fut_leg()]),
        json!({"name": name, "product": "NRML"}),
    ));
    t.m.store.get_strategy(sid, USER).unwrap().unwrap()
}

async fn signal(
    t: &T,
    s: &StrategyRow,
    action: &str,
) -> openalgo_desktop_lib::strategy::signals::SignalResult {
    let fresh = t.m.store.get_strategy(s.id, USER).unwrap().unwrap();
    t.m.handle_signal(&fresh, action, Some(&json!(1)), None, None)
        .await
}

fn run_of(t: &T, s: &StrategyRow) -> i64 {
    t.m.store
        .get_strategy(s.id, USER)
        .unwrap()
        .unwrap()
        .current_run_id
        .unwrap()
}

async fn long_filled(t: &T, s: &StrategyRow, price: f64) -> i64 {
    assert!(signal(t, s, "long_entry").await.acted());
    let run = run_of(t, s);
    let o = t.orders(run)[0].clone();
    t.frame(
        o.broker_order_id.as_deref().unwrap(),
        "complete",
        o.qty,
        price,
    )
    .await;
    assert_eq!(t.leg(run, 1).status, "open");
    run
}

fn book_row(qty: i64, ltp: f64) -> Value {
    json!({"symbol": FUT, "exchange": "NFO", "product": "NRML", "quantity": qty, "ltp": ltp})
}

fn events(t: &T, s: &StrategyRow, kind: &str) -> Vec<Value> {
    t.m.store
        .list_events(s.id, None, Some(kind), None, 10)
        .unwrap()
}

#[tokio::test]
async fn an_expired_sandbox_leg_settles_at_the_sandbox_price_with_no_order() {
    let t = t();
    let s = strategy(&t, "Expiring");
    let run = long_filled(&t, &s, 1000.0).await;
    let mode = t.m.mode_of_run(run);
    // The sandbox settled the contract at 1050 (its row: flat, LTP 1050).
    *t.gw.position_rows.lock() = vec![book_row(0, 1050.0)];
    t.clock.set(ist(2026, 10, 28, 9, 20));
    assert_eq!(t.m.settle_expired_legs(run).await, 1);
    let leg = t.leg(run, 1);
    assert_eq!(leg.status, "closed");
    if mode == RunMode::Sandbox {
        assert!(
            (leg.realized_pnl - 50.0 * 1000.0).abs() < 1e-6,
            "{}",
            leg.realized_pnl
        );
    }
    assert_eq!(
        t.gw.placed().len(),
        1,
        "no exit order for an expired contract"
    );
    let orders = t.orders(run);
    assert_eq!(orders[1].kind, "exit_expiry");
    assert_eq!(orders[1].status, "complete");
    assert_eq!(events(&t, &s, "expiry_settled").len(), 1);
    // A signal run stays open and flat; a later CLOSE LONG is a no-op.
    assert!(t.run(run).stopped_at.is_none());
    let r = signal(&t, &s, "long_exit").await;
    assert_eq!(r.note.as_deref(), Some("no_matching_position"));
    assert_eq!(t.gw.placed().len(), 1);
}

#[tokio::test]
async fn a_restart_after_expiry_settlement_sends_no_order() {
    let t = t();
    let s = strategy(&t, "Expiring restart");
    let run = long_filled(&t, &s, 1000.0).await;
    *t.gw.position_rows.lock() = vec![book_row(0, 1050.0)];
    t.clock.set(ist(2026, 10, 28, 9, 20));
    assert_eq!(t.m.settle_expired_legs(run).await, 1);
    // Restart: the durable rows rebuild the leg closed, so the flat run is
    // finished like any other flat run and nothing is sent.
    t.m.state.clear(run);
    let rec = recover_run(&t.m, run).await;
    assert!(rec.ok || rec.finalised, "{:?}", rec);
    assert_eq!(rec.open_legs, 0);
    assert_eq!(t.gw.placed().len(), 1);
}

#[tokio::test]
async fn an_exit_for_an_expired_contract_is_never_sent() {
    let t = t();
    let s = strategy(&t, "Expiring exit");
    let run = long_filled(&t, &s, 1000.0).await;
    t.clock.set(ist(2026, 10, 28, 9, 20));
    // A close, a stop or a signal: none sends an order for the expired leg.
    let r = t.m.close_open_legs(run, USER).await;
    assert!(r.ok, "{:?}", r);
    assert_eq!(t.gw.placed().len(), 1);
    assert_eq!(t.leg(run, 1).status, "closed");
    assert_eq!(t.orders(run)[1].kind, "exit_expiry");
}

#[tokio::test]
async fn a_contract_closed_outside_the_strategy_closes_the_leg_at_the_fill() {
    let t = t();
    let s = strategy(&t, "Outside close");
    let run = long_filled(&t, &s, 1000.0).await;
    let mode = t.m.mode_of_run(run);
    // Still held: nothing changes.
    *t.gw.position_rows.lock() = vec![book_row(1000, 1010.0)];
    assert!(t.m.reconcile_positions(mode).await.is_empty());
    assert_eq!(t.leg(run, 1).status, "open");
    // Closed from the Positions page: the book is flat, the last trade 1020.
    *t.gw.position_rows.lock() = vec![book_row(0, 1020.0)];
    *t.gw.trade_rows.lock() = vec![json!({"symbol": FUT, "exchange": "NFO", "product": "NRML",
        "action": "SELL", "quantity": 1000, "average_price": 1020.0, "strategy": "UI Exit Position"})];
    let closed = t.m.reconcile_positions(mode).await;
    assert_eq!(closed.len(), 1, "{:?}", closed);
    assert_eq!(closed[0]["reason"], "external_close");
    let leg = t.leg(run, 1);
    assert_eq!(leg.status, "closed");
    assert!((leg.realized_pnl - 20.0 * 1000.0).abs() < 1e-6);
    assert_eq!(t.gw.placed().len(), 1);
    assert_eq!(t.orders(run)[1].kind, "exit_external");
    assert_eq!(events(&t, &s, "external_close").len(), 1);
    // Idempotent; and the next BUY opens fresh.
    assert!(t.m.reconcile_positions(mode).await.is_empty());
    assert!(signal(&t, &s, "long_entry").await.acted());
    assert_eq!(t.gw.actions(), vec!["BUY", "BUY"]);
}

#[tokio::test]
async fn the_mis_auto_square_off_is_recorded_as_such() {
    let t = t();
    let s = strategy(&t, "Squared");
    let run = long_filled(&t, &s, 1000.0).await;
    let mode = t.m.mode_of_run(run);
    *t.gw.trade_rows.lock() = vec![json!({"symbol": FUT, "exchange": "NFO", "product": "NRML",
        "action": "SELL", "quantity": 1000, "average_price": 990.0, "strategy": "AUTO_SQUARE_OFF"})];
    let closed = t.m.reconcile_positions(mode).await;
    assert_eq!(closed[0]["reason"], "auto_squareoff");
    assert_eq!(t.orders(run)[1].kind, "exit_auto_squareoff");
    assert!((t.leg(run, 1).realized_pnl + 10.0 * 1000.0).abs() < 1e-6);
    assert_eq!(events(&t, &s, "auto_squareoff").len(), 1);
}

#[tokio::test]
async fn offsetting_strategies_on_a_flat_book_are_left_alone() {
    let t = t();
    let a = strategy(&t, "A");
    let b = strategy(&t, "B");
    let ra = long_filled(&t, &a, 1000.0).await;
    assert!(signal(&t, &b, "short_entry").await.acted());
    let rb = run_of(&t, &b);
    t.m.apply_fill(rb, 1, Some(1000.0), true, FillOpts::default())
        .await;
    let mode = t.m.mode_of_run(ra);
    *t.gw.position_rows.lock() = vec![book_row(0, 1000.0)];
    assert!(t.m.reconcile_positions(mode).await.is_empty());
    assert_eq!(t.leg(ra, 1).status, "open");
    assert_eq!(t.leg(rb, 1).status, "open");
}
