//! The Bridge 1.0.5: a permanent public webhook URL, so TradingView (and
//! other alert senders) can reach this computer.
//!
//! Two providers, both run as a child process the app supervises:
//! * ngrok, with the free dev domain every ngrok account gets
//!   (`<name>.ngrok-free.dev` / `.ngrok-free.app`, permanent);
//! * Cloudflare Tunnel (`cloudflared tunnel run`), with a named tunnel's
//!   token, on the trader's own domain.
//!
//! Neither binary is bundled: ngrok's terms only allow redistributing the
//! agent when the distributor owns the account, so both are downloaded from
//! their official sources on first use into the app's data folder.
//!
//! The token is encrypted in openalgo.db like broker keys (AES-GCM, AAD bound
//! to the row), handed to the child through its environment
//! (`NGROK_AUTHTOKEN` / `TUNNEL_TOKEN`) and never logged or returned.
//! While the tunnel is on, callers through it reach only the webhook routes
//! ([`tunnel_path_allowed`]); orders still need the API key.

use crate::error::{AppError, Result};
use crate::security::crypto::Aad;
use crate::state::AppState;
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Ngrok,
    Cloudflare,
}

impl Provider {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "ngrok" => Some(Self::Ngrok),
            "cloudflare" => Some(Self::Cloudflare),
            _ => None,
        }
    }
    fn as_str(self) -> &'static str {
        match self {
            Self::Ngrok => "ngrok",
            Self::Cloudflare => "cloudflare",
        }
    }
}

/// What is saved (the token stays encrypted in the row).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TunnelSettings {
    pub provider: Provider,
    /// `abc.ngrok-free.dev` or `alerts.example.com` (no scheme).
    pub hostname: String,
    pub enabled: bool,
    pub webhook_only: bool,
}

impl TunnelSettings {
    pub fn public_url(&self) -> String {
        format!("https://{}", self.hostname)
    }
}

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tunnel_settings (
            id INTEGER PRIMARY KEY CHECK (id = 1),
            provider TEXT NOT NULL,
            hostname TEXT NOT NULL,
            token_ct TEXT NOT NULL,
            token_nonce TEXT NOT NULL,
            enabled INTEGER NOT NULL DEFAULT 1,
            webhook_only INTEGER NOT NULL DEFAULT 1,
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        )",
    )?;
    Ok(())
}

fn aad(provider: Provider) -> Aad {
    Aad::new("tunnel_settings", "token", provider.as_str())
}

/// The hostname the trader typed, cleaned: scheme, path and spaces dropped,
/// lower case. `None` when it is not a plausible public host name.
pub fn clean_hostname(input: &str) -> Option<String> {
    let s = input.trim().to_ascii_lowercase();
    let s = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .unwrap_or(&s);
    let host = s.split(['/', '?', '#']).next().unwrap_or("").trim_end_matches('.');
    let ok = host.contains('.')
        && host.len() <= 253
        && host
            .split('.')
            .all(|l| !l.is_empty() && l.len() <= 63 && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') && !l.starts_with('-') && !l.ends_with('-'))
        && !matches!(host, "localhost")
        && host.parse::<std::net::IpAddr>().is_err();
    ok.then(|| host.to_string())
}

/// A token as pasted: trimmed; refused when empty, multi-line or absurd.
/// Cloudflare's dashboard shows the token inside a full command
/// (`cloudflared service install <token>`); the token is the last word.
pub fn clean_token(provider: Provider, input: &str) -> Option<String> {
    let t = input.trim();
    let t = match provider {
        Provider::Cloudflare => t.split_whitespace().last().unwrap_or(""),
        Provider::Ngrok => t
            .strip_prefix("ngrok config add-authtoken ")
            .unwrap_or(t)
            .trim(),
    };
    (t.len() >= 20 && t.len() <= 4096 && !t.chars().any(char::is_whitespace)).then(|| t.to_string())
}

pub fn load(conn: &Connection) -> Result<Option<TunnelSettings>> {
    Ok(conn
        .query_row(
            "SELECT provider, hostname, enabled, webhook_only FROM tunnel_settings WHERE id = 1",
            [],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)? != 0,
                    r.get::<_, i64>(3)? != 0,
                ))
            },
        )
        .optional()?
        .and_then(|(p, h, e, w)| {
            Provider::parse(&p).map(|provider| TunnelSettings {
                provider,
                hostname: h,
                enabled: e,
                webhook_only: w,
            })
        }))
}

