//! What the run's page asks of the runner: its spec, bars and inbox, log
//! lines, the end of a run, and the intents the strategy decided (each
//! checked against the run and sent as an order).

use super::inbox::Message;
use super::orders::{Planned, Target};
use super::{orders, repeats, store};
use super::{PageState, Refusal, RunEntry, Runner, MAX_LOG_LINES};
use chrono::Duration as ChronoDuration;
use chrono_tz::Asia::Kolkata;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

impl Runner {
    fn entry_for(&self, run_id: &str, token: &str) -> Option<Arc<RunEntry>> {
        let entry = self.reg.lock().runs.get(run_id).cloned()?;
        if !crate::server::form::tokens_match(token, &entry.token) {
            return None;
        }
        *entry.last_seen.lock() = Instant::now();
        Some(entry)
    }

    /// Whether this token belongs to this running run.
    pub fn authenticate(&self, run_id: &str, token: &str) -> bool {
        self.entry_for(run_id, token).is_some()
    }

    /// What the page needs to load the program, with the inputs its run
    /// started with (never read again here: see `RunEntry::inputs`).
    pub fn page_spec(&self, run_id: &str, token: &str) -> Option<Result<Value, Refusal>> {
        let entry = self.entry_for(run_id, token)?;
        let info = &entry.info;
        let Some(program) = self.scripts.program(&info.script) else {
            return Some(Err(Refusal::new(
                409,
                format!("{} has no compiled program any more.", info.script),
            )));
        };
        let program = String::from_utf8_lossy(&program).into_owned();
        let inputs = Value::Object(entry.inputs.clone());
        let today = self.clock.now().with_timezone(&Kolkata).date_naive();
        let facts = self
            .services()
            .instrument(&info.symbol, &info.exchange, today);
        Some(Ok(json!({
            "status": "success",
            "run": info.answer(),
            "program": program,
            "inputs": inputs,
            "facts": facts,
            "product": orders::product_for(&info.product),
            "lotSize": self.services().lot_size(&info.symbol, &info.exchange),
            "barPollMs": self.options.read().bar_poll.as_millis() as u64,
        })))
    }

    /// The history a run is replayed over; sets the anchor new bars follow.
    pub async fn page_bars(&self, run_id: &str, token: &str) -> Option<Result<Value, Refusal>> {
        let entry = self.entry_for(run_id, token)?;
        let info = &entry.info;
        let today = self.clock.now().with_timezone(&Kolkata).date_naive();
        let days = self.options.read().history_days.max(1);
        let start = today - ChronoDuration::days(days);
        let got = self
            .services()
            .history(&info.symbol, &info.exchange, &info.interval, start, today)
            .await;
        Some(match got {
            Ok(bars) => {
                if let Some(last) = bars.last() {
                    *entry.anchor.lock() = Some(last.time);
                    *entry.last_bar.lock() = Some(last.json());
                }
                Ok(json!({
                    "status": "success",
                    "data": bars.iter().map(|b| b.json()).collect::<Vec<_>>(),
                }))
            }
            Err(why) => Err(Refusal::new(
                502,
                if why.is_empty() {
                    "History could not be read just now.".to_string()
                } else {
                    why
                },
            )),
        })
    }

    /// The page's long poll.
    pub async fn page_inbox(&self, run_id: &str, token: &str, after: u64) -> Option<Value> {
        let entry = self.entry_for(run_id, token)?;
        let wait = self.options.read().inbox_wait;
        let (messages, closed) = entry.inbox.next(after, wait).await;
        *entry.last_seen.lock() = Instant::now();
        Some(json!({"status": "success", "messages": messages, "stop": closed}))
    }

    /// Lines the page writes into its run's log, and what it says of a
    /// batch whose answer it never got: whether it is still waiting, and
    /// whether it has waited long enough that the trader must be told.
    pub fn page_log(&self, run_id: &str, token: &str, lines: &[String], state: PageState) -> bool {
        let Some(entry) = self.entry_for(run_id, token) else {
            return false;
        };
        if let Some(waiting) = state.waiting {
            entry.page_waiting.store(waiting, Ordering::SeqCst);
            if !waiting {
                self.stuck_over(&entry);
            }
        }
        if state.stuck && !entry.stuck_told.swap(true, Ordering::SeqCst) {
            self.tell_stuck(&entry);
        }
        let kept: Vec<String> = lines
            .iter()
            .take(MAX_LOG_LINES)
            .map(|l| l.chars().take(2000).collect())
            .collect();
        self.write_log(&entry.info, &kept);
        true
    }

    /// A batch has had no answer for over a minute: nothing new is sent,
    /// the run's own exits included, until its outcome is known. Said in
    /// the run's log and on the Health page, once per wait (NEW-TR-04).
    fn tell_stuck(&self, entry: &RunEntry) {
        let message = format!(
            "OpenScript {} on {}: an order of this strategy has had no answer from The Bridge for over a minute, so whether it was placed is not known. Until it is, this strategy sends nothing new, its own exits included. Check your order book{}.",
            entry.info.script,
            entry.info.symbol,
            if entry.info.mode == crate::strategy::dispatch::RunMode::Live {
                " at your broker"
            } else {
                ""
            }
        );
        tracing::error!("{}", message);
        self.write_log(&entry.info, std::slice::from_ref(&message));
        self.services().alert(message);
    }

