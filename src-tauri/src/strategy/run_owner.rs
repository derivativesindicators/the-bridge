//! A Live run's orders go only through the broker session it was opened on
//! (LOG-07, defence in depth; `services::live_owner`).
//!
//! The run row records its broker and account at start. Every order and
//! cancel of a run goes through [`StrategyModule::place_for_run`] or
//! [`StrategyModule::cancel_for_run`], which refuse without sending when the
//! session connected now is another broker or account. A refused exit
//! releases its claim and the run stays open and managed, as for any
//! refused exit; risk and Stop check [`StrategyModule::authorised_for`]
//! first and say so once, as they do for a missing session.

use super::dispatch::{DispatchResult, OrderPayload, RunMode};
use super::StrategyModule;
use crate::services::live_owner::LiveOwner;

/// What an order of a run answers when the run's owner cannot be read.
const OWNER_UNREADABLE: &str = "The Bridge could not read which broker account this run belongs to, so the order was not sent. Try again.";

impl StrategyModule {
    /// Why an order of this run must not go through the session connected
    /// now, or `None` when it may (a Sandbox run, or the broker and account
    /// the run was opened on).
    pub(crate) fn owner_refusal(&self, run_id: i64, mode: RunMode) -> Option<String> {
        if mode != RunMode::Live {
            return None;
        }
        let run = match self.store.get_run(run_id) {
            Ok(Some(run)) => run,
            Ok(None) => return None,
            Err(e) => {
                tracing::error!("Could not read the broker of run {}: {}", run_id, e);
                return Some(OWNER_UNREADABLE.into());
            }
        };
        LiveOwner::new(run.broker, run.broker_account)
            .refusal(LiveOwner::active(self.gateway.as_ref()).as_ref())
    }

    /// Whether this run can send orders now: a session for its side, and
    /// for Live the one it was opened on.
    pub(crate) fn authorised_for(&self, run_id: i64, mode: RunMode) -> Result<(), String> {
        self.gateway.authorised(mode)?;
        self.owner_refusal(run_id, mode).map_or(Ok(()), Err)
    }

    /// Place one order of a run, unless it belongs to another broker
    /// session: then nothing is sent (a definite refusal, LOG-08).
    pub(crate) async fn place_for_run(
        &self,
        run_id: i64,
        mode: RunMode,
        order: &OrderPayload,
    ) -> DispatchResult {
        if let Some(why) = self.owner_refusal(run_id, mode) {
            tracing::warn!(
                "Run {}: {} {} {} was not sent: {}",
                run_id,
                order.action,
                order.quantity,
                order.symbol,
                why
            );
            return DispatchResult::refused(why);
        }
        self.gateway.place(mode, order).await
    }

    /// Cancel one order of a run, under the same rule.
    pub(crate) async fn cancel_for_run(
        &self,
        run_id: i64,
        mode: RunMode,
        broker_order_id: &str,
    ) -> DispatchResult {
        if let Some(why) = self.owner_refusal(run_id, mode) {
            return DispatchResult::refused(why);
        }
        self.gateway.cancel(mode, broker_order_id).await
    }
}
