//! The webhook pipeline (web `webhook_bridge.py`): token, kill switch,
//! address, payload, action, mode, the live gate, the dedupe and cooling
//! windows, then dispatch, each refusal audited.

use super::{
    digest_matches, ip_allowed, looks_like_token, parse_payload, redact, Audit, StrategyModule,
    WebhookOutcome, COOLING_OFF, DEDUPE_WINDOW, MAX_TRACKED_KEYS,
};
use crate::strategy::store::{StrategyRow, RUN_MODES};
use serde_json::{Map, Value};
use std::time::Instant;

/// A webhook's dedupe key: the strategy, the action and the mode.
type WebhookKey = (i64, String, Option<String>);

/// An outcome stamped with its strategy and audit row.
fn stamped(mut o: WebhookOutcome, sid: i64, id: Option<i64>) -> WebhookOutcome {
    o.strategy_id = Some(sid);
    o.webhook_event_id = id;
    o
}

/// The alert's action, one this strategy kind accepts. `Err`: the audited
/// refusal.
fn webhook_action(
    audit: &Audit<'_>,
    strategy: &StrategyRow,
    payload: &Map<String, Value>,
    safe: &Value,
) -> Result<String, Box<WebhookOutcome>> {
    let sid = strategy.id;
    let allowed = crate::strategy::signals::actions_for(&strategy.strategy_kind);
    let raw_action = payload.get("action");
    let action = raw_action
        .and_then(Value::as_str)
        .map(|a| a.trim().to_ascii_lowercase());
    let Some(action) = action.filter(|a| allowed.contains(&a.as_str())) else {
        let shown = match raw_action {
            Some(Value::String(s)) => Some(s.trim().to_ascii_lowercase()),
            Some(Value::Null) | None => None,
            Some(other) => Some(other.to_string()),
        };
        let id = audit.write(
            "rejected_invalid_action",
            Some(sid),
            shown.as_deref(),
            None,
            Some(safe),
            Some("Unrecognised action"),
        );
        return Err(Box::new(stamped(
            WebhookOutcome::new(
                "rejected_invalid_action",
                format!("'action' must be one of {}", allowed.join(", ")),
                false,
            ),
            sid,
            id,
        )));
    };
    Ok(action)
}

/// The run mode a start asks for (a stop ignores it), and the live gate:
/// Live only when the strategy opted in. `Err`: the audited refusal.
fn webhook_mode(
    audit: &Audit<'_>,
    strategy: &StrategyRow,
    action: &str,
    (payload, safe): (&Map<String, Value>, &Value),
) -> Result<Option<String>, Box<WebhookOutcome>> {
    let sid = strategy.id;
    let mut mode: Option<String> = None;
    if action == "start" {
        let raw = payload.get("mode");
        let m = raw
            .and_then(Value::as_str)
            .map(|m| m.trim().to_ascii_lowercase());
        match m.filter(|m| RUN_MODES.contains(&m.as_str())) {
            Some(m) => mode = Some(m),
            None => {
                let shown = match raw {
                    Some(Value::String(s)) => Some(s.trim().to_ascii_lowercase()),
                    Some(Value::Null) | None => None,
                    Some(o) => Some(o.to_string()),
                };
                let id = audit.write(
                    "rejected_invalid_action",
                    Some(sid),
                    Some(action),
                    shown.as_deref(),
                    Some(safe),
                    Some("Unrecognised mode"),
                );
                return Err(Box::new(stamped(
                    WebhookOutcome::new(
                        "rejected_invalid_action",
                        format!(
                            "'mode' must be one of {} when starting",
                            RUN_MODES.join(", ")
                        ),
                        false,
                    ),
                    sid,
                    id,
                )));
            }
        }
        // 7. The live gate.
        if mode.as_deref() == Some("live") && !strategy.live_enabled {
            let id = audit.write(
                "rejected_live_disabled",
                Some(sid),
                Some(action),
                mode.as_deref(),
                Some(safe),
                Some("Live trading is not enabled for this strategy"),
            );
            return Err(Box::new(stamped(
                WebhookOutcome::new(
                    "rejected_live_disabled",
                    "Live trading is not enabled for this strategy",
                    false,
                ),
                sid,
                id,
            )));
        }
    }
    Ok(mode)
}

