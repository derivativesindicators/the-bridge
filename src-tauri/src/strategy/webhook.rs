//! The public `/strategy/webhook/<token>` pipeline (web
//! `services/strategy_module/webhook.py` and `webhook_bridge.py`).
//!
//! The URL token is the whole credential (TradingView cannot set a header).
//! It is stored only as a SHA-256 digest, never logged, never written to the
//! audit row (the payload is redacted first), and an unknown token answers
//! exactly like a malformed one.
//!
//! Stages, in order, each audited in `sm_webhook_event`:
//!
//! | Stage | Result | Status |
//! |---|---|---|
//! | token resolves | `rejected_token` | 404 |
//! | kill switch off | `rejected_locked` | 403 |
//! | IP in allowlist | `rejected_ip` | 403 |
//! | payload parses and fits | `rejected_payload` | 400 |
//! | action valid for this kind | `rejected_invalid_action` | 400 |
//! | start names a mode | `rejected_invalid_action` | 400 |
//! | live opt-in | `rejected_live_disabled` | 403 |
//! | not a duplicate (batch) | `rejected_dedupe` | 200, ok |
//! | not cooling off (batch) | `rejected_cooling_off` | 409 |
//! | engine acted | `rejected_engine_error` | 500 |
//!
//! Signal actions skip the dedupe and cooling-off windows: signal mode is
//! idempotent by meaning, and a 60 second window would suppress a genuine
//! long, short, long sequence.

use super::store::{hash_webhook_token, StrategyRow, WebhookEvent, WEBHOOK_TOKEN_PREFIX};
use super::StrategyModule;
use parking_lot::Mutex;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::time::{Duration, Instant};

pub mod messages;
mod pipeline;

pub const MAX_PAYLOAD_BYTES: usize = 16384;
pub const DEDUPE_WINDOW: Duration = Duration::from_secs(60);
pub const COOLING_OFF: Duration = Duration::from_secs(30);
pub const MAX_TRACKED_KEYS: usize = 4096;
/// `WEBHOOK_RATE_LIMIT`: 100 per minute, by caller address and by token.
pub const RATE_LIMIT: usize = 100;
pub const RATE_WINDOW: Duration = Duration::from_secs(60);

const UNKNOWN_TOKEN_MESSAGE: &str = "Unknown or expired webhook token";
const REDACTED: &str = "[redacted]";
const SECRET_KEY_HINTS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passwd",
    "apikey",
    "api_key",
    "auth",
    "signature",
];
const MAX_AUDIT_ITEMS: usize = 50;
const MAX_AUDIT_STRING: usize = 500;
const MAX_AUDIT_DEPTH: usize = 4;

pub fn result_status(result: &str) -> u16 {
    match result {
        "ok" | "rejected_dedupe" => 200,
        "rejected_token" => 404,
        "rejected_locked" | "rejected_ip" | "rejected_live_disabled" => 403,
        "rejected_cooling_off" => 409,
        "rejected_engine_error" => 500,
        "rate_limited" => 429,
        _ => 400,
    }
}

/// The decision, ready for the route to turn into a response.
#[derive(Debug, Clone, PartialEq)]
pub struct WebhookOutcome {
    pub ok: bool,
    pub result: String,
    pub status: u16,
    pub message: String,
    pub strategy_id: Option<i64>,
    pub run_id: Option<i64>,
    pub webhook_event_id: Option<i64>,
    pub stop_pending: Option<bool>,
    pub exits: Vec<Value>,
}

impl WebhookOutcome {
    fn new(result: &str, message: impl Into<String>, ok: bool) -> Self {
        Self {
            ok,
            result: result.into(),
            status: result_status(result),
            message: message.into(),
            strategy_id: None,
            run_id: None,
            webhook_event_id: None,
            stop_pending: None,
            exits: vec![],
        }
    }

    /// The JSON body. Carries nothing a caller has not already proved.
    pub fn body(&self) -> Value {
        let mut m = Map::new();
        m.insert(
            "status".into(),
            json!(if self.ok { "success" } else { "error" }),
        );
        m.insert("result".into(), json!(self.result));
        m.insert("message".into(), json!(self.message));
        if let Some(s) = self.strategy_id {
            m.insert("strategy_id".into(), json!(s));
        }
        if let Some(r) = self.run_id {
            m.insert("run_id".into(), json!(r));
        }
        if let Some(p) = self.stop_pending {
            m.insert("stop_pending".into(), json!(p));
            m.insert("exits".into(), json!(self.exits));
        }
        Value::Object(m)
    }
}

/// (strategy id, action, mode): one dedupe window entry.
type DedupeKey = (i64, String, Option<String>);

