//! The Bridge 1.0.11: legs closed outside the strategy.
//!
//! A position can leave the book without a strategy order: the sandbox
//! settles an expired contract (`sandbox/expiry.rs`), the sandbox MIS
//! square-off closes the net position (`sandbox/squareoff.rs`), or the
//! trader uses the app-wide "close all positions" (Positions page). The
//! strategy leg would otherwise stay "open" and a later exit would open the
//! opposite position. Here such a leg is closed WITHOUT an order: a durable
//! exit row (kind `exit_expiry` / `exit_external` / `exit_auto_squareoff`,
//! status complete, no order sent) so recovery rebuilds it closed, then the
//! ordinary exit fill, so P&L is realised and a signal run stays open and
//! flat exactly as after a signal exit. An expired contract never gets an
//! exit order, in either mode.

use super::dispatch::{exit_action, product_for_exchange, RunMode};
use super::engine::FillOpts;
use super::store::{EventFields, NewOrder};
use super::StrategyModule;
use chrono::{DateTime, NaiveTime, Utc};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Strategy tag the sandbox square-off sends its closes with.
pub const AUTO_SQUARE_OFF_TAG: &str = "AUTO_SQUARE_OFF";

/// Whether the contract named by `symbol` has expired at `now`: after its
/// expiry date, or on it once that venue's session has closed (15:30 IST on
/// NSE/BSE derivatives, 23:30 on commodity venues). Non-expiring
/// instruments never expire.
pub fn contract_expired(symbol: &str, exchange: &str, now: DateTime<Utc>) -> bool {
    let ex = exchange.to_ascii_uppercase();
    let Some(expiry) =
        crate::sandbox::expiry::parse_expiry_from_symbol(&symbol.to_ascii_uppercase(), &ex)
    else {
        return false;
    };
    let ist = now.with_timezone(&chrono_tz::Asia::Kolkata);
    let today = ist.date_naive();
    if today > expiry {
        return true;
    }
    if today < expiry {
        return false;
    }
    let close = if matches!(ex.as_str(), "MCX" | "NCDEX" | "NCO") {
        NaiveTime::from_hms_opt(23, 30, 0)
    } else {
        NaiveTime::from_hms_opt(15, 30, 0)
    }
    .unwrap_or_default();
    ist.time() >= close
}

/// One leg the book no longer holds.
#[derive(Debug, Clone)]
struct HeldLeg {
    run_id: i64,
    leg_id: i64,
    strategy_id: i64,
    user_id: String,
    symbol: String,
    exchange: String,
    product: String,
    signed_qty: i64,
    ltp: Option<f64>,
    entry: f64,
}

