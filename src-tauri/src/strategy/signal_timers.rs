//! The Bridge 1.0.8: the time-driven side of a signal strategy's optional
//! behaviours (see `signal_options`): pending re-entries (expiry roll,
//! deferred entry), the expiry-day roll, and the closing-session (CAS)
//! Supertrend exit. Run once a second from the scheduler pass; every
//! action goes through the same signal entry / exit paths an alert uses.

use super::signal_options::{SignalOptions, Supertrend};
use super::signals::{leg_id_of, order_side, SignalResult};
use super::state::LegSpec;
use super::store::{EventFields, StrategyRow};
use super::StrategyModule;
use crate::brokers::common::master_contract::parse_oa_expiry;
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Timelike, Utc, Weekday};
use chrono_tz::Asia::Kolkata;
use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;

/// When the F&O session opens (candles are anchored here, as the engine's).
const SESSION_OPEN: (u32, u32) = (9, 15);
/// How long a pending re-entry waits for the leg it reopens to be flat.
const PENDING_WAIT_MINUTES: i64 = 5;

/// One CAS tracker: the option-premium Supertrend of a position's main leg.
struct CasTracker {
    symbol: String,
    exchange: String,
    favourable: i8,
    st: Supertrend,
    /// The open candle: (slot start minute of day, high, low, close).
    candle: Option<(u32, f64, f64, f64)>,
    done: bool,
}

/// In-memory timer state (re-derived after a restart; anything that must
/// survive one is in `sm_strategy.signal_pending`).
#[derive(Default)]
pub struct SignalTimers {
    rolled: Mutex<HashMap<i64, NaiveDate>>,
    cas: Mutex<HashMap<(i64, String), CasTracker>>,
    cas_closed_day: Mutex<HashMap<i64, NaiveDate>>,
    cas_failed: Mutex<HashMap<(i64, String), NaiveDate>>,
}

fn minute_of(t: NaiveTime) -> u32 {
    t.hour() * 60 + t.minute()
}

/// The start of the `tf`-minute candle holding `t`, anchored at 09:15.
pub fn slot_start(t: NaiveTime, tf: i64) -> u32 {
    let open = SESSION_OPEN.0 * 60 + SESSION_OPEN.1;
    let m = minute_of(t);
    if m < open {
        return m;
    }
    let tf = tf.max(1) as u32;
    open + (m - open) / tf * tf
}

fn interval_name(tf: i64) -> String {
    if tf == 60 {
        "1h".into()
    } else {
        format!("{}m", tf)
    }
}

impl crate::strategy::store::Store {
    /// A weekday that is not a market holiday (a failed read counts
    /// weekdays only).
    pub fn is_trading_day(&self, date: NaiveDate) -> bool {
        if matches!(date.weekday(), Weekday::Sat | Weekday::Sun) {
            return false;
        }
        match self.conn_for_calendar() {
            Some(conn) => {
                !crate::db::sqlite::market_calendar::is_market_holiday(&conn, date).unwrap_or(false)
            }
            None => true,
        }
    }

    /// The next trading day after `date`.
    pub fn next_trading_day(&self, date: NaiveDate) -> NaiveDate {
        let mut d = date + Duration::days(1);
        for _ in 0..15 {
            if self.is_trading_day(d) {
                return d;
            }
            d += Duration::days(1);
        }
        d
    }
}

impl StrategyModule {
    fn ist_now(&self) -> DateTime<chrono_tz::Tz> {
        self.clock.now().with_timezone(&Kolkata)
    }

    /// A signal clears the pending re-entry it supersedes: any entry, or
    /// an exit of the pending side.
    pub(crate) fn clear_pending_for_signal(&self, strategy: &StrategyRow, action: &str, side: &str) {
        let Ok(p) = self.store.signal_pending(strategy.id) else {
            return;
        };
        if p.is_null() {
            return;
        }
        if action.ends_with("_entry") || p["side"].as_str() == Some(side) {
            let _ = self.store.set_signal_pending(strategy.id, &Value::Null);
            tracing::info!("Strategy {}: pending re-entry cleared by {}", strategy.id, action);
        }
    }

