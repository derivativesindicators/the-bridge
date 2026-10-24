//! Starting a run: resolve every leg, claim the strategy, place the
//! entries and record their acknowledgements (web `engine.py` `start_run`).

use super::{
    ev, leg_id_of, rejection_summary, StartResult, StrategyModule, ORDER_UNCONFIRMED, UNCONFIRMED,
};
use crate::services::live_owner::LiveOwner;
use crate::strategy::dispatch::{build_order, entry_action, DispatchResult, OrderPayload, RunMode};
use crate::strategy::resolver::{resolve_leg, Failure};
use crate::strategy::start_claim::START_READ_FAILED;
use crate::strategy::state::{new_leg_state, new_position_ref, LegSpec, RunState};
use crate::strategy::store::{EventFields, NewOrder, StrategyRow, LIVE_NOT_ENABLED};
use futures_util::future::BoxFuture;
use serde_json::{json, Value};

impl StrategyModule {
    /// Start a batch strategy, or explain why it did not start.
    pub async fn start_run(
        &self,
        strategy_id: i64,
        user_id: &str,
        mode: &str,
        trigger_source: &str,
        webhook_event_id: Option<i64>,
    ) -> StartResult {
        let strategy = match self.store.get_strategy(strategy_id, user_id) {
            Ok(Some(s)) => s,
            Ok(None) => return StartResult::fail("Strategy not found"),
            Err(e) => {
                tracing::error!("Could not read strategy {}: {}", strategy_id, e);
                return StartResult::fail(START_READ_FAILED);
            }
        };
        let Some(run_mode) = RunMode::parse(mode) else {
            return StartResult::fail(format!("Unknown run mode: '{}'", mode));
        };
        if let Some(why) = self.start_refusal(&strategy, run_mode) {
            return StartResult::fail(why);
        }

        // Resolve everything before claiming anything.
        let (resolved, failures) = self.resolve_all_legs(&strategy).await;
        if !failures.is_empty() {
            let error = failures[0]["error"].as_str().unwrap_or("").to_string();
            return StartResult {
                ok: false,
                run_id: None,
                error: Some(error),
                legs: failures,
            };
        }

        // One conditional UPDATE that re-checks what was decided above
        // against the row as it is now (SM-01).
        if let Err(refusal) = self.claim_start(&strategy, run_mode, trigger_source) {
            return StartResult::fail(refusal);
        }

        let run_id = match self.open_linked_run(
            strategy_id,
            &resolved,
            (mode, run_mode),
            trigger_source,
            webhook_event_id,
        ) {
            Ok(id) => id,
            Err(failed) => return failed,
        };

        if let Err(failed) = self
            .install_run(
                run_id,
                strategy_id,
                user_id,
                &resolved,
                (mode, trigger_source),
            )
            .await
        {
            return failed;
        }

        let placed = self
            .place_entries(run_id, &strategy, &resolved, run_mode, user_id)
            .await;

        self.start_outcome(run_id, strategy_id, user_id, placed)
            .await
    }

    /// Why this strategy cannot start in `run_mode`: a signal strategy has
    /// no start, Live is opt-in per strategy (checked here as the last
    /// point before real orders as well as at every caller), the gateway
    /// must allow the mode, and no new exposure is taken while a gap in
    /// order facts is repaired (ARCH-01).
    fn start_refusal(&self, strategy: &StrategyRow, run_mode: RunMode) -> Option<String> {
        if strategy.strategy_kind == "signal" {
            return Some(
                "A signal strategy has no start. Its run opens on the first long_entry or short_entry signal after the session boundary.".into(),
            );
        }
        // Live is opt-in per strategy, checked here as the last point before
        // real orders as well as at every caller.
        if run_mode == RunMode::Live && !strategy.live_enabled {
            return Some(LIVE_NOT_ENABLED.into());
        }
        if let Err(e) = self.gateway.authorised(run_mode) {
            return Some(e);
        }
        // A gap in order facts is being repaired: no new exposure until the
        // book has been read (ARCH-01). Exits are never held by this.
        self.entry_refusal(run_mode)
    }

