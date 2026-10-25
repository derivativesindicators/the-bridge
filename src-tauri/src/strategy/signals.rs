//! Signal-mode strategies: one TradingView alert moves one leg (web
//! `services/strategy_module/signals.py`).
//!
//! ```text
//! {"action": "long_entry", "leg_id": 1}
//! {"action": "short_exit", "symbol": "RELIANCE", "exchange": "NSE"}
//! ```
//!
//! Three outcomes, and the difference is the design:
//!
//! * acted: an order was placed (200);
//! * no-op: understood and correctly did nothing (200 with a note:
//!   `already_long`, `already_short`, `no_matching_position`,
//!   `outside_entry_window`, `outside_trading_window`), because an alert
//!   engine repeats itself and a retry on an order path is how one alert
//!   becomes two positions;
//! * refused: the signal contradicts the configuration (4xx).
//!
//! The side a leg is held comes from the signal that opened it, never from
//! configuration. An opposite entry squares first, then opens. A leg returns
//! to `configured` after an exit so it can be signalled again the same day.
//!
//! The entry path is in `entries` and the exit path in `exits`.

use super::dispatch::{DispatchResult, RunMode};
use super::resolver::is_derivative_exchange;
use super::state::ClaimId;
use super::store::{EventFields, StrategyRow};
use super::StrategyModule;
use crate::services::live_owner::LiveOwner;
use chrono::NaiveTime;
use chrono_tz::Asia::Kolkata;
use serde_json::Value;

mod entries;
mod exits;

pub const SIGNAL_ACTIONS: &[&str] = &["long_entry", "long_exit", "short_entry", "short_exit"];
pub const BATCH_ACTIONS: &[&str] = &["start", "stop"];

/// Which actions this kind of strategy accepts.
pub fn actions_for(kind: &str) -> &'static [&'static str] {
    if kind == "signal" {
        SIGNAL_ACTIONS
    } else {
        BATCH_ACTIONS
    }
}

/// What one signal did, or why it did nothing.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SignalResult {
    pub ok: bool,
    pub note: Option<String>,
    pub error: Option<String>,
    pub leg_id: Option<i64>,
    pub run_id: Option<i64>,
    pub flipped: bool,
}

impl SignalResult {
    pub fn acted(&self) -> bool {
        self.ok && self.note.is_none()
    }

    fn refuse(error: impl Into<String>, leg_id: Option<i64>, run_id: Option<i64>) -> Self {
        Self {
            ok: false,
            error: Some(error.into()),
            leg_id,
            run_id,
            ..Default::default()
        }
    }

    fn note(note: &str, leg_id: Option<i64>, run_id: Option<i64>) -> Self {
        Self {
            ok: true,
            note: Some(note.into()),
            leg_id,
            run_id,
            ..Default::default()
        }
    }
}

fn side_of(action: &str) -> &'static str {
    if action.starts_with("long") {
        "long"
    } else {
        "short"
    }
}

fn position_of(side: &str) -> &'static str {
    if side == "long" {
        "B"
    } else {
        "S"
    }
}

pub(crate) fn leg_id_of(leg: &Value) -> Option<i64> {
    leg.get("id")
        .or_else(|| leg.get("leg_id"))
        .and_then(Value::as_i64)
}

