//! The hudsucker HTTP handler: looks up the compiled ruleset per request and
//! applies header edits / redirects. hudsucker clones the handler per request
//! (the `&mut self` methods guarantee exclusive access), so `pending` holds
//! this exchange's response-header ops safely even under HTTP/2 multiplexing.

use std::sync::{Arc, RwLock};

use http::{
    header::{HeaderName, HeaderValue, HOST, LOCATION},
    HeaderMap, Method, Request, Response, StatusCode,
};
use hudsucker::{
    tokio_tungstenite::tungstenite::Message, Body, HttpContext, HttpHandler, RequestOrResponse,
    WebSocketContext, WebSocketHandler,
};
use reheader_core::rules::{Compiled, HeaderAction, HeaderOp};

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