    /// Opens the run row (with the resolved expiries) and links the
    /// strategy to it. A run that cannot be opened releases the claim; one
    /// that cannot be linked is finished and its empty claim released, and
    /// no order is placed (`Err`: the start's answer).
    fn open_linked_run(
        &self,
        strategy_id: i64,
        resolved: &[LegSpec],
        (mode, run_mode): (&str, RunMode),
        trigger_source: &str,
        webhook_event_id: Option<i64>,
    ) -> Result<i64, StartResult> {
        let expiries: serde_json::Map<String, Value> = resolved
            .iter()
            .filter_map(|l| {
                l.expiry
                    .as_ref()
                    .map(|e| (l.leg_id.to_string(), Value::String(e.clone())))
            })
            .collect();
        let run_id = match self.store.create_run(
            strategy_id,
            mode,
            &LiveOwner::of_gateway(self.gateway.as_ref(), run_mode),
            trigger_source,
            webhook_event_id,
            Some(&Value::Object(expiries)),
        ) {
            Ok(id) => id,
            Err(e) => {
                tracing::error!("Could not open a run for strategy {}: {}", strategy_id, e);
                let _ = self.store.release_strategy(strategy_id);
                return Err(StartResult::fail("Could not open a run"));
            }
        };

        if !matches!(
            self.store
                .set_strategy_status(strategy_id, "running", Some(run_id)),
            Ok(true)
        ) {
            let cleaned = self
                .store
                .finish_unlinked_run_and_release_claim(run_id, strategy_id, "error")
                .unwrap_or(false);
            if !cleaned {
                tracing::error!(
                    "Run {} could not be linked to strategy {} and its empty claim could not be released",
                    run_id,
                    strategy_id
                );
            }
            return Err(StartResult {
                ok: false,
                run_id: if cleaned { None } else { Some(run_id) },
                error: Some(
                    "Could not link the new run to its strategy; no order was placed".into(),
                ),
                legs: vec![],
            });
        }
        Ok(run_id)
    }

    /// The run's live state, its prices (before entries: a fill can be
    /// reported within milliseconds), the run_started event and the run's
    /// first push. A leg whose state cannot be built finishes the run
    /// before any entry (`Err`: the start's answer).
    async fn install_run(
        &self,
        run_id: i64,
        strategy_id: i64,
        user_id: &str,
        resolved: &[LegSpec],
        (mode, trigger_source): (&str, &str),
    ) -> Result<(), StartResult> {
        let mut legs = Vec::with_capacity(resolved.len());
        for spec in resolved {
            match new_leg_state(spec) {
                Ok(l) => legs.push(l),
                Err(e) => {
                    tracing::error!("Run {} leg refused: {}", run_id, e);
                    self.finalise(
                        run_id,
                        strategy_id,
                        user_id,
                        "error",
                        "Start failed before any entry dispatch",
                    )
                    .await;
                    return Err(StartResult {
                        ok: false,
                        run_id: Some(run_id),
                        error: Some("Could not start the strategy".into()),
                        legs: vec![],
                    });
                }
            }
        }
        self.state.install(RunState::new(run_id, strategy_id, legs));
        // Prices before entries: a fill can be reported within milliseconds.
        let symbols: Vec<(String, String)> = resolved
            .iter()
            .map(|l| (l.symbol.clone(), l.exchange.clone()))
            .collect();
        self.follow_prices(run_id, &symbols).await;
        self.emit(
            strategy_id,
            user_id,
            "run_started",
            &format!("Run started in {} mode ({})", mode, trigger_source),
            ev(Some(run_id), None, "info"),
        )
        .await;
        if let Ok(Some(run)) = self.store.get_run(run_id) {
            self.broadcast
                .push_run_update(strategy_id, run.to_dict())
                .await;
        }
        Ok(())
    }

    /// The start's answer once the entries were sent. An unconfirmed entry
    /// may be a position: the run stays open and managed until the order
    /// reconciler settles it (LOG-08); with no entry accepted or
    /// unconfirmed, the run is finished.
    async fn start_outcome(
        &self,
        run_id: i64,
        strategy_id: i64,
        user_id: &str,
        placed: Vec<Value>,
    ) -> StartResult {
        if !placed
            .iter()
            .any(|l| l["ok"] == json!(true) || l["unconfirmed"] == json!(true))
        {
            let finalised = self
                .finalise(
                    run_id,
                    strategy_id,
                    user_id,
                    "error",
                    "No entry order was accepted",
                )
                .await;
            let error = if finalised {
                rejection_summary(&placed)
            } else {
                "Every entry order was rejected, but the flat run could not be finalised; retry stop"
                    .into()
            };
            return StartResult {
                ok: false,
                run_id: Some(run_id),
                error: Some(error),
                legs: placed,
            };
        }
        StartResult {
            ok: true,
            run_id: Some(run_id),
            error: None,
            legs: placed,
        }
    }