    /// Add one leg to the strategy's pending re-entry (replacing one for
    /// the other side).
    fn add_pending(
        &self,
        strategy: &StrategyRow,
        side: &str,
        lid: i64,
        due: (NaiveDate, NaiveTime),
        reason: &str,
        resolve_after: Option<NaiveDate>,
    ) {
        let mut p = self.store.signal_pending(strategy.id).unwrap_or(Value::Null);
        if p["side"].as_str() != Some(side) || p["reason"].as_str() != Some(reason) {
            p = json!({"side": side, "legs": [], "reason": reason});
        }
        p["due_date"] = json!(due.0.format("%Y-%m-%d").to_string());
        p["due_time"] = json!(due.1.format("%H:%M").to_string());
        p["resolve_after"] = match resolve_after {
            Some(d) => json!(d.format("%Y-%m-%d").to_string()),
            None => Value::Null,
        };
        if let Some(a) = p["legs"].as_array_mut() {
            if !a.iter().any(|v| v.as_i64() == Some(lid)) {
                a.push(json!(lid));
            }
        }
        if let Err(e) = self.store.set_signal_pending(strategy.id, &p) {
            tracing::error!("Strategy {}: pending re-entry not saved: {}", strategy.id, e);
        }
    }

    /// The deferral rule: `Some` (the signal's answer) when this entry
    /// must wait for the next session instead of being placed.
    pub(crate) async fn maybe_defer_entry(
        &self,
        strategy: &StrategyRow,
        run_id: i64,
        leg: &Value,
        lid: i64,
        spec: &LegSpec,
    ) -> Option<SignalResult> {
        let opts = SignalOptions::from_value(&strategy.signal_options);
        if !opts.defers() {
            return None;
        }
        let expiry = spec.expiry.as_deref().and_then(parse_oa_expiry)?;
        let today = self.ist_now().date_naive();
        let days = (expiry - today).num_days();
        let defer = days < opts.min_days_to_expiry || (days == 0 && opts.expiry_day_entry == "defer");
        if !defer {
            return None;
        }
        let due = self.store.next_trading_day(today);
        let side = leg["side"].as_str().unwrap_or("long").to_string();
        self.add_pending(strategy, &side, lid, (due, opts.reopen_time), "defer", None);
        let msg = format!(
            "Leg {}: {} expires in {} day(s); entry deferred to {} {} (expiry rule)",
            lid,
            spec.symbol,
            days,
            due.format("%d-%b-%Y"),
            opts.reopen_time.format("%H:%M")
        );
        self.emit(
            strategy.id,
            &strategy.user_id,
            "entry_deferred",
            &msg,
            EventFields {
                run_id: Some(run_id),
                leg_id: Some(lid),
                severity: Some("info"),
                ..Default::default()
            },
        )
        .await;
        Some(SignalResult {
            ok: true,
            note: Some("entry_deferred".into()),
            leg_id: Some(lid),
            run_id: Some(run_id),
            ..Default::default()
        })
    }

    /// One pass of every signal strategy's timers.
    pub async fn run_signal_timers(&self) {
        let ids = match self.store.signal_timer_strategy_ids() {
            Ok(ids) => ids,
            Err(e) => {
                tracing::debug!("Signal timers not read: {}", e);
                return;
            }
        };
        for id in ids {
            let Ok(Some(strategy)) = self.store.get_strategy_unscoped(id) else {
                continue;
            };
            let opts = SignalOptions::from_value(&strategy.signal_options);
            self.run_pending_entry(&strategy, &opts).await;
            let run_id = match strategy.current_run_id {
                Some(r) if strategy.status == "running" && self.state.with_run(r, |_| ()).is_some() => r,
                _ => {
                    self.timers.cas.lock().retain(|(sid, _), _| *sid != id);
                    continue;
                }
            };
            let now = self.ist_now();
            let (today, time) = (now.date_naive(), now.time());
            if !self.store.is_trading_day(today) {
                continue;
            }
            if opts.expiry_roll
                && time >= opts.roll_time
                && time < super::signal_options::expiring_contract_close()
                && self.timers.rolled.lock().get(&id) != Some(&today)
            {
                self.timers.rolled.lock().insert(id, today);
                self.expiry_roll(&strategy, &opts, run_id, today).await;
            }
            if opts.cas_exit {
                let g = self.symbols.snapshot();
                let expiring = self.open_resolved_legs(&strategy, run_id).iter().any(|(_, s, e)| {
                    g.by_symbol(e, s)
                        .and_then(|r| parse_oa_expiry(&r.expiry))
                        .is_some_and(|d| d == today)
                });
                let (start, end) = opts.effective(today, expiring);
                if time >= start {
                    let mut eff = opts.clone();
                    eff.cas_start = start;
                    eff.session_end = end;
                    self.cas_pass(&strategy, &eff, run_id, today, time).await;
                }
            }
        }
    }

