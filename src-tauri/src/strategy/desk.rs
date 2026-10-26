//! The Bridge 1.0.10: the Strategy desk. Every open position of every
//! strategy in one mode (Sandbox by default, or Live), with live LTP and MTM
//! from that mode's book, per-strategy subtotals, today's realized P&L from
//! the strategy's own fills, and the round trips closed today.
//!
//! Positions come from the same book and attribution as each strategy's
//! Detail page (`views.rs`): the book of the mode, filtered to the contracts
//! the strategy's own orders in that mode still hold. A contract held by two
//! strategies is one broker row and is shown under each (marked shared).

use super::dispatch::{Book, RunMode};
use super::store::{ModedOrder, OrderRow};
use super::views::{filled_quantity, number, overlay, placed_in, residual_owners, rows, text};
use super::StrategyModule;
use crate::error::AppError;
use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// One strategy as the desk lists it.
#[derive(Debug, Clone)]
pub struct DeskStrategy {
    pub id: i64,
    pub name: String,
    pub status: String,
    pub product: String,
    /// The mode of the current (unstopped) run, when running.
    pub run_mode: Option<String>,
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// The IST calendar date of a stored UTC timestamp (`YYYY-MM-DD HH:MM:SS.ffffff`).
pub fn ist_date(stored_utc: &str) -> Option<NaiveDate> {
    let t = stored_utc.trim();
    let naive = NaiveDateTime::parse_from_str(t, "%Y-%m-%d %H:%M:%S%.f")
        .or_else(|_| NaiveDateTime::parse_from_str(t, "%Y-%m-%dT%H:%M:%S%.f"))
        .ok()
        .or_else(|| chrono::DateTime::parse_from_rfc3339(t).ok().map(|d| d.naive_utc()))?;
    Some(
        Utc.from_utc_datetime(&naive)
            .with_timezone(&chrono_tz::Asia::Kolkata)
            .date_naive(),
    )
}

fn fill_price(o: &OrderRow) -> Option<f64> {
    o.avg_fill_price.filter(|p| *p > 0.0)
}

/// Round trips (entry matched FIFO to exit, per run and leg) whose exit was
/// filled on `today` (IST), newest first, from the orders placed in `mode`.
pub fn closed_on(history: &[ModedOrder], mode: RunMode, today: NaiveDate) -> Vec<Value> {
    let mut by_leg: BTreeMap<(i64, i64), Vec<&OrderRow>> = BTreeMap::new();
    for o in placed_in(history, mode) {
        if filled_quantity(o) > 0 && fill_price(o).is_some() {
            by_leg.entry((o.run_id, o.leg_id)).or_default().push(o);
        }
    }
    let mut trips = Vec::new();
    for ((run_id, leg_id), mut list) in by_leg {
        let when = |o: &OrderRow| o.filled_at.clone().unwrap_or_else(|| o.placed_at.clone());
        list.sort_by(|a, b| (when(a), a.id).cmp(&(when(b), b.id)));
        // (side +1 long / -1 short, qty, price, symbol, entry time)
        let mut open: Vec<(i64, i64, f64)> = Vec::new();
        for o in list {
            let side = if o.action.eq_ignore_ascii_case("BUY") { 1 } else { -1 };
            let price = fill_price(o).unwrap_or(0.0);
            let mut remaining = filled_quantity(o);
            let mut realized = 0.0;
            let mut matched_total = 0;
            let mut entry_value = 0.0;
            let mut lot_side = 0;
            while remaining > 0 && !open.is_empty() && open[0].0 != side {
                let m = remaining.min(open[0].1);
                realized += (price - open[0].2) * m as f64 * open[0].0 as f64;
                entry_value += open[0].2 * m as f64;
                lot_side = open[0].0;
                matched_total += m;
                open[0].1 -= m;
                remaining -= m;
                if open[0].1 <= 0 {
                    open.remove(0);
                }
            }
            if remaining > 0 {
                open.push((side, remaining, price));
            }
            if matched_total > 0 && ist_date(&when(o)) == Some(today) {
                trips.push(json!({
                    "run_id": run_id, "leg_id": leg_id,
                    "symbol": o.symbol, "exchange": o.exchange,
                    "side": if lot_side > 0 { "long" } else { "short" },
                    "qty": matched_total,
                    "entry_price": round2(entry_value / matched_total as f64),
                    "exit_price": price,
                    "exit_time": when(o),
                    "exit_kind": o.kind,
                    "pnl": round2(realized),
                }));
            }
        }
    }
    trips.sort_by(|a, b| text(&b["exit_time"]).cmp(&text(&a["exit_time"])));
    trips
}

impl StrategyModule {
    /// Lots, expiry, strike and instrument type from the master contract.
    fn describe(&self, row: &mut Value) {
        let qty = number(&row["quantity"]) as i64;
        if let Some(t) = self.symbols.by_symbol(&text(&row["exchange"]), &text(&row["symbol"])) {
            let lot = i64::from(t.lot_size.max(1));
            row["lot_size"] = json!(lot);
            row["lots"] = json!((qty.abs() as f64 / lot as f64 * 100.0).round() / 100.0);
            row["expiry"] = json!(t.expiry);
            row["strike"] = if t.strike > 0.0 { json!(t.strike) } else { Value::Null };
            row["instrument_type"] = json!(t.instrument_type);
        } else {
            row["lot_size"] = Value::Null;
            row["lots"] = Value::Null;
        }
        row["side"] = json!(if qty > 0 { "long" } else if qty < 0 { "short" } else { "flat" });
    }