    /// Resolve every leg to a contract, pricing the underlying once.
    async fn resolve_all_legs(&self, strategy: &StrategyRow) -> (Vec<LegSpec>, Vec<Value>) {
        let legs = strategy.legs.as_array().cloned().unwrap_or_default();
        if legs.is_empty() {
            return (
                vec![],
                vec![json!({"leg_id": null, "ok": false, "error": "The strategy has no legs"})],
            );
        }
        let g = self.symbols.snapshot();
        let today = self
            .clock
            .now()
            .with_timezone(&chrono_tz::Asia::Kolkata)
            .date_naive();
        let mut shared_ltp: Option<f64> = None;
        let mut resolved = Vec::new();
        let mut failures = Vec::new();
        for (index, leg) in legs.iter().enumerate() {
            let leg_id = leg_id_of(leg, index);
            let position = leg["position"].as_str().unwrap_or("").to_ascii_uppercase();
            match resolve_leg(
                &g,
                self.gateway.as_ref(),
                leg,
                &strategy.underlying,
                &strategy.underlying_exchange,
                today,
                shared_ltp,
            )
            .await
            {
                Err(Failure { code, error }) => failures.push(json!({
                    "leg_id": leg_id,
                    "ok": false,
                    "error": format!("Leg {}: {}", leg_id, error),
                    "code": code,
                })),
                Ok(r) => {
                    if shared_ltp.is_none() {
                        shared_ltp = r.underlying_ltp;
                    }
                    let trail = &leg["trail"];
                    resolved.push(LegSpec {
                        leg_id,
                        position,
                        symbol: r.symbol,
                        exchange: r.exchange,
                        lots: r.lots,
                        quantity: r.quantity,
                        position_ref: Some(new_position_ref()),
                        sl_pts: leg["sl_pts"].as_f64(),
                        target_pts: leg["target_pts"].as_f64(),
                        trail_x: trail["x"].as_f64().unwrap_or(0.0),
                        trail_y: trail["y"].as_f64().unwrap_or(0.0),
                        risk_unit: leg["risk_unit"].as_str().unwrap_or("points").to_string(),
                        expiry: r.expiry,
                        expiry_fallback: r.expiry_fallback,
                        expiry_rank: r.expiry_rank,
                    });
                }
            }
        }
        (resolved, failures)
    }

    /// Place every leg's entry, longs first.
    async fn place_entries(
        &self,
        run_id: i64,
        strategy: &StrategyRow,
        resolved: &[LegSpec],
        mode: RunMode,
        user_id: &str,
    ) -> Vec<Value> {
        let mut ordered: Vec<&LegSpec> = resolved.iter().collect();
        ordered.sort_by_key(|l| if l.position == "B" { 0 } else { 1 });
        let mut outcomes = Vec::new();
        for leg in ordered {
            if leg.expiry_fallback {
                self.emit(
                    strategy.id,
                    user_id,
                    "leg_expiry_fallback",
                    &format!(
                        "Leg {} asked for the {} expiry; the chain lists only {}, which was used",
                        leg.leg_id,
                        leg.expiry_rank.clone().unwrap_or_default(),
                        leg.expiry.clone().unwrap_or_default()
                    ),
                    ev(Some(run_id), Some(leg.leg_id), "warn"),
                )
                .await;
            }
            let action = entry_action(&leg.position);
            let order = build_order(
                &leg.symbol,
                &leg.exchange,
                action,
                leg.quantity,
                &strategy.product,
                &strategy.name,
                &strategy.pricetype,
            );
            // Durable BEFORE the broker is called.
            let row_id = match self
                .record_entry_row(run_id, strategy, leg, (&order, action), user_id)
                .await
            {
                Ok(id) => id,
                Err(outcome) => {
                    outcomes.push(outcome);
                    continue;
                }
            };

            let result = self.place_for_run(run_id, mode, &order).await;
            let acknowledged = self
                .record_acknowledgement(row_id, &result, strategy.id, user_id, run_id, leg.leg_id)
                .await;

            self.note_entry_answer(run_id, leg, row_id, &result);

            if result.ok {
                // After the leg's bookkeeping: a sandbox fill published inside
                // the dispatch was held and is applied now.
                self.replay_for(result.broker_order_id.as_deref()).await;
            }

            if !result.uncertain {
                self.emit_entry_answer((strategy, user_id), run_id, leg, action, &result)
                    .await;
            }

            outcomes.push(json!({
                "leg_id": leg.leg_id,
                "ok": result.ok,
                "unconfirmed": result.uncertain,
                "symbol": leg.symbol,
                "broker_order_id": result.broker_order_id,
                "error": result.error,
                "acknowledged": acknowledged,
            }));
        }
        outcomes
    }

