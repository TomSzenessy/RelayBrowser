//! `relay` — the browser, plus the subcommands agents and setup use.

mod app;
mod bangs;
mod calc;
mod clean_url;
mod inspector;
mod keymap;
mod library;
mod mcp_config;
mod palette;
mod startpage;
mod tabstrip;
mod theme;
mod web;
mod webapp;

use anyhow::{bail, Context as _};
use gpui::{point, px, size, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions};
use relay_bridge::{paths, ChannelBrowser, Owner, SharedHub, PROTOCOL_VERSION};
use relay_extensions::{chrome_import, crx, manifest, store::Store};
use std::sync::Arc;

const HELP: &str = "relay — keyboard-first WebKit browser with a built-in MCP server

USAGE:
  relay [URL]                       open the browser (optionally at URL)
  relay --profile NAME [URL]        open a separate named profile (own tabs/data/login state)
  relay --app URL                   open URL as a single-page web app window (used by installed apps)
  relay mcp                         stdio MCP server for agents (connects to the running browser)
  relay config [list | get KEY | set KEY VALUE]
                                    inspect or update profile settings
  relay export [FILE]               export bookmarks, history, settings, and extensions as JSON
  relay import FILE                 merge a Relay JSON backup into this profile
  relay clear-data [--history] [--session] [--bookmarks] [--audit] --yes
                                    delete stored history/session/bookmarks and/or the agent audit log
  relay audit [--limit N]           show the lean log of agent tool calls
  relay mcp-config [CLIENT]         print the MCP config for 9 clients (claude-code, codex, opencode, cursor, zed, vscode, windsurf, claude-desktop, gemini)
  relay import-safari [--dry-run] import Safari bookmarks + history (needs Full Disk Access)
  relay import-chrome [--dry-run] [--profile DIR]
                                    import Chrome/Chromium-family bookmarks + history into Relay's profile
  relay import-passwords CSV        move a Chrome password export into the macOS Keychain, then delete the CSV
  relay install-extension ID        fetch a Chrome Web Store extension (loaded on next launch)
  relay compile-rules FILE          compile EasyList filters to WebKit content-blocker JSON
  relay doctor                      print versions, paths, profile and MCP socket state
  relay --version | --help

Agents: claude mcp add relay -- relay mcp
Data:   $RELAY_HOME (default ~/Library/Application Support/Relay)";

/// Keep the last panic (message, location, backtrace) in `logs/panic.log` so
/// a crash report is possible without a debugger attached. Best effort: a
/// read-only home must never turn one crash into two.
fn install_panic_log() {
    use std::io::Write;
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let dir = paths::relay_home().join("logs");
        if std::fs::create_dir_all(&dir).is_ok() {
            let path = dir.join("panic.log");
            if std::fs::metadata(&path).is_ok_and(|m| m.len() > 512 * 1024) {
                let _ = std::fs::remove_file(&path);
            }
            if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = writeln!(
                    file,
                    "relay {} panic: {info}\nthread: {:?}\n{}\n",
                    env!("CARGO_PKG_VERSION"),
                    std::thread::current().id(),
                    std::backtrace::Backtrace::force_capture()
                );
            }
        }
        default(info);
    }));
}

/// Consume a leading `--profile NAME` / `--profile=NAME`. The subcommand
/// `import-chrome --profile DIR` owns its own flag and is left alone.
fn take_profile_flag(args: &mut Vec<String>) -> anyhow::Result<Option<String>> {
    let Some(first) = args.first() else { return Ok(None) };
    let name = if first == "--profile" {
        if args.len() < 2 {
            bail!("--profile needs a name");
        }
        args.remove(0);
        args.remove(0)
    } else if let Some(name) = first.strip_prefix("--profile=") {
        let name = name.to_string();
        args.remove(0);
        name
    } else {
        return Ok(None);
    };
    let name = normalize_profile_name(&name);
    if name.is_empty() {
        bail!("profile name must contain a letter, digit, '-' or '_'");
    }
    Ok(Some(name))
}

fn normalize_profile_name(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.trim().chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if c.is_whitespace() || c == '/' || c == '\\' {
            if !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
        }
    }
    out.trim_matches('-').chars().take(32).collect()
}