    /// The desk for `mode`: one book read, every strategy's share of it.
    /// A book the broker could not give is an error envelope; a local read
    /// failure is `Err`.
    pub async fn desk(&self, strategies: Vec<DeskStrategy>, mode: RunMode) -> Result<Value, AppError> {
        let today = Utc::now().with_timezone(&chrono_tz::Asia::Kolkata).date_naive();
        let book = match self.gateway.book(mode, Book::Positions).await {
            Ok(p) => Some(rows(&p["data"])),
            Err(_) => None,
        };
        // Every strategy's own residuals first, so a contract held by more
        // than one owner (two strategies, or two legs) is known before any
        // broker row is attributed: the broker nets such a contract into one
        // row that belongs to none of them (A long 1 lot + B short 1 lot is a
        // flat row), so each owner keeps its OWN quantity and average, and
        // only the LTP is read from the book.
        let mut gathered = Vec::new();
        for s in strategies {
            let history = self.order_history(s.id, None).await?;
            if !history.iter().any(|o| o.mode == mode.as_str()) {
                continue;
            }
            let product = s.product.to_ascii_uppercase();
            let owners = residual_owners(placed_in(&history, mode), &product);
            gathered.push((s, history, product, owners));
        }
        let holders = owner_counts(gathered.iter().map(|g| &g.3));
        let net = {
            let fills: Vec<&OrderRow> = gathered.iter().flat_map(|g| placed_in(&g.1, mode)).collect();
            let mut rows = net_positions(&fills, today);
            for r in rows.iter_mut() {
                self.describe(r);
                let row = book.as_deref().and_then(|b| b.iter().find(|x| {
                    text(&x["symbol"]).eq_ignore_ascii_case(&text(&r["symbol"]))
                        && text(&x["exchange"]).eq_ignore_ascii_case(&text(&r["exchange"]))
                        && {
                            let p = text(&x["product"]).to_ascii_uppercase();
                            p.is_empty() || p == text(&r["product"])
                        }
                }));
                if let Some(x) = row {
                    r["ltp"] = x["ltp"].clone();
                    if mode == RunMode::Live {
                        let bq = number(&x["quantity"]);
                        r["broker_quantity"] = json!(bq);
                        r["mismatch"] = json!(bq != number(&r["quantity"]));
                    }
                } else if mode == RunMode::Live && number(&r["quantity"]) != 0.0 && book.is_some() {
                    r["broker_quantity"] = json!(0);
                    r["mismatch"] = json!(true);
                }
                let (q, ltp) = (number(&r["quantity"]), r["ltp"].clone());
                if q != 0.0 && !ltp.is_null() {
                    let u = round2((number(&ltp) - number(&r["average_price"])) * q);
                    r["unrealized"] = json!(u);
                    r["pnl"] = json!(round2(u + number(&r["realized"])));
                    let avg = number(&r["average_price"]);
                    r["change_percent"] = if avg > 0.0 { json!(round2((number(&ltp) - avg) / avg * 100.0)) } else { Value::Null };
                }
            }
            rows
        };
        let net_total: f64 = net.iter().map(|r| number(&r["pnl"])).sum();
        let mut out = Vec::new();
        let (mut t_real, mut t_unreal) = (0.0, 0.0);
        for (s, history, product, owners) in gathered {
            let mut positions = attribute(owners, book.as_deref(), &product, &holders);
            positions.retain(|r| number(&r["quantity"]) != 0.0);
            for p in positions.iter_mut() {
                self.describe(p);
                // Mark-to-market of the open quantity from the book's LTP.
                let (q, avg, ltp) = (number(&p["quantity"]), &p["average_price"], &p["ltp"]);
                if !avg.is_null() && !ltp.is_null() {
                    p["mtm"] = json!(round2((number(ltp) - number(avg)) * q));
                } else {
                    p["mtm"] = Value::Null;
                }
            }
            let closed = closed_on(&history, mode, today);
            let realized: f64 = closed.iter().map(|t| number(&t["pnl"])).sum();
            let unrealized: f64 = positions.iter().map(|p| number(&p["mtm"])).sum();
            if positions.is_empty() && closed.is_empty() {
                continue;
            }
            t_real += realized;
            t_unreal += unrealized;
            out.push(json!({
                "id": s.id, "name": s.name, "status": s.status, "product": product,
                "run_mode": s.run_mode,
                "positions": positions, "closed_today": closed,
                "realized": round2(realized), "unrealized": round2(unrealized),
                "total": round2(realized + unrealized),
            }));
        }
        Ok(json!({"status": "success", "data": {
            "mode": mode.as_str(),
            "book_available": book.is_some(),
            "strategies": out,
            "net": net,
            "net_total": round2(net_total),
            "totals": {"realized": round2(t_real), "unrealized": round2(t_unreal), "total": round2(t_real + t_unreal)},
        }}))
    }
}

fn contract(row: &Value) -> (String, String, String) {
    (
        text(&row["symbol"]).to_ascii_uppercase(),
        text(&row["exchange"]).to_ascii_uppercase(),
        text(&row["product"]).to_ascii_uppercase(),
    )
}

/// How many owners (strategy legs, across every strategy of the mode) hold
/// each contract.
pub fn owner_counts<'a>(
    owners: impl IntoIterator<Item = &'a Vec<Value>>,
) -> BTreeMap<(String, String, String), usize> {
    let mut n = BTreeMap::new();
    for list in owners {
        for o in list {
            *n.entry(contract(o)).or_insert(0) += 1;
        }
    }
    n
}

