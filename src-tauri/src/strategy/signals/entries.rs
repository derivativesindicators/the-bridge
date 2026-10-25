//! A signal's entries (web `signals.py` entry path): resolve the leg,
//! claim it, record the order before it is sent, and bind the entry to
//! its claim.

use super::{position_of, reject_uncarryable_short, SignalResult, StrategyModule};
use crate::strategy::dispatch::{build_order, entry_action, DispatchResult, OrderPayload};
use crate::strategy::resolver::{contract_exists, resolve_quantity};
use crate::strategy::state::{EntryDecision, LegSpec};
use crate::strategy::store::{EventFields, NewOrder, StrategyRow};
use serde_json::Value;

impl StrategyModule {
    pub(crate) async fn signal_enter(
        &self,
        strategy: &StrategyRow,
        run_id: i64,
        leg: &Value,
        lid: i64,
        side: &str,
    ) -> SignalResult {
        // A gap in order facts is being repaired: no new exposure until the
        // book has been read (ARCH-01). Exit signals are never held by this.
        if let Some(why) = self.entry_refusal(self.run_mode(run_id)) {
            return SignalResult::refuse(why, Some(lid), Some(run_id));
        }
        let claim = match self
            .state
            .claim_signal_entry(run_id, lid, position_of(side))
        {
            None => return SignalResult::refuse("No active run", Some(lid), Some(run_id)),
            Some(EntryDecision::Note(n)) => {
                return SignalResult {
                    ok: n != "run_stopping",
                    note: Some(n.into()),
                    leg_id: Some(lid),
                    run_id: Some(run_id),
                    ..Default::default()
                }
            }
            Some(EntryDecision::Claimed(c)) => c,
        };
        let token = claim.claim_token.clone();
        let result = self
            .signal_enter_claimed(strategy, run_id, leg, lid, side, claim)
            .await;
        if self.state.release_signal_entry_claim(run_id, lid, &token) {
            // Every refusal before the entry finished leaves the claim here; a
            // stop may have become durable meanwhile.
            self.reconcile_pending_stop(run_id).await;
        }
        result
    }

    async fn signal_enter_claimed(
        &self,
        strategy: &StrategyRow,
        run_id: i64,
        leg: &Value,
        lid: i64,
        side: &str,
        claim: crate::strategy::state::EntryClaim,
    ) -> SignalResult {
        let held = claim
            .held_position
            .as_deref()
            .map(|p| if p == "B" { "long" } else { "short" });
        // Refused before anything is squared: a refusal must cost nothing.
        if let Some(e) = reject_uncarryable_short(strategy, leg, side) {
            return SignalResult::refuse(format!("Leg {}: {}", lid, e), Some(lid), None);
        }
        let mut flipped = false;
        if let Some(held) = held {
            // Opposite side: square first, then open.
            let closed = self.signal_exit(strategy, run_id, leg, lid, held).await;
            if !closed.ok || closed.note.is_some() {
                return closed;
            }
            flipped = true;
        }
        let resolved = if crate::strategy::validate::is_resolved_signal_leg(leg) {
            self.resolve_signal_contract(strategy, leg, lid, side).await
        } else {
            self.resolve_signal_leg(leg, lid, side)
        };
        // The Bridge 1.0.8: an entry into a contract too near its expiry
        // waits for the next session (the backtest engine's defer rule).
        if let Ok(s) = &resolved {
            if crate::strategy::validate::is_resolved_signal_leg(leg) {
                if let Some(note) = self.maybe_defer_entry(strategy, run_id, leg, lid, s).await {
                    return note;
                }
            }
        }
        let spec = match resolved {
            Ok(mut s) => {
                s.position_ref = Some(claim.position_ref.clone());
                s
            }
            Err(e) => {
                let msg = format!("Leg {}: {}", lid, e);
                if leg["strike_mode"].as_str() == Some("premium") {
                    // The Bridge 1.0.7: a premium-based strike that could not be
                    // chosen places nothing; say so where the trader looks.
                    tracing::warn!(
                        "Strategy {} signal {} not placed. {}",
                        strategy.id,
                        side,
                        msg
                    );
                    self.emit(
                        strategy.id,
                        &strategy.user_id,
                        "leg_entry_rejected",
                        &msg,
                        EventFields {
                            run_id: Some(run_id),
                            leg_id: Some(lid),
                            severity: Some("warn"),
                            ..Default::default()
                        },
                    )
                    .await;
                }
                return SignalResult::refuse(msg, Some(lid), None);
            }
        };
        match self
            .signal_place_entry(strategy, run_id, &spec, &claim)
            .await
        {
            Ok(()) => SignalResult {
                ok: true,
                leg_id: Some(lid),
                run_id: Some(run_id),
                flipped,
                ..Default::default()
            },
            Err(e) => SignalResult::refuse(e, Some(lid), Some(run_id)),
        }
    }

