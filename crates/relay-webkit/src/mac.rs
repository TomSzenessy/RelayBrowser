//! macOS WKWebView driver: one WKWebView per awake tab, the bridge injected at
//! document start, bridge messages routed into the [`SharedHub`], content
//! rules applied, and sleep/wake via snapshots. Main thread only.
//!
//! VERIFY-ON-MAC: type-checked for aarch64-apple-darwin on Linux; behaviour is
//! verified only by running the app on macOS (see docs/verification.md).

use crate::downloads::{download_destination_in, folder as default_download_folder};
use crate::sleep::{restore_scroll_script, SleptTab};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSApplication, NSBitmapImageFileType, NSBitmapImageRep, NSEvent, NSImage, NSMenu, NSMenuItem, NSPasteboard, NSPasteboardTypeString,
    NSModalResponseOK, NSPrintInfo, NSSavePanel, NSTextField, NSUserInterfaceItemIdentification, NSView,
};
use objc2_app_kit::NSWorkspace;
use objc2_media_player::{MPRemoteCommandCenter, MPRemoteCommandHandlerStatus};
use objc2_foundation::{
    NSArray, NSBundle, NSData, NSDate, NSDictionary, NSError, NSHTTPCookie, NSNumber, NSObjectProtocol, NSPoint, NSProgressReporting, NSRect,
    NSSize, NSString, NSURL, NSURLRequest, NSURLResponse, NSUUID,
};
use objc2_web_kit::{
    WKContentRuleList, WKContentRuleListStore, WKContentWorld, WKDownload, WKDownloadDelegate, WKFindConfiguration, WKFindResult, WKFrameInfo,
    WKNavigation, WKNavigationAction, WKNavigationDelegate, WKNavigationResponse, WKNavigationResponsePolicy, WKScriptMessage,
    WKMediaCaptureType, WKPermissionDecision, WKScriptMessageHandler, WKSecurityOrigin, WKUIDelegate, WKUserContentController, WKUserScript,
    WKUserScriptInjectionTime, WKWebExtensionController, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore, WKWindowFeatures,
};
use relay_bridge::{Cookie, SharedHub, TabId, BRIDGE_JS};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hash, Hasher};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::{Duration, Instant};

const HANDLER: &str = "relay";
const LOGIN_WORLD: &str = "relay.login";
const LOGIN_HANDLER: &str = "relayLogin";
const RULES_ID: &str = "relay-content-rules";
/// Native user-gesture window (D4 in docs/login-autofill.md): focus/submit
/// signals outside this window after a real NSView mouseDown/keyDown are ignored.
const TRUSTED_GESTURE_WINDOW: Duration = Duration::from_millis(1500);
/// Appended to WebKit's default user-agent via `applicationNameForUserAgent` so
/// sites like YouTube Music and Google Workspace recognize Relay as a modern
/// Safari/WebKit browser instead of rejecting it as an unsupported webview.
const SAFARI_UA_SUFFIX: &str = "Version/18.4 Safari/605.1.15";

/// Isolated-world listener: posts ONLY non-secret `{ kind, form }` signals to
/// `relayLogin` inside `WKContentWorld("relay.login")`. Never reads passwords.
const LOGIN_WATCH_JS: &str = r#"(function(){
  if (window.__relayLoginInstalled) return;
  window.__relayLoginInstalled = true;
  function findFormIndex(el) {
    var form = el && el.form ? el.form : (el && el.closest ? el.closest('form') : null);
    if (!form) return -1;
    var forms = Array.prototype.slice.call(document.forms || []);
    return forms.indexOf(form);
  }
  function fieldFrom(e) {
    var path = e.composedPath ? e.composedPath() : [];
    for (var i = 0; i < path.length; i++) {
      if (path[i] && path[i].tagName === 'INPUT') return path[i];
    }
    return e.target;
  }
  function isLoginField(el) {
    if (!el || el.tagName !== 'INPUT') return false;
    var t = (el.type || 'text').toLowerCase();
    if (t === 'hidden' || el.disabled || el.readOnly) return false;
    if (t === 'password') return true;
    // Two-step sign-ins show only the account field first.
    if (t === 'email') return true;
    if (t === 'text' || t === 'tel') {
      var ac = (el.autocomplete || '').toLowerCase();
      var name = ((el.name || '') + ' ' + (el.id || '')).toLowerCase().trim();
      if (ac.indexOf('username') !== -1 || ac.indexOf('email') !== -1) return true;
      return /^(user(name)?|email|login|account|identifier|userid|user-id|user_name)$/.test(name);
    }
    return false;
  }
  function post(obj) {
    obj.nonce = window.__relayLoginNonce || "";
    try { window.webkit.messageHandlers.relayLogin.postMessage(JSON.stringify(obj)); } catch (_) {}
  }
  document.addEventListener('focusin', function(e) {
    var field = fieldFrom(e);
    if (isLoginField(field)) post({ kind: 'focus', form: findFormIndex(field) });
  }, true);
  document.addEventListener('submit', function(e) {
    // A real user submission is trusted; requestSubmit()/form.submit() from
    // page script is not, so it cannot trigger a password capture.
    if (!e.isTrusted) return;
    var form = e.target;
    if (!form || !form.querySelector) return;
    if (!form.querySelector('input[type="password"]')) return;
    var forms = Array.prototype.slice.call(document.forms || []);
    post({ kind: 'submit', form: forms.indexOf(form) });
  }, true);
})();"#;

/// Isolated-world fill: receives `formIdx`, `username`, `password` as native
/// `NSDictionary` arguments (never interpolated into JS source — D2).
const LOGIN_FILL_JS: &str = r#"
  if (typeof nonce !== "string" || !nonce || window.__relayLoginNonce !== nonce) return false;
  function roots() {
    var out = [document];
    function walk(root) {
      if (out.length >= 50) return;
      var all = root.querySelectorAll('*');
      for (var i = 0; i < all.length; i++) {
        if (all[i].shadowRoot) { out.push(all[i].shadowRoot); walk(all[i].shadowRoot); }
        if (out.length >= 50) return;
      }
    }
    try { walk(document); } catch (e) {}
    return out;
  }
  var scope = (typeof formIdx === 'number' && formIdx >= 0 && document.forms[formIdx]) ? document.forms[formIdx] : null;
  var candidates = scope ? [scope] : roots();
  var pw = null, user = null;
  for (var s = 0; s < candidates.length && !pw; s++) {
    var root = candidates[s];
    pw = root.querySelector('input[type="password"]:not([disabled]):not([readonly])');
    if (!pw) continue;
    var inputs = Array.prototype.slice.call(root.querySelectorAll('input'));
    for (var i = 0; i < inputs.length; i++) {
      var inp = inputs[i];
      if (inp === pw) break;
      var t = (inp.type || 'text').toLowerCase();
      if ((t === 'email' || t === 'text' || t === 'tel') && !inp.disabled && !inp.readOnly) user = inp;
    }
  }
  if (!pw) {
    // A two-step sign-in: there is no password field yet, only the account.
    for (var r = 0; r < candidates.length && !user; r++) {
      var fields = candidates[r].querySelectorAll('input[type="email"], input[type="text"], input[type="tel"]');
      for (var j = 0; j < fields.length; j++) {
        var f = fields[j];
        var hint = ((f.autocomplete || '') + ' ' + (f.name || '') + ' ' + (f.id || '')).toLowerCase();
        if (!f.disabled && !f.readOnly && /user|email|login|account/.test(hint)) { user = f; break; }
      }
    }
  }
  var setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
  function setVal(el, val) {
    if (!el || typeof val !== 'string' || !val) return;
    setter.call(el, val);
    el.dispatchEvent(new Event('input', { bubbles: true }));
    el.dispatchEvent(new Event('change', { bubbles: true }));
  }
  if (!pw && !user) return false;
  if (user && username) setVal(user, username);
  if (pw) setVal(pw, password);
  return true;
"#;

/// Isolated-world capture: reads `{ username, password }` directly into native
/// completion handler memory without calling `postMessage`.
const LOGIN_CAPTURE_JS: &str = r#"
  if (typeof nonce !== "string" || !nonce || window.__relayLoginNonce !== nonce) return null;
  var root = (typeof formIdx === 'number' && formIdx >= 0 && document.forms[formIdx]) ? document.forms[formIdx] : document;
  var pw = root.querySelector('input[type="password"]');
  if (!pw || !pw.value) return null;
  var inputs = Array.prototype.slice.call(root.querySelectorAll('input'));
  var user = '';
  for (var i = 0; i < inputs.length; i++) {
    var inp = inputs[i];
    if (inp === pw) break;
    var t = (inp.type || 'text').toLowerCase();
    if ((t === 'email' || t === 'text' || t === 'tel') && inp.value) user = inp.value;
  }
  return { username: user, password: pw.value };
"#;

const LOGIN_HAS_PW_JS: &str = r#"
  var pws = document.querySelectorAll('input[type="password"]');
  for (var i = 0; i < pws.length; i++) {
    var r = pws[i].getBoundingClientRect();
    if (r.width > 0 && r.height > 0) return true;
  }
  return false;
"#;

/// A per-navigation value for the isolated login world. RandomState seeds
/// from the OS; page script cannot read the named content world, so this is a
/// freshness check rather than a secret.
fn new_login_nonce() -> String {
    let mut out = String::with_capacity(32);
    for _ in 0..2 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .hash(&mut hasher);
        std::process::id().hash(&mut hasher);
        out.push_str(&format!("{:016x}", hasher.finish()));
    }
    out
}

