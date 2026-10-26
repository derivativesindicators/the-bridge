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
        let mut out = Vec::new();
        let (mut t_real, mut t_unreal) = (0.0, 0.0);
        for s in strategies {
            let history = self.order_history(s.id, None).await?;
            if !history.iter().any(|o| o.mode == mode.as_str()) {
                continue;
            }
            let product = s.product.to_ascii_uppercase();
            let owners = residual_owners(placed_in(&history, mode), &product);
            let mut positions: Vec<Value> = if owners.is_empty() {
                Vec::new()
            } else {
                match &book {
                    Some(b) => overlay(b.clone(), owners, &product),
                    None => owners,
                }
            };
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
                "positions": positions, "closed_today": closed,
                "realized": round2(realized), "unrealized": round2(unrealized),
                "total": round2(realized + unrealized),
            }));
        }
        Ok(json!({"status": "success", "data": {
            "mode": mode.as_str(),
            "book_available": book.is_some(),
            "strategies": out,
            "totals": {"realized": round2(t_real), "unrealized": round2(t_unreal), "total": round2(t_real + t_unreal)},
        }}))
    }
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