/// Give a named profile its own `RELAY_HOME` (tabs, settings, extensions,
/// MCP socket) and a stable WebKit data-store UUID (cookies, cache, logins).
fn apply_profile(name: &str) -> anyhow::Result<()> {
    let base = std::env::var_os("RELAY_BASE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("RELAY_HOME").map(std::path::PathBuf::from))
        .unwrap_or_else(relay_bridge::paths::relay_home);
    let home = base.join("profiles").join(name);
    std::fs::create_dir_all(&home)?;
    // SAFETY: called once at process start, before any worker thread exists.
    unsafe {
        std::env::set_var("RELAY_BASE_HOME", &base);
        std::env::set_var("RELAY_HOME", &home);
        std::env::set_var("RELAY_PROFILE", name);
        let id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, name.as_bytes());
        std::env::set_var("RELAY_PROFILE_ID", id.to_string());
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(name) = take_profile_flag(&mut args)? {
        apply_profile(&name)?;
    }
    install_panic_log();
    match args.first().map(String::as_str) {
        Some("-h" | "--help" | "help") => {
            println!("{HELP}");
            Ok(())
        }
        Some("-V" | "--version") => {
            println!("relay {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("mcp") => run_mcp_proxy(),
        Some("config") => config(&args[1..]),
        Some("export") => export_profile(args.get(1).map(String::as_str)),
        Some("import") => import_profile(args.get(1).map(String::as_str)),
        Some("clear-data") => clear_data(&args[1..]),
        Some("audit") => audit(&args[1..]),
        Some("mcp-config") => mcp_config(&args[1..]),
        Some("import-chrome") => import_chrome(&args[1..]),
        Some("import-safari") => import_safari(&args[1..]),
        Some("import-passwords") => import_passwords(args.get(1).map(String::as_str)),
        Some("install-extension") => install_extension(args.get(1).map(String::as_str)),
        Some("compile-rules") => compile_rules(args.get(1).map(String::as_str)),
        Some("doctor") => doctor(),
        Some("--app") => launch_app(&args[1..]),
        Some(other) if other.starts_with("--app=") => {
            let url = &other["--app=".len()..];
            if !webapp::is_http_url(url) {
                bail!("--app needs an http(s) URL, got {url:?}");
            }
            run_browser(Some(url.to_string()), true, None, None);
            Ok(())
        }
        Some(other) if other.starts_with('-') => bail!("unknown option {other}\n\n{HELP}"),
        url => {
            run_browser(url.map(str::to_string), false, None, None);
            Ok(())
        }
    }
}

/// `relay --app URL [--app-name NAME] [--app-icon PATH]`, the shape an
/// installed bundle's launcher uses. The identity flags only affect the
/// window/menu name and the Dock icon; the process stays Relay, so the app
/// shares the browser's cookies, WebKit store, and updates.
fn launch_app(args: &[String]) -> anyhow::Result<()> {
    let url = args.first().context("--app needs a URL")?;
    if !webapp::is_http_url(url) {
        bail!("--app needs an http(s) URL, got {url:?}");
    }
    let mut name = None;
    let mut icon = None;
    let mut rest = args[1..].iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--app-name" => name = Some(rest.next().context("--app-name needs a name")?.clone()),
            "--app-icon" => icon = Some(std::path::PathBuf::from(rest.next().context("--app-icon needs a path")?)),
            other => bail!("unknown --app option {other}"),
        }
    }
    run_browser(Some(url.clone()), true, name, icon);
    Ok(())
}

/// The only URLs an installed app window may start on.
fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    Ok(tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build()?)
}

fn run_mcp_proxy() -> anyhow::Result<()> {
    runtime()?.block_on(relay_mcp::stdio_proxy(&paths::mcp_socket()))
}

/// One screen of facts for a bug report or a first-run check. Deliberately
/// portable: it works on the Linux dev host too, where WebKit is absent.
fn doctor() -> anyhow::Result<()> {
    println!("relay {} ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    if cfg!(target_os = "macos") {
        println!("engine: system WebKit (WKWebView), extensions need macOS 15.4+");
    } else {
        println!("engine: unavailable here; WebKit is macOS-only (this host can run tests only)");
    }

    let home = paths::relay_home();
    println!("home: {}", home.display());
    let socket = paths::mcp_socket();
    let socket_state = if socket.exists() {
        #[cfg(unix)]
        {
            match std::os::unix::net::UnixStream::connect(&socket) {
                Ok(_) => "present (a Relay instance is serving)",
                Err(_) => "stale file (no listener; remove it or start Relay)",
            }
        }
        #[cfg(not(unix))]
        {
            "present"
        }
    } else {
        "absent (start Relay, then connect)"
    };
    println!("mcp socket: {} — {socket_state}", socket.display());

    println!("crash log: {}", home.join("logs/panic.log").display());
    #[cfg(target_os = "macos")]
    println!("passkeys: only in a Developer ID build signed with Apple's web-browser entitlement (docs/passkeys.md)");
    let db = home.join("profile.sqlite");
    if db.exists() {
        match Store::open(&db) {
            Ok(store) => {
                let bookmarks = store.bookmarks().map(|b| b.len()).unwrap_or(0);
                let extensions = store.enabled_extensions().map(|e| e.len()).unwrap_or(0);
                println!("profile: {} — {bookmarks} bookmarks, {extensions} enabled extensions", db.display());
                print_doctor_settings(&load_config(Some(&store)));
            }
            Err(e) => println!("profile: {} — unreadable: {e}", db.display()),
        }
    } else {
        println!("profile: {} — absent (created on first run)", db.display());
        print_doctor_settings(&load_config(None));
    }

    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "relay".into());
    println!("bridge: {} bytes, protocol v{PROTOCOL_VERSION}", relay_bridge::BRIDGE_JS.len());
    println!("agents: claude mcp add relay -- {exe} mcp");
    Ok(())
}