/// Register the system media keys. Handlers only set an atomic request code;
/// the shell drains it on its main-thread pump, which keeps the blocks free of
/// non-Send callbacks. Returns the opaque targets that must stay alive.
fn install_media_keys(media_key: Arc<AtomicU8>) -> Vec<Retained<AnyObject>> {
    let center = unsafe { MPRemoteCommandCenter::sharedCommandCenter() };
    let mut targets = Vec::new();
    for (command, code) in [
        (unsafe { center.playCommand() }, 1u8),
        (unsafe { center.pauseCommand() }, 2),
        (unsafe { center.togglePlayPauseCommand() }, 3),
    ] {
        unsafe { command.setEnabled(true) };
        let key = media_key.clone();
        let handler = RcBlock::new(move |_event: NonNull<objc2_media_player::MPRemoteCommandEvent>| {
            key.store(code, Ordering::Relaxed);
            MPRemoteCommandHandlerStatus::Success
        });
        targets.push(unsafe { command.addTargetWithHandler(&handler) });
    }
    targets
}

/// Things the engine needs the shell to do. Never carries passwords.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// `target=_blank` / `window.open`: open this URL in a new tab after `opener`.
    OpenInNewTab { opener: TabId, url: String },
    /// A sleeping tab's snapshot is ready (or failed).
    Slept(TabId),
    /// Native-trusted focus in a login form on `url` (main frame only).
    /// `-1` means the field is outside a `<form>` (common in SPAs).
    LoginFocus { tab: TabId, url: String, form_idx: i64 },
    /// The page answered a fill request: true when a password field was set.
    LoginFilled { tab: TabId, ok: bool },
    /// The default website data store finished removing cookies/cache.
    SiteDataCleared,
    /// The system answered a request to make Relay the default browser.
    DefaultBrowserSet { ok: bool, error: Option<String> },
    /// A trusted login submit succeeded on the same origin; offer to save
    /// `username` (the password remains strictly inside `Engine`).
    LoginSaveOffer { tab: TabId, url: String, username: String },
    /// The user allowed or blocked camera/mic for a host (ordinary tabs only).
    MediaPermission { host: String, kind: i64, granted: bool },
}

struct DownloadIvars {
    hub: SharedHub,
    id: u64,
    path: RefCell<Option<String>>,
    download: RefCell<Option<Retained<WKDownload>>>,
    mtm: MainThreadMarker,
    dir: Rc<RefCell<std::path::PathBuf>>,
    ask: Rc<Cell<bool>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; we do not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RelayDownloadHandler"]
    #[ivars = DownloadIvars]
    struct DownloadHandler;

    unsafe impl NSObjectProtocol for DownloadHandler {}

    unsafe impl WKDownloadDelegate for DownloadHandler {
        #[unsafe(method(download:decideDestinationUsingResponse:suggestedFilename:completionHandler:))]
        fn decide_destination(
            &self,
            _download: &WKDownload,
            _response: &NSURLResponse,
            suggested_filename: &NSString,
            completion_handler: &block2::DynBlock<dyn Fn(*mut NSURL)>,
        ) {
            let suggested = suggested_filename.to_string();
            let dir = self.ivars().dir.borrow().clone();
            if self.ivars().ask.get() {
                let panel = NSSavePanel::savePanel(self.ivars().mtm);
                panel.setNameFieldStringValue(&NSString::from_str(&crate::downloads::safe_filename(&suggested)));
                panel.setDirectoryURL(Some(&NSURL::fileURLWithPath(&NSString::from_str(&dir.to_string_lossy()))));
                if panel.runModal() == NSModalResponseOK {
                    if let Some(url) = panel.URL() {
                        if let Some(path) = url.path().map(|p| p.to_string()) {
                            let filename = std::path::Path::new(&path)
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| crate::downloads::safe_filename(&suggested));
                            self.ivars().hub.update(|h| h.set_download_destination(self.ivars().id, &filename, &path));
                            *self.ivars().path.borrow_mut() = Some(path);
                            completion_handler.call((Retained::as_ptr(&url) as *mut NSURL,));
                            return;
                        }
                    }
                }
                self.ivars().hub.update(|h| h.fail_download(self.ivars().id, "Download cancelled"));
                completion_handler.call((std::ptr::null_mut(),));
                return;
            }
            let (filename, path) = download_destination_in(&dir, &suggested);
            *self.ivars().path.borrow_mut() = Some(path.clone());
            self.ivars().hub.update(|h| h.set_download_destination(self.ivars().id, &filename, &path));
            let url = NSURL::fileURLWithPath(&NSString::from_str(&path));
            completion_handler.call((&*url as *const NSURL as *mut NSURL,));
        }

        #[unsafe(method(downloadDidFinish:))]
        fn did_finish(&self, _download: &WKDownload) {
            self.ivars().hub.update(|h| h.finish_download(self.ivars().id));
        }

        #[unsafe(method(download:didFailWithError:resumeData:))]
        fn did_fail(&self, _download: &WKDownload, error: &NSError, _resume_data: Option<&objc2_foundation::NSData>) {
            self.ivars().hub.update(|h| h.fail_download(self.ivars().id, &error.localizedDescription().to_string()));
        }
    }
);

impl DownloadHandler {
    fn new(mtm: MainThreadMarker, ivars: DownloadIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }

    fn cancel(&self) {
        self.ivars().hub.update(|h| h.cancel_download(self.ivars().id));
        if let Some(download) = self.ivars().download.borrow().as_ref() {
            unsafe { download.cancel(None) };
        }
    }

    fn progress(&self) -> Option<(u64, u64)> {
        let download = self.ivars().download.borrow();
        let download = download.as_ref()?;
        let progress = download.progress();
        let total = progress.totalUnitCount().max(0) as u64;
        let received = progress.completedUnitCount().max(0) as u64;
        (total > 0).then_some((received.min(total), total))
    }
}

/// Native-only in-memory login capture; never serialized, logged, or placed in Hub.
struct PendingCapture {
    url: String,
    username: String,
    /// Wiped on drop even when this capture is dropped without saving.
    password: zeroize::Zeroizing<String>,
}

/// A confirmed login handed to the shell; the password zeroizes on drop.
pub struct PendingLogin {
    pub url: String,
    pub username: String,
    pub password: zeroize::Zeroizing<String>,
}

impl std::fmt::Debug for PendingCapture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingCapture")
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

struct LoginHandlerIvars {
    tab: TabId,
    events: Rc<dyn Fn(EngineEvent)>,
    last_interaction: Rc<Cell<Option<Instant>>>,
    pending_captures: Rc<RefCell<HashMap<TabId, PendingCapture>>>,
    /// Off when the user disabled saving: the password is never read.
    capture: Rc<Cell<bool>>,
    /// The per-navigation value injected into the isolated world.
    nonce: Rc<RefCell<String>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; we do not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RelayLoginHandler"]
    #[ivars = LoginHandlerIvars]
    struct LoginHandler;

    unsafe impl NSObjectProtocol for LoginHandler {}

    unsafe impl WKScriptMessageHandler for LoginHandler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _ucc: &WKUserContentController, message: &WKScriptMessage) {
            if !unsafe { message.frameInfo().isMainFrame() } {
                return;
            }
            // D4: reject synthetic page focus/submit outside a real native mouse/key gesture.
            let trusted = self
                .ivars()
                .last_interaction
                .get()
                .is_some_and(|at| at.elapsed() <= TRUSTED_GESTURE_WINDOW);
            if !trusted {
                return;
            }
            let Some(view) = (unsafe { message.webView() }) else { return };
            let Some(url) = (unsafe { view.URL() }).and_then(|u| u.absoluteString()).map(|s| s.to_string()) else {
                return;
            };
            let body = unsafe { message.body() };
            let Some(raw) = body.downcast_ref::<NSString>().map(|s| s.to_string()) else { return };
            let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) else { return };
            let kind = val.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let nonce = val.get("nonce").and_then(|v| v.as_str()).unwrap_or("");
            if nonce.is_empty() || nonce != self.ivars().nonce.borrow().as_str() {
                return;
            }
            let form_idx = val.get("form").and_then(|v| v.as_i64()).unwrap_or(-1);
            let tab = self.ivars().tab;
            match kind {
                "focus" => {
                    (self.ivars().events)(EngineEvent::LoginFocus { tab, url, form_idx });
                }
                "submit" => {
                    if !self.ivars().capture.get() {
                        return;
                    }
                    let mtm = MainThreadMarker::from(&*view);
                    let world = unsafe { WKContentWorld::worldWithName(&NSString::from_str(LOGIN_WORLD), mtm) };
                    let key = NSString::from_str("formIdx");
                    let num = NSNumber::new_i64(form_idx);
                    let num_obj: &AnyObject = num.as_ref();
                    let nonce_key = NSString::from_str("nonce");
                    let nonce_value = NSString::from_str(&self.ivars().nonce.borrow());
                    let nonce_obj: &AnyObject = nonce_value.as_ref();
                    let args: Retained<NSDictionary<NSString, AnyObject>> =
                        NSDictionary::from_slices(&[&*key, &*nonce_key], &[num_obj, nonce_obj]);
                    let pending = self.ivars().pending_captures.clone();
                    let done = RcBlock::new(move |res: *mut AnyObject, _err: *mut NSError| {
                        let Some(obj) = (unsafe { res.as_ref() }) else { return };
                        let Some(dict) = obj.downcast_ref::<NSDictionary>() else { return };
                        let pw_key = NSString::from_str("password");
                        let pw = dict
                            .objectForKey(pw_key.as_ref())
                            .and_then(|v| v.downcast_ref::<NSString>().map(|s| s.to_string()))
                            .unwrap_or_default();
                        if pw.is_empty() || pw.len() > 4096 {
                            return;
                        }
                        let user_key = NSString::from_str("username");
                        let username = dict
                            .objectForKey(user_key.as_ref())
                            .and_then(|v| v.downcast_ref::<NSString>().map(|s| s.to_string()))
                            .unwrap_or_default();
                        pending.borrow_mut().insert(tab, PendingCapture { url: url.clone(), username, password: zeroize::Zeroizing::new(pw) });
                    });
                    unsafe {
                        view.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                            &NSString::from_str(LOGIN_CAPTURE_JS),
                            Some(&args),
                            None,
                            &world,
                            Some(&done),
                        );
                    }
                }
                _ => {}
            }
        }
    }
);

