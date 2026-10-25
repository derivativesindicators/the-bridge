//! What the tick path needs of a run that does not change while it runs,
//! kept in memory so no tick waits on the database.
//!
//! Every tick of a held symbol used to read the run row and its strategy
//! (two synchronous SQLite reads on the async worker) and render the
//! strategy to a dict before evaluating; at a few hundred held legs the
//! tick consumer fell behind the feed. A running strategy's configuration
//! cannot change (edits, the Live opt-in and deletion are refused while it
//! runs; the webhook lock and token are not read here), so it is read
//! once per run, on the blocking pool, and kept until the run ends, as the
//! scalping monitor keeps its legs. A run that has ended is not in the live
//! state registry, so it is never evaluated; its entry is forgotten when it
//! finishes and pruned with every new one.
//!
//! The daily loss limit's banked P&L (the realized P&L of the strategy's
//! other runs this session) cannot change during the run either (one run
//! per strategy at a time); it is read once per session, also off the
//! async worker.
//!
//! **The tick reader never waits on the database** (PROD-REL-01). It
//! decides every price as it arrives, so a read there (a pool exhausted
//! for its 10 s, or a failing read tried again on every tick) held every
//! run's prices behind it until the market-data channel overran. Both are
//! read off the reader: when the run's prices are subscribed
//! (`follow_prices`, before the subscription), and by the evaluator before
//! it decides a polled price (`process_tick`). What the reader finds
//! missing it asks for in a task the module owns, and decides meanwhile as
//! a failed read always did: a run whose context is not read is not judged
//! on that price, and a session whose banked P&L is not read counts nothing
//! banked. A read that fails is asked for again after a backoff doubling
//! from `RETRY_FIRST` to `RETRY_MAX`, so a failing database costs a read
//! now and then, not one per tick.

use super::dispatch::RunMode;
use super::store::{RunRow, StrategyRow};
use super::StrategyModule;
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The wait before a failed read is asked for again, doubled per failure
/// after the first up to `RETRY_MAX`.
const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(30);

/// A strategy's daily loss limit and the P&L its other runs banked this
/// session (both `None` without a limit).
pub(crate) type SessionLoss = (Option<f64>, Option<f64>);

/// One running run's fixed inputs to the tick path.
pub struct TickContext {
    pub strategy: StrategyRow,
    /// `strategy.to_dict(false)`, what the risk core reads.
    pub strategy_dict: Value,
    pub mode: RunMode,
    /// When the run started (The Bridge 1.0.8: a run started before this
    /// session was carried overnight).
    pub run_started: Option<DateTime<Utc>>,
    /// The banked P&L and the session it was read for.
    banked: Mutex<Option<(DateTime<Utc>, f64)>>,
}

impl TickContext {
    /// The strategy's daily loss limit, when it has one.
    fn daily_loss_limit(&self) -> Option<f64> {
        self.strategy
            .daily_loss_limit_inr
            .map(f64::abs)
            .filter(|l| *l > 0.0)
    }

    /// The banked P&L read for the session that started at `since`.
    fn banked_since(&self, since: DateTime<Utc>) -> Option<f64> {
        match *self.banked.lock() {
            Some((at, banked)) if at == since => Some(banked),
            _ => None,
        }
    }
}

/// A read the tick reader asked for: out now, or failed and waiting until
/// it may be asked for again.
struct Asked {
    reading: bool,
    failures: u32,
    not_before: Instant,
}

/// The contexts of the running runs, bounded by the live state registry.
#[derive(Default)]
pub struct TickContexts {
    map: Mutex<HashMap<i64, Arc<TickContext>>>,
    /// The reads the reader asked for, by run; bounded the same way.
    asked: Mutex<HashMap<i64, Asked>>,
}

impl TickContexts {
    fn get(&self, run_id: i64) -> Option<Arc<TickContext>> {
        self.map.lock().get(&run_id).cloned()
    }

