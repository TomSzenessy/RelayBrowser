//! The one URL policy shared by page `window.open` and agent navigation.
//! Case-insensitive http(s), plus exactly `about:blank`.

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
    }
}