/// Bounded in-memory windows: dedupe, cooling off and the two rate limits.
pub struct WebhookState {
    dedupe: Mutex<HashMap<DedupeKey, Instant>>,
    cooling: Mutex<HashMap<i64, Instant>>,
    rate: Mutex<HashMap<String, VecDeque<Instant>>>,
    /// Failed authentications per webhook (strategy id), bounded.
    failures: Mutex<HashMap<i64, VecDeque<Instant>>>,
    throttle_logged: Mutex<Option<Instant>>,
    /// Added to `Instant::now()`; tests drive the windows without sleeping.
    offset: Mutex<Duration>,
}

impl Default for WebhookState {
    fn default() -> Self {
        Self::new()
    }
}

impl WebhookState {
    pub fn new() -> Self {
        Self {
            dedupe: Mutex::new(HashMap::new()),
            cooling: Mutex::new(HashMap::new()),
            rate: Mutex::new(HashMap::new()),
            failures: Mutex::new(HashMap::new()),
            throttle_logged: Mutex::new(None),
            offset: Mutex::new(Duration::ZERO),
        }
    }

    pub fn now(&self) -> Instant {
        Instant::now() + *self.offset.lock()
    }

    /// Move the windows' clock forward (tests).
    pub fn advance(&self, by: Duration) {
        *self.offset.lock() += by;
    }

    /// Drop every window.
    pub fn reset(&self) {
        self.dedupe.lock().clear();
        self.cooling.lock().clear();
        self.rate.lock().clear();
        self.failures.lock().clear();
    }

    /// Arm the cooling-off window for a strategy whose run just ended (every
    /// stop, not just webhook ones).
    pub fn note_run_stopped(&self, strategy_id: i64, _at: Instant) {
        let now = self.now();
        let mut c = self.cooling.lock();
        c.retain(|_, at| now.saturating_duration_since(*at) < COOLING_OFF);
        if c.len() >= MAX_TRACKED_KEYS {
            c.clear();
        }
        c.insert(strategy_id, now);
    }

    fn cooling_remaining(&self, strategy_id: i64, now: Instant) -> u64 {
        match self.cooling.lock().get(&strategy_id) {
            Some(at) => {
                let elapsed = now.saturating_duration_since(*at);
                if elapsed >= COOLING_OFF {
                    0
                } else {
                    (COOLING_OFF - elapsed).as_secs().max(1)
                }
            }
            None => 0,
        }
    }

    /// Count one hit against `key`; false when over the limit.
    pub fn rate_check(&self, key: &str) -> bool {
        let now = self.now();
        let mut map = self.rate.lock();
        if map.len() >= MAX_TRACKED_KEYS {
            map.retain(|_, q| {
                q.back()
                    .is_some_and(|t| now.saturating_duration_since(*t) < RATE_WINDOW)
            });
            if map.len() >= MAX_TRACKED_KEYS {
                map.clear();
            }
        }
        let q = map.entry(key.to_string()).or_default();
        while q
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= RATE_WINDOW)
        {
            q.pop_front();
        }
        if q.len() >= RATE_LIMIT {
            return false;
        }
        q.push_back(now);
        true
    }

    /// Keys held across every window (hygiene tests).
    pub fn tracked(&self) -> usize {
        self.dedupe.lock().len()
            + self.cooling.lock().len()
            + self.rate.lock().len()
            + self.failures.lock().len()
    }

    /// Count one failed authentication against a webhook. True when this
    /// failure reaches the lockout threshold inside the window (the caller
    /// then locks the webhook until the trader unlocks it).
    pub fn record_webhook_failure(&self, strategy_id: i64) -> bool {
        let now = self.now();
        let mut map = self.failures.lock();
        map.retain(|_, q| {
            q.back()
                .is_some_and(|t| now.saturating_duration_since(*t) < LOCKOUT_WINDOW)
        });
        if map.len() >= MAX_TRACKED_KEYS && !map.contains_key(&strategy_id) {
            // Bounded: drop the stalest webhook's history.
            if let Some(oldest) = map
                .iter()
                .min_by_key(|(_, q)| q.back().copied())
                .map(|(k, _)| *k)
            {
                map.remove(&oldest);
            }
        }
        let q = map.entry(strategy_id).or_default();
        while q
            .front()
            .is_some_and(|t| now.saturating_duration_since(*t) >= LOCKOUT_WINDOW)
        {
            q.pop_front();
        }
        q.push_back(now);
        while q.len() > LOCKOUT_FAILURES {
            q.pop_front();
        }
        if q.len() >= LOCKOUT_FAILURES {
            map.remove(&strategy_id);
            return true;
        }
        false
    }

    /// Forget a webhook's failure history (the trader unlocked it).
    pub fn clear_webhook_failures(&self, strategy_id: i64) {
        self.failures.lock().remove(&strategy_id);
    }

    /// Log throttling once per window, not once per refused request.
    pub(crate) fn note_throttled(&self, why: &str) {
        let now = self.now();
        let mut last = self.throttle_logged.lock();
        if last.is_none_or(|t| now.saturating_duration_since(t) >= RATE_WINDOW) {
            *last = Some(now);
            tracing::warn!(
                "Strategy webhook requests are being refused: {} (logged once per minute)",
                why
            );
        }
    }
}

