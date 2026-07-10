//! The hudsucker HTTP handler: looks up the compiled ruleset per request and
//! applies header edits / redirects. hudsucker clones the handler per request
//! (the `&mut self` methods guarantee exclusive access), so `pending` holds
//! this exchange's response-header ops safely even under HTTP/2 multiplexing.

use std::sync::{Arc, RwLock};

use http::{
    header::{HeaderName, HeaderValue, CONTENT_TYPE, HOST, LOCATION},
    HeaderMap, Method, Request, Response, StatusCode,
};
use hudsucker::{
    tokio_tungstenite::tungstenite::Message, Body, HttpContext, HttpHandler, RequestOrResponse,
    WebSocketContext, WebSocketHandler,
};
use reheader_core::rules::{Compiled, HeaderAction, HeaderOp, Plan};

/// A made-up host the browser can visit to see what ReHeader is doing. All
/// browser traffic is proxied through us, so a request to this host never hits
/// DNS — we intercept it and return a report instead.
const ECHO_HOST: &str = "reheader.echo";

#[derive(Clone)]
pub struct RuleHandler {
    compiled: Arc<RwLock<Compiled>>,
    pending: Vec<HeaderAction>,
    /// In verify mode, a human-readable summary of what this exchange applied,
    /// echoed back as the `X-ReHeader-Applied` response header.
    applied: Option<String>,
}

impl RuleHandler {
    pub fn new(compiled: Arc<RwLock<Compiled>>) -> Self {
        Self {
            compiled,
            pending: Vec::new(),
            applied: None,
        }
    }
}

impl HttpHandler for RuleHandler {
    async fn handle_request(
        &mut self,
        _ctx: &HttpContext,
        req: Request<Body>,
    ) -> RequestOrResponse {
        // CONNECT is the tunnel-setup request (authority-form, no real URL);
        // hudsucker hands it to us before MITM. Never apply rules to it — that
        // would break the tunnel and match against a bogus `host:443` URL.
        if req.method() == Method::CONNECT {
            return RequestOrResponse::Request(req);
        }

        // Built-in inspector: visiting http://reheader.echo shows the request
        // headers ReHeader sends (with injections applied), since a browser's
        // own DevTools can't reveal proxy-added request headers.
        if request_host(&req).as_deref() == Some(ECHO_HOST) {
            return RequestOrResponse::Response(echo_page(&req, &self.compiled));
        }

        let url = full_url(&req);
        let (plan, verify) = match self.compiled.read() {
            Ok(c) => (c.plan_for(&url), c.verify),
            Err(_) => return RequestOrResponse::Request(req),
        };

        if let Some(target) = plan.redirect_to {
            let resp = Response::builder()
                .status(StatusCode::TEMPORARY_REDIRECT)
                .header(LOCATION, target)
                .body(Body::empty())
                .expect("valid redirect response");
            return RequestOrResponse::Response(resp);
        }

        self.applied = verify.then(|| verify_summary(&plan));
        self.pending = plan.response_headers;
        let (mut parts, body) = req.into_parts();
        apply(&mut parts.headers, &plan.request_headers);
        RequestOrResponse::Request(Request::from_parts(parts, body))
    }

    async fn handle_response(
        &mut self,
        _ctx: &HttpContext,
        mut res: Response<Body>,
    ) -> Response<Body> {
        apply(res.headers_mut(), &self.pending);
        if let Some(summary) = &self.applied {
            if let Ok(value) = HeaderValue::from_str(summary) {
                res.headers_mut()
                    .insert(HeaderName::from_static("x-reheader-applied"), value);
            }
        }
        res
    }
}

