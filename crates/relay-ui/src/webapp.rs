//! Install the current page as a small macOS app.
//!
//! A web app is the page's manifest, a wrapper `.app` in `~/Applications`
//! with its own icon, and a launcher that runs Relay in `--app` mode. Relay
//! itself stays tiny: the installed app is one WKWebView, not another browser
//! runtime. The filesystem parts are portable so the bundle contract can be
//! tested on Linux; only the `.icns` conversion is macOS-only.
//!
//! Everything here treats the manifest as untrusted data: lengths are capped,
//! only http(s) same-origin start URLs are accepted, and the icon must be a
//! real PNG data URL.

use serde_json::Value;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// A manifest icon larger than this is ignored rather than copied.
pub const MAX_ICON_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_NAME_CHARS: usize = 64;
const DEFAULT_BUNDLE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The parts of a web app manifest Relay acts on.
#[derive(Debug, Clone, PartialEq)]
pub struct Manifest {
    pub name: String,
    pub start_url: String,
    pub display: String,
    /// `data:image/png;base64,...` decoded, if the page offered a usable icon.
    pub icon_png: Option<Vec<u8>>,
}

/// Parse the bridge's `web_app` reply. `page_url` is the tab's own URL (not
/// the page's claim), so a page cannot install an app for another origin.
pub fn manifest_from_result(value: &Value, page_url: &str) -> Result<Manifest, String> {
    let page = Origin::parse(page_url).ok_or_else(|| "Only http(s) pages can be installed as apps.".to_string())?;
    if value.get("available").and_then(Value::as_bool) != Some(true) {
        let reason = value.get("reason").and_then(Value::as_str).unwrap_or("This page does not declare a web app manifest.");
        return Err(reason.to_string());
    }
    let start_raw = value.get("start_url").and_then(Value::as_str).unwrap_or(page_url);
    if Origin::parse(start_raw).is_none() {
        return Err("The web app's start address is not an http(s) URL.".to_string());
    }
    if Origin::parse(start_raw).as_ref() != Some(&page) {
        return Err("The web app starts on a different site; open that site and install it from there.".to_string());
    }
    let raw_name = value
        .get("name")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .or_else(|| value.get("short_name").and_then(Value::as_str))
        .unwrap_or("");
    let name = display_name(raw_name, &page.host);
    let display = match value.get("display").and_then(Value::as_str).unwrap_or("standalone").to_ascii_lowercase().as_str() {
        "fullscreen" => "fullscreen",
        "minimal-ui" => "minimal-ui",
        "browser" => "browser",
        _ => "standalone",
    }
    .to_string();
    let icon_png = value.get("icon").and_then(Value::as_str).and_then(decode_png_data_url);
    Ok(Manifest { name, start_url: start_raw.to_string(), display, icon_png })
}

/// A display name that is safe to show and close to a filename. Trailing
/// `.app` is dropped so `Foo.app` does not become `Foo.app.app`.
pub fn display_name(raw: &str, host: &str) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut name: String = collapsed.chars().filter(|c| !c.is_control()).collect();
    let trimmed = name.trim().trim_start_matches('.').trim_end_matches(['.', ' ']).to_string();
    name = trimmed.strip_suffix(".app").unwrap_or(&trimmed).trim_end().to_string();
    if name.is_empty() {
        name = host.trim_start_matches("www.").to_string();
    }
    if name.is_empty() {
        name = "Web App".to_string();
    }
    name.chars().take(MAX_NAME_CHARS).collect::<String>().trim_end().to_string()
}

