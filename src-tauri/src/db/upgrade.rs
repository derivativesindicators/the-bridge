//! Opening the data folder across versions (REL-02): back up before an
//! upgrade migrates, put the backup back when the upgraded open fails, and
//! refuse a folder written by a newer version.
//!
//! The folder records the version that last opened it, in
//! `data-version.json`. A version records itself before it opens the
//! stores (after the backup, before any migration), so once a newer version
//! may have changed the data an older one is refused, also after a crash
//! mid-migration; a record that cannot be written stops the start with
//! nothing changed, and a failed upgraded open puts the old record back with
//! the stores (NEW-REL-06). At start:
//!
//! | Recorded | This copy | What happens |
//! | --- | --- | --- |
//! | nothing, no stores | any | a new folder: record, open |
//! | nothing, stores present | any | from before versions were recorded: back up, record, open |
//! | an older version | newer | back up every store to `backups/<old>-<UTC time>/`, record, open (migrations run) |
//! | the same version | same | open |
//! | a newer version | older | refused: nothing is opened, migrated or written |
//!
//! **A backup lives only for the start that made it** (security review of
//! REL-02; `docs/port/data-upgrades.md`). It exists to undo a failed
//! migration, automatically, at that same start, and for nothing else:
//!
//! - once the upgraded open succeeds, every backup in `backups/` is deleted
//!   (on every successful open, so a leftover from an earlier start goes
//!   too);
//! - when the open after a backup fails, every store is put back from that
//!   backup (side files such as `-wal` removed first, each file replaced by
//!   a rename), the recorded version stays the old one, and the backup is
//!   deleted: the live folder now equals it;
//! - only a backup that could not be put back is kept, and the start-up
//!   page names it, since it is then the one good copy (a later start that
//!   fails and puts its own backup back leaves it; the next successful open
//!   deletes it).
//!
//! So no backup outlives a successful start: broker keys forgotten, a
//! session revoked, an API key regenerated, a password or TOTP changed, or
//! an account reset while the app runs are never kept in an older copy,
//! and no older copy can be restored over newer state. A leftover that a
//! successful start could not delete (another program holding one of its
//! files) is deleted again by every action that removes something a copy
//! would still hold ([`purge_backups`]: forgetting a broker, any broker
//! sign-out, a password change or reset, account reset). The broker-revoke
//! marker (`broker-revoke.pending`) is not a store: a restore never
//! removes it.
//!
//! A backup is made in `backups/.partial-<name>/` and renamed into place
//! only once every file and its manifest are on disk, so a crash mid-backup
//! leaves a `.partial-` folder that is never used and is removed later,
//! while the live stores were only read. SQLite stores are copied with
//! `VACUUM INTO` (a consistent copy, the WAL included); the DuckDB store and
//! the key files are copied byte for byte, which is safe because nothing has
//! them open yet (the app-start path holds the folder lock,
//! [`DataFolderLock`], so no second copy of OpenAlgo has either). Every
//! folder is created owner-only (0700) and every file owner-only (0600)
//! from its first byte, never widened and narrowed later (Windows: created
//! with the default security of the data folder, nothing added). `backups/` and the folders in it are never
//! followed through a link: a link there is left alone, and a `backups`
//! that is not a plain folder stops the upgrade. At most
//! [`KEEP_BACKUPS`] leftovers are ever kept.
//!
//! Anything an upgrade changes outside this folder (the OS keychain) is not
//! in the backup: change it only after the open has succeeded.

use crate::error::AppError;
use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// The version that last opened the folder.
pub const MARKER: &str = "data-version.json";
/// Where backups go, inside the data folder.
pub const BACKUPS_DIR: &str = "backups";
/// Leftover backups kept at most (normally there are none: see the module
/// documentation).
pub const KEEP_BACKUPS: usize = 3;
/// Held by the running app for its whole life (see [`DataFolderLock`]).
pub const LOCK_FILE: &str = "data-folder.lock";
const MANIFEST: &str = "manifest.json";
const PARTIAL: &str = ".partial-";

/// How a store is copied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// SQLite: `VACUUM INTO`; side files `-wal`, `-shm`, `-journal`.
    Sqlite,
    /// DuckDB: the file and its `.wal`, byte for byte, while closed.
    DuckDb,
    /// A plain file (key material), byte for byte.
    File,
}

/// Every store in the data folder, in the order they are copied.
pub const STORES: &[(&str, Kind)] = &[
    ("openalgo.db", Kind::Sqlite),
    ("logs.db", Kind::Sqlite),
    ("sandbox.db", Kind::Sqlite),
    ("historify.duckdb", Kind::DuckDb),
    ("vault.json", Kind::File),
    ("vault.next.json", Kind::File),
    ("secrets.dat", Kind::File),
];

fn side_files(name: &str, kind: Kind) -> Vec<String> {
    match kind {
        Kind::Sqlite => vec![
            format!("{}-wal", name),
            format!("{}-shm", name),
            format!("{}-journal", name),
        ],
        Kind::DuckDb => vec![format!("{}.wal", name)],
        Kind::File => Vec::new(),
    }
}

/// What a backup holds (its `manifest.json`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// The version whose data this is (`unknown` when none was recorded).
    pub from_version: String,
    /// The version that made the backup, about to migrate the data.
    pub to_version: String,
    pub created_at: String,
    /// The files in the backup.
    pub files: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Marker {
    version: String,
    written_at: String,
}

/// The version recorded in the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recorded {
    /// No `data-version.json`.
    Nothing,
    /// A readable version.
    Version(semver::Version),
    /// A file that does not name a version (treated as unknown, never as
    /// newer).
    Unreadable,
}

pub fn read_recorded(dir: &Path) -> Recorded {
    let p = dir.join(MARKER);
    let Ok(text) = fs::read_to_string(&p) else {
        return if p.exists() {
            Recorded::Unreadable
        } else {
            Recorded::Nothing
        };
    };
    serde_json::from_str::<Marker>(&text)
        .ok()
        .and_then(|m| semver::Version::parse(m.version.trim_start_matches('v')).ok())
        .map(Recorded::Version)
        .unwrap_or(Recorded::Unreadable)
}

/// Record `version` as the one that last opened the folder (atomic).
pub fn write_recorded(dir: &Path, version: &str, now: DateTime<Utc>) -> io::Result<()> {
    let m = Marker {
        version: version.to_string(),
        written_at: now.to_rfc3339_opts(SecondsFormat::Secs, true),
    };
    let bytes = serde_json::to_vec_pretty(&m).map_err(io::Error::other)?;
    crate::security::fsperm::write_private_file(&dir.join(MARKER), &bytes)?;
    crate::security::fsperm::sync_dir(dir);
    Ok(())
}

/// Whether the folder holds any store (a folder in use, not a new one).
pub fn holds_stores(dir: &Path) -> bool {
    STORES.iter().any(|(name, _)| dir.join(name).exists())
}

