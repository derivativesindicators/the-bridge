//! The Detail page's books and totals per strategy (strategy/views.rs):
//! attribution by the mode of the run that placed each order (SM-03, SM-05).

use super::*;
use openalgo_desktop_lib::strategy::store::NewOrder;

const X: &str = "NIFTY28MAY2625000CE";
const Y: &str = "NIFTY28MAY2625500CE";

/// A complete BUY of 65 on `symbol` in `run`, at the strategy's product.
fn filled_buy(t: &T, run: i64, symbol: &str, product: &str, broker_id: &str) {
    let id =
        t.m.store
            .record_order(
                run,
                1,
                "entry",
                &NewOrder {
                    symbol: symbol.into(),
                    exchange: "NFO".into(),
                    action: "BUY".into(),
                    qty: 65,
                    product: Some(product.into()),
                    pricetype: "MARKET".into(),
                    status: "open".into(),
                    position_ref: None,
                    broker_order_id: Some(broker_id.into()),
                },
            )
            .unwrap();
    t.m.store
        .update_order(id, Some("complete"), None, None)
        .unwrap();
}

/// A run of `mode`, linked to the strategy as its current run.
fn open_run(t: &T, sid: i64, mode: &str) -> i64 {
    let run =
        t.m.store
            .create_run(
                sid,
                mode,
                &openalgo_desktop_lib::services::live_owner::LiveOwner::new(mode, ""),
                "manual",
                None,
                None,
            )
            .unwrap();
    assert!(t
        .m
        .store
        .set_strategy_status(sid, "running", Some(run))
        .unwrap());
    run
}

/// Finish the current `run` with `pnl` realized.
fn finish(t: &T, sid: i64, run: i64, pnl: f64) {
    assert!(t
        .m
        .store
        .finish_run_and_release_strategy(run, sid, "manual", pnl, pnl, pnl)
        .unwrap());
}

fn position(symbol: &str, product: &str, pnl: f64) -> Value {
    json!({
        "symbol": symbol, "exchange": "NFO", "product": product,
        "quantity": 65, "average_price": 100.0, "ltp": 101.0, "pnl": pnl,
    })
}

/// A finished Sandbox run left X open; the Live run that followed traded Y,
/// and the Live book also holds X from outside this strategy.
fn sandbox_residual_then_live(t: &T) -> (i64, i64, i64) {
    let sid = t.default_strategy();
    let product = t.row(sid).product;
    let sandbox = open_run(t, sid, "sandbox");
    filled_buy(t, sandbox, X, &product, "SB-1");
    finish(t, sid, sandbox, 0.0);
    let live = open_run(t, sid, "live");
    filled_buy(t, live, Y, &product, "L-1");
    *t.gw.position_rows.lock() = vec![position(X, &product, 500.0), position(Y, &product, 40.0)];
    (sid, sandbox, live)
}

#[tokio::test]
async fn a_sandbox_residual_never_owns_a_row_of_the_live_book() {
    let t = t();
    let (sid, sandbox, live) = sandbox_residual_then_live(&t);
    // The run named explicitly, and the latest-run default.
    for run_id in [Some(live), None] {
        let book = t.m.strategy_positions(sid, run_id).await.unwrap();
        let rows = book["data"].as_array().unwrap();
        assert!(
            rows.iter().all(|r| r["run_id"] != json!(sandbox)),
            "a Live row carries the Sandbox run: {:?}",
            rows
        );
        assert!(
            rows.iter().all(|r| r["symbol"] != X),
            "the Sandbox residual's contract is in the Live book: {:?}",
            rows
        );
        assert_eq!(rows.len(), 1, "{:?}", rows);
        assert_eq!(rows[0]["run_id"], json!(live));
        // Only this strategy's Live row counts in the totals.
        assert_eq!(book["total_pnl"], json!(40.0), "{}", book);
    }
}

#[test]
fn realized_pnl_is_summed_per_mode_over_every_finalised_run() {
    let t = t();
    let sid = t.default_strategy();
    // An older Live run, then more Sandbox runs than the page loads (100).
    let run = open_run(&t, sid, "live");
    finish(&t, sid, run, 250.0);
    for _ in 0..101 {
        let run = open_run(&t, sid, "sandbox");
        finish(&t, sid, run, -1.5);
    }
    // A run still open adds nothing yet: its realized is written when it ends.
    open_run(&t, sid, "live");
    let detail = t.m.strategy_detail(&t.row(sid)).unwrap();
    assert_eq!(
        detail["realized_by_mode"],
        json!({"live": 250.0, "sandbox": -151.5})
    );
    assert_eq!(detail["id"], json!(sid), "the strategy record is kept");
}

/// The Bridge 1.0.10: the Strategy desk lists each mode's open positions
/// across strategies, with MTM from the book's LTP, and only that mode's.
#[tokio::test]
async fn the_desk_lists_open_positions_of_one_mode_across_strategies() {
    use openalgo_desktop_lib::strategy::desk::DeskStrategy;
    use openalgo_desktop_lib::strategy::dispatch::RunMode;
    let t = t();
    let (sid, _sandbox, _live) = sandbox_residual_then_live(&t);
    let row = t.row(sid);
    let list = || {
        vec![DeskStrategy {
            id: sid,
            name: row.name.clone(),
            status: row.status.clone(),
            product: row.product.clone(),
            run_mode: None,
        }]
    };
    let live = t.m.desk(list(), RunMode::Live).await.unwrap();
    let s = &live["data"]["strategies"][0];
    assert_eq!(s["id"], json!(sid));
    let p = s["positions"].as_array().unwrap();
    assert_eq!(p.len(), 1, "{:?}", p);
    assert_eq!(p[0]["symbol"], Y);
    assert_eq!(p[0]["side"], "long");
    // (101 - 100) x 65
    assert_eq!(p[0]["mtm"], json!(65.0));
    assert_eq!(live["data"]["totals"]["unrealized"], json!(65.0));

    let sandbox = t.m.desk(list(), RunMode::Sandbox).await.unwrap();
    let p = sandbox["data"]["strategies"][0]["positions"].as_array().unwrap().clone();
    assert_eq!(p.len(), 1, "{:?}", p);
    assert_eq!(p[0]["symbol"], X);
}
