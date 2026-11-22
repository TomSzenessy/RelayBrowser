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
    for expected in [
        "relay ",
        "engine:",
        "home:",
        "mcp socket:",
        "profile:",
        "search: default (DuckDuckGo)",
        "blocking: on",
        "sleep: 15 min",
        "mcp: on",
        "bridge:",
        "agents: claude mcp add relay",
    ] {
        assert!(text.contains(expected), "missing {expected:?} in:\n{text}");
    }
    assert!(text.contains("absent"), "a fresh RELAY_HOME has no socket:\n{text}");
}

#[test]
fn help_lists_doctor_and_version_is_clean() {
    let dir = tempfile::tempdir().unwrap();
    let help = relay(dir.path(), &["--help"]);
    assert!(help.status.success());
    let help_text = String::from_utf8_lossy(&help.stdout);
    for expected in ["relay config", "relay export", "relay clear-data", "relay doctor", "relay --app URL"] {
        assert!(help_text.contains(expected), "missing {expected:?} in:\n{help_text}");
    }

    let version = relay(dir.path(), &["--version"]);
    assert!(version.status.success());
    assert_eq!(String::from_utf8_lossy(&version.stdout).trim(), format!("relay {}", env!("CARGO_PKG_VERSION")));
}

#[test]
fn config_list_get_set_and_rejects_unknown_key() {
    let dir = tempfile::tempdir().unwrap();

    let list = relay(dir.path(), &["config", "list"]);
    assert!(list.status.success(), "{}", String::from_utf8_lossy(&list.stderr));
    let list_text = String::from_utf8_lossy(&list.stdout);
    assert!(list_text.contains("DuckDuckGo"), "default search should be DuckDuckGo:\n{list_text}");
    assert!(list_text.contains("blocking = on"), ":\n{list_text}");
    assert!(list_text.contains("sleep_minutes = 15"), ":\n{list_text}");
    assert!(list_text.contains("mcp = on"), ":\n{list_text}");
    assert!(list_text.contains("restore_session = on"), ":\n{list_text}");

    for args in [
        &["config", "set", "search", "kagi"][..],
        &["config", "set", "blocking", "off"][..],
        &["config", "set", "sleep_minutes", "0"][..],
        &["config", "set", "mcp", "off"][..],
        &["config", "set", "restore_session", "off"][..],
    ] {
        let out = relay(dir.path(), args);
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    let checks = [
        ("search", "https://kagi.com/search?q=%s"),
        ("blocking", "off"),
        ("sleep_minutes", "0"),
        ("mcp", "off"),
        ("restore_session", "off"),
    ];
    for (key, expected) in checks {
        let out = relay(dir.path(), &["config", "get", key]);
        assert!(out.status.success(), "get {key}: {}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), expected);
    }

    let doc = relay(dir.path(), &["doctor"]);
    assert!(doc.status.success(), "{}", String::from_utf8_lossy(&doc.stderr));
    let doc_text = String::from_utf8_lossy(&doc.stdout);
    for expected in ["https://kagi.com/search?q=%s (Kagi)", "blocking: off", "sleep: off", "mcp: off"] {
        assert!(doc_text.contains(expected), "missing {expected:?} in doctor:\n{doc_text}");
    }

    for bad in [
        &["config", "set", "bogus", "foo"][..],
        &["config", "get", "bogus"][..],
        &["config", "set", "blocking", "maybe"][..],
        &["config", "set", "sleep_minutes", "-1"][..],
        &["config", "set", "sleep_minutes", "99999"][..],
        &["config", "set", "search", "not-a-url"][..],
    ] {
        let out = relay(dir.path(), bad);
        assert!(!out.status.success(), "expected failure for {bad:?}, got stdout: {}", String::from_utf8_lossy(&out.stdout));
    }
}

#[test]
fn export_outputs_valid_json_and_writes_private_file() {
    let dir = tempfile::tempdir().unwrap();
    let set = relay(dir.path(), &["config", "set", "search", "brave"]);
    assert!(set.status.success(), "{}", String::from_utf8_lossy(&set.stderr));

    let stdout_out = relay(dir.path(), &["export"]);
    assert!(stdout_out.status.success(), "{}", String::from_utf8_lossy(&stdout_out.stderr));
    let raw = String::from_utf8_lossy(&stdout_out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("valid export JSON");
    assert_eq!(parsed["settings"]["search"], "https://search.brave.com/search?q=%s");
    assert!(parsed["bookmarks"].is_array());
    assert!(parsed["history"].is_array());
    assert!(parsed.get("passwords").is_none());
    assert!(!raw.to_ascii_lowercase().contains("password"));

    let export_path = dir.path().join("backup.json");
    let file_out = relay(dir.path(), &["export", export_path.to_str().unwrap()]);
    assert!(file_out.status.success(), "{}", String::from_utf8_lossy(&file_out.stderr));
    let file_text = std::fs::read_to_string(&export_path).expect("read exported file");
    let file_parsed: serde_json::Value = serde_json::from_str(&file_text).expect("valid exported file JSON");
    assert_eq!(file_parsed["settings"]["search"], "https://search.brave.com/search?q=%s");
    assert!(file_parsed["bookmarks"].is_array());
    assert!(file_parsed["history"].is_array());
    assert!(file_parsed.get("passwords").is_none());
    assert!(!file_text.to_ascii_lowercase().contains("password"));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&export_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "export file must be 0600, got {mode:#o}");
    }
}

#[test]
fn clear_data_requires_yes_confirmation() {
    let dir = tempfile::tempdir().unwrap();

    let denied = relay(dir.path(), &["clear-data", "--history"]);
    assert!(!denied.status.success(), "clear-data without --yes must fail");
    assert!(String::from_utf8_lossy(&denied.stderr).contains("--yes"));

    let ok_hist = relay(dir.path(), &["clear-data", "--history", "--yes"]);
    assert!(ok_hist.status.success(), "{}", String::from_utf8_lossy(&ok_hist.stderr));
    assert!(String::from_utf8_lossy(&ok_hist.stdout).contains("Cleared 0 history entries"));

    let ok_bm = relay(dir.path(), &["clear-data", "--bookmarks", "--yes"]);
    assert!(ok_bm.status.success(), "{}", String::from_utf8_lossy(&ok_bm.stderr));
    assert!(String::from_utf8_lossy(&ok_bm.stdout).contains("Cleared 0 bookmarks"));
}

#[test]
fn clear_data_removes_the_saved_session() {
    use relay_extensions::store::{SessionTab, Store};
    let dir = tempfile::tempdir().unwrap();
    let seed = |n: usize| {
        let store = Store::open(&dir.path().join("profile.sqlite")).unwrap();
        let tabs: Vec<SessionTab> = (0..n)
            .map(|i| SessionTab {
                url: format!("https://example.com/{i}"),
                title: "T".into(),
                pinned: false,
                scroll_y: 0.0,
                active: i == 0,
                position: i as i64,
            })
            .collect();
        store.save_session(&tabs).unwrap();
    };
    let saved = || Store::open(&dir.path().join("profile.sqlite")).unwrap().load_session().unwrap().len();

    seed(2);
    let only = relay(dir.path(), &["clear-data", "--session", "--yes"]);
    assert!(only.status.success(), "{}", String::from_utf8_lossy(&only.stderr));
    let text = String::from_utf8_lossy(&only.stdout);
    assert!(text.contains("Cleared 2 saved session tabs"), "{text}");
    assert!(!text.contains("history entries"), "--session alone must not clear history:\n{text}");
    assert_eq!(saved(), 0);

    seed(3);
    let hist = relay(dir.path(), &["clear-data", "--history", "--yes"]);
    assert!(hist.status.success(), "{}", String::from_utf8_lossy(&hist.stderr));
    assert!(String::from_utf8_lossy(&hist.stdout).contains("Cleared 3 saved session tabs"));
    assert_eq!(saved(), 0, "clearing history must also clear the saved session");
}

#[test]
fn config_bangs_validates_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let ok = relay(dir.path(), &["config", "set", "bangs", "!GH=https://example.com/s?q=%s  w=https://wiki.test/?q=%s"]);
    assert!(ok.status.success(), "{}", String::from_utf8_lossy(&ok.stderr));
    let get = relay(dir.path(), &["config", "get", "bangs"]);
    assert_eq!(String::from_utf8_lossy(&get.stdout).trim(), "gh=https://example.com/s?q=%s w=https://wiki.test/?q=%s");
    let list = relay(dir.path(), &["config", "list"]);
    assert!(String::from_utf8_lossy(&list.stdout).contains("bangs = gh=https://example.com/s?q=%s"));

    let bad = relay(dir.path(), &["config", "set", "bangs", "x=javascript:alert(1)"]);
    assert!(!bad.status.success(), "non-http templates must be rejected");
    assert!(String::from_utf8_lossy(&bad.stderr).contains("invalid bangs entry"));
    let still = relay(dir.path(), &["config", "get", "bangs"]);
    assert!(String::from_utf8_lossy(&still.stdout).starts_with("gh="), "a rejected value leaves the old one");
}

