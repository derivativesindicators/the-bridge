//! Management writes on a strategy: edit, delete, the start claim and its
//! release, the webhook token, the Live opt-in and the webhook lock. The
//! reads and the run, order and event bookkeeping are in the store's other
//! modules (`strategies`, `runs`, `orders`, `events`).
//!
//! Every write here checks the status it refuses on in its own UPDATE, or
//! reads it inside its own `BEGIN IMMEDIATE` transaction, never in a read
//! before the write. Every configuration write (edit, Live opt-in, webhook
//! lock, token) advances `sm_strategy.revision`, and a start's claim and an
//! edit compare against the revision they validated (SM-01, SM-02): a change
//! that lands while a start awaits its legs, or while an edit is validated,
//! refuses that start or edit instead of being overtaken by it.

use super::{
    generate_webhook_token, hash_webhook_token, json_col, num_param, strategy_from, time_col,
    Store, StrategyRow, STRATEGY_COLS, UPDATABLE_FIELDS,
};
use crate::error::{AppError, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{Map, Value};

/// A start refused because the strategy was edited (or its Live opt-in,
/// webhook lock or token changed) after the start read it.
pub const CHANGED_WHILE_STARTING: &str =
    "The strategy was changed while it was starting, so it did not start. Start it again.";
/// An edit refused because the strategy changed after it was read.
pub const CHANGED_WHILE_EDITING: &str =
    "The strategy was changed somewhere else while you were editing it. Reload it and try again.";
pub const ALREADY_RUNNING: &str = "This strategy is already running";
pub const LIVE_NOT_ENABLED: &str =
    "This strategy is not enabled for live trading. Enable it first.";
pub const WEBHOOK_LOCKED: &str = "This strategy's webhook is locked";

/// Migration `086_strategy_revision`: the `revision` every configuration
/// write advances. Idempotent; existing rows start at 0, which is all a
/// revision needs, since only later changes are compared.
pub fn migrate_revision(conn: &Connection) -> Result<()> {
    if !crate::db::sqlite::migrations::column_exists(conn, "sm_strategy", "revision")? {
        conn.execute_batch(
            "ALTER TABLE sm_strategy ADD COLUMN revision INTEGER NOT NULL DEFAULT 0",
        )?;
    }
    Ok(())
}

/// Migration `098_strategy_webhook_messages` (The Bridge 1.0.7): a signal
/// strategy's plain-text alert rules, JSON; NULL means the defaults.
/// Idempotent.
pub fn migrate_webhook_messages(conn: &Connection) -> Result<()> {
    if !crate::db::sqlite::migrations::column_exists(conn, "sm_strategy", "webhook_messages")? {
        conn.execute_batch("ALTER TABLE sm_strategy ADD COLUMN webhook_messages TEXT")?;
    }
    Ok(())
}

/// Migration `099_strategy_signal_options` (The Bridge 1.0.8): a signal
/// strategy's optional overnight carry / expiry roll / CAS exit settings,
/// and its durable pending re-entries (expiry roll, deferred entry).
/// Idempotent.
pub fn migrate_signal_options(conn: &Connection) -> Result<()> {
    for col in ["signal_options", "signal_pending"] {
        if !crate::db::sqlite::migrations::column_exists(conn, "sm_strategy", col)? {
            conn.execute_batch(&format!("ALTER TABLE sm_strategy ADD COLUMN {} TEXT", col))?;
        }
    }
    Ok(())
}

impl Store {
    /// The Bridge 1.0.8: ids of signal strategies with options or a pending
    /// re-entry (what the signal timers look at each second).
    pub fn signal_timer_strategy_ids(&self) -> Result<Vec<i64>> {
        let conn = self.conn()?;
        let mut st = conn.prepare(
            "SELECT id FROM sm_strategy WHERE strategy_kind = 'signal' \
             AND (signal_options IS NOT NULL OR signal_pending IS NOT NULL) ORDER BY id",
        )?;
        let ids = st
            .query_map([], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(ids)
    }

    /// A strategy's pending re-entry, `Null` when none.
    pub fn signal_pending(&self, strategy_id: i64) -> Result<Value> {
        let conn = self.conn()?;
        let text: Option<String> = conn
            .query_row(
                "SELECT signal_pending FROM sm_strategy WHERE id = ?1",
                [strategy_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        Ok(text
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null))
    }

    /// Replace (or with `Null`, clear) a strategy's pending re-entry. Not a
    /// configuration write: the revision is untouched.
    pub fn set_signal_pending(&self, strategy_id: i64, pending: &Value) -> Result<()> {
        let conn = self.conn()?;
        let text = if pending.is_null() {
            None
        } else {
            Some(pending.to_string())
        };
        conn.execute(
            "UPDATE sm_strategy SET signal_pending = ?1 WHERE id = ?2",
            rusqlite::params![text, strategy_id],
        )?;
        Ok(())
    }
}

/// What a start decided on, checked again by the statement that moves the
/// strategy to running (SM-01).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartClaim {
    /// The `revision` the start read before resolving its legs.
    pub revision: i64,
    /// A Live start: the strategy must still be opted in to Live.
    pub live: bool,
    /// A webhook start: the webhook must still be unlocked (kill switch).
    pub webhook: bool,
}

/// The answer to a [`StartClaim`]. Only `Claimed` moved the row; the other
/// variants say, from a read after the refusal, why it did not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOutcome {
    Claimed,
    /// The strategy no longer exists.
    Missing,
    /// Another start or a run holds it.
    Running,
    /// Live was switched off after the start read the strategy.
    LiveDisabled,
    /// The webhook was locked after the start read the strategy.
    WebhookLocked,
    /// Changed in some other way after the start read it.
    Changed,
}

impl ClaimOutcome {
    /// The trader-facing reason for a refusal; `None` for `Claimed`.
    pub fn refusal(self) -> Option<&'static str> {
        match self {
            ClaimOutcome::Claimed => None,
            ClaimOutcome::Missing => Some("Strategy not found"),
            ClaimOutcome::Running => Some(ALREADY_RUNNING),
            ClaimOutcome::LiveDisabled => Some(LIVE_NOT_ENABLED),
            ClaimOutcome::WebhookLocked => Some(WEBHOOK_LOCKED),
            ClaimOutcome::Changed => Some(CHANGED_WHILE_STARTING),
        }
    }
}

