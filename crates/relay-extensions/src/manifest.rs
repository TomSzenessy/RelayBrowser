//! Extension `manifest.json`: localized names and a WebKit compatibility
//! report shown before install, so users know which Chrome APIs Relay shims
//! and which WebKit lacks.

use crate::Error;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct Manifest {
    pub manifest_version: u8,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub default_locale: Option<String>,
    #[serde(default)]
    pub permissions: Vec<Value>,
    #[serde(default)]
    pub optional_permissions: Vec<Value>,
    #[serde(default)]
    pub host_permissions: Vec<String>,
    #[serde(default)]
    pub side_panel: Option<Value>,
    #[serde(default)]
    pub background: Option<Value>,
}

/// Chrome APIs WebKit's WKWebExtension does not implement, and whether Relay
/// shims them (js/chrome-shims.js).
const GAPS: &[(&str, Support)] = &[
    ("sidePanel", Support::Shimmed),
    ("offscreen", Support::Shimmed),
    ("identity", Support::Missing),
    ("debugger", Support::Missing),
    ("tabCapture", Support::Missing),
    ("desktopCapture", Support::Missing),
    ("enterprise.platformKeys", Support::Missing),
    ("gcm", Support::Missing),
    ("tts", Support::Missing),
    ("ttsEngine", Support::Missing),
    ("proxy", Support::Missing),
    ("webRequestBlocking", Support::Missing),
    ("nativeMessaging", Support::Partial),
    ("downloads", Support::Partial),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Support {
    Shimmed,
    Partial,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionReport {
    pub requested: Vec<String>,
    pub optional: Vec<String>,
    pub hosts: Vec<String>,
}

impl PermissionReport {
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.push(if self.requested.is_empty() { "requested permissions: none".to_string() } else { format!("requested permissions: {}", self.requested.join(", ")) });
        if !self.optional.is_empty() {
            out.push(format!("optional permissions: {}", self.optional.join(", ")));
        }
        out.push(if self.hosts.is_empty() { "host access: none".to_string() } else { format!("host access: {}", self.hosts.join(", ")) });
        out
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Compatibility {
    pub shimmed: Vec<String>,
    pub partial: Vec<String>,
    pub missing: Vec<String>,
    /// Host match patterns the extension asked for (granted at install).
    pub hosts: Vec<String>,
    /// MV2 background pages are unsupported in WebKit for new installs.
    pub warnings: Vec<String>,
}

impl Compatibility {
    /// The one user-facing report; install prints exactly these lines.
    pub fn report_lines(&self) -> Vec<String> {
        let mut out = vec![];
        if !self.shimmed.is_empty() {
            out.push(format!("shimmed by Relay: {}", self.shimmed.join(", ")));
        }
        if !self.partial.is_empty() {
            out.push(format!("partial (some APIs may do nothing): {}", self.partial.join(", ")));
        }
        if !self.missing.is_empty() {
            out.push(format!("missing in WebKit: {}", self.missing.join(", ")));
        }
        if !self.hosts.is_empty() {
            out.push(format!("host access: {}", self.hosts.join(", ")));
        }
        for w in &self.warnings {
            out.push(format!("warning: {w}"));
        }
        out
    }
}

impl Manifest {
    pub fn read(dir: &Path) -> Result<Self, Error> {
        Ok(serde_json::from_slice(&std::fs::read(dir.join("manifest.json"))?)?)
    }

    /// Resolve `__MSG_key__` names via `_locales/<default>/messages.json`.
    pub fn display_name(&self, dir: &Path) -> String {
        localize(&self.name, dir, self.default_locale.as_deref()).unwrap_or_else(|| self.name.clone())
    }

    /// What `relay install-extension` must show before granting anything:
    /// requested APIs, optional APIs, and host match patterns, deduplicated
    /// in manifest order. The same struct feeds the printed report and the
    /// docs' statement that installing is blanket consent.
    pub fn permission_report(&self) -> PermissionReport {
        fn dedup(values: impl IntoIterator<Item = String>) -> Vec<String> {
            let mut seen = std::collections::HashSet::new();
            values.into_iter().filter(|v| seen.insert(v.clone())).collect()
        }
        PermissionReport {
            requested: dedup(self.permissions.iter().filter_map(|p| p.as_str().map(str::to_string))),
            optional: dedup(self.optional_permissions.iter().filter_map(|p| p.as_str().map(str::to_string))),
            hosts: dedup(self.host_permissions.iter().cloned()),
        }
    }

    pub fn permission_names(&self) -> Vec<String> {
        self.permissions.iter().chain(&self.optional_permissions).filter_map(|p| p.as_str().map(str::to_string)).collect()
    }

    pub fn compatibility(&self) -> Compatibility {
        let perms = self.permission_names();
        let mut c = Compatibility { shimmed: vec![], partial: vec![], missing: vec![], hosts: self.host_permissions.clone(), warnings: vec![] };
        for (api, support) in GAPS {
            let used = perms.iter().any(|p| p == api) || (*api == "sidePanel" && self.side_panel.is_some());
            if used {
                match support {
                    Support::Shimmed => c.shimmed.push(api.to_string()),
                    Support::Partial => c.partial.push(api.to_string()),
                    Support::Missing => c.missing.push(api.to_string()),
                }
            }
        }
        if self.manifest_version < 3 {
            c.warnings.push("Manifest V2: WebKit loads it, but Chrome has retired MV2; expect breakage.".into());
        }
        if self.background.as_ref().and_then(|b| b.get("persistent")).and_then(|p| p.as_bool()) == Some(true) {
            c.warnings.push("Persistent background pages are not supported; WebKit makes them non-persistent.".into());
        }
        c
    }
}

fn localize(name: &str, dir: &Path, locale: Option<&str>) -> Option<String> {
    let key = name.strip_prefix("__MSG_")?.strip_suffix("__")?;
    let locales = [locale.unwrap_or("en"), "en", "en_US"];
    for loc in locales {
        let Ok(bytes) = std::fs::read(dir.join("_locales").join(loc).join("messages.json")) else { continue };
        let Ok(v) = serde_json::from_slice::<Value>(&bytes) else { continue };
        // Message keys are case-insensitive in Chrome.
        if let Some(obj) = v.as_object() {
            for (k, entry) in obj {
                if k.eq_ignore_ascii_case(key) {
                    return entry.get("message").and_then(|m| m.as_str()).map(str::to_string);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_report_lists_requested_optional_and_hosts_once() {
        let m: Manifest = serde_json::from_str(r#"{"manifest_version":3,"name":"X","version":"1.0","permissions":["storage","tabs","storage"],"optional_permissions":["downloads"],"host_permissions":["https://*.example.com/*","https://*.example.com/*"]}"#).unwrap();
        let r = m.permission_report();
        assert_eq!(r.requested, vec!["storage", "tabs"]);
        assert_eq!(r.optional, vec!["downloads"]);
        assert_eq!(r.hosts, vec!["https://*.example.com/*"]);
        let lines = r.lines();
        assert!(lines.iter().any(|l| l.contains("storage, tabs")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("optional permissions: downloads")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("host access: https://*.example.com/*")), "{lines:?}");
        let empty: Manifest = serde_json::from_str(r#"{"manifest_version":3,"name":"E","version":"1"}"#).unwrap();
        assert!(empty.permission_report().lines().iter().any(|l| l == "requested permissions: none"));
    }

    #[test]
    fn compatibility_report() {
        let m: Manifest = serde_json::from_str(
            r#"{"manifest_version":3,"name":"X","version":"1","permissions":["storage","sidePanel","offscreen","identity","downloads"],"host_permissions":["https://*.example.com/*"]}"#,
        )
        .unwrap();
        let c = m.compatibility();
        assert_eq!(c.shimmed, ["sidePanel", "offscreen"]);
        assert_eq!(c.partial, ["downloads"]);
        assert_eq!(c.missing, ["identity"]);
        assert_eq!(c.hosts, ["https://*.example.com/*"]);
        let lines = c.report_lines();
        assert!(lines.iter().any(|l| l.contains("downloads")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("host access")), "{lines:?}");
    }

    #[test]
    fn mv2_persistent_background_warns() {
        let m: Manifest = serde_json::from_str(r#"{"manifest_version":2,"name":"X","version":"1","background":{"scripts":["b.js"],"persistent":true}}"#).unwrap();
        let c = m.compatibility();
        assert_eq!(c.warnings.len(), 2);
    }
}

pub const SHIM_FILE: &str = "relay-chrome-shims.js";
const SW_WRAPPER: &str = "relay-service-worker.js";

/// Install `js/chrome-shims.js` into an unpacked extension so it runs before
/// the extension's own background code:
/// - MV3 classic service worker: a wrapper worker `importScripts` the shim then the original;
/// - MV3 module worker: the wrapper `import`s both;
/// - background scripts/page scripts: the shim is prepended.
pub fn install_shims(dir: &Path) -> Result<(), Error> {
    let path = dir.join("manifest.json");
    let mut m: Value = serde_json::from_slice(&std::fs::read(&path)?)?;
    std::fs::write(dir.join(SHIM_FILE), crate::CHROME_SHIMS_JS)?;
    let Some(bg) = m.get_mut("background").and_then(|b| b.as_object_mut()) else {
        return Ok(()); // no background context: nothing calls sidePanel/offscreen
    };
    if let Some(sw) = bg.get("service_worker").and_then(|s| s.as_str()).map(str::to_string) {
        if sw == SW_WRAPPER {
            return Ok(()); // already installed
        }
        let module = bg.get("type").and_then(|t| t.as_str()) == Some("module");
        let js_str = |s: &str| serde_json::to_string(s).expect("string");
        let wrapper = if module {
            format!("import {};\nimport {};\n", js_str(&format!("./{SHIM_FILE}")), js_str(&format!("./{}", sw.trim_start_matches('/'))))
        } else {
            format!("importScripts({}, {});\n", js_str(SHIM_FILE), js_str(sw.trim_start_matches('/')))
        };
        std::fs::write(dir.join(SW_WRAPPER), wrapper)?;
        bg.insert("service_worker".into(), Value::String(SW_WRAPPER.into()));
    } else if let Some(scripts) = bg.get_mut("scripts").and_then(|s| s.as_array_mut()) {
        if scripts.first().and_then(|s| s.as_str()) != Some(SHIM_FILE) {
            scripts.insert(0, Value::String(SHIM_FILE.into()));
        }
    }
    std::fs::write(&path, serde_json::to_vec_pretty(&m)?)?;
    Ok(())
}

#[cfg(test)]
mod shim_tests {
    use super::*;

    fn ext(manifest: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("manifest.json"), manifest).unwrap();
        d
    }

    #[test]
    fn classic_service_worker_gets_a_wrapper() {
        let d = ext(r#"{"manifest_version":3,"name":"x","version":"1","background":{"service_worker":"bg.js"}}"#);
        install_shims(d.path()).unwrap();
        install_shims(d.path()).unwrap(); // idempotent
        let m: Value = serde_json::from_slice(&std::fs::read(d.path().join("manifest.json")).unwrap()).unwrap();
        assert_eq!(m["background"]["service_worker"], SW_WRAPPER);
        let w = std::fs::read_to_string(d.path().join(SW_WRAPPER)).unwrap();
        assert_eq!(w, "importScripts(\"relay-chrome-shims.js\", \"bg.js\");\n");
        assert!(std::fs::read_to_string(d.path().join(SHIM_FILE)).unwrap().contains("chrome.sidePanel"));
    }

    #[test]
    fn module_worker_and_background_scripts() {
        let d = ext(r#"{"manifest_version":3,"name":"x","version":"1","background":{"service_worker":"sw.mjs","type":"module"}}"#);
        install_shims(d.path()).unwrap();
        assert!(std::fs::read_to_string(d.path().join(SW_WRAPPER)).unwrap().starts_with("import \"./relay-chrome-shims.js\";"));
        let d = ext(r#"{"manifest_version":2,"name":"x","version":"1","background":{"scripts":["a.js"]}}"#);
        install_shims(d.path()).unwrap();
        install_shims(d.path()).unwrap();
        let m: Value = serde_json::from_slice(&std::fs::read(d.path().join("manifest.json")).unwrap()).unwrap();
        assert_eq!(m["background"]["scripts"], serde_json::json!([SHIM_FILE, "a.js"]));
    }
}
