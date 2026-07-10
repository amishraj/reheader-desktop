//! ReHeader Desktop — modify browser HTTP headers via a local MITM proxy,
//! with no browser extension, no system proxy change, and no admin rights.
//! Chains through a corporate/upstream proxy when present.

mod ca;
mod launch;
mod proxy;
mod ui;
mod upstream;

use std::future::Future;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::Result;
use clap::Parser;
use hudsucker::certificate_authority::RcgenAuthority;
use hudsucker::hyper_util::client::legacy::connect::Connect;
use hudsucker::{rustls::crypto::aws_lc_rs, Proxy};
use reheader_core::rules::{compile, default_state, AppState};

use ui::Runtime;

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
    /// Route upstream traffic through this corporate proxy (host:port).
    /// Overrides auto-detection and is remembered.
    #[arg(long)]
    upstream_proxy: Option<String>,
    /// Force direct upstream connections, ignoring any detected system proxy.
    #[arg(long)]
    no_upstream_proxy: bool,
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
    let settings_path = data_dir.join("settings.json");

    let state = load_state(&state_path);
    let compiled = compile(&state);
    for e in &compiled.errors {
        tracing::warn!("{e}");
    }
    let state_arc = Arc::new(RwLock::new(state));
    let compiled_arc = Arc::new(RwLock::new(compiled));

    // Effective upstream proxy: --upstream-proxy > saved setting > auto-detected.
    // --no-upstream-proxy forces direct.
    let detected = if args.no_upstream_proxy {
        None
    } else {
        upstream::detect_system_proxy()
    };
    let effective_upstream = if args.no_upstream_proxy {
        None
    } else {
        args.upstream_proxy
            .clone()
            .or_else(|| upstream::load_upstream(&settings_path))
            .or_else(|| detected.clone())
    };
    if args.upstream_proxy.is_some() {
        upstream::save_upstream(&settings_path, effective_upstream.as_deref());
    }

    // Load the CA once for its SPKI/cert path (the info the UI needs). The proxy
    // loop reloads it to get a fresh authority on each (re)start.
    let ca_info = ca::load_or_create(&data_dir)?;
    let cfg = Arc::new(ui::Config {
        proxy_port: args.proxy_port,
        ui_port: args.ui_port,
        spki: ca_info.spki_sha256_b64.clone(),
        ca_path: ca_info.cert_path.clone(),
        state_path,
        profile_dir: profile_dir.clone(),
    });
    drop(ca_info);

    let runtime = Arc::new(Runtime {
        upstream: RwLock::new(effective_upstream),
        detected,
        settings_path,
        restart: tokio::sync::Notify::new(),
        stop: AtomicBool::new(false),
    });

    let ui_ctx = ui::AppCtx {
        state: state_arc,
        compiled: compiled_arc.clone(),
        cfg: cfg.clone(),
        runtime: runtime.clone(),
    };
    tokio::spawn(ui::serve(
        SocketAddr::from(([127, 0, 0, 1], args.ui_port)),
        ui_ctx,
    ));

    print_banner(&cfg, runtime.upstream.read().unwrap().as_deref());

    if let Some(browser) = &args.launch {
        match launch::launch(browser, args.proxy_port, args.ui_port, &cfg.spki, &profile_dir) {
            Ok(name) => tracing::info!("launched {name}"),
            Err(e) => tracing::error!("launch failed: {e}"),
        }
    }

    // Supervised proxy: rebuilds (with a fresh connector) whenever the upstream
    // proxy setting changes; exits on Ctrl+C.
    let addr = SocketAddr::from(([127, 0, 0, 1], args.proxy_port));
    loop {
        let upstream_now = runtime.upstream.read().unwrap().clone();
        let ca = ca::load_or_create(&data_dir)?;
        let handler = proxy::RuleHandler::new(compiled_arc.clone());
        let rt = runtime.clone();
        let shutdown = async move {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => rt.stop.store(true, Ordering::SeqCst),
                _ = rt.restart.notified() => {}
            }
        };

        let result = match upstream_now.as_deref() {
            Some(u) => match upstream::build_connector(u) {
                Ok(conn) => {
                    tracing::info!("upstream proxy: {u}");
                    start_chained(addr, ca.authority, conn, handler, shutdown).await
                }
                Err(e) => {
                    tracing::error!("invalid upstream proxy {u:?}: {e}; using direct");
                    start_direct(addr, ca.authority, handler, shutdown).await
                }
            },
            None => {
                tracing::info!("upstream: direct (no corporate proxy)");
                start_direct(addr, ca.authority, handler, shutdown).await
            }
        };

        if let Err(e) = result {
            tracing::error!("proxy stopped: {e}");
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        if runtime.stop.load(Ordering::SeqCst) {
            break;
        }
        tracing::info!("restarting proxy with updated settings…");
    }
    Ok(())
}

async fn start_chained<C, F>(
    addr: SocketAddr,
    ca: RcgenAuthority,
    connector: C,
    handler: proxy::RuleHandler,
    shutdown: F,
) -> Result<()>
where
    C: Connect + Clone + Send + Sync + 'static,
    F: Future<Output = ()> + Send + 'static,
{
    let proxy = Proxy::builder()
        .with_addr(addr)
        .with_ca(ca)
        .with_http_connector(connector)
        .with_http_handler(handler.clone())
        .with_websocket_handler(handler)
        .with_graceful_shutdown(shutdown)
        .build()
        .expect("failed to build proxy");
    proxy.start().await.map_err(|e| anyhow::anyhow!("{e}"))
}

async fn start_direct<F>(
    addr: SocketAddr,
    ca: RcgenAuthority,
    handler: proxy::RuleHandler,
    shutdown: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let proxy = Proxy::builder()
        .with_addr(addr)
        .with_ca(ca)
        .with_rustls_connector(aws_lc_rs::default_provider())
        .with_http_handler(handler.clone())
        .with_websocket_handler(handler)
        .with_graceful_shutdown(shutdown)
        .build()
        .expect("failed to build proxy");
    proxy.start().await.map_err(|e| anyhow::anyhow!("{e}"))
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    // hudsucker logs one ERROR per failed upstream connection. On a corporate
    // network the browser fires constant third-party telemetry (analytics,
    // crash reporters) that the proxy blocks, which floods the terminal with
    // scary-looking but harmless 407s. Silence hudsucker by default; set
    // RUST_LOG=hudsucker=info to see per-connection detail when debugging.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,hudsucker=off"));
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

fn print_banner(cfg: &ui::Config, upstream: Option<&str>) {
    println!();
    println!("  ReHeader Desktop");
    println!("  ──────────────────────────────────────────────");
    println!("  Control panel : http://127.0.0.1:{}", cfg.ui_port);
    println!("  Proxy address : 127.0.0.1:{}", cfg.proxy_port);
    println!("  Upstream proxy: {}", upstream.unwrap_or("direct (none)"));
    println!("  CA SPKI pin   : {}", cfg.spki);
    println!();
    println!("  1. Open the control panel above.");
    println!("  2. Click “Launch secure browser”.");
    println!("     (No proxy setup, no certificate install, no admin needed.)");
    println!();
    println!("  Ctrl+C to stop.");
    println!();
}