/// Why the data folder was not opened, for the start-up page.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// The folder was last opened by a newer version.
    Downgrade { recorded: String, current: String },
    /// Another copy of OpenAlgo has the folder open.
    InUse,
    /// The backup before the upgrade could not be made; nothing was opened.
    BackupFailed { detail: String },
    /// The upgraded open failed and the backup was put back (and then
    /// deleted: the folder equals it).
    Restored {
        from: String,
        current: String,
        cause: String,
    },
    /// The upgraded open failed and the backup could not be put back; it
    /// is kept, as the one good copy.
    RestoreFailed { backup: PathBuf, detail: String },
    /// This version could not be recorded in the folder before opening it,
    /// so nothing was opened or changed (NEW-REL-06).
    NotRecorded { detail: String },
}

impl Refusal {
    /// The trader's message: what happened, that the data is safe, and
    /// the next step.
    pub fn message(&self) -> String {
        match self {
            Refusal::Downgrade { recorded, current } => format!(
                "Your The Bridge data was last opened by version {recorded}, which is newer than this copy ({current}). \
Opening it with an older version could damage it, so nothing was opened or changed. \
Install The Bridge {recorded} or newer from the release page."
            ),
            Refusal::InUse => "The Bridge is already running and has your data open. Close the other The Bridge window, then start it again.".into(),
            Refusal::BackupFailed { .. } => "The Bridge backs up your data before an update changes it, and this backup could not be made, so nothing was changed. \
Check that there is free disk space (about as much as your data folder) and that the backups folder inside the data folder is an ordinary folder, then start The Bridge again.".into(),
            Refusal::Restored { from, current, cause } => format!(
                "The Bridge {current} could not open your data ({cause}). So that nothing is lost, your data was put back exactly as it was before this version opened it. \
Start The Bridge again to retry. If this happens again, install version {from} from the release page, which opens this data as before."
            ),
            Refusal::NotRecorded { .. } => "The Bridge writes a small file in your data folder (data-version.json) before it opens your data, so that an older version never opens data a newer one has changed. It could not write that file, so nothing was opened or changed. \
Check that there is free disk space and that the data folder and that file are not read-only, then start The Bridge again.".into(),
            Refusal::RestoreFailed { backup, .. } => format!(
                "The Bridge could not open your data after the update, and could not put back the copy it made just before. That copy, your data exactly as the previous version left it, is safe in {}. \
Close The Bridge; in the data folder delete data-version.json and every file ending in -wal, -shm or .wal; copy the files from that folder into the data folder, replacing the ones there; then start The Bridge again.",
                backup.display()
            ),
        }
    }

    /// For the log.
    pub fn detail(&self) -> String {
        match self {
            Refusal::BackupFailed { detail }
            | Refusal::RestoreFailed { detail, .. }
            | Refusal::NotRecorded { detail } => detail.clone(),
            Refusal::Restored { cause, .. } => cause.clone(),
            other => format!("{:?}", other),
        }
    }
}

/// How opening the data folder ended, when it did not open.
#[derive(Debug)]
pub enum OpenError {
    /// Refused or restored; the start-up page shows the message.
    Refused(Refusal),
    /// The open failed with no upgrade involved: the error as it was.
    Open(AppError),
}

/// The cause of a failed open, in a few words a trader can read.
fn cause_of(e: &AppError) -> String {
    match e {
        AppError::Database(_) | AppError::Pool(_) | AppError::DuckDb(_) => {
            "its databases could not be updated to this version".into()
        }
        other => other.to_string(),
    }
}

/// Open the data folder in `dir` for version `current`: refuse a folder
/// written by a newer version, back up one from an older version (or one
/// with no recorded version), record `current`, run `open`, put that backup
/// (and the old record) back if `open` fails, and delete the backups once
/// it succeeded.
pub fn open_data<T>(
    dir: &Path,
    current: &str,
    now: DateTime<Utc>,
    open: impl FnOnce() -> crate::error::Result<T>,
) -> Result<T, OpenError> {
    let this = this_version(current)?;
    let recorded = read_recorded(dir);
    refuse_newer(&recorded, &this, current)?;
    let from = match &recorded {
        Recorded::Version(v) => v.to_string(),
        _ => "unknown".to_string(),
    };
    let backup = back_up_before_upgrade(dir, &recorded, &this, &from, current, now)?;
    let prior = record_before_open(dir, &recorded, &this, current, now, backup.as_deref())?;
    match open() {
        Ok(t) => {
            discard_backups(dir);
            Ok(t)
        }
        Err(e) => match backup {
            Some(backup) => Err(put_back_after_failed_open(
                dir, backup, prior, from, current, e,
            )),
            None => Err(OpenError::Open(e)),
        },
    }
}

/// What `data-version.json` held before this start recorded its own
/// version, so a failed upgraded open puts it back with the stores.
enum PriorMarker {
    /// This start recorded nothing (the folder already named this version).
    Unchanged,
    Absent,
    Held(Vec<u8>),
}

/// Record this version before the stores are opened, and so before any
/// migration changes them (NEW-REL-06): from the moment a newer version may
/// have touched the data, an older one is refused. A record that cannot be
/// written stops the start with nothing changed (this start's backup is
/// deleted: the data equals it).
fn record_before_open(
    dir: &Path,
    recorded: &Recorded,
    this: &semver::Version,
    current: &str,
    now: DateTime<Utc>,
    backup: Option<&Path>,
) -> Result<PriorMarker, OpenError> {
    if matches!(recorded, Recorded::Version(v) if v == this) {
        return Ok(PriorMarker::Unchanged);
    }
    let prior = match fs::read(dir.join(MARKER)) {
        Ok(bytes) => PriorMarker::Held(bytes),
        Err(_) => PriorMarker::Absent,
    };
    if let Err(e) = write_recorded(dir, current, now) {
        tracing::error!(
            "Could not record the data folder's version before opening it: {}",
            e
        );
        if let Some(b) = backup {
            if let Err(re) = fs::remove_dir_all(b) {
                tracing::warn!("Could not remove the unused data backup: {}", re);
            }
        }
        return Err(OpenError::Refused(Refusal::NotRecorded {
            detail: e.to_string(),
        }));
    }
    Ok(prior)
}

/// This build's version, which must be a semantic version.
fn this_version(current: &str) -> Result<semver::Version, OpenError> {
    semver::Version::parse(current).map_err(|e| {
        OpenError::Open(AppError::Internal(format!(
            "this build's version {} is not a version: {}",
            current, e
        )))
    })
}

/// A folder written by a newer version is refused before anything is
/// opened: an older version could damage data in a newer format.
fn refuse_newer(
    recorded: &Recorded,
    this: &semver::Version,
    current: &str,
) -> Result<(), OpenError> {
    let Recorded::Version(v) = recorded else {
        return Ok(());
    };
    if v.cmp_precedence(this) != Ordering::Greater {
        return Ok(());
    }
    tracing::error!(
        "Data folder was written by The Bridge {}; this is {}: not opened",
        v,
        current
    );
    Err(OpenError::Refused(Refusal::Downgrade {
        recorded: v.to_string(),
        current: current.to_string(),
    }))
}