    /// Records a leg's entry before the broker is called. An entry that
    /// cannot be recorded is not placed: its intent is rejected, the trader
    /// is told, and its outcome is the refusal (`Err`).
    async fn record_entry_row(
        &self,
        run_id: i64,
        strategy: &StrategyRow,
        leg: &LegSpec,
        (order, action): (&OrderPayload, &str),
        user_id: &str,
    ) -> Result<i64, Value> {
        let row = self.store.record_order(
            run_id,
            leg.leg_id,
            "entry",
            &NewOrder {
                symbol: leg.symbol.clone(),
                exchange: leg.exchange.clone(),
                action: action.into(),
                qty: leg.quantity,
                product: Some(order.product.clone()),
                pricetype: strategy.pricetype.clone(),
                status: "pending".into(),
                position_ref: leg.position_ref.clone(),
                broker_order_id: None,
            },
        );
        match row {
            Ok(id) => Ok(id),
            Err(e) => {
                tracing::error!(
                    "Entry row for run {} leg {} not written: {}",
                    run_id,
                    leg.leg_id,
                    e
                );
                // An entry that cannot be recorded is not placed.
                self.state
                    .reject_entry_intent(run_id, leg.leg_id, leg.position_ref.as_deref());
                self.emit(
                    strategy.id,
                    user_id,
                    "leg_entry_rejected",
                    &format!(
                        "Entry for leg {} not placed: its order row could not be written",
                        leg.leg_id
                    ),
                    ev(Some(run_id), Some(leg.leg_id), "critical"),
                )
                .await;
                Err(json!({
                    "leg_id": leg.leg_id, "ok": false, "symbol": leg.symbol,
                    "broker_order_id": null,
                    "error": "Could not record the order before placing it",
                }))
            }
        }
    }

    /// The broker's answer on the leg, when the leg still holds this entry:
    /// its row, and open or rejected. An uncertain entry stays pending on
    /// its row: it may become a position, so it stays managed until the
    /// order reconciler settles it (LOG-08).
    fn note_entry_answer(&self, run_id: i64, leg: &LegSpec, row_id: i64, result: &DispatchResult) {
        self.state.with_run(run_id, |run| {
            if let Some(l) = run.leg_mut(leg.leg_id) {
                if l.position_ref == leg.position_ref && l.entry_status == "pending" {
                    l.entry_order_id = Some(row_id);
                    // An uncertain entry stays pending on its row: it may
                    // become a position, so it stays managed until the
                    // order reconciler settles it (LOG-08).
                    if !result.uncertain {
                        let s = if result.ok { "open" } else { "rejected" };
                        l.entry_status = s.into();
                        l.status = s.into();
                    }
                }
            }
        });
    }

    /// The `leg_entry_placed` or `leg_entry_rejected` event for an answered
    /// entry.
    async fn emit_entry_answer(
        &self,
        (strategy, user_id): (&StrategyRow, &str),
        run_id: i64,
        leg: &LegSpec,
        action: &str,
        result: &DispatchResult,
    ) {
        self.emit(
            strategy.id,
            user_id,
            if result.ok {
                "leg_entry_placed"
            } else {
                "leg_entry_rejected"
            },
            &if result.ok {
                format!("Entry {} {} {} placed", action, leg.quantity, leg.symbol)
            } else {
                format!(
                    "Entry rejected on leg {}: {}",
                    leg.leg_id,
                    result.error.clone().unwrap_or_default()
                )
            },
            ev(
                Some(run_id),
                Some(leg.leg_id),
                if result.ok { "info" } else { "warn" },
            ),
        )
        .await;
    }

