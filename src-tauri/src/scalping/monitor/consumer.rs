//! The monitor's owned consumer: every tick judged as it arrives, and the
//! saved stops synced whenever a sync is asked for.
//!
//! A sync runs beside the ticks, never in their way (PROD-REL-02): it ran
//! inside the consumer's loop, so while its read waited for a pooled
//! connection (up to the pool's 10 s) no leg was judged and the bounded
//! tick channel skipped prices. A sync whose read fails is tried again
//! after a capped backoff, and the trader is told when it keeps failing: a
//! stop saved meanwhile is not watched until the stops are read.

use super::RiskMonitor;
use crate::brokers::common::streaming::{FeedEvent, MarketEvent};
use crate::strategy::tick_feed::PriceSource;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Weak};
use std::time::Duration;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

/// The wait before a failed read of the stops is tried again; it doubles
/// with each failure in a row, up to [`SYNC_RETRY_MAX`].
const SYNC_RETRY_FIRST: Duration = Duration::from_millis(500);
const SYNC_RETRY_MAX: Duration = Duration::from_secs(30);

/// Failed reads of the stops in a row before the trader is told.
const SYNC_ALERT_AFTER: u32 = 3;

/// What the trader is told when the stops keep failing to be read.
pub const STOPS_UNREAD: &str = "Scalping stops could not be read from The Bridge's data folder, so a stop saved or changed recently may not be watched yet. Stops already watched stay in place, and The Bridge keeps trying. Check that the disk has free space; if this lasts, watch your open positions yourself and restart The Bridge.";

type SyncFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// The consumer's loop, until `token` is cancelled (`RiskMonitor::stop`).
/// `ticks` is subscribed before the task starts, so no tick sent after
/// `start` is missed.
pub(super) async fn consume(
    weak: Weak<RiskMonitor>,
    prices: Option<Arc<dyn PriceSource>>,
    mut ticks: Option<broadcast::Receiver<MarketEvent>>,
    token: CancellationToken,
) {
    let mut syncing: Option<SyncFuture> = None;
    loop {
        let Some(m) = weak.upgrade() else { break };
        tokio::select! {
            _ = token.cancelled() => break,
            // Asked for while a sync runs, the request waits (its permit
            // stays stored) and starts the next sync once this one ends.
            _ = m.sync_wanted.notified(), if syncing.is_none() => {
                syncing = Some(Box::pin(m.clone().sync_until_read()));
            }
            _ = in_flight(&mut syncing), if syncing.is_some() => syncing = None,
            ev = recv(&mut ticks) => match ev {
                Ok(ev) => {
                    let FeedEvent::Tick(t) = ev.as_ref() else { continue };
                    m.process_tick(&t.symbol, &t.exchange, t.ltp).await;
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!("Scalping tick consumer lagged by {}", n);
                    if let Some(p) = &prices {
                        p.note_skipped(n);
                    }
                }
                Err(broadcast::error::RecvError::Closed) => ticks = None,
            },
        }
    }
}

/// The sync in flight; pending when there is none.
async fn in_flight(syncing: &mut Option<SyncFuture>) {
    match syncing {
        Some(sync) => sync.await,
        None => std::future::pending().await,
    }
}

async fn recv(
    rx: &mut Option<broadcast::Receiver<MarketEvent>>,
) -> Result<MarketEvent, broadcast::error::RecvError> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

/// How long to wait after `failures` failed reads in a row.
fn retry_after(failures: u32) -> Duration {
    let doublings = 2u32.saturating_pow(failures.saturating_sub(1));
    SYNC_RETRY_FIRST
        .saturating_mul(doublings)
        .min(SYNC_RETRY_MAX)
}

impl RiskMonitor {
    /// Sync until the saved stops could be read. A read that fails (no
    /// free pooled connection, an unreadable table) leaves the legs held as
    /// they are and is tried again after a capped backoff, or at once when
    /// another sync is asked for (a stop was saved); after
    /// [`SYNC_ALERT_AFTER`] failures in a row the trader is told once.
    async fn sync_until_read(self: Arc<Self>) {
        let mut failures = 0u32;
        while !self.sync().await {
            failures = failures.saturating_add(1);
            if failures == SYNC_ALERT_AFTER {
                self.alert(STOPS_UNREAD.to_string());
            }
            tokio::select! {
                _ = tokio::time::sleep(retry_after(failures)) => {}
                _ = self.sync_wanted.notified() => {}
            }
        }
        if failures >= SYNC_ALERT_AFTER {
            tracing::info!(
                "Scalping stops were read again after {} failed reads; every saved stop is watched",
                failures
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_read_waits_twice_as_long_each_time_up_to_30_s() {
        let waits: Vec<u64> = (1..=8).map(|f| retry_after(f).as_millis() as u64).collect();
        assert_eq!(waits, [500, 1000, 2000, 4000, 8000, 16000, 30000, 30000]);
        assert_eq!(retry_after(u32::MAX), SYNC_RETRY_MAX);
    }
}