/// Build the `X-ReHeader-Applied` value: what request and response header
/// changes this exchange applied. `req[]`/`resp[]` empty means no rule matched
/// (e.g. a URL filter excluded this request) — useful for debugging filters.
fn verify_summary(plan: &reheader_core::rules::Plan) -> String {
    fn fmt(ops: &[HeaderAction]) -> String {
        ops.iter()
            .map(|op| match op.op {
                HeaderOp::Set => format!("{}={}", op.name, op.value),
                HeaderOp::Remove => format!("-{}", op.name),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
    format!(
        "req[{}] resp[{}]",
        fmt(&plan.request_headers),
        fmt(&plan.response_headers)
    )
}

// WebSockets are tunneled through unchanged.
impl WebSocketHandler for RuleHandler {
    async fn handle_message(&mut self, _ctx: &WebSocketContext, msg: Message) -> Option<Message> {
        Some(msg)
    }
}

fn apply(headers: &mut HeaderMap, ops: &[HeaderAction]) {
    for op in ops {
        let Ok(name) = HeaderName::from_bytes(op.name.as_bytes()) else {
            continue;
        };
        match op.op {
            HeaderOp::Remove => {
                headers.remove(&name);
            }
            HeaderOp::Set => {
                if let Ok(value) = HeaderValue::from_str(&op.value) {
                    headers.insert(name, value);
                }
            }
        }
    }
}

fn request_host(req: &Request<Body>) -> Option<String> {
    if let Some(h) = req.uri().host() {
        return Some(h.to_ascii_lowercase());
    }
    req.headers()
        .get(HOST)
        .and_then(|h| h.to_str().ok())
        .map(|s| s.split(':').next().unwrap_or(s).to_ascii_lowercase())
}

/// Build the inspector page. Without `?url=`, shows every configured change
/// (filters ignored). With `?url=<target>`, shows what would apply to that
/// specific URL, respecting filters.
fn echo_page(req: &Request<Body>, compiled: &std::sync::Arc<std::sync::RwLock<Compiled>>) -> Response<Body> {
    let target = req.uri().query().and_then(|q| query_param(q, "url"));
    let (scope, plan) = match compiled.read() {
        Ok(c) => match &target {
            Some(u) => (format!("requests matching <code>{}</code>", html_escape(u)), c.plan_for(u)),
            None => ("all requests (URL filters ignored here)".to_string(), c.global_plan()),
        },
        Err(_) => ("rules unavailable".to_string(), Plan::default()),
    };
    let html = render_echo(&scope, &plan, req.headers());
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "text/html; charset=utf-8")
        .header("cache-control", "no-store")
        .body(Body::from(html))
        .expect("echo response builds")
}

fn render_echo(scope: &str, plan: &Plan, incoming: &HeaderMap) -> String {
    let mut changes = String::new();
    for op in &plan.request_headers {
        match op.op {
            HeaderOp::Set => changes.push_str(&format!(
                "<tr><td class=add>ADD</td><td>{}</td><td>{}</td></tr>",
                html_escape(&op.name), html_escape(&op.value)
            )),
            HeaderOp::Remove => changes.push_str(&format!(
                "<tr><td class=rem>REMOVE</td><td>{}</td><td>—</td></tr>",
                html_escape(&op.name)
            )),
        }
    }
    if plan.request_headers.is_empty() {
        changes.push_str("<tr><td colspan=3 class=none>No request-header changes apply here.</td></tr>");
    }

    // The actual outgoing request headers for THIS page load, with changes applied.
    let mut merged: Vec<(String, String, &'static str)> = Vec::new();
    for (name, value) in incoming {
        let n = name.as_str().to_string();
        if plan.request_headers.iter().any(|o| o.op == HeaderOp::Remove && o.name.eq_ignore_ascii_case(&n)) {
            continue;
        }
        if plan.request_headers.iter().any(|o| o.op == HeaderOp::Set && o.name.eq_ignore_ascii_case(&n)) {
            continue; // shown below as ReHeader's value
        }
        merged.push((n, value.to_str().unwrap_or("").to_string(), ""));
    }
    for op in &plan.request_headers {
        if op.op == HeaderOp::Set {
            merged.push((op.name.clone(), op.value.clone(), "hl"));
        }
    }
    merged.sort_by(|a, b| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()));
    let outgoing: String = merged
        .iter()
        .map(|(n, v, cls)| format!(
            "<tr class={}><td>{}</td><td>{}</td></tr>",
            cls, html_escape(n), html_escape(v)
        ))
        .collect();

    format!(
        r#"<!doctype html><html><head><meta charset=utf-8><title>ReHeader inspector</title>
<style>
body{{font:14px/1.5 -apple-system,Segoe UI,Roboto,sans-serif;background:#0f1116;color:#e6e9ee;max-width:820px;margin:32px auto;padding:0 16px}}
h1{{font-size:18px}} h2{{font-size:14px;margin:22px 0 8px;color:#8b93a1;text-transform:uppercase;letter-spacing:.5px}}
.tag{{color:#6d5ef2;font-weight:700}}
table{{width:100%;border-collapse:collapse;font-size:13px}}
td{{padding:6px 8px;border-bottom:1px solid #2d323c;vertical-align:top;word-break:break-all}}
tr.hl td{{background:#6d5ef21f}} tr.hl td:first-child::after{{content:' ← added by ReHeader';color:#6d5ef2;font-size:11px}}
td.add{{color:#10b981;font-weight:700}} td.rem{{color:#ef4444;font-weight:700}} td.none{{color:#8b93a1}}
.note{{color:#8b93a1;font-size:12.5px;margin-top:18px;border-top:1px solid #2d323c;padding-top:12px}}
code{{background:#1e2128;padding:1px 5px;border-radius:4px}}
</style></head><body>
<h1><span class=tag>ReHeader</span> request inspector</h1>
<p>Showing header changes for {scope}.</p>
<h2>Changes ReHeader applies</h2>
<table>{changes}</table>
<h2>Outgoing request headers (this page)</h2>
<p style="color:#8b93a1;font-size:12.5px;margin:0 0 8px">This is what ReHeader actually sends upstream for this request — the highlighted rows are your injected headers. The server receives exactly this.</p>
<table>{outgoing}</table>
<p class=note>Your browser's DevTools shows the headers the browser sent to ReHeader, so it can't display request headers ReHeader adds afterward — that's why they appear here instead. To check a specific site (respecting URL filters), visit <code>http://reheader.echo/?url=https://your-site.com/path</code>. Response-header changes are visible normally in DevTools.</p>
</body></html>"#,
        scope = scope,
        changes = changes,
        outgoing = outgoing,
    )
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        if k == key {
            Some(percent_decode(v))
        } else {
            None
        }
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.replace('+', " ");
    let bytes = bytes.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&String::from_utf8_lossy(&bytes[i + 1..i + 3]), 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Reconstruct the absolute URL used for filter/redirect matching, normalized
/// to what a user expects to write: `scheme://host/path?query`, with the
/// default port (:443 / :80) stripped. hudsucker hands intercepted HTTPS
/// requests in absolute-form with an explicit `:443`, which we drop so filters
/// like `^https://host/path` match.
fn full_url(req: &Request<Body>) -> String {
    let uri = req.uri();
    let scheme = uri.scheme_str().unwrap_or("https");
    let authority = uri
        .authority()
        .map(|a| a.as_str().to_string())
        .or_else(|| {
            req.headers()
                .get(HOST)
                .and_then(|h| h.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let default_port = if scheme == "https" { ":443" } else { ":80" };
    let host = authority.strip_suffix(default_port).unwrap_or(&authority);
    let path = uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
    format!("{scheme}://{host}{path}")
}