/// An edit's preconditions, read inside its transaction (SM-02): the
/// strategy is the caller's and stopped, the kind does not change, nothing
/// changed since `validated` was read, and a new name is free.
fn check_editable(
    tx: &Connection,
    validated: &StrategyRow,
    changes: &Map<String, Value>,
) -> Result<()> {
    let current: Option<(String, String, i64)> = tx
        .query_row(
            "SELECT status, strategy_kind, revision FROM sm_strategy \
             WHERE id = ?1 AND user_id = ?2",
            params![validated.id, validated.user_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((status, kind, revision)) = current else {
        return Err(AppError::NotFound("Strategy not found".into()));
    };
    if status == "running" {
        return Err(AppError::Validation(
            "Stop the strategy before editing it".into(),
        ));
    }
    if let Some(wanted) = changes.get("strategy_kind").and_then(Value::as_str) {
        if wanted != kind {
            return Err(AppError::Validation(
                "A strategy cannot change between batch and signal. The two kinds do not \
                 share a leg shape, so every leg would describe the wrong kind of contract. \
                 Create a new strategy instead."
                    .into(),
            ));
        }
    }
    if revision != validated.revision {
        return Err(AppError::Validation(CHANGED_WHILE_EDITING.into()));
    }
    if let Some(name) = changes.get("name").and_then(Value::as_str) {
        let taken: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sm_strategy WHERE user_id = ?1 AND name = ?2 \
             AND id != ?3)",
            params![validated.user_id, name, validated.id],
            |r| r.get(0),
        )?;
        if taken {
            return Err(AppError::Validation(format!(
                "A strategy named '{}' already exists",
                name
            )));
        }
    }
    Ok(())
}