impl LoginHandler {
    fn new(mtm: MainThreadMarker, ivars: LoginHandlerIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }
}

pub struct HandlerIvars {
    tab: TabId,
    hub: SharedHub,
    events: Rc<dyn Fn(EngineEvent)>,
    downloads: RefCell<HashMap<u64, Retained<DownloadHandler>>>,
    pending_captures: Rc<RefCell<HashMap<TabId, PendingCapture>>>,
    offered_logins: Rc<RefCell<HashMap<TabId, PendingCapture>>>,
    /// HTTP status of the current main-frame response, for the save gate.
    last_status: Rc<Cell<Option<i64>>>,
    /// Shared with the login handler and the engine for this tab.
    login_nonce: Rc<RefCell<String>>,
    /// Private tabs ask every time and never share a remembered decision.
    private: bool,
    /// `(host, capture kind)` → allowed, shared across ordinary tabs.
    media_permissions: Rc<RefCell<HashMap<(String, i64), bool>>>,
    downloads_dir: Rc<RefCell<std::path::PathBuf>>,
    downloads_ask: Rc<Cell<bool>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; we do not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RelayBridgeHandler"]
    #[ivars = HandlerIvars]
    pub struct BridgeHandler;

    unsafe impl NSObjectProtocol for BridgeHandler {}

    unsafe impl WKScriptMessageHandler for BridgeHandler {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _ucc: &WKUserContentController, message: &WKScriptMessage) {
            // Only the main frame speaks for the tab; iframes could forge state.
            if !unsafe { message.frameInfo().isMainFrame() } {
                return;
            }
            let body = unsafe { message.body() };
            if let Some(s) = body.downcast_ref::<NSString>() {
                let _ = self.ivars().hub.ingest_json(self.ivars().tab, &s.to_string());
            }
        }
    }

    unsafe impl WKNavigationDelegate for BridgeHandler {
        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        fn did_start(&self, web_view: &WKWebView, _navigation: Option<&WKNavigation>) {
            self.ivars().last_status.set(None);
            // Rotate the isolated-world nonce on every provisional load and
            // inject it where page script cannot read it. Login focus/submit
            // signals and the fill helper compare against this value.
            let nonce = new_login_nonce();
            *self.ivars().login_nonce.borrow_mut() = nonce.clone();
            let quoted = serde_json::to_string(&nonce).unwrap_or_else(|_| "\"\"".into());
            let script = format!("window.__relayLoginNonce = {quoted};");
            let mtm = MainThreadMarker::from(web_view);
            let world = unsafe { WKContentWorld::worldWithName(&NSString::from_str(LOGIN_WORLD), mtm) };
            unsafe {
                web_view.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                    &NSString::from_str(&script),
                    None,
                    None,
                    &world,
                    None,
                );
            }
            if let Some(url) = unsafe { web_view.URL() }.and_then(|u| u.absoluteString()).map(|s| s.to_string()) {
                let tab = self.ivars().tab;
                self.ivars().hub.update(|h| h.navigation_started(tab, &url));
            }
        }

        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn decide_response(
            &self,
            _web_view: &WKWebView,
            response: &WKNavigationResponse,
            decision_handler: &block2::DynBlock<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            if unsafe { response.isForMainFrame() } {
                let raw = unsafe { response.response() };
                if let Some(http) = raw.downcast_ref::<objc2_foundation::NSHTTPURLResponse>() {
                    self.ivars().last_status.set(Some(http.statusCode() as i64));
                }
            }
            let policy = if unsafe { response.canShowMIMEType() } {
                WKNavigationResponsePolicy::Allow
            } else {
                WKNavigationResponsePolicy::Download
            };
            decision_handler.call((policy,));
        }

        #[unsafe(method(webView:navigationResponse:didBecomeDownload:))]
        fn did_become_download(&self, _web_view: &WKWebView, _response: &WKNavigationResponse, download: &WKDownload) {
            let url = unsafe { download.originalRequest() }
                .and_then(|r| r.URL())
                .and_then(|u| u.absoluteString())
                .map(|s| s.to_string())
                .unwrap_or_default();
            let filename = url.split(['/', '?']).next_back().filter(|s| !s.is_empty()).unwrap_or("download").to_string();
            let id = self.ivars().hub.update(|h| h.begin_download(self.ivars().tab, &url, &filename));
            let Some(mtm) = MainThreadMarker::new() else { return };
            let handler = DownloadHandler::new(
                mtm,
                DownloadIvars {
                    hub: self.ivars().hub.clone(),
                    id,
                    path: RefCell::new(None),
                    download: RefCell::new(unsafe { Retained::retain(download as *const WKDownload as *mut WKDownload) }),
                    mtm,
                    dir: self.ivars().downloads_dir.clone(),
                    ask: self.ivars().downloads_ask.clone(),
                },
            );
            unsafe { download.setDelegate(Some(ProtocolObject::from_ref(&*handler))) };
            self.ivars().downloads.borrow_mut().insert(id, handler);
        }

        #[unsafe(method(webView:didFinishNavigation:))]
        fn did_finish(&self, web_view: &WKWebView, _navigation: Option<&WKNavigation>) {
            let tab = self.ivars().tab;
            let Some(capture) = self.ivars().pending_captures.borrow_mut().remove(&tab) else {
                return;
            };
            let Some(dest_url) = (unsafe { web_view.URL() }).and_then(|u| u.absoluteString()).map(|s| s.to_string()) else {
                return;
            };
            if !same_origin_url(&capture.url, &dest_url) {
                return;
            }
            // Offer only after a real 2xx navigation. A 4xx/5xx page can
            // still render a form and must not trigger a password save.
            if !self.ivars().last_status.get().is_some_and(|code| (200..=299).contains(&code)) {
                return;
            }
            let mtm = MainThreadMarker::from(web_view);
            let world = unsafe { WKContentWorld::worldWithName(&NSString::from_str(LOGIN_WORLD), mtm) };
            let offered = self.ivars().offered_logins.clone();
            let events = self.ivars().events.clone();
            let capture_cell = Rc::new(RefCell::new(Some(capture)));
            let done = RcBlock::new(move |res: *mut AnyObject, err: *mut NSError| {
                if !err.is_null() {
                    return;
                }
                let has_pw = unsafe { res.as_ref() }
                    .and_then(|o| o.downcast_ref::<NSNumber>())
                    .is_some_and(|n| n.as_bool());
                if has_pw {
                    // Login form is still visible (likely wrong password): drop capture.
                    return;
                }
                if let Some(cap) = capture_cell.borrow_mut().take() {
                    let url = cap.url.clone();
                    let username = cap.username.clone();
                    offered.borrow_mut().insert(tab, cap);
                    events(EngineEvent::LoginSaveOffer { tab, url, username });
                }
            });
            unsafe {
                web_view.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                    &NSString::from_str(LOGIN_HAS_PW_JS),
                    None,
                    None,
                    &world,
                    Some(&done),
                );
            }
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(&self, _web_view: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.ivars().pending_captures.borrow_mut().remove(&self.ivars().tab);
            self.note_error(error);
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, _web_view: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.ivars().pending_captures.borrow_mut().remove(&self.ivars().tab);
            self.note_error(error);
        }

        /// WebKit killed the tab's WebContent process (memory pressure or a
        /// page crash). The view is dead: clear the stale page state and show
        /// the shell's Retry pane instead of a silent white tab.
        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn web_content_process_terminated(&self, web_view: &WKWebView) {
            let tab = self.ivars().tab;
            let url = unsafe { web_view.URL() }
                .and_then(|u| u.absoluteString())
                .map(|u| u.to_string())
                .or_else(|| self.ivars().hub.lock().tab(tab).map(|t| t.url.clone()))
                .unwrap_or_default();
            self.ivars().pending_captures.borrow_mut().remove(&tab);
            self.ivars().hub.update(|h| {
                h.navigation_started(tab, &url);
                if let Some(t) = h.tab_mut(tab) {
                    t.load_error = Some("The page's process stopped (usually memory pressure). Reload to continue.".into());
                    t.sleeping = false;
                }
            });
        }
    }

    unsafe impl WKUIDelegate for BridgeHandler {
        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create_web_view(
            &self,
            _web_view: &WKWebView,
            _configuration: &WKWebViewConfiguration,
            action: &WKNavigationAction,
            _features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            let url = unsafe { action.request().URL() }.and_then(|u| u.absoluteString()).map(|s| s.to_string());
            if let Some(url) = url {
                (self.ivars().events)(EngineEvent::OpenInNewTab { opener: self.ivars().tab, url });
            }
            None // we open a real Relay tab instead of a child window
        }

        #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
        fn media_capture_permission(
            &self,
            _web_view: &WKWebView,
            origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo,
            capture_type: WKMediaCaptureType,
            decision_handler: &block2::DynBlock<dyn Fn(WKPermissionDecision)>,
        ) {
            let host = unsafe { origin.host() }.to_string();
            let kind = i64::from(capture_type.0 as i32);
            let key = (host.clone(), kind);
            let remembered = if self.ivars().private {
                None
            } else {
                self.ivars().media_permissions.borrow().get(&key).copied()
            };
            let granted = match remembered {
                Some(granted) => granted,
                None => {
                    let mtm = MainThreadMarker::from(_web_view);
                    let alert = NSAlert::new(mtm);
                    let what = match capture_type {
                        WKMediaCaptureType::Microphone => "microphone",
                        WKMediaCaptureType::Camera => "camera",
                        _ => "camera and microphone",
                    };
                    alert.setMessageText(&NSString::from_str(&format!("{host} wants to use the {what}")));
                    alert.setInformativeText(&NSString::from_str(
                        "Allow only if you trust this site. Relay remembers the choice until it quits.",
                    ));
                    alert.addButtonWithTitle(&NSString::from_str("Allow"));
                    alert.addButtonWithTitle(&NSString::from_str("Block"));
                    let granted = alert.runModal() == NSAlertFirstButtonReturn;
                    if !self.ivars().private {
                        self.ivars().media_permissions.borrow_mut().insert(key, granted);
                        (self.ivars().events)(EngineEvent::MediaPermission { host: host.clone(), kind, granted });
                    }
                    granted
                }
            };
            decision_handler.call((if granted { WKPermissionDecision::Grant } else { WKPermissionDecision::Deny },));
        }

        #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn run_js_alert(
            &self,
            web_view: &WKWebView,
            message: &NSString,
            _frame: &WKFrameInfo,
            completion_handler: &block2::DynBlock<dyn Fn()>,
        ) {
            let mtm = MainThreadMarker::from(web_view);
            let alert = NSAlert::new(mtm);
            let host = unsafe { web_view.URL() }
                .and_then(|u| u.host())
                .map(|h| h.to_string())
                .unwrap_or_else(|| "This page".to_string());
            alert.setMessageText(&NSString::from_str(&format!("{host} says")));
            alert.setInformativeText(message);
            alert.addButtonWithTitle(&NSString::from_str("OK"));
            alert.runModal();
            completion_handler.call(());
        }

        #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn run_js_confirm(
            &self,
            web_view: &WKWebView,
            message: &NSString,
            _frame: &WKFrameInfo,
            completion_handler: &block2::DynBlock<dyn Fn(Bool)>,
        ) {
            let mtm = MainThreadMarker::from(web_view);
            let alert = NSAlert::new(mtm);
            let host = unsafe { web_view.URL() }
                .and_then(|u| u.host())
                .map(|h| h.to_string())
                .unwrap_or_else(|| "This page".to_string());
            alert.setMessageText(&NSString::from_str(&format!("{host} says")));
            alert.setInformativeText(message);
            alert.addButtonWithTitle(&NSString::from_str("OK"));
            alert.addButtonWithTitle(&NSString::from_str("Cancel"));
            let ok = alert.runModal() == NSAlertFirstButtonReturn;
            completion_handler.call((Bool::new(ok),));
        }

        #[unsafe(method(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:))]
        fn run_js_prompt(
            &self,
            web_view: &WKWebView,
            prompt: &NSString,
            default_text: Option<&NSString>,
            _frame: &WKFrameInfo,
            completion_handler: &block2::DynBlock<dyn Fn(*mut NSString)>,
        ) {
            let mtm = MainThreadMarker::from(web_view);
            let alert = NSAlert::new(mtm);
            let host = unsafe { web_view.URL() }
                .and_then(|u| u.host())
                .map(|h| h.to_string())
                .unwrap_or_else(|| "This page".to_string());
            alert.setMessageText(&NSString::from_str(&format!("{host} asks")));
            alert.setInformativeText(prompt);
            alert.addButtonWithTitle(&NSString::from_str("OK"));
            alert.addButtonWithTitle(&NSString::from_str("Cancel"));
            let field = NSTextField::initWithFrame(
                NSTextField::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(280.0, 24.0)),
            );
            if let Some(default_text) = default_text {
                field.setStringValue(default_text);
            }
            alert.setAccessoryView(Some(&field));
            if alert.runModal() == NSAlertFirstButtonReturn {
                let value = field.stringValue();
                completion_handler.call((&*value as *const NSString as *mut NSString,));
            } else {
                completion_handler.call((std::ptr::null_mut(),));
            }
        }
    }
);