struct EffectiveConfig {
    search: String,
    search_is_default: bool,
    blocking: bool,
    sleep_minutes: u64,
    mcp: bool,
    restore_session: bool,
    /// Custom bangs as stored (`trigger=template ...`); empty when none.
    bangs: String,
    /// New-tab section order as stored, or the default order.
    start_sections: String,
}

fn load_config(store: Option<&Store>) -> EffectiveConfig {
    let raw_search = store.and_then(|s| s.setting("search").ok().flatten());
    let (search, search_is_default) = match raw_search {
        Some(s) => (s, false),
        None => (palette::DEFAULT_SEARCH.to_string(), true),
    };
    let blocking = app::setting_enabled(store.and_then(|s| s.setting("blocking").ok().flatten()).as_deref());
    let sleep_minutes = store.and_then(|s| s.setting("sleep_minutes").ok().flatten()).and_then(|m| m.parse::<u64>().ok()).unwrap_or(15);
    let mcp = app::setting_enabled(store.and_then(|s| s.setting("mcp").ok().flatten()).as_deref());
    let restore_session = app::setting_enabled(store.and_then(|s| s.setting("restore_session").ok().flatten()).as_deref());
    let bangs = store.and_then(|s| s.setting("bangs").ok().flatten()).unwrap_or_default();
    let start_sections = store
        .and_then(|s| s.setting("start_sections").ok().flatten())
        .unwrap_or_else(|| "recent,frequent,apps,bookmarks".to_string());
    EffectiveConfig { search, search_is_default, blocking, sleep_minutes, mcp, restore_session, bangs, start_sections }
}

fn print_doctor_settings(cfg: &EffectiveConfig) {
    let search = if cfg.search_is_default {
        "default (DuckDuckGo)".to_string()
    } else {
        format!("{} ({})", cfg.search, palette::search_label(&cfg.search))
    };
    let sleep = if cfg.sleep_minutes == 0 { "off".to_string() } else { format!("{} min", cfg.sleep_minutes) };
    println!("search: {search}");
    println!("blocking: {}", if cfg.blocking { "on" } else { "off" });
    println!("sleep: {sleep}");
    println!("mcp: {}", if cfg.mcp { "on" } else { "off" });
    println!("restore session: {}", if cfg.restore_session { "on" } else { "off" });
}

fn open_store() -> anyhow::Result<Store> {
    let path = paths::relay_home().join("profile.sqlite");
    Store::open(&path).with_context(|| format!("opening {}", path.display()))
}

fn open_store_if_exists() -> anyhow::Result<Option<Store>> {
    let path = paths::relay_home().join("profile.sqlite");
    if path.exists() {
        Ok(Some(Store::open(&path).with_context(|| format!("opening {}", path.display()))?))
    } else {
        Ok(None)
    }
}