    /// The configured legs held open (or working) in the run, with config.
    fn open_resolved_legs(&self, strategy: &StrategyRow, run_id: i64) -> Vec<(Value, String, String)> {
        let legs = strategy.legs.as_array().cloned().unwrap_or_default();
        legs.into_iter()
            .filter(crate::strategy::validate::is_resolved_signal_leg)
            .filter_map(|cfg| {
                let lid = leg_id_of(&cfg)?;
                let held = self
                    .state
                    .with_run(run_id, |r| {
                        r.leg(lid)
                            .filter(|l| l.status == "open")
                            .map(|l| (l.symbol.clone(), l.exchange.clone()))
                    })
                    .flatten()?;
                Some((cfg, held.0, held.1))
            })
            .collect()
    }

    /// The expiry-day roll: close the legs whose contract expires today and
    /// queue their re-entry in the next expiry.
    async fn expiry_roll(&self, strategy: &StrategyRow, opts: &SignalOptions, run_id: i64, today: NaiveDate) {
        let g = self.symbols.snapshot();
        let expiring: Vec<(Value, String)> = self
            .open_resolved_legs(strategy, run_id)
            .into_iter()
            .filter(|(_, sym, ex)| {
                g.by_symbol(ex, sym)
                    .and_then(|r| parse_oa_expiry(&r.expiry))
                    .is_some_and(|d| d == today)
            })
            .map(|(cfg, sym, _)| (cfg, sym))
            .collect();
        if expiring.is_empty() {
            return;
        }
        let (due, resolve_after) = if opts.reopen == "immediate" {
            let now = self.ist_now().time();
            (
                (today, NaiveTime::from_hms_opt(now.hour(), now.minute(), 0).unwrap_or(now)),
                Some(today),
            )
        } else {
            ((self.store.next_trading_day(today), opts.reopen_time), None)
        };
        for (cfg, sym) in expiring {
            let lid = leg_id_of(&cfg).unwrap_or(1);
            let r = self
                .signal_exit(strategy, run_id, &cfg, lid, order_side(&cfg))
                .await;
            let (msg, sev) = if r.ok && r.note.is_none() {
                let side = cfg["side"].as_str().unwrap_or("long").to_string();
                self.add_pending(strategy, &side, lid, due, "roll", resolve_after);
                (
                    format!(
                        "Expiry roll: leg {} closed {} (expires today); reopens in the next expiry {} {}",
                        lid,
                        sym,
                        if opts.reopen == "immediate" { "now" } else { "at" },
                        if opts.reopen == "immediate" {
                            String::new()
                        } else {
                            format!("{} {}", due.0.format("%d-%b-%Y"), due.1.format("%H:%M"))
                        }
                    ),
                    "info",
                )
            } else {
                (
                    format!(
                        "Expiry roll: leg {} ({}) could not be closed: {}. Not reopened.",
                        lid,
                        sym,
                        r.error.or(r.note).unwrap_or_default()
                    ),
                    "warn",
                )
            };
            self.emit(
                strategy.id,
                &strategy.user_id,
                "expiry_roll",
                &msg,
                EventFields {
                    run_id: Some(run_id),
                    leg_id: Some(lid),
                    severity: Some(sev),
                    ..Default::default()
                },
            )
            .await;
        }
    }