/// Back up every store when an older version (or an unrecorded one) left
/// the folder, before this version's migrations change it. `None` when no
/// backup is needed; a backup that cannot be made refuses the open.
fn back_up_before_upgrade(
    dir: &Path,
    recorded: &Recorded,
    this: &semver::Version,
    from: &str,
    current: &str,
    now: DateTime<Utc>,
) -> Result<Option<PathBuf>, OpenError> {
    let older = match recorded {
        Recorded::Version(v) => v.cmp_precedence(this) == Ordering::Less,
        Recorded::Nothing | Recorded::Unreadable => true,
    };
    if !older || !holds_stores(dir) {
        return Ok(None);
    }
    let b = backup(dir, from, current, now).map_err(|e| {
        tracing::error!("Could not back up the data before the upgrade: {}", e);
        OpenError::Refused(Refusal::BackupFailed {
            detail: e.to_string(),
        })
    })?;
    tracing::info!(
        "Backed up the data of The Bridge {} to {} before opening it with {}",
        from,
        b.display(),
        current
    );
    if let Err(e) = prune(dir, KEEP_BACKUPS) {
        tracing::warn!("Could not remove old data backups: {}", e);
    }
    Ok(Some(b))
}

/// The open after this start's backup failed: put that backup back (never
/// an older one) and say so on the start-up page.
fn put_back_after_failed_open(
    dir: &Path,
    backup: PathBuf,
    prior: PriorMarker,
    from: String,
    current: &str,
    e: AppError,
) -> OpenError {
    tracing::error!(
        "The Bridge {} could not open the data after backing it up: {}",
        current,
        e
    );
    if let Err(re) = restore(dir, &backup) {
        tracing::error!("Could not put the backup back: {}", re);
        return OpenError::Refused(Refusal::RestoreFailed {
            backup,
            detail: re.to_string(),
        });
    }
    tracing::info!("Put the data back from {}", backup.display());
    put_back_marker(dir, prior);
    // The folder now equals this start's backup. A copy an earlier start
    // could not put back stays (bounded): no start has opened the folder
    // since it was made, and it may be the only good copy.
    if let Err(e) = fs::remove_dir_all(&backup) {
        tracing::warn!("Could not remove the data backup just put back: {}", e);
    }
    OpenError::Refused(Refusal::Restored {
        from,
        current: current.to_string(),
        cause: cause_of(&e),
    })
}

/// The record as it was before this start, with the stores it describes.
/// Should that fail, no record at all: the next start then treats the
/// folder as older and backs it up again before migrating.
fn put_back_marker(dir: &Path, prior: PriorMarker) {
    let path = dir.join(MARKER);
    let put = match &prior {
        PriorMarker::Unchanged => return,
        PriorMarker::Held(bytes) => crate::security::fsperm::write_private_file(&path, bytes),
        PriorMarker::Absent => fs::remove_file(&path),
    };
    if let Err(e) = put {
        tracing::error!("Could not put the data version record back: {}", e);
        if let Err(e) = fs::remove_file(&path) {
            tracing::error!("Could not remove the data version record either: {}", e);
        }
    }
}

fn discard_backups(dir: &Path) {
    purge_backups(dir, "The data opened");
}

/// Delete every backup in `backups/` after `after` (a short description
/// for the log) removed something a copy would still hold. Normally there
/// is none (a successful start deletes them); this catches a leftover that
/// start could not delete. Failures are logged, never raised: the action
/// itself has happened.
pub fn purge_backups(dir: &Path, after: &str) {
    match delete_backups(dir) {
        Ok(0) => {}
        Ok(n) => tracing::info!("{}: removed {} data backup(s) no longer needed", after, n),
        Err(e) => tracing::warn!("{}: could not remove the data backups: {}", after, e),
    }
}

fn label(version: &str) -> String {
    version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// A new folder, owner-only from the start (0700 on Unix; on Windows the
/// parent's ACL is inherited).
fn create_private_dir(p: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new().mode(0o700).create(p)?;
        // Exactly 0700 whatever the umask (which can only narrow it).
        fs::set_permissions(p, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    fs::DirBuilder::new().create(p)?;
    Ok(())
}

/// A new, empty file, owner-only from its first byte (0600 on Unix).
fn create_private_file(p: &Path) -> io::Result<fs::File> {
    let mut o = fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(p)
}

/// `backups/` as a plain folder, or `None` when there is none yet. A link
/// or a file in its place is never followed or used.
fn backups_root(dir: &Path) -> io::Result<Option<PathBuf>> {
    let root = dir.join(BACKUPS_DIR);
    match fs::symlink_metadata(&root) {
        Ok(m) if m.file_type().is_dir() => Ok(Some(root)),
        Ok(_) => Err(io::Error::other(
            "the backups folder is a link or a file, not a folder",
        )),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// The plain folders directly in `root` (links are skipped, never
/// followed), with their names.
fn plain_folders(root: &Path) -> io::Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    for e in fs::read_dir(root)? {
        let e = e?;
        if e.file_type()?.is_dir() {
            out.push((e.file_name().to_string_lossy().into_owned(), e.path()));
        }
    }
    Ok(out)
}

/// Copy every store in `dir` into a new complete backup and return it.
pub fn backup(dir: &Path, from: &str, to: &str, now: DateTime<Utc>) -> io::Result<PathBuf> {
    backup_with(dir, from, to, now, &mut |_| Ok(()))
}

/// [`backup`] with a hook run before each file is copied (tests use it to
/// stop a backup half way, as a crash would).
pub fn backup_with(
    dir: &Path,
    from: &str,
    to: &str,
    now: DateTime<Utc>,
    before_each: &mut dyn FnMut(&str) -> io::Result<()>,
) -> io::Result<PathBuf> {
    let root = match backups_root(dir)? {
        Some(r) => r,
        None => {
            let r = dir.join(BACKUPS_DIR);
            create_private_dir(&r)?;
            r
        }
    };
    remove_partials(&root);
    let base = format!("{}-{}", label(from), now.format("%Y%m%dT%H%M%SZ"));
    let mut name = base.clone();
    let mut n = 1;
    while fs::symlink_metadata(root.join(&name)).is_ok() {
        n += 1;
        name = format!("{}-{}", base, n);
    }
    let partial = root.join(format!("{}{}", PARTIAL, name));
    create_private_dir(&partial)?;
    match fill(dir, &partial, from, to, now, before_each) {
        Ok(()) => {
            let done = root.join(&name);
            fs::rename(&partial, &done)?;
            crate::security::fsperm::sync_dir(&root);
            Ok(done)
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&partial);
            Err(e)
        }
    }
}

fn fill(
    dir: &Path,
    out: &Path,
    from: &str,
    to: &str,
    now: DateTime<Utc>,
    before_each: &mut dyn FnMut(&str) -> io::Result<()>,
) -> io::Result<()> {
    let mut files = Vec::new();
    for (name, kind) in STORES {
        let src = dir.join(name);
        if !src.exists() {
            continue;
        }
        before_each(name)?;
        let dst = out.join(name);
        match kind {
            Kind::Sqlite => vacuum_into(&src, &dst)?,
            Kind::DuckDb | Kind::File => copy_private(&src, &dst)?,
        }
        files.push(name.to_string());
        if *kind == Kind::DuckDb {
            for side in side_files(name, *kind) {
                let s = dir.join(&side);
                if s.exists() {
                    copy_private(&s, &out.join(&side))?;
                    files.push(side);
                }
            }
        }
    }
    let manifest = Manifest {
        from_version: from.to_string(),
        to_version: to.to_string(),
        created_at: now.to_rfc3339_opts(SecondsFormat::Millis, true),
        files,
    };
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
    crate::security::fsperm::write_private_file(&out.join(MANIFEST), &bytes)?;
    crate::security::fsperm::sync_dir(out);
    Ok(())
}

/// A consistent copy of a SQLite database, its WAL included, into a file
/// created owner-only first (`VACUUM INTO` fills an existing empty file and
/// would otherwise create it with the default permissions).
fn vacuum_into(src: &Path, dst: &Path) -> io::Result<()> {
    let to = dst
        .to_str()
        .ok_or_else(|| io::Error::other("the backup folder's path is not valid text"))?;
    drop(create_private_file(dst)?);
    let conn = rusqlite::Connection::open_with_flags(
        src,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| io::Error::other(format!("open {}: {}", src.display(), e)))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))
        .map_err(io::Error::other)?;
    conn.execute("VACUUM INTO ?1", [to])
        .map_err(|e| io::Error::other(format!("copy {}: {}", src.display(), e)))?;
    drop(conn);
    // Opened for writing: on Windows a flush needs a handle that may write
    // (a read-only one fails with "Access is denied").
    fs::OpenOptions::new().write(true).open(dst)?.sync_all()
}

