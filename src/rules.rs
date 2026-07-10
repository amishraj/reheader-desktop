//! State model + rule compilation. The JSON shape is deliberately compatible
//! with the ReHeader browser extension so profiles are portable in both
//! directions. Header rewriting is decided here; the proxy just applies the
//! returned `Plan`.

use regex::Regex;
use serde::{Deserialize, Serialize};

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Header {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub comment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Redirect {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
}

/// Filter type. `include`/`exclude` match the request URL by regex; `types`
/// is accepted for extension-format compatibility but ignored by the proxy
/// (resource-type is a browser concept the proxy can't reconstruct reliably).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filter {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub title: String,
    #[serde(default = "default_color")]
    pub color: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub headers: Vec<Header>,
    #[serde(default, rename = "respHeaders")]
    pub resp_headers: Vec<Header>,
    #[serde(default)]
    pub redirects: Vec<Redirect>,
    #[serde(default)]
    pub filters: Vec<Filter>,
}

fn default_color() -> String {
    "#6d5ef2".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppState {
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default, rename = "selectedProfile")]
    pub selected_profile: usize,
    #[serde(default)]
    pub paused: bool,
    #[serde(default = "default_theme")]
    pub theme: String,
}

fn default_theme() -> String {
    "auto".to_string()
}

pub fn default_state() -> AppState {
    AppState {
        profiles: vec![Profile {
            title: "Profile 1".into(),
            color: default_color(),
            enabled: true,
            headers: vec![Header {
                enabled: true,
                name: String::new(),
                value: String::new(),
                comment: String::new(),
            }],
            resp_headers: vec![],
            redirects: vec![],
            filters: vec![],
        }],
        selected_profile: 0,
        paused: false,
        theme: default_theme(),
    }
}

// --- Compiled form -------------------------------------------------------
// State is compiled once per change into regex-precompiled profiles, so the
// hot path (one lookup per request) never recompiles a regex.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderOp {
    Set,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderAction {
    pub op: HeaderOp,
    pub name: String,
    pub value: String,
}

struct CompiledRedirect {
    from: Regex,
    to: String,
}

struct CompiledProfile {
    includes: Vec<Regex>,
    excludes: Vec<Regex>,
    req_headers: Vec<HeaderAction>,
    resp_headers: Vec<HeaderAction>,
    redirects: Vec<CompiledRedirect>,
}

/// The precompiled, ready-to-serve ruleset.
pub struct Compiled {
    paused: bool,
    profiles: Vec<CompiledProfile>,
    /// Regex sources that failed to compile, surfaced to the UI.
    pub errors: Vec<String>,
    pub active_count: usize,
}

/// What to do with a single request, decided by matching every enabled profile.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub redirect_to: Option<String>,
    pub request_headers: Vec<HeaderAction>,
    pub response_headers: Vec<HeaderAction>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.redirect_to.is_none()
            && self.request_headers.is_empty()
            && self.response_headers.is_empty()
    }
}

// RFC 7230 token characters — what a valid header name may contain.
pub fn is_valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+'
                        | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
                )
        })
}

fn header_actions(rows: &[Header], errors: &mut Vec<String>) -> Vec<HeaderAction> {
    let mut out = Vec::new();
    for row in rows {
        if !row.enabled {
            continue;
        }
        let name = row.name.trim();
        if name.is_empty() {
            continue;
        }
        if !is_valid_header_name(name) {
            errors.push(format!("Invalid header name: {name:?}"));
            continue;
        }
        if row.value.is_empty() {
            out.push(HeaderAction {
                op: HeaderOp::Remove,
                name: name.to_string(),
                value: String::new(),
            });
        } else {
            out.push(HeaderAction {
                op: HeaderOp::Set,
                name: name.to_string(),
                value: row.value.clone(),
            });
        }
    }
    out
}

fn compile_regexes(profile: &Profile, kind: &str, errors: &mut Vec<String>) -> Vec<Regex> {
    let mut out = Vec::new();
    for f in &profile.filters {
        if !f.enabled || f.kind != kind || f.value.is_empty() {
            continue;
        }
        match Regex::new(&f.value) {
            Ok(re) => out.push(re),
            Err(e) => errors.push(format!("Bad {kind} filter {:?}: {e}", f.value)),
        }
    }
    out
}