/// The on-disk bundle name. macOS refuses `/` in a component and Finder
/// displays `:` as `/`, so both become spaces; the suffix keeps two apps with
/// the same title apart.
pub fn bundle_name(display: &str) -> String {
    let mut out = String::with_capacity(display.len());
    for ch in display.chars() {
        match ch {
            '/' | '\\' | ':' => out.push(' '),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    let name = out.split_whitespace().collect::<Vec<_>>().join(" ");
    let name = name.trim_matches(['.', ' ']).trim_end_matches(".app").trim_end();
    if name.is_empty() { "Web App".to_string() } else { name.chars().take(MAX_NAME_CHARS).collect() }
}

/// A stable identifier for one installed web app.
pub fn bundle_id(start_url: &str) -> String {
    let host = Origin::parse(start_url).map(|o| o.host).unwrap_or_else(|| "web".into());
    let slug: String = host
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let slug = if slug.is_empty() { "web".to_string() } else { slug.chars().take(40).collect() };
    format!("app.relay.webapp.{slug}.{:016x}", fnv1a(start_url.as_bytes()))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for b in bytes {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// scheme://authority, with default ports removed and the host lowercased.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    scheme: &'static str,
    host: String,
    port: Option<u16>,
}

impl Origin {
    pub fn parse(url: &str) -> Option<Self> {
        let (scheme, rest) = url.split_once("://")?;
        let scheme = match scheme.to_ascii_lowercase().as_str() {
            "http" => "http",
            "https" => "https",
            _ => return None,
        };
        let authority = rest.split(['/', '?', '#']).next()?.rsplit('@').next()?.trim();
        if authority.is_empty() {
            return None;
        }
        let (host, port) = split_authority(authority, scheme)?;
        Some(Self { scheme, host: host.to_ascii_lowercase(), port })
    }
}

fn split_authority<'a>(authority: &'a str, scheme: &str) -> Option<(&'a str, Option<u16>)> {
    // `[::1]:8080` keeps its brackets around the host.
    let (host, port) = if let Some(rest) = authority.strip_prefix('[') {
        let close = rest.find(']')?;
        let after = &rest[close + 1..];
        let port = after.strip_prefix(':');
        (&authority[..close + 2], port)
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => (host, Some(port)),
            Some(_) if authority.contains(':') => return None,
            _ => (authority, None),
        }
    };
    if host.is_empty() {
        return None;
    }
    let port = match port {
        Some(p) => Some(p.parse::<u16>().ok()?),
        None => None,
    };
    let default = match scheme {
        "http" => Some(80),
        "https" => Some(443),
        _ => None,
    };
    let port = port.filter(|p| Some(*p) != default);
    Some((host, port))
}

/// Same scheme, host, and port. Used to answer "is this site installed?"
/// even when the current page is not exactly the manifest's start URL.
pub fn same_origin(a: &str, b: &str) -> bool {
    match (Origin::parse(a), Origin::parse(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// A PNG data URL the bridge produced. SVG and other formats are refused
/// because macOS's icon pipeline wants a bitmap.
pub fn decode_png_data_url(url: &str) -> Option<Vec<u8>> {
    let payload = url.strip_prefix("data:image/png;base64,")?;
    if payload.len() > MAX_ICON_BYTES.div_ceil(3) * 4 + 4 {
        return None;
    }
    let bytes = base64_decode(payload)?;
    if bytes.len() < 8 || bytes.len() > MAX_ICON_BYTES || !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    Some(bytes)
}

pub fn base64_decode(input: &str) -> Option<Vec<u8>> {
    let bytes = input.as_bytes();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (i, chunk) in bytes.chunks(4).enumerate() {
        let last = i + 1 == bytes.len() / 4;
        let a = b64(chunk[0])?;
        let b = b64(chunk[1])?;
        let c = if chunk[2] == b'=' { 0 } else { b64(chunk[2])? };
        let d = if chunk[3] == b'=' { 0 } else { b64(chunk[3])? };
        if chunk[2] == b'=' && (!last || chunk[3] != b'=') {
            return None;
        }
        if chunk[3] == b'=' && !last {
            return None;
        }
        out.push(a << 2 | b >> 4);
        if chunk[2] != b'=' {
            out.push(b << 4 | c >> 2);
        }
        if chunk[3] != b'=' {
            out.push(c << 6 | d);
        }
    }
    Some(out)
}

fn b64(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Everything needed to write one bundle. Pure data so a test can inspect
/// the plist and launcher before anything touches the disk.
#[derive(Debug, Clone, PartialEq)]
pub struct BundlePlan {
    pub path: PathBuf,
    pub name: String,
    pub bundle_id: String,
    pub start_url: String,
    pub info_plist: String,
    pub launcher: String,
    pub marker: String,
    pub icon_png: Option<Vec<u8>>,
}

pub fn plan_bundle(apps_dir: &Path, manifest: &Manifest, source_binary: &Path) -> BundlePlan {
    let bundle_id = bundle_id(&manifest.start_url);
    let path = unique_bundle_path(apps_dir, &bundle_name(&manifest.name), &manifest.start_url);
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| manifest.name.clone());
    let info_plist = info_plist(&name, &bundle_id, manifest.icon_png.is_some());
    let icon = manifest
        .icon_png
        .as_ref()
        .map(|_| path.join("Contents/Resources/AppIcon.icns"));
    let launcher = launcher(&manifest.start_url, source_binary, &name, icon.as_deref());
    let marker = serde_json::json!({
        "relay_web_app": 1,
        "name": manifest.name,
        "start_url": manifest.start_url,
        "bundle_id": bundle_id,
        "version": DEFAULT_BUNDLE_VERSION,
    })
    .to_string();
    BundlePlan {
        path,
        name,
        bundle_id,
        start_url: manifest.start_url.clone(),
        info_plist,
        launcher,
        marker,
        icon_png: manifest.icon_png.clone(),
    }
}

fn unique_bundle_path(apps_dir: &Path, name: &str, start_url: &str) -> PathBuf {
    let base = apps_dir.join(format!("{name}.app"));
    if !base.exists() || marker_matches(&base, start_url) {
        return base;
    }
    for n in 2..100 {
        let candidate = apps_dir.join(format!("{name} {n}.app"));
        if !candidate.exists() || marker_matches(&candidate, start_url) {
            return candidate;
        }
    }
    apps_dir.join(format!("{name} {}.app", fnv1a(start_url.as_bytes())))
}

fn marker_path(bundle: &Path) -> PathBuf {
    bundle.join("Contents/.relay-webapp")
}

fn marker_matches(bundle: &Path, start_url: &str) -> bool {
    read_marker(bundle).is_some_and(|m| m.get("start_url").and_then(Value::as_str) == Some(start_url))
}

fn read_marker(bundle: &Path) -> Option<Value> {
    let text = fs::read_to_string(marker_path(bundle)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write the bundle and return its path. The marker is written last: a
/// half-written bundle never looks like a Relay app.
pub fn write_bundle(plan: &BundlePlan) -> io::Result<PathBuf> {
    let contents = plan.path.join("Contents");
    fs::create_dir_all(contents.join("MacOS"))?;
    fs::create_dir_all(contents.join("Resources"))?;
    fs::write(contents.join("Info.plist"), plan.info_plist.as_bytes())?;
    write_executable(&contents.join("MacOS/RelayApp"), plan.launcher.as_bytes())?;
    if let Some(png) = &plan.icon_png {
        let icon = contents.join("Resources/icon.png");
        fs::write(&icon, png)?;
        let icns = contents.join("Resources/AppIcon.icns");
        if !build_icns(&icon, &icns) {
            let _ = fs::remove_file(&icns);
        }
    }
    fs::write(marker_path(&plan.path), plan.marker.as_bytes())?;
    Ok(plan.path.clone())
}

/// Remove a Relay-created bundle only. A folder without a matching marker
/// is never deleted, even if the database points at it.
pub fn remove_bundle(bundle: &Path, start_url: &str) -> io::Result<()> {
    let name = bundle.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if !name.ends_with(".app") {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "not an app bundle"));
    }
    let marker = read_marker(bundle).ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "not an app installed by Relay"))?;
    if marker.get("relay_web_app").and_then(Value::as_u64) != Some(1)
        || marker.get("start_url").and_then(Value::as_str) != Some(start_url)
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "the bundle belongs to a different app"));
    }
    fs::remove_dir_all(bundle)
}

