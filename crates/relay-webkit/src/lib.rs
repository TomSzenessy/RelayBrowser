//! WebKit engine layer: content blocking, tab-sleep policy, and download
//! naming (portable, tested everywhere) plus the WKWebView implementation
//! (macOS only).

pub mod content_rules;
pub mod downloads;
pub mod float;
pub mod sleep;

#[cfg(target_os = "macos")]
pub mod mac;
