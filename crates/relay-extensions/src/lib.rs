//! Daily-driver plumbing: Chrome migration, extension packaging and loading,
//! Chrome API shims, and the local SQLite profile store.

pub mod ai;
pub mod bookmarks_html;
pub mod chrome_import;
pub mod safari;
pub mod crx;
pub mod login;
pub mod manifest;
pub mod store;

#[cfg(target_os = "macos")]
pub mod mac;

/// Polyfills injected into extension contexts (chrome.sidePanel, chrome.offscreen, ...).
pub const CHROME_SHIMS_JS: &str = include_str!("../../../js/chrome-shims.js");

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("sqlite: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("zip: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("csv: {0}")]
    Csv(#[from] csv::Error),
    #[error("http: {0}")]
    Http(String),
    #[error("{0}")]
    Invalid(String),
}

/// Bounded badge text and unread state read from `WKWebExtensionAction`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionBadge {
    pub text: String,
    pub unread: bool,
}

/// Trim, strip control characters, and bound toolbar badge text to 4 chars.
/// Empty or whitespace-only badges return `None` so no badge is drawn.
pub fn sanitize_badge_text(raw: &str) -> Option<String> {
    let cleaned: String = raw.chars().filter(|c| !c.is_control()).collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return None;
    }
    let count = trimmed.chars().count();
    if count <= 4 {
        Some(trimmed.to_string())
    } else {
        let head: String = trimmed.chars().take(3).collect();
        Some(format!("{head}…"))
    }
}

#[cfg(test)]
mod badge_tests {
    use super::*;

    #[test]
    fn sanitize_badge_text_bounds_and_rejects_blank_inputs() {
        assert_eq!(sanitize_badge_text(""), None);
        assert_eq!(sanitize_badge_text("   \n\t "), None);
        assert_eq!(sanitize_badge_text("\u{0007}"), None);
        assert_eq!(sanitize_badge_text(" 7 "), Some("7".into()));
        assert_eq!(sanitize_badge_text("999+"), Some("999+".into()));
        assert_eq!(sanitize_badge_text("123456789"), Some("123…".into()));
    }
}