fn validate_config_value(key: &str, value: &str) -> anyhow::Result<String> {
    let trimmed = value.trim();
    match key {
        "search" => {
            if let Some(&(_, template)) = palette::SEARCH_PRESETS.iter().find(|&&(name, _)| name.eq_ignore_ascii_case(trimmed)) {
                return Ok(template.to_string());
            }
            if (trimmed.starts_with("http://") || trimmed.starts_with("https://"))
                && trimmed.contains("%s")
                && !trimmed.chars().any(char::is_whitespace)
            {
                return Ok(trimmed.to_string());
            }
            bail!("invalid search value {value:?}: use duckduckgo, kagi, google, brave, or an http(s)://...%s... URL template");
        }
        "blocking" | "mcp" | "restore_session" | "tabs_folded" | "bottom_bar" | "autofill_logins" | "login_offers" | "login_fill"
        | "login_save" | "tab_glyphs" | "compact_tabs" | "tab_numbers" | "agent_eval" | "shell_control" | "cookie_banners" | "bookmarks_bar" | "address_bar" | "tab_icons_only" | "extension_toolbar" | "handoff" | "bookmarks_icons_only" | "bookmarks_new_tab_only" | "always_show_close"
        | "downloads_ask" => match trimmed.to_ascii_lowercase().as_str() {
            "on" => Ok("on".to_string()),
            "off" => Ok("off".to_string()),
            _ => bail!("invalid value {value:?} for {key}: expected 'on' or 'off'"),
        },
        "sync_folder" => {
            let path = trimmed;
            if !path.is_empty() && !path.starts_with('/') && !path.starts_with("~/") && path != "~" {
                bail!("invalid sync_folder {value:?}: use an absolute path, ~/..., or empty to turn it off");
            }
            Ok(path.to_string())
        }
        "downloads_dir" => {
            let path = trimmed;
            if path.is_empty() || (!path.starts_with('/') && !path.starts_with("~/") && path != "~") {
                bail!("invalid downloads_dir {value:?}: use an absolute path or ~/... directory");
            }
            Ok(path.to_string())
        }
        "default_zoom" => {
            let zoom: f64 = trimmed.parse().with_context(|| format!("invalid default_zoom {value:?}: expected 0.5..=3.0"))?;
            if !(0.5..=3.0).contains(&zoom) {
                bail!("invalid default_zoom {value:?}: expected 0.5..=3.0");
            }
            Ok(format!("{zoom:.2}"))
        }
        "tabs_position" => match trimmed.to_ascii_lowercase().as_str() {
            "left" | "top" | "right" => Ok(trimmed.to_ascii_lowercase()),
            _ => bail!("invalid tabs_position {value:?}: expected left, top, or right"),
        },
        "sleep_minutes" => {
            let mins: u64 = trimmed
                .parse()
                .with_context(|| format!("invalid sleep_minutes {value:?}: expected an integer 0..=1440 (0 disables sleeping)"))?;
            if mins > 1440 {
                bail!("invalid sleep_minutes {value:?}: maximum is 1440 (24 hours; 0 disables sleeping)");
            }
            Ok(mins.to_string())
        }
        "start_sections" => {
            let sections = startpage::validate_sections(trimmed).map_err(|e| anyhow::anyhow!(e))?;
            Ok(if sections.is_empty() {
                "none".to_string()
            } else {
                sections.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(",")
            })
        }
        "bangs" => {
            let pairs = bangs::parse_custom(trimmed).map_err(|bad| {
                anyhow::anyhow!("invalid bangs entry {bad:?}: use space-separated trigger=https://...%s pairs (trigger: 1-16 letters or digits)")
            })?;
            Ok(pairs.iter().map(|(t, u)| format!("{t}={u}")).collect::<Vec<_>>().join(" "))
        }
        _ => bail!("unknown config key {key:?} (valid keys: search, blocking, sleep_minutes, mcp, restore_session, start_sections, bangs, tabs_position, tabs_folded, bottom_bar, autofill_logins, login_offers, login_fill, login_save, tab_glyphs, compact_tabs, tab_numbers, agent_eval, shell_control, cookie_banners, bookmarks_bar, address_bar, tab_icons_only, extension_toolbar, sync_folder, bookmarks_icons_only, bookmarks_new_tab_only, always_show_close, downloads_ask, downloads_dir, default_zoom)"),
    }
}