impl BridgeHandler {
    fn new(mtm: MainThreadMarker, ivars: HandlerIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
    }

    fn cancel_download(&self, id: u64) {
        if let Some(handler) = self.ivars().downloads.borrow().get(&id) {
            handler.cancel();
        }
    }

    fn download_progress(&self) -> Vec<(u64, u64, u64)> {
        self.ivars()
            .downloads
            .borrow()
            .iter()
            .filter_map(|(id, handler)| handler.progress().map(|(received, total)| (*id, received, total)))
            .collect()
    }

    /// A failed main-frame load: WebKit renders its own blank error page, so
    /// the shell hides the view and shows a readable error with Retry.
    fn note_error(&self, error: &NSError) {
        if error.code() == -999 && error.domain().to_string() == "NSURLErrorDomain" {
            return;
        }
        let message = error.localizedDescription().to_string();
        let tab = self.ivars().tab;
        self.ivars().hub.update(|h| {
            if let Some(t) = h.tab_mut(tab) {
                t.load_error = Some(message);
            }
        });
    }
}

struct WebViewIvars {
    last_interaction: Rc<Cell<Option<Instant>>>,
}

define_class!(
    // SAFETY: WKWebView supports subclassing on the main thread; we do not implement Drop.
    #[unsafe(super(WKWebView))]
    #[thread_kind = MainThreadOnly]
    #[name = "RelayWebView"]
    #[ivars = WebViewIvars]
    struct RelayWebView;

    impl RelayWebView {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.ivars().last_interaction.set(Some(Instant::now()));
            let _: () = unsafe { msg_send![super(self), mouseDown: event] };
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            self.ivars().last_interaction.set(Some(Instant::now()));
            let _: () = unsafe { msg_send![super(self), keyDown: event] };
        }
        /// Customize WebKit's native right-click context menu: rename "Open Link in New Window"
        /// to "Open Link in New Tab" (matching our WKUIDelegate), remove the redundant
        /// "Open Link" item, and ensure Back, Forward, Reload, Copy Page URL, Print…, and
        /// Inspect Element are available on page menus.
        #[unsafe(method(willOpenMenu:withEvent:))]
        fn will_open_menu(&self, menu: &NSMenu, event: &NSEvent) {
            let _: () = unsafe { msg_send![super(self), willOpenMenu: menu, withEvent: event] };
            let mtm = MainThreadMarker::from(self);
            let mut has_back = false;
            let mut has_forward = false;
            let mut has_reload = false;
            let mut has_inspect = false;
            let mut is_link_or_selection = false;
            let mut open_link_idx: Option<isize> = None;

            let items = menu.itemArray();
            for (idx, item) in items.iter().enumerate() {
                let id = item.identifier().map(|s| s.to_string()).unwrap_or_default();
                match id.as_str() {
                    "WKMenuItemIdentifierOpenLink" => {
                        open_link_idx = Some(idx as isize);
                        is_link_or_selection = true;
                    }
                    "WKMenuItemIdentifierOpenLinkInNewWindow" => {
                        item.setTitle(&NSString::from_str("Open Link in New Tab"));
                        is_link_or_selection = true;
                    }
                    "WKMenuItemIdentifierCopyLink"
                    | "WKMenuItemIdentifierCopy"
                    | "WKMenuItemIdentifierCopyImage"
                    | "WKMenuItemIdentifierOpenImageInNewWindow"
                    | "WKMenuItemIdentifierPaste" => {
                        is_link_or_selection = true;
                    }
                    "WKMenuItemIdentifierGoBack" => has_back = true,
                    "WKMenuItemIdentifierGoForward" => has_forward = true,
                    "WKMenuItemIdentifierReload" => has_reload = true,
                    "WKMenuItemIdentifierInspectElement" => has_inspect = true,
                    _ => {}
                }
            }
            if let Some(idx) = open_link_idx {
                menu.removeItemAtIndex(idx);
            }

            if !is_link_or_selection {
                let mut insert_at: isize = 0;
                if !has_back {
                    let back = make_menu_item(mtm, "Back", sel!(relayGoBack:), self);
                    menu.insertItem_atIndex(&back, insert_at);
                    insert_at += 1;
                }
                if !has_forward {
                    let fwd = make_menu_item(mtm, "Forward", sel!(relayGoForward:), self);
                    menu.insertItem_atIndex(&fwd, insert_at);
                    insert_at += 1;
                }
                if !has_reload {
                    let reload = make_menu_item(mtm, "Reload", sel!(relayReload:), self);
                    menu.insertItem_atIndex(&reload, insert_at);
                }

                let sep_page = NSMenuItem::separatorItem(mtm);
                menu.addItem(&sep_page);
                let copy_url = make_menu_item(mtm, "Copy Page URL", sel!(relayCopyPageUrl:), self);
                menu.addItem(&copy_url);
                let reader = make_menu_item(mtm, "Reading Mode", sel!(relayToggleReader:), self);
                menu.addItem(&reader);
                let print = make_menu_item(mtm, "Print…", sel!(relayPrint:), self);
                menu.addItem(&print);
            }

            if !has_inspect {
                let sep = NSMenuItem::separatorItem(mtm);
                menu.addItem(&sep);
                let inspect = make_menu_item(mtm, "Inspect Element", sel!(relayInspectElement:), self);
                menu.addItem(&inspect);
            }
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            match item.action() {
                Some(a) if a == sel!(relayGoBack:) => unsafe { self.canGoBack() },
                Some(a) if a == sel!(relayGoForward:) => unsafe { self.canGoForward() },
                Some(a) if a == sel!(relayCopyPageUrl:) => unsafe { self.URL() }.is_some(),
                Some(a) if a == sel!(relayReload:) || a == sel!(relayPrint:) || a == sel!(relayInspectElement:) || a == sel!(relayToggleReader:) => true,
                _ => unsafe { msg_send![super(self), validateMenuItem: item] },
            }
        }

        #[unsafe(method(relayGoBack:))]
        fn relay_go_back(&self, _sender: Option<&AnyObject>) {
            unsafe { self.goBack() };
        }

        #[unsafe(method(relayGoForward:))]
        fn relay_go_forward(&self, _sender: Option<&AnyObject>) {
            unsafe { self.goForward() };
        }

        #[unsafe(method(relayReload:))]
        fn relay_reload(&self, _sender: Option<&AnyObject>) {
            unsafe { self.reload() };
        }

        #[unsafe(method(relayCopyPageUrl:))]
        fn relay_copy_page_url(&self, _sender: Option<&AnyObject>) {
            if let Some(url) = unsafe { self.URL() }.and_then(|u| u.absoluteString()) {
                let pb = NSPasteboard::generalPasteboard();
                pb.clearContents();
                let _ = pb.setString_forType(&url, unsafe { NSPasteboardTypeString });
            }
        }

        #[unsafe(method(relayToggleReader:))]
        fn relay_toggle_reader(&self, _sender: Option<&AnyObject>) {
            let js = NSString::from_str(
                "(function(){var on=document.documentElement.dataset.relayReader!=='1';return window.__relay.exec({id:0,op:'reader',on:on});})()",
            );
            unsafe {
                self.evaluateJavaScript_completionHandler(&js, None);
            }
        }

        #[unsafe(method(relayPrint:))]
        fn relay_print(&self, _sender: Option<&AnyObject>) {
            let info = NSPrintInfo::sharedPrintInfo();
            let op = unsafe { self.printOperationWithPrintInfo(&info) };
            let _ = op.runOperation();
        }

        #[unsafe(method(relayInspectElement:))]
        fn relay_inspect_element(&self, _sender: Option<&AnyObject>) {
            let view: &AnyObject = self.as_ref();
            let responds: bool = unsafe { msg_send![view, respondsToSelector: sel!(_inspector)] };
            if responds {
                let inspector: *mut AnyObject = unsafe { msg_send![view, _inspector] };
                if let Some(i) = unsafe { inspector.as_ref() } {
                    let _: () = unsafe { msg_send![i, show] };
                }
            }
        }
    }
);

