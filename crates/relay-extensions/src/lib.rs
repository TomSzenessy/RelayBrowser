//! Daily-driver plumbing: Chrome migration, extension packaging and loading,
//! Chrome API shims, and the local SQLite profile store.

pub mod chrome_import;
pub mod crx;
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

pub type Result<T> = std::result::Result<T, Error>;