/// Failed authentications on one webhook that lock it.
pub const LOCKOUT_FAILURES: usize = 10;
/// The window those failures are counted in.
pub const LOCKOUT_WINDOW: Duration = Duration::from_secs(600);

/// The web's per-address limit (`WEBHOOK_RATE_LIMIT`, 100 a minute) for
/// this computer and devices on the network, before any lookup. Tunnel
/// callers share one identity, so they are limited per token once the
/// token checked out ([`admit_token`]). `None`: over the caller's own
/// window. `Some(true)`: the caller is in the full shared overflow window,
/// to be refused only if its token is unknown.
pub fn admit_address(
    state: &WebhookState,
    limiter: &crate::server::ratelimit::RateLimiter,
    ip: IpAddr,
) -> Option<bool> {
    use crate::server::ratelimit::{Admission, Bucket};
    match limiter.admit(Bucket::StrategyWebhook, ip, limiter.now()) {
        Admission::Allowed => Some(false),
        Admission::OverflowOver(_) => Some(true),
        Admission::Over(_) => {
            state.note_throttled("one address is over the webhook rate limit");
            None
        }
    }
}

/// The limits of a token that checked out: behind a tunnel its own
/// per-address window (`tunnel_key`, `middleware::limiter_key`), then the
/// per-token window keyed on the digest (never the raw token). A valid
/// token is never limited by other callers' traffic or failures.
pub fn admit_token(
    state: &WebhookState,
    limiter: &crate::server::ratelimit::RateLimiter,
    tunnel_key: Option<IpAddr>,
    token: &str,
) -> bool {
    use crate::server::ratelimit::Bucket;
    if let Some(key) = tunnel_key {
        if limiter
            .check(Bucket::StrategyWebhook, key, limiter.now())
            .is_err()
        {
            state.note_throttled("one webhook is over the webhook rate limit");
            return false;
        }
    }
    if !state.rate_check(&format!("token:{}", hash_webhook_token(token))) {
        state.note_throttled("one webhook is over its rate limit");
        return false;
    }
    true
}

/// Constant-time comparison of a token's digest with a stored digest.
fn digest_matches(token: &str, stored: &str) -> bool {
    use subtle::ConstantTimeEq;
    let got = hash_webhook_token(token);
    got.len() == stored.len() && bool::from(got.as_bytes().ct_eq(stored.as_bytes()))
}

fn looks_like_token(token: &str) -> bool {
    let Some(body) = token.strip_prefix(WEBHOOK_TOKEN_PREFIX) else {
        return false;
    };
    (16..=128).contains(&body.len())
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Whether `ip` falls inside the allowlist. Empty allows everything; a
/// non-empty list is closed, a malformed entry is skipped.
pub fn ip_allowed(ip: Option<&str>, allowlist: &Value) -> bool {
    let entries: Vec<&str> = allowlist
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    if entries.is_empty() {
        return true;
    }
    let Some(addr) = ip.and_then(|i| i.trim().parse::<IpAddr>().ok()) else {
        return false;
    };
    let mut candidates = vec![addr];
    if let IpAddr::V6(v6) = addr {
        if let Some(v4) = v6.to_ipv4_mapped() {
            candidates.push(IpAddr::V4(v4));
        }
    }
    entries.iter().any(|entry| match parse_network(entry) {
        Some((net, prefix)) => candidates.iter().any(|c| in_network(*c, net, prefix)),
        None => {
            tracing::warn!("Skipping a malformed webhook allowlist entry");
            false
        }
    })
}

/// `addr` or `addr/prefix`, read like Python's `ip_network(strict=False)`.
pub fn parse_network(entry: &str) -> Option<(IpAddr, u8)> {
    let (a, p) = match entry.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (entry, None),
    };
    let addr: IpAddr = a.trim().parse().ok()?;
    let max = if addr.is_ipv4() { 32 } else { 128 };
    let prefix = match p {
        Some(p) => p.trim().parse::<u8>().ok().filter(|p| *p <= max)?,
        None => max,
    };
    Some((addr, prefix))
}

fn in_network(c: IpAddr, net: IpAddr, prefix: u8) -> bool {
    match (c, net) {
        (IpAddr::V4(c), IpAddr::V4(n)) => {
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - u32::from(prefix))
            };
            (u32::from(c) & mask) == (u32::from(n) & mask)
        }
        (IpAddr::V6(c), IpAddr::V6(n)) => {
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - u32::from(prefix))
            };
            (u128::from(c) & mask) == (u128::from(n) & mask)
        }
        _ => false,
    }
}