impl StrategyModule {
    /// Run one inbound alert through the pipeline and act on it, or refuse
    /// it. Never fails: an unexpected error is `rejected_engine_error`.
    pub async fn handle_webhook(
        &self,
        token: &str,
        body: &[u8],
        ip: Option<&str>,
        ua: Option<&str>,
    ) -> WebhookOutcome {
        let audit = Audit {
            module: self,
            ip,
            ua,
        };
        // 1. Token: shape first (no lookup for a non-token), then the digest.
        let strategy = match self.webhook_strategy(token, ip, ua) {
            Ok(s) => s,
            Err(refused) => return *refused,
        };
        let sid = strategy.id;

        // 2 and 3. The kill switch and the address, before the body is parsed.
        if let Some(refused) = self.gate_refusal(&audit, &strategy).await {
            return refused;
        }
        // 4. Payload.
        let payload = match parse_payload(body) {
            Ok(p) => p,
            // The Bridge 1.0.7: a signal strategy also takes a plain-text
            // message (e.g. `BUY`, `CLOSE LONG: Normal Exit`), mapped to an
            // action by its `webhook_messages` rules.
            Err(_)
                if strategy.strategy_kind == "signal"
                    && body.len() <= super::MAX_PAYLOAD_BYTES
                    && super::messages::body_text(body).is_some() =>
            {
                let text = super::messages::body_text(body).unwrap_or_default();
                match super::messages::action_for(&text, &strategy.webhook_messages) {
                    Some(action) => {
                        let mut m = Map::new();
                        m.insert("action".into(), Value::String(action));
                        m.insert("message".into(), Value::String(text));
                        m
                    }
                    None => {
                        let shown: String = text.chars().take(80).collect();
                        let e = format!(
                            "The message '{}' matches none of this strategy's alert messages. Add it under Webhook messages, or send JSON such as {{\"action\": \"long_entry\"}}.",
                            redact(&Value::String(shown), token, 0).as_str().unwrap_or("")
                        );
                        let id =
                            audit.write("rejected_payload", Some(sid), None, None, None, Some(&e));
                        return stamped(WebhookOutcome::new("rejected_payload", e, false), sid, id);
                    }
                }
            }
            Err(e) => {
                let id = audit.write("rejected_payload", Some(sid), None, None, None, Some(&e));
                return stamped(WebhookOutcome::new("rejected_payload", e, false), sid, id);
            }
        };
        let safe = redact(&Value::Object(payload.clone()), token, 0);

        // 5. Action, by kind.
        let action = match webhook_action(&audit, &strategy, &payload, &safe) {
            Ok(a) => a,
            Err(refused) => return *refused,
        };

        // 5b. Signal mode branches out before the batch-only stages.
        if crate::strategy::signals::SIGNAL_ACTIONS.contains(&action.as_str()) {
            return self
                .signal_webhook(&audit, &strategy, &action, (&payload, &safe))
                .await;
        }

        // 6 and 7. Mode, required by start, ignored by stop, and the live gate.
        let mode = match webhook_mode(&audit, &strategy, &action, (&payload, &safe)) {
            Ok(m) => m,
            Err(refused) => return *refused,
        };

        // 8 and 9: both windows read and the claim written under one lock.
        let key = (sid, action.clone(), mode.clone());
        let now = self.webhook.now();
        if let Some(refused) = self.claim_window(&audit, &key, now, &safe) {
            return refused;
        }

        // 10. Dispatch.
        self.dispatch_webhook(&audit, &strategy, (&key, now), &safe)
            .await
    }

    /// The strategy a webhook token names: a token's shape first (no
    /// lookup for a non-token), then the digest lookup confirmed in constant
    /// time, so the comparison leaks nothing about it. `Err`: the unknown
    /// token's answer.
    fn webhook_strategy(
        &self,
        token: &str,
        ip: Option<&str>,
        ua: Option<&str>,
    ) -> Result<StrategyRow, Box<WebhookOutcome>> {
        if !looks_like_token(token) {
            return Err(Box::new(self.unknown_token_outcome(ip, ua)));
        }
        match self.store.get_strategy_by_webhook_token(token) {
            // The row was found through the digest index; confirm the digest
            // in constant time so the comparison leaks nothing about it.
            Ok(Some(s)) if digest_matches(token, &s.webhook_token_hash) => Ok(s),
            Ok(_) => Err(Box::new(self.unknown_token_outcome(ip, ua))),
            Err(e) => {
                tracing::error!("Could not resolve a webhook token: {}", e);
                Err(Box::new(self.unknown_token_outcome(ip, ua)))
            }
        }
    }

