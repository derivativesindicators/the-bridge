//! The state of this app's update: the check, the offer, the verified
//! download waiting for the restart, and the decisions to go ahead.

use super::activity::{activity, broker_name, Activity};
use super::release::{Offer, Package, ProgressFn, UpdateBackend, UpdateFailure};
use super::warning::Warning;
use super::{CHECK_TIMEOUT, RELEASES_URL, STATUS_EVENT};
use crate::events::subscribers::UiEmitter;
use crate::state::AppState;
use chrono::{DateTime, SecondsFormat, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use serde_json::Value;
use std::sync::{Arc, Weak};
use std::time::Duration;

/// How often download progress is pushed to the window.
const PROGRESS_EVERY: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    NotChecked,
    Checking,
    UpToDate,
    Available,
    Downloading { received: u64, total: Option<u64> },
    Ready,
    Restarting,
    Failed { message: String },
}

impl Phase {
    fn name(&self) -> &'static str {
        match self {
            Phase::NotChecked => "not_checked",
            Phase::Checking => "checking",
            Phase::UpToDate => "up_to_date",
            Phase::Available => "available",
            Phase::Downloading { .. } => "downloading",
            Phase::Ready => "ready",
            Phase::Restarting => "restarting",
            Phase::Failed { .. } => "failed",
        }
    }

    /// A download, a downloaded update or a restart is under way: a check
    /// must not replace the offer it belongs to.
    fn busy(&self) -> bool {
        matches!(
            self,
            Phase::Downloading { .. } | Phase::Ready | Phase::Restarting
        )
    }
}

/// Download progress, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub received: u64,
    pub total: Option<u64>,
}

/// What the window shows (`GET /api/update/status`, the `update_status`
/// push).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UpdateStatus {
    pub current_version: String,
    /// `not_checked`, `checking`, `up_to_date`, `available`,
    /// `downloading`, `ready` (downloaded and verified, waiting for the
    /// restart), `restarting` or `failed`.
    pub state: &'static str,
    /// Why the last step failed (`failed`), in a trader's words.
    pub message: Option<String>,
    pub progress: Option<Progress>,
    pub offer: Option<Offer>,
    pub package: Package,
    /// This copy can download and install the update itself.
    pub can_install: bool,
    /// Why it cannot (a `.deb` install, a build from source).
    pub manual_reason: Option<&'static str>,
    pub releases_url: &'static str,
    /// When the last check finished (RFC 3339).
    pub checked_at: Option<String>,
}

struct Inner {
    phase: Phase,
    offer: Option<Offer>,
    checked_at: Option<DateTime<Utc>>,
}

/// The answer to an Update click or a restart request.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// The step went ahead.
    Proceed(UpdateStatus),
    /// Something is running: ask the trader, naming it, and send this
    /// activity back as the confirmation.
    Confirm {
        activity: Activity,
        warning: Warning,
    },
    /// The step cannot happen now; the message says why.
    Refused(String),
}

/// The update state of this app: one per process, in `AppState`.
pub struct UpdateService {
    current: String,
    backend: parking_lot::RwLock<Option<Arc<dyn UpdateBackend>>>,
    inner: Mutex<Inner>,
}

impl UpdateService {
    pub fn new(current_version: &str) -> Arc<Self> {
        Arc::new(Self {
            current: current_version.to_string(),
            backend: parking_lot::RwLock::new(None),
            inner: Mutex::new(Inner {
                phase: Phase::NotChecked,
                offer: None,
                checked_at: None,
            }),
        })
    }

    /// The update plugin of the desktop shell (unset in a test or the
    /// development server, where updates are not offered).
    pub fn set_backend(&self, backend: Arc<dyn UpdateBackend>) {
        *self.backend.write() = Some(backend);
    }

    fn backend(&self) -> Option<Arc<dyn UpdateBackend>> {
        self.backend.read().clone()
    }

    pub fn current_version(&self) -> &str {
        &self.current
    }

    fn package(&self) -> Package {
        self.backend()
            .map(|b| b.package())
            .unwrap_or(Package::Unknown)
    }

