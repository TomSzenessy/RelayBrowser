//! macOS-only: register unpacked extensions with WKWebExtension and move
//! imported passwords into the login Keychain (where Safari/WebKit AutoFill
//! and Touch ID find them).
//!
//! VERIFY-ON-MAC: type-checked for aarch64-apple-darwin on Linux; exercised only on a Mac.

use crate::chrome_import::Credential;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::MainThreadMarker;
use objc2_foundation::{NSDate, NSDictionary, NSError, NSString, NSURL};
use objc2_web_kit::{WKWebExtension, WKWebExtensionContext, WKWebExtensionController, WKWebExtensionPermission};
use std::path::Path;

/// Load an unpacked extension directory into `controller`. Every tab whose
/// configuration uses this controller sees it. `done` gets the display name.
pub fn load_extension(
    mtm: MainThreadMarker,
    controller: &Retained<WKWebExtensionController>,
    dir: &Path,
    unique_id: &str,
    done: impl Fn(Result<String, String>) + 'static,
) {
    let Some(url) = NSURL::from_directory_path(dir) else {
        done(Err(format!("bad extension path {}", dir.display())));
        return;
    };
    let controller = controller.clone();
    let uid = NSString::from_str(unique_id);
    let block = RcBlock::new(move |ext: *mut WKWebExtension, err: *mut NSError| {
        if let Some(e) = unsafe { err.as_ref() } {
            done(Err(e.localizedDescription().to_string()));
            return;
        }
        let Some(ext) = (unsafe { Retained::retain(ext) }) else {
            done(Err("WebKit returned no extension".into()));
            return;
        };
        unsafe {
            let ctx = WKWebExtensionContext::contextForExtension(&ext);
            ctx.setUniqueIdentifier(&uid);
            // The install sheet already showed the requested permissions.
            ctx.setGrantedPermissions(&grant_requested(&ext));
            match controller.loadExtensionContext_error(&ctx) {
                Ok(()) => done(Ok(ext.displayName().map(|n| n.to_string()).unwrap_or_else(|| uid.to_string()))),
                Err(e) => done(Err(e.localizedDescription().to_string())),
            }
        }
    });
    unsafe { WKWebExtension::extensionWithResourceBaseURL_completionHandler(&url, &block, mtm) };
}

fn grant_requested(ext: &WKWebExtension) -> Retained<NSDictionary<WKWebExtensionPermission, NSDate>> {
    let requested = unsafe { ext.requestedPermissions() };
    let keys: Vec<Retained<WKWebExtensionPermission>> = requested.iter().collect();
    let key_refs: Vec<&WKWebExtensionPermission> = keys.iter().map(|k| &**k).collect();
    let forever = NSDate::distantFuture();
    let values: Vec<Retained<NSDate>> = keys.iter().map(|_| forever.clone()).collect();
    NSDictionary::from_retained_objects(&key_refs, &values)
}

/// Result of a Keychain import; never contains secrets.
#[derive(Debug, Default, PartialEq)]
pub struct KeychainImport {
    pub saved: usize,
    pub failed: Vec<String>,
}

/// Store each credential as an internet password (protocol HTTPS, HTML form),
/// the item type Safari AutoFill reads.
pub fn import_passwords(creds: &[Credential]) -> KeychainImport {
    use security_framework::passwords::set_internet_password;
    use security_framework_sys::keychain::{SecAuthenticationType, SecProtocolType};
    let mut r = KeychainImport::default();
    for c in creds {
        let Some((https, host, port, path)) = split_url(&c.url) else {
            r.failed.push(format!("{}: unparseable URL", c.url));
            continue;
        };
        let proto = if https { SecProtocolType::HTTPS } else { SecProtocolType::HTTP };
        match set_internet_password(&host, None, &c.username, &path, port, proto, SecAuthenticationType::HTMLForm, c.password.as_bytes()) {
            Ok(()) => r.saved += 1,
            Err(e) => r.failed.push(format!("{host}: {e}")),
        }
    }
    r
}

fn split_url(url: &str) -> Option<(bool, String, Option<u16>, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let https = match scheme.to_ascii_lowercase().as_str() {
        "https" => true,
        "http" => false,
        _ => return None,
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].split(['?', '#']).next().unwrap_or("")),
        None => (rest, ""),
    };
    let authority = authority.rsplit('@').next()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h, Some(p.parse().ok()?)),
        None => (authority, None),
    };
    (!host.is_empty()).then(|| (https, host.to_ascii_lowercase(), port, path.to_string()))
}