/// The configured leg this signal targets. `leg_id` wins; the symbol
/// fallback lets one alert template serve strategies numbered differently.
pub fn find_leg(
    strategy: &StrategyRow,
    leg_id: Option<&Value>,
    symbol: Option<&str>,
    exchange: Option<&str>,
) -> Option<Value> {
    let legs = strategy.legs.as_array()?;
    if let Some(wanted) = leg_id.filter(|v| !v.is_null()) {
        let wanted = match wanted {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        return legs
            .iter()
            .find(|l| leg_id_of(l).map(|i| i.to_string()) == Some(wanted.clone()))
            .cloned();
    }
    let symbol = symbol.filter(|s| !s.is_empty())?.to_ascii_uppercase();
    let exchange = exchange.unwrap_or("").to_ascii_uppercase();
    legs.iter()
        .find(|l| {
            l["symbol"].as_str().unwrap_or("").to_ascii_uppercase() == symbol
                && (exchange.is_empty()
                    || l["exchange"].as_str().unwrap_or("").to_ascii_uppercase() == exchange)
        })
        .cloned()
}

/// Legs chosen at signal time, optionally only those firing on `side`.
pub(crate) fn resolved_legs(strategy: &StrategyRow, side: Option<&str>) -> Vec<Value> {
    strategy
        .legs
        .as_array()
        .map(|legs| {
            legs.iter()
                .filter(|l| crate::strategy::validate::is_resolved_signal_leg(l))
                .filter(|l| side.is_none_or(|s| l["side"].as_str() == Some(s)))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

/// A resolved leg's order side as the engine's long/short: a bought
/// contract is held long, a sold one short, whichever signal fired it.
pub(crate) fn order_side(leg: &Value) -> &'static str {
    if leg["position"].as_str().unwrap_or("B").eq_ignore_ascii_case("S") {
        "short"
    } else {
        "long"
    }
}

fn hhmm(text: Option<&str>) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(text?, "%H:%M").ok()
}

/// The claimed leg a signal exit is placed for: what it holds, the claim
/// token, and whether it is a superseded position's exit.
struct SignalExit<'a> {
    lid: i64,
    position: &'a str,
    symbol: &'a str,
    exchange: &'a str,
    quantity: i64,
    position_ref: Option<String>,
    claim_token: &'a str,
    superseded: bool,
}

impl StrategyModule {
    /// Why this signal is outside the strategy's trading window, if it is.
    /// Exits stay allowed before the entry window: a carried position must
    /// always be closable.
    fn window_note(&self, strategy: &StrategyRow, action: &str) -> Option<&'static str> {
        if strategy.strategy_type != "intraday" {
            return None;
        }
        let now = self.clock.now().with_timezone(&Kolkata).time();
        if let Some(exit) = hhmm(strategy.exit_time.as_deref()) {
            if now >= exit {
                return Some("outside_trading_window");
            }
        }
        if action.ends_with("_entry") {
            if let Some(entry) = hhmm(strategy.entry_time.as_deref()) {
                if now < entry {
                    return Some("outside_entry_window");
                }
            }
        }
        None
    }

    /// Apply one signal to one leg.
    pub async fn handle_signal(
        &self,
        strategy: &StrategyRow,
        action: &str,
        leg_id: Option<&Value>,
        symbol: Option<&str>,
        exchange: Option<&str>,
    ) -> SignalResult {
        if !SIGNAL_ACTIONS.contains(&action) {
            return SignalResult::refuse(
                format!("Unknown signal action: '{}'", action),
                None,
                None,
            );
        }
        let side = side_of(action);
        // The Bridge 1.0.8: a signal supersedes a pending re-entry the way
        // the backtest engine's does: any entry clears it, an exit clears
        // the pending entry of its own side.
        self.clear_pending_for_signal(strategy, action, side);
        let allowed = match strategy.direction.as_str() {
            "long_only" => side == "long",
            "short_only" => side == "short",
            _ => true,
        };
        if !allowed {
            return SignalResult::refuse(
                format!(
                    "This strategy is {}; a {} signal is not accepted",
                    strategy.direction, side
                ),
                None,
                None,
            );
        }
        let Some(leg) = find_leg(strategy, leg_id, symbol, exchange) else {
            // The Bridge 1.0.6: an alert naming no leg drives every leg chosen
            // at signal time that fires on this side (e.g. long_entry buys the
            // CE leg, short_entry buys the PE leg).
            if leg_id.is_none_or(Value::is_null) && symbol.is_none_or(str::is_empty) {
                let targets = resolved_legs(strategy, Some(side));
                if !targets.is_empty() {
                    return self.handle_resolved(strategy, action, side, targets).await;
                }
            }
            return SignalResult::refuse("No leg matches this signal", None, None);
        };
        if crate::strategy::validate::is_resolved_signal_leg(&leg) {
            let lid = leg_id_of(&leg).unwrap_or(1);
            if leg["side"].as_str() != Some(side) {
                return SignalResult::refuse(
                    format!(
                        "Leg {} only accepts {} signals",
                        lid,
                        leg["side"].as_str().unwrap_or("?")
                    ),
                    Some(lid),
                    None,
                );
            }
            return self.handle_resolved(strategy, action, side, vec![leg]).await;
        }
        let lid = leg_id_of(&leg).unwrap_or(1);
        let leg_side = leg["side"].as_str().unwrap_or("both").to_ascii_lowercase();
        if leg_side != "both" && leg_side != side {
            return SignalResult::refuse(
                format!("Leg {} only accepts {} signals", lid, leg_side),
                Some(lid),
                None,
            );
        }
        if let Some(note) = self.window_note(strategy, action) {
            return SignalResult::note(note, Some(lid), None);
        }
        let run_id = match self.day_run(strategy).await {
            Ok(id) => id,
            Err(e) => return SignalResult::refuse(e, Some(lid), None),
        };
        if action.ends_with("_entry") {
            if let Ok(Some(run)) = self.store.get_run(run_id) {
                if run.stop_requested_reason.is_some() {
                    return SignalResult {
                        ok: false,
                        note: Some("run_stopping".into()),
                        leg_id: Some(lid),
                        run_id: Some(run_id),
                        ..Default::default()
                    };
                }
            }
            return self.signal_enter(strategy, run_id, &leg, lid, side).await;
        }
        self.signal_exit(strategy, run_id, &leg, lid, side).await
    }

    /// Signals for legs whose contract is chosen at signal time. Each such
    /// leg has a fixed order side (`position`) and fires on one signal side,
    /// so its own claim and P&L use the real order side. An entry first
    /// closes the opposite side's open legs (the flip), and opens nothing if
    /// one of those closes is refused.
    async fn handle_resolved(
        &self,
        strategy: &StrategyRow,
        action: &str,
        side: &str,
        targets: Vec<Value>,
    ) -> SignalResult {
        let first = targets.first().and_then(leg_id_of);
        if let Some(note) = self.window_note(strategy, action) {
            return SignalResult::note(note, first, None);
        }
        let run_id = match self.day_run(strategy).await {
            Ok(id) => id,
            Err(e) => return SignalResult::refuse(e, first, None),
        };
        let entry = action.ends_with("_entry");
        if entry {
            if let Ok(Some(run)) = self.store.get_run(run_id) {
                if run.stop_requested_reason.is_some() {
                    return SignalResult {
                        ok: false,
                        note: Some("run_stopping".into()),
                        leg_id: first,
                        run_id: Some(run_id),
                        ..Default::default()
                    };
                }
            }
            let opposite = if side == "long" { "short" } else { "long" };
            let mut flipped = false;
            for leg in resolved_legs(strategy, Some(opposite)) {
                let lid = leg_id_of(&leg).unwrap_or(1);
                let held = self
                    .state
                    .with_run(run_id, |r| r.leg(lid).is_some_and(|l| l.status == "open"))
                    .unwrap_or(false);
                if !held {
                    continue;
                }
                let closed = self
                    .signal_exit(strategy, run_id, &leg, lid, order_side(&leg))
                    .await;
                if !closed.ok {
                    return closed;
                }
                flipped = true;
            }
            let mut out = SignalResult {
                ok: true,
                leg_id: first,
                run_id: Some(run_id),
                flipped,
                ..Default::default()
            };
            for leg in &targets {
                let lid = leg_id_of(leg).unwrap_or(1);
                let r = self
                    .signal_enter(strategy, run_id, leg, lid, order_side(leg))
                    .await;
                if !r.ok {
                    return r;
                }
                if out.note.is_none() {
                    out.note = r.note;
                }
            }
            return out;
        }
        let mut out = SignalResult {
            ok: true,
            leg_id: first,
            run_id: Some(run_id),
            ..Default::default()
        };
        let mut any = false;
        for leg in &targets {
            let lid = leg_id_of(leg).unwrap_or(1);
            let r = self
                .signal_exit(strategy, run_id, leg, lid, order_side(leg))
                .await;
            if !r.ok {
                return r;
            }
            any |= r.note.is_none();
        }
        if !any {
            out.note = Some("no_matching_position".into());
        }
        out
    }

    /// The strategy's open run, kept while it started this session. A
    /// signal run IS a trading day: a stale one is rolled through the
    /// durable end-of-day stop, and only confirmed flatness lets a
    /// replacement open (`None`).
    async fn todays_open_run(&self, strategy: &StrategyRow, current: &StrategyRow) -> Option<i64> {
        let run_id = current.current_run_id?;
        let Ok(Some(run)) = self.store.get_run(run_id) else {
            return None;
        };
        if run.stopped_at.is_some() {
            return None;
        }
        let started = super::store::parse_utc(&run.started_at);
        let stale = started
            .map(|s| {
                super::session::started_before_today(
                    s,
                    self.clock.now(),
                    self.session.0,
                    self.session.1,
                )
            })
            .unwrap_or(false);
        if !stale {
            return Some(run_id);
        }
        // The Bridge 1.0.8: overnight carry keeps a run that still holds a
        // position (or an order that may become one) into the new day.
        let opts = super::signal_options::SignalOptions::from_value(&current.signal_options);
        if opts.carry_overnight {
            let holds = self
                .state
                .with_run(run_id, |r| r.legs.values().any(|l| l.requires_management()))
                .unwrap_or(false);
            if holds {
                tracing::info!("Signal run {} carried into a new day", run_id);
                self.emit(
                    strategy.id,
                    &current.user_id,
                    "position_carried",
                    "Open position carried overnight into this session (overnight carry is on)",
                    EventFields {
                        run_id: Some(run_id),
                        ..Default::default()
                    },
                )
                .await;
                return Some(run_id);
            }
        }
        tracing::info!("Rolling signal run {} through end-of-day stop", run_id);
        let r = self.stop_run(run_id, &current.user_id, "eod").await;
        if !r.ok || r.stop_pending {
            return Some(run_id);
        }
        self.emit(
            strategy.id,
            &current.user_id,
            "eod_squareoff",
            "Previous day's run closed on the first signal of a new day",
            EventFields {
                run_id: Some(run_id),
                severity: Some("warn"),
                ..Default::default()
            },
        )
        .await;
        None
    }

    /// Opens a signal run in `mode`: the run row, its live state, then the
    /// strategy's link to it. A run that cannot be linked is finished and
    /// the claim released; no order is placed.
    fn open_signal_run(&self, strategy: &StrategyRow, mode: RunMode) -> Result<i64, String> {
        let broker = {
            let b = self.gateway.broker_name(mode);
            if b.is_empty() {
                mode.as_str().to_string()
            } else {
                b
            }
        };
        let owner = LiveOwner::new(broker, self.gateway.broker_account(mode));
        let run_id =
            match self
                .store
                .create_run(strategy.id, mode.as_str(), &owner, "webhook", None, None)
            {
                Ok(id) => id,
                Err(e) => {
                    tracing::error!(
                        "Could not open a signal run for strategy {}: {}",
                        strategy.id,
                        e
                    );
                    let _ = self.store.release_strategy(strategy.id);
                    return Err("Could not open a run".into());
                }
            };
        // State before the link: once the row names this run, any reader may
        // act on it.
        self.state
            .install(super::state::RunState::new(run_id, strategy.id, vec![]));
        if !matches!(
            self.store
                .set_strategy_status(strategy.id, "running", Some(run_id)),
            Ok(true)
        ) {
            self.state.clear(run_id);
            let _ = self
                .store
                .finish_unlinked_run_and_release_claim(run_id, strategy.id, "error");
            return Err("Could not link the new signal run; no order was placed".into());
        }
        Ok(run_id)
    }

    /// The run this signal belongs to, opening one if the session has none.
    /// Serialised per strategy so two alerts on one bar join one run.
    pub(crate) async fn day_run(&self, strategy: &StrategyRow) -> Result<i64, String> {
        let lock = self.day_run_lock(strategy.id);
        let _held = lock.lock().await;
        let current = self
            .store
            .get_strategy_unscoped(strategy.id)
            .ok()
            .flatten()
            .unwrap_or_else(|| strategy.clone());
        if let Some(run_id) = self.todays_open_run(strategy, &current).await {
            return Ok(run_id);
        }
        // Mode is the strategy's own opt-in: live only if enabled.
        let mode = if current.live_enabled {
            RunMode::Live
        } else {
            RunMode::Sandbox
        };
        // The claim re-checks the revision this mode was decided from and
        // the webhook lock (SM-01).
        if let Err(refusal) = self.claim_start(&current, mode, "webhook") {
            if let Ok(Some(r)) = self.store.get_strategy_unscoped(strategy.id) {
                if let Some(id) = r.current_run_id {
                    return Ok(id);
                }
            }
            return Err(refusal);
        }
        let run_id = self.open_signal_run(strategy, mode)?;
        self.emit(
            strategy.id,
            &current.user_id,
            "run_started",
            &format!("Signal run opened in {} mode", mode.as_str()),
            EventFields {
                run_id: Some(run_id),
                ..Default::default()
            },
        )
        .await;
        Ok(run_id)
    }

    fn run_mode(&self, run_id: i64) -> RunMode {
        self.store
            .get_run(run_id)
            .ok()
            .flatten()
            .and_then(|r| RunMode::parse(&r.mode))
            .unwrap_or(RunMode::Sandbox)
    }
}