    pub fn status(&self) -> UpdateStatus {
        let package = self.package();
        let can_install = package.self_updates() && self.backend().is_some();
        let inner = self.inner.lock();
        let (message, progress) = match &inner.phase {
            Phase::Failed { message } => (Some(message.clone()), None),
            Phase::Downloading { received, total } => (
                None,
                Some(Progress {
                    received: *received,
                    total: *total,
                }),
            ),
            _ => (None, None),
        };
        UpdateStatus {
            current_version: self.current.clone(),
            state: inner.phase.name(),
            message,
            progress,
            offer: inner.offer.clone(),
            package,
            can_install,
            manual_reason: if can_install {
                None
            } else {
                package.manual_reason().or(Package::Unknown.manual_reason())
            },
            releases_url: RELEASES_URL,
            checked_at: inner
                .checked_at
                .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true)),
        }
    }

    async fn push(&self, ctx: &AppState) {
        let v = serde_json::to_value(self.status()).unwrap_or(Value::Null);
        ctx.ui.emit(STATUS_EVENT, v).await;
    }

    /// Ask the update address for a newer version. A check while one is
    /// running, or while an update downloads or waits for the restart,
    /// answers with the state as it is.
    pub async fn check(&self, ctx: &AppState) -> UpdateStatus {
        let Some(backend) = self.backend() else {
            {
                let mut inner = self.inner.lock();
                if !inner.phase.busy() {
                    inner.phase = Phase::Failed {
                        message: "This copy of The Bridge cannot check for updates. Download the newest version from the release page.".into(),
                    };
                }
            }
            return self.status();
        };
        let already = {
            let mut inner = self.inner.lock();
            let already = inner.phase.busy() || inner.phase == Phase::Checking;
            if !already {
                inner.phase = Phase::Checking;
            }
            already
        };
        if already {
            return self.status();
        }
        self.push(ctx).await;
        let found = match tokio::time::timeout(CHECK_TIMEOUT, backend.check()).await {
            Ok(r) => r,
            Err(_) => Err(UpdateFailure::Network("the check timed out".into())),
        };
        {
            let mut inner = self.inner.lock();
            inner.checked_at = Some(ctx.now());
            match found {
                Ok(Some(offer)) => {
                    tracing::info!("Update: version {} is available", offer.version);
                    inner.offer = Some(offer);
                    inner.phase = Phase::Available;
                }
                Ok(None) => {
                    tracing::info!("Update: {} is the newest version", self.current);
                    inner.offer = None;
                    inner.phase = Phase::UpToDate;
                }
                Err(f) => {
                    tracing::warn!("Update check failed: {}", f.detail());
                    inner.offer = None;
                    inner.phase = Phase::Failed {
                        message: f.message(&self.current),
                    };
                }
            }
        }
        self.push(ctx).await;
        self.status()
    }

    /// The trader clicked Update. Refused unless a self-updating copy has an
    /// offer; asks first when something is running and `confirmed` does
    /// not cover it; otherwise starts the download (an owned task) and
    /// answers at once. Progress and the result arrive as `update_status`.
    pub async fn request_install(
        self: &Arc<Self>,
        ctx: &AppState,
        confirmed: Option<Activity>,
    ) -> Decision {
        let Some(backend) = self.backend() else {
            return Decision::Refused(
                Package::Unknown
                    .manual_reason()
                    .unwrap_or_default()
                    .to_string(),
            );
        };
        if let Some(reason) = backend.package().manual_reason() {
            return Decision::Refused(reason.to_string());
        }
        if let Some(refusal) = self.install_refusal() {
            return Decision::Refused(refusal);
        }
        let now = activity(ctx).await;
        if let Some(d) = confirm_unless_covered(now, confirmed, broker_name(ctx)) {
            return d;
        }
        let version = {
            let mut inner = self.inner.lock();
            // Claim under the lock that checks: a second click while the
            // first is downloading changes nothing.
            let can_start = matches!(inner.phase, Phase::Available | Phase::Failed { .. });
            match (&inner.offer, can_start) {
                (Some(offer), true) => {
                    let v = offer.version.clone();
                    inner.phase = Phase::Downloading {
                        received: 0,
                        total: None,
                    };
                    v
                }
                _ => return Decision::Refused(refusal_for(&inner)),
            }
        };
        tracing::info!("Update: downloading version {}", version);
        self.push(ctx).await;
        let svc = self.clone();
        let weak: Weak<AppState> = match ctx.arc() {
            Some(a) => Arc::downgrade(&a),
            None => Weak::new(),
        };
        ctx.spawn(async move { svc.run_download(weak, backend, version).await });
        Decision::Proceed(self.status())
    }

    fn install_refusal(&self) -> Option<String> {
        let inner = self.inner.lock();
        match (&inner.phase, &inner.offer) {
            (Phase::Available | Phase::Failed { .. }, Some(_)) => None,
            _ => Some(refusal_for(&inner)),
        }
    }

    async fn run_download(
        self: Arc<Self>,
        ctx: Weak<AppState>,
        backend: Arc<dyn UpdateBackend>,
        version: String,
    ) {
        let progress_svc = self.clone();
        let progress: ProgressFn = Box::new(move |received, total| {
            let mut inner = progress_svc.inner.lock();
            if let Phase::Downloading { .. } = inner.phase {
                inner.phase = Phase::Downloading { received, total };
            }
        });
        let download = backend.download(&version, progress);
        tokio::pin!(download);
        let mut tick = tokio::time::interval(PROGRESS_EVERY);
        tick.tick().await;
        let result = loop {
            tokio::select! {
                r = &mut download => break r,
                _ = tick.tick() => {
                    if let Some(c) = ctx.upgrade() {
                        self.push(&c).await;
                    }
                }
            }
        };
        {
            let mut inner = self.inner.lock();
            match &result {
                Ok(()) => {
                    tracing::info!("Update: version {} is downloaded and verified", version);
                    inner.phase = Phase::Ready;
                }
                Err(f) => {
                    tracing::warn!("Update download failed: {}", f.detail());
                    inner.phase = Phase::Failed {
                        message: f.download_message(&self.current),
                    };
                }
            }
        }
        if let Some(c) = ctx.upgrade() {
            self.push(&c).await;
        }
    }

    /// The trader chose Later after the download: forget the downloaded
    /// update (it is held in memory) and offer it again.
    pub async fn cancel(&self, ctx: &AppState) -> UpdateStatus {
        let discard = {
            let mut inner = self.inner.lock();
            if inner.phase == Phase::Ready {
                inner.phase = Phase::Available;
                true
            } else {
                false
            }
        };
        if discard {
            if let Some(b) = self.backend() {
                b.discard();
            }
            self.push(ctx).await;
        }
        self.status()
    }

    /// The window asks to restart into the downloaded update. Goes ahead
    /// only with a verified update waiting and everything running within
    /// what the trader confirmed; then the state is `restarting` and the
    /// shell installs and restarts.
    pub async fn begin_restart(&self, ctx: &AppState, confirmed: Option<Activity>) -> Decision {
        {
            let inner = self.inner.lock();
            if inner.phase != Phase::Ready {
                return Decision::Refused(refusal_for(&inner));
            }
        }
        let now = activity(ctx).await;
        if let Some(d) = confirm_unless_covered(now, confirmed, broker_name(ctx)) {
            return d;
        }
        {
            let mut inner = self.inner.lock();
            if inner.phase != Phase::Ready {
                return Decision::Refused(refusal_for(&inner));
            }
            inner.phase = Phase::Restarting;
        }
        // A run restarts paused: the trader starts it again on the new
        // version, as after any restart.
        let paused = ctx
            .trading
            .runner
            .pause_all("Paused because The Bridge restarted to install an update");
        if !paused.is_empty() {
            tracing::info!("Paused {} OpenScript run(s) for the update", paused.len());
        }
        tracing::info!("Update: restarting to install the update");
        self.push(ctx).await;
        Decision::Proceed(self.status())
    }

    /// Installing failed after [`Self::begin_restart`]: OpenAlgo stays on
    /// this version and says why.
    pub async fn install_failed(&self, ctx: &AppState, failure: UpdateFailure) {
        tracing::error!("Update install failed: {}", failure.detail());
        if let Some(b) = self.backend() {
            b.discard();
        }
        {
            let mut inner = self.inner.lock();
            inner.phase = Phase::Failed {
                message: failure.message(&self.current),
            };
        }
        self.push(ctx).await;
    }
}