    /// Keep one, dropping those of runs no longer running.
    fn keep(&self, run_id: i64, ctx: Arc<TickContext>, running: &[i64]) {
        let mut map = self.map.lock();
        map.retain(|id, _| running.contains(id));
        map.insert(run_id, ctx);
        self.asked.lock().retain(|id, _| running.contains(id));
    }

    /// Forget a finished run's context.
    pub fn forget(&self, run_id: i64) {
        self.map.lock().remove(&run_id);
        self.asked.lock().remove(&run_id);
    }

    /// Contexts held (hygiene tests).
    pub fn len(&self) -> usize {
        self.map.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the reader starts a read for the run now: none is out and
    /// the backoff after the last failure has passed. Marks it out.
    fn ask(&self, run_id: i64, now: Instant) -> bool {
        let mut asked = self.asked.lock();
        let read = asked.entry(run_id).or_insert(Asked {
            reading: false,
            failures: 0,
            not_before: now,
        });
        if read.reading || now < read.not_before {
            return false;
        }
        read.reading = true;
        true
    }

    /// A read the reader asked for has ended: forgotten when everything
    /// was read, otherwise asked for again no sooner than the backoff.
    fn answered(&self, run_id: i64, read_all: bool, now: Instant) {
        let mut asked = self.asked.lock();
        if read_all {
            asked.remove(&run_id);
        } else if let Some(read) = asked.get_mut(&run_id) {
            read.reading = false;
            read.failures = read.failures.saturating_add(1);
            read.not_before = now + backoff(read.failures);
        }
    }
}

/// The wait after `failures` failed reads in a row.
fn backoff(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    RETRY_FIRST.saturating_mul(1 << doublings).min(RETRY_MAX)
}

impl StrategyModule {
    /// Read what the run's ticks need, then subscribe its instruments, so
    /// its first price finds them in memory.
    pub(crate) async fn follow_prices(&self, run_id: i64, symbols: &[(String, String)]) {
        self.read_tick_inputs(run_id).await;
        self.feed.add_run(run_id, symbols).await;
    }

    /// Read what memory does not hold yet of the run's tick inputs: its
    /// context and, with a daily loss limit, this session's banked P&L.
    /// Never on the tick reader. Says whether both are held now.
    pub(crate) async fn read_tick_inputs(&self, run_id: i64) -> bool {
        let Some(ctx) = self.tick_context(run_id).await else {
            return false;
        };
        self.read_banked(&ctx, run_id).await
    }

    /// What the tick reader decides the run's price with, from memory
    /// alone: the run's context and its session loss. `None` while the
    /// context is not read: the price is not judged, as when a read
    /// failed. Whatever is missing is read in a task the module owns.
    pub(crate) fn held_tick_inputs(
        self: &Arc<Self>,
        run_id: i64,
    ) -> Option<(Arc<TickContext>, SessionLoss)> {
        let Some(ctx) = self.tick_contexts.get(run_id) else {
            self.ask_tick_inputs(run_id);
            return None;
        };
        let Some(limit) = ctx.daily_loss_limit() else {
            return Some((ctx, (None, None)));
        };
        let banked = ctx.banked_since(self.session_start());
        if banked.is_none() {
            self.ask_tick_inputs(run_id);
        }
        // Nothing banked until it is read: what a failed read always
        // counted.
        Some((ctx, (Some(limit), Some(banked.unwrap_or(0.0)))))
    }

    /// Read the run's missing tick inputs in an owned task (ended on
    /// module shutdown), unless a read is out or backing off.
    fn ask_tick_inputs(self: &Arc<Self>, run_id: i64) {
        if !self.tick_contexts.ask(run_id, Instant::now()) {
            return;
        }
        let module = Arc::downgrade(self);
        self.spawn(async move {
            let Some(m) = module.upgrade() else { return };
            let read_all = m.read_tick_inputs(run_id).await;
            m.tick_contexts.answered(run_id, read_all, Instant::now());
        });
    }