    /// The Bridge 1.0.6: a leg chosen at signal time, resolved now against
    /// the strategy's underlying with the batch resolver (expiry rank, ATM
    /// offset from the live price), so the run holds the exact contract and
    /// its exit closes that contract.
    async fn resolve_signal_contract(
        &self,
        strategy: &StrategyRow,
        leg: &Value,
        lid: i64,
        side: &str,
    ) -> Result<LegSpec, String> {
        let g = self.symbols.snapshot();
        let mut today = self
            .clock
            .now()
            .with_timezone(&chrono_tz::Asia::Kolkata)
            .date_naive();
        // The Bridge 1.0.8: an expiry roll reopening the same day ranks
        // expiries from the day after the one it left.
        if let Some(after) = leg["_resolve_after"]
            .as_str()
            .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        {
            today = today.max(after + chrono::Duration::days(1));
        }
        let r = crate::strategy::resolver::resolve_leg(
            &g,
            self.gateway.as_ref(),
            leg,
            &strategy.underlying,
            &strategy.underlying_exchange,
            today,
            None,
        )
        .await
        .map_err(|f| f.error)?;
        let trail = &leg["trail"];
        Ok(LegSpec {
            leg_id: lid,
            position: position_of(side).into(),
            symbol: r.symbol,
            exchange: r.exchange,
            lots: r.lots,
            quantity: r.quantity,
            position_ref: None,
            sl_pts: leg["sl_pts"].as_f64(),
            target_pts: leg["target_pts"].as_f64(),
            trail_x: trail["x"].as_f64().unwrap_or(0.0),
            trail_y: trail["y"].as_f64().unwrap_or(0.0),
            risk_unit: leg["risk_unit"].as_str().unwrap_or("points").to_string(),
            expiry: r.expiry,
            expiry_fallback: r.expiry_fallback,
            expiry_rank: r.expiry_rank,
        })
    }

    /// A signal leg in run-state shape. The side comes from the signal.
    fn resolve_signal_leg(&self, leg: &Value, lid: i64, side: &str) -> Result<LegSpec, String> {
        let symbol = leg["symbol"].as_str().unwrap_or("").to_ascii_uppercase();
        let exchange = leg["exchange"].as_str().unwrap_or("").to_ascii_uppercase();
        let raw_qty = leg
            .get("qty")
            .or_else(|| leg.get("quantity"))
            .and_then(crate::risk::value_to_f64)
            .unwrap_or(0.0);
        if symbol.is_empty() || exchange.is_empty() || raw_qty <= 0.0 {
            return Err("symbol, exchange and quantity are all required".into());
        }
        let g = self.symbols.snapshot();
        if !contract_exists(&g, &symbol, &exchange) {
            return Err(format!("{} is not a contract on {}", symbol, exchange));
        }
        let mode = leg["qty_mode"].as_str().unwrap_or("units");
        let (quantity, _lot) = resolve_quantity(&g, raw_qty as i64, mode, &symbol, &exchange)?;
        let trail = &leg["trail"];
        Ok(LegSpec {
            leg_id: lid,
            position: position_of(side).into(),
            symbol,
            exchange,
            lots: if mode == "lots" { raw_qty as i64 } else { 1 },
            quantity,
            position_ref: None,
            sl_pts: leg["sl_pts"].as_f64(),
            target_pts: leg["target_pts"].as_f64(),
            trail_x: trail["x"].as_f64().unwrap_or(0.0),
            trail_y: trail["y"].as_f64().unwrap_or(0.0),
            risk_unit: leg["risk_unit"].as_str().unwrap_or("points").to_string(),
            ..Default::default()
        })
    }