impl RelayWebView {
    fn new(
        mtm: MainThreadMarker,
        frame: NSRect,
        config: &WKWebViewConfiguration,
        last_interaction: Rc<Cell<Option<Instant>>>,
    ) -> Retained<WKWebView> {
        let this = Self::alloc(mtm).set_ivars(WebViewIvars { last_interaction });
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame, configuration: config] };
        Retained::into_super(view)
    }
}

fn make_menu_item(mtm: MainThreadMarker, title: &str, action: objc2::runtime::Sel, target: &RelayWebView) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            Some(action),
            &NSString::from_str(""),
        )
    };
    unsafe { item.setTarget(Some(target.as_ref())) };
    item
}

pub struct WebTab {
    pub view: Retained<WKWebView>,
    handler: Retained<BridgeHandler>,
    login_handler: Retained<LoginHandler>,
    ucc: Retained<WKUserContentController>,
}

/// The editing commands that arrive from Relay's native Edit menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditCommand {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

pub struct Engine {
    mtm: MainThreadMarker,
    hub: SharedHub,
    events: Rc<dyn Fn(EngineEvent)>,
    rules: Rc<RefCell<Option<Retained<WKContentRuleList>>>>,
    blocking_enabled: Rc<std::cell::Cell<bool>>,
    /// Every live tab's controller, shared with the async rule compile so
    /// tabs opened before compilation finishes still get blocking.
    uccs: Rc<RefCell<Vec<Retained<WKUserContentController>>>>,
    extensions: Option<Retained<WKWebExtensionController>>,
    /// `Some(true)` forces DarkAqua, `Some(false)` forces Aqua, `None` follows system.
    appearance: Option<bool>,
    tabs: HashMap<TabId, WebTab>,
    slept: Rc<RefCell<HashMap<TabId, SleptTab>>>,
    /// Tabs woken from sleep: keep the native view hidden until the bridge
    /// delivers a DOM, so the snapshot the shell shows is not covered by a
    /// white WKWebView.
    warming: RefCell<HashSet<TabId>>,
    pending_restore: HashMap<TabId, String>,
    /// Native-only in-memory login captures awaiting post-navigation confirmation.
    pending_captures: Rc<RefCell<HashMap<TabId, PendingCapture>>>,
    /// Native-only confirmed login captures awaiting explicit user Save/Decline.
    offered_logins: Rc<RefCell<HashMap<TabId, PendingCapture>>>,
    /// Shared with every login handler; see `set_login_capture`.
    login_capture: Rc<Cell<bool>>,
    /// Per-tab isolated-world nonce, rotated on navigation.
    login_nonces: HashMap<TabId, Rc<RefCell<String>>>,
    /// Media-key request codes set from MediaPlayer, drained by the shell.
    media_key: Arc<AtomicU8>,
    /// Opaque targets returned by MPRemoteCommand; retained for the process.
    _media_targets: Vec<Retained<AnyObject>>,
    /// Session-level camera/microphone decisions for ordinary tabs.
    media_permissions: Rc<RefCell<HashMap<(String, i64), bool>>>,
    /// Named-profile data store; `None` uses WebKit's default store.
    profile_store: Option<Retained<WKWebsiteDataStore>>,
    downloads_dir: Rc<RefCell<std::path::PathBuf>>,
    downloads_ask: Rc<Cell<bool>>,
}

impl Engine {
    pub fn new(mtm: MainThreadMarker, hub: SharedHub, events: impl Fn(EngineEvent) + 'static) -> Self {
        let media_key = Arc::new(AtomicU8::new(0));
        let media_targets = install_media_keys(media_key.clone());
        Self {
            mtm,
            hub,
            events: Rc::new(events),
            rules: Rc::new(RefCell::new(None)),
            blocking_enabled: Rc::new(std::cell::Cell::new(false)),
            uccs: Rc::default(),
            extensions: None,
            appearance: None,
            tabs: HashMap::new(),
            slept: Rc::new(RefCell::new(HashMap::new())),
            warming: RefCell::new(HashSet::new()),
            pending_restore: HashMap::new(),
            pending_captures: Rc::default(),
            offered_logins: Rc::default(),
            login_capture: Rc::new(Cell::new(true)),
            login_nonces: HashMap::new(),
            media_key,
            _media_targets: media_targets,
            media_permissions: Rc::default(),
            profile_store: None,
            downloads_dir: Rc::new(RefCell::new(default_download_folder())),
            downloads_ask: Rc::new(Cell::new(false)),
        }
    }

    /// Remove cookies, cache, and local storage for every site. Used by
    /// Settings → Privacy; completion is reported through `EngineEvent`.
    pub fn clear_site_data(&self) {
        let store = match &self.profile_store {
            Some(store) => store.clone(),
            None => unsafe { WKWebsiteDataStore::defaultDataStore(self.mtm) },
        };
        let types = unsafe { WKWebsiteDataStore::allWebsiteDataTypes(self.mtm) };
        let events = self.events.clone();
        let done = RcBlock::new(move || events(EngineEvent::SiteDataCleared));
        unsafe {
            store.removeDataOfTypes_modifiedSince_completionHandler(&types, &NSDate::distantPast(), &done);
        }
    }

    /// Use a named profile's persistent website data store. The identifier is
    /// a stable UUID string derived from the profile name by the shell.
    pub fn set_profile_id(&mut self, id: &str) {
        if !objc2::available!(macos = 14.0) {
            eprintln!("relay: named profiles need macOS 14; using the default data store");
            return;
        }
        let Some(uuid) = NSUUID::initWithUUIDString(NSUUID::alloc(), &NSString::from_str(id)) else {
            eprintln!("relay: invalid profile data-store id; using the default store");
            return;
        };
        self.profile_store = Some(unsafe { WKWebsiteDataStore::dataStoreForIdentifier(&uuid, self.mtm) });
    }

    /// Choose where downloads go and whether each one asks first.
    pub fn set_downloads(&mut self, dir: std::path::PathBuf, ask: bool) {
        *self.downloads_dir.borrow_mut() = dir;
        self.downloads_ask.set(ask);
    }

    /// Replace the remembered camera/mic decisions at startup.
    pub fn set_media_permissions(&mut self, entries: Vec<(String, i64, bool)>) {
        let mut map = self.media_permissions.borrow_mut();
        map.clear();
        map.extend(entries.into_iter().map(|(host, kind, granted)| ((host, kind), granted)));
    }