/// One field of an edit in its stored form.
fn column_value(field: &str, value: &Value) -> rusqlite::types::Value {
    match field {
        "entry_time" | "exit_time" => time_col(value).into(),
        "legs" | "lock_profit" | "scheduler" | "webhook_ip_allowlist" | "webhook_messages" | "signal_options" => {
            json_col(value).into()
        }
        "overall_sl_mtm" | "overall_target_mtm" | "daily_loss_limit_inr" => num_param(value).into(),
        "trail_sl_to_entry" => i64::from(value.as_bool().unwrap_or(false)).into(),
        _ => value.as_str().map(str::to_string).into(),
    }
}

impl Store {
    /// Save already-validated `changes` to the strategy `validated` was read
    /// as, in one transaction (SM-02): the preconditions are checked under
    /// the write lock, then every field, `updated_at` and the revision commit
    /// together or not at all. The web commits an edit once too.
    pub fn update_strategy(
        &self,
        validated: &StrategyRow,
        changes: &Map<String, Value>,
    ) -> Result<StrategyRow> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        check_editable(&tx, validated, changes)?;
        for (field, value) in changes {
            if !UPDATABLE_FIELDS.contains(&field.as_str()) {
                continue;
            }
            tx.execute(
                &format!("UPDATE sm_strategy SET {} = ?1 WHERE id = ?2", field),
                params![column_value(field, value), validated.id],
            )?;
        }
        tx.execute(
            "UPDATE sm_strategy SET updated_at = ?1, revision = revision + 1 WHERE id = ?2",
            params![self.utcnow(), validated.id],
        )?;
        let row = tx.query_row(
            &format!("SELECT {} FROM sm_strategy WHERE id = ?1", STRATEGY_COLS),
            [validated.id],
            strategy_from,
        )?;
        tx.commit()?;
        Ok(row)
    }

    /// Delete a stopped strategy and every row that belongs to it. The status
    /// is read inside the delete's own transaction, so a start that claims
    /// the strategy cannot slip between the check and the delete.
    pub fn delete_strategy(&self, strategy_id: i64, user_id: &str) -> Result<()> {
        let mut conn = self.conn()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let status: Option<String> = tx
            .query_row(
                "SELECT status FROM sm_strategy WHERE id = ?1 AND user_id = ?2",
                params![strategy_id, user_id],
                |r| r.get(0),
            )
            .optional()?;
        match status.as_deref() {
            None => return Err(AppError::NotFound("Strategy not found".into())),
            Some("running") => {
                return Err(AppError::Validation(
                    "Stop the strategy before deleting it".into(),
                ))
            }
            Some(_) => {}
        }
        tx.execute(
            "DELETE FROM sm_strategy_checkpoint WHERE run_id IN \
             (SELECT id FROM sm_strategy_run WHERE strategy_id = ?1)",
            [strategy_id],
        )?;
        tx.execute(
            "DELETE FROM sm_strategy_order WHERE run_id IN \
             (SELECT id FROM sm_strategy_run WHERE strategy_id = ?1)",
            [strategy_id],
        )?;
        tx.execute(
            "DELETE FROM sm_strategy_event WHERE strategy_id = ?1",
            [strategy_id],
        )?;
        tx.execute(
            "DELETE FROM sm_webhook_event WHERE strategy_id = ?1",
            [strategy_id],
        )?;
        tx.execute(
            "DELETE FROM sm_strategy_run WHERE strategy_id = ?1",
            [strategy_id],
        )?;
        tx.execute("DELETE FROM sm_strategy WHERE id = ?1", [strategy_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Stopped to running, in one conditional UPDATE that also checks what
    /// the start decided on (SM-01): the revision it read, the Live opt-in
    /// for a Live start and the webhook lock for a webhook start. Only the
    /// caller that made the move gets `Claimed`.
    pub fn claim_strategy_for_run(
        &self,
        strategy_id: i64,
        claim: StartClaim,
    ) -> Result<ClaimOutcome> {
        let conn = self.conn()?;
        let moved = conn.execute(
            "UPDATE sm_strategy SET status = 'running' WHERE id = ?1 AND status = 'stopped' \
             AND revision = ?2 AND (?3 = 0 OR live_enabled = 1) AND (?4 = 0 OR webhook_locked = 0)",
            params![strategy_id, claim.revision, claim.live, claim.webhook],
        )?;
        if moved == 1 {
            return Ok(ClaimOutcome::Claimed);
        }
        // The UPDATE above decided; this read only names the reason.
        let row: Option<(String, bool, bool)> = conn
            .query_row(
                "SELECT status, live_enabled, webhook_locked FROM sm_strategy WHERE id = ?1",
                [strategy_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(match row {
            None => ClaimOutcome::Missing,
            Some((status, _, _)) if status != "stopped" => ClaimOutcome::Running,
            Some((_, live_enabled, _)) if claim.live && !live_enabled => ClaimOutcome::LiveDisabled,
            Some((_, _, locked)) if claim.webhook && locked => ClaimOutcome::WebhookLocked,
            Some(_) => ClaimOutcome::Changed,
        })
    }

    pub fn release_strategy(&self, strategy_id: i64) -> Result<bool> {
        let conn = self.conn()?;
        Ok(conn.execute(
            "UPDATE sm_strategy SET status = 'stopped', current_run_id = NULL WHERE id = ?1",
            [strategy_id],
        )? == 1)
    }

    /// Issue a new webhook token (returned once). The owner check is the
    /// UPDATE's own condition, and the revision moves so a start that read
    /// the strategy before the rotation does not complete.
    pub fn rotate_webhook_token(&self, strategy_id: i64, user_id: &str) -> Result<String> {
        let token = generate_webhook_token();
        let conn = self.conn()?;
        let moved = conn.execute(
            "UPDATE sm_strategy SET webhook_token_hash = ?1, updated_at = ?2, \
             revision = revision + 1 WHERE id = ?3 AND user_id = ?4",
            params![
                hash_webhook_token(&token),
                self.utcnow(),
                strategy_id,
                user_id
            ],
        )?;
        if moved != 1 {
            return Err(AppError::NotFound("Strategy not found".into()));
        }
        Ok(token)
    }

    /// Switch the Live opt-in of a strategy that is not running, in one
    /// conditional UPDATE (SM-01): a start that claims first makes this
    /// refuse, and this landing first makes a Live start's claim refuse.
    pub fn set_live_enabled(&self, strategy_id: i64, user_id: &str, enabled: bool) -> Result<()> {
        let conn = self.conn()?;
        let moved = conn.execute(
            "UPDATE sm_strategy SET live_enabled = ?1, revision = revision + 1 \
             WHERE id = ?2 AND user_id = ?3 AND status != 'running'",
            params![enabled, strategy_id, user_id],
        )?;
        if moved == 1 {
            return Ok(());
        }
        let exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sm_strategy WHERE id = ?1 AND user_id = ?2)",
            params![strategy_id, user_id],
            |r| r.get(0),
        )?;
        if exists {
            Err(AppError::Validation(
                "Stop the strategy before changing its mode".into(),
            ))
        } else {
            Err(AppError::NotFound("Strategy not found".into()))
        }
    }

    /// Lock or unlock the webhook (the kill switch's first step). Allowed
    /// while running; the revision moves so a start already under way when
    /// the lock lands does not complete.
    pub fn set_webhook_locked(&self, strategy_id: i64, user_id: &str, locked: bool) -> Result<()> {
        let conn = self.conn()?;
        let moved = conn.execute(
            "UPDATE sm_strategy SET webhook_locked = ?1, revision = revision + 1 \
             WHERE id = ?2 AND user_id = ?3",
            params![locked, strategy_id, user_id],
        )?;
        if moved != 1 {
            return Err(AppError::NotFound("Strategy not found".into()));
        }
        Ok(())
    }
}