fn config(args: &[String]) -> anyhow::Result<()> {
    match args.first().map(String::as_str) {
        None | Some("list") => {
            if args.len() > 1 {
                bail!("usage: relay config [list | get KEY | set KEY VALUE]");
            }
            let store = open_store_if_exists()?;
            let cfg = load_config(store.as_ref());
            println!("search = {} ({})", cfg.search, palette::search_label(&cfg.search));
            println!("blocking = {}", if cfg.blocking { "on" } else { "off" });
            println!("sleep_minutes = {}", cfg.sleep_minutes);
            println!("mcp = {}", if cfg.mcp { "on" } else { "off" });
            println!("restore_session = {}", if cfg.restore_session { "on" } else { "off" });
            let setting = |key: &str, default: &str| {
                store.as_ref().and_then(|s| s.setting(key).ok().flatten()).unwrap_or_else(|| default.to_string())
            };
            println!("tabs_position = {}", setting("tabs_position", "left"));
            println!("bottom_bar = {}", setting("bottom_bar", "on"));
            println!("login_fill = {}", setting("login_fill", "on"));
            println!("login_save = {}", setting("login_save", "on"));
            println!("autofill_logins = {}", setting("autofill_logins", "on"));
            println!("tab_glyphs = {}", setting("tab_glyphs", "on"));
            println!("compact_tabs = {}", setting("compact_tabs", "off"));
            println!("default_zoom = {}", setting("default_zoom", "1.00"));
            println!("cookie_banners = {}", setting("cookie_banners", "on"));
            println!("bookmarks_bar = {}", setting("bookmarks_bar", "off"));
            println!("address_bar = {}", setting("address_bar", "off"));
            println!("tab_icons_only = {}", setting("tab_icons_only", "off"));
            println!("extension_toolbar = {}", setting("extension_toolbar", "on"));
            println!("handoff = {}", setting("handoff", "off"));
            println!("sync_folder = {}", setting("sync_folder", ""));
            println!("bookmarks_icons_only = {}", setting("bookmarks_icons_only", "off"));
            println!("bookmarks_new_tab_only = {}", setting("bookmarks_new_tab_only", "off"));
            println!("downloads_ask = {}", setting("downloads_ask", "off"));
            println!("downloads_dir = {}", setting("downloads_dir", ""));
            println!("always_show_close = {}", setting("always_show_close", "off"));
            println!("bangs = {}", cfg.bangs);
            println!("start_sections = {}", cfg.start_sections);
            Ok(())
        }
        Some("get") => {
            if args.len() != 2 {
                bail!("usage: relay config get KEY");
            }
            let key = args[1].as_str();
            let store = open_store_if_exists()?;
            let cfg = load_config(store.as_ref());
            match key {
                "search" => println!("{}", cfg.search),
                "blocking" => println!("{}", if cfg.blocking { "on" } else { "off" }),
                "sleep_minutes" => println!("{}", cfg.sleep_minutes),
                "mcp" => println!("{}", if cfg.mcp { "on" } else { "off" }),
                "restore_session" => println!("{}", if cfg.restore_session { "on" } else { "off" }),
                "tabs_position" => println!("{}", store.as_ref().and_then(|s| s.setting("tabs_position").ok().flatten()).unwrap_or_else(|| "left".into())),
                "bottom_bar" => println!("{}", store.as_ref().and_then(|s| s.setting("bottom_bar").ok().flatten()).unwrap_or_else(|| "on".into())),
                "login_fill" | "login_save" | "autofill_logins" | "login_offers" | "tab_glyphs" | "tab_numbers" | "agent_eval" | "shell_control" | "cookie_banners" | "bookmarks_bar" | "address_bar" | "tab_icons_only" | "extension_toolbar" | "bookmarks_icons_only" | "bookmarks_new_tab_only" | "always_show_close"
        | "downloads_ask" => {
                    println!("{}", store.as_ref().and_then(|s| s.setting(key).ok().flatten()).unwrap_or_else(|| "on".into()))
                }
                "compact_tabs" => println!("{}", store.as_ref().and_then(|s| s.setting("compact_tabs").ok().flatten()).unwrap_or_else(|| "off".into())),
                "sync_folder" => {
                    println!("{}", store.as_ref().and_then(|s| s.setting("sync_folder").ok().flatten()).unwrap_or_default())
                }
                "downloads_dir" => {
                    println!("{}", store.as_ref().and_then(|s| s.setting("downloads_dir").ok().flatten()).unwrap_or_default())
                }
                "default_zoom" => {
                    println!("{}", store.as_ref().and_then(|s| s.setting("default_zoom").ok().flatten()).unwrap_or_else(|| "1.00".into()))
                }
                "bangs" => println!("{}", cfg.bangs),
                "start_sections" => println!("{}", cfg.start_sections),
                _ => bail!("unknown config key {key:?} (valid keys: search, blocking, sleep_minutes, mcp, restore_session, start_sections, bangs, tabs_position, tabs_folded, bottom_bar, autofill_logins, login_offers, login_fill, login_save, tab_glyphs, compact_tabs, tab_numbers, agent_eval, shell_control, cookie_banners, bookmarks_bar, address_bar, tab_icons_only, extension_toolbar, sync_folder, bookmarks_icons_only, bookmarks_new_tab_only, always_show_close, downloads_ask, downloads_dir, default_zoom)"),
            }
            Ok(())
        }
        Some("set") => {
            if args.len() != 3 {
                bail!("usage: relay config set KEY VALUE");
            }
            let (key, value) = (args[1].as_str(), args[2].as_str());
            let canonical = validate_config_value(key, value)?;
            open_store()?.set_setting(key, &canonical)?;
            println!("Set {key} = {canonical}");
            Ok(())
        }
        _ => bail!("usage: relay config [list | get KEY | set KEY VALUE]"),
    }
}

fn import_profile(file: Option<&str>) -> anyhow::Result<()> {
    let file = std::path::Path::new(file.context("usage: relay import <relay-profile.json>")?);
    let json = std::fs::read_to_string(file)?;
    let summary = open_store()?.import_export_json(&json)?;
    println!(
        "Imported {} bookmarks, {} history rows, {} settings from {}.",
        summary.bookmarks,
        summary.history,
        summary.settings,
        file.display()
    );
    Ok(())
}

fn export_profile(file: Option<&str>) -> anyhow::Result<()> {
    let json = open_store()?.export_json()?;
    if let Some(file) = file {
        let path = std::path::Path::new(file);
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        write_private_file(path, json.as_bytes())?;
        println!("Exported profile to {}", path.display());
    } else {
        println!("{json}");
    }
    Ok(())
}

