//! spr-nostr — a single Rust binary that IS the whole plugin backend.
//!
//! It runs a Nostr relay in-process (`nostr-relay-builder` `LocalRelay` +
//! `nostr-lmdb` persistence) AND serves the SPR plugin's REST API + iframe UI
//! over a unix socket using axum on a tokio `UnixListener`. There is no Go and
//! no forked relay process — "restart" rebuilds the relay in-process.

mod config;
mod http;
mod nip11;
mod relay;
mod topology;

use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Serialize;
use tokio::sync::Mutex;

use crate::config::{Config, Mode};
use crate::relay::Supervisor;

const ENGINE: &str = "nostr-relay-builder 0.44.1";

fn env_path(key: &str, default: &str) -> PathBuf {
    std::env::var_os(key)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(default))
}

/// Discover the container's own IP (the eth0 address on the spr-nostr bridge)
/// without extra crates: a connected UDP socket selects its source address from
/// the routing table without sending a packet. TEST-NET-1 is never local, so
/// the selected source is the container's routable interface address.
fn discover_container_ip() -> Option<IpAddr> {
    let sock = UdpSocket::bind(("0.0.0.0", 0)).ok()?;
    sock.connect(("192.0.2.1", 9)).ok()?;
    let ip = sock.local_addr().ok()?.ip();
    if ip.is_unspecified() || ip.is_loopback() {
        None
    } else {
        Some(ip)
    }
}

#[derive(Clone)]
struct AppState {
    sup: Arc<Supervisor>,
    cfg: Arc<Mutex<Config>>,
    cfg_path: PathBuf,
    host: String,
    ui_html: Arc<String>,
}

#[derive(Serialize)]
struct StatusResp {
    #[serde(rename = "Running")]
    running: bool,
    #[serde(rename = "Address")]
    address: String,
    #[serde(rename = "Host")]
    host: String,
    #[serde(rename = "Port")]
    port: u16,
    #[serde(rename = "Mode")]
    mode: Mode,
    #[serde(rename = "RequireAuth")]
    require_auth: bool,
    #[serde(rename = "Version")]
    version: String,
    #[serde(rename = "Engine")]
    engine: String,
    /// We serve a NIP-11 relay information document from the relay port
    /// ourselves (see http.rs), so this is true and the metadata is configurable.
    #[serde(rename = "Nip11Supported")]
    nip11_supported: bool,
    #[serde(rename = "UptimeSeconds")]
    uptime_seconds: u64,
    #[serde(rename = "DbBytes")]
    db_bytes: u64,
}

#[derive(Serialize)]
struct AddressResp {
    #[serde(rename = "Address")]
    address: String,
    #[serde(rename = "Host")]
    host: String,
    #[serde(rename = "Port")]
    port: u16,
}

fn relay_url(host: &str, port: u16) -> String {
    format!("ws://{host}:{port}")
}

async fn handle_status(State(st): State<AppState>) -> Json<StatusResp> {
    let cfg = st.cfg.lock().await.clone();
    let snap = st.sup.snapshot().await;
    Json(StatusResp {
        running: snap.running,
        address: relay_url(&st.host, cfg.port),
        host: st.host.clone(),
        port: cfg.port,
        mode: cfg.mode,
        require_auth: cfg.require_auth,
        version: concat!("v", env!("CARGO_PKG_VERSION")).to_string(),
        engine: ENGINE.to_string(),
        nip11_supported: true,
        uptime_seconds: snap.uptime_seconds,
        db_bytes: st.sup.db_bytes(),
    })
}

async fn handle_address(State(st): State<AppState>) -> Json<AddressResp> {
    let cfg = st.cfg.lock().await.clone();
    Json(AddressResp {
        address: relay_url(&st.host, cfg.port),
        host: st.host.clone(),
        port: cfg.port,
    })
}

async fn handle_get_config(State(st): State<AppState>) -> Json<Config> {
    Json(st.cfg.lock().await.clone())
}