/// The result of a successful install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub name: String,
    pub bundle_id: String,
    pub path: PathBuf,
    pub start_url: String,
}

/// Install (or reinstall) a bundle under `apps_dir`.
pub fn install(apps_dir: &Path, manifest: &Manifest, source_binary: &Path) -> io::Result<Installed> {
    fs::create_dir_all(apps_dir)?;
    let plan = plan_bundle(apps_dir, manifest, source_binary);
    let path = write_bundle(&plan)?;
    Ok(Installed { name: plan.name, bundle_id: plan.bundle_id, path, start_url: plan.start_url })
}

pub fn default_apps_dir() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join("Applications"),
        None => relay_bridge::paths::relay_home().join("Applications"),
    }
}

fn info_plist(name: &str, bundle_id: &str, has_icon: bool) -> String {
    let icon = if has_icon {
        "  <key>CFBundleIconFile</key><string>AppIcon</string>\n"
    } else {
        ""
    };
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key><string>en</string>
  <key>CFBundleDisplayName</key><string>{name}</string>
  <key>CFBundleExecutable</key><string>RelayApp</string>
  <key>CFBundleIdentifier</key><string>{id}</string>
  <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
  <key>CFBundleName</key><string>{name}</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>CFBundleVersion</key><string>1</string>
{icon}  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  <key>LSMinimumSystemVersion</key><string>13.3</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
"#,
        name = xml_escape(name),
        id = xml_escape(bundle_id),
        version = xml_escape(DEFAULT_BUNDLE_VERSION),
        icon = icon,
    )
}