#[cfg(unix)]
fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(bytes)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn write_private_file(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    std::fs::write(path, bytes)?;
    Ok(())
}

fn clear_data(args: &[String]) -> anyhow::Result<()> {
    const USAGE: &str = "usage: relay clear-data [--history] [--session] [--bookmarks] [--audit] --yes";
    let mut history = false;
    let mut session = false;
    let mut bookmarks = false;
    let mut audit_log = false;
    let mut yes = false;
    for arg in args {
        match arg.as_str() {
            "--history" => history = true,
            "--session" => session = true,
            "--bookmarks" => bookmarks = true,
            "--audit" => audit_log = true,
            "--yes" => yes = true,
            other => bail!("unknown flag {other}\n{USAGE}"),
        }
    }
    if !history && !session && !bookmarks && !audit_log {
        history = true;
    }
    // Saved session tabs are URLs the user visited: clearing history clears them too.
    session |= history;
    if !yes {
        bail!("refusing to clear data without --yes (pass --yes to confirm)");
    }
    let store = open_store()?;
    if history {
        let n = store.clear_history()?;
        println!("Cleared {n} history entries.");
    }
    if session {
        let n = store.clear_session()?;
        println!("Cleared {n} saved session tabs.");
    }
    if bookmarks {
        let n = store.clear_bookmarks()?;
        println!("Cleared {n} bookmarks.");
    }
    if audit_log {
        let n = store.clear_agent_audit()?;
        println!("Cleared {n} agent audit rows.");
    }
    Ok(())
}

/// UTC timestamp without a date library: days since the Unix epoch converted
/// with the civil-from-days algorithm.
fn format_unix(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let rem = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

fn mcp_config(args: &[String]) -> anyhow::Result<()> {
    const USAGE: &str = "usage: relay mcp-config [claude-code|codex|opencode|cursor|zed|vscode|windsurf|claude-desktop|gemini]";
    let mut wanted = None;
    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other if other.starts_with('-') => bail!("unknown option {other}\n{USAGE}"),
            other => {
                if wanted.is_some() {
                    bail!("pass one client\n{USAGE}");
                }
                wanted = Some(mcp_config::Client::parse(other).with_context(|| format!("unknown client {other:?}\n{USAGE}"))?);
            }
        }
    }
    let command = mcp_config::relay_command();
    match wanted {
        Some(client) => {
            println!("# {} — {}", client.name(), client.target());
            print!("{}", client.snippet(&command));
        }
        None => {
            for client in mcp_config::Client::ALL {
                println!("# {} — {}", client.name(), client.target());
                print!("{}", client.snippet(&command));
                println!();
            }
        }
    }
    Ok(())
}

fn audit(args: &[String]) -> anyhow::Result<()> {
    const USAGE: &str = "usage: relay audit [--limit N]";
    let mut limit = 100usize;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--limit" => {
                i += 1;
                let value = args.get(i).with_context(|| format!("--limit needs a number\n{USAGE}"))?;
                limit = value.parse::<usize>().with_context(|| format!("--limit needs a number, got {value:?}"))?.clamp(1, 5000);
            }
            other => bail!("unknown option {other}\n{USAGE}"),
        }
        i += 1;
    }
    let rows = open_store()?.agent_audit(limit)?;
    if rows.is_empty() {
        println!("No agent calls yet.");
        return Ok(());
    }
    println!("{:<20} {:<18} {:<24} {:<5} {}", "TIME", "TOOL", "HOST", "TAB", "OUTCOME");
    for r in rows {
        let tab = r.tab.map(|t| t.to_string()).unwrap_or_else(|| "-".into());
        println!("{:<20} {:<18} {:<24} {:<5} {}", format_unix(r.at), r.tool, r.host, tab, r.outcome);
    }
    Ok(())
}