/// Compile the whole state into a `Compiled` ruleset once.
pub fn compile(state: &AppState) -> Compiled {
    let mut errors = Vec::new();
    let mut profiles = Vec::new();
    let mut active_count = 0usize;

    for profile in &state.profiles {
        if !profile.enabled {
            continue;
        }
        let includes = compile_regexes(profile, "include", &mut errors);
        let excludes = compile_regexes(profile, "exclude", &mut errors);

        // If the user declared include filters but every one failed to
        // compile, skip the profile entirely rather than apply it everywhere.
        let include_declared = profile
            .filters
            .iter()
            .any(|f| f.enabled && f.kind == "include" && !f.value.is_empty());
        if include_declared && includes.is_empty() {
            errors.push(format!(
                "Profile {:?}: all include filters invalid — profile disabled",
                profile.title
            ));
            continue;
        }

        let req_headers = header_actions(&profile.headers, &mut errors);
        let resp_headers = header_actions(&profile.resp_headers, &mut errors);

        let mut redirects = Vec::new();
        for r in &profile.redirects {
            if !r.enabled || r.from.is_empty() || r.to.is_empty() {
                continue;
            }
            match Regex::new(&r.from) {
                Ok(re) => redirects.push(CompiledRedirect {
                    from: re,
                    to: r.to.clone(),
                }),
                Err(e) => errors.push(format!("Bad redirect {:?}: {e}", r.from)),
            }
        }

        active_count += req_headers.len() + resp_headers.len() + redirects.len();
        profiles.push(CompiledProfile {
            includes,
            excludes,
            req_headers,
            resp_headers,
            redirects,
        });
    }

    Compiled {
        paused: state.paused,
        profiles,
        errors,
        active_count,
    }
}

impl Compiled {
    /// Decide what to do with a request to `url`. Later profiles override
    /// earlier ones on conflicting header names (last-write-wins), mirroring
    /// the extension's higher-priority-profile behavior.
    pub fn plan_for(&self, url: &str) -> Plan {
        let mut plan = Plan::default();
        if self.paused {
            return plan;
        }

        for profile in &self.profiles {
            if !profile.matches(url) {
                continue;
            }

            // First matching redirect wins and short-circuits — a redirected
            // request goes to a new URL, so downstream header edits for the
            // old URL are moot.
            if plan.redirect_to.is_none() {
                for r in &profile.redirects {
                    if let Some(caps) = r.from.captures(url) {
                        plan.redirect_to = Some(expand_backrefs(&r.to, &caps));
                        break;
                    }
                }
            }

            merge_headers(&mut plan.request_headers, &profile.req_headers);
            merge_headers(&mut plan.response_headers, &profile.resp_headers);
        }

        plan
    }

    pub fn active_count(&self) -> usize {
        self.active_count
    }
}

impl CompiledProfile {
    fn matches(&self, url: &str) -> bool {
        if self.excludes.iter().any(|re| re.is_match(url)) {
            return false;
        }
        if self.includes.is_empty() {
            return true;
        }
        self.includes.iter().any(|re| re.is_match(url))
    }
}

/// Append actions, replacing any existing action for the same header name
/// (case-insensitive) so the latest profile wins.
fn merge_headers(into: &mut Vec<HeaderAction>, from: &[HeaderAction]) {
    for a in from {
        into.retain(|e| !e.name.eq_ignore_ascii_case(&a.name));
        into.push(a.clone());
    }
}