#[test]
fn app_mode_requires_an_http_url() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        &["--app", "file:///etc/passwd"][..],
        &["--app", "javascript:alert(1)"][..],
        &["--app", "http://"][..],
        &["--app", "https://"][..],
        &["--app", "https:///nohost"][..],
        &["--app"][..],
    ] {
        let out = relay(dir.path(), args);
        assert!(!out.status.success(), "{args:?} must be refused");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("--app needs"), "missing reason for {args:?}:\n{stderr}");
    }
    for args in [
        &["--app", "https://example.com", "--bogus"][..],
        &["--app", "https://example.com", "--app-name"][..],
        &["--app", "https://example.com", "--app-icon"][..],
    ] {
        let out = relay(dir.path(), args);
        assert!(!out.status.success(), "{args:?} must be refused");
        assert!(String::from_utf8_lossy(&out.stderr).contains("--app"));
    }
}

#[test]
fn start_sections_validates_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let set = relay(dir.path(), &["config", "set", "start_sections", "apps, recent apps"]);
    assert!(set.status.success(), "{}", String::from_utf8_lossy(&set.stderr));
    let get = relay(dir.path(), &["config", "get", "start_sections"]);
    assert_eq!(String::from_utf8_lossy(&get.stdout).trim(), "apps,recent");

    let bad = relay(dir.path(), &["config", "set", "start_sections", "recent,nope"]);
    assert!(!bad.status.success(), "unknown section must be rejected");
    assert!(String::from_utf8_lossy(&bad.stderr).contains("unknown start section"));
    assert_eq!(String::from_utf8_lossy(&relay(dir.path(), &["config", "get", "start_sections"]).stdout).trim(), "apps,recent");

    assert!(relay(dir.path(), &["config", "set", "start_sections", "none"]).status.success());
    assert_eq!(String::from_utf8_lossy(&relay(dir.path(), &["config", "get", "start_sections"]).stdout).trim(), "none");
    assert!(!relay(dir.path(), &["config", "set", "start_sections", ""]).status.success(), "empty is not a section list");
}