/// One strategy's open positions: a contract it alone holds is the broker
/// row (the authority for money); a contract held by any other owner keeps
/// this owner's own quantity and average from its fills, priced at the
/// book's LTP and marked `local/shared`, because the netted broker row
/// cannot be divided (and may read flat while both strategies are open).
pub fn attribute(
    owners: Vec<Value>,
    book: Option<&[Value]>,
    product: &str,
    holders: &BTreeMap<(String, String, String), usize>,
) -> Vec<Value> {
    let (shared, exclusive): (Vec<Value>, Vec<Value>) = owners
        .into_iter()
        .partition(|o| holders.get(&contract(o)).copied().unwrap_or(0) > 1);
    let mut out = if exclusive.is_empty() {
        Vec::new()
    } else {
        match book {
            Some(b) => overlay(b.to_vec(), exclusive, product),
            None => exclusive,
        }
    };
    for mut o in shared {
        let key = contract(&o);
        if let Some(row) = book.and_then(|b| b.iter().find(|r| {
            let (s, e, p) = contract(r);
            (s.as_str(), e.as_str()) == (key.0.as_str(), key.1.as_str()) && (p.is_empty() || p == key.2)
        })) {
            o["ltp"] = row["ltp"].clone();
        }
        o["source"] = json!("local/shared");
        o["shared"] = json!(true);
        out.push(o);
    }
    out
}