/// A signal exit's outcome: placed, uncertain (its claim stays bound to the
/// row until the order reconciler settles it, LOG-08) or refused (its
/// claim goes back to the caller).
fn signal_exit_outcome(
    result: DispatchResult,
    claim: ClaimId,
) -> Result<(), (String, Option<ClaimId>)> {
    if result.ok {
        Ok(())
    } else if result.uncertain {
        // The exit may be at the broker: its claim stays bound to the
        // row until the order reconciler settles it (LOG-08).
        Err((
            result
                .error
                .unwrap_or_else(|| crate::brokers::common::outcome::UNCERTAIN_MESSAGE.into()),
            None,
        ))
    } else {
        Err((
            result.error.unwrap_or_else(|| "Order rejected".into()),
            Some(claim),
        ))
    }
}

/// A short that the product cannot carry: cash sold short under anything
/// but MIS is a naked short delivery.
fn reject_uncarryable_short(strategy: &StrategyRow, leg: &Value, side: &str) -> Option<String> {
    if side != "short" || strategy.product.eq_ignore_ascii_case("MIS") {
        return None;
    }
    let exchange = leg["exchange"].as_str().unwrap_or("").to_ascii_uppercase();
    if exchange.is_empty() || is_derivative_exchange(&exchange) {
        return None;
    }
    Some(format!(
        "cash cannot be held short overnight, and product {} carries the position. Use MIS for an intraday short.",
        strategy.product.to_ascii_uppercase()
    ))
}