    /// A run's wait is over (its page has its answer, or the run ended):
    /// the Health alert goes once no run waits any more.
    pub(super) fn stuck_over(&self, entry: &RunEntry) {
        if !entry.stuck_told.swap(false, Ordering::SeqCst) {
            return;
        }
        let others = self
            .reg
            .lock()
            .runs
            .values()
            .any(|e| e.stuck_told.load(Ordering::SeqCst));
        if !others {
            self.services().alert_cleared();
        }
    }

    /// The page says its script stopped.
    pub fn page_ended(&self, run_id: &str, token: &str, message: &str) -> bool {
        if self.entry_for(run_id, token).is_none() {
            return false;
        }
        let why: String = message.chars().take(500).collect();
        self.end_run(
            run_id,
            &format!(
                "Stopped: {}",
                if why.is_empty() {
                    "the script ended"
                } else {
                    &why
                }
            ),
        );
        true
    }

    /// Intents from a confirmed bar of the run `incarnation`. Each answers
    /// with a frame in the inbox.
    ///
    /// **The batch runs to its end even when the page's request does not**
    /// (NEW-TR-04, `batches`). A placement cut off between sending and
    /// saving its answer is the hazard; a reply the page never reads is
    /// not, because the page sends the same batch again and is answered
    /// from the record (`repeats`).
    pub async fn page_intents(
        &self,
        run_id: &str,
        token: &str,
        incarnation: Option<i64>,
        intents: Vec<Value>,
    ) -> Option<Result<Value, Refusal>> {
        let entry = self.entry_for(run_id, token)?;
        // Every intent is keyed by its number before anything is sent.
        if let Err(why) = repeats::check_numbers(&intents) {
            return Some(Err(why));
        }
        let Some(runner) = self.me.upgrade() else {
            return Some(Ok(
                json!({"status": "error", "message": "The Bridge is shutting down."}),
            ));
        };
        let answer =
            self.spawn_batch(async move { runner.submit(&entry, incarnation, &intents).await });
        Some(Ok(answer.await.unwrap_or_else(|_| {
            tracing::error!("An OpenScript order batch did not finish");
            json!({"status": "error", "message": "The orders of this bar could not be handled."})
        })))
    }

    /// One batch, under the run's order lock: an intent answered already is
    /// answered the same again (`repeats`), the rest are checked and sent.
    async fn submit(
        &self,
        entry: &Arc<RunEntry>,
        incarnation: Option<i64>,
        intents: &[Value],
    ) -> Value {
        let held = entry.order_lock.lock().await;
        // The page's secret names its run; a batch that names another run
        // (a request replayed across a restart) places nothing.
        if let Some(other) = incarnation.filter(|n| *n != entry.info.incarnation) {
            return self.answer_other_run(entry, other, intents);
        }
        let fresh = match self.answer_repeats(entry, intents) {
            Ok(fresh) => fresh,
            Err(why) => return json!({"status": "error", "message": why}),
        };
        let repeated = intents.len() - fresh.len();
        let mut reply = if fresh.is_empty() {
            json!({"status": "success", "sent": 0})
        } else {
            self.send_intents(entry, held, &fresh).await
        };
        if repeated > 0 {
            reply["repeated"] = json!(repeated);
        }
        reply
    }