fn launcher(start_url: &str, source_binary: &Path, name: &str, icon: Option<&Path>) -> String {
    let src = shell_quote(&source_binary.to_string_lossy());
    let url = shell_quote(start_url);
    let name = shell_quote(name);
    let icon = icon.map(|p| format!(" --app-icon {}", shell_quote(&p.to_string_lossy()))).unwrap_or_default();
    format!(
        "#!/bin/sh\n\
         # Relay web app. Runs the installed Relay so the app shares its session,\n\
         # cookies, and updates; --app-name/--app-icon give it the site's identity.\n\
         set -eu\n\
         if [ -x {src} ]; then exec {src} --app {url} --app-name {name}{icon}; fi\n\
         # Relay moved: let LaunchServices find it by name and pass the same args.\n\
         exec open -a Relay --args --app {url} --app-name {name}{icon}\n"
    )
}

/// Single-quote a value for `/bin/sh`; the empty string becomes `''`.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn xml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c if c.is_control() => out.push(' '),
            c => out.push(c),
        }
    }
    out
}

#[cfg(unix)]
fn write_executable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, bytes)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn write_executable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    fs::write(path, bytes)
}

/// `sips` + `iconutil` are part of macOS; off-Mac there is nothing to do and
/// the bundle keeps the PNG for a later run.
#[cfg(target_os = "macos")]
fn build_icns(png: &Path, out: &Path) -> bool {
    let iconset = out.with_file_name("AppIcon.iconset");
    if fs::create_dir_all(&iconset).is_err() {
        return false;
    }
    let mut ok = true;
    for size in [16u32, 32, 128, 256, 512] {
        for scale in [1u32, 2] {
            let px = size * scale;
            let name = if scale == 1 { format!("icon_{size}x{size}.png") } else { format!("icon_{size}x{size}@2x.png") };
            if !run("sips", &["-z".into(), px.to_string(), px.to_string(), png.display().to_string(), "--out".into(), iconset.join(name).display().to_string()]) {
                ok = false;
                break;
            }
        }
        if !ok {
            break;
        }
    }
    if ok {
        ok = run("iconutil", &["-c".into(), "icns".into(), iconset.display().to_string(), "-o".into(), out.display().to_string()]);
    }
    let _ = fs::remove_dir_all(&iconset);
    ok && out.exists()
}

#[cfg(not(target_os = "macos"))]
fn build_icns(_png: &Path, _out: &Path) -> bool {
    false
}