    /// Drain a media key pressed while the app was active. 1 play, 2 pause,
    /// 3 toggle; the shell maps it to the active tab.
    pub fn take_media_key(&self) -> Option<&'static str> {
        match self.media_key.swap(0, Ordering::Relaxed) {
            1 => Some("play"),
            2 => Some("pause"),
            3 => Some("toggle"),
            _ => None,
        }
    }

    /// Ask LaunchServices to open http(s) links with Relay. macOS shows its
    /// own confirmation; the reply is reported through `EngineEvent`.
    pub fn set_default_browser(&self) {
        let workspace = NSWorkspace::sharedWorkspace();
        let app = NSBundle::mainBundle().bundleURL();
        for scheme in ["http", "https"] {
            let events = self.events.clone();
            let done = RcBlock::new(move |err: *mut NSError| {
                let error = unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string());
                events(EngineEvent::DefaultBrowserSet { ok: error.is_none(), error });
            });
            workspace.setDefaultApplicationAtURL_toOpenURLsWithScheme_completionHandler(
                &app,
                &NSString::from_str(scheme),
                Some(&done),
            );
        }
    }

    /// Propagate Dark/Light/System appearance to all live and future WKWebViews
    /// so websites' `prefers-color-scheme` matches Relay's theme.
    pub fn set_appearance(&mut self, dark: Option<bool>) {
        self.appearance = dark;
        for t in self.tabs.values() {
            apply_appearance(&t.view, dark);
        }
    }

    pub fn extension_controller(&self) -> Option<&Retained<WKWebExtensionController>> {
        self.extensions.as_ref()
    }

    /// Allow or forbid reading a password from a submitted login form.
    pub fn set_login_capture(&mut self, enabled: bool) {
        self.login_capture.set(enabled);
    }

    /// Attach a WKWebExtensionController to every tab created from now on.
    pub fn set_extension_controller(&mut self, c: Retained<WKWebExtensionController>) {
        self.extensions = Some(c);
    }

    /// Compile WebKit content-blocker JSON once (cached by WebKit across launches)
    /// and apply it to every tab.
    pub fn load_content_rules(&self, json: &str) {
        self.load_content_rules_then(json, || {});
    }

    /// [`Engine::load_content_rules`], calling `applied` once the compiled
    /// list is on every tab (not on failure, nor if blocking was turned off
    /// meanwhile). Reload a page from `applied`, never right after the call.
    pub fn load_content_rules_then(&self, json: &str, applied: impl Fn() + 'static) {
        self.blocking_enabled.set(true);
        let Some(store) = (unsafe { WKContentRuleListStore::defaultStore(self.mtm) }) else { return };
        let slot = self.rules.clone();
        let enabled = self.blocking_enabled.clone();
        let uccs = self.uccs.clone();
        let done = RcBlock::new(move |list: *mut WKContentRuleList, err: *mut NSError| {
            if !enabled.get() {
                return;
            }
            if let Some(list) = unsafe { Retained::retain(list) } {
                for ucc in uccs.borrow().iter() {
                    unsafe {
                        ucc.removeAllContentRuleLists();
                        ucc.addContentRuleList(&list);
                    }
                }
                *slot.borrow_mut() = Some(list);
                applied();
            } else if let Some(e) = unsafe { err.as_ref() } {
                eprintln!("relay: content rules failed to compile: {}", e.localizedDescription());
            }
        });
        unsafe {
            store.compileContentRuleListForIdentifier_encodedContentRuleList_completionHandler(
                Some(&NSString::from_str(RULES_ID)),
                Some(&NSString::from_str(json)),
                Some(&done),
            );
        }
    }

    /// Remove any active content rule lists from every live tab and stop
    /// applying them to future tabs.
    /// Access the tab's native view. Used by the macOS smoke example to read
    /// WebKit's own URL/title; the shell keeps its own placements.
    #[doc(hidden)]
    pub fn view(&self, tab: TabId) -> Option<&Retained<WKWebView>> {
        self.tabs.get(&tab).map(|t| &t.view)
    }

    pub fn clear_content_rules(&self) {
        self.blocking_enabled.set(false);
        *self.rules.borrow_mut() = None;
        for ucc in self.uccs.borrow().iter() {
            unsafe { ucc.removeAllContentRuleLists() };
        }
    }

    pub fn is_awake(&self, tab: TabId) -> bool {
        self.tabs.contains_key(&tab)
    }

    /// Create the WKWebView for a tab and start loading `url`.
    pub fn create(&mut self, tab: TabId, url: &str) -> &Retained<WKWebView> {
        self.create_with_privacy(tab, url, false)
    }

    /// Create the WKWebView for a tab; when `private` is true, attaches a
    /// non-persistent `WKWebsiteDataStore` so cookies and site storage are
    /// isolated and discarded when the tab closes.
    pub fn create_with_privacy(&mut self, tab: TabId, url: &str, private: bool) -> &Retained<WKWebView> {
        self.clear_error(tab);
        let mtm = self.mtm;
        let last_interaction = Rc::new(Cell::new(None));
        let login_nonce = Rc::new(RefCell::new(String::new()));
        self.login_nonces.insert(tab, login_nonce.clone());
        let handler = BridgeHandler::new(
            mtm,
            HandlerIvars {
                tab,
                hub: self.hub.clone(),
                events: self.events.clone(),
                downloads: RefCell::new(HashMap::new()),
                pending_captures: self.pending_captures.clone(),
                offered_logins: self.offered_logins.clone(),
                last_status: Rc::new(Cell::new(None)),
                login_nonce: login_nonce.clone(),
                private,
                media_permissions: self.media_permissions.clone(),
                downloads_dir: self.downloads_dir.clone(),
                downloads_ask: self.downloads_ask.clone(),
            },
        );
        let login_handler = LoginHandler::new(
            mtm,
            LoginHandlerIvars {
                tab,
                events: self.events.clone(),
                last_interaction: last_interaction.clone(),
                pending_captures: self.pending_captures.clone(),
                capture: self.login_capture.clone(),
                nonce: login_nonce.clone(),
            },
        );
        let ucc = unsafe { WKUserContentController::new(mtm) };
        let script = unsafe {
            WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &NSString::from_str(BRIDGE_JS),
                WKUserScriptInjectionTime::AtDocumentStart,
                true,
            )
        };
        let login_world = unsafe { WKContentWorld::worldWithName(&NSString::from_str(LOGIN_WORLD), mtm) };
        let login_script = unsafe {
            WKUserScript::initWithSource_injectionTime_forMainFrameOnly_inContentWorld(
                WKUserScript::alloc(mtm),
                &NSString::from_str(LOGIN_WATCH_JS),
                WKUserScriptInjectionTime::AtDocumentStart,
                true,
                &login_world,
            )
        };
        unsafe {
            ucc.addUserScript(&script);
            ucc.addScriptMessageHandler_name(ProtocolObject::from_ref(&*handler), &NSString::from_str(HANDLER));
            // Private tabs never offer to save or fill logins (#57).
            if !private {
                ucc.addUserScript(&login_script);
                ucc.addScriptMessageHandler_contentWorld_name(
                    ProtocolObject::from_ref(&*login_handler),
                    &login_world,
                    &NSString::from_str(LOGIN_HANDLER),
                );
            }
            if let Some(list) = self.rules.borrow().as_ref() {
                ucc.addContentRuleList(list);
            }
        }
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        unsafe {
            config.setApplicationNameForUserAgent(Some(&NSString::from_str(SAFARI_UA_SUFFIX)));
            config.setUserContentController(&ucc);
            if private {
                let ephemeral = WKWebsiteDataStore::nonPersistentDataStore(mtm);
                config.setWebsiteDataStore(&ephemeral);
            } else if let Some(store) = &self.profile_store {
                config.setWebsiteDataStore(store);
            }
            let prefs = config.preferences();
            let yes = NSNumber::new_bool(true);
            let _: () = msg_send![&*prefs, setValue: &*yes, forKey: &*NSString::from_str("developerExtrasEnabled")];
            if !private {
                if let Some(ext) = &self.extensions {
                    config.setWebExtensionController(Some(ext));
                }
            }
        }
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
        let view = RelayWebView::new(mtm, frame, &config, last_interaction);
        apply_appearance(&view, self.appearance);
        unsafe {
            view.setInspectable(true); // Develop menu / Web Inspector fallback
            view.setAllowsBackForwardNavigationGestures(true);
            view.setUIDelegate(Some(ProtocolObject::from_ref(&*handler)));
            view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*handler)));
        }
        load(&view, url);
        self.hub.update(|h| {
            if let Some(t) = h.tab_mut(tab) {
                t.sleeping = false;
            }
        });
        self.uccs.borrow_mut().push(ucc.clone());
        self.tabs.insert(tab, WebTab { view, handler, login_handler, ucc });
        &self.tabs[&tab].view
    }

    /// Render `tab` as a PDF and let the user choose where to save it.
    pub fn export_pdf(&self, tab: TabId) {
        let Some(t) = self.tabs.get(&tab) else { return };
        let mtm = MainThreadMarker::from(&*t.view);
        let title = self.hub.lock().tab(tab).map(|s| s.title.clone()).unwrap_or_default();
        let block = RcBlock::new(move |data: *mut NSData, err: *mut NSError| {
            if let Some(e) = unsafe { err.as_ref() } {
                eprintln!("relay: PDF export failed: {}", e.localizedDescription());
                return;
            }
            let Some(data) = (unsafe { Retained::retain(data) }) else { return };
            let panel = NSSavePanel::savePanel(mtm);
            let name = if title.trim().is_empty() { "page.pdf".to_string() } else { format!("{}.pdf", title.trim()) };
            panel.setNameFieldStringValue(&NSString::from_str(&name));
            if panel.runModal() == NSModalResponseOK {
                if let Some(url) = panel.URL() {
                    if !data.writeToURL_atomically(&url, true) {
                        eprintln!("relay: could not write PDF to {}", url.path().map(|p| p.to_string()).unwrap_or_default());
                    }
                }
            }
        });
        unsafe { t.view.createPDFWithConfiguration_completionHandler(None, &block) };
    }

    /// Open the native macOS Print dialog for `tab`.
    pub fn print(&self, tab: TabId) {
        if let Some(t) = self.tabs.get(&tab) {
            let info = NSPrintInfo::sharedPrintInfo();
            let op = unsafe { t.view.printOperationWithPrintInfo(&info) };
            let _ = op.runOperation();
        }
    }

    /// Fill a login form in the isolated `relay.login` content world using
    /// `NSDictionary` arguments (never interpolating the password into JS source).
    /// Live native URL of `tab` (never page-supplied).
    pub fn current_url(&self, tab: TabId) -> Option<String> {
        let t = self.tabs.get(&tab)?;
        unsafe { t.view.URL() }.and_then(|u| u.absoluteString()).map(|s| s.to_string())
    }

    pub fn fill_login(&self, tab: TabId, form_idx: i64, username: &str, password: &str) {
        let Some(t) = self.tabs.get(&tab) else { return };
        let Some(nonce) = self.login_nonces.get(&tab).map(|n| n.borrow().clone()).filter(|n| !n.is_empty()) else {
            (self.events)(EngineEvent::LoginFilled { tab, ok: false });
            return;
        };
        let world = unsafe { WKContentWorld::worldWithName(&NSString::from_str(LOGIN_WORLD), self.mtm) };
        let k_form = NSString::from_str("formIdx");
        let k_user = NSString::from_str("username");
        let k_pass = NSString::from_str("password");
        let k_nonce = NSString::from_str("nonce");
        let v_form = NSNumber::new_i64(form_idx);
        let v_user = NSString::from_str(username);
        let v_pass = NSString::from_str(password);
        let v_nonce = NSString::from_str(&nonce);
        let args: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
            &[&*k_form, &*k_user, &*k_pass, &*k_nonce],
            &[v_form.as_ref(), v_user.as_ref(), v_pass.as_ref(), v_nonce.as_ref()],
        );
        let events = self.events.clone();
        let done = RcBlock::new(move |res: *mut AnyObject, err: *mut NSError| {
            let ok = err.is_null()
                && unsafe { res.as_ref() }
                    .and_then(|o| o.downcast_ref::<NSNumber>())
                    .is_some_and(|n| n.as_bool());
            events(EngineEvent::LoginFilled { tab, ok });
        });
        unsafe {
            t.view.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                &NSString::from_str(LOGIN_FILL_JS),
                Some(&args),
                None,
                &world,
                Some(&done),
            );
        }
    }

    /// Pop the confirmed pending login for `tab` so `WebHost` can write it
    /// directly to the Keychain and drop it immediately. A named struct, not
    /// a tuple, so the secret cannot be confused with the public fields.
    pub fn take_pending_login(&self, tab: TabId) -> Option<PendingLogin> {
        self.offered_logins.borrow_mut().remove(&tab).map(|c| PendingLogin {
            url: c.url,
            username: c.username,
            password: c.password,
        })
    }

    /// Discard any pending or offered login capture for `tab`.
    pub fn drop_pending_login(&self, tab: TabId) {
        self.pending_captures.borrow_mut().remove(&tab);
        self.offered_logins.borrow_mut().remove(&tab);
    }

    pub fn navigate(&self, tab: TabId, url: &str) {
        if let Some(t) = self.tabs.get(&tab) {
            self.clear_error(tab);
            load(&t.view, url);
        }
    }

    pub fn go_back(&self, tab: TabId) {
        if let Some(t) = self.tabs.get(&tab) {
            unsafe { t.view.goBack() };
        }
    }

    pub fn go_forward(&self, tab: TabId) {
        if let Some(t) = self.tabs.get(&tab) {
            unsafe { t.view.goForward() };
        }
    }

    pub fn reload(&self, tab: TabId) {
        if let Some(t) = self.tabs.get(&tab) {
            self.clear_error(tab);
            unsafe { t.view.reload() };
        }
    }

    fn clear_error(&self, tab: TabId) {
        self.hub.update(|h| {
            if let Some(t) = h.tab_mut(tab) {
                t.load_error = None;
            }
        });
    }

    /// Override a tab's user agent for its next requests (`None` restores
    /// WebKit's default). No-op for sleeping tabs.
    pub fn set_user_agent(&self, tab: TabId, ua: Option<&str>) {
        if let Some(t) = self.tabs.get(&tab) {
            let ua = ua.map(NSString::from_str);
            unsafe { t.view.setCustomUserAgent(ua.as_deref()) };
        }
    }

    /// Set a tab's page zoom (1.0 = 100%). No-op for sleeping tabs.
    pub fn set_zoom(&self, tab: TabId, zoom: f64) {
        if let Some(t) = self.tabs.get(&tab) {
            unsafe { t.view.setPageZoom(zoom) };
        }
    }

    /// Find `text` in the page, selecting and scrolling to the next match
    /// (wrapping). `done` receives whether anything matched.
    pub fn find(&self, tab: TabId, text: &str, backwards: bool, case_sensitive: bool, done: impl Fn(bool) + 'static) {
        let Some(t) = self.tabs.get(&tab) else {
            done(false);
            return;
        };
        let mtm = MainThreadMarker::from(&*t.view);
        let config = unsafe { WKFindConfiguration::new(mtm) };
        unsafe {
            config.setBackwards(backwards);
            config.setWraps(true);
            config.setCaseSensitive(case_sensitive);
        }
        let block = RcBlock::new(move |result: NonNull<WKFindResult>| {
            done(unsafe { result.as_ref().matchFound() });
        });
        unsafe { t.view.findString_withConfiguration_completionHandler(&NSString::from_str(text), Some(&config), &block) };
    }

    pub fn eval(&self, tab: TabId, script: &str) {
        if let Some(t) = self.tabs.get(&tab) {
            unsafe { t.view.evaluateJavaScript_completionHandler(&NSString::from_str(script), None) };
        }
    }

    /// Place a tab's view inside `parent` at a rect given in top-left-origin
    /// points (GPUI's coordinate space); hides every other tab's view.
    pub fn show(&self, tabs: &[(TabId, NSRect)], parent: &NSView) {
        let flipped = parent.isFlipped();
        let height = parent.frame().size.height;
        for (id, t) in &self.tabs {
            match tabs.iter().find(|(tid, _)| tid == id) {
                Some((_, r)) => {
                    if self.warming.borrow().contains(id) {
                        if self.hub.lock().tab(*id).is_some_and(|t| !t.dom.is_empty()) {
                            self.warming.borrow_mut().remove(id);
                        } else {
                            t.view.setHidden(true);
                            continue;
                        }
                    }
                    let y = if flipped { r.origin.y } else { height - r.origin.y - r.size.height };
                    let frame = NSRect::new(NSPoint::new(r.origin.x, y), r.size);
                    // A degenerate GPUI layout (zero, NaN, or infinite) makes
                    // AppKit raise on setFrame: and aborts the process. Hide
                    // the view for this frame instead.
                    if !r.origin.x.is_finite()
                        || !r.origin.y.is_finite()
                        || !r.size.width.is_finite()
                        || !r.size.height.is_finite()
                        || r.size.width < 1.0
                        || r.size.height < 1.0
                        || !y.is_finite()
                    {
                        t.view.setHidden(true);
                        continue;
                    }
                    let attached = unsafe { t.view.superview() }.is_some_and(|s| &*s == parent);
                    if !attached {
                        parent.addSubview(&t.view);
                    }
                    t.view.setFrame(frame);
                    t.view.setHidden(false);
                }
                None => t.view.setHidden(true),
            }
        }
        // A failed page has no bridge and therefore no DOM: hide WebKit's
        // blank error view so the shell's error card is visible.
        for (id, t) in &self.tabs {
            let failed = self.hub.lock().tab(*id).is_some_and(|s| s.load_error.is_some() && s.dom.is_empty());
            if failed {
                t.view.setHidden(true);
            }
        }
    }

    /// Give the page keyboard focus (typing goes to WebKit).
    /// Returns true only once WebKit's view is attached and actually took
    /// first responder; a woken/slept tab has no window yet on the first call.
    pub fn focus_page(&self, tab: TabId) -> bool {
        let Some(t) = self.tabs.get(&tab) else { return false };
        let Some(w) = t.view.window() else { return false };
        w.makeFirstResponder(Some(&t.view))
    }

    /// Give `shell` (GPUI's view) keyboard focus, e.g. while the palette or
    /// link hints are open; otherwise WebKit swallows plain keystrokes.
    pub fn focus_shell(shell: &NSView) {
        if let Some(w) = shell.window() {
            w.makeFirstResponder(Some(shell));
        }
    }

    /// Send a standard editing selector to the window's first responder.
    /// Deliberately not `NSApplication.sendAction:`: GPUI's app delegate also
    /// implements `copy:`/`paste:` for its own menu items, so the app-level
    /// chain could route the command back into Relay instead of the page.
    /// The first responder in a focused page is WebKit's content view, which
    /// owns the real editing implementation.
    pub fn edit(&self, command: EditCommand) {
        let action = match command {
            EditCommand::Undo => sel!(undo:),
            EditCommand::Redo => sel!(redo:),
            EditCommand::Cut => sel!(cut:),
            EditCommand::Copy => sel!(copy:),
            EditCommand::Paste => sel!(paste:),
            EditCommand::SelectAll => sel!(selectAll:),
        };
        let app = NSApplication::sharedApplication(self.mtm);
        let Some(window) = app.keyWindow() else { return };
        let Some(responder) = window.firstResponder() else { return };
        unsafe {
            responder.tryToPerform_with(action, None);
        }
    }

    pub fn minimize(&self) {
        let app = NSApplication::sharedApplication(self.mtm);
        if let Some(window) = app.keyWindow() {
            window.performMiniaturize(None);
        }
    }

    /// An installed web app runs the shared Relay binary so it keeps the
    /// browser's cookies and WebKit store; only its Dock icon differs, so
    /// set that from the bundle's generated `.icns` at launch.
    pub fn set_application_icon(&self, path: &std::path::Path) -> bool {
        let path = NSString::from_str(&path.to_string_lossy());
        let Some(image) = NSImage::initWithContentsOfFile(NSImage::alloc(), &path) else {
            return false;
        };
        let app = NSApplication::sharedApplication(self.mtm);
        unsafe { app.setApplicationIconImage(Some(&image)) };
        true
    }

    pub fn close(&mut self, tab: TabId) {
        self.login_nonces.remove(&tab);
        if let Some(t) = self.tabs.remove(&tab) {
            self.uccs.borrow_mut().retain(|u| !std::ptr::eq::<WKUserContentController>(&**u, &*t.ucc));
            teardown(&t);
        }
        self.slept.borrow_mut().remove(&tab);
        self.warming.borrow_mut().remove(&tab);
        self.drop_pending_login(tab);
    }

    /// Snapshot, remember URL + scroll, then destroy the WKWebView (0 MB).
    pub fn sleep(&mut self, tab: TabId, scroll: (f64, f64)) {
        self.drop_pending_login(tab);
        self.login_nonces.remove(&tab);
        let Some(t) = self.tabs.remove(&tab) else { return };
        self.uccs.borrow_mut().retain(|u| !std::ptr::eq::<WKUserContentController>(&**u, &*t.ucc));
        let url = unsafe { t.view.URL() }.and_then(|u| u.absoluteString()).map(|s| s.to_string()).unwrap_or_default();
        let slept = self.slept.clone();
        let events = self.events.clone();
        self.hub.update(|h| {
            if let Some(s) = h.tab_mut(tab) {
                s.sleeping = true;
                s.dom = Default::default();
            }
        });
        // Record the tab now so a wake before the snapshot lands still works;
        // the callback only attaches the image (if the tab is still asleep).
        slept.borrow_mut().insert(tab, SleptTab { url: url.clone(), scroll_x: scroll.0, scroll_y: scroll.1, snapshot_jpeg: None });
        let view = t.view.clone();
        let tab_holder = Rc::new(RefCell::new(Some(t)));
        let holder_for_block = tab_holder.clone();
        let keep = RcBlock::new(move |img: *mut NSImage, err: *mut NSError| {
            if let Some(e) = unsafe { err.as_ref() } {
                eprintln!("relay: tab {tab} snapshot failed: {}", e.localizedDescription());
            }
            let jpeg = unsafe { img.as_ref() }.and_then(jpeg_from_image);
            if let Some(entry) = slept.borrow_mut().get_mut(&tab) {
                entry.snapshot_jpeg = jpeg;
                events(EngineEvent::Slept(tab));
            }
            if let Some(tab) = holder_for_block.borrow_mut().take() {
                teardown(&tab);
            }
        });
        unsafe { view.takeSnapshotWithConfiguration_completionHandler(None, &keep) };
    }

    /// Push WKDownload progress into the hub. Called by the shell pump at a
    /// low frequency; each retained download reports an NSProgress.
    pub fn poll_download_progress(&self) {
        let active: HashSet<u64> = self
            .hub
            .lock()
            .downloads()
            .into_iter()
            .filter(|d| d.state == relay_bridge::DownloadState::Downloading)
            .map(|d| d.id)
            .collect();
        if active.is_empty() {
            return;
        }
        for t in self.tabs.values() {
            for (id, received, total) in t.handler.download_progress() {
                if active.contains(&id) {
                    self.hub.update(|h| h.set_download_progress(id, received, total));
                }
            }
        }
    }

    /// Cancel one of the tab's downloads; the bridge handler owns the native
    /// WKDownload object.
    pub fn cancel_download(&self, tab: TabId, id: u64) {
        if let Some(t) = self.tabs.get(&tab) {
            t.handler.cancel_download(id);
        }
    }

    /// Capture the visible tab as JPEG for the MCP `screenshot` tool.
    pub fn screenshot(&self, tab: TabId, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let Some(t) = self.tabs.get(&tab) else {
            done(Err(format!("tab {tab} is not awake")));
            return;
        };
        let done = std::rc::Rc::new(std::cell::RefCell::new(Some(done)));
        let done_for_block = done.clone();
        let keep = RcBlock::new(move |img: *mut NSImage, err: *mut NSError| {
            let Some(done) = done_for_block.borrow_mut().take() else { return };
            if let Some(e) = unsafe { err.as_ref() } {
                done(Err(e.localizedDescription().to_string()));
                return;
            }
            match unsafe { img.as_ref() }.and_then(jpeg_from_image) {
                Some(jpeg) => done(Ok(jpeg)),
                None => done(Err("could not encode the screenshot".into())),
            }
        });
        unsafe { t.view.takeSnapshotWithConfiguration_completionHandler(None, &keep) };
    }

    /// Register a tab restored from the saved session as already asleep: no
    /// WKWebView (0 MB) until [`Engine::wake`] loads `url` and restores scroll.
    pub fn restore_asleep(&mut self, tab: TabId, url: &str, scroll_y: f64) {
        if self.tabs.contains_key(&tab) {
            return;
        }
        self.slept.borrow_mut().insert(tab, SleptTab { url: url.to_string(), scroll_x: 0.0, scroll_y, snapshot_jpeg: None });
        self.hub.update(|h| {
            if let Some(s) = h.tab_mut(tab) {
                s.sleeping = true;
            }
        });
    }

    pub fn slept(&self, tab: TabId) -> Option<SleptTab> {
        self.slept.borrow().get(&tab).cloned()
    }

    /// Recreate a slept tab; the shell shows the snapshot until the bridge
    /// says hello, then calls [`Engine::after_load`] to restore scroll.
    pub fn wake(&mut self, tab: TabId) -> Option<&Retained<WKWebView>> {
        let s = self.slept.borrow_mut().remove(&tab)?;
        self.pending_restore.insert(tab, restore_scroll_script(&s));
        self.warming.borrow_mut().insert(tab);
        let private = self.hub.lock().tab(tab).is_some_and(|t| t.private);
        Some(self.create_with_privacy(tab, &s.url, private))
    }

    /// Call when a tab's `load_seq` advances: finishes a wake-up restore.
    pub fn after_load(&mut self, tab: TabId) {
        if let Some(script) = self.pending_restore.remove(&tab) {
            self.eval(tab, &script);
        }
    }

    /// Fetch cookies for the tab's site from WKHTTPCookieStore into the hub.
    pub fn refresh_cookies(&self, tab: TabId) {
        let Some(t) = self.tabs.get(&tab) else { return };
        let host = unsafe { t.view.URL() }.and_then(|u| u.host()).map(|h| h.to_string()).unwrap_or_default();
        let store = unsafe { t.view.configuration().websiteDataStore().httpCookieStore() };
        let hub = self.hub.clone();
        let done = RcBlock::new(move |cookies: NonNull<NSArray<NSHTTPCookie>>| {
            let cookies = unsafe { cookies.as_ref() };
            let list: Vec<Cookie> = cookies
                .iter()
                .filter(|c| domain_matches(&c.domain().to_string(), &host))
                .map(|c| Cookie {
                    name: c.name().to_string(),
                    value: c.value().to_string(),
                    domain: c.domain().to_string(),
                    path: c.path().to_string(),
                    secure: c.isSecure(),
                    http_only: c.isHTTPOnly(),
                    expires: c.expiresDate().map(|d| d.timeIntervalSince1970()),
                })
                .collect();
            hub.update(|h| h.set_cookies(tab, list));
        });
        unsafe { store.getAllCookies(&done) };
    }

    /// Open WebKit's full Web Inspector for the 5% the native one skips.
    /// Uses the private `_inspector` SPI when present; otherwise the user can
    /// use Develop > Show Web Inspector (the view is inspectable).
    pub fn open_web_inspector(&self, tab: TabId) {
        let Some(t) = self.tabs.get(&tab) else { return };
        let view: &AnyObject = t.view.as_ref();
        let responds: bool = unsafe { msg_send![view, respondsToSelector: sel!(_inspector)] };
        if responds {
            let inspector: *mut AnyObject = unsafe { msg_send![view, _inspector] };
            if let Some(i) = unsafe { inspector.as_ref() } {
                let _: () = unsafe { msg_send![i, show] };
            }
        }
    }
}

