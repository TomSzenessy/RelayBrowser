//! macOS WKWebView driver: one WKWebView per awake tab, the bridge injected at
//! document start, bridge messages routed into the [`SharedHub`], content
//! rules applied, and sleep/wake via snapshots. Main thread only.
//!
//! VERIFY-ON-MAC: type-checked for aarch64-apple-darwin on Linux; behaviour is
//! verified only by running the app on macOS (see docs/verification.md).

use crate::sleep::{restore_scroll_script, SleptTab};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSImage, NSView};
use objc2_foundation::{NSArray, NSDictionary, NSError, NSHTTPCookie, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURLRequest, NSURL};
use objc2_web_kit::{
    WKContentRuleList, WKContentRuleListStore, WKNavigation, WKNavigationAction, WKNavigationDelegate, WKScriptMessage,
    WKScriptMessageHandler, WKUIDelegate, WKUserContentController, WKUserScript, WKUserScriptInjectionTime,
    WKWebExtensionController, WKWebView, WKWebViewConfiguration, WKWindowFeatures,
};
use relay_bridge::{Cookie, SharedHub, TabId, BRIDGE_JS};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ptr::NonNull;
use std::rc::Rc;

const HANDLER: &str = "relay";
const RULES_ID: &str = "relay-content-rules";

/// Things the engine needs the shell to do.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// `target=_blank` / `window.open`: open this URL in a new tab after `opener`.
    OpenInNewTab { opener: TabId, url: String },
    /// A sleeping tab's snapshot is ready (or failed).
    Slept(TabId),
}

pub struct HandlerIvars {
    tab: TabId,
    hub: SharedHub,
    events: Rc<dyn Fn(EngineEvent)>,
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
        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(&self, _web_view: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.note_error(error);
        }

        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn did_fail(&self, _web_view: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            self.note_error(error);
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
    }
);

impl BridgeHandler {
    fn new(mtm: MainThreadMarker, ivars: HandlerIvars) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ivars);
        unsafe { msg_send![super(this), init] }
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

pub struct WebTab {
    pub view: Retained<WKWebView>,
    handler: Retained<BridgeHandler>,
    ucc: Retained<WKUserContentController>,
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
    tabs: HashMap<TabId, WebTab>,
    slept: Rc<RefCell<HashMap<TabId, SleptTab>>>,
    /// Tabs woken from sleep: keep the native view hidden until the bridge
    /// delivers a DOM, so the snapshot the shell shows is not covered by a
    /// white WKWebView.
    warming: RefCell<HashSet<TabId>>,
    pending_restore: HashMap<TabId, String>,
}

impl Engine {
    pub fn new(mtm: MainThreadMarker, hub: SharedHub, events: impl Fn(EngineEvent) + 'static) -> Self {
        Self {
            mtm,
            hub,
            events: Rc::new(events),
            rules: Rc::new(RefCell::new(None)),
            blocking_enabled: Rc::new(std::cell::Cell::new(false)),
            uccs: Rc::default(),
            extensions: None,
            tabs: HashMap::new(),
            slept: Rc::new(RefCell::new(HashMap::new())),
            warming: RefCell::new(HashSet::new()),
            pending_restore: HashMap::new(),
        }
    }

    pub fn extension_controller(&self) -> Option<&Retained<WKWebExtensionController>> {
        self.extensions.as_ref()
    }

    /// Attach a WKWebExtensionController to every tab created from now on.
    pub fn set_extension_controller(&mut self, c: Retained<WKWebExtensionController>) {
        self.extensions = Some(c);
    }

    /// Compile WebKit content-blocker JSON once (cached by WebKit across launches)
    /// and apply it to every tab.
    pub fn load_content_rules(&self, json: &str) {
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
    pub fn clear_content_rules(&self) {
        self.blocking_enabled.set(false);
        *self.rules.borrow_mut() = None;
        for ucc in self.uccs.borrow().iter() {
            unsafe { ucc.removeAllContentRuleLists() };
        }
    }

    pub fn view(&self, tab: TabId) -> Option<&Retained<WKWebView>> {
        self.tabs.get(&tab).map(|t| &t.view)
    }

    pub fn is_awake(&self, tab: TabId) -> bool {
        self.tabs.contains_key(&tab)
    }

    /// Create the WKWebView for a tab and start loading `url`.
    pub fn create(&mut self, tab: TabId, url: &str) -> &Retained<WKWebView> {
        self.clear_error(tab);
        let mtm = self.mtm;
        let handler = BridgeHandler::new(mtm, HandlerIvars { tab, hub: self.hub.clone(), events: self.events.clone() });
        let ucc = unsafe { WKUserContentController::new(mtm) };
        let script = unsafe {
            WKUserScript::initWithSource_injectionTime_forMainFrameOnly(
                WKUserScript::alloc(mtm),
                &NSString::from_str(BRIDGE_JS),
                WKUserScriptInjectionTime::AtDocumentStart,
                true,
            )
        };
        unsafe {
            ucc.addUserScript(&script);
            ucc.addScriptMessageHandler_name(ProtocolObject::from_ref(&*handler), &NSString::from_str(HANDLER));
            if let Some(list) = self.rules.borrow().as_ref() {
                ucc.addContentRuleList(list);
            }
        }
        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        unsafe {
            config.setUserContentController(&ucc);
            if let Some(ext) = &self.extensions {
                config.setWebExtensionController(Some(ext));
            }
        }
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(800.0, 600.0));
        let view = unsafe { WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), frame, &config) };
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
        self.tabs.insert(tab, WebTab { view, handler, ucc });
        &self.tabs[&tab].view
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
    pub fn focus_page(&self, tab: TabId) {
        if let Some(t) = self.tabs.get(&tab) {
            if let Some(w) = t.view.window() {
                w.makeFirstResponder(Some(&t.view));
            }
        }
    }

    /// Give `shell` (GPUI's view) keyboard focus, e.g. while the palette or
    /// link hints are open; otherwise WebKit swallows plain keystrokes.
    pub fn focus_shell(shell: &NSView) {
        if let Some(w) = shell.window() {
            w.makeFirstResponder(Some(shell));
        }
    }

    pub fn close(&mut self, tab: TabId) {
        if let Some(t) = self.tabs.remove(&tab) {
            self.uccs.borrow_mut().retain(|u| !std::ptr::eq::<WKUserContentController>(&**u, &*t.ucc));
            teardown(&t);
        }
        self.slept.borrow_mut().remove(&tab);
        self.warming.borrow_mut().remove(&tab);
    }

    /// Snapshot, remember URL + scroll, then destroy the WKWebView (0 MB).
    pub fn sleep(&mut self, tab: TabId, scroll: (f64, f64)) {
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
        let view = tab_holder.borrow().as_ref().unwrap().view.clone();
        unsafe { view.takeSnapshotWithConfiguration_completionHandler(None, &keep) };
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
        Some(self.create(tab, &s.url))
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

fn teardown(t: &WebTab) {
    unsafe {
        t.ucc.removeScriptMessageHandlerForName(&NSString::from_str(HANDLER));
        t.view.setUIDelegate(None);
        t.view.stopLoading();
    }
    t.view.removeFromSuperview();
    let _ = &t.handler;
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
