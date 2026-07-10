# ReHeader Desktop

**Modify HTTP request & response headers in any browser — with no browser
extension, no system proxy change, and no admin rights.** Built for locked-down
environments where extensions are blocked by policy.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/panel-dark.png">
  <img src="docs/panel-light.png" alt="ReHeader Desktop control panel" width="720">
</picture>

ReHeader Desktop is a tiny local app (one self-contained binary, ~no
dependencies) that runs a local proxy and rewrites headers in flight — the same
approach Charles Proxy and Fiddler use. It's the companion to the
[ReHeader browser extension](https://github.com/amishraj/reheader); profiles are
interchangeable between them.

## Why this exists

Header-modifying browser extensions (like ModHeader) are increasingly blocked by
corporate policy — and ModHeader itself
[shipped spyware](https://github.com/amishraj/reheader#why-you-can-trust-it) and
was pulled from the Chrome Web Store. A local proxy is a completely different
mechanism, so an extension-install policy doesn't stop it.

## The locked-down-friendly design

Modifying **HTTPS** traffic normally requires installing a trusted root
certificate — which locked-down machines often block (it needs admin). ReHeader
Desktop avoids that entirely:

1. It generates its own local certificate authority.
2. It launches a **dedicated browser window** pointed at the local proxy and
   told — via Chromium's `--ignore-certificate-errors-spki-list` flag — to trust
   **only** this app's certificate, identified by its public-key hash.

The result: **no system proxy setting is changed, no certificate is installed,
and no admin rights are needed.** Your normal browser windows are untouched; only
the dedicated window routes through ReHeader.

> This is verified end-to-end in testing: real Chrome, nothing installed, loads
> an intercepted HTTPS page and sees the injected header.

## Works behind a corporate proxy

On a company network, all traffic must go through the corporate proxy — so
ReHeader forwards **through** it (`browser → ReHeader → company proxy →
internet`) rather than connecting directly. It **auto-detects** your system
proxy on startup (environment variables, the Windows registry / a PAC script, or
macOS network settings) and chains through it automatically.

If detection misses it, open the control panel and set **Corporate / upstream
proxy** to `host:port` — it's remembered and applied without restarting the app.
Set it back to blank for a direct connection. (Transparent proxies and ones your
machine authenticates to automatically work out of the box; proxies that pop up
a username/password prompt aren't supported yet.)

> If you previously saw **502 errors** on every site, this is why — an earlier
> build connected directly and your network blocked it. Set/confirm the upstream
> proxy and it'll route correctly.

## Install

1. Download the binary for your OS from
   [Releases](https://github.com/amishraj/reheader-desktop/releases):
   - macOS Apple Silicon: `reheader-desktop-aarch64-apple-darwin`
   - macOS Intel: `reheader-desktop-x86_64-apple-darwin`
   - Linux: `reheader-desktop-x86_64-unknown-linux-gnu`
   - Windows: `reheader-desktop-x86_64-pc-windows-msvc.exe`
2. Run it:
   - **macOS/Linux:** `chmod +x reheader-desktop-* && ./reheader-desktop-*`
     (macOS may quarantine an unsigned download — if it refuses to open, run
     `xattr -d com.apple.quarantine ./reheader-desktop-*` first, or right-click →
     Open once.)
   - **Windows:** double-click the `.exe` (SmartScreen → *More info* → *Run
     anyway* for an unsigned build).
3. The control panel opens at <http://127.0.0.1:8889>. Add your headers, then
   click **Launch secure browser**. Use that window for your requests.

That's it — nothing is installed system-wide. To stop, press `Ctrl+C` in the
terminal (or close the app).

## Features

- Add / override / remove **request headers** and **response headers**
  (empty value = remove the header)
- **Redirect URLs** by regex, with `\1…\9` capture groups
- **Multiple profiles** with colors, cloning, and one-click switching; all
  enabled profiles apply at once
- **Filters** — apply a profile only to URLs matching a regex, or exclude URLs
- **Per-header comments**, header-name autocomplete
- **Import / export** JSON — including profiles exported from the ReHeader
  extension **and from ModHeader**
- **Pause** everything; live count of active modifications; light / dark themes
- One-click launch for **Chrome, Edge, Brave, Arc**, or any Chromium
- **Works behind a corporate proxy** — auto-detected and chained through
  (see below)

## Prefer to configure it yourself?

Open **Manual setup & details** in the control panel. You can point any browser's
HTTP/HTTPS proxy at `127.0.0.1:8888` and either:

- **Install the CA** (`Download reheader-ca.pem`) into your trust store — works
  for every browser, but may need admin; or
- **Launch a Chromium browser with the SPKI pin** shown there — no install
  needed. The exact command line is displayed for you to copy.

## Verifying your headers are applied

Because ReHeader is a **proxy**, not an extension, it adds your **request**
headers *after* the browser has sent them — so Chrome DevTools' **Request
Headers** won't show them (DevTools shows what the browser sent; the server
still receives your headers). This surprises people coming from ModHeader,
which runs inside the browser. Every proxy tool (Charles, Fiddler, mitmproxy)
behaves the same way.

**Built-in inspector (easiest):** in the launched browser, visit
**`http://reheader.echo`**. It lists the real outgoing request headers with your
injected ones highlighted — the visibility DevTools can't give you. To check a
specific site respecting URL filters, use
`http://reheader.echo/?url=https://your-site.com/path`.

**Verify mode:** turn it on in the control panel to also add a response header
you *can* see in DevTools:

```
X-ReHeader-Applied: req[Authorization=Bearer x, -Referer] resp[X-Frame-Options=ALLOW]
```

Open DevTools → **Network** → click the request → **Response Headers**. If you
see `X-ReHeader-Applied` listing your changes, they're working. If it shows
`req[] resp[]`, ReHeader is in the path but no rule matched that URL — usually a
**filter** that doesn't match (loosen or remove it). If the header is absent
entirely, that request wasn't intercepted (wrong window, or a cached response —
hard-reload with cache disabled).

**Response-header** changes are always visible in DevTools directly (ReHeader
edits them before they reach the browser). Turn Verify mode off for normal use.

## How it works

```
 browser ──HTTP/HTTPS──▶ 127.0.0.1:8888 (local proxy) ──▶ real server
                              │
                    applies your header rules
```

The proxy is a MITM proxy built on [hudsucker](https://github.com/omjadas/hudsucker).
For HTTPS it terminates TLS using a leaf certificate signed by the local CA;
because every leaf reuses the CA's key, a single pinned SPKI hash trusts all
intercepted hosts. Rule matching (`src/rules.rs`) is a pure, unit-tested module
shared with nothing external.

### Notes & limitations

- **Filters/redirects use Rust `regex`** (RE2-style, no backreferences in the
  pattern). Redirect targets use `\1…\9` for capture groups. If every include
  filter on a profile is invalid, the profile is disabled rather than applied
  everywhere.
- **Redirects** are implemented as `307 Temporary Redirect` responses (the
  browser re-requests the new URL).
- **Resource-type filters** (XHR, image, …) from the extension format are
  accepted on import but ignored — a proxy can't reliably reconstruct the
  browser's resource classification. URL filters cover most cases.
- Some hop-by-hop headers are managed by the network stack and can't be
  overridden.
- The proxy binds to `127.0.0.1` only. Your CA private key lives in the app's
  data directory (`~/Library/Application Support/ReHeaderDesktop` on macOS,
  `~/.local/share/ReHeaderDesktop` on Linux, `%APPDATA%` on Windows), readable
  only by you. Anyone with that key could MITM your dedicated browser, so keep it
  private; delete the folder to reset.

## CLI

```
reheader-desktop [--proxy-port 8888] [--ui-port 8889] [--data-dir <path>]
                 [--launch chrome|edge|brave|arc]
                 [--upstream-proxy host:port] [--no-upstream-proxy]
```

`--launch` also opens the pre-configured browser on startup. `--upstream-proxy`
overrides auto-detection (and is remembered); `--no-upstream-proxy` forces a
direct connection.

## Build from source

Requires a Rust toolchain (and a C compiler + CMake, for the TLS backend).

```sh
cargo test --lib     # fast, pure rule-engine tests
cargo build --release
./target/release/reheader-desktop
```

The web UI is embedded into the binary at build time, so the release binary is
fully self-contained.

## License

[MIT](LICENSE)