    /// Check a batch's new intents against the run and send what may go.
    async fn send_intents(
        &self,
        entry: &Arc<RunEntry>,
        held: tokio::sync::MutexGuard<'_, ()>,
        intents: &[Value],
    ) -> Value {
        let info = entry.info.clone();
        let own = match self.bar_gate(entry) {
            Ok(own) => own,
            Err((text, flag)) => {
                self.reject_intents(entry, intents, text);
                let mut reply = json!({"status": "success", "sent": 0});
                reply[flag] = json!(true);
                return reply;
            }
        };
        let services = self.services();
        let lot = services.lot_size(&info.symbol, &info.exchange);
        let planned: Vec<Planned> = intents.iter().map(|i| orders::plan(i, lot)).collect();
        if let Some(Planned::Unroutable { kind, .. }) = planned
            .iter()
            .find(|p| matches!(p, Planned::Unroutable { .. }))
        {
            self.reject_intents(
                entry,
                intents,
                "this runner cannot send that order as one piece",
            );
            let why = format!(
                "{} asked for an order this runner cannot send as one piece ({}). Nothing was sent for this bar and the run has stopped.",
                info.script, kind
            );
            drop(held);
            self.end_run(&info.run_id, &why);
            return json!({"status": "success", "sent": 0, "stopped": true});
        }
        let product = orders::product_for(&info.product);
        let target = Target {
            strategy: &info.run_id,
            symbol: &info.symbol,
            exchange: &info.exchange,
            product: &product,
        };
        // Order facts on this side may be missing after lost order updates
        // (ARCH-01): no new exposure until the order book has been read
        // again. An order that only reduces what this run holds still goes,
        // as a strategy's exits do; Stop is never held by this.
        let paused = services.entry_refusal(info.mode);
        let mut room = orders::ExitRoom::of(&own, &info.symbol, &info.exchange, &product);
        let mut sent = 0;
        for p in planned {
            match p {
                Planned::Place(placement) => {
                    if let Some(why) = paused.as_deref() {
                        if !room.take(placement.action, placement.quantity) {
                            self.write_log(
                                &info,
                                &[format!(
                                    "{} {} {} was not sent: {}",
                                    placement.action, placement.quantity, info.symbol, why
                                )],
                            );
                            self.reject_intent(entry, placement.intent_id, why);
                            continue;
                        }
                    }
                    match self
                        .send(&info, &target, &placement, Some(entry), false)
                        .await
                    {
                        Ok((_, r)) => sent += usize::from(r.ok),
                        Err(e) => {
                            tracing::error!("Could not record an OpenScript order: {}", e);
                            self.reject_intent(
                                entry,
                                placement.intent_id,
                                "the order could not be recorded",
                            );
                        }
                    }
                }
                Planned::Cancel { intent_id, tag } => {
                    self.cancel_tagged(entry, &tag).await;
                    self.remember(entry, intent_id, None);
                }
                Planned::Refuse { intent_id, reason } => {
                    self.write_log(&info, &[format!("An order was not sent: {}", reason)]);
                    self.reject_intent(entry, intent_id, &reason);
                }
                Planned::Unroutable { .. } => {}
            }
        }
        json!({"status": "success", "sent": sent})
    }

    /// Whether a bar's intents may be sent at all, under the run's order
    /// lock: its own orders when they may, else the text each intent is
    /// rejected with and the reply's flag.
    fn bar_gate(
        &self,
        entry: &RunEntry,
    ) -> Result<Vec<store::OrderRow>, (&'static str, &'static str)> {
        if entry.halted.load(Ordering::SeqCst) {
            return Err(("this run is stopping, so nothing further is sent", "halted"));
        }
        // An earlier order's answer not yet saved: its fill cannot be
        // counted, so a new order would be sized and sent blind.
        self.flush_unsaved(entry);
        if !entry.unsaved.lock().is_empty() {
            return Err((
                "an earlier order of this run could not be recorded yet, so nothing new is sent",
                "unsaved",
            ));
        }
        // An order the destination did not confirm may be there: a new
        // order would be sized without it, so nothing is sent until the
        // order book settles it (LOG-08). A ledger that cannot be read is
        // treated the same way.
        let Ok(own) = self.read_own(&entry.info) else {
            return Err((
                "this run's own orders could not be read, so nothing new is sent",
                "unconfirmed",
            ));
        };
        if own.iter().any(|o| {
            o.orderid.is_none() && (o.status == store::UNCONFIRMED || o.status == "placed")
        }) {
            return Err((
                "an earlier order of this run was not confirmed, so nothing new is sent until the order book settles it",
                "unconfirmed",
            ));
        }
        Ok(own)
    }

    /// Answer one intent with a rejected frame, the answer it gets again if
    /// its page asks again (`repeats`).
    fn reject_intent(&self, entry: &RunEntry, id: i64, text: &str) {
        let now_ms = self.clock.now().timestamp_millis();
        let frame = orders::frame(id, "rejected", 0, None, None, Some(text), now_ms);
        self.remember(entry, id, Some(frame.clone()));
        entry.inbox.push(Message::Frame {
            intent_id: id,
            frame,
        });
    }

    /// Answer every intent of a bar with a rejected frame.
    fn reject_intents(&self, entry: &RunEntry, intents: &[Value], text: &str) {
        for i in intents {
            let id = i.get("intentId").and_then(Value::as_i64).unwrap_or(-1);
            self.reject_intent(entry, id, text);
        }
    }

    /// Ask for every working order this run's engine placed tagged `tag` to
    /// be cancelled, saying in the run's log what came of each. An earlier
    /// run's order of the same tag is not this engine's (NEW-TR-03).
    async fn cancel_tagged(&self, entry: &RunEntry, tag: &str) {
        let info = &entry.info;
        let rows = match self.read_own(info) {
            Ok(rows) => rows,
            Err(why) => {
                self.write_log(
                    info,
                    &[format!(
                        "The orders tagged {} were not cancelled: {}",
                        tag, why
                    )],
                );
                Vec::new()
            }
        };
        for o in rows.into_iter().filter(|o| {
            o.tag == tag
                && o.incarnation == Some(info.incarnation)
                && !store::TERMINAL.contains(&o.status.as_str())
        }) {
            if let Some(oid) = o.orderid.as_deref() {
                let r = self.cancel_own(info, oid).await;
                self.write_log(
                    info,
                    &[if r.ok {
                        format!("Asked for order {} to be cancelled.", oid)
                    } else {
                        format!(
                            "Order {} could not be cancelled: {}",
                            oid,
                            r.error.unwrap_or_default()
                        )
                    }],
                );
            }
        }
    }
}
