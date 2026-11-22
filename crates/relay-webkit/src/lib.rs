//! WebKit engine layer: content blocking and tab-sleep policy (portable,
//! tested everywhere) plus the WKWebView implementation (macOS only).

pub mod content_rules;
pub mod sleep;

#[cfg(target_os = "macos")]
pub mod mac;