#[cfg(target_os = "macos")]
fn run(program: &str, args: &[String]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PAGE: &str = "https://shop.relay.test/cart";

    fn manifest() -> Manifest {
        Manifest {
            name: "Relay Shop".into(),
            start_url: "https://shop.relay.test/app".into(),
            display: "standalone".into(),
            icon_png: None,
        }
    }

    #[test]
    fn parses_a_manifest_and_rejects_cross_origin_starts() {
        let good = json!({"available":true,"name":"  Relay   Shop ","start_url":"https://shop.relay.test/app","display":"standalone"});
        let m = manifest_from_result(&good, PAGE).unwrap();
        assert_eq!(m.name, "Relay Shop");
        assert_eq!(m.start_url, "https://shop.relay.test/app");
        assert_eq!(m.display, "standalone");

        let cross = json!({"available":true,"name":"Evil","start_url":"https://evil.test/app"});
        assert!(manifest_from_result(&cross, PAGE).unwrap_err().contains("different site"));

        let js = json!({"available":true,"name":"JS","start_url":"javascript:alert(1)"});
        assert!(manifest_from_result(&js, PAGE).is_err());

        let none = json!({"available":false,"reason":"no manifest"});
        assert_eq!(manifest_from_result(&none, PAGE).unwrap_err(), "no manifest");

        let other = json!({"available":false});
        assert!(manifest_from_result(&other, PAGE).is_err());
    }

    #[test]
    fn name_falls_back_and_cannot_break_the_bundle() {
        let m = json!({"available":true,"start_url":"https://shop.relay.test/app","name":"   "});
        assert_eq!(manifest_from_result(&m, PAGE).unwrap().name, "shop.relay.test");
        assert_eq!(display_name("a/b:c", "example.test"), "a/b:c");
        assert_eq!(bundle_name("a/b:c"), "a b c");
        assert_eq!(bundle_name("  ..Foo.app  "), "Foo");
        assert_eq!(bundle_name(""), "Web App");
        assert_eq!(display_name("x".repeat(200).as_str(), "h").chars().count(), MAX_NAME_CHARS);
    }

    #[test]
    fn origin_normalizes_ports_and_hosts() {
        assert_eq!(Origin::parse("HTTPS://Shop.Relay.Test:443/x"), Origin::parse("https://shop.relay.test/y"));
        assert_eq!(Origin::parse("http://a.test:80/"), Origin::parse("http://a.test/"));
        assert_ne!(Origin::parse("http://a.test/"), Origin::parse("https://a.test/"));
        assert_ne!(Origin::parse("http://a.test:8080/"), Origin::parse("http://a.test/"));
        assert_eq!(Origin::parse("https://[::1]:8443/"), Origin::parse("https://[::1]:8443/x"));
        assert_eq!(Origin::parse("file:///etc/passwd"), None);
        assert_eq!(Origin::parse("https:///nohost"), None);
        assert_eq!(Origin::parse("not a url"), None);
        assert!(same_origin("https://shop.test/cart", "https://shop.test/app/start"));
        assert!(!same_origin("https://shop.test/cart", "https://other.test/app"));
        assert!(!same_origin("https://shop.test/cart", "http://shop.test/app"));
        assert!(!same_origin("file:///etc/passwd", "file:///etc/passwd"));
    }

    #[test]
    fn base64_and_png_validation() {
        assert_eq!(base64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(base64_decode("aGVsbG8h").unwrap(), b"hello!");
        assert!(base64_decode("a").is_none());
        assert!(base64_decode("aGVsbG8*").is_none());
        let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[0u8; 8]].concat();
        let data = format!("data:image/png;base64,{}", base64_encode(&png));
        assert_eq!(decode_png_data_url(&data).unwrap(), png);
        assert!(decode_png_data_url("data:image/svg+xml;base64,PHN2Zz4=").is_none());
        assert!(decode_png_data_url("data:image/png;base64,aGVsbG8=").is_none(), "not a PNG");
    }

    fn base64_encode(bytes: &[u8]) -> String {
        const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let a = chunk[0];
            let b = chunk.get(1).copied().unwrap_or(0);
            let c = chunk.get(2).copied().unwrap_or(0);
            out.push(TABLE[(a >> 2) as usize] as char);
            out.push(TABLE[(((a & 3) << 4) | (b >> 4)) as usize] as char);
            out.push(if chunk.len() > 1 { TABLE[(((b & 15) << 2) | (c >> 6)) as usize] as char } else { '=' });
            out.push(if chunk.len() > 2 { TABLE[(c & 63) as usize] as char } else { '=' });
        }
        out
    }

    #[test]
    fn plist_and_launcher_escape_their_inputs() {
        assert_eq!(xml_escape(r#"a & <b> "c" 'd'"#), "a &amp; &lt;b&gt; &quot;c&quot; &apos;d&apos;");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
        assert_eq!(shell_quote(""), "''");
        let plan = plan_bundle(Path::new("/tmp/apps"), &manifest(), Path::new("/Applications/Relay.app/Contents/MacOS/relay"));
        assert!(plan.info_plist.contains("<string>Relay Shop</string>"));
        assert!(plan.info_plist.contains("<string>app.relay.webapp.shop-relay-test."));
        assert!(plan.launcher.contains("--app 'https://shop.relay.test/app'"));
        assert!(plan.launcher.contains("--app-name 'Relay Shop'"));
        assert!(plan.launcher.contains("exec '/Applications/Relay.app/Contents/MacOS/relay'"));
    }

    #[test]
    fn installing_writes_a_relaunchable_bundle_and_removes_only_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let apps = dir.path().join("Applications");
        let source = dir.path().join("relay");
        fs::write(&source, b"relay-binary").unwrap();
        let first = install(&apps, &manifest(), &source).unwrap();
        assert_eq!(first.path, apps.join("Relay Shop.app"));
        assert!(!first.path.join("Contents/MacOS/Relay").exists(), "no second copy of Relay is bundled");
        let launcher = fs::read_to_string(first.path.join("Contents/MacOS/RelayApp")).unwrap();
        assert!(launcher.starts_with("#!/bin/sh"));
        assert!(launcher.contains("--app 'https://shop.relay.test/app'"));
        assert!(launcher.contains("--app-name 'Relay Shop'"));
        let marker: Value = serde_json::from_str(&fs::read_to_string(marker_path(&first.path)).unwrap()).unwrap();
        assert_eq!(marker["start_url"], "https://shop.relay.test/app");
        assert_eq!(marker["relay_web_app"], 1);

        // Reinstalling the same app replaces it in place.
        let again = install(&apps, &manifest(), &source).unwrap();
        assert_eq!(again.path, first.path);

        // A same-named app for another URL gets its own folder.
        let mut other = manifest();
        other.start_url = "https://shop.relay.test/other".into();
        let second = install(&apps, &other, &source).unwrap();
        assert_eq!(second.path, apps.join("Relay Shop 2.app"));

        assert!(remove_bundle(&first.path, "https://shop.relay.test/other").is_err(), "wrong URL refused");
        assert!(first.path.exists());
        remove_bundle(&first.path, &first.start_url).unwrap();
        assert!(!first.path.exists());
    }

    #[test]
    fn an_icon_is_written_and_referenced() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("relay");
        fs::write(&source, b"relay-binary").unwrap();
        let png = [b"\x89PNG\r\n\x1a\n".as_slice(), &[0u8; 8]].concat();
        let mut m = manifest();
        m.icon_png = Some(png.clone());
        let installed = install(&dir.path().join("Applications"), &m, &source).unwrap();
        assert_eq!(fs::read(installed.path.join("Contents/Resources/icon.png")).unwrap(), png);
        let plist = fs::read_to_string(installed.path.join("Contents/Info.plist")).unwrap();
        assert!(plist.contains("<key>CFBundleIconFile</key><string>AppIcon</string>"));
        let launcher = fs::read_to_string(installed.path.join("Contents/MacOS/RelayApp")).unwrap();
        assert!(launcher.contains("--app-icon '"));
    }

    #[cfg(unix)]
    #[test]
    fn the_launcher_runs_the_installed_relay_with_the_app_identity() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("relay");
        fs::write(&source, "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$RELAY_TEST_ARGS\"\n").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
        let installed = install(&dir.path().join("Applications"), &manifest(), &source).unwrap();
        let args = dir.path().join("args.txt");
        let launcher = installed.path.join("Contents/MacOS/RelayApp");
        assert!(std::process::Command::new(&launcher).env("RELAY_TEST_ARGS", &args).status().unwrap().success());
        let lines: Vec<String> = fs::read_to_string(&args).unwrap().lines().map(str::to_string).collect();
        assert_eq!(
            lines,
            vec!["--app", "https://shop.relay.test/app", "--app-name", "Relay Shop"],
            "the launcher passes the URL and the bundle's display name"
        );
    }

    #[test]
    fn a_foreign_folder_is_never_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Some.app");
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        fs::write(bundle.join("Contents/.relay-webapp"), "{}").unwrap();
        assert!(remove_bundle(&bundle, "https://a.test").is_err());
        assert!(bundle.exists());
        let plain = dir.path().join("Plain");
        assert!(remove_bundle(&plain, "https://a.test").is_err());
    }

    #[test]
    fn bundle_ids_are_stable_and_filename_safe() {
        let a = bundle_id("https://shop.relay.test/app");
        assert_eq!(a, bundle_id("https://shop.relay.test/app"));
        assert_ne!(a, bundle_id("https://shop.relay.test/other"));
        assert!(a.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'));
        let fallback = bundle_id("file:///etc/passwd");
        assert!(fallback.starts_with("app.relay.webapp.web."), "{fallback}");
        assert!(fallback.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-'));
    }
}