fn load(view: &WKWebView, url: &str) {
    if let Some(u) = NSURL::URLWithString(&NSString::from_str(url)) {
        let req = NSURLRequest::requestWithURL(&u);
        unsafe { view.loadRequest(&req) };
    }
}

fn apply_appearance(view: &WKWebView, dark: Option<bool>) {
    let appearance = match dark {
        Some(true) => unsafe { NSAppearance::appearanceNamed(NSAppearanceNameDarkAqua) },
        Some(false) => unsafe { NSAppearance::appearanceNamed(NSAppearanceNameAqua) },
        None => None,
    };
    view.setAppearance(appearance.as_deref());
}

fn teardown(t: &WebTab) {
    let mtm = MainThreadMarker::from(&*t.view);
    let login_world = unsafe { WKContentWorld::worldWithName(&NSString::from_str(LOGIN_WORLD), mtm) };
    unsafe {
        t.ucc.removeScriptMessageHandlerForName(&NSString::from_str(HANDLER));
        t.ucc.removeScriptMessageHandlerForName_contentWorld(&NSString::from_str(LOGIN_HANDLER), &login_world);
        t.view.setUIDelegate(None);
        t.view.setNavigationDelegate(None);
        t.view.stopLoading();
    }
    t.view.removeFromSuperview();
    let _ = (&t.handler, &t.login_handler);
}

fn same_origin_url(a: &str, b: &str) -> bool {
    let origin_key = |u: &str| -> Option<(String, String)> {
        let (scheme, rest) = u.split_once("://")?;
        let auth = rest.split(['/', '?', '#']).next()?;
        Some((scheme.to_ascii_lowercase(), auth.to_ascii_lowercase()))
    };
    match (origin_key(a), origin_key(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn jpeg_from_image(img: &NSImage) -> Option<Vec<u8>> {
    let tiff = img.TIFFRepresentation()?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
    let props = NSDictionary::new();
    let data = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::JPEG, &props) }?;
    Some(data.to_vec())
}

fn domain_matches(cookie_domain: &str, host: &str) -> bool {
    let d = cookie_domain.trim_start_matches('.');
    host == d || host.ends_with(&format!(".{d}"))
}

#[cfg(test)]
mod login_nonce_tests {
    use super::new_login_nonce;

    #[test]
    fn nonces_are_fresh_opaque_and_fixed_width() {
        let a = new_login_nonce();
        let b = new_login_nonce();
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