/// Copy `src` into a new owner-only file `dst`, synced.
fn copy_private(src: &Path, dst: &Path) -> io::Result<()> {
    let mut from = fs::File::open(src)?;
    let mut to = create_private_file(dst)?;
    io::copy(&mut from, &mut to)?;
    to.sync_all()
}

/// Leftovers of a backup that never finished (a crash or power cut).
fn remove_partials(root: &Path) {
    let Ok(folders) = plain_folders(root) else {
        return;
    };
    for (name, path) in folders {
        if name.starts_with(PARTIAL) {
            if let Err(err) = fs::remove_dir_all(&path) {
                tracing::warn!("Could not remove an unfinished backup: {}", err);
            }
        }
    }
}

/// The complete backups in the folder, oldest first. Only plain folders
/// with a manifest count; links are never followed.
pub fn backups(dir: &Path) -> Vec<(PathBuf, Manifest)> {
    let mut all = Vec::new();
    let Ok(Some(root)) = backups_root(dir) else {
        return all;
    };
    let Ok(folders) = plain_folders(&root) else {
        return all;
    };
    for (name, path) in folders {
        if name.starts_with('.') {
            continue;
        }
        let manifest = path.join(MANIFEST);
        if !fs::symlink_metadata(&manifest).is_ok_and(|m| m.file_type().is_file()) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&manifest) else {
            continue;
        };
        if let Ok(m) = serde_json::from_str::<Manifest>(&text) {
            all.push((path, m));
        }
    }
    all.sort_by(|a, b| {
        (a.1.created_at.as_str(), a.0.as_path()).cmp(&(b.1.created_at.as_str(), b.0.as_path()))
    });
    all
}

/// Keep the newest `keep` complete backups; folders that are not backups,
/// and links, are left alone.
pub fn prune(dir: &Path, keep: usize) -> io::Result<()> {
    let all = backups(dir);
    let excess = all.len().saturating_sub(keep);
    for (path, _) in all.into_iter().take(excess) {
        fs::remove_dir_all(&path)?;
    }
    Ok(())
}

/// Delete every backup and unfinished backup in `backups/` (after a good
/// upgrade, after a restore, on account reset). Folders that are not
/// backups, and links, are left alone. Returns how many were removed.
pub fn delete_backups(dir: &Path) -> io::Result<usize> {
    let Some(root) = backups_root(dir)? else {
        return Ok(0);
    };
    remove_partials(&root);
    let mut removed = 0;
    for (path, _) in backups(dir) {
        fs::remove_dir_all(&path)?;
        removed += 1;
    }
    Ok(removed)
}

/// Put the stores in `dir` back from `backup`. Stores the backup does not
/// hold were created by the failed open and are removed; side files go
/// first, and each store is replaced by a rename of an owner-only copy, so
/// a crash leaves either the old file or the backup's, never half of one.
/// Files that are not stores (the broker-revoke marker) are not touched.
///
/// Private on purpose: the only restore is [`open_data`]'s, right after a
/// failed open, from the backup that same start made. No command, route or
/// later start may put an older copy back over newer state.
fn restore(dir: &Path, backup: &Path) -> io::Result<()> {
    let text = fs::read_to_string(backup.join(MANIFEST))?;
    let manifest: Manifest = serde_json::from_str(&text).map_err(io::Error::other)?;
    let held = |n: &str| manifest.files.iter().any(|f| f == n);
    for (name, kind) in STORES {
        restore_store(dir, backup, name, *kind, &held).map_err(|e| naming(name, e))?;
    }
    crate::security::fsperm::sync_dir(dir);
    Ok(())
}

/// Put one store back: its side files go first, then the backup's copy
/// replaces it by a rename (or, when the backup does not hold it, the store
/// the failed open created is removed).
fn restore_store(
    dir: &Path,
    backup: &Path,
    name: &str,
    kind: Kind,
    held: &dyn Fn(&str) -> bool,
) -> io::Result<()> {
    for side in side_files(name, kind) {
        retry_while_in_use(|| remove_if_present(&dir.join(&side)))?;
    }
    let live = dir.join(name);
    if !held(name) {
        return retry_while_in_use(|| remove_if_present(&live));
    }
    let staged = dir.join(format!("{}.restoring", name));
    remove_if_present(&staged)?;
    copy_private(&backup.join(name), &staged)?;
    retry_while_in_use(|| fs::rename(&staged, &live))?;
    for side in side_files(name, kind) {
        if held(&side) {
            copy_private(&backup.join(&side), &dir.join(&side))?;
        }
    }
    Ok(())
}

/// How long a restore waits for a store the failed open (or another
/// program, such as a virus scanner) still holds open.
const IN_USE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

/// Run `op` again while the file it needs is in use. On Windows a file that
/// is open cannot be replaced or removed ("used by another process"); a
/// failed open can leave a database connection closing on a pool thread
/// for a moment after it returned. Other errors, and every error elsewhere,
/// are returned at once.
fn retry_while_in_use(mut op: impl FnMut() -> io::Result<()>) -> io::Result<()> {
    let deadline = std::time::Instant::now() + IN_USE_WAIT;
    let mut pause = std::time::Duration::from_millis(50);
    loop {
        match op() {
            Err(e) if in_use(&e) && std::time::Instant::now() < deadline => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(std::time::Duration::from_secs(1));
            }
            other => return other,
        }
    }
}

/// A file another handle holds (Windows: sharing or lock violation).
fn in_use(e: &io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(32) | Some(33))
}

/// The error with the store it concerns, for the log.
fn naming(name: &str, e: io::Error) -> io::Error {
    io::Error::new(e.kind(), format!("{}: {}", name, e))
}