/// The Bridge 1.0.11: Kite-style net positions. Every strategy's fills of
/// one mode, netted by contract (symbol + exchange + product) the way a
/// broker nets an account: fills in time order; adding to the net side
/// re-averages, reducing keeps the average and books realized P&L, crossing
/// zero books the closed part and opens the rest at the fill price. No
/// strategy names. A contract netted to zero is listed (qty 0, realized)
/// only when it traded `today` (IST). `realized` is today's realized P&L.
pub fn net_positions(fills: &[&OrderRow], today: NaiveDate) -> Vec<Value> {
    #[derive(Default)]
    struct Net {
        qty: i64,
        avg: f64,
        realized_today: f64,
        traded_today: bool,
        buy_qty: i64,
        sell_qty: i64,
    }
    let mut list: Vec<&OrderRow> = fills
        .iter()
        .copied()
        .filter(|o| filled_quantity(o) > 0 && fill_price(o).is_some())
        .collect();
    let when = |o: &OrderRow| o.filled_at.clone().unwrap_or_else(|| o.placed_at.clone());
    list.sort_by(|a, b| (when(a), a.id).cmp(&(when(b), b.id)));
    let mut nets: BTreeMap<(String, String, String), Net> = BTreeMap::new();
    for o in list {
        let key = (
            o.symbol.to_ascii_uppercase(),
            o.exchange.to_ascii_uppercase(),
            o.product.clone().unwrap_or_default().to_ascii_uppercase(),
        );
        let n = nets.entry(key).or_default();
        let q = filled_quantity(o);
        let signed = if o.action.eq_ignore_ascii_case("BUY") { q } else { -q };
        let price = fill_price(o).unwrap_or(0.0);
        let is_today = ist_date(&when(o)) == Some(today);
        if is_today {
            n.traded_today = true;
            if signed > 0 { n.buy_qty += q } else { n.sell_qty += q }
        }
        let mut realized = 0.0;
        if n.qty == 0 || n.qty.signum() == signed.signum() {
            let total = n.qty + signed;
            n.avg = (n.avg * n.qty.abs() as f64 + price * signed.abs() as f64) / total.abs() as f64;
            n.qty = total;
        } else {
            let closing = signed.abs().min(n.qty.abs());
            realized = (price - n.avg) * closing as f64 * n.qty.signum() as f64;
            let total = n.qty + signed;
            if total == 0 {
                n.avg = 0.0;
            } else if total.signum() != n.qty.signum() {
                n.avg = price;
            }
            n.qty = total;
        }
        if is_today {
            n.realized_today += realized;
        }
    }
    nets.into_iter()
        .filter(|(_, n)| n.qty != 0 || n.traded_today)
        .map(|((symbol, exchange, product), n)| {
            json!({
                "symbol": symbol, "exchange": exchange,
                "product": if product.is_empty() { "NRML".to_string() } else { product },
                "quantity": n.qty,
                "average_price": if n.qty != 0 { json!(round2(n.avg * 10000.0) / 10000.0) } else { Value::Null },
                "realized": round2(n.realized_today),
                "buy_quantity": n.buy_qty, "sell_quantity": n.sell_qty,
                "ltp": Value::Null, "unrealized": Value::Null, "pnl": round2(n.realized_today),
            })
        })
        .collect()
}

