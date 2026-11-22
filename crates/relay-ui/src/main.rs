//! `relay` — the browser, plus the subcommands agents and setup use.

mod app;
mod inspector;
mod palette;
mod theme;
mod web;

use anyhow::{bail, Context as _};
use gpui::{px, size, AppContext, Application, Bounds, TitlebarOptions, WindowBounds, WindowOptions};
use relay_bridge::{paths, ChannelBrowser, Owner, SharedHub};
use relay_extensions::{chrome_import, crx, manifest, store::Store};
use std::sync::Arc;

const HELP: &str = "relay — keyboard-first WebKit browser with a built-in MCP server

USAGE:
  relay [URL]                       open the browser (optionally at URL)
  relay mcp                         stdio MCP server for agents (connects to the running browser)
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
        Some("import-chrome") => import_chrome(&args[1..]),
        Some("import-passwords") => import_passwords(args.get(1).map(String::as_str)),
        Some("install-extension") => install_extension(args.get(1).map(String::as_str)),
        Some("compile-rules") => compile_rules(args.get(1).map(String::as_str)),
        Some("doctor") => doctor(),
        Some(other) if other.starts_with('-') => bail!("unknown option {other}\n\n{HELP}"),
        url => {
            run_browser(url.map(str::to_string));
            Ok(())
        }
    }
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
                let search = store.setting("search").ok().flatten().unwrap_or_else(|| "default (Google)".into());
                println!("profile: {} — {bookmarks} bookmarks, {extensions} enabled extensions", db.display());
                println!("search: {search}");
            }
            Err(e) => println!("profile: {} — unreadable: {e}", db.display()),
        }
    } else {
        println!("profile: {} — absent (created on first run)", db.display());
    }

    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "relay".into());
    println!("bridge: {} bytes, protocol v1", relay_bridge::BRIDGE_JS.len());
    println!("agents: claude mcp add relay -- {exe} mcp");
    Ok(())
}

fn open_store() -> anyhow::Result<Store> {
    let path = paths::relay_home().join("profile.sqlite");
    Store::open(&path).with_context(|| format!("opening {}", path.display()))
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

fn run_browser(initial_url: Option<String>) {
    let hub = SharedHub::new();
    let (browser, requests) = ChannelBrowser::new();
    let shell_requests = browser.clone();
    let socket = paths::mcp_socket();

    // MCP server: background tokio runtime, same hub.
    {
        let hub = hub.clone();
        let socket = socket.clone();
        std::thread::Builder::new()
            .name("relay-mcp".into())
            .spawn(move || match runtime() {
                Ok(rt) => {
                    if let Err(e) = rt.block_on(relay_mcp::serve_socket(&socket, hub, Arc::new(browser))) {
                        eprintln!("relay: MCP server stopped: {e:#}");
                    }
                }
                Err(e) => eprintln!("relay: no runtime for MCP: {e}"),
            })
            .expect("spawn MCP thread");
    }

    let store = match open_store() {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("relay: profile store unavailable: {e:#}");
            None
        }
    };
    let socket_label = socket.display().to_string();

    Application::new().run(move |cx| {
        cx.bind_keys(app::key_bindings());
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("Relay".into()), ..Default::default() }),
            ..Default::default()
        };
        let hub_for_window = hub.clone();
        let window = cx.open_window(options, move |window, cx| {
            cx.new(|cx| app::RelayApp::new(hub_for_window, requests, store, socket_label, window, cx))
        });
        if let Err(e) = window {
            eprintln!("relay: could not open a window: {e:#}");
            cx.quit();
            return;
        }
        if let Some(url) = initial_url {
            let url = match palette::resolve(&url, palette::DEFAULT_SEARCH) {
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