fn load_token(ctx: &AppState, provider: Provider) -> Result<crate::security::Secret> {
    let conn = ctx.sqlite.conn()?;
    let (ct, nonce): (String, String) = conn.query_row(
        "SELECT token_ct, token_nonce FROM tunnel_settings WHERE id = 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    ctx.security.decrypt(&ct, &nonce, &aad(provider))
}

/// Save (or replace) the setup. The token is encrypted before it is stored.
pub fn save(ctx: &AppState, provider: Provider, hostname: &str, token: &str) -> Result<TunnelSettings> {
    let (ct, nonce) = ctx.security.encrypt(token, &aad(provider))?;
    let conn = ctx.sqlite.conn()?;
    conn.execute(
        "INSERT INTO tunnel_settings (id, provider, hostname, token_ct, token_nonce, enabled, webhook_only)
         VALUES (1, ?1, ?2, ?3, ?4, 1, 1)
         ON CONFLICT(id) DO UPDATE SET provider = ?1, hostname = ?2, token_ct = ?3,
             token_nonce = ?4, enabled = 1, updated_at = datetime('now')",
        params![provider.as_str(), hostname, ct, nonce],
    )?;
    Ok(load(&conn)?.expect("just saved"))
}

pub fn set_enabled(ctx: &AppState, on: bool) -> Result<()> {
    ctx.sqlite.conn()?.execute(
        "UPDATE tunnel_settings SET enabled = ?1, updated_at = datetime('now') WHERE id = 1",
        params![on as i64],
    )?;
    Ok(())
}

pub fn remove(ctx: &AppState) -> Result<()> {
    ctx.sqlite.conn()?.execute("DELETE FROM tunnel_settings WHERE id = 1", [])?;
    Ok(())
}

/// Whether a caller arriving through the tunnel may reach `path` while
/// "webhook only" is on: the order and alert endpoints (API key checked
/// there), the strategy/Chartink webhooks (their own secret ids), and broker
/// sign-in callbacks. The app's pages, sign-in and settings stay local.
pub fn tunnel_path_allowed(path: &str) -> bool {
    path.starts_with("/api/v1/")
        || path.starts_with("/strategy/webhook/")
        || path.starts_with("/chartink/webhook/")
        || (path.ends_with("/callback") && path.matches('/').count() == 2)
}

// ---------------------------------------------------------------- binaries

/// Official download for this platform: (url, archive kind, binary name).
pub fn download_source(provider: Provider) -> Option<(&'static str, Archive, &'static str)> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    Some(match (provider, os, arch) {
        (Provider::Ngrok, "windows", "x86_64") => (
            "https://bin.equinox.io/c/bNyj1mQVY4c/ngrok-v3-stable-windows-amd64.zip",
            Archive::Zip,
            "ngrok.exe",
        ),
        (Provider::Ngrok, "macos", "aarch64") => (
            "https://bin.equinox.io/c/bNyj1mQVY4c/ngrok-v3-stable-darwin-arm64.zip",
            Archive::Zip,
            "ngrok",
        ),
        (Provider::Ngrok, "macos", "x86_64") => (
            "https://bin.equinox.io/c/bNyj1mQVY4c/ngrok-v3-stable-darwin-amd64.zip",
            Archive::Zip,
            "ngrok",
        ),
        (Provider::Ngrok, "linux", "x86_64") => (
            "https://bin.equinox.io/c/bNyj1mQVY4c/ngrok-v3-stable-linux-amd64.tgz",
            Archive::Tgz,
            "ngrok",
        ),
        (Provider::Cloudflare, "windows", "x86_64") => (
            "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe",
            Archive::Plain,
            "cloudflared.exe",
        ),
        (Provider::Cloudflare, "macos", "aarch64") => (
            "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-arm64.tgz",
            Archive::Tgz,
            "cloudflared",
        ),
        (Provider::Cloudflare, "macos", "x86_64") => (
            "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-amd64.tgz",
            Archive::Tgz,
            "cloudflared",
        ),
        (Provider::Cloudflare, "linux", "x86_64") => (
            "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64",
            Archive::Plain,
            "cloudflared",
        ),
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Archive {
    Zip,
    Tgz,
    Plain,
}

fn tools_dir(ctx: &AppState) -> PathBuf {
    ctx.data_dir.join("tools")
}

/// The binary, downloaded once from the official source.
pub async fn ensure_binary(ctx: &AppState, provider: Provider) -> Result<PathBuf> {
    let (url, kind, name) = download_source(provider).ok_or_else(|| {
        AppError::Validation("This tunnel is not available on this computer's system.".into())
    })?;
    let dir = tools_dir(ctx);
    let path = dir.join(name);
    if path.exists() {
        return Ok(path);
    }
    std::fs::create_dir_all(&dir)?;
    tracing::info!("Downloading {} from its official source", name);
    let bytes = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| AppError::Internal(e.to_string()))?
        .get(url)
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| AppError::Internal(format!("Could not download {}: {}", name, e)))?
        .bytes()
        .await
        .map_err(|e| AppError::Internal(format!("Could not download {}: {}", name, e)))?;
    let tmp = dir.join(format!("{}.part", name));
    extract(&bytes, kind, name, &tmp)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

pub fn extract(bytes: &[u8], kind: Archive, name: &str, out: &Path) -> Result<()> {
    match kind {
        Archive::Plain => std::fs::write(out, bytes)?,
        Archive::Zip => {
            let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes))
                .map_err(|e| AppError::Internal(format!("Damaged download: {}", e)))?;
            let mut f = z
                .by_name(name)
                .map_err(|e| AppError::Internal(format!("Damaged download: {}", e)))?;
            let mut w = std::fs::File::create(out)?;
            std::io::copy(&mut f, &mut w)?;
        }
        Archive::Tgz => {
            let gz = flate2::read::GzDecoder::new(bytes);
            let mut found = false;
            let mut ar = TarReader::new(gz);
            while let Some((path, data)) = ar.next_file()? {
                if Path::new(&path).file_name().and_then(|n| n.to_str()) == Some(name) {
                    std::fs::write(out, data)?;
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(AppError::Internal(format!("{} missing from the download", name)));
            }
        }
    }
    Ok(())
}

/// Minimal ustar reader (regular files only): enough for the single-binary
/// archives ngrok and cloudflared publish, without another dependency.
struct TarReader<R: std::io::Read> {
    r: R,
}

impl<R: std::io::Read> TarReader<R> {
    fn new(r: R) -> Self {
        Self { r }
    }
    fn next_file(&mut self) -> Result<Option<(String, Vec<u8>)>> {
        loop {
            let mut h = [0u8; 512];
            if read_full(&mut self.r, &mut h)? < 512 || h.iter().all(|b| *b == 0) {
                return Ok(None);
            }
            let name = cstr(&h[0..100]);
            let prefix = cstr(&h[345..500]);
            let size = usize::from_str_radix(cstr(&h[124..136]).trim(), 8).unwrap_or(0);
            let typ = h[156];
            let mut data = vec![0u8; size];
            read_full(&mut self.r, &mut data)?;
            let pad = (512 - size % 512) % 512;
            let mut skip = vec![0u8; pad];
            read_full(&mut self.r, &mut skip)?;
            if typ == b'0' || typ == 0 {
                let full = if prefix.is_empty() { name } else { format!("{}/{}", prefix, name) };
                return Ok(Some((full, data)));
            }
        }
    }
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|c| *c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).to_string()
}

