//! A signal's exits (web `signals.py` exit path): close the leg held on a
//! side, claimed before dispatch, record the order before it is sent, and
//! map the broker's answer.

use super::{position_of, signal_exit_outcome, SignalExit, SignalResult, StrategyModule};
use crate::strategy::dispatch::{build_order, exit_action, DispatchResult, EXIT_PRICETYPE};
use crate::strategy::state::ClaimId;
use crate::strategy::store::{EventFields, NewOrder, StrategyRow};
use serde_json::Value;

impl StrategyModule {
    /// An exit for a side the leg does not hold: real only when a flip
    /// whose closing order was refused left that outgoing side held, else
    /// noted as no matching position.
    async fn exit_outgoing_side(
        &self,
        strategy: &StrategyRow,
        run_id: i64,
        lid: i64,
        side: &str,
    ) -> SignalResult {
        if let Some(c) = self
            .state
            .claim_superseded_exit(run_id, lid, position_of(side))
        {
            let placed = self
                .signal_place_exit(
                    strategy,
                    run_id,
                    SignalExit {
                        lid,
                        position: &c.position,
                        symbol: &c.symbol,
                        exchange: &c.exchange,
                        quantity: c.quantity,
                        position_ref: c.position_ref.clone(),
                        claim_token: &c.claim_token,
                        superseded: true,
                    },
                )
                .await;
            return match placed {
                Ok(()) => SignalResult {
                    ok: true,
                    leg_id: Some(lid),
                    run_id: Some(run_id),
                    ..Default::default()
                },
                Err((e, claim)) => {
                    // `None`: unconfirmed, the claim stays (LOG-08).
                    if let Some(claim) = claim {
                        if self.state.release_superseded_exit(run_id, lid, &claim) {
                            self.report_flip_outgoing_exit_rejected(run_id, lid, "refused", None)
                                .await;
                        }
                    }
                    SignalResult::refuse(e, Some(lid), Some(run_id))
                }
            };
        }
        SignalResult::note("no_matching_position", Some(lid), Some(run_id))
    }