fn import_chrome(args: &[String]) -> anyhow::Result<()> {
    const USAGE: &str = "usage: relay import-chrome [--dry-run] [--profile DIR]";
    let mut dry = false;
    let mut profile = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            "--dry-run" => dry = true,
            "--profile" => {
                i += 1;
                let value = args.get(i).with_context(|| format!("--profile needs a directory\n{USAGE}"))?;
                if value.starts_with("--") {
                    bail!("--profile needs a directory, got {value:?}\n{USAGE}");
                }
                profile = Some(std::path::PathBuf::from(value));
            }
            other => bail!("unknown option {other}\n{USAGE}"),
        }
        i += 1;
    }
    let profile = match profile {
        Some(p) => p,
        None => chrome_import::default_profile().context("no Chrome-family profile found; pass --profile DIR")?,
    };
    let p = chrome_import::preview(&profile, 20_000);
    println!("{} profile: {}", chrome_import::browser_name(&profile), profile.display());
    println!("  bookmarks:  {}", p.bookmarks.len());
    println!("  history:    {}", p.history.len());
    println!("  extensions: {}", p.extensions.len());
    for e in &p.extensions {
        let m = manifest::Manifest::read(&profile.join("Extensions").join(&e.id).join(format!("{}_0", e.version))).ok();
        let note = m.map(|m| m.compatibility()).map(|c| if c.missing.is_empty() { String::new() } else { format!("  (WebKit lacks: {})", c.missing.join(", ")) }).unwrap_or_default();
        println!("    {} {} [{}]{note}", e.name, e.version, e.id);
    }
    for e in &p.errors {
        println!("  ! {e}");
    }
    if dry {
        println!("(dry run: nothing written)");
        return Ok(());
    }
    let mut store = open_store()?;
    let c = store.import(&p.bookmarks, &p.history)?;
    println!("Imported {} new bookmarks and {} history entries.", c.bookmarks, c.history);
    if !p.extensions.is_empty() {
        println!("Install extensions with: relay install-extension <id>");
    }
    println!("Passwords: export them from chrome://password-manager/settings (Export passwords), then run: relay import-passwords <file.csv>");
    Ok(())
}

fn import_safari(args: &[String]) -> anyhow::Result<()> {
    let dry_run = args.iter().any(|a| a == "--dry-run");
    let bookmarks = relay_extensions::safari::default_bookmarks();
    let history = relay_extensions::safari::default_history();
    if bookmarks.is_none() && history.is_none() {
        bail!("no Safari bookmarks/history found; grant Full Disk Access to Relay in System Settings → Privacy & Security");
    }
    let store = open_store()?;
    let empty = std::path::Path::new("/nonexistent");
    let counts = relay_extensions::safari::import(
        &store,
        bookmarks.as_deref().unwrap_or(empty),
        history.as_deref().unwrap_or(empty),
        dry_run,
    )?;
    println!(
        "{} {} bookmarks and {} history rows{}.",
        if dry_run { "Would import" } else { "Imported" },
        counts.bookmarks,
        counts.history,
        if counts.skipped > 0 { format!(" ({} already present)", counts.skipped) } else { String::new() }
    );
    Ok(())
}

fn import_passwords(file: Option<&str>) -> anyhow::Result<()> {
    let file = std::path::Path::new(file.context("usage: relay import-passwords <Chrome Passwords.csv>")?);
    let creds = chrome_import::read_password_csv(file)?;
    #[cfg(target_os = "macos")]
    {
        let profile = std::env::var("RELAY_PROFILE").ok().filter(|p| !p.is_empty()).map(|p| format!("relay-profile:{p}"));
        let r = relay_extensions::mac::import_passwords(&creds, profile.as_deref());
        if let Ok(store) = open_store() {
            for c in &creds {
                if let Ok((origin, path)) = relay_extensions::login::Origin::parse_with_path(&c.url) {
                    // Recorded so Settings only lists/deletes logins Relay
                    // itself wrote, never another app's Keychain items.
                    let _ = store.record_saved_login(origin.host(), &c.username, &path, 0);
                }
            }
        }
        println!("Saved {} of {} logins to the login Keychain.", r.saved, creds.len());
        for f in &r.failed {
            println!("  ! {f}");
        }
        if r.failed.is_empty() {
            std::fs::remove_file(file)?;
            println!("Deleted {} (it held plaintext passwords).", file.display());
        } else {
            println!("Kept {} because some logins failed; delete it yourself when done.", file.display());
        }
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        bail!("{} logins parsed, but the Keychain exists only on macOS; nothing was stored", creds.len())
    }
}

fn install_extension(id: Option<&str>) -> anyhow::Result<()> {
    let id = id.context("usage: relay install-extension <chrome web store id>")?;
    let bytes = crx::download(id)?;
    let dir = crx::unpack(&bytes, id, &paths::relay_home().join("extensions"))?;
    manifest::install_shims(&dir)?;
    let m = manifest::Manifest::read(&dir)?;
    let compat = m.compatibility();
    let name = m.display_name(&dir);
    for line in m.permission_report().lines() {
        println!("  {line}");
    }
    println!("  installing grants every requested permission and host pattern.");
    open_store()?.record_extension(id, &name, &m.version, &dir)?;
    println!("Installed {name} {} → {}", m.version, dir.display());
    for line in compat.report_lines() {
        println!("  {line}");
    }
    println!("It loads the next time Relay starts.");
    Ok(())
}

