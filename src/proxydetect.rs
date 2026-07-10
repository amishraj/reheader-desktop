//! Pure parsers for system-proxy auto-detection. No I/O here so they're fast to
//! unit-test; the binary does the actual registry reads / process calls / PAC
//! fetches and feeds the raw text into these.

use regex::Regex;

/// What a platform proxy config points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Detected {
    /// A concrete `host:port` upstream proxy.
    Server(String),
    /// A proxy auto-config (PAC) URL that must be fetched and scanned.
    Pac(String),
}

/// Extract the first usable `host:port` from a PAC file's body. PAC scripts are
/// JavaScript, but the proxy targets appear as `PROXY host:port` / `HTTPS
/// host:port` string literals, which is enough to auto-fill the field.
pub fn scan_pac(body: &str) -> Option<String> {
    let re = Regex::new(r#"(?i)\b(?:PROXY|HTTPS)\s+([A-Za-z0-9_.\-]+:\d{1,5})"#).ok()?;
    re.captures(body)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Parse the Windows `ProxyServer` registry string. It's either a bare
/// `host:port`, or per-protocol pairs like `http=h:80;https=h:443;socks=h:1080`.
/// Prefer the HTTPS entry (most traffic), then HTTP, then any.
pub fn parse_win_proxy_server(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    if !raw.contains('=') {
        return Some(raw.to_string());
    }
    let mut http = None;
    let mut https = None;
    let mut other = None;
    for part in raw.split(';') {
        let Some((scheme, addr)) = part.split_once('=') else {
            continue;
        };
        let addr = addr.trim().to_string();
        if addr.is_empty() {
            continue;
        }
        match scheme.trim().to_ascii_lowercase().as_str() {
            "https" => https = Some(addr),
            "http" => http = Some(addr),
            _ => {
                other.get_or_insert(addr);
            }
        }
    }
    https.or(http).or(other)
}

/// Parse `scutil --proxy` output on macOS into a proxy server or PAC URL.
pub fn parse_scutil(text: &str) -> Option<Detected> {
    let field = |key: &str| -> Option<String> {
        text.lines().find_map(|line| {
            let (k, v) = line.split_once(':')?;
            if k.trim() == key {
                Some(v.trim().to_string())
            } else {
                None
            }
        })
    };
    let enabled = |key: &str| field(key).as_deref() == Some("1");

    if enabled("HTTPSEnable") {
        if let (Some(h), Some(p)) = (field("HTTPSProxy"), field("HTTPSPort")) {
            return Some(Detected::Server(format!("{h}:{p}")));
        }
    }
    if enabled("HTTPEnable") {
        if let (Some(h), Some(p)) = (field("HTTPProxy"), field("HTTPPort")) {
            return Some(Detected::Server(format!("{h}:{p}")));
        }
    }
    if enabled("ProxyAutoConfigEnable") {
        if let Some(url) = field("ProxyAutoConfigURLString") {
            if !url.is_empty() {
                return Some(Detected::Pac(url));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_pac_finds_proxy() {
        let pac = r#"
            function FindProxyForURL(url, host) {
              if (isPlainHostName(host)) return "DIRECT";
              return "PROXY proxy.corp.example.com:8080; DIRECT";
            }"#;
        assert_eq!(scan_pac(pac).as_deref(), Some("proxy.corp.example.com:8080"));
    }

    #[test]
    fn scan_pac_handles_https_keyword_and_none() {
        assert_eq!(scan_pac("return 'HTTPS gw.corp:443';").as_deref(), Some("gw.corp:443"));
        assert_eq!(scan_pac("return 'DIRECT';"), None);
    }

    #[test]
    fn win_proxy_bare_and_pairs() {
        assert_eq!(parse_win_proxy_server("10.1.2.3:8080").as_deref(), Some("10.1.2.3:8080"));
        assert_eq!(
            parse_win_proxy_server("http=h:80;https=s:443;socks=x:1080").as_deref(),
            Some("s:443")
        );
        assert_eq!(parse_win_proxy_server("http=only:80").as_deref(), Some("only:80"));
        assert_eq!(parse_win_proxy_server(""), None);
    }

    #[test]
    fn scutil_prefers_https_then_pac() {
        let with_https = "HTTPSEnable : 1\nHTTPSProxy : 10.0.0.1\nHTTPSPort : 3128\n";
        assert_eq!(parse_scutil(with_https), Some(Detected::Server("10.0.0.1:3128".into())));

        let pac = "HTTPSEnable : 0\nProxyAutoConfigEnable : 1\nProxyAutoConfigURLString : http://wpad/wpad.dat\n";
        assert_eq!(parse_scutil(pac), Some(Detected::Pac("http://wpad/wpad.dat".into())));

        assert_eq!(parse_scutil("HTTPSEnable : 0\nHTTPEnable : 0\n"), None);
    }
}