/// Expand `\0`..`\9` capture-group backreferences in a redirect target, using
/// the same `\N` syntax as the browser extension (and Chrome's DNR). `\\`
/// yields a literal backslash; everything else is copied verbatim, so literal
/// `$` in URLs is never misinterpreted.
fn expand_backrefs(template: &str, caps: &regex::Captures) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            Some(&'\\') => {
                out.push('\\');
                chars.next();
            }
            Some(&d) if d.is_ascii_digit() => {
                let idx = (d as u8 - b'0') as usize;
                if let Some(m) = caps.get(idx) {
                    out.push_str(m.as_str());
                }
                chars.next();
            }
            _ => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(name: &str, value: &str) -> Header {
        Header {
            enabled: true,
            name: name.into(),
            value: value.into(),
            comment: String::new(),
        }
    }

    fn profile(headers: Vec<Header>) -> Profile {
        Profile {
            title: "T".into(),
            color: "#fff".into(),
            enabled: true,
            headers,
            resp_headers: vec![],
            redirects: vec![],
            filters: vec![],
        }
    }

    fn state(p: Profile) -> AppState {
        AppState {
            profiles: vec![p],
            selected_profile: 0,
            paused: false,
            theme: "auto".into(),
        }
    }

    fn find<'a>(v: &'a [HeaderAction], name: &str) -> Option<&'a HeaderAction> {
        v.iter().find(|a| a.name.eq_ignore_ascii_case(name))
    }

    #[test]
    fn sets_and_removes_request_headers() {
        let p = profile(vec![header("X-Api-Key", "abc"), header("Referer", "")]);
        let c = compile(&state(p));
        let plan = c.plan_for("https://example.com/");
        let set = find(&plan.request_headers, "X-Api-Key").unwrap();
        assert_eq!(set.op, HeaderOp::Set);
        assert_eq!(set.value, "abc");
        let rm = find(&plan.request_headers, "Referer").unwrap();
        assert_eq!(rm.op, HeaderOp::Remove);
        assert_eq!(c.active_count(), 2);
    }

    #[test]
    fn response_headers_are_separate() {
        let mut p = profile(vec![]);
        p.resp_headers = vec![header("Access-Control-Allow-Origin", "*")];
        let c = compile(&state(p));
        let plan = c.plan_for("https://example.com/");
        assert!(plan.request_headers.is_empty());
        assert_eq!(
            find(&plan.response_headers, "Access-Control-Allow-Origin")
                .unwrap()
                .value,
            "*"
        );
    }

    #[test]
    fn disabled_rows_and_invalid_names_skipped() {
        let mut p = profile(vec![
            Header { enabled: false, ..header("X-A", "1") },
            header("", "x"),
            header("bad name", "x"),
        ]);
        p.headers.push(header("  ", "y"));
        let c = compile(&state(p));
        assert_eq!(c.active_count(), 0);
        assert!(c.plan_for("https://x/").request_headers.is_empty());
    }

    #[test]
    fn paused_and_disabled_produce_nothing() {
        let mut s = state(profile(vec![header("X-A", "1")]));
        s.paused = true;
        assert!(compile(&s).plan_for("https://x/").is_empty());
        s.paused = false;
        s.profiles[0].enabled = false;
        assert!(compile(&s).plan_for("https://x/").is_empty());
    }

    #[test]
    fn include_filter_gates_matching() {
        let mut p = profile(vec![header("X-A", "1")]);
        p.filters = vec![Filter {
            enabled: true,
            kind: "include".into(),
            value: r"://api\.example\.com/".into(),
        }];
        let c = compile(&state(p));
        assert!(c.plan_for("https://other.com/").request_headers.is_empty());
        assert_eq!(c.plan_for("https://api.example.com/v1").request_headers.len(), 1);
    }

    #[test]
    fn exclude_filter_blocks_matching() {
        let mut p = profile(vec![header("X-A", "1")]);
        p.filters = vec![Filter {
            enabled: true,
            kind: "exclude".into(),
            value: r"\.png$".into(),
        }];
        let c = compile(&state(p));
        assert!(c.plan_for("https://x/a.png").request_headers.is_empty());
        assert!(!c.plan_for("https://x/a.js").request_headers.is_empty());
    }

    #[test]
    fn all_invalid_includes_disable_profile_not_widen() {
        let mut p = profile(vec![header("X-A", "1")]);
        p.filters = vec![Filter {
            enabled: true,
            kind: "include".into(),
            value: "(unclosed".into(),
        }];
        let c = compile(&state(p));
        assert!(c.plan_for("https://anything/").request_headers.is_empty());
        assert!(!c.errors.is_empty());
    }

    #[test]
    fn redirect_with_backrefs() {
        let mut p = profile(vec![]);
        p.redirects = vec![Redirect {
            enabled: true,
            from: r"^https://prod\.example\.com/(.*)$".into(),
            to: r"http://localhost:3000/\1".into(),
        }];
        let c = compile(&state(p));
        let plan = c.plan_for("https://prod.example.com/api/users?id=5");
        assert_eq!(
            plan.redirect_to.as_deref(),
            Some("http://localhost:3000/api/users?id=5")
        );
    }

    #[test]
    fn expand_backrefs_handles_literals() {
        let re = Regex::new(r"^(a)(b)$").unwrap();
        let caps = re.captures("ab").unwrap();
        // \1 \2 expand; literal $ and unknown groups are preserved/empty.
        assert_eq!(expand_backrefs(r"x\1y\2z$5", &caps), "xaybz$5");
        assert_eq!(expand_backrefs(r"a\\b", &caps), r"a\b");
    }

    #[test]
    fn later_profile_overrides_header() {
        let mut s = state(profile(vec![header("X-Env", "staging")]));
        s.profiles.push(profile(vec![header("x-env", "prod")]));
        let c = compile(&s);
        let plan = c.plan_for("https://x/");
        let hits: Vec<_> = plan
            .request_headers
            .iter()
            .filter(|a| a.name.eq_ignore_ascii_case("x-env"))
            .collect();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].value, "prod");
    }

    #[test]
    fn deserializes_extension_export_format() {
        // Mirrors what the ReHeader browser extension exports.
        let json = r##"[{
            "title": "From Extension",
            "color": "#0ea5e9",
            "enabled": true,
            "headers": [{"enabled": true, "name": "X-Token", "value": "s", "comment": "n"}],
            "respHeaders": [{"enabled": true, "name": "X-R", "value": "1"}],
            "redirects": [{"enabled": true, "from": "a", "to": "b"}],
            "filters": [{"enabled": true, "type": "include", "value": "example"}]
        }]"##;
        let profiles: Vec<Profile> = serde_json::from_str(json).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].resp_headers[0].name, "X-R");
        assert_eq!(profiles[0].filters[0].kind, "include");
        assert_eq!(profiles[0].headers[0].comment, "n");
    }

    #[test]
    fn missing_fields_get_defaults() {
        // A minimal profile with only headers should fill enabled/color/etc.
        let p: Profile = serde_json::from_str(r#"{"headers":[{"name":"A","value":"1"}]}"#).unwrap();
        assert!(p.enabled);
        assert!(p.headers[0].enabled);
        assert_eq!(p.color, "#6d5ef2");
    }
}
