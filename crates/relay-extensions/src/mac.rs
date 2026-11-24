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
use objc2_web_kit::{
    WKWebExtension, WKWebExtensionContext, WKWebExtensionContextPermissionStatus, WKWebExtensionController,
    WKWebExtensionPermission,
};
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
            // `relay install-extension` printed the requested permissions; installing is the consent.
            ctx.setGrantedPermissions(&grant_requested(&ext));
            // Host access too, or content scripts never run on any page.
            for pattern in ext.allRequestedMatchPatterns().iter() {
                ctx.setPermissionStatus_forMatchPattern(WKWebExtensionContextPermissionStatus::GrantedExplicitly, &pattern);
            }
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

/// The extension's options-page URL, if it declares one. The shell opens it
/// in a normal tab with the extension controller attached.
pub fn extension_options_url(controller: &Retained<WKWebExtensionController>, uid: &str) -> Option<String> {
    for ctx in unsafe { controller.extensionContexts() }.iter() {
        if unsafe { ctx.uniqueIdentifier() }.to_string() == uid {
            let url = unsafe { ctx.optionsPageURL() }?;
            return Some(unsafe { url.absoluteString() }?.to_string());
        }
    }
    None
}

/// Result of a Keychain import; never contains secrets.
#[derive(Debug, Default, PartialEq)]
pub struct KeychainImport {
    pub saved: usize,
    pub failed: Vec<String>,
}

/// Store each credential as an internet password (protocol HTTPS, HTML form),
/// the item type Safari AutoFill reads.
pub fn import_passwords(creds: &[Credential], profile: Option<&str>) -> KeychainImport {
    use security_framework::passwords::set_internet_password;
    use security_framework_sys::keychain::{SecAuthenticationType, SecProtocolType};
    let mut r = KeychainImport::default();
    for c in creds {
        let Some((https, host, port, path)) = split_url(&c.url) else {
            r.failed.push(format!("{}: unparseable URL", c.url));
            continue;
        };
        let proto = if https { SecProtocolType::HTTPS } else { SecProtocolType::HTTP };
        match set_internet_password(&host, profile, &c.username, &path, port, proto, SecAuthenticationType::HTMLForm, c.password.as_bytes()) {
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

/// List non-secret Keychain internet-password metadata matching `origin` exactly.
/// Never sets `load_data(true)`, so no password bytes leave the Keychain here.
pub fn list_logins(origin: &crate::login::Origin) -> Vec<crate::login::SavedLoginMeta> {
    use crate::login::{Origin, SavedLoginMeta};
    use security_framework::item::{ItemClass, ItemSearchOptions, Limit};

    if !origin.is_secure() {
        return Vec::new();
    }
    let Ok(results) = ItemSearchOptions::new()
        .class(ItemClass::internet_password())
        .load_attributes(true)
        .limit(Limit::Max(256))
        .search()
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (idx, item) in results.into_iter().enumerate() {
        let Some(dict) = item.simplify_dict() else { continue };
        let Some(server) = dict.get("srvr").map(String::as_str).filter(|s| !s.is_empty()) else { continue };
        let scheme = match dict.get("ptcl").map(String::as_str) {
            Some("htps") => "https",
            Some("http") => "http",
            _ => continue,
        };
        let port = dict
            .get("port")
            .and_then(|p| p.parse::<u16>().ok())
            .filter(|&p| p > 0);
        let url = match port {
            Some(p) => format!("{scheme}://{server}:{p}/"),
            None => format!("{scheme}://{server}/"),
        };
        let Ok(item_origin) = Origin::parse(&url) else { continue };
        if item_origin != *origin {
            continue;
        }
        let username = dict.get("acct").cloned().unwrap_or_default();
        let path = dict.get("path").map(String::as_str).unwrap_or("/");
        out.push(SavedLoginMeta::new((idx + 1) as u64, item_origin, username, path, 0));
    }
    out
}

/// Keychain status → the model's failure kind. A user who cancels the
/// Keychain prompt is not the same as an unavailable Keychain.
fn failure_from(err: security_framework::base::Error) -> crate::login::SaveFailure {
    use crate::login::SaveFailure;
    match err.code() {
        -128 | -25293 | -25308 => SaveFailure::Denied,
        _ => SaveFailure::Unavailable,
    }
}

/// Read the secret password for one explicitly confirmed Keychain candidate.
pub fn read_login_password(
    origin: &crate::login::Origin,
    username: &str,
    path: &str,
    profile: Option<&str>,
) -> Result<crate::login::Password, crate::login::SaveFailure> {
    use crate::login::{Password, SaveFailure};
    use security_framework::passwords::get_internet_password;
    use security_framework_sys::keychain::{SecAuthenticationType, SecProtocolType};

    if !origin.is_secure() {
        return Err(SaveFailure::InvalidItem);
    }
    let proto = if origin.is_https() { SecProtocolType::HTTPS } else { SecProtocolType::HTTP };
    let bytes = match get_internet_password(origin.host(), profile, username, path, origin.port(), proto, SecAuthenticationType::HTMLForm) {
        Ok(bytes) => bytes,
        // Only a missing exact path falls back to the origin root; do not
        // re-prompt a denied Keychain request.
        Err(e) if e.code() == -25300 => {
            get_internet_password(origin.host(), profile, username, "", origin.port(), proto, SecAuthenticationType::HTMLForm)
                .map_err(failure_from)?
        }
        Err(e) => return Err(failure_from(e)),
    };
    let text = String::from_utf8(bytes).map_err(|_| SaveFailure::InvalidItem)?;
    Ok(Password::new(text))
}

/// Write or update one captured login in the macOS login Keychain.
pub fn save_login(login: &crate::login::CapturedLogin, profile: Option<&str>) -> Result<(), crate::login::SaveFailure> {
    use crate::login::SaveFailure;
    use security_framework::passwords::set_internet_password;
    use security_framework_sys::keychain::{SecAuthenticationType, SecProtocolType};

    if !login.origin().is_secure() || !login.is_usable() {
        return Err(SaveFailure::InvalidItem);
    }
    let proto = if login.origin().is_https() { SecProtocolType::HTTPS } else { SecProtocolType::HTTP };
    set_internet_password(
        login.origin().host(),
        profile,
        login.username(),
        login.path(),
        login.origin().port(),
        proto,
        SecAuthenticationType::HTMLForm,
        login.password().as_bytes(),
    )
    .map_err(failure_from)
}

/// Delete one Keychain internet password for `origin` + `username`.
pub fn delete_login(origin: &crate::login::Origin, username: &str, path: &str, profile: Option<&str>) -> Result<bool, crate::login::SaveFailure> {
    use crate::login::SaveFailure;
    use security_framework::passwords::delete_internet_password;
    use security_framework_sys::keychain::{SecAuthenticationType, SecProtocolType};

    if !origin.is_secure() {
        return Err(SaveFailure::InvalidItem);
    }
    let proto = if origin.is_https() { SecProtocolType::HTTPS } else { SecProtocolType::HTTP };
    match delete_internet_password(origin.host(), profile, username, path, origin.port(), proto, SecAuthenticationType::HTMLForm) {
        Ok(()) => Ok(true),
        Err(e) if e.code() == -25300 => {
            match delete_internet_password(origin.host(), profile, username, "", origin.port(), proto, SecAuthenticationType::HTMLForm) {
                Ok(()) => Ok(true),
                Err(e) if e.code() == -25300 => Ok(false),
                Err(e) => Err(failure_from(e)),
            }
        }
        Err(e) => Err(failure_from(e)),
    }
}