fn compile_rules(file: Option<&str>) -> anyhow::Result<()> {
    let file = file.context("usage: relay compile-rules <easylist.txt>")?;
    let text = std::fs::read_to_string(file)?;
    let c = relay_webkit::content_rules::compile(&text);
    eprintln!("{} rules, {} filters skipped (unsupported syntax)", c.rules.len(), c.skipped);
    println!("{}", c.to_json());
    Ok(())
}

fn run_browser(initial_url: Option<String>, app_mode: bool, app_name: Option<String>, app_icon: Option<std::path::PathBuf>) {
    let hub = SharedHub::new();
    let (browser, requests) = ChannelBrowser::new();
    let shell_requests = browser.clone();
    let socket = paths::mcp_socket();

    let store = match open_store() {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("relay: profile store unavailable: {e:#}");
            None
        }
    };
    let cfg = load_config(store.as_ref());
    // An installed web app is a single page: it must not compete for the
    // browser's MCP socket or expose an agent surface of its own.
    let socket_label = if cfg.mcp && !app_mode {
        // MCP server: background tokio runtime, same hub.
        let hub = hub.clone();
        let socket_for_thread = socket.clone();
        std::thread::Builder::new()
            .name("relay-mcp".into())
            .spawn(move || match runtime() {
                Ok(rt) => {
                    let audit = open_store().ok().map(|s| Arc::new(std::sync::Mutex::new(s)));
                    if let Err(e) = rt.block_on(relay_mcp::serve_socket(&socket_for_thread, hub, Arc::new(browser), audit)) {
                        eprintln!("relay: MCP server stopped: {e:#}");
                    }
                }
                Err(e) => eprintln!("relay: no runtime for MCP: {e}"),
            })
            .expect("spawn MCP thread");
        socket.display().to_string()
    } else {
        "disabled".to_string()
    };
    let initial_search = cfg.search;

    let window_title = app_name.clone().unwrap_or_else(|| "Relay".to_string());
    let application = Application::new();
    if !app_mode {
        // LaunchServices sends links from other apps here once Relay is the
        // default browser, instead of passing them on argv.
        let hub_open = hub.clone();
        let browser_open = shell_requests.clone();
        let search = initial_search.clone();
        application.on_open_urls(move |urls| {
            for raw in urls {
                let url = match palette::resolve(&raw, &search) {
                    palette::Item::Open(u) | palette::Item::Search { url: u, .. } => u,
                    _ => raw,
                };
                if !webapp::is_http_url(&url) {
                    continue;
                }
                let tab = hub_open.update(|h| {
                    let id = h.create_tab(&url, Owner::User);
                    h.set_active(id);
                    id
                });
                relay_bridge::Browser::open(&browser_open, tab, &url);
            }
        });
    }
    application.run(move |cx| {
        cx.bind_keys(app::key_bindings());
        let bounds = if app_mode {
            Bounds::centered(None, size(px(980.), px(760.)), cx)
        } else {
            Bounds::centered(None, size(px(1280.), px(820.)), cx)
        };
        let _ = window_title;
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: None,
                appears_transparent: true,
                traffic_light_position: Some(point(px(12.), px(12.))),
            }),
            ..Default::default()
        };
        let hub_for_window = hub.clone();
        let window = cx.open_window(options, move |window, cx| {
            cx.new(|cx| app::RelayApp::new(hub_for_window, requests, store, socket_label, app_mode, app_name, app_icon, window, cx))
        });
        if let Err(e) = window {
            eprintln!("relay: could not open a window: {e:#}");
            cx.quit();
            return;
        }
        if let Some(url) = initial_url {
            let url = match palette::resolve(&url, &initial_search) {
                palette::Item::Open(u) | palette::Item::Search { url: u, .. } => u,
                _ => url,
            };
            let tab = hub.update(|h| {
                let id = h.create_tab(&url, Owner::User);
                h.set_active(id);
                id
            });
            // The pump applies this on the next frame, like an MCP request.
            relay_bridge::Browser::open(&shell_requests, tab, &url);
        }
        cx.activate(true);
    });
}


#[cfg(test)]
mod profile_tests {
    use super::*;

    #[test]
    fn profile_flag_is_consumed_only_when_leading() {
        let mut args = vec!["--profile".to_string(), "Work / A".to_string(), "export".to_string()];
        assert_eq!(take_profile_flag(&mut args).unwrap().as_deref(), Some("Work-A"));
        assert_eq!(args, vec!["export"]);
        let mut args = vec!["import-chrome".to_string(), "--profile".to_string(), "/tmp/chrome".to_string()];
        assert_eq!(take_profile_flag(&mut args).unwrap(), None);
        assert_eq!(args, vec!["import-chrome", "--profile", "/tmp/chrome"]);
        let mut args = vec!["--profile=Personal".to_string()];
        assert_eq!(take_profile_flag(&mut args).unwrap().as_deref(), Some("Personal"));
    }
}