    /// The kill switch, ahead of everything a caller controls, then the
    /// caller's address, before the body is parsed. A refused address
    /// counts as a failed authentication on this webhook, whatever the
    /// source address, so rotating addresses locks it.
    async fn gate_refusal(
        &self,
        audit: &Audit<'_>,
        strategy: &StrategyRow,
    ) -> Option<WebhookOutcome> {
        let sid = strategy.id;
        if strategy.webhook_locked {
            let id = audit.write(
                "rejected_locked",
                Some(sid),
                None,
                None,
                None,
                Some("The webhook kill switch is engaged"),
            );
            return Some(stamped(
                WebhookOutcome::new(
                    "rejected_locked",
                    "This strategy's webhook is locked",
                    false,
                ),
                sid,
                id,
            ));
        }
        if !ip_allowed(audit.ip, &strategy.webhook_ip_allowlist) {
            let id = audit.write(
                "rejected_ip",
                Some(sid),
                None,
                None,
                None,
                Some("The caller address is outside the allowlist"),
            );
            // A failed authentication on this webhook, counted per webhook
            // whatever the source address, so rotating addresses locks it.
            if self.webhook.record_webhook_failure(sid) {
                self.lock_webhook_after_failures(strategy).await;
            }
            return Some(stamped(
                WebhookOutcome::new(
                    "rejected_ip",
                    "This address is not allowed to trigger this strategy",
                    false,
                ),
                sid,
                id,
            ));
        }
        None
    }

    /// A signal's action: run through the signal path, the answer audited.
    async fn signal_webhook(
        &self,
        audit: &Audit<'_>,
        strategy: &StrategyRow,
        action: &str,
        (payload, safe): (&Map<String, Value>, &Value),
    ) -> WebhookOutcome {
        let sid = strategy.id;
        let leg_id = payload.get("leg_id");
        let symbol = payload.get("symbol").and_then(Value::as_str);
        let exchange = payload.get("exchange").and_then(Value::as_str);
        let r = self
            .handle_signal(strategy, action, leg_id, symbol, exchange)
            .await;
        if !r.ok {
            let err = r
                .error
                .clone()
                .unwrap_or_else(|| "The signal was refused".into());
            let id = audit.write(
                "rejected_invalid_action",
                Some(sid),
                Some(action),
                None,
                Some(safe),
                Some(&err),
            );
            return stamped(
                WebhookOutcome::new("rejected_invalid_action", err, false),
                sid,
                id,
            );
        }
        let id = audit.write("ok", Some(sid), Some(action), None, Some(safe), None);
        let message = match &r.note {
            Some(n) => format!("Signal accepted ({})", n),
            None => "Signal accepted".into(),
        };
        let mut o = stamped(WebhookOutcome::new("ok", message, true), sid, id);
        o.run_id = r.run_id;
        o
    }