/// Margin request rows (the `/api/v1/margin` shape) for open positions:
/// one per contract, netted across strategies.
pub fn margin_positions(positions: &[Value]) -> Vec<Value> {
    let mut net: BTreeMap<(String, String, String), i64> = BTreeMap::new();
    for p in positions {
        let q = number(&p["quantity"]) as i64;
        if q == 0 {
            continue;
        }
        *net.entry((text(&p["symbol"]), text(&p["exchange"]), text(&p["product"]).to_ascii_uppercase()))
            .or_default() += q;
    }
    net.into_iter()
        .filter(|(_, q)| *q != 0)
        .map(|((symbol, exchange, product), q)| {
            json!({
                "symbol": symbol, "exchange": exchange,
                "action": if q > 0 { "BUY" } else { "SELL" },
                "quantity": q.abs().to_string(),
                "product": if product.is_empty() { "NRML".to_string() } else { product },
                "pricetype": "MARKET", "price": "0", "trigger_price": "0",
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(id: i64, leg: i64, kind: &str, action: &str, qty: i64, price: f64, at: &str) -> ModedOrder {
        ModedOrder {
            order: OrderRow {
                id, run_id: 1, leg_id: leg, kind: kind.into(), position_ref: None,
                broker_order_id: Some(format!("b{}", id)), symbol: "NIFTY28OCT2625000CE".into(),
                exchange: "NFO".into(), action: action.into(), qty, product: Some("NRML".into()),
                pricetype: "MARKET".into(), price: 0.0, trigger_price: 0.0, status: "complete".into(),
                placed_at: at.into(), filled_at: Some(at.into()), avg_fill_price: Some(price),
                filled_qty: Some(qty), reject_reason: None,
            },
            mode: "sandbox".into(),
        }
    }

    #[test]
    fn ist_dates_cross_midnight_utc() {
        assert_eq!(ist_date("2026-10-25 19:00:00.000000"), NaiveDate::from_ymd_opt(2026, 10, 26));
        assert_eq!(ist_date("2026-10-26 03:45:00.123456"), NaiveDate::from_ymd_opt(2026, 10, 26));
        assert_eq!(ist_date("junk"), None);
    }

    #[test]
    fn closed_today_counts_only_exits_filled_today() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 26).unwrap();
        let h = vec![
            order(1, 1, "entry", "BUY", 75, 100.0, "2026-10-26 04:00:00.000000"),
            order(2, 1, "exit_target", "SELL", 75, 120.0, "2026-10-26 05:00:00.000000"),
            order(3, 2, "entry", "SELL", 75, 50.0, "2026-10-24 04:00:00.000000"),
            order(4, 2, "exit_sl", "BUY", 75, 60.0, "2026-10-24 05:00:00.000000"),
            order(5, 3, "entry", "BUY", 75, 10.0, "2026-10-26 04:00:00.000000"),
        ];
        let c = closed_on(&h, RunMode::Sandbox, today);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0]["pnl"], json!(1500.0));
        assert_eq!(c[0]["side"], "long");
        // Live view sees none of the sandbox orders.
        assert!(closed_on(&h, RunMode::Live, today).is_empty());
    }

    fn owner(run: i64, qty: i64, avg: f64) -> Value {
        json!({"symbol": "NIFTY27OCT2623000CE", "exchange": "NFO", "product": "NRML",
            "quantity": qty, "average_price": avg, "ltp": null, "source": "local/unreconciled",
            "run_id": run, "leg_id": 1})
    }

    #[test]
    fn opposite_strategies_on_one_contract_keep_their_own_quantities() {
        // Strategy A long 1 lot, B short 1 lot: the broker row nets to flat.
        let a = vec![owner(1, 75, 100.0)];
        let b = vec![owner(2, -75, 110.0)];
        let holders = owner_counts([&a, &b]);
        let book = vec![json!({"symbol": "NIFTY27OCT2623000CE", "exchange": "NFO",
            "product": "NRML", "quantity": 0, "average_price": 0.0, "ltp": 120.0})];
        let pa = attribute(a, Some(&book), "NRML", &holders);
        let pb = attribute(b, Some(&book), "NRML", &holders);
        assert_eq!((pa.len(), pb.len()), (1, 1));
        assert_eq!(number(&pa[0]["quantity"]), 75.0);
        assert_eq!(number(&pb[0]["quantity"]), -75.0);
        assert_eq!(number(&pa[0]["ltp"]), 120.0);
        assert_eq!(pa[0]["source"], "local/shared");
        assert_eq!(number(&pb[0]["average_price"]), 110.0);
    }

    #[test]
    fn a_contract_one_strategy_alone_holds_is_the_broker_row() {
        let a = vec![owner(1, 75, 100.0)];
        let holders = owner_counts([&a]);
        let book = vec![json!({"symbol": "NIFTY27OCT2623000CE", "exchange": "NFO",
            "product": "NRML", "quantity": 75, "average_price": 101.0, "ltp": 120.0})];
        let p = attribute(a, Some(&book), "NRML", &holders);
        assert_eq!(p[0]["source"], "broker");
        assert_eq!(number(&p[0]["average_price"]), 101.0);
    }

    #[test]
    fn same_side_strategies_are_not_double_counted() {
        // Both long 1 lot: the broker row is 2 lots, but each shows its own 1.
        let a = vec![owner(1, 75, 100.0)];
        let b = vec![owner(2, 75, 100.0)];
        let holders = owner_counts([&a, &b]);
        let book = vec![json!({"symbol": "NIFTY27OCT2623000CE", "exchange": "NFO",
            "product": "NRML", "quantity": 150, "average_price": 100.0, "ltp": 120.0})];
        let total: f64 = attribute(a, Some(&book), "NRML", &holders)
            .iter()
            .chain(attribute(b, Some(&book), "NRML", &holders).iter())
            .map(|p| number(&p["quantity"]))
            .sum();
        assert_eq!(total, 150.0);
    }

    fn fill(id: i64, sym: &str, product: &str, action: &str, qty: i64, price: f64) -> OrderRow {
        let mut o = order(id, 1, "entry", action, qty, price, "2026-10-26 04:00:00").order;
        o.symbol = sym.into();
        o.product = Some(product.into());
        o.filled_at = Some(format!("2026-10-26 04:{:02}:00", id));
        o
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 26).unwrap()
    }

    #[test]
    fn net_partial_offset_is_one_row_on_the_bigger_side() {
        // A sells 10 lots (750), B buys 20 lots (1500): net +750 at B's price.
        let a = fill(1, "X", "NRML", "SELL", 750, 100.0);
        let b = fill(2, "X", "NRML", "BUY", 1500, 90.0);
        let n = net_positions(&[&a, &b], today());
        assert_eq!(n.len(), 1);
        assert_eq!(number(&n[0]["quantity"]), 750.0);
        assert_eq!(number(&n[0]["average_price"]), 90.0);
        assert_eq!(number(&n[0]["realized"]), 7500.0);
    }

    #[test]
    fn net_full_offset_is_a_closed_row_with_realized() {
        let a = fill(1, "X", "NRML", "BUY", 75, 100.0);
        let b = fill(2, "X", "NRML", "SELL", 75, 110.0);
        let n = net_positions(&[&a, &b], today());
        assert_eq!(n.len(), 1);
        assert_eq!(number(&n[0]["quantity"]), 0.0);
        assert!(n[0]["average_price"].is_null());
        assert_eq!(number(&n[0]["realized"]), 750.0);
    }

    #[test]
    fn net_flips_from_long_to_short() {
        let a = fill(1, "X", "NRML", "BUY", 10, 100.0);
        let b = fill(2, "X", "NRML", "BUY", 10, 110.0);
        let c = fill(3, "X", "NRML", "SELL", 25, 120.0);
        let n = net_positions(&[&a, &b, &c], today());
        assert_eq!(number(&n[0]["quantity"]), -5.0);
        assert_eq!(number(&n[0]["average_price"]), 120.0);
        assert_eq!(number(&n[0]["realized"]), 300.0); // 20 * (120 - 105)
    }

    #[test]
    fn different_products_do_not_net() {
        let a = fill(1, "X", "MIS", "BUY", 75, 100.0);
        let b = fill(2, "X", "NRML", "SELL", 75, 100.0);
        let n = net_positions(&[&a, &b], today());
        assert_eq!(n.len(), 2);
        let q: Vec<f64> = n.iter().map(|r| number(&r["quantity"])).collect();
        assert!(q.contains(&75.0) && q.contains(&-75.0));
    }

    #[test]
    fn a_contract_closed_on_an_earlier_day_is_not_listed() {
        let mut a = fill(1, "X", "NRML", "BUY", 75, 100.0);
        let mut b = fill(2, "X", "NRML", "SELL", 75, 110.0);
        a.filled_at = Some("2026-10-20 04:00:00".into());
        b.filled_at = Some("2026-10-20 05:00:00".into());
        assert!(net_positions(&[&a, &b], today()).is_empty());
        // Carried open from an earlier day: listed, no realized today.
        let n = net_positions(&[&a], today());
        assert_eq!((number(&n[0]["quantity"]), number(&n[0]["realized"])), (75.0, 0.0));
    }

    #[test]
    fn margin_rows_net_contracts_across_strategies() {
        let p = vec![
            json!({"symbol": "A", "exchange": "NFO", "product": "NRML", "quantity": 75}),
            json!({"symbol": "A", "exchange": "NFO", "product": "NRML", "quantity": -25}),
            json!({"symbol": "B", "exchange": "NFO", "product": "MIS", "quantity": -75}),
            json!({"symbol": "C", "exchange": "NFO", "product": "MIS", "quantity": 0}),
        ];
        let m = margin_positions(&p);
        assert_eq!(m.len(), 2);
        assert_eq!((m[0]["action"].as_str(), m[0]["quantity"].as_str()), (Some("BUY"), Some("50")));
        assert_eq!((m[1]["action"].as_str(), m[1]["quantity"].as_str()), (Some("SELL"), Some("75")));
    }
}
