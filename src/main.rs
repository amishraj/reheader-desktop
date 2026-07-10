//! ReHeader Desktop — modify browser HTTP headers via a local MITM proxy,
//! with no browser extension, no system proxy change, and no admin rights.

mod ca;
mod launch;
mod proxy;
mod ui;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::Result;
use clap::Parser;
use hudsucker::{rustls::crypto::aws_lc_rs, Proxy};
use reheader_core::rules::{compile, default_state, AppState};

#[derive(Parser)]
#[command(
    name = "reheader-desktop",
    version,
    about = "Modify browser HTTP request/response headers via a local proxy — no extension required."
)]
struct Args {
    /// Port for the local proxy the browser connects to.
    #[arg(long, default_value_t = 8888)]
    proxy_port: u16,
    /// Port for the local control-panel web UI.
    #[arg(long, default_value_t = 8889)]
    ui_port: u16,
    /// Override the data directory (CA, profiles, browser profile).
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Also launch a pre-configured browser on startup (chrome|edge|brave|arc).
    #[arg(long)]
    launch: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    init_tracing();
    let _ = aws_lc_rs::default_provider().install_default();

    let data_dir = args.data_dir.unwrap_or_else(default_data_dir);
    std::fs::create_dir_all(&data_dir)?;
    let profile_dir = data_dir.join("browser-profile");
    let state_path = data_dir.join("state.json");

    let ca = ca::load_or_create(&data_dir)?;

    let state = load_state(&state_path);
    let compiled = compile(&state);
    for e in &compiled.errors {
        tracing::warn!("{e}");
    }

    let state_arc = Arc::new(RwLock::new(state));
    let compiled_arc = Arc::new(RwLock::new(compiled));

    let cfg = Arc::new(ui::Config {
        proxy_port: args.proxy_port,
        ui_port: args.ui_port,
        spki: ca.spki_sha256_b64.clone(),
        ca_path: ca.cert_path.clone(),
        state_path,
        profile_dir: profile_dir.clone(),
    });

    let ui_ctx = ui::AppCtx {
        state: state_arc,
        compiled: compiled_arc.clone(),
        cfg: cfg.clone(),
    };
    let ui_addr = SocketAddr::from(([127, 0, 0, 1], args.ui_port));
    tokio::spawn(ui::serve(ui_addr, ui_ctx));

    print_banner(&cfg);

    if let Some(browser) = &args.launch {
        match launch::launch(browser, args.proxy_port, args.ui_port, &cfg.spki, &profile_dir) {
            Ok(name) => tracing::info!("launched {name}"),
            Err(e) => tracing::error!("launch failed: {e}"),
        }
    }

    let handler = proxy::RuleHandler::new(compiled_arc);
    let proxy = Proxy::builder()
        .with_addr(SocketAddr::from(([127, 0, 0, 1], args.proxy_port)))
        .with_ca(ca.authority)
        .with_rustls_connector(aws_lc_rs::default_provider())
        .with_http_handler(handler.clone())
        .with_websocket_handler(handler)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutting down");
        })
        .build()
        .expect("failed to build proxy");

    tracing::info!("proxy listening on 127.0.0.1:{}", args.proxy_port);
    proxy
        .start()
        .await
        .map_err(|e| anyhow::anyhow!("proxy error: {e}"))?;
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

fn default_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("ReHeaderDesktop")
}

fn load_state(path: &Path) -> AppState {
    match std::fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
            tracing::warn!("could not parse {}: {e}; using defaults", path.display());
            default_state()
        }),
        Err(_) => default_state(),
    }
}

fn print_banner(cfg: &ui::Config) {
    println!();
    println!("  ReHeader Desktop");
    println!("  ──────────────────────────────────────────────");
    println!("  Control panel : http://127.0.0.1:{}", cfg.ui_port);
    println!("  Proxy address : 127.0.0.1:{}", cfg.proxy_port);
    println!("  CA SPKI pin   : {}", cfg.spki);
    println!();
    println!("  1. Open the control panel above.");
    println!("  2. Click “Launch secure browser”.");
    println!("     (No proxy setup, no certificate install, no admin needed.)");
    println!();
    println!("  Ctrl+C to stop.");
    println!();
}
