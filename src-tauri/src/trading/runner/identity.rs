//! A run's own orders, and nobody else's (NEW-TR-01, NEW-TR-03).
//!
//! **Destination.** A deployment keeps its id from run to run, and a Pause
//! lets the trader switch brokers. Its rows are therefore bound to where
//! they went (`store::identity`): a run's ledger is the rows its destination
//! owns, and every order, cancel, status read and book read of the run goes
//! through the session connected now only when that is the run's own
//! ([`Runner::session_refusal`]). A run started on broker B never counts,
//! cancels or closes what broker A holds; that stays with A, closeable by a
//! run on A, and the trader is told about it.
//!
//! **Incarnation.** Each start is a new engine whose intents are numbered
//! from 1 again. A frame reaches a page only for a row of that page's own
//! incarnation ([`Runner::frame_to_page`]), so an earlier run's order, filled
//! or cancelled late, never lands on a fresh engine's intent of the same
//! number. Its fill still counts in the deployment's ledger at its
//! destination, which a Stop closes.

use super::inbox::Message;
use super::{store, RunEntry, RunInfo, Runner};
use crate::services::live_owner::Destination;
use crate::strategy::dispatch::{DispatchResult, RunMode};
use serde_json::Value;

/// What a Live book answers without a broker session whose account is
/// known: unavailable, never empty.
const BOOK_UNAVAILABLE: &str = "This strategy's orders and positions at your broker cannot be read without a broker login whose account The Bridge knows. Log in to your broker, then look again.";

/// The rows `destination` owns.
pub(super) fn owned_by(
    destination: &Destination,
    rows: Vec<store::OrderRow>,
) -> Vec<store::OrderRow> {
    rows.into_iter()
        .filter(|o| o.destination().is_some_and(|d| destination.owns(&d)))
        .collect()
}

impl Runner {
    /// Why this run must not send, cancel or read through the session
    /// connected now (another broker or account than its own), or `None`.
    pub(super) fn session_refusal(&self, info: &RunInfo) -> Option<String> {
        info.destination
            .refusal(self.services().live_owner().as_ref())
    }

    /// Ask for one of this run's orders to be cancelled, only through its
    /// own session.
    pub(super) async fn cancel_own(&self, info: &RunInfo, orderid: &str) -> DispatchResult {
        if let Some(why) = self.session_refusal(info) {
            return DispatchResult::refused(why);
        }
        self.services().cancel(info.mode, orderid).await
    }

    /// Tell a run's page what became of one of its rows: only a row its own
    /// engine placed. A row of an earlier run of the deployment, or one from
    /// before runs were recorded, is that run's, whose page is gone.
    pub(super) fn frame_to_page(&self, entry: &RunEntry, row: &store::OrderRow, frame: Value) {
        let (Some(intent), Some(run)) = (row.intent_id, row.incarnation) else {
            return;
        };
        if run != entry.info.incarnation {
            tracing::debug!(
                "{}: order row {} belongs to run {}, not to the running one, so its page is not told",
                entry.info.run_id,
                row.id,
                run
            );
            return;
        }
        entry.inbox.push(Message::Frame {
            intent_id: intent,
            frame,
        });
    }

    /// Where a run of `given` would trade now: its own destination while it
    /// runs, else the session connected now on the side it last started on.
    /// Without a session whose account is known, the sentence saying its
    /// book cannot be read.
    /// While it runs on another session than the one connected now, the
    /// sentence saying so: its book is not read through that one.
    pub(super) fn book_destination(&self, given: &str) -> Result<Destination, String> {
        if let Some(info) = self.status_of(given) {
            return match self.session_refusal(&info) {
                Some(why) => Err(why),
                None => Ok(info.destination),
            };
        }
        match self.mode_for(given) {
            RunMode::Sandbox => Ok(Destination::sandbox()),
            // No session, or one whose account is not known: the book cannot
            // be read, which is never shown as an empty one.
            RunMode::Live => self
                .services()
                .live_owner()
                .map(Destination::live)
                .filter(Destination::is_known)
                .ok_or_else(|| BOOK_UNAVAILABLE.to_string()),
        }
    }

    /// The run id a deployment's books are tagged with, and their
    /// destination ([`Runner::book_destination`]), worked out on the
    /// blocking pool: a deployment that is not running is read from its
    /// saved settings, and the Strategies panel polls its books every 15 s
    /// (PROD-REL-02).
    pub async fn book_target(&self, given: &str) -> Result<(String, Destination), String> {
        let name = given.to_string();
        let read = self
            .blocking(move |r| r.book_destination(&name).map(|at| (r.as_run_id(&name), at)))
            .await;
        read.unwrap_or_else(|e| {
            tracing::error!("Could not tell where the books of {} are: {}", given, e);
            Err(super::ORDERS_UNREADABLE.to_string())
        })
    }
}