/// Read the body into an object, or say why it cannot be one.
pub fn parse_payload(body: &[u8]) -> Result<Map<String, Value>, String> {
    if body.len() > MAX_PAYLOAD_BYTES {
        return Err(format!(
            "The request body is larger than {} bytes",
            MAX_PAYLOAD_BYTES
        ));
    }
    let text =
        std::str::from_utf8(body).map_err(|_| "The request body is not valid UTF-8".to_string())?;
    let text = text.trim();
    if text.is_empty() {
        return Err("The request body is empty".into());
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err("The request body must be a JSON object".into()),
        Err(_) => Err("The request body is not valid JSON".into()),
    }
}

/// A copy of the payload safe to store: credential-named keys lose their
/// value, any string carrying the token (or the token prefix) is replaced,
/// and depth, item count and length are capped.
pub fn redact(value: &Value, token: &str, depth: usize) -> Value {
    if depth > MAX_AUDIT_DEPTH {
        return json!("[truncated]");
    }
    match value {
        Value::Object(m) => {
            let mut out = Map::new();
            for (k, v) in m.iter().take(MAX_AUDIT_ITEMS) {
                let name: String = k.chars().take(MAX_AUDIT_STRING).collect();
                let lower = name.to_ascii_lowercase();
                if SECRET_KEY_HINTS.iter().any(|h| lower.contains(h)) {
                    out.insert(name, json!(REDACTED));
                } else {
                    out.insert(name, redact(v, token, depth + 1));
                }
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(
            a.iter()
                .take(MAX_AUDIT_ITEMS)
                .map(|v| redact(v, token, depth + 1))
                .collect(),
        ),
        Value::String(s) => {
            if (!token.is_empty() && s.contains(token)) || s.contains(WEBHOOK_TOKEN_PREFIX) {
                json!(REDACTED)
            } else {
                json!(s.chars().take(MAX_AUDIT_STRING).collect::<String>())
            }
        }
        other => other.clone(),
    }
}

struct Audit<'a> {
    module: &'a StrategyModule,
    ip: Option<&'a str>,
    ua: Option<&'a str>,
}

impl Audit<'_> {
    fn write(
        &self,
        result: &str,
        strategy_id: Option<i64>,
        action: Option<&str>,
        mode: Option<&str>,
        payload: Option<&Value>,
        error: Option<&str>,
    ) -> Option<i64> {
        match self.module.store.record_webhook_event(&WebhookEvent {
            result,
            strategy_id,
            action,
            mode,
            payload,
            ip: self.ip,
            user_agent: self.ua,
            error,
        }) {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::error!("Could not audit a webhook ({}): {}", result, e);
                None
            }
        }
    }
}

impl StrategyModule {
    /// Whether `token` is a live webhook token: its shape, then the digest
    /// lookup and a constant-time confirmation. Cheap, and writes nothing.
    pub fn token_known(&self, token: &str) -> bool {
        looks_like_token(token)
            && matches!(
                self.store.get_strategy_by_webhook_token(token),
                Ok(Some(s)) if digest_matches(token, &s.webhook_token_hash)
            )
    }

    /// The canonical answer to a token that resolves to nothing.
    pub fn unknown_token_outcome(&self, ip: Option<&str>, ua: Option<&str>) -> WebhookOutcome {
        let id = Audit {
            module: self,
            ip,
            ua,
        }
        .write("rejected_token", None, None, None, None, None);
        let mut o = WebhookOutcome::new("rejected_token", UNKNOWN_TOKEN_MESSAGE, false);
        o.webhook_event_id = id;
        o
    }

    /// Lock a webhook that crossed the failed-authentication threshold. It
    /// stays locked until the trader unlocks it.
    async fn lock_webhook_after_failures(&self, strategy: &StrategyRow) {
        if let Err(e) = self
            .store
            .set_webhook_locked(strategy.id, &strategy.user_id, true)
        {
            tracing::error!("Could not lock a webhook after failed attempts: {}", e);
            return;
        }
        tracing::warn!(
            "Strategy {} webhook locked after {} refused requests",
            strategy.id,
            LOCKOUT_FAILURES
        );
        let message = format!(
            "Webhook locked after {} refused alerts from addresses outside the allowlist. \
             Check who has the webhook URL, then unlock it or rotate it.",
            LOCKOUT_FAILURES
        );
        self.emit(
            strategy.id,
            &strategy.user_id,
            "webhook_locked",
            &message,
            super::store::EventFields {
                severity: Some("critical"),
                ..Default::default()
            },
        )
        .await;
    }
}