fn read_full<R: std::io::Read>(r: &mut R, buf: &mut [u8]) -> Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
}

// ---------------------------------------------------------------- process

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RunState {
    Off,
    Downloading,
    Starting,
    Running,
    Failed { message: String },
}

struct Runner {
    child: Option<tokio::process::Child>,
    state: RunState,
    for_settings: Option<TunnelSettings>,
}

fn runner() -> &'static Mutex<Runner> {
    static R: OnceLock<Mutex<Runner>> = OnceLock::new();
    R.get_or_init(|| {
        Mutex::new(Runner {
            child: None,
            state: RunState::Off,
            for_settings: None,
        })
    })
}

/// Whether the running tunnel restricts tunnel callers to webhook routes.
pub fn webhook_only_active() -> bool {
    runner()
        .lock()
        .for_settings
        .as_ref()
        .is_some_and(|s| s.webhook_only)
}

pub fn run_state() -> RunState {
    runner().lock().state.clone()
}

fn set_state(s: RunState) {
    runner().lock().state = s;
}

/// The child's arguments (never the token; that goes in the environment).
pub fn command_args(s: &TunnelSettings, http_port: u16) -> (Vec<String>, &'static str) {
    match s.provider {
        Provider::Ngrok => (
            vec![
                "http".into(),
                format!("http://127.0.0.1:{}", http_port),
                "--url".into(),
                s.public_url(),
                "--log".into(),
                "stdout".into(),
                "--log-format".into(),
                "logfmt".into(),
            ],
            "NGROK_AUTHTOKEN",
        ),
        Provider::Cloudflare => (
            vec!["tunnel".into(), "--no-autoupdate".into(), "run".into()],
            "TUNNEL_TOKEN",
        ),
    }
}

