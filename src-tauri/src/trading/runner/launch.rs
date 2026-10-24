//! Starting a deployment: the checks that refuse it, and the launch of its
//! run in the hidden strategy window.

use super::host::{Launch, RunnerHost};
use super::inbox::Inbox;
use super::store;
use super::{
    random_token, Refusal, RunEntry, RunInfo, Runner, NO_HOST_MESSAGE, SHUTTING_DOWN_MESSAGE,
};
use crate::services::live_owner::Destination;
use crate::strategy::dispatch::RunMode;
use crate::trading::names::names_something;
use chrono_tz::Asia::Kolkata;
use parking_lot::Mutex;
use serde_json::{Map, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

impl Runner {
    /// Start one deployment and answer with its run. Never waits for the
    /// strategy to do anything: the page loads, replays history and begins.
    pub fn start(&self, name: &str) -> Result<RunInfo, Refusal> {
        if !names_something(name) {
            return Err(Refusal::new(
                400,
                crate::trading::names::script_name_refusal(name),
            ));
        }
        let saved = {
            let conn = self
                .db
                .conn()
                .map_err(|_| Refusal::new(409, "The run settings could not be read just now."))?;
            match store::require(&conn, name) {
                Ok(Ok(d)) => d,
                Ok(Err(why)) => return Err(Refusal::new(409, why)),
                Err(e) => {
                    tracing::error!("Could not read OpenScript run settings: {}", e);
                    return Err(Refusal::new(
                        409,
                        "The run settings could not be read just now.",
                    ));
                }
            }
        };
        let run_id = saved.deployment.clone();
        let where_ = format!(
            "{} on {} {} at {}",
            saved.script, saved.symbol, saved.exchange, saved.interval
        );
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(Refusal::new(409, SHUTTING_DOWN_MESSAGE));
        }
        let Some(host) = self.host.read().clone() else {
            return Err(Refusal::new(409, NO_HOST_MESSAGE));
        };
        if self.scripts.program(&saved.script).is_none() {
            return Err(Refusal::new(
                409,
                format!(
                    "{} has no compiled program yet. Open it in the chart and save it once the console shows no errors.",
                    saved.script
                ),
            ));
        }
        {
            let mut reg = self.reg.lock();
            if self.shutting_down.load(Ordering::SeqCst) {
                return Err(Refusal::new(409, SHUTTING_DOWN_MESSAGE));
            }
            if reg.runs.contains_key(&run_id) {
                return Err(Refusal::new(409, format!("{} is already running", where_)));
            }
            if reg.stopping.contains(&run_id) || reg.closing.contains(&run_id) {
                return Err(Refusal::new(
                    409,
                    format!("{} is still stopping, try again in a moment", where_),
                ));
            }
            if reg.starting.contains(&run_id) {
                return Err(Refusal::new(409, format!("{} is already starting", where_)));
            }
            if reg.deleting.contains(&run_id) {
                return Err(Refusal::new(
                    409,
                    format!("{} is being removed, so it was not started", where_),
                ));
            }
            // Claimed in the hold that checked; released on every path below.
            reg.starting.insert(run_id.clone());
        }
        let result = self.launch(&saved, &host, &where_);
        self.reg.lock().starting.remove(&run_id);
        result
    }

    fn launch(
        &self,
        saved: &store::Deployment,
        host: &Arc<dyn RunnerHost>,
        where_: &str,
    ) -> Result<RunInfo, Refusal> {
        let run_id = saved.deployment.clone();
        // The settings were read before the claim: a removal may have landed.
        let still = self
            .db
            .conn()
            .ok()
            .and_then(|c| store::read(&c, &run_id).ok().flatten());
        if still.is_none() {
            return Err(Refusal::new(
                409,
                format!(
                    "{} was removed while it was being started, so it was not started",
                    where_
                ),
            ));
        }
        let mode = self.side_for(saved)?;
        let started = self.clock.now();
        let log_file = self.logs_dir.join(format!(
            "{}_{}_IST.log",
            run_id,
            started.with_timezone(&Kolkata).format("%Y%m%d_%H%M%S")
        ));
        if let Err(e) = std::fs::create_dir_all(&self.logs_dir) {
            tracing::error!("Could not create the OpenScript log folder: {}", e);
            return Err(Refusal::new(
                409,
                "The folder strategy logs are written to could not be created. Check that the app's data folder can be written to.",
            ));
        }
        let (destination, incarnation) = self.begin_run(saved, mode)?;
        let info = RunInfo {
            run_id: run_id.clone(),
            script: saved.script.clone(),
            symbol: saved.symbol.clone(),
            exchange: saved.exchange.clone(),
            interval: saved.interval.clone(),
            product: saved.product.clone(),
            mode,
            started_at: started,
            log_file,
            close_pending: false,
            unsaved_orders: 0,
            unresolved_orders: 0,
            awaiting_confirmation: false,
            destination,
            incarnation,
        };
        let entry = self.new_entry(&info, saved.inputs.clone());
        let mut lines = vec![format!(
            "=== Run started at {} on the {} side ===",
            started
                .with_timezone(&Kolkata)
                .format("%Y-%m-%d %H:%M:%S IST"),
            mode.as_str()
        )];
        lines.extend(self.start_lines(&info));
        self.write_log(&info, &lines);
        self.reg.lock().runs.insert(run_id.clone(), entry.clone());
        let launch = Launch {
            run_id: run_id.clone(),
            page: format!(
                "/openscript-runner.html#run={}&token={}",
                urlencoding::encode(&run_id),
                entry.token
            ),
            title: format!("OpenScript {}", saved.script),
        };
        if let Err(why) = host.open(&launch) {
            self.reg.lock().runs.remove(&run_id);
            entry.inbox.close();
            if let Err(e) = self
                .db
                .conn()
                .and_then(|c| store::forget_incarnation(&c, incarnation))
            {
                tracing::warn!("Could not forget the run that did not start: {}", e);
            }
            self.write_log(
                &info,
                &[format!("The strategy could not be started: {}", why)],
            );
            return Err(Refusal::new(409, why));
        }
        if let Ok(conn) = self.db.conn() {
            if let Err(e) = store::record_mode(&conn, &run_id, mode.as_str()) {
                tracing::error!("Could not record the side of {}: {}", run_id, e);
            }
        }
        self.spawn_pump(&entry);
        tracing::info!("Started the OpenScript run {}", run_id);
        Ok(info)
    }

    /// The side a run starts on, decided here once and kept for the life of
    /// the run. A mode that cannot be read refuses the start (LOG-09):
    /// taken as off, a deployment meant for the sandbox would start live.
    fn side_for(&self, saved: &store::Deployment) -> Result<RunMode, Refusal> {
        let services = self.services();
        let mode = match services.analyzer_on() {
            Ok(true) => RunMode::Sandbox,
            Ok(false) => RunMode::Live,
            Err(why) => {
                tracing::error!("{} was not started: {}", saved.deployment, why);
                return Err(Refusal::new(
                    409,
                    format!(
                        "{} was not started, because The Bridge could not read whether analyzer mode is on. Restart The Bridge, then start it again.",
                        saved.script
                    ),
                ));
            }
        };
        if let Err(why) = services.authorised(mode) {
            return Err(Refusal::new(
                409,
                format!(
                    "{} trades live and cannot start until your broker is connected. {}",
                    saved.script, why
                ),
            ));
        }
        Ok(mode)
    }

    /// Where a run starting now trades, and its incarnation, recorded before
    /// it can send anything (NEW-TR-01, NEW-TR-03). A Live start whose
    /// broker session cannot be named is refused: its orders could not be
    /// bound to an account.
    fn begin_run(
        &self,
        saved: &store::Deployment,
        mode: RunMode,
    ) -> Result<(Destination, i64), Refusal> {
        let destination = match mode {
            RunMode::Sandbox => Destination::sandbox(),
            RunMode::Live => self
                .services()
                .live_owner()
                .map(Destination::live)
                .filter(Destination::is_known)
                .ok_or_else(|| {
                    Refusal::new(
                        409,
                        format!(
                            "{} was not started, because The Bridge could not tell which broker account it would trade through. Log in to your broker again, then start it.",
                            saved.script
                        ),
                    )
                })?,
        };
        let recorded = self.db.conn().and_then(|c| {
            store::begin_incarnation(&c, &saved.deployment, &destination, self.clock.now())
        });
        match recorded {
            Ok(incarnation) => Ok((destination, incarnation)),
            Err(e) => {
                tracing::error!("Could not record the run of {}: {}", saved.deployment, e);
                Err(Refusal::new(
                    409,
                    format!(
                        "{} was not started, because its run could not be recorded. Try again in a moment.",
                        saved.script
                    ),
                ))
            }
        }
    }

    /// A new run's registry entry, with the inputs it started with, its page
    /// secret and an empty inbox.
    fn new_entry(&self, info: &RunInfo, inputs: Map<String, Value>) -> Arc<RunEntry> {
        Arc::new(RunEntry {
            info: info.clone(),
            inputs,
            token: random_token(),
            inbox: Inbox::new(info.incarnation),
            halted: AtomicBool::new(false),
            close_pending: AtomicBool::new(false),
            unsaved: Mutex::new(Vec::new()),
            unresolved: AtomicUsize::new(0),
            page_waiting: AtomicBool::new(false),
            stuck_told: AtomicBool::new(false),
            last_seen: Mutex::new(Instant::now()),
            anchor: Mutex::new(None),
            last_bar: Mutex::new(None),
            pump: Mutex::new(None),
            cancel: self.cancel.child_token(),
            order_lock: tokio::sync::Mutex::new(()),
            answered: Mutex::new(std::collections::BTreeMap::new()),
        })
    }
}