    /// Write what the broker answered onto the order row, and say whether it
    /// stuck. Retried once; a failure leaves an `order_ack_unrecorded`
    /// critical event carrying the exact row, run, leg and broker facts, from
    /// which the row is repaired.
    pub(crate) async fn record_acknowledgement(
        &self,
        row_id: i64,
        result: &DispatchResult,
        strategy_id: i64,
        user_id: &str,
        run_id: i64,
        leg_id: i64,
    ) -> bool {
        if result.uncertain {
            return self
                .record_unconfirmed(row_id, result, strategy_id, user_id, run_id, leg_id)
                .await;
        }
        let status = if result.ok { "open" } else { "rejected" };
        let reason = if result.ok {
            None
        } else {
            result.error.as_deref()
        };
        let write = || {
            self.store
                .update_order(
                    row_id,
                    Some(status),
                    result.broker_order_id.as_deref(),
                    reason,
                )
                .unwrap_or(false)
        };
        if write() || write() {
            return true;
        }
        tracing::error!(
            "Could not record the broker acknowledgement for strategy order row {}",
            row_id
        );
        let message = if result.ok {
            format!(
                "Broker order {} was accepted for leg {}, but order row {} remains pending. The order details are kept for automatic reconciliation; the position remains managed until the broker confirms it.",
                result.broker_order_id.clone().unwrap_or_else(|| "(no broker id)".into()),
                leg_id,
                row_id
            )
        } else {
            format!(
                "The broker rejected the order for leg {}, but the rejection could not be written to order row {}. The details are kept for automatic reconciliation; the rejected order created no position.",
                leg_id, row_id
            )
        };
        self.emit(
            strategy_id,
            user_id,
            "order_ack_unrecorded",
            &message,
            EventFields {
                run_id: Some(run_id),
                leg_id: Some(leg_id),
                severity: Some("critical"),
                payload: Some(json!({
                    "version": 1,
                    "order_id": row_id,
                    "run_id": run_id,
                    "leg_id": leg_id,
                    "broker_order_id": result.broker_order_id,
                    "accepted": result.ok,
                    "status": status,
                    "reject_reason": reason,
                })),
            },
        )
        .await;
        let _ = self.reconcile_acks(run_id, false).await;
        false
    }

    /// An uncertain placement (LOG-08): the order may be at the broker, so
    /// the row is marked `unconfirmed` (never `rejected`), the claim stays
    /// with it, and an `order_unconfirmed` witness keeps the client tag the
    /// order reconciler looks it up by. Returns whether the row moved.
    async fn record_unconfirmed(
        &self,
        row_id: i64,
        result: &DispatchResult,
        strategy_id: i64,
        user_id: &str,
        run_id: i64,
        leg_id: i64,
    ) -> bool {
        let write = || {
            self.store
                .update_order(row_id, Some(UNCONFIRMED), None, None)
                .unwrap_or(false)
        };
        let written = write() || write();
        let row = self.store.get_order(row_id).ok().flatten();
        let what = row
            .as_ref()
            .map(|r| format!("{} {} {}", r.action, r.qty, r.symbol))
            .unwrap_or_else(|| format!("order row {}", row_id));
        self.emit(
            strategy_id,
            user_id,
            ORDER_UNCONFIRMED,
            &format!(
                "{} for leg {} was sent, but the broker did not confirm it. It may have been placed, so it is not sent again; The Bridge is checking the order book and keeps the position managed meanwhile.",
                what, leg_id
            ),
            EventFields {
                run_id: Some(run_id),
                leg_id: Some(leg_id),
                severity: Some("critical"),
                payload: Some(json!({
                    "version": 1,
                    "order_id": row_id,
                    "run_id": run_id,
                    "leg_id": leg_id,
                    "client_tag": result.client_tag,
                    "detail": result.error,
                    "status_recorded": written,
                })),
            },
        )
        .await;
        written
    }

    /// Repair every pending row named by an `order_ack_unrecorded` witness.
    /// Returns how many accepted acknowledgements still cannot be linked.
    /// Boxed: a replayed fill can reach recovery, which reconciles again.
    pub fn reconcile_acks(&self, run_id: i64, replay: bool) -> BoxFuture<'_, usize> {
        Box::pin(self.reconcile_acks_inner(run_id, replay))
    }

    async fn reconcile_acks_inner(&self, run_id: i64, replay: bool) -> usize {
        let events = match self.store.list_order_ack_events(run_id) {
            Ok(e) => e,
            Err(e) => {
                tracing::error!(
                    "Could not read acknowledgement witnesses for run {}: {}",
                    run_id,
                    e
                );
                return 1;
            }
        };
        let mut unresolved = 0;
        for event in events {
            let p = &event.payload;
            let (Some(order_id), Some(leg_id)) = (p["order_id"].as_i64(), p["leg_id"].as_i64())
            else {
                continue;
            };
            let accepted = p["accepted"].as_bool().unwrap_or(false);
            let outcome = self
                .store
                .bind_order_acknowledgement(
                    order_id,
                    run_id,
                    leg_id,
                    p["broker_order_id"].as_str(),
                    p["status"].as_str().unwrap_or(""),
                    p["reject_reason"].as_str(),
                )
                .unwrap_or("conflict");
            match outcome {
                "repaired" | "already_bound" => {
                    if replay && accepted {
                        self.replay_for(p["broker_order_id"].as_str()).await;
                    }
                }
                _ if accepted => unresolved += 1,
                _ => {}
            }
        }
        unresolved
    }
}