    /// Place a due pending re-entry (roll or deferred entry).
    async fn run_pending_entry(&self, strategy: &StrategyRow, opts: &SignalOptions) {
        let Ok(p) = self.store.signal_pending(strategy.id) else {
            return;
        };
        if p.is_null() {
            return;
        }
        let now = self.ist_now();
        let (today, time) = (now.date_naive(), now.time());
        let due_date = p["due_date"]
            .as_str()
            .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
        let due_time = p["due_time"]
            .as_str()
            .and_then(|t| NaiveTime::parse_from_str(t, "%H:%M").ok())
            .unwrap_or(opts.reopen_time);
        let Some(due_date) = due_date else {
            let _ = self.store.set_signal_pending(strategy.id, &Value::Null);
            return;
        };
        let open = NaiveTime::from_hms_opt(SESSION_OPEN.0, SESSION_OPEN.1, 0).unwrap_or_default();
        if today < due_date
            || (today == due_date && time < due_time)
            || !self.store.is_trading_day(today)
            || time < open
            || time >= opts.effective(today, false).1
        {
            return;
        }
        let legs: Vec<i64> = p["legs"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default();
        let configs: Vec<Value> = strategy
            .legs
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|l| leg_id_of(l).is_some_and(|id| legs.contains(&id)))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        // A rolled leg whose closing order has not filled yet is waited for.
        if let Some(run) = strategy.current_run_id {
            let busy = configs.iter().any(|cfg| {
                let lid = leg_id_of(cfg).unwrap_or(0);
                self.state
                    .with_run(run, |r| r.leg(lid).is_some_and(|l| l.requires_management()))
                    .unwrap_or(false)
            });
            let waited = today > due_date
                || minute_of(time) as i64 - minute_of(due_time) as i64 > PENDING_WAIT_MINUTES;
            if busy && !waited {
                return;
            }
        }
        let _ = self.store.set_signal_pending(strategy.id, &Value::Null);
        let run_id = match self.day_run(strategy).await {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!("Strategy {}: pending re-entry not placed: {}", strategy.id, e);
                return;
            }
        };
        let reason = p["reason"].as_str().unwrap_or("roll").to_string();
        for cfg in configs {
            let lid = leg_id_of(&cfg).unwrap_or(1);
            let mut leg = cfg.clone();
            if let Some(after) = p["resolve_after"].as_str() {
                leg["_resolve_after"] = json!(after);
            }
            let r = self
                .signal_enter(strategy, run_id, &leg, lid, order_side(&cfg))
                .await;
            let (msg, sev) = if r.ok {
                (
                    format!(
                        "Leg {}: {} entry placed ({})",
                        lid,
                        if reason == "roll" { "expiry-roll" } else { "deferred" },
                        r.note.clone().unwrap_or_else(|| "ok".into())
                    ),
                    "info",
                )
            } else {
                (
                    format!(
                        "Leg {}: {} entry not placed: {}",
                        lid,
                        if reason == "roll" { "expiry-roll" } else { "deferred" },
                        r.error.clone().unwrap_or_default()
                    ),
                    "warn",
                )
            };
            self.emit(
                strategy.id,
                &strategy.user_id,
                "pending_entry_placed",
                &msg,
                EventFields {
                    run_id: Some(run_id),
                    leg_id: Some(lid),
                    severity: Some(sev),
                    ..Default::default()
                },
            )
            .await;
        }
    }

    /// One CAS pass: at the session end, the optional close; before it,
    /// each side's main-leg Supertrend advanced and a flip against the
    /// position closes that side's legs.
    async fn cas_pass(
        &self,
        strategy: &StrategyRow,
        opts: &SignalOptions,
        run_id: i64,
        today: NaiveDate,
        time: NaiveTime,
    ) {
        let id = strategy.id;
        if time >= opts.session_end {
            self.timers.cas.lock().retain(|(sid, _), _| *sid != id);
            if self.timers.cas_closed_day.lock().get(&id) == Some(&today) {
                return;
            }
            self.timers.cas_closed_day.lock().insert(id, today);
            if opts.cas_at_end == "close" {
                let open = self.open_resolved_legs(strategy, run_id);
                if !open.is_empty() {
                    for (cfg, _, _) in &open {
                        let lid = leg_id_of(cfg).unwrap_or(1);
                        let _ = self.signal_exit(strategy, run_id, cfg, lid, order_side(cfg)).await;
                    }
                    self.emit(
                        strategy.id,
                        &strategy.user_id,
                        "cas_exit",
                        &format!(
                            "Closing session ended at {}: open legs closed (CAS exit: close at session end)",
                            opts.session_end.format("%H:%M")
                        ),
                        EventFields {
                            run_id: Some(run_id),
                            severity: Some("warn"),
                            ..Default::default()
                        },
                    )
                    .await;
                }
            }
            return;
        }
        let open = self.open_resolved_legs(strategy, run_id);
        for side in ["long", "short"] {
            let legs: Vec<&(Value, String, String)> =
                open.iter().filter(|(c, _, _)| c["side"].as_str() == Some(side)).collect();
            let key = (id, side.to_string());
            if legs.is_empty() {
                self.timers.cas.lock().remove(&key);
                continue;
            }
            let pos = |c: &Value| c["position"].as_str().unwrap_or("B").to_ascii_uppercase();
            let sold = legs.iter().find(|(c, _, _)| pos(c) == "S");
            let bought = legs.iter().find(|(c, _, _)| pos(c) == "B");
            let main = match opts.cas_leg.as_str() {
                "sell" => sold,
                "buy" => bought,
                _ => sold.or(bought),
            };
            let Some((cfg, symbol, exchange)) = main.copied() else {
                continue;
            };
            let favourable: i8 = if pos(cfg) == "S" { -1 } else { 1 };
            let exists = self
                .timers
                .cas
                .lock()
                .get(&key)
                .map(|t| t.symbol == *symbol && !t.done);
            let done = self.timers.cas.lock().get(&key).is_some_and(|t| t.symbol == *symbol && t.done);
            if done {
                continue;
            }
            if exists != Some(true) {
                if self.timers.cas_failed.lock().get(&(id, symbol.clone())) == Some(&today) {
                    continue;
                }
                match self.warm_cas(opts, symbol, exchange, favourable, today, time).await {
                    Ok((tracker, flipped)) => {
                        self.timers.cas.lock().insert(key.clone(), tracker);
                        if let Some(at) = flipped {
                            self.cas_close_side(strategy, run_id, side, symbol, at, opts).await;
                            if let Some(t) = self.timers.cas.lock().get_mut(&key) {
                                t.done = true;
                            }
                            continue;
                        }
                    }
                    Err(why) => {
                        self.timers.cas_failed.lock().insert((id, symbol.clone()), today);
                        self.emit(
                            strategy.id,
                            &strategy.user_id,
                            "cas_tracking_unavailable",
                            &format!(
                                "CAS exit not tracking {} today: {}. The position is held as with Ignore CAS.",
                                symbol, why
                            ),
                            EventFields {
                                run_id: Some(run_id),
                                severity: Some("warn"),
                                ..Default::default()
                            },
                        )
                        .await;
                        continue;
                    }
                }
            }
            let price = self
                .gateway
                .ltps(&[(symbol.clone(), exchange.clone())])
                .await
                .ok()
                .and_then(|v| v.into_iter().next().flatten());
            let Some(price) = price else { continue };
            let slot = slot_start(time, opts.cas_timeframe);
            let cas_start = minute_of(opts.cas_start);
            let flipped_at = {
                let mut map = self.timers.cas.lock();
                let Some(t) = map.get_mut(&key) else { continue };
                let mut flipped = None;
                match t.candle {
                    Some((s, h, l, _)) if s == slot => {
                        t.candle = Some((s, h.max(price), l.min(price), price));
                    }
                    Some((s, h, l, c)) => {
                        let before = t.st.trend;
                        let after = t.st.push(h, l, c);
                        if s >= cas_start
                            && t.st.ready()
                            && before == t.favourable
                            && after == -t.favourable
                        {
                            t.done = true;
                            flipped = Some(s);
                        }
                        t.candle = Some((slot, price, price, price));
                    }
                    None => t.candle = Some((slot, price, price, price)),
                }
                flipped
            };
            if let Some(at) = flipped_at {
                self.cas_close_side(strategy, run_id, side, symbol, at, opts).await;
            }
        }
    }

    /// Warm a tracker on the contract's own candles before the current
    /// slot. Also says whether a candle already closed inside the CAS
    /// window flipped against the position (a late start).
    async fn warm_cas(
        &self,
        opts: &SignalOptions,
        symbol: &str,
        exchange: &str,
        favourable: i8,
        today: NaiveDate,
        time: NaiveTime,
    ) -> Result<(CasTracker, Option<u32>), String> {
        let candles = self
            .gateway
            .history(
                symbol,
                exchange,
                &interval_name(opts.cas_timeframe),
                today - Duration::days(10),
                today,
            )
            .await?;
        let current = slot_start(time, opts.cas_timeframe);
        let cas_start = minute_of(opts.cas_start);
        let mut st = Supertrend::new(opts.cas_st_atr, opts.cas_st_mult);
        let mut flipped = None;
        let mut n = 0usize;
        for c in &candles {
            let Some(at) = DateTime::<Utc>::from_timestamp(c.timestamp, 0) else {
                continue;
            };
            let at = at.with_timezone(&Kolkata);
            let m = minute_of(at.time());
            if at.date_naive() > today || (at.date_naive() == today && m >= current) {
                continue;
            }
            let before = st.trend;
            let after = st.push(c.high, c.low, c.close);
            n += 1;
            if at.date_naive() == today
                && m >= cas_start
                && st.ready()
                && before == favourable
                && after == -favourable
                && flipped.is_none()
            {
                flipped = Some(m);
            }
        }
        if !st.ready() || n < opts.cas_st_atr + 1 {
            return Err(format!(
                "only {} earlier {} candles could be read for its Supertrend warm-up",
                n,
                interval_name(opts.cas_timeframe)
            ));
        }
        Ok((
            CasTracker {
                symbol: symbol.to_string(),
                exchange: exchange.to_string(),
                favourable,
                st,
                candle: None,
                done: flipped.is_some(),
            },
            flipped,
        ))
    }

    async fn cas_close_side(
        &self,
        strategy: &StrategyRow,
        run_id: i64,
        side: &str,
        symbol: &str,
        candle_minute: u32,
        opts: &SignalOptions,
    ) {
        let open = self.open_resolved_legs(strategy, run_id);
        for (cfg, _, _) in open.iter().filter(|(c, _, _)| c["side"].as_str() == Some(side)) {
            let lid = leg_id_of(cfg).unwrap_or(1);
            let _ = self.signal_exit(strategy, run_id, cfg, lid, order_side(cfg)).await;
        }
        self.emit(
            strategy.id,
            &strategy.user_id,
            "cas_exit",
            &format!(
                "CAS exit: {} premium Supertrend({}, {}) on {}-minute candles flipped against the {} position on the {:02}:{:02} candle; its legs were closed",
                symbol,
                opts.cas_st_atr,
                opts.cas_st_mult,
                opts.cas_timeframe,
                side,
                candle_minute / 60,
                candle_minute % 60
            ),
            EventFields {
                run_id: Some(run_id),
                severity: Some("warn"),
                ..Default::default()
            },
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_anchor_at_nine_fifteen() {
        let t = |h, m| NaiveTime::from_hms_opt(h, m, 30).unwrap();
        assert_eq!(slot_start(t(15, 14), 5), 15 * 60 + 10);
        assert_eq!(slot_start(t(15, 15), 5), 15 * 60 + 15);
        assert_eq!(slot_start(t(15, 19), 5), 15 * 60 + 15);
        assert_eq!(slot_start(t(15, 20), 3), 15 * 60 + 18);
        assert_eq!(slot_start(t(10, 14), 60), 9 * 60 + 15);
        assert_eq!(interval_name(60), "1h");
        assert_eq!(interval_name(5), "5m");
    }
}