    /// Claims the signal's dedupe window and checks the cooling-off after
    /// a stop, both under one lock; the refusal when either holds.
    fn claim_window(
        &self,
        audit: &Audit<'_>,
        key: &WebhookKey,
        now: Instant,
        safe: &Value,
    ) -> Option<WebhookOutcome> {
        let (sid, action, mode) = key;
        let sid = *sid;
        let refusal = {
            let mut dedupe = self.webhook.dedupe.lock();
            dedupe.retain(|_, at| now.saturating_duration_since(*at) < DEDUPE_WINDOW);
            if dedupe.len() >= MAX_TRACKED_KEYS {
                dedupe.clear();
            }
            let cooling = if action == "start" {
                self.webhook.cooling_remaining(sid, now)
            } else {
                0
            };
            if dedupe.contains_key(key) {
                Some(("rejected_dedupe", 0))
            } else if cooling > 0 {
                Some(("rejected_cooling_off", cooling))
            } else {
                dedupe.insert(key.clone(), now);
                None
            }
        };
        match refusal {
            Some(("rejected_dedupe", _)) => {
                let id = audit.write(
                    "rejected_dedupe",
                    Some(sid),
                    Some(action.as_str()),
                    mode.as_deref(),
                    Some(safe),
                    Some(&format!(
                        "Duplicate signal inside {}s",
                        DEDUPE_WINDOW.as_secs()
                    )),
                );
                return Some(stamped(
                    WebhookOutcome::new(
                        "rejected_dedupe",
                        format!(
                            "Duplicate signal ignored, already handled within {}s",
                            DEDUPE_WINDOW.as_secs()
                        ),
                        true,
                    ),
                    sid,
                    id,
                ));
            }
            Some((_, remaining)) => {
                let id = audit.write(
                    "rejected_cooling_off",
                    Some(sid),
                    Some(action.as_str()),
                    mode.as_deref(),
                    Some(safe),
                    Some(&format!(
                        "Stopped within the last {}s",
                        COOLING_OFF.as_secs()
                    )),
                );
                return Some(stamped(
                    WebhookOutcome::new(
                        "rejected_cooling_off",
                        format!(
                            "This strategy stopped recently, try again in {}s",
                            remaining
                        ),
                        false,
                    ),
                    sid,
                    id,
                ));
            }
            None => {}
        }
        None
    }

    /// Dispatches an accepted start or stop. The accepted row is written
    /// first, so the run can point at it; an engine refusal releases the
    /// dedupe claim so the sender's retry is not swallowed.
    async fn dispatch_webhook(
        &self,
        audit: &Audit<'_>,
        strategy: &StrategyRow,
        (key, now): (&WebhookKey, Instant),
        safe: &Value,
    ) -> WebhookOutcome {
        let (sid, action, mode) = key;
        let sid = *sid;
        let event_id = audit.write(
            "ok",
            Some(sid),
            Some(action.as_str()),
            mode.as_deref(),
            Some(safe),
            None,
        );
        let was_live = strategy.status != "stopped";
        let (ok, run_id, error, stop_pending, exits) = if action == "start" {
            let r = self
                .start_run(
                    sid,
                    &strategy.user_id,
                    mode.as_deref().unwrap_or("sandbox"),
                    "webhook",
                    event_id,
                )
                .await;
            (r.ok, r.run_id, r.error, None, vec![])
        } else {
            self.bridge_stop(strategy).await
        };
        if !ok {
            // Release the claim so the sender's retry is not swallowed.
            self.webhook.dedupe.lock().remove(key);
            let err = error.unwrap_or_else(|| "The engine could not act on the signal".into());
            let id = audit.write(
                "rejected_engine_error",
                Some(sid),
                Some(action.as_str()),
                mode.as_deref(),
                Some(safe),
                Some(&err),
            );
            let mut o = stamped(
                WebhookOutcome::new("rejected_engine_error", err, false),
                sid,
                id,
            );
            o.run_id = run_id;
            o.stop_pending = stop_pending;
            o.exits = exits;
            return o;
        }
        if action == "stop" && was_live && stop_pending != Some(true) {
            self.webhook.note_run_stopped(sid, now);
        }
        let mut o = stamped(
            WebhookOutcome::new("ok", format!("Strategy {} accepted", action), true),
            sid,
            event_id,
        );
        o.run_id = run_id;
        o.stop_pending = stop_pending;
        o.exits = exits;
        o
    }

    /// Web `webhook_bridge.stop_run`: a stop applies to the current run, and
    /// stopping an already-flat strategy is a success.
    async fn bridge_stop(
        &self,
        strategy: &StrategyRow,
    ) -> (bool, Option<i64>, Option<String>, Option<bool>, Vec<Value>) {
        let current = self
            .store
            .get_strategy_unscoped(strategy.id)
            .ok()
            .flatten()
            .and_then(|s| s.current_run_id);
        let Some(run_id) = current else {
            return (true, None, None, None, vec![]);
        };
        let r = self.stop_run(run_id, &strategy.user_id, "manual").await;
        (r.ok, Some(run_id), r.error, Some(r.stop_pending), r.exits)
    }
}