fn remove_if_present(p: &Path) -> io::Result<()> {
    match fs::remove_file(p) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// The running app's hold on its data folder: a second copy of OpenAlgo
/// started on the same folder is refused before it reads, backs up or
/// restores anything. Released when the process ends.
#[derive(Debug)]
pub struct DataFolderLock {
    _file: fs::File,
}

impl DataFolderLock {
    /// Take the lock, or `Err(Refusal::InUse)` when another copy holds it.
    /// A file system without locks is used unlocked (logged).
    pub fn acquire(dir: &Path) -> Result<Option<Self>, Refusal> {
        if let Err(e) = crate::security::fsperm::ensure_private_dir(dir) {
            tracing::warn!("Could not prepare the data folder for its lock: {}", e);
            return Ok(None);
        }
        let mut opts = fs::OpenOptions::new();
        opts.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file = match opts.open(dir.join(LOCK_FILE)) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!("Could not open the data folder lock: {}", e);
                return Ok(None);
            }
        };
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(fs::TryLockError::WouldBlock) => Err(Refusal::InUse),
            Err(fs::TryLockError::Error(e)) => {
                tracing::warn!("The data folder cannot be locked: {}", e);
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(min: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 15, 9, min, 0)
            .single()
            .unwrap()
    }

    fn sqlite_store(path: &Path, rows: &[&str]) {
        let c = rusqlite::Connection::open(path).unwrap();
        c.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS t (v TEXT);")
            .unwrap();
        for r in rows {
            c.execute("INSERT INTO t (v) VALUES (?1)", [*r]).unwrap();
        }
    }

    fn rows(path: &Path) -> Vec<String> {
        let c = rusqlite::Connection::open(path).unwrap();
        let mut s = c.prepare("SELECT v FROM t ORDER BY rowid").unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<String>>>()
            .unwrap()
    }

    fn duck_store(path: &Path, rows: &[&str]) {
        let c = duckdb::Connection::open(path).unwrap();
        c.execute_batch("CREATE TABLE IF NOT EXISTS h (v VARCHAR);")
            .unwrap();
        for r in rows {
            c.execute("INSERT INTO h VALUES (?)", [*r]).unwrap();
        }
        drop(c);
    }

    fn duck_rows(path: &Path) -> Vec<String> {
        let c = duckdb::Connection::open(path).unwrap();
        let mut s = c.prepare("SELECT v FROM h ORDER BY v").unwrap();
        let out = s
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<duckdb::Result<Vec<String>>>()
            .unwrap();
        drop(s);
        drop(c);
        out
    }

    /// A data folder as version `version` left it: the main, logs and
    /// Historify stores and a key file.
    fn folder(version: Option<&str>) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        sqlite_store(&d.path().join("openalgo.db"), &["account"]);
        sqlite_store(&d.path().join("logs.db"), &["log line"]);
        duck_store(&d.path().join("historify.duckdb"), &["candle"]);
        fs::write(d.path().join("vault.json"), b"{\"wrapped\":\"key\"}").unwrap();
        if let Some(v) = version {
            write_recorded(d.path(), v, at(0)).unwrap();
        }
        d
    }

    fn version_of(dir: &Path) -> Recorded {
        read_recorded(dir)
    }

    fn v(s: &str) -> Recorded {
        Recorded::Version(semver::Version::parse(s).unwrap())
    }

    fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
        let mut out: Vec<(String, Vec<u8>)> = fs::read_dir(dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .map(|e| {
                (
                    e.file_name().to_string_lossy().to_string(),
                    fs::read(e.path()).unwrap(),
                )
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn a_folder_written_by_a_newer_version_is_refused_and_left_untouched() {
        for (recorded, current) in [
            ("1.2.0", "1.1.0"),
            ("1.1.0-beta.1", "1.0.0"),
            ("1.1.0", "1.1.0-rc.1"),
        ] {
            let d = folder(Some(recorded));
            let before = snapshot(d.path());
            let got = open_data(d.path(), current, at(5), || -> crate::error::Result<()> {
                panic!("a newer folder must not be opened")
            });
            match got {
                Err(OpenError::Refused(r @ Refusal::Downgrade { .. })) => {
                    let m = r.message();
                    assert!(
                        m.contains(&format!("last opened by version {}", recorded)),
                        "{}",
                        m
                    );
                    assert!(m.contains("nothing was opened or changed"), "{}", m);
                }
                other => panic!("{} on {}: {:?}", recorded, current, other.err()),
            }
            assert_eq!(snapshot(d.path()), before, "{} on {}", recorded, current);
            assert!(!d.path().join(BACKUPS_DIR).exists());
        }
    }

    #[test]
    fn an_upgrade_backs_up_every_store_before_the_open_then_deletes_the_backup() {
        let d = folder(Some("1.0.0"));
        let dir = d.path().to_path_buf();
        let opened = open_data(&dir, "1.1.0", at(5), || {
            // The backup is complete before anything is opened.
            let all = backups(&dir);
            assert_eq!(all.len(), 1);
            let (b, m) = &all[0];
            assert_eq!(m.from_version, "1.0.0");
            assert_eq!(m.to_version, "1.1.0");
            assert_eq!(
                m.files,
                vec!["openalgo.db", "logs.db", "historify.duckdb", "vault.json"]
            );
            assert!(b
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("1.0.0-20261015T0905"));
            assert_eq!(rows(&b.join("openalgo.db")), vec!["account"]);
            assert_eq!(rows(&b.join("logs.db")), vec!["log line"]);
            assert_eq!(duck_rows(&b.join("historify.duckdb")), vec!["candle"]);
            assert_eq!(
                fs::read(b.join("vault.json")).unwrap(),
                b"{\"wrapped\":\"key\"}"
            );
            // NEW-REL-06: recorded before anything is opened or migrated.
            assert_eq!(version_of(&dir), v("1.1.0"));
            Ok("opened")
        });
        assert_eq!(opened.unwrap(), "opened");
        assert_eq!(version_of(&dir), v("1.1.0"));
        // A good upgrade keeps no copy of the older state.
        assert!(backups(&dir).is_empty());
        // The same version again makes no backup.
        open_data(&dir, "1.1.0", at(6), || {
            assert!(backups(&dir).is_empty());
            Ok(())
        })
        .unwrap();
    }

    /// Security review, finding 1: what the trader deletes while the app
    /// runs (forgotten broker keys, a revoked session, an old API key,
    /// password or TOTP secret) never survives in a backup. A successful
    /// open, upgrade or not, removes every backup and unfinished backup,
    /// leftovers of earlier starts included; the trader's own folders stay.
    #[test]
    fn no_backup_outlives_a_successful_open() {
        let d = folder(Some("1.1.0"));
        let dir = d.path().to_path_buf();
        // A leftover holding state that has since been deleted.
        let old = backup(&dir, "1.0.0", "1.1.0", at(1)).unwrap();
        sqlite_store(&old.join("openalgo.db"), &["revoked broker token"]);
        let partial = dir
            .join(BACKUPS_DIR)
            .join(".partial-1.0.0-20261015T090200Z");
        fs::create_dir_all(&partial).unwrap();
        fs::write(partial.join("openalgo.db"), b"half").unwrap();
        let mine = dir.join(BACKUPS_DIR).join("my-notes");
        fs::create_dir_all(&mine).unwrap();
        open_data(&dir, "1.1.0", at(5), || Ok(())).unwrap();
        assert!(!old.exists());
        assert!(!partial.exists());
        assert!(mine.exists());
        assert!(backups(&dir).is_empty());
        // The same after an upgrade.
        let old = backup(&dir, "1.0.0", "1.1.0", at(6)).unwrap();
        open_data(&dir, "1.2.0", at(7), || Ok(())).unwrap();
        assert!(!old.exists());
        assert!(backups(&dir).is_empty());
        // And account reset's purge.
        backup(&dir, "1.1.0", "1.2.0", at(8)).unwrap();
        assert_eq!(delete_backups(&dir).unwrap(), 1);
        assert!(backups(&dir).is_empty());
    }

    #[test]
    fn a_failed_migration_puts_every_store_back_from_that_starts_backup() {
        let d = folder(Some("1.0.0"));
        let dir = d.path().to_path_buf();
        // A pending broker revoke (SEC-05b) is not a store: a restore never
        // removes it.
        fs::write(dir.join("broker-revoke.pending"), b"pending").unwrap();
        // Security review, finding 2: an older backup (left by an earlier
        // start) is never what a restore puts back.
        let older = backup(&dir, "0.9.0", "1.0.0", at(1)).unwrap();
        sqlite_store(&older.join("openalgo.db"), &["old api key"]);
        let older_before = snapshot(&older);
        let got: Result<(), OpenError> = open_data(&dir, "1.1.0", at(5), || {
            // A migration that half ran: new rows and schema, a new store,
            // a DuckDB change and a WAL left behind, then the failure.
            sqlite_store(&dir.join("openalgo.db"), &["migrated"]);
            rusqlite::Connection::open(dir.join("openalgo.db"))
                .unwrap()
                .execute_batch("CREATE TABLE new_feature (x INTEGER);")
                .unwrap();
            sqlite_store(&dir.join("logs.db"), &["migrated"]);
            sqlite_store(&dir.join("sandbox.db"), &["created by the new version"]);
            duck_store(&dir.join("historify.duckdb"), &["migrated"]);
            fs::write(dir.join("vault.json"), b"rewritten").unwrap();
            fs::write(dir.join("openalgo.db-wal"), b"stale").unwrap();
            Err(AppError::Database(rusqlite::Error::InvalidQuery))
        });
        let refusal = match got {
            Err(OpenError::Refused(r)) => r,
            other => panic!("expected the restore, got {:?}", other.err()),
        };
        assert_eq!(
            refusal,
            Refusal::Restored {
                from: "1.0.0".into(),
                current: "1.1.0".into(),
                cause: "its databases could not be updated to this version".into(),
            }
        );
        let m = refusal.message();
        assert!(
            m.contains("put back exactly as it was before this version opened it"),
            "{}",
            m
        );
        assert!(m.contains("install version 1.0.0"), "{}", m);
        assert!(!m.contains(BACKUPS_DIR), "no copy is offered: {}", m);
        // Every store is as it was at this start, not as the older backup.
        assert_eq!(rows(&dir.join("openalgo.db")), vec!["account"]);
        let c = rusqlite::Connection::open(dir.join("openalgo.db")).unwrap();
        let new_table: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'new_feature'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(new_table, 0);
        drop(c);
        assert_eq!(rows(&dir.join("logs.db")), vec!["log line"]);
        assert!(
            !dir.join("sandbox.db").exists(),
            "a store the old version did not have is removed"
        );
        assert_eq!(duck_rows(&dir.join("historify.duckdb")), vec!["candle"]);
        assert_eq!(
            fs::read(dir.join("vault.json")).unwrap(),
            b"{\"wrapped\":\"key\"}"
        );
        assert_eq!(
            fs::read(dir.join("broker-revoke.pending")).unwrap(),
            b"pending"
        );
        // The old version still owns the folder. This start's backup is
        // gone (the folder equals it); the older leftover was neither used
        // nor changed, and the next successful open deletes it.
        assert_eq!(version_of(&dir), v("1.0.0"));
        let left: Vec<PathBuf> = backups(&dir).into_iter().map(|(p, _)| p).collect();
        assert_eq!(left, vec![older.clone()]);
        assert_eq!(snapshot(&older), older_before);
        open_data(&dir, "1.0.0", at(9), || Ok(())).unwrap();
        assert!(backups(&dir).is_empty());
    }

    /// NEW-REL-06: once a newer version may have touched the data, an
    /// older one is refused, at any point: here an older copy starts while
    /// the newer one is migrating (as after a crash mid-migration). Before,
    /// the record was written only after a successful open, so the older
    /// copy found its own version and opened half-migrated data.
    #[test]
    fn an_older_version_is_refused_once_a_newer_one_may_have_migrated() {
        let d = folder(Some("1.0.0"));
        let dir = d.path().to_path_buf();
        open_data(&dir, "1.1.0", at(5), || {
            sqlite_store(&dir.join("openalgo.db"), &["migrated"]);
            let older: Result<(), OpenError> = open_data(&dir, "1.0.0", at(6), || {
                panic!("the older version opened data a newer one is migrating")
            });
            assert!(
                matches!(
                    older,
                    Err(OpenError::Refused(Refusal::Downgrade { ref recorded, .. })) if recorded == "1.1.0"
                ),
                "{:?}",
                older.err()
            );
            Ok(())
        })
        .unwrap();
        assert_eq!(version_of(&dir), v("1.1.0"));
    }

    /// NEW-REL-06: a record that cannot be written stops the start before
    /// anything is opened or migrated, visibly, and leaves the folder as it
    /// was: the old record, the stores, no backup. The older version still
    /// opens it. Before, the failure was only logged after the upgraded open
    /// and the app ran on with the old record and no backup.
    #[test]
    fn a_record_that_cannot_be_written_stops_the_start_before_any_change() {
        let d = folder(Some("1.0.0"));
        let dir = d.path().to_path_buf();
        // The record is written to a temporary file and renamed into place;
        // a folder where that file goes makes the write fail.
        let tmp = dir.join(MARKER).with_extension("tmp");
        fs::create_dir_all(tmp.join("blocked")).unwrap();
        let before = snapshot(&dir);
        let got: Result<(), OpenError> = open_data(&dir, "1.1.0", at(5), || {
            panic!("the stores were opened though the version was not recorded")
        });
        let refusal = match got {
            Err(OpenError::Refused(r @ Refusal::NotRecorded { .. })) => r,
            other => panic!("expected NotRecorded, got {:?}", other.err()),
        };
        let m = refusal.message();
        assert!(m.contains("data-version.json"), "{}", m);
        assert!(m.contains("nothing was opened or changed"), "{}", m);
        assert_eq!(snapshot(&dir), before);
        assert_eq!(version_of(&dir), v("1.0.0"));
        assert!(
            backups(&dir).is_empty(),
            "this start's unused backup is deleted"
        );
        assert_eq!(rows(&dir.join("openalgo.db")), vec!["account"]);
        // The version that owns the data still opens it.
        open_data(&dir, "1.0.0", at(6), || Ok(())).unwrap();
    }

    /// A failed upgraded open of a folder that recorded no version puts the
    /// folder back without a record, as it was: the next start backs it up
    /// again before migrating.
    #[test]
    fn a_failed_open_of_an_unrecorded_folder_leaves_it_unrecorded() {
        let d = folder(None);
        let dir = d.path().to_path_buf();
        let got: Result<(), OpenError> = open_data(&dir, "1.1.0", at(5), || {
            assert_eq!(version_of(&dir), v("1.1.0"));
            Err(AppError::Database(rusqlite::Error::InvalidQuery))
        });
        assert!(matches!(
            got,
            Err(OpenError::Refused(Refusal::Restored { .. }))
        ));
        assert_eq!(version_of(&dir), Recorded::Nothing);
        open_data(&dir, "1.1.0", at(6), || {
            assert_eq!(backups(&dir).len(), 1, "backed up again before migrating");
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_failed_open_with_no_upgrade_is_passed_on_untouched() {
        let d = folder(Some("1.1.0"));
        let got: Result<(), OpenError> = open_data(d.path(), "1.1.0", at(5), || {
            Err(AppError::KeychainUnavailable)
        });
        assert!(matches!(
            got,
            Err(OpenError::Open(AppError::KeychainUnavailable))
        ));
        assert!(backups(d.path()).is_empty());
    }

    /// A crash mid-backup: the live stores were only read, the half-made
    /// copy is never used, and a later start clears it.
    #[test]
    fn a_crash_mid_backup_is_safe() {
        let d = folder(Some("1.0.0"));
        let dir = d.path().to_path_buf();
        let before = snapshot(&dir);
        // Stopped after the first store, as a crash would stop it.
        let mut seen = 0;
        let got = backup_with(&dir, "1.0.0", "1.1.0", at(5), &mut |_| {
            seen += 1;
            if seen == 2 {
                Err(io::Error::other("power cut"))
            } else {
                Ok(())
            }
        });
        assert!(got.is_err());
        assert_eq!(snapshot(&dir), before, "the live stores are unchanged");
        assert!(backups(&dir).is_empty(), "no backup counts as made");
        // What a real crash leaves behind: a half folder, even with a
        // manifest in it, is never taken for a backup.
        let leftover = dir
            .join(BACKUPS_DIR)
            .join(".partial-1.0.0-20261015T090500Z");
        fs::create_dir_all(&leftover).unwrap();
        fs::write(leftover.join("openalgo.db"), b"half a copy").unwrap();
        fs::write(
            leftover.join(MANIFEST),
            serde_json::to_vec(&Manifest {
                from_version: "1.0.0".into(),
                to_version: "1.1.0".into(),
                created_at: "2026-10-15T09:05:00.000Z".into(),
                files: vec!["openalgo.db".into()],
            })
            .unwrap(),
        )
        .unwrap();
        assert!(backups(&dir).is_empty());
        // The next start makes a complete backup, clears the leftover, and
        // once open keeps nothing.
        open_data(&dir, "1.1.0", at(6), || {
            assert!(!leftover.exists());
            let all = backups(&dir);
            assert_eq!(all.len(), 1);
            assert_eq!(rows(&all[0].0.join("openalgo.db")), vec!["account"]);
            Ok(())
        })
        .unwrap();
        assert!(backups(&dir).is_empty());
    }

    #[test]
    fn a_backup_that_cannot_be_made_stops_the_upgrade_before_any_change() {
        let d = folder(Some("1.0.0"));
        // Something in the way of the backups folder.
        fs::write(d.path().join(BACKUPS_DIR), b"not a folder").unwrap();
        let before = snapshot(d.path());
        let got = open_data(d.path(), "1.1.0", at(5), || -> crate::error::Result<()> {
            panic!("nothing may be opened without a backup")
        });
        match got {
            Err(OpenError::Refused(r @ Refusal::BackupFailed { .. })) => {
                assert!(r.message().contains("nothing was changed"))
            }
            other => panic!("{:?}", other.err()),
        }
        assert_eq!(snapshot(d.path()), before);
        assert_eq!(version_of(d.path()), v("1.0.0"));
    }

    #[test]
    fn leftover_backups_stay_bounded() {
        let d = folder(None);
        let dir = d.path();
        // A folder of the trader's own is never removed.
        fs::create_dir_all(dir.join(BACKUPS_DIR).join("my-notes")).unwrap();
        for (i, ver) in ["0.9.0", "1.0.0", "1.0.1", "1.0.2", "1.0.3", "1.1.0"]
            .iter()
            .enumerate()
        {
            backup(dir, ver, "x", at(i as u32)).unwrap();
            prune(dir, KEEP_BACKUPS).unwrap();
            assert!(backups(dir).len() <= KEEP_BACKUPS);
        }
        let kept: Vec<String> = backups(dir)
            .into_iter()
            .map(|(_, m)| m.from_version)
            .collect();
        assert_eq!(kept, vec!["1.0.2", "1.0.3", "1.1.0"]);
        assert!(dir.join(BACKUPS_DIR).join("my-notes").exists());
        // Failed upgrades in a row (each puts its backup back, and a
        // backup that could not be put back is the only kind kept) never
        // leave more than the bound.
        let mut current = semver::Version::parse("2.0.0").unwrap();
        for i in 0..5 {
            current.minor += 1;
            let got = open_data(
                dir,
                &current.to_string(),
                at(10 + i),
                || -> crate::error::Result<()> {
                    Err(AppError::Internal("migration failed".into()))
                },
            );
            assert!(
                matches!(got, Err(OpenError::Refused(Refusal::Restored { .. }))),
                "{:?}",
                got.err()
            );
            assert!(backups(dir).len() <= KEEP_BACKUPS);
        }
        assert!(dir.join(BACKUPS_DIR).join("my-notes").exists());
    }

    #[test]
    fn a_folder_without_a_recorded_version_is_backed_up_once() {
        let d = folder(None);
        let dir = d.path().to_path_buf();
        open_data(&dir, "1.1.0", at(5), || {
            let all = backups(&dir);
            assert_eq!(all.len(), 1);
            assert_eq!(all[0].1.from_version, "unknown");
            Ok(())
        })
        .unwrap();
        assert_eq!(version_of(&dir), v("1.1.0"));
        open_data(&dir, "1.1.0", at(6), || {
            assert!(
                backups(&dir).is_empty(),
                "the same version backs up nothing"
            );
            Ok(())
        })
        .unwrap();
        // An unreadable record is never taken as a newer version.
        fs::write(dir.join(MARKER), b"{not json").unwrap();
        assert_eq!(version_of(&dir), Recorded::Unreadable);
        open_data(&dir, "1.1.0", at(7), || {
            assert_eq!(backups(&dir).len(), 1);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_new_folder_opens_without_a_backup() {
        let d = tempfile::tempdir().unwrap();
        open_data(d.path(), "1.1.0", at(5), || Ok(())).unwrap();
        assert!(!d.path().join(BACKUPS_DIR).exists());
        assert_eq!(version_of(d.path()), v("1.1.0"));
    }

    #[test]
    fn a_second_copy_of_the_app_is_refused_while_the_first_holds_the_folder() {
        let d = tempfile::tempdir().unwrap();
        let first = DataFolderLock::acquire(d.path()).unwrap();
        assert!(first.is_some());
        assert_eq!(
            DataFolderLock::acquire(d.path()).unwrap_err(),
            Refusal::InUse
        );
        assert!(Refusal::InUse.message().contains("already running"));
        drop(first);
        assert!(DataFolderLock::acquire(d.path()).unwrap().is_some());
    }

    #[cfg(unix)]
    fn mode(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::symlink_metadata(p).unwrap().permissions().mode() & 0o777
    }

    /// Security review, finding 3: every backup folder and file, the
    /// staging files of a restore and the folder lock are owner-only from
    /// the moment they exist; nothing is created readable and narrowed
    /// later. Checked on each copy step as well as on the result.
    #[cfg(unix)]
    #[test]
    fn backups_are_owner_only_from_the_first_byte() {
        let d = folder(Some("1.0.0"));
        let dir = d.path().to_path_buf();
        let scratch = tempfile::tempdir().unwrap();
        // The copy steps themselves, without any later narrowing.
        vacuum_into(&dir.join("openalgo.db"), &scratch.path().join("a.db")).unwrap();
        assert_eq!(mode(&scratch.path().join("a.db")), 0o600);
        copy_private(
            &dir.join("historify.duckdb"),
            &scratch.path().join("h.duckdb"),
        )
        .unwrap();
        assert_eq!(mode(&scratch.path().join("h.duckdb")), 0o600);
        create_private_dir(&scratch.path().join("f")).unwrap();
        assert_eq!(mode(&scratch.path().join("f")), 0o700);
        // A whole backup, and the files a restore writes.
        let got = open_data(&dir, "1.1.0", at(5), || -> crate::error::Result<()> {
            let root = dir.join(BACKUPS_DIR);
            assert_eq!(mode(&root), 0o700);
            let (b, _) = &backups(&dir)[0];
            assert_eq!(mode(b), 0o700);
            for e in fs::read_dir(b).unwrap().flatten() {
                assert_eq!(mode(&e.path()), 0o600, "{}", e.path().display());
            }
            Err(AppError::Internal("migration failed".into()))
        });
        assert!(
            matches!(got, Err(OpenError::Refused(Refusal::Restored { .. }))),
            "{:?}",
            got.err()
        );
        for name in ["openalgo.db", "logs.db", "historify.duckdb", "vault.json"] {
            assert_eq!(mode(&dir.join(name)), 0o600, "{}", name);
        }
        let _lock = DataFolderLock::acquire(&dir).unwrap();
        assert_eq!(mode(&dir.join(LOCK_FILE)), 0o600);
    }

    /// The accounts an access entry names on `p` (`icacls`).
    #[cfg(windows)]
    fn who_has_access(p: &Path) -> std::collections::BTreeSet<String> {
        let out = std::process::Command::new("icacls")
            .arg(p)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).replace(&*p.to_string_lossy(), "");
        text.lines()
            .filter_map(|l| l.split_once(":("))
            .map(|(who, _)| who.trim().to_string())
            .collect()
    }

    /// Security review, finding 3 on Windows: backup folders and files grant
    /// no account that the data folder does not (they are created with the
    /// default security of their folder, and nothing adds an entry), so they
    /// are as private as the data folder.
    #[cfg(windows)]
    #[test]
    fn backups_grant_no_one_the_data_folder_does_not_on_windows() {
        let d = folder(Some("1.0.0"));
        let dir = d.path().to_path_buf();
        let folder_has = who_has_access(&dir);
        assert!(!folder_has.is_empty());
        let got = open_data(&dir, "1.1.0", at(5), || -> crate::error::Result<()> {
            let (b, _) = &backups(&dir)[0];
            let mut paths = vec![dir.join(BACKUPS_DIR), b.clone()];
            paths.extend(fs::read_dir(b).unwrap().flatten().map(|e| e.path()));
            for p in paths {
                let has = who_has_access(&p);
                assert!(!has.is_empty(), "{}", p.display());
                for who in has {
                    assert!(
                        folder_has.contains(&who),
                        "{} grants {}, which the data folder does not",
                        p.display(),
                        who
                    );
                }
            }
            Err(AppError::Internal("migration failed".into()))
        });
        // The backup was made and checked (a backup that could not be made
        // would end in an error too).
        assert!(
            matches!(got, Err(OpenError::Refused(Refusal::Restored { .. }))),
            "{:?}",
            got.err()
        );
    }

    /// Security review, finding 4: clearing and pruning backups never
    /// follows a link out of `backups/`, whether `backups` itself is a link
    /// or a link sits inside it.
    #[cfg(unix)]
    #[test]
    fn backup_cleanup_never_follows_a_link_out_of_the_folder() {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().unwrap();
        let victim = outside.path().join("1.0.0-20261015T090000Z");
        fs::create_dir_all(&victim).unwrap();
        fs::write(
            victim.join(MANIFEST),
            serde_json::to_vec(&Manifest {
                from_version: "1.0.0".into(),
                to_version: "1.1.0".into(),
                created_at: "2026-10-15T09:00:00.000Z".into(),
                files: vec![],
            })
            .unwrap(),
        )
        .unwrap();
        fs::write(victim.join("precious"), b"not ours").unwrap();
        let half = outside.path().join(".partial-1.0.0-20261015T090100Z");
        fs::create_dir_all(&half).unwrap();
        fs::write(half.join("precious"), b"not ours").unwrap();

        // `backups` is a link to another folder.
        let d = folder(Some("1.1.0"));
        symlink(outside.path(), d.path().join(BACKUPS_DIR)).unwrap();
        open_data(d.path(), "1.1.0", at(5), || Ok(())).unwrap();
        assert!(delete_backups(d.path()).is_err());
        prune(d.path(), 0).unwrap();
        assert!(backups(d.path()).is_empty());
        // An upgrade will not back up through it either.
        let got = open_data(d.path(), "1.2.0", at(6), || -> crate::error::Result<()> {
            panic!("no open without a backup")
        });
        assert!(matches!(
            got,
            Err(OpenError::Refused(Refusal::BackupFailed { .. }))
        ));
        assert!(victim.join("precious").exists());
        assert!(half.join("precious").exists());

        // A link inside a real `backups/`, named like a backup.
        let d = folder(Some("1.1.0"));
        let root = d.path().join(BACKUPS_DIR);
        fs::create_dir_all(&root).unwrap();
        symlink(&victim, root.join("1.0.0-20261015T090000Z")).unwrap();
        symlink(&half, root.join(".partial-1.0.0-20261015T090100Z")).unwrap();
        open_data(d.path(), "1.1.0", at(7), || Ok(())).unwrap();
        prune(d.path(), 0).unwrap();
        delete_backups(d.path()).unwrap();
        assert!(victim.join("precious").exists());
        assert!(half.join("precious").exists());
    }
}