/// Lines worth showing from the child's log (errors), with anything that
/// looks like a credential removed.
pub fn redact_line(line: &str) -> String {
    line.split_whitespace()
        .map(|w| {
            let lw = w.to_ascii_lowercase();
            if lw.contains("token") || lw.contains("authtoken") || (w.len() >= 32 && !w.contains('/') && !w.contains(':')) {
                "[redacted]"
            } else {
                w
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub async fn stop() {
    let child = {
        let mut r = runner().lock();
        r.state = RunState::Off;
        r.for_settings = None;
        r.child.take()
    };
    if let Some(mut c) = child {
        let _ = c.kill().await;
    }
}

/// Start (or restart) the tunnel for the saved settings, then save the
/// public URL as the Host Server URL so webhook URLs use it.
pub async fn start(ctx: &Arc<AppState>) -> Result<RunState> {
    let s = {
        let conn = ctx.sqlite.conn()?;
        load(&conn)?
    }
    .ok_or_else(|| AppError::Validation("Set up the webhook URL first.".into()))?;
    if !ctx.security.is_unlocked() {
        return Err(AppError::Auth("Sign in to The Bridge first.".into()));
    }
    stop().await;
    set_state(RunState::Downloading);
    let bin = match ensure_binary(ctx, s.provider).await {
        Ok(b) => b,
        Err(e) => {
            set_state(RunState::Failed { message: e.to_string() });
            return Err(e);
        }
    };
    set_state(RunState::Starting);
    let token = load_token(ctx, s.provider)?;
    let (args, env) = command_args(&s, ctx.listening_port());
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(&args)
        .env(env, token.expose())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().map_err(|e| {
        let m = format!("Could not start the tunnel: {}", e);
        set_state(RunState::Failed { message: m.clone() });
        AppError::Internal(m)
    })?;
    for pipe in [
        child.stdout.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        child.stderr.take().map(|p| Box::new(p) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
    ]
    .into_iter()
    .flatten()
    {
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(pipe).lines();
            while let Ok(Some(l)) = lines.next_line().await {
                let low = l.to_ascii_lowercase();
                if low.contains("lvl=eror") || low.contains("err=") || low.contains(" err ") || low.contains("error") {
                    let m = redact_line(&l);
                    tracing::warn!("Tunnel: {}", m);
                    let mut r = runner().lock();
                    if r.state != RunState::Off {
                        r.state = RunState::Failed { message: m };
                    }
                } else if low.contains("started tunnel") || low.contains("registered tunnel connection") || low.contains("url=https://") {
                    let mut r = runner().lock();
                    if matches!(r.state, RunState::Starting) {
                        r.state = RunState::Running;
                    }
                }
            }
        });
    }
    {
        let mut r = runner().lock();
        r.child = Some(child);
        r.for_settings = Some(s.clone());
    }
    // The webhook URL shown everywhere follows the tunnel.
    let update = crate::config::ServerConfigUpdate {
        host_server: Some(s.public_url()),
        ngrok_allow: Some(true),
        ..Default::default()
    };
    crate::services::settings_service::save_server(ctx, &update)?;
    let _ = ctx.reload_config();
    tracing::info!("Tunnel starting for {}", s.public_url());
    Ok(run_state())
}

/// Whether the child is still alive; a dead child is reported once.
fn reap() {
    let mut r = runner().lock();
    if let Some(c) = r.child.as_mut() {
        if let Ok(Some(status)) = c.try_wait() {
            r.child = None;
            if !matches!(r.state, RunState::Failed { .. }) {
                r.state = RunState::Failed {
                    message: format!("The tunnel stopped ({}). It restarts by itself.", status),
                };
            }
        }
    }
}

/// Keeps the saved tunnel up: starts it once the app is unlocked (the token
/// is encrypted with keys that come with sign-in), and restarts it if it
/// exits. Spawned once at startup.
pub fn supervise(ctx: Arc<AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(15));
        loop {
            tick.tick().await;
            reap();
            let saved = ctx.sqlite.conn().ok().and_then(|c| load(&c).ok().flatten());
            let running = runner().lock().child.is_some();
            match saved {
                Some(s) if s.enabled && ctx.security.is_unlocked() && !running => {
                    if let Err(e) = start(&ctx).await {
                        tracing::warn!("Tunnel did not start: {}", e);
                    }
                }
                Some(s) if !s.enabled && running => stop().await,
                None if running => stop().await,
                _ => {}
            }
        }
    });
}

