//! The setup/diagnostic commands an owner or an agent runs before filing a bug.
use std::process::Command;

fn relay(home: &std::path::Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_relay"))
        .args(args)
        .env("RELAY_HOME", home)
        .output()
        .expect("run the relay binary")
}

#[test]
fn doctor_prints_versions_paths_profile_and_mcp_state() {
    let dir = tempfile::tempdir().unwrap();
    let out = relay(dir.path(), &["doctor"]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8_lossy(&out.stdout);
    for expected in ["relay ", "engine:", "home:", "mcp socket:", "profile:", "bridge:", "agents: claude mcp add relay"] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
    assert!(text.contains("absent"), "a fresh RELAY_HOME has no socket:\n{text}");
}

#[test]
fn help_lists_doctor_and_version_is_clean() {
    let dir = tempfile::tempdir().unwrap();
    let help = relay(dir.path(), &["--help"]);
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("relay doctor"));

    let version = relay(dir.path(), &["--version"]);
    assert!(version.status.success());
    assert_eq!(String::from_utf8_lossy(&version.stdout).trim(), format!("relay {}", env!("CARGO_PKG_VERSION")));
}
