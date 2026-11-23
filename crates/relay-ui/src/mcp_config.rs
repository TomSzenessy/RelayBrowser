//! Exact MCP client configs for the setup board. Text only: no config file is
//! written, so a wrong guess cannot corrupt a client's settings.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Client {
    ClaudeCode,
    Codex,
    OpenCode,
    Cursor,
    Zed,
}

impl Client {
    pub const ALL: [Client; 5] = [Client::ClaudeCode, Client::Codex, Client::OpenCode, Client::Cursor, Client::Zed];

    pub fn id(self) -> &'static str {
        match self {
            Client::ClaudeCode => "claude-code",
            Client::Codex => "codex",
            Client::OpenCode => "opencode",
            Client::Cursor => "cursor",
            Client::Zed => "zed",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Client::ClaudeCode => "Claude Code",
            Client::Codex => "Codex",
            Client::OpenCode => "OpenCode",
            Client::Cursor => "Cursor",
            Client::Zed => "Zed",
        }
    }

    pub fn target(self) -> &'static str {
        match self {
            Client::ClaudeCode => "run once in a terminal",
            Client::Codex => "~/.codex/config.toml",
            Client::OpenCode => "opencode.json",
            Client::Cursor => "~/.cursor/mcp.json",
            Client::Zed => "~/.config/zed/settings.json",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claudecode" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            "opencode" | "open-code" => Some(Self::OpenCode),
            "cursor" => Some(Self::Cursor),
            "zed" => Some(Self::Zed),
            _ => None,
        }
    }

    /// A copy-pasteable snippet. `command` is the absolute Relay executable or
    /// `relay` on PATH.
    pub fn snippet(self, command: &str) -> String {
        match self {
            Client::ClaudeCode => format!("claude mcp add relay -- {command} mcp"),
            Client::Codex => format!("[mcp_servers.relay]\ncommand = {command:?}\nargs = [\"mcp\"]\n"),
            Client::OpenCode => format!(
                "{{\n  \"mcp\": {{\n    \"relay\": {{\n      \"type\": \"local\",\n      \"command\": [{command:?}, \"mcp\"],\n      \"enabled\": true\n    }}\n  }}\n}}\n"
            ),
            Client::Cursor => format!(
                "{{\n  \"mcpServers\": {{\n    \"relay\": {{\n      \"command\": {command:?},\n      \"args\": [\"mcp\"]\n    }}\n  }}\n}}\n"
            ),
            Client::Zed => format!(
                "{{\n  \"context_servers\": {{\n    \"relay\": {{\n      \"command\": {{\n        \"path\": {command:?},\n        \"args\": [\"mcp\"]\n      }}\n    }}\n  }}\n}}\n"
            ),
        }
    }
}

pub fn relay_command() -> String {
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "relay".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_client_snippet_starts_relay_mcp() {
        for client in Client::ALL {
            let s = client.snippet("/Applications/Relay.app/Contents/MacOS/relay");
            assert!(s.contains("mcp"), "{}: {s}", client.name());
            assert!(s.contains("/Applications/Relay.app/Contents/MacOS/relay"), "{}: {s}", client.name());
            assert!(!client.target().is_empty());
            assert_eq!(Client::parse(client.id()), Some(client));
        }
        assert!(Client::Cursor.snippet("relay").contains("mcpServers"));
        assert!(Client::Codex.snippet("relay").contains("[mcp_servers.relay]"));
        assert!(Client::Zed.snippet("relay").contains("context_servers"));
        assert_eq!(Client::parse("nope"), None);
    }
}
