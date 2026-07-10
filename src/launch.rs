//! Launch a Chromium-family browser pre-configured to use our proxy and to
//! trust only our proxy's certificate (via SPKI pinning). This needs no system
//! proxy change, no certificate installation, and no admin rights — the key
//! property for locked-down environments.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Browser {
    pub id: &'static str,
    pub name: &'static str,
    pub path: PathBuf,
}

#[cfg(target_os = "macos")]
fn candidates() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("chrome", "Google Chrome", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
        ("edge", "Microsoft Edge", "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
        ("brave", "Brave", "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"),
        ("arc", "Arc", "/Applications/Arc.app/Contents/MacOS/Arc"),
        ("chromium", "Chromium", "/Applications/Chromium.app/Contents/MacOS/Chromium"),
    ]
}

#[cfg(target_os = "linux")]
fn candidates() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("chrome", "Google Chrome", "/usr/bin/google-chrome"),
        ("chrome", "Google Chrome", "/usr/bin/google-chrome-stable"),
        ("chromium", "Chromium", "/usr/bin/chromium"),
        ("chromium", "Chromium", "/usr/bin/chromium-browser"),
        ("chromium", "Chromium", "/snap/bin/chromium"),
        ("edge", "Microsoft Edge", "/usr/bin/microsoft-edge"),
        ("brave", "Brave", "/usr/bin/brave-browser"),
    ]
}

#[cfg(target_os = "windows")]
fn candidates() -> Vec<(&'static str, &'static str, String)> {
    let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into());
    let pf86 =
        std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
    let local = std::env::var("LOCALAPPDATA").unwrap_or_default();
    vec![
        ("chrome", "Google Chrome", format!(r"{pf}\Google\Chrome\Application\chrome.exe")),
        ("chrome", "Google Chrome", format!(r"{pf86}\Google\Chrome\Application\chrome.exe")),
        ("chrome", "Google Chrome", format!(r"{local}\Google\Chrome\Application\chrome.exe")),
        ("edge", "Microsoft Edge", format!(r"{pf86}\Microsoft\Edge\Application\msedge.exe")),
        ("edge", "Microsoft Edge", format!(r"{pf}\Microsoft\Edge\Application\msedge.exe")),
        ("brave", "Brave", format!(r"{pf}\BraveSoftware\Brave-Browser\Application\brave.exe")),
    ]
}

/// Every installed Chromium-family browser we can find, de-duplicated by id.
pub fn available() -> Vec<Browser> {
    let mut out: Vec<Browser> = Vec::new();
    for (id, name, path) in candidates() {
        let p = PathBuf::from(&path);
        if p.exists() && !out.iter().any(|b| b.id == id) {
            out.push(Browser { id, name, path: p });
        }
    }
    out
}

/// Launch `browser_id` (or the first available if empty/unknown) against the
/// proxy, opening the control panel. Returns the browser's display name.
pub fn launch(
    browser_id: &str,
    proxy_port: u16,
    ui_port: u16,
    spki: &str,
    profile_dir: &Path,
) -> Result<String> {
    let browsers = available();
    if browsers.is_empty() {
        bail!("No Chromium-based browser (Chrome, Edge, Brave, Arc) found to launch.");
    }
    let browser = browsers
        .iter()
        .find(|b| b.id == browser_id)
        .unwrap_or(&browsers[0]);

    let _ = std::fs::create_dir_all(profile_dir);

    Command::new(&browser.path)
        .arg(format!("--proxy-server=127.0.0.1:{proxy_port}"))
        .arg(format!("--ignore-certificate-errors-spki-list={spki}"))
        .arg(format!("--user-data-dir={}", profile_dir.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg(format!("http://127.0.0.1:{ui_port}/"))
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to start {}: {e}", browser.name))?;

    Ok(browser.name.to_string())
}
