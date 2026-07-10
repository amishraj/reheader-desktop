//! Local control-panel server (axum). Serves the embedded UI and a small JSON
//! API for reading/writing profiles, downloading the CA, and launching a
//! pre-configured browser. Bound to 127.0.0.1 only.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use crate::launch;
use reheader_core::rules::{compile, AppState, Compiled};

pub struct Config {
    pub proxy_port: u16,
    pub ui_port: u16,
    pub spki: String,
    pub ca_path: PathBuf,
    pub state_path: PathBuf,
    pub profile_dir: PathBuf,
}

#[derive(Clone)]
pub struct AppCtx {
    pub state: Arc<RwLock<AppState>>,
    pub compiled: Arc<RwLock<Compiled>>,
    pub cfg: Arc<Config>,
}

const INDEX: &str = include_str!("../ui/index.html");
const CSS: &str = include_str!("../ui/app.css");
const JS: &str = include_str!("../ui/app.js");

pub async fn serve(addr: std::net::SocketAddr, ctx: AppCtx) {
    let app = Router::new()
        .route("/", get(index))
        .route("/app.css", get(css))
        .route("/app.js", get(js))
        .route("/api/state", get(get_state).post(set_state))
        .route("/api/info", get(info))
        .route("/api/ca", get(download_ca))
        .route("/api/launch", post(launch_browser))
        .with_state(ctx);

    match tokio::net::TcpListener::bind(addr).await {
        Ok(listener) => {
            if let Err(e) = axum::serve(listener, app).await {
                eprintln!("UI server error: {e}");
            }
        }
        Err(e) => eprintln!("Failed to bind control panel on {addr}: {e}"),
    }
}

async fn index() -> Html<&'static str> {
    Html(INDEX)
}

async fn css() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], CSS)
}

async fn js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/javascript; charset=utf-8")],
        JS,
    )
}

async fn get_state(State(ctx): State<AppCtx>) -> Json<AppState> {
    Json(ctx.state.read().unwrap().clone())
}

#[derive(Serialize)]
struct SaveResult {
    errors: Vec<String>,
    active_count: usize,
}

async fn set_state(State(ctx): State<AppCtx>, Json(new_state): Json<AppState>) -> Json<SaveResult> {
    let compiled = compile(&new_state);
    let result = SaveResult {
        errors: compiled.errors.clone(),
        active_count: compiled.active_count(),
    };

    if let Ok(json) = serde_json::to_string_pretty(&new_state) {
        let _ = std::fs::write(&ctx.cfg.state_path, json);
    }
    *ctx.state.write().unwrap() = new_state;
    *ctx.compiled.write().unwrap() = compiled;

    Json(result)
}

#[derive(Serialize)]
struct BrowserInfo {
    id: String,
    name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Info {
    proxy_port: u16,
    ui_port: u16,
    spki: String,
    ca_path: String,
    platform: String,
    active_count: usize,
    errors: Vec<String>,
    browsers: Vec<BrowserInfo>,
}

async fn info(State(ctx): State<AppCtx>) -> Json<Info> {
    let (active_count, errors) = {
        let c = ctx.compiled.read().unwrap();
        (c.active_count(), c.errors.clone())
    };
    let browsers = launch::available()
        .into_iter()
        .map(|b| BrowserInfo {
            id: b.id.to_string(),
            name: b.name.to_string(),
        })
        .collect();

    Json(Info {
        proxy_port: ctx.cfg.proxy_port,
        ui_port: ctx.cfg.ui_port,
        spki: ctx.cfg.spki.clone(),
        ca_path: ctx.cfg.ca_path.display().to_string(),
        platform: std::env::consts::OS.to_string(),
        active_count,
        errors,
        browsers,
    })
}

async fn download_ca(State(ctx): State<AppCtx>) -> Response {
    match std::fs::read(&ctx.cfg.ca_path) {
        Ok(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "application/x-pem-file"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"reheader-ca.pem\"",
                ),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "CA certificate not found").into_response(),
    }
}

#[derive(Deserialize)]
struct LaunchReq {
    #[serde(default)]
    browser: String,
}

#[derive(Serialize)]
struct LaunchResp {
    ok: bool,
    message: String,
}

async fn launch_browser(State(ctx): State<AppCtx>, Json(req): Json<LaunchReq>) -> Json<LaunchResp> {
    match launch::launch(
        &req.browser,
        ctx.cfg.proxy_port,
        ctx.cfg.ui_port,
        &ctx.cfg.spki,
        &ctx.cfg.profile_dir,
    ) {
        Ok(name) => Json(LaunchResp {
            ok: true,
            message: format!("Launched {name} — a new window opened, pre-configured and trusted."),
        }),
        Err(e) => Json(LaunchResp {
            ok: false,
            message: e.to_string(),
        }),
    }
}