/// Why Update or a restart cannot go ahead in the current state.
fn refusal_for(inner: &Inner) -> String {
    match (&inner.phase, &inner.offer) {
        (Phase::Downloading { .. }, _) => "The update is already downloading.".into(),
        (Phase::Restarting, _) => "The Bridge is already restarting to install the update.".into(),
        (Phase::Ready, _) => "The update is downloaded. Restart The Bridge to install it.".into(),
        (Phase::Checking, _) => "The Bridge is checking for updates. Try again in a moment.".into(),
        (Phase::Available | Phase::Failed { .. }, Some(_)) => {
            "Download the update first: choose Update.".into()
        }
        _ => "There is no update to install. Check for updates first.".into(),
    }
}

fn confirm_unless_covered(
    now: Activity,
    confirmed: Option<Activity>,
    broker: Option<String>,
) -> Option<Decision> {
    if now.is_quiet() || confirmed.is_some_and(|c| now.covered_by(&c)) {
        return None;
    }
    let warning = now.warning(broker.as_deref())?;
    Some(Decision::Confirm {
        activity: now,
        warning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_confirmation_covers_only_what_the_trader_saw() {
        let seen = Activity {
            live_strategies: 1,
            sandbox_open_orders: 2,
            ..Default::default()
        };
        assert!(seen.covered_by(&seen));
        // Less is still covered: a strategy stopped since.
        assert!(Activity::default().covered_by(&seen));
        // More is not: a new Live order, an unreadable order book.
        let more = Activity {
            live_open_orders: 1,
            ..seen
        };
        assert!(!more.covered_by(&seen));
        let unknown = Activity {
            live_orders_unknown: true,
            ..seen
        };
        assert!(!unknown.covered_by(&seen));
        assert!(confirm_unless_covered(seen, Some(seen), None).is_none());
        assert!(matches!(
            confirm_unless_covered(more, Some(seen), None),
            Some(Decision::Confirm { .. })
        ));
        assert!(matches!(
            confirm_unless_covered(seen, None, None),
            Some(Decision::Confirm { .. })
        ));
        assert!(confirm_unless_covered(Activity::default(), None, None).is_none());
    }
}
