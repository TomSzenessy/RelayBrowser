//! `relay` — the browser, plus the subcommands agents and setup use.

mod app;
mod bangs;
mod calc;
mod clean_url;
mod inspector;
mod palette;
mod startpage;
mod tabstrip;
mod theme;
mod web;
mod webapp;

use anyhow::{bail, Context as _};
use gpui::{px, size, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions};
use relay_bridge::{paths, ChannelBrowser, Owner, SharedHub};
use relay_extensions::{chrome_import, crx, manifest, store::Store};
use std::sync::Arc;

const HELP: &str = "relay — keyboard-first WebKit browser with a built-in MCP server

USAGE:
  relay [URL]                       open the browser (optionally at URL)
  relay --app URL                   open URL as a single-page web app window (used by installed apps)
  relay mcp                         stdio MCP server for agents (connects to the running browser)
  relay config [list | get KEY | set KEY VALUE]
                                    inspect or update profile settings
  relay export [FILE]               export bookmarks, history, settings, and extensions as JSON
  relay clear-data [--history] [--session] [--bookmarks] --yes
                                    delete stored history (incl. the saved session), the saved session, and/or bookmarks
  relay import-chrome [--dry-run] [--profile DIR]
                                    import Chrome bookmarks + history into Relay's profile
  relay import-passwords CSV        move a Chrome password export into the macOS Keychain, then delete the CSV
  relay install-extension ID        fetch a Chrome Web Store extension (loaded on next launch)
  relay compile-rules FILE          compile EasyList filters to WebKit content-blocker JSON
  relay doctor                      print versions, paths, profile and MCP socket state
  relay --version | --help

Agents: claude mcp add relay -- relay mcp
Data:   $RELAY_HOME (default ~/Library/Application Support/Relay)";

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
        Some("clear-data") => clear_data(&args[1..]),
        Some("import-chrome") => import_chrome(&args[1..]),
        Some("import-passwords") => import_passwords(args.get(1).map(String::as_str)),
        Some("install-extension") => install_extension(args.get(1).map(String::as_str)),
        Some("compile-rules") => compile_rules(args.get(1).map(String::as_str)),
        Some("doctor") => doctor(),
        Some("--app") => launch_app(&args[1..]),
        Some(other) if other.starts_with("--app=") => {
            let url = &other["--app=".len()..];
            if !is_web_url(url) {
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
    if !is_web_url(url) {
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
fn is_web_url(url: &str) -> bool {
    webapp::is_http_url(url)
}

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
    let socket_state = if socket.exists() { "present (a Relay instance is serving)" } else { "absent (start Relay, then connect)" };
    println!("mcp socket: {} — {socket_state}", socket.display());

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
    println!("bridge: {} bytes, protocol v1", relay_bridge::BRIDGE_JS.len());
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
    EffectiveConfig { search, search_is_default, blocking, sleep_minutes, mcp, restore_session, bangs }
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
        "blocking" | "mcp" | "restore_session" => match trimmed.to_ascii_lowercase().as_str() {
            "on" => Ok("on".to_string()),
            "off" => Ok("off".to_string()),
            _ => bail!("invalid value {value:?} for {key}: expected 'on' or 'off'"),
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
        "bangs" => {
            let pairs = bangs::parse_custom(trimmed).map_err(|bad| {
                anyhow::anyhow!("invalid bangs entry {bad:?}: use space-separated trigger=https://...%s pairs (trigger: 1-16 letters or digits)")
            })?;
            Ok(pairs.iter().map(|(t, u)| format!("{t}={u}")).collect::<Vec<_>>().join(" "))
        }
        _ => bail!("unknown config key {key:?} (valid keys: search, blocking, sleep_minutes, mcp, restore_session, bangs)"),
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
            println!("bangs = {}", cfg.bangs);
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
                "bangs" => println!("{}", cfg.bangs),
                _ => bail!("unknown config key {key:?} (valid keys: search, blocking, sleep_minutes, mcp, restore_session, bangs)"),
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
    const USAGE: &str = "usage: relay clear-data [--history] [--session] [--bookmarks] --yes";
    let mut history = false;
    let mut session = false;
    let mut bookmarks = false;
    let mut yes = false;
    for arg in args {
        match arg.as_str() {
            "--history" => history = true,
            "--session" => session = true,
            "--bookmarks" => bookmarks = true,
            "--yes" => yes = true,
            other => bail!("unknown flag {other}\n{USAGE}"),
        }
    }
    if !history && !session && !bookmarks {
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
    Ok(())
}

fn import_chrome(args: &[String]) -> anyhow::Result<()> {
    let dry = args.iter().any(|a| a == "--dry-run");
    let profile = match args.iter().position(|a| a == "--profile") {
        Some(i) => args.get(i + 1).map(std::path::PathBuf::from).context("--profile needs a directory")?,
        None => chrome_import::default_profile().context("no Chrome profile found; pass --profile DIR")?,
    };
    let p = chrome_import::preview(&profile, 20_000);
    println!("Chrome profile: {}", profile.display());
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

fn import_passwords(file: Option<&str>) -> anyhow::Result<()> {
    let file = std::path::Path::new(file.context("usage: relay import-passwords <Chrome Passwords.csv>")?);
    let creds = chrome_import::read_password_csv(file)?;
    #[cfg(target_os = "macos")]
    {
        let r = relay_extensions::mac::import_passwords(&creds);
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
    open_store()?.record_extension(id, &name, &m.version, &dir)?;
    println!("Installed {name} {} → {}", m.version, dir.display());
    if !compat.shimmed.is_empty() {
        println!("  shimmed: {}", compat.shimmed.join(", "));
    }
    if !compat.missing.is_empty() {
        println!("  WebKit lacks: {} (those features will not work)", compat.missing.join(", "));
    }
    for w in compat.warnings {
        println!("  note: {w}");
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
                    if let Err(e) = rt.block_on(relay_mcp::serve_socket(&socket_for_thread, hub, Arc::new(browser))) {
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
    Application::new().run(move |cx| {
        cx.bind_keys(app::key_bindings());
        let bounds = if app_mode {
            Bounds::centered(None, size(px(980.), px(760.)), cx)
        } else {
            Bounds::centered(None, size(px(1280.), px(820.)), cx)
        };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some(window_title.clone().into()), ..Default::default() }),
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
