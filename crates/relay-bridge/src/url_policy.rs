//! The one URL policy shared by page `window.open` and agent navigation.
//! Case-insensitive http(s), plus exactly `about:blank`.

use serde::{Deserialize, Serialize};

/// What an attached agent may do on one site. `Allow` is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AgentPolicy {
    #[default]
    Allow,
    ReadOnly,
    Block,
}

impl AgentPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentPolicy::Allow => "allow",
            AgentPolicy::ReadOnly => "read_only",
            AgentPolicy::Block => "block",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "allow" => Some(AgentPolicy::Allow),
            "read_only" | "read-only" | "readonly" => Some(AgentPolicy::ReadOnly),
            "block" | "blocked" => Some(AgentPolicy::Block),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AgentPolicy::Allow => "Allow",
            AgentPolicy::ReadOnly => "Read only",
            AgentPolicy::Block => "Block",
        }
    }

    pub fn next(self) -> Self {
        match self {
            AgentPolicy::Allow => AgentPolicy::ReadOnly,
            AgentPolicy::ReadOnly => AgentPolicy::Block,
            AgentPolicy::Block => AgentPolicy::Allow,
        }
    }
}

/// Lowercase scheme://authority host, without `www.`; `None` for non-web URLs.
pub fn host_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?;
    let host = authority.split(':').next()?.trim().to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    (!host.is_empty()).then_some(host)
}

pub fn is_http_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

pub fn is_openable_url(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    is_http_url(&lower) || lower == "about:blank"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_table_for_page_and_agent_navigation() {
        for (url, ok) in [
            ("http://example.com", true),
            ("https://example.com/x", true),
            ("HTTPS://EXAMPLE.COM", true),
            ("about:blank", true),
            ("about:blankx", false),
            ("about:blank/", false),
            ("file:///etc/hosts", false),
            ("javascript:alert(1)", false),
            ("data:text/html,x", false),
            ("view-source:https://x", false),
            ("", false),
        ] {
            assert_eq!(is_openable_url(url), ok, "{url:?}");
        }
        assert!(is_http_url("HTTP://x"));
        assert!(!is_http_url("about:blank"));
        assert_eq!(host_of("https://www.Example.com:8443/x"), Some("example.com".into()));
        assert_eq!(host_of("about:blank"), None);
        assert_eq!(AgentPolicy::parse("read-only"), Some(AgentPolicy::ReadOnly));
        assert_eq!(AgentPolicy::Block.next(), AgentPolicy::Allow);
    }
}
