//! REL-02 against the real stores (`AppState::open_upgrading`): an upgrade
//! backs up every store before the migrations run and keeps no copy once it
//! opened, a migration that fails puts that start's backup back so the
//! older version opens the folder as before, no leftover backup survives
//! an action that deletes what it would hold (forgetting a broker, a broker
//! sign-out, a fresh start, account reset), and a folder written by a
//! newer version is refused before anything is opened. The decisions are unit-tested in `db::upgrade`.

use openalgo_desktop_lib::brokers::BrokerRegistry;
use openalgo_desktop_lib::clock::ManualClock;
use openalgo_desktop_lib::db::upgrade::{self, OpenError, Recorded, Refusal};
use openalgo_desktop_lib::events::SessionEndReason;
use openalgo_desktop_lib::security::keystore::MemoryKeyStore;
use openalgo_desktop_lib::services::auth_service::AuthService;
use openalgo_desktop_lib::services::broker_auth_service::{BrokerAuthService, REVOKE_MARKER};
use openalgo_desktop_lib::services::broker_config_service::{BrokerConfigService, ForgetOutcome};
use openalgo_desktop_lib::state::{AppState, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::webui_support::ist;

fn opts(keys: &Arc<MemoryKeyStore>) -> OpenOptions {
    OpenOptions {
        keystore: keys.clone(),
        clock: ManualClock::new(ist(2026, 10, 15, 9, 0)),
        brokers: Arc::new(BrokerRegistry::with(vec![])),
    }
}

/// Stop the app and wait until nothing holds it, so its databases are
/// closed (Windows cannot replace a file that is still open).
async fn close(ctx: Arc<AppState>) {
    ctx.shutdown().await;
    let weak = Arc::downgrade(&ctx);
    drop(ctx);
    for _ in 0..500 {
        if weak.upgrade().is_none() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("the app context was still held after shutdown");
}

/// The schema and the recorded migrations of a SQLite store.
fn shape(db: &Path) -> (Vec<(String, String)>, Vec<String>, i64) {
    let c = rusqlite::Connection::open(db).unwrap();
    let mut s = c
        .prepare(
            "SELECT type, name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap();
    let objects = s
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let mut s = c
        .prepare("SELECT name FROM migrations ORDER BY name")
        .unwrap();
    let migrations = s
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<String>>>()
        .unwrap();
    let users: i64 = c
        .query_row("SELECT COUNT(*) FROM users", [], |r| r.get(0))
        .unwrap();
    (objects, migrations, users)
}

fn recorded(dir: &Path) -> String {
    match upgrade::read_recorded(dir) {
        Recorded::Version(v) => v.to_string(),
        other => format!("{:?}", other),
    }
}

/// A folder version 1.0.0 opened and set up, then closed.
async fn folder_of_1_0_0(keys: &Arc<MemoryKeyStore>) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let ctx = AppState::open_upgrading(dir.path(), opts(keys), "1.0.0").unwrap();
    AuthService::setup(&ctx, "trader", "trader@example.com", "Secret@123").unwrap();
    close(ctx).await;
    assert_eq!(recorded(dir.path()), "1.0.0");
    assert!(
        !dir.path().join(upgrade::BACKUPS_DIR).exists(),
        "a new folder needs no backup"
    );
    dir
}

/// Every store is copied before the migrations run (read back through
/// `db::upgrade::backup` on the same folder), and once the upgraded app has
/// opened, no copy of the older state is kept.
#[tokio::test(flavor = "multi_thread")]
async fn an_upgrade_backs_up_every_store_and_keeps_no_copy_once_open() {
    let keys = Arc::new(MemoryKeyStore::new());
    let dir = folder_of_1_0_0(&keys).await;
    let before = shape(&dir.path().join("openalgo.db"));
    let probe = upgrade::backup(dir.path(), "1.0.0", "1.1.0", ist(2026, 10, 15, 8, 0)).unwrap();
    let manifest = &upgrade::backups(dir.path())[0].1;
    for store in ["openalgo.db", "logs.db", "sandbox.db", "historify.duckdb"] {
        assert!(manifest.files.iter().any(|f| f == store), "{}", store);
        assert!(probe.join(store).is_file(), "{}", store);
    }
    assert_eq!(shape(&probe.join("openalgo.db")), before);
    drop(duckdb::Connection::open(probe.join("historify.duckdb")).unwrap());

    let ctx = AppState::open_upgrading(dir.path(), opts(&keys), "1.1.0").unwrap();
    assert_eq!(recorded(dir.path()), "1.1.0");
    assert!(
        upgrade::backups(dir.path()).is_empty(),
        "a good upgrade keeps no copy of the older state"
    );
    close(ctx).await;
}

/// A backup a successful start could not delete (another program held one
/// of its files), as it sits in `backups/` while the app runs.
fn plant_leftover(dir: &Path) -> PathBuf {
    let leftover = dir
        .join(upgrade::BACKUPS_DIR)
        .join("1.0.0-20261015T093000Z");
    std::fs::create_dir_all(&leftover).unwrap();
    std::fs::write(
        leftover.join("manifest.json"),
        serde_json::to_vec(&upgrade::Manifest {
            from_version: "1.0.0".into(),
            to_version: "1.1.0".into(),
            created_at: "2026-10-15T09:30:00.000Z".into(),
            files: vec!["openalgo.db".into()],
        })
        .unwrap(),
    )
    .unwrap();
    std::fs::write(leftover.join("openalgo.db"), b"the account as it was").unwrap();
    assert_eq!(upgrade::backups(dir).len(), 1);
    leftover
}

/// Security review, finding 1: what the trader deletes never survives in
/// `backups/`. A successful start deletes every backup; a leftover it
/// could not delete goes with the first action that removes something a
/// copy would still hold: forgetting a broker, a broker sign-out (logout),
/// the daily boundary ending a broker session (SES-02), a fresh start after
/// a forgotten password (KEY-01) and account reset.
#[tokio::test(flavor = "multi_thread")]
async fn no_backup_survives_a_deleting_action() {
    let mut survived = Vec::new();
    for action in [
        "forget broker",
        "broker sign-out",
        "daily boundary",
        "fresh start",
        "account reset",
    ] {
        let keys = Arc::new(MemoryKeyStore::new());
        let dir = folder_of_1_0_0(&keys).await;
        let ctx = AppState::open_upgrading(dir.path(), opts(&keys), "1.0.0").unwrap();
        let leftover = plant_leftover(dir.path());
        match action {
            "forget broker" => {
                let got = BrokerConfigService::forget(&ctx, "zerodha", false)
                    .await
                    .unwrap();
                assert!(matches!(got, ForgetOutcome::Forgotten { .. }), "{:?}", got);
            }
            "broker sign-out" => BrokerAuthService::revoke(&ctx, SessionEndReason::Logout)
                .await
                .unwrap(),
            "daily boundary" => {
                let boundary = ctx.now() + chrono::Duration::seconds(1);
                BrokerAuthService::expire_before(&ctx, boundary)
                    .await
                    .unwrap();
            }
            "fresh start" => AuthService::start_fresh(&ctx, "Another@456", "Another@456").unwrap(),
            "account reset" => AuthService::reset_account_everywhere(&ctx).await.unwrap(),
            other => unreachable!("{}", other),
        }
        if leftover.exists() || !upgrade::backups(dir.path()).is_empty() {
            survived.push(action);
        }
        close(ctx).await;
    }
    assert!(
        survived.is_empty(),
        "a leftover backup survived: {:?}",
        survived
    );
}

/// A migration that fails half way (one migration already applied, a later
/// one refused) leaves the folder exactly as the old version left it.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_migration_puts_the_backup_back_and_keeps_the_old_version() {
    let keys = Arc::new(MemoryKeyStore::new());
    let dir = folder_of_1_0_0(&keys).await;
    let main = dir.path().join("openalgo.db");
    // A pending broker revoke (SEC-05b) survives the restore.
    std::fs::write(dir.path().join(REVOKE_MARKER), b"pending").unwrap();
    {
        // 062 will re-create alert_log (a change the failed open makes);
        // 070 then fails: its index cannot go on a view.
        let c = rusqlite::Connection::open(&main).unwrap();
        c.execute_batch(
            "DROP TABLE alert_log;
             DELETE FROM migrations WHERE name = '062_alert_log';
             DROP INDEX ix_telegram_users_telegram_id;
             ALTER TABLE telegram_users RENAME TO telegram_users_kept;
             CREATE VIEW telegram_users AS SELECT * FROM telegram_users_kept;
             DELETE FROM migrations WHERE name = '070_telegram';",
        )
        .unwrap();
    }
    let before = shape(&main);
    let refusal = match AppState::open_upgrading(dir.path(), opts(&keys), "1.1.0") {
        Err(OpenError::Refused(r)) => r,
        Err(OpenError::Open(e)) => panic!("expected the restore, got {}", e),
        Ok(_) => panic!("the migration was expected to fail"),
    };
    match &refusal {
        Refusal::Restored {
            from,
            current,
            cause,
        } => {
            assert_eq!((from.as_str(), current.as_str()), ("1.0.0", "1.1.0"));
            assert_eq!(cause, "its databases could not be updated to this version");
        }
        other => panic!("{:?}", other),
    }
    assert!(refusal
        .message()
        .contains("your data was put back exactly as it was before this version opened it"));
    // alert_log, which the failed open re-created, is gone again; nothing
    // the half-run upgrade did is left.
    assert_eq!(shape(&main), before);
    assert_eq!(recorded(dir.path()), "1.0.0");
    assert!(dir.path().join(REVOKE_MARKER).exists());
    // The folder equals the backup now; no copy is kept.
    assert!(upgrade::backups(dir.path()).is_empty());
    // Undo the sabotage: the restored folder then opens and migrates.
    {
        let c = rusqlite::Connection::open(&main).unwrap();
        c.execute_batch(
            "DROP VIEW telegram_users;
             ALTER TABLE telegram_users_kept RENAME TO telegram_users;",
        )
        .unwrap();
    }
    let ctx = AppState::open_upgrading(dir.path(), opts(&keys), "1.1.0").unwrap();
    close(ctx).await;
    assert_eq!(recorded(dir.path()), "1.1.0");
    let (objects, migrations, users) = shape(&main);
    assert!(objects.contains(&("table".into(), "alert_log".into())));
    assert!(migrations.contains(&"070_telegram".to_string()));
    assert_eq!(users, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_folder_written_by_a_newer_version_is_refused_before_anything_opens() {
    let keys = Arc::new(MemoryKeyStore::new());
    let dir = tempfile::tempdir().unwrap();
    let ctx = AppState::open_upgrading(dir.path(), opts(&keys), "1.2.0").unwrap();
    close(ctx).await;
    let files = |d: &Path| {
        let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(d)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| {
                (
                    e.file_name().to_string_lossy().into_owned(),
                    std::fs::read(e.path()).unwrap(),
                )
            })
            .collect();
        v.sort();
        v
    };
    let before = files(dir.path());
    match AppState::open_upgrading(dir.path(), opts(&keys), "1.1.0") {
        Err(OpenError::Refused(r @ Refusal::Downgrade { .. })) => {
            assert!(r.message().starts_with(
                "Your The Bridge data was last opened by version 1.2.0, which is newer than this copy (1.1.0)."
            ));
        }
        Err(OpenError::Refused(other)) => panic!("{:?}", other),
        Err(OpenError::Open(e)) => panic!("{}", e),
        Ok(_) => panic!("an older version opened a newer folder"),
    }
    assert_eq!(files(dir.path()), before, "nothing was opened or written");
    assert_eq!(recorded(dir.path()), "1.2.0");
}