/// Calls the public URL's `/api/v1/ping` with the API key, the way an alert
/// would arrive, and reports what came back.
pub async fn test_webhook(ctx: &AppState) -> Result<serde_json::Value> {
    let s = {
        let conn = ctx.sqlite.conn()?;
        load(&conn)?
    }
    .ok_or_else(|| AppError::Validation("Set up the webhook URL first.".into()))?;
    let key = crate::services::apikey_service::ApiKeyService::current(ctx)?
        .ok_or_else(|| AppError::Validation("Generate an API key first (API Key page).".into()))?;
    let url = format!("{}/api/v1/ping", s.public_url());
    let started = std::time::Instant::now();
    let resp = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| AppError::Internal(e.to_string()))?
        .post(&url)
        .header("ngrok-skip-browser-warning", "1")
        .json(&serde_json::json!({"apikey": key.expose()}))
        .send()
        .await;
    let ms = started.elapsed().as_millis() as u64;
    Ok(match resp {
        Ok(r) => {
            let status = r.status().as_u16();
            let body: serde_json::Value = r.json().await.unwrap_or(serde_json::Value::Null);
            let ok = status == 200 && body["status"] == "success";
            serde_json::json!({"ok": ok, "http_status": status, "ms": ms, "url": url,
                "message": if ok { "Your webhook URL reached The Bridge.".to_string() }
                           else { format!("The Bridge answered {} through the tunnel.", status) }})
        }
        Err(e) => serde_json::json!({"ok": false, "ms": ms, "url": url,
            "message": format!("Could not reach {}: {}", url, crate::brokers::common::redact::url_safe_error(&e))}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostnames_are_cleaned_and_checked() {
        assert_eq!(clean_hostname(" https://Abc-12.ngrok-free.dev/ "), Some("abc-12.ngrok-free.dev".into()));
        assert_eq!(clean_hostname("alerts.example.com/api"), Some("alerts.example.com".into()));
        for bad in ["", "localhost", "127.0.0.1", "nodot", "-a.example.com", "a..b.com", "has space.com"] {
            assert_eq!(clean_hostname(bad), None, "{}", bad);
        }
    }

    #[test]
    fn tokens_are_taken_from_pasted_commands() {
        let cf = "eyJhIjoiMTIzNDU2Nzg5MCIsInQiOiJhYmMifQ";
        assert_eq!(clean_token(Provider::Cloudflare, &format!("cloudflared.exe service install {}", cf)).as_deref(), Some(cf));
        let ng = "2abcDEFghiJKLmnoPQRstuVWX_1234567890abcdefgh";
        assert_eq!(clean_token(Provider::Ngrok, &format!("ngrok config add-authtoken {}", ng)).as_deref(), Some(ng));
        assert_eq!(clean_token(Provider::Ngrok, "short"), None);
        assert_eq!(clean_token(Provider::Ngrok, "two words that are long enough"), None);
    }

    #[test]
    fn tunnel_callers_reach_only_webhook_routes() {
        for ok in ["/api/v1/placesmartorder", "/api/v1/placeorder", "/api/v1/ping", "/strategy/webhook/abc", "/chartink/webhook/x", "/zerodha/callback"] {
            assert!(tunnel_path_allowed(ok), "{}", ok);
        }
        for no in ["/", "/dashboard", "/login", "/auth/login", "/settings/api/server", "/apikey", "/api/websocket/apikey", "/playground/api-key", "/a/b/callback"] {
            assert!(!tunnel_path_allowed(no), "{}", no);
        }
    }

    #[test]
    fn the_token_is_never_an_argument() {
        let s = TunnelSettings { provider: Provider::Ngrok, hostname: "x.ngrok-free.dev".into(), enabled: true, webhook_only: true };
        let (args, env) = command_args(&s, 5050);
        assert_eq!(env, "NGROK_AUTHTOKEN");
        assert!(args.contains(&"https://x.ngrok-free.dev".to_string()));
        assert!(args.contains(&"http://127.0.0.1:5050".to_string()));
        let s = TunnelSettings { provider: Provider::Cloudflare, ..s };
        let (args, env) = command_args(&s, 5050);
        assert_eq!((args.join(" ").as_str(), env), ("tunnel --no-autoupdate run", "TUNNEL_TOKEN"));
    }

    #[test]
    fn log_lines_lose_credentials() {
        let l = "t=1 lvl=eror msg=\"bad\" authtoken=2abcDEFghiJKLmnoPQRstuVWX_1234567890abcdefgh";
        let r = redact_line(l);
        assert!(!r.contains("2abcDEF"), "{}", r);
        assert!(r.contains("lvl=eror"));
    }

    #[test]
    fn official_sources_only() {
        for p in [Provider::Ngrok, Provider::Cloudflare] {
            if let Some((url, _, _)) = download_source(p) {
                assert!(url.starts_with("https://bin.equinox.io/") || url.starts_with("https://github.com/cloudflare/cloudflared/releases/"), "{}", url);
            }
        }
    }

    #[test]
    fn zip_and_tgz_are_unpacked() {
        let dir = tempfile::tempdir().unwrap();
        // zip
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            w.start_file("ngrok", zip::write::SimpleFileOptions::default()).unwrap();
            std::io::Write::write_all(&mut w, b"BIN").unwrap();
            w.finish().unwrap();
        }
        let out = dir.path().join("a");
        extract(buf.get_ref(), Archive::Zip, "ngrok", &out).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"BIN");
        // tgz (one ustar entry)
        let mut tar = vec![0u8; 512];
        tar[..11].copy_from_slice(b"cloudflared");
        tar[124..135].copy_from_slice(b"00000000003");
        tar[156] = b'0';
        tar.extend_from_slice(b"XYZ");
        tar.resize(1024 + 1024, 0);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, &tar).unwrap();
        let out = dir.path().join("b");
        extract(&gz.finish().unwrap(), Archive::Tgz, "cloudflared", &out).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"XYZ");
    }

    #[test]
    fn settings_round_trip_with_the_token_encrypted() {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        c.execute(
            "INSERT INTO tunnel_settings (id, provider, hostname, token_ct, token_nonce) VALUES (1,'ngrok','x.ngrok-free.dev','ct','n')",
            [],
        )
        .unwrap();
        let s = load(&c).unwrap().unwrap();
        assert_eq!(s.public_url(), "https://x.ngrok-free.dev");
        assert!(s.enabled && s.webhook_only);
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("ct"), "{}", json);
    }
}
