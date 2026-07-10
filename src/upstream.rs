//! Corporate/upstream proxy chaining. In a locked-down network the browser must
//! reach the internet through the company proxy; ReHeader sits in front of it,
//! so its own outbound client has to forward *through* that proxy rather than
//! dialing servers directly. This module detects the system proxy and builds a
//! CONNECT-tunneling connector for hudsucker's upstream client.

use std::net::ToSocketAddrs;
use std::path::Path;

use anyhow::Context;
use http::Uri;
use hudsucker::hyper_util::client::legacy::connect::HttpConnector;
use hyper_http_proxy::{Intercept, Proxy, ProxyConnector};
use reheader_core::proxydetect::{self, Detected};
use serde::{Deserialize, Serialize};

/// Detect the effective system proxy: environment variables first, then the OS
/// setting (static server or a PAC URL, which we fetch and scan).
pub fn detect_system_proxy() -> Option<String> {
    for key in [
        "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy",
    ] {
        if let Ok(v) = std::env::var(key) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    match os_detect() {
        Some(Detected::Server(s)) => Some(s),
        Some(Detected::Pac(url)) => http_get(&url).and_then(|body| proxydetect::scan_pac(&body)),
        None => None,
    }
}

#[cfg(target_os = "macos")]
fn os_detect() -> Option<Detected> {
    let out = std::process::Command::new("scutil").arg("--proxy").output().ok()?;
    proxydetect::parse_scutil(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(target_os = "windows")]
fn os_detect() -> Option<Detected> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let settings = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()?;
    let enabled: u32 = settings.get_value("ProxyEnable").unwrap_or(0);
    if enabled == 1 {
        if let Ok(server) = settings.get_value::<String, _>("ProxyServer") {
            if let Some(s) = proxydetect::parse_win_proxy_server(&server) {
                return Some(Detected::Server(s));
            }
        }
    }
    if let Ok(pac) = settings.get_value::<String, _>("AutoConfigURL") {
        if !pac.trim().is_empty() {
            return Some(Detected::Pac(pac));
        }
    }
    None
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn os_detect() -> Option<Detected> {
    None
}

/// Minimal blocking HTTP GET, used only to fetch a PAC file at startup. HTTP
/// only — enterprise PAC URLs are typically served over plain HTTP internally.
fn http_get(url: &str) -> Option<String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let rest = url.strip_prefix("http://")?;
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let host = hostport.split(':').next().unwrap_or(hostport);
    let addr = if hostport.contains(':') {
        hostport.to_string()
    } else {
        format!("{hostport}:80")
    };
    let socket = addr.to_socket_addrs().ok()?.next()?;
    let mut stream = TcpStream::connect_timeout(&socket, Duration::from_secs(3)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
    let req = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).ok()?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).ok()?;
    buf.split_once("\r\n\r\n").map(|(_, body)| body.to_string())
}

/// Build a connector that tunnels all upstream traffic through `upstream`
/// (`host:port` or a full URL). Uses the machine's native trust store so
/// corporate TLS-inspection certificates validate.
pub fn build_connector(upstream: &str) -> anyhow::Result<ProxyConnector<HttpConnector>> {
    let url = normalize(upstream);
    let uri: Uri = url
        .parse()
        .with_context(|| format!("invalid proxy address {url:?}"))?;
    let proxy = Proxy::new(Intercept::All, uri);
    let mut http = HttpConnector::new();
    http.enforce_http(false);
    let connector = ProxyConnector::from_proxy(http, proxy)
        .context("failed to build upstream proxy connector")?;
    Ok(connector)
}

fn normalize(s: &str) -> String {
    let s = s.trim();
    if s.contains("://") {
        s.to_string()
    } else {
        format!("http://{s}")
    }
}

// --- persisted settings (separate from the rule profiles) ----------------

#[derive(Serialize, Deserialize, Default)]
struct Settings {
    #[serde(rename = "upstreamProxy")]
    upstream_proxy: Option<String>,
}

pub fn load_upstream(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<Settings>(&text).ok()?.upstream_proxy
}

pub fn save_upstream(path: &Path, value: Option<&str>) {
    let settings = Settings {
        upstream_proxy: value.map(str::to_string),
    };
    if let Ok(json) = serde_json::to_string_pretty(&settings) {
        let _ = std::fs::write(path, json);
    }
}
