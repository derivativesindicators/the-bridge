//! The Bridge 1.0.5: the permanent webhook URL setup (`crate::tunnel`).

use crate::server::envelope::error;
use crate::server::routes::webui::{failed, ok, JsonBody};
use crate::state::AppState;
use crate::tunnel::{self, Provider};
use axum::{extract::State, http::StatusCode, response::Response};
use serde_json::json;
use std::sync::Arc;

type Ctx = State<Arc<AppState>>;

fn status_json(ctx: &AppState) -> serde_json::Value {
    let saved = ctx
        .sqlite
        .conn()
        .ok()
        .and_then(|c| tunnel::load(&c).ok().flatten());
    json!({
        "status": "success",
        "configured": saved.is_some(),
        "settings": saved,
        "public_url": saved.as_ref().map(|s| s.public_url()),
        "run": tunnel::run_state(),
        "host_server": ctx.server_config().host_server,
        "http_port": ctx.listening_port(),
    })
}

/// GET /api/tunnel
pub async fn status(State(ctx): Ctx) -> Response {
    ok(status_json(&ctx))
}

/// POST /api/tunnel (json: provider, token, hostname). Saves (token
/// encrypted) and starts it.
pub async fn setup(State(ctx): Ctx, JsonBody(b): JsonBody) -> Response {
    let s = |k: &str| b.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let Some(provider) = Provider::parse(s("provider")) else {
        return error(StatusCode::BAD_REQUEST, "Choose ngrok or Cloudflare.");
    };
    let Some(token) = tunnel::clean_token(provider, s("token")) else {
        return error(StatusCode::BAD_REQUEST, match provider {
            Provider::Ngrok => "Paste your ngrok authtoken (Your Authtoken page in the ngrok dashboard).",
            Provider::Cloudflare => "Paste the tunnel token (or the whole install command) from the Cloudflare dashboard.",
        });
    };
    let Some(host) = tunnel::clean_hostname(s("hostname")) else {
        return error(StatusCode::BAD_REQUEST, "Enter the public host name, for example abc-123.ngrok-free.dev or alerts.yourdomain.com.");
    };
    if provider == Provider::Ngrok
        && !(host.ends_with(".ngrok-free.dev")
            || host.ends_with(".ngrok-free.app")
            || host.ends_with(".ngrok.app")
            || host.ends_with(".ngrok.dev")
            || host.ends_with(".ngrok.io")
            || host.ends_with(".ngrok-free.pizza")
            || host.ends_with(".ngrok.pizza"))
    {
        return error(StatusCode::BAD_REQUEST, "That is not an ngrok domain. Copy your dev domain from ngrok's Domains page (it ends in .ngrok-free.dev or .ngrok-free.app).");
    }
    if let Err(e) = tunnel::save(&ctx, provider, &host, &token) {
        return failed(
            "Saving the webhook tunnel",
            e,
            "Could not save the setup. Try again.",
        );
    }
    tracing::info!("Webhook tunnel set up ({:?}, {})", provider, host);
    if let Err(e) = tunnel::start(&ctx).await {
        return error(
            StatusCode::BAD_GATEWAY,
            &format!("Saved, but the tunnel did not start: {}", e),
        );
    }
    ok(status_json(&ctx))
}

/// POST /api/tunnel/start | /api/tunnel/stop
pub async fn start(State(ctx): Ctx) -> Response {
    let _ = tunnel::set_enabled(&ctx, true);
    match tunnel::start(&ctx).await {
        Ok(_) => ok(status_json(&ctx)),
        Err(e) => error(StatusCode::BAD_GATEWAY, &e.to_string()),
    }
}

pub async fn stop(State(ctx): Ctx) -> Response {
    let _ = tunnel::set_enabled(&ctx, false);
    tunnel::stop().await;
    ok(status_json(&ctx))
}

/// POST /api/tunnel/remove: stops it, forgets the token, and gives the
/// webhook URL back to this computer's address.
pub async fn remove(State(ctx): Ctx) -> Response {
    tunnel::stop().await;
    if let Err(e) = tunnel::remove(&ctx) {
        return failed(
            "Removing the webhook tunnel",
            e,
            "Could not remove it. Try again.",
        );
    }
    if let Ok(conn) = ctx.sqlite.conn() {
        let _ = conn.execute(
            "UPDATE settings SET host_server = NULL, ngrok_allow = 0 WHERE id = 1",
            [],
        );
    }
    let _ = ctx.reload_config();
    ok(status_json(&ctx))
}

/// POST /api/tunnel/test
pub async fn test(State(ctx): Ctx) -> Response {
    match tunnel::test_webhook(&ctx).await {
        Ok(v) => ok(json!({"status": "success", "result": v})),
        Err(e) => error(StatusCode::BAD_REQUEST, &e.to_string()),
    }
}
