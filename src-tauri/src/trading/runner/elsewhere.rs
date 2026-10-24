//! What a deployment holds, or may still have working, somewhere its run
//! does not trade (NEW-TR-01): at another broker or account, or recorded
//! before this version kept the account an order went through.
//!
//! None of it is counted, cancelled or closed by the run (`identity`), so a
//! trader must hear of it wherever they look: the start log, the Stop
//! answer (as `held_elsewhere`, so a caller cannot take a Stop with exposure
//! left elsewhere for a clean close) and the run's status. A working order
//! is told as well as a position: unfilled, it can still fill.

use super::{side_name, store, RunInfo, Runner};
use crate::services::live_owner::Destination;
use std::collections::BTreeMap;

/// One line for a trader about a position held somewhere else.
fn holding_line(at: &Destination, h: &store::Holding) -> String {
    let what = format!(
        "{} {} {} {}",
        side_name(h.quantity),
        h.quantity.unsigned_abs(),
        h.symbol,
        h.product
    );
    if at.is_known() {
        format!(
            "This strategy also holds {} through {}, from an earlier run there. It is not managed or closed from here: sign in to {} and Stop it there, or close it at that broker.",
            what,
            at.describe(),
            at.describe()
        )
    } else {
        format!(
            "This strategy also holds {} at your broker, placed before this version recorded which account it went through, so The Bridge does not manage or close it. Close it at your broker.",
            what
        )
    }
}

/// One line for a trader about an order that may still be working
/// somewhere else.
fn working_line(at: &Destination, o: &store::OrderRow) -> String {
    let what = format!(
        "{} {} {} {}{}",
        o.action,
        o.quantity,
        o.symbol,
        o.product,
        o.orderid
            .as_deref()
            .map(|id| format!(" (order {})", id))
            .unwrap_or_default()
    );
    if o.status == store::UNCONFIRMED && !at.is_known() {
        // Never settled as absent from a book of a session it may not
        // belong to (`reconcile`): only the trader can.
        format!(
            "An order of this strategy may have been placed at your broker before this version recorded which account it went through: {}. Its outcome is not known. Check your broker's order book, then mark it on the Health page.",
            what
        )
    } else if at.is_known() {
        format!(
            "An order of this strategy may still be working through {}: {}. It is not followed or cancelled from here: cancel it there if it should not fill.",
            at.describe(),
            what
        )
    } else {
        format!(
            "An order of this strategy may still be working at your broker, placed before this version recorded which account it went through: {}. The Bridge does not follow or cancel it: cancel it at your broker if it should not fill.",
            what
        )
    }
}

impl Runner {
    /// What the deployment holds, or may still have working, on
    /// `destination`'s side somewhere other than `destination`: one line
    /// each for a trader. Empty when its rows cannot be read: whoever needs
    /// them says so.
    pub(super) fn held_elsewhere(&self, run_id: &str, destination: &Destination) -> Vec<String> {
        let rows = match self
            .db
            .conn()
            .and_then(|c| store::orders_of(&c, run_id, destination.mode.as_str()))
        {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!("Could not read the orders of {}: {}", run_id, e);
                return Vec::new();
            }
        };
        let mut by_place: BTreeMap<(String, String), (Destination, Vec<store::OrderRow>)> =
            BTreeMap::new();
        for o in rows {
            let Some(at) = o.destination().filter(|d| !destination.owns(d)) else {
                continue;
            };
            let key = (
                at.owner.broker.to_lowercase(),
                at.owner.account.to_lowercase(),
            );
            by_place
                .entry(key)
                .or_insert_with(|| (at, Vec::new()))
                .1
                .push(o);
        }
        let mut lines = Vec::new();
        for (at, rows) in by_place.into_values() {
            for h in store::holdings(&rows).iter().filter(|h| h.quantity != 0) {
                lines.push(holding_line(&at, h));
            }
            for o in rows
                .iter()
                .filter(|o| !store::TERMINAL.contains(&o.status.as_str()))
            {
                lines.push(working_line(&at, o));
            }
        }
        lines
    }

    /// [`Self::held_elsewhere`] for a deployment by name, as its status
    /// reports it. When where it trades cannot be told (no broker login, an
    /// account OpenAlgo does not know, or a run on another session than the
    /// one connected), that is said first: it is when exposure is hardest
    /// to see, never a time to show nothing.
    pub fn held_elsewhere_of(&self, given: &str) -> Vec<String> {
        if let Some(info) = self.status_of(given) {
            let mut lines = self.held_elsewhere(&info.run_id, &info.destination);
            if self.session_refusal(&info).is_some() {
                lines.insert(
                    0,
                    format!(
                        "This strategy trades through {}, but The Bridge is signed in elsewhere now, so nothing of it is read or sent until you sign in to {} again.",
                        info.destination.describe(),
                        info.destination.describe()
                    ),
                );
            }
            return lines;
        }
        match self.book_destination(given) {
            Ok(at) => self.held_elsewhere(&self.as_run_id(given), &at),
            Err(why) => vec![why],
        }
    }

    /// What a run starting now is told about the deployment's earlier
    /// positions: those at its own destination it does not trade but a Stop
    /// closes (ARCH-04 d), and those elsewhere it never touches.
    pub(super) fn start_lines(&self, info: &RunInfo) -> Vec<String> {
        let mut lines = vec![format!(
            "This run trades through {}. Its engine begins holding nothing.",
            info.destination.describe()
        )];
        if let Ok(own) = self.read_own(info) {
            for h in store::holdings(&own).iter().filter(|h| h.quantity != 0) {
                lines.push(format!(
                    "Earlier runs of this strategy left {} {} {} {} here. This run does not trade it; Stop closes it.",
                    side_name(h.quantity),
                    h.quantity.unsigned_abs(),
                    h.symbol,
                    h.product
                ));
            }
        }
        lines.extend(self.held_elsewhere(&info.run_id, &info.destination));
        lines
    }
}