    async fn signal_place_entry(
        &self,
        strategy: &StrategyRow,
        run_id: i64,
        spec: &LegSpec,
        claim: &crate::strategy::state::EntryClaim,
    ) -> Result<(), String> {
        let mode = self.run_mode(run_id);
        self.authorised_for(run_id, mode)?;
        let action = entry_action(&spec.position);
        let order = build_order(
            &spec.symbol,
            &spec.exchange,
            action,
            spec.quantity,
            &strategy.product,
            &strategy.name,
            &strategy.pricetype,
        );
        let row_id = self
            .record_signal_entry(strategy, run_id, spec, (&order, action))
            .await?;
        if self
            .state
            .add_leg(
                run_id,
                spec,
                &claim.claim_token,
                claim.expected_position_ref.as_deref(),
                row_id,
            )
            .is_none()
        {
            let _ = self.store.update_order(
                row_id,
                Some("rejected"),
                None,
                Some("Signal entry claim changed before dispatch"),
            );
            return Err("The position changed before its entry could be placed".into());
        }
        let symbols = vec![(spec.symbol.clone(), spec.exchange.clone())];
        self.follow_prices(run_id, &symbols).await;
        let result = self.place_for_run(run_id, mode, &order).await;
        self.record_acknowledgement(
            row_id,
            &result,
            strategy.id,
            &strategy.user_id,
            run_id,
            spec.leg_id,
        )
        .await;
        if result.uncertain {
            // The entry may be at the broker: the leg stays pending on its
            // row and the claim stays, so a repeated alert cannot send a
            // second entry, until the order reconciler settles it (LOG-08).
            self.state
                .hold_signal_entry_claim(run_id, spec.leg_id, &claim.claim_token, row_id);
            return Err(result
                .error
                .unwrap_or_else(|| crate::brokers::common::outcome::UNCERTAIN_MESSAGE.into()));
        }
        self.state.finish_signal_entry(
            run_id,
            spec.leg_id,
            spec.position_ref.as_deref().unwrap_or(""),
            &claim.claim_token,
            result.ok,
        );
        self.emit_signal_entry_placed(strategy, run_id, spec, action, &result)
            .await;
        if result.ok {
            self.replay_for(result.broker_order_id.as_deref()).await;
            Ok(())
        } else {
            self.reconcile_pending_stop(run_id).await;
            Err(result.error.unwrap_or_else(|| "Order rejected".into()))
        }
    }

    /// Records a signal entry's order before it is sent; an entry whose row
    /// cannot be written is not placed.
    async fn record_signal_entry(
        &self,
        strategy: &StrategyRow,
        run_id: i64,
        spec: &LegSpec,
        (order, action): (&OrderPayload, &str),
    ) -> Result<i64, String> {
        match self.store.record_order(
            run_id,
            spec.leg_id,
            "entry",
            &NewOrder {
                symbol: spec.symbol.clone(),
                exchange: spec.exchange.clone(),
                action: action.into(),
                qty: spec.quantity,
                product: Some(order.product.clone()),
                pricetype: order.pricetype.clone(),
                status: "pending".into(),
                position_ref: spec.position_ref.clone(),
                broker_order_id: None,
            },
        ) {
            Ok(id) => Ok(id),
            Err(e) => {
                tracing::error!("Signal entry row not written: {}", e);
                self.emit(
                    strategy.id,
                    &strategy.user_id,
                    "leg_entry_rejected",
                    &format!(
                        "Signal entry for leg {} not placed: its order row could not be written",
                        spec.leg_id
                    ),
                    EventFields {
                        run_id: Some(run_id),
                        leg_id: Some(spec.leg_id),
                        severity: Some("critical"),
                        payload: None,
                    },
                )
                .await;
                Err("Could not record the order before placing it".into())
            }
        }
    }

    /// The `leg_entry_placed` event: the entry, and how the broker answered.
    async fn emit_signal_entry_placed(
        &self,
        strategy: &StrategyRow,
        run_id: i64,
        spec: &LegSpec,
        action: &str,
        result: &DispatchResult,
    ) {
        self.emit(
            strategy.id,
            &strategy.user_id,
            "leg_entry_placed",
            &format!(
                "Signal {} {} {}{}",
                action,
                spec.quantity,
                spec.symbol,
                if result.ok {
                    String::new()
                } else {
                    format!(" rejected: {}", result.error.clone().unwrap_or_default())
                }
            ),
            EventFields {
                run_id: Some(run_id),
                leg_id: Some(spec.leg_id),
                severity: Some(if result.ok { "info" } else { "warn" }),
                payload: None,
            },
        )
        .await;
    }
}
