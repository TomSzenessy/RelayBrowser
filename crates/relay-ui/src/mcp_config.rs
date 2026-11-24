//! Exact MCP client configs for the setup board. Read-only: detects installed
//! clients on disk so the Connect board highlights them, but never modifies
//! external config files so a wrong guess cannot corrupt a client's settings.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Client {
    ClaudeCode,
    Codex,
    OpenCode,
    Cursor,
    Zed,
    VsCode,
    Windsurf,
    ClaudeDesktop,
    GeminiCli,
}

impl Client {
    pub const ALL: [Client; 9] = [
        Client::ClaudeCode,
        Client::Codex,
        Client::OpenCode,
        Client::Cursor,
        Client::Zed,
        Client::VsCode,
        Client::Windsurf,
        Client::ClaudeDesktop,
        Client::GeminiCli,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Client::ClaudeCode => "claude-code",
            Client::Codex => "codex",
            Client::OpenCode => "opencode",
            Client::Cursor => "cursor",
            Client::Zed => "zed",
            Client::VsCode => "vscode",
            Client::Windsurf => "windsurf",
            Client::ClaudeDesktop => "claude-desktop",
            Client::GeminiCli => "gemini",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Client::ClaudeCode => "Claude Code",
            Client::Codex => "Codex",
            Client::OpenCode => "OpenCode",
            Client::Cursor => "Cursor",
            Client::Zed => "Zed",
            Client::VsCode => "VS Code",
            Client::Windsurf => "Windsurf",
            Client::ClaudeDesktop => "Claude Desktop",
            Client::GeminiCli => "Gemini CLI",
        }
    }

    pub fn target(self) -> &'static str {
        match self {
            Client::ClaudeCode => "run once in a terminal (or ~/.claude.json)",
            Client::Codex => "~/.codex/config.toml",
            Client::OpenCode => "~/.config/opencode/opencode.json",
            Client::Cursor => "~/.cursor/mcp.json",
            Client::Zed => "~/.config/zed/settings.json",
            Client::VsCode => "~/Library/Application Support/Code/User/mcp.json",
            Client::Windsurf => "~/.codeium/windsurf/mcp_config.json",
            Client::ClaudeDesktop => "~/Library/Application Support/Claude/claude_desktop_config.json",
            Client::GeminiCli => "~/.gemini/settings.json",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "claude" | "claude-code" | "claudecode" => Some(Self::ClaudeCode),
            "codex" => Some(Self::Codex),
            "opencode" | "open-code" => Some(Self::OpenCode),
            "cursor" => Some(Self::Cursor),
            "zed" => Some(Self::Zed),
            "vscode" | "vs-code" | "code" => Some(Self::VsCode),
            "windsurf" => Some(Self::Windsurf),
            "claude-desktop" | "claudedesktop" => Some(Self::ClaudeDesktop),
            "gemini" | "gemini-cli" => Some(Self::GeminiCli),
            _ => None,
        }
    }

    /// Relative paths under `$HOME` whose existence indicates this client is
    /// installed on the machine. Read-only heuristic; never creates files.
    fn detection_paths(self) -> &'static [&'static str] {
        match self {
            Client::ClaudeCode => &[".claude.json", ".claude"],
            Client::Codex => &[".codex"],
            Client::OpenCode => &[".config/opencode"],
            Client::Cursor => &[".cursor"],
            Client::Zed => &[".config/zed"],
            Client::VsCode => &["Library/Application Support/Code", ".config/Code"],
            Client::Windsurf => &[".codeium/windsurf"],
            Client::ClaudeDesktop => &["Library/Application Support/Claude", ".config/Claude"],
            Client::GeminiCli => &[".gemini"],
        }
    }

    pub fn is_installed_in(self, home: &Path) -> bool {
        self.detection_paths().iter().any(|rel| home.join(rel).exists())
    }

    pub fn is_installed(self) -> bool {
        user_home().is_some_and(|h| self.is_installed_in(&h))
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
            Client::Cursor | Client::Windsurf | Client::ClaudeDesktop | Client::GeminiCli => format!(
                "{{\n  \"mcpServers\": {{\n    \"relay\": {{\n      \"command\": {command:?},\n      \"args\": [\"mcp\"]\n    }}\n  }}\n}}\n"
            ),
            Client::VsCode => format!(
                "{{\n  \"servers\": {{\n    \"relay\": {{\n      \"type\": \"stdio\",\n      \"command\": {command:?},\n      \"args\": [\"mcp\"]\n    }}\n  }}\n}}\n"
            ),
            Client::Zed => format!(
                "{{\n  \"context_servers\": {{\n    \"relay\": {{\n      \"command\": {{\n        \"path\": {command:?},\n        \"args\": [\"mcp\"]\n      }}\n    }}\n  }}\n}}\n"
            ),
        }
    }
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn relay_command() -> String {
    std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "relay".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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
        assert!(Client::VsCode.snippet("relay").contains("\"servers\""));
        assert!(Client::Windsurf.snippet("relay").contains("mcpServers"));
        assert!(Client::ClaudeDesktop.snippet("relay").contains("mcpServers"));
        assert!(Client::GeminiCli.snippet("relay").contains("mcpServers"));
        assert_eq!(Client::parse("vs-code"), Some(Client::VsCode));
        assert_eq!(Client::parse("gemini-cli"), Some(Client::GeminiCli));
        assert_eq!(Client::parse("nope"), None);
    }

    #[test]
    fn detection_is_read_only_and_finds_existing_client_dirs() {
        let dir = std::env::temp_dir().join(format!("relay-mcp-detect-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".cursor")).unwrap();
        fs::create_dir_all(dir.join(".config/zed")).unwrap();
        fs::write(dir.join(".claude.json"), "{}").unwrap();

        assert!(Client::ClaudeCode.is_installed_in(&dir));
        assert!(Client::Cursor.is_installed_in(&dir));
        assert!(Client::Zed.is_installed_in(&dir));
        assert!(!Client::Codex.is_installed_in(&dir));
        assert!(!Client::Windsurf.is_installed_in(&dir));
        assert!(!Client::VsCode.is_installed_in(&dir));

        let _ = fs::remove_dir_all(&dir);
    }
}
