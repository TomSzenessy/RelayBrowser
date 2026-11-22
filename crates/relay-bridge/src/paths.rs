//! Where Relay keeps local state. One owner so every crate agrees.

use std::path::PathBuf;

/// `$RELAY_HOME`, else `~/Library/Application Support/Relay` on macOS,
/// else `$XDG_DATA_HOME/relay` (or `~/.local/share/relay`).
pub fn relay_home() -> PathBuf {
    if let Some(p) = std::env::var_os("RELAY_HOME") {
        return PathBuf::from(p);
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Relay")
    } else if let Some(x) = std::env::var_os("XDG_DATA_HOME") {
        PathBuf::from(x).join("relay")
    } else {
        home.join(".local/share/relay")
    }
}

/// The local MCP socket the running browser listens on.
pub fn mcp_socket() -> PathBuf {
    relay_home().join("mcp.sock")
}