async fn handle_put_config(
    State(st): State<AppState>,
    Json(update): Json<Config>,
) -> Result<Json<Config>, (StatusCode, String)> {
    update
        .validate()
        .map_err(|e| (StatusCode::BAD_REQUEST, e))?;

    // Persist first, then apply (rebuild the relay in-process).
    {
        let mut cur = st.cfg.lock().await;
        update.save_atomic(&st.cfg_path).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("saving config: {e}"),
            )
        })?;
        *cur = update.clone();
    }
    st.sup.restart(&update).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("config saved but relay restart failed: {e}"),
        )
    })?;
    Ok(Json(update))
}

async fn handle_restart(
    State(st): State<AppState>,
) -> Result<Json<StatusResp>, (StatusCode, String)> {
    let cfg = st.cfg.lock().await.clone();
    st.sup
        .restart(&cfg)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(handle_status(State(st)).await)
}

async fn handle_topology(State(st): State<AppState>) -> impl IntoResponse {
    let snap = st.sup.snapshot().await;
    Json(topology::build_topology(snap.running, &st.host))
}

/// Serve the bundled single-file UI for any non-API path. The frontend build
/// inlines every asset into index.html, so one document covers the whole SPA.
async fn serve_ui(State(st): State<AppState>) -> Html<String> {
    Html((*st.ui_html).clone())
}

async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("install SIGTERM handler");
        let mut int = signal(SignalKind::interrupt()).expect("install SIGINT handler");
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg_path = env_path("CONFIG_FILE", "/configs/spr-nostr/config.json");
    let db_path = env_path("NOSTR_DB", "/state/plugins/spr-nostr/db");
    let socket_path = env_path("SOCKET_PATH", "/run/spr-krun-plugin/spr-nostr.sock");
    let ui_dir = env_path("UI_DIR", "/ui");

    // Load config; on first boot persist the defaults so the file exists.
    let cfg = Config::load(&cfg_path).unwrap_or_else(|e| {
        eprintln!("spr-nostr: loading config failed ({e}); using defaults");
        Config::default()
    });
    if !cfg_path.exists() {
        if let Err(e) = cfg.save_atomic(&cfg_path) {
            eprintln!("spr-nostr: writing default config failed: {e}");
        }
    }

    // Bind IP: explicit override, else the discovered container IP, else all
    // interfaces on the bridge (there are no published host ports either way).
    let discovered = discover_container_ip();
    let bind_ip: IpAddr = match std::env::var("NOSTR_ADDR")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        Some(ip) => ip,
        None => discovered.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
    };
    // Host shown in the copyable ws:// address (never 0.0.0.0).
    let host = match discovered {
        Some(ip) => ip.to_string(),
        None if !bind_ip.is_unspecified() => bind_ip.to_string(),
        None => "127.0.0.1".to_string(),
    };

    let sup = Arc::new(Supervisor::new(db_path, bind_ip)?);
    if let Err(e) = sup.start(&cfg).await {
        // Keep the API up so the UI can show the error and offer Restart.
        eprintln!("spr-nostr: starting relay failed: {e}");
    }

    let ui_html = std::fs::read_to_string(ui_dir.join("index.html")).unwrap_or_else(|_| {
        "<!doctype html><html><body><p>spr-nostr UI not bundled in this image.</p></body></html>"
            .to_string()
    });

    let state = AppState {
        sup: sup.clone(),
        cfg: Arc::new(Mutex::new(cfg)),
        cfg_path,
        host: host.clone(),
        ui_html: Arc::new(ui_html),
    };

    let app = Router::new()
        .route("/status", get(handle_status))
        .route("/address", get(handle_address))
        .route("/config", get(handle_get_config).put(handle_put_config))
        .route("/restart", post(handle_restart))
        .route("/topology", get(handle_topology))
        .fallback(serve_ui)
        .with_state(state);

    // Fresh unix socket, chmod 0770 (SPR proxies /plugins/spr-nostr/* to it).
    if let Some(dir) = socket_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let _ = std::fs::remove_file(&socket_path);
    let listener = tokio::net::UnixListener::bind(&socket_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o770))?;
    }
    println!("spr-nostr: API listening on unix:{}", socket_path.display());

    axum::serve(listener, app)
        .with_graceful_shutdown(wait_for_shutdown())
        .await?;

    // Graceful shutdown: stop the relay and remove the socket.
    sup.shutdown().await;
    let _ = std::fs::remove_file(&socket_path);
    println!("spr-nostr: shut down cleanly");
    Ok(())
}