    /// The run's tick context, read on the blocking pool the first time.
    /// `None` when the run has ended or cannot be read.
    async fn tick_context(&self, run_id: i64) -> Option<Arc<TickContext>> {
        if let Some(ctx) = self.tick_contexts.get(run_id) {
            return Some(ctx);
        }
        let store = self.store.clone();
        let read = tokio::task::spawn_blocking(move || -> crate::error::Result<_> {
            let Some(run) = store.get_run(run_id)? else {
                return Ok(None);
            };
            let strategy = store.get_strategy_unscoped(run.strategy_id)?;
            Ok(strategy.map(|s| (run, s)))
        })
        .await;
        let (run, strategy): (RunRow, StrategyRow) = match read {
            Ok(Ok(Some(found))) => found,
            Ok(Ok(None)) => return None,
            Ok(Err(e)) => {
                tracing::error!(
                    "Run {} was not evaluated: it could not be read: {}",
                    run_id,
                    e
                );
                return None;
            }
            Err(e) => {
                tracing::error!("Run {} was not evaluated: {}", run_id, e);
                return None;
            }
        };
        if run.stopped_at.is_some() {
            return None;
        }
        let ctx = Arc::new(TickContext {
            strategy_dict: strategy.to_dict(false),
            mode: self.mode_of(&run.mode),
            run_started: super::store::parse_utc(&run.started_at),
            strategy,
            banked: Mutex::new(None),
        });
        self.tick_contexts
            .keep(run_id, ctx.clone(), &self.state.active_run_ids());
        Some(ctx)
    }

    /// This session's banked P&L, read on the blocking pool unless held or
    /// the strategy has no daily loss limit. Says whether it is held now.
    async fn read_banked(&self, ctx: &TickContext, run_id: i64) -> bool {
        if ctx.daily_loss_limit().is_none() {
            return true;
        }
        let since = self.session_start();
        if ctx.banked_since(since).is_some() {
            return true;
        }
        let store = self.store.clone();
        let strategy_id = ctx.strategy.id;
        let read = tokio::task::spawn_blocking(move || {
            store.realized_pnl_since(strategy_id, since, Some(run_id))
        })
        .await;
        match read {
            Ok(Ok(banked)) => {
                *ctx.banked.lock() = Some((since, banked));
                true
            }
            Ok(Err(e)) => {
                tracing::error!("Banked P&L of strategy {} not read: {}", strategy_id, e);
                false
            }
            Err(e) => {
                tracing::error!("Banked P&L of strategy {} not read: {}", strategy_id, e);
                false
            }
        }
    }

    /// When this session began: what banked P&L is read for.
    pub(crate) fn session_start(&self) -> DateTime<Utc> {
        super::session::session_started_at(self.clock.now(), self.session.0, self.session.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_backoff_doubles_to_its_cap() {
        let secs: Vec<u64> = [1, 2, 3, 5, 6, 40, u32::MAX]
            .iter()
            .map(|f| backoff(*f).as_secs())
            .collect();
        assert_eq!(secs, vec![1, 2, 4, 16, 30, 30, 30]);
    }

    /// One read out at a time per run, a failed one asked for again only
    /// after its backoff, and a read that landed forgotten.
    #[test]
    fn a_failed_read_is_asked_for_again_only_after_its_backoff() {
        let contexts = TickContexts::default();
        let t0 = Instant::now();
        assert!(contexts.ask(7, t0));
        assert!(!contexts.ask(7, t0), "one read out at a time");
        assert!(contexts.ask(8, t0), "per run");
        contexts.answered(7, false, t0);
        assert!(!contexts.ask(7, t0 + Duration::from_millis(999)));
        assert!(contexts.ask(7, t0 + RETRY_FIRST));
        let at = t0 + RETRY_FIRST;
        contexts.answered(7, false, at);
        assert!(!contexts.ask(7, at + RETRY_FIRST), "doubled");
        assert!(contexts.ask(7, at + 2 * RETRY_FIRST));
        contexts.answered(7, true, at);
        assert!(
            !contexts.asked.lock().contains_key(&7),
            "forgotten once read"
        );
        contexts.forget(8);
        assert!(contexts.asked.lock().is_empty());
    }
}