impl StrategyModule {
    /// Close one held leg without sending an order, at `price`: a durable
    /// complete exit row, then the exit fill. Skips a leg with an exit in
    /// flight or no confirmed quantity. True when the leg was closed.
    pub async fn close_leg_without_order(
        &self,
        run_id: i64,
        leg_id: i64,
        price: Option<f64>,
        (order_kind, event_kind, severity): (&str, &str, &'static str),
        message: &str,
    ) -> bool {
        let Some((position, symbol, exchange, qty, pref)) = self
            .state
            .with_run(run_id, |r| {
                r.leg(leg_id)
                    .filter(|l| l.status == "open" && l.entry_status == "complete" && l.qty > 0)
                    .filter(|l| !l.exit_in_flight() && l.superseded.is_none())
                    .map(|l| {
                        (
                            l.position.clone(),
                            l.symbol.clone(),
                            l.exchange.clone(),
                            l.qty,
                            l.position_ref.clone(),
                        )
                    })
            })
            .flatten()
        else {
            return false;
        };
        let Ok(Some(run)) = self.store.get_run(run_id) else {
            return false;
        };
        let Ok(Some(strategy)) = self.store.get_strategy_unscoped(run.strategy_id) else {
            return false;
        };
        let Ok(action) = exit_action(&position) else {
            return false;
        };
        let tag = format!(
            "noorder:{}:{}:{}:{}",
            order_kind,
            run_id,
            leg_id,
            self.clock.now().timestamp_micros()
        );
        let row = match self.store.record_order(
            run_id,
            leg_id,
            order_kind,
            &NewOrder {
                symbol: symbol.clone(),
                exchange: exchange.clone(),
                action: action.to_string(),
                qty,
                product: Some(product_for_exchange(&strategy.product, &exchange)),
                pricetype: "MARKET".into(),
                status: "open".into(),
                position_ref: pref.clone(),
                broker_order_id: Some(tag),
            },
        ) {
            Ok(id) => id,
            Err(e) => {
                tracing::error!(
                    "Run {} leg {}: could not record the {} close: {}",
                    run_id,
                    leg_id,
                    order_kind,
                    e
                );
                return false;
            }
        };
        let _ = self
            .store
            .fold_order_broker_frame(row, "complete", price, Some(qty), None);
        let claimed = self
            .state
            .with_run(run_id, |r| {
                r.leg_mut(leg_id).map(|l| {
                    if l.exit_in_flight() {
                        return false;
                    }
                    l.exit_order_id = Some(row);
                    l.exit_kind = Some(order_kind.to_string());
                    true
                })
            })
            .flatten()
            .unwrap_or(false);
        if !claimed {
            let _ = self.store.update_order(
                row,
                Some("cancelled"),
                None,
                Some("leg changed before it was closed"),
            );
            return false;
        }
        self.apply_fill(
            run_id,
            leg_id,
            price,
            false,
            FillOpts {
                filled_qty: Some(qty),
                order_row_id: Some(row),
                position_ref: pref,
                cumulative_filled_qty: Some(qty),
                order_terminal: true,
                allow_prior_order_correction: false,
            },
        )
        .await;
        self.emit(
            strategy.id,
            &strategy.user_id,
            event_kind,
            message,
            EventFields {
                run_id: Some(run_id),
                leg_id: Some(leg_id),
                severity: Some(severity),
                payload: Some(
                    json!({"reason": event_kind, "symbol": symbol, "exchange": exchange,
                    "quantity": qty, "price": price, "order_sent": false}),
                ),
                ..Default::default()
            },
        )
        .await;
        true
    }

    /// Close every held leg of `run_id` whose contract has expired, without
    /// an order. Sandbox: at the price the sandbox settled the contract at
    /// (its settled row's LTP), else the leg's last price, else its entry.
    /// Live: marked closed with a warning; the broker settles the contract.
    /// Answers how many legs were closed.
    pub async fn settle_expired_legs(&self, run_id: i64) -> usize {
        let Ok(Some(run)) = self.store.get_run(run_id) else {
            return 0;
        };
        let mode = self.mode_of(&run.mode);
        let product = self
            .store
            .get_strategy_unscoped(run.strategy_id)
            .ok()
            .flatten()
            .map(|s| s.product)
            .unwrap_or_default();
        let now = self.clock.now();
        let expired: Vec<(i64, String, String, Option<f64>, f64)> = self
            .state
            .with_run(run_id, |r| {
                r.legs
                    .values()
                    .filter(|l| l.status == "open" && contract_expired(&l.symbol, &l.exchange, now))
                    .map(|l| {
                        (
                            l.leg_id,
                            l.symbol.clone(),
                            l.exchange.clone(),
                            l.ltp,
                            l.entry_avg,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut n = 0;
        for (leg_id, symbol, exchange, ltp, entry) in expired {
            let p = product_for_exchange(&product, &exchange);
            let settled = match mode {
                RunMode::Sandbox => match self
                    .gateway
                    .contract_position(mode, &symbol, &exchange, &p)
                    .await
                {
                    Ok(Some((0, Some(px)))) => Some(px),
                    _ => None,
                },
                RunMode::Live => None,
            };
            let price = settled.or(ltp).or((entry > 0.0).then_some(entry));
            let (severity, message) = match mode {
                RunMode::Sandbox => ("info", format!(
                    "{} expired; leg {} closed at the settlement price {} (no order sent)",
                    symbol, leg_id, price.map(|p| format!("{:.2}", p)).unwrap_or_else(|| "unavailable".into())
                )),
                RunMode::Live => ("warn", format!(
                    "{} expired; leg {} marked closed as expired and no order was sent. The broker settles expired contracts; check its P&L.",
                    symbol, leg_id
                )),
            };
            if self
                .close_leg_without_order(
                    run_id,
                    leg_id,
                    price,
                    ("exit_expiry", "expiry_settled", severity),
                    &message,
                )
                .await
            {
                n += 1;
            }
        }
        n
    }

    /// Bring the open legs of every run in `mode` in line with the book:
    /// expired contracts are settled (above); a contract whose book row is
    /// flat (or gone) while strategy legs still hold it was closed outside
    /// the strategy, so those legs are closed at the close's price (the
    /// newest trade), as `auto_squareoff` when the sandbox square-off made
    /// that trade, else `external_close`. A contract still held in the book
    /// is never touched (strategies sharing it net to anything); a book that
    /// cannot be read changes nothing. Answers one entry per leg closed.
    pub async fn reconcile_positions(&self, mode: RunMode) -> Vec<Value> {
        let mut out = Vec::new();
        let mut held: BTreeMap<(String, String, String), Vec<HeldLeg>> = BTreeMap::new();
        for run_id in self.state.active_run_ids() {
            let Ok(Some(run)) = self.store.get_run(run_id) else {
                continue;
            };
            if run.stopped_at.is_some() || self.mode_of(&run.mode) != mode {
                continue;
            }
            let expired = self.settle_expired_legs(run_id).await;
            if expired > 0 {
                out.push(json!({"run_id": run_id, "reason": "expiry_settled", "legs": expired}));
            }
            let Ok(Some(strategy)) = self.store.get_strategy_unscoped(run.strategy_id) else {
                continue;
            };
            let legs: Vec<HeldLeg> = self
                .state
                .with_run(run_id, |r| {
                    r.legs
                        .values()
                        .filter(|l| l.status == "open" && l.entry_status == "complete" && l.qty > 0)
                        .map(|l| HeldLeg {
                            run_id,
                            leg_id: l.leg_id,
                            strategy_id: strategy.id,
                            user_id: strategy.user_id.clone(),
                            symbol: l.symbol.to_ascii_uppercase(),
                            exchange: l.exchange.to_ascii_uppercase(),
                            product: product_for_exchange(&strategy.product, &l.exchange),
                            signed_qty: if l.position == "B" { l.qty } else { -l.qty },
                            ltp: l.ltp,
                            entry: l.entry_avg,
                        })
                        .collect()
                })
                .unwrap_or_default();
            for l in legs {
                held.entry((l.symbol.clone(), l.exchange.clone(), l.product.clone()))
                    .or_default()
                    .push(l);
            }
        }
        for ((symbol, exchange, product), legs) in held {
            let net: i64 = legs.iter().map(|l| l.signed_qty).sum();
            if net == 0 && legs.len() > 1 {
                // Offsetting strategies: a flat book row is expected.
                continue;
            }
            let book_qty = match self
                .gateway
                .contract_position(mode, &symbol, &exchange, &product)
                .await
            {
                Ok(Some((q, _))) => q,
                Ok(None) => 0,
                Err(e) => {
                    tracing::warn!("Book reconcile skipped {} {}: {}", symbol, exchange, e);
                    continue;
                }
            };
            if book_qty != 0 {
                if book_qty != net {
                    tracing::warn!(
                        "Book holds {} of {} {} {} while strategies hold {}; left as is",
                        book_qty,
                        symbol,
                        exchange,
                        product,
                        net
                    );
                }
                continue;
            }
            let trade = self
                .gateway
                .last_trade(mode, &symbol, &exchange, &product)
                .await;
            let auto = trade
                .as_ref()
                .is_some_and(|(tag, _)| tag.eq_ignore_ascii_case(AUTO_SQUARE_OFF_TAG));
            let (order_kind, reason) = if auto {
                ("exit_auto_squareoff", "auto_squareoff")
            } else {
                ("exit_external", "external_close")
            };
            for l in legs {
                let price = trade
                    .as_ref()
                    .map(|t| t.1)
                    .or(l.ltp)
                    .or((l.entry > 0.0).then_some(l.entry));
                let message = format!(
                    "{} was closed outside the strategy ({}); leg {} marked closed at {} with no order sent",
                    symbol,
                    if auto { "sandbox MIS auto square-off" } else { "closed from the Positions page or elsewhere" },
                    l.leg_id,
                    price.map(|p| format!("{:.2}", p)).unwrap_or_else(|| "an unknown price".into())
                );
                if self
                    .close_leg_without_order(
                        l.run_id,
                        l.leg_id,
                        price,
                        (order_kind, reason, "warn"),
                        &message,
                    )
                    .await
                {
                    let _ = (&l.user_id, l.strategy_id);
                    out.push(
                        json!({"run_id": l.run_id, "leg_id": l.leg_id, "strategy_id": l.strategy_id,
                        "symbol": symbol, "reason": reason, "price": price}),
                    );
                }
            }
        }
        out
    }

    /// The mode a run was started in (Sandbox when it cannot be read).
    pub fn mode_of_run(&self, run_id: i64) -> RunMode {
        self.store
            .get_run(run_id)
            .ok()
            .flatten()
            .map(|r| self.mode_of(&r.mode))
            .unwrap_or(RunMode::Sandbox)
    }

    /// Open strategy legs per mode, for the Positions page warning.
    pub fn open_leg_count(&self, mode: RunMode) -> usize {
        self.state
            .active_run_ids()
            .into_iter()
            .filter(|id| {
                self.store
                    .get_run(*id)
                    .ok()
                    .flatten()
                    .is_some_and(|r| r.stopped_at.is_none() && self.mode_of(&r.mode) == mode)
            })
            .map(|id| {
                self.state
                    .with_run(id, |r| r.open_legs().len())
                    .unwrap_or(0)
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ist(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        chrono_tz::Asia::Kolkata
            .with_ymd_and_hms(y, m, d, h, mi, 0)
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn a_contract_expires_after_its_session_on_expiry_day() {
        let s = "NIFTY27OCT2623000CE";
        assert!(!contract_expired(s, "NFO", ist(2026, 10, 26, 10, 0)));
        assert!(!contract_expired(s, "NFO", ist(2026, 10, 27, 15, 29)));
        assert!(contract_expired(s, "NFO", ist(2026, 10, 27, 15, 30)));
        assert!(contract_expired(s, "NFO", ist(2026, 10, 28, 9, 15)));
        assert!(!contract_expired(
            "CRUDEOIL19NOV26FUT",
            "MCX",
            ist(2026, 11, 19, 20, 0)
        ));
        assert!(contract_expired(
            "CRUDEOIL19NOV26FUT",
            "MCX",
            ist(2026, 11, 19, 23, 31)
        ));
        assert!(!contract_expired("RELIANCE", "NSE", ist(2030, 1, 1, 10, 0)));
    }
}