    /// Close a leg held on `side`, or say it was not held.
    pub(crate) fn signal_exit<'a>(
        &'a self,
        strategy: &'a StrategyRow,
        run_id: i64,
        leg: &'a Value,
        lid: i64,
        side: &'a str,
    ) -> futures_util::future::BoxFuture<'a, SignalResult> {
        Box::pin(async move {
            let _ = leg;
            let held = self
                .state
                .with_run(run_id, |run| {
                    run.leg(lid).filter(|l| l.status == "open").map(|l| {
                        if l.position == "B" {
                            "long"
                        } else {
                            "short"
                        }
                    })
                })
                .flatten();
            if held != Some(side) {
                return self.exit_outgoing_side(strategy, run_id, lid, side).await;
            }
            // Claim before dispatching: a repeated exit alert must not send a
            // second closing order.
            let Some(c) = self.state.claim_leg_exit(run_id, lid, "exit_signal") else {
                let unfilled = self
                    .state
                    .with_run(run_id, |r| {
                        r.leg(lid)
                            .is_some_and(|l| l.status == "open" && l.entry_status != "complete")
                    })
                    .unwrap_or(false);
                if unfilled {
                    return SignalResult::refuse(
                        "The entry for this leg has been accepted but not filled, so there is no confirmed quantity to exit. Retry once it fills.",
                        Some(lid),
                        Some(run_id),
                    );
                }
                return SignalResult::note("no_matching_position", Some(lid), Some(run_id));
            };
            match self
                .signal_place_exit(
                    strategy,
                    run_id,
                    SignalExit {
                        lid,
                        position: &c.leg.position,
                        symbol: &c.leg.symbol,
                        exchange: &c.leg.exchange,
                        quantity: c.leg.qty,
                        position_ref: c.leg.position_ref.clone(),
                        claim_token: &c.claim_token,
                        superseded: false,
                    },
                )
                .await
            {
                Ok(()) => SignalResult {
                    ok: true,
                    leg_id: Some(lid),
                    run_id: Some(run_id),
                    ..Default::default()
                },
                Err((e, claim)) => {
                    // Leave the leg exitable: its stop, target and square-off
                    // all skip a leg that looks like it has an exit in flight.
                    // An unconfirmed exit (`None`) keeps its claim (LOG-08).
                    if let Some(claim) = claim {
                        self.state.release_leg_exit(run_id, lid, &claim);
                    }
                    SignalResult::refuse(e, Some(lid), Some(run_id))
                }
            }
        })
    }

    async fn signal_place_exit(
        &self,
        strategy: &StrategyRow,
        run_id: i64,
        exit: SignalExit<'_>,
    ) -> Result<(), (String, Option<ClaimId>)> {
        let mut claim = ClaimId::Token(exit.claim_token.to_string());
        let mode = self.run_mode(run_id);
        if let Err(e) = self.authorised_for(run_id, mode) {
            return Err((e, Some(claim)));
        }
        let action = exit_action(exit.position).map_err(|e| (e, Some(claim.clone())))?;
        let order = build_order(
            exit.symbol,
            exit.exchange,
            action,
            exit.quantity,
            &strategy.product,
            &strategy.name,
            EXIT_PRICETYPE,
        );
        let row_id = self
            .record_signal_exit(strategy, run_id, &exit, (action, &order.product))
            .await
            .map_err(|e| (e, Some(claim.clone())))?;
        claim = ClaimId::Row(row_id);
        let result = self.place_for_run(run_id, mode, &order).await;
        self.record_acknowledgement(
            row_id,
            &result,
            strategy.id,
            &strategy.user_id,
            run_id,
            exit.lid,
        )
        .await;
        self.emit_signal_exit_placed(strategy, run_id, &exit, action, &result)
            .await;
        if result.ok {
            self.replay_for(result.broker_order_id.as_deref()).await;
        }
        signal_exit_outcome(result, claim)
    }

    /// Records a signal exit's order and binds it to the leg's claim before
    /// it is sent. An exit with no row is not sent (P2-6): it could not be
    /// found again if its answer were lost; the caller releases the claim,
    /// the leg stays managed and the exit is tried again.
    async fn record_signal_exit(
        &self,
        strategy: &StrategyRow,
        run_id: i64,
        exit: &SignalExit<'_>,
        (action, product): (&str, &str),
    ) -> Result<i64, String> {
        let row = self.store.record_order(
            run_id,
            exit.lid,
            "exit_signal",
            &NewOrder {
                symbol: exit.symbol.into(),
                exchange: exit.exchange.into(),
                action: action.into(),
                qty: exit.quantity,
                product: Some(product.to_string()),
                pricetype: EXIT_PRICETYPE.into(),
                status: "pending".into(),
                position_ref: exit.position_ref.clone(),
                broker_order_id: None,
            },
        );
        match row.as_ref().ok().copied() {
            Some(row_id) => {
                let bound = if exit.superseded {
                    self.state
                        .bind_superseded_exit(run_id, exit.lid, exit.claim_token, row_id)
                } else {
                    self.state.bind_live_exit(
                        run_id,
                        exit.lid,
                        exit.claim_token,
                        row_id,
                        exit.position_ref.as_deref(),
                    )
                };
                if !bound {
                    let _ = self.store.update_order(
                        row_id,
                        Some("rejected"),
                        None,
                        Some(if exit.superseded {
                            "Outgoing position exit claim changed before dispatch"
                        } else {
                            "Live position exit claim changed before dispatch"
                        }),
                    );
                    return Err("The position changed before its exit could be placed".into());
                }
                Ok(row_id)
            }
            None => {
                // Not sent (P2-6): an exit with no row cannot be found again
                // if its answer is lost. The caller releases the claim; the
                // leg stays managed and the exit is tried again.
                if let Err(e) = &row {
                    tracing::error!(
                        "Signal exit row for run {} leg {} not written: {}",
                        run_id,
                        exit.lid,
                        e
                    );
                }
                self.emit(
                    strategy.id,
                    &strategy.user_id,
                    "leg_exit_rejected",
                    &format!(
                        "Signal exit for leg {} was not sent: its order could not be recorded. The leg stays managed; check the disk and the app's data folder.",
                        exit.lid
                    ),
                    EventFields {
                        run_id: Some(run_id),
                        leg_id: Some(exit.lid),
                        severity: Some("critical"),
                        payload: None,
                    },
                )
                .await;
                Err("The exit could not be recorded, so it was not sent".into())
            }
        }
    }

    /// The `leg_exit_placed` event: the exit, and how the broker answered.
    async fn emit_signal_exit_placed(
        &self,
        strategy: &StrategyRow,
        run_id: i64,
        exit: &SignalExit<'_>,
        action: &str,
        result: &DispatchResult,
    ) {
        self.emit(
            strategy.id,
            &strategy.user_id,
            "leg_exit_placed",
            &format!(
                "Signal {} {} {}{}",
                action,
                exit.quantity,
                exit.symbol,
                if result.ok {
                    String::new()
                } else if result.uncertain {
                    " sent, but not confirmed by the broker; not sent again".to_string()
                } else {
                    format!(" rejected: {}", result.error.clone().unwrap_or_default())
                }
            ),
            EventFields {
                run_id: Some(run_id),
                leg_id: Some(exit.lid),
                severity: Some(if result.ok { "info" } else { "warn" }),
                payload: None,
            },
        )
        .await;
    }
}
