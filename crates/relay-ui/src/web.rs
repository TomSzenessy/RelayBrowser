//! The web-content host behind the shell. On macOS it drives the WebKit
//! `Engine` inside GPUI's NSView; elsewhere it is a no-op stand-in so the
//! shell (and every portable test) builds and runs on Linux, with a
//! placeholder where pages would render.

use gpui::{Bounds, Pixels, Window};
use relay_bridge::{BrowserRequest, SharedHub, TabId};

/// The standard editing commands the Edit menu forwards. With a page
/// focused they go to WKWebView's own editing implementation; with an
/// overlay focused the shell handles paste itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditAction {
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
}

/// Things WebKit tells the shell (only the macOS engine produces them).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub enum WebEvent {
    OpenInNewTab { opener: TabId, url: String },
    Slept(TabId),
    /// Recompiled content rules are live: reload this tab now.
    RulesApplied { reload: TabId },
    /// Reply to [`WebHost::find`]: whether the text occurs in the page.
    Found { tab: TabId, found: bool },
    /// Native-trusted focus in a login form (never carries secrets).
    LoginFocus { tab: TabId, url: String, form_idx: i64 },
    /// The page answered a fill request: true when a password field was set.
    LoginFilled { tab: TabId, ok: bool },
    /// Cookies and site data were removed from the default store.
    SiteDataCleared,
    /// LaunchServices answered a default-browser request.
    DefaultBrowserSet { ok: bool, error: Option<String> },
    /// The user set a camera/mic decision for a host.
    MediaPermission { host: String, kind: i64, granted: bool },
    /// Native-verified post-submit login save offer (never carries secrets).
    LoginSaveOffer { tab: TabId, url: String, username: String },
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use objc2::rc::Retained;
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSView;
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use relay_webkit::mac::{EditCommand, Engine, EngineEvent};
    use std::cell::RefCell;
    use std::rc::Rc;

    pub struct WebHost {
        engine: Engine,
        shell: Option<Retained<NSView>>,
        events: Rc<RefCell<Vec<WebEvent>>>,
        mtm: MainThreadMarker,
    }

    impl WebHost {
        pub fn new(hub: SharedHub, window: &Window) -> Self {
            let mtm = MainThreadMarker::new().expect("the shell runs on the main thread");
            let events: Rc<RefCell<Vec<WebEvent>>> = Rc::default();
            let sink = events.clone();
            let mut engine = Engine::new(mtm, hub, move |e| {
                sink.borrow_mut().push(match e {
                    EngineEvent::OpenInNewTab { opener, url } => WebEvent::OpenInNewTab { opener, url },
                    EngineEvent::Slept(t) => WebEvent::Slept(t),
                    EngineEvent::LoginFocus { tab, url, form_idx } => WebEvent::LoginFocus { tab, url, form_idx },
                    EngineEvent::LoginFilled { tab, ok } => WebEvent::LoginFilled { tab, ok },
                    EngineEvent::SiteDataCleared => WebEvent::SiteDataCleared,
                    EngineEvent::DefaultBrowserSet { ok, error } => WebEvent::DefaultBrowserSet { ok, error },
                    EngineEvent::MediaPermission { host, kind, granted } => WebEvent::MediaPermission { host, kind, granted },
                    EngineEvent::LoginSaveOffer { tab, url, username } => WebEvent::LoginSaveOffer { tab, url, username },
                })
            });
            if let Ok(id) = std::env::var("RELAY_PROFILE_ID") {
                engine.set_profile_id(&id);
            }
            // WKWebExtension needs macOS 15.4+; older systems browse without extensions.
            if objc2::available!(macos = 15.4) {
                engine.set_extension_controller(unsafe { objc2_web_kit::WKWebExtensionController::new(mtm) });
            }
            let shell = HasWindowHandle::window_handle(window).ok().and_then(|h| match h.as_raw() {
                // SAFETY: AppKit handles carry a valid NSView for the window's lifetime.
                RawWindowHandle::AppKit(a) => unsafe { Retained::retain(a.ns_view.as_ptr().cast::<NSView>()) },
                _ => None,
            });
            Self { engine, shell, events, mtm }
        }

        pub fn renders_pages(&self) -> bool {
            true
        }

        pub fn load_content_rules(&self, json: &str) {
            self.engine.load_content_rules(json);
        }

        pub fn clear_content_rules(&self) {
            self.engine.clear_content_rules();
        }

        /// Recompile the rules, then reload `tab` once they apply
        /// ([`WebEvent::RulesApplied`]).
        pub fn load_content_rules_and_reload(&self, json: &str, tab: TabId) {
            let events = self.events.clone();
            self.engine.load_content_rules_then(json, move || events.borrow_mut().push(WebEvent::RulesApplied { reload: tab }));
        }

        pub fn poll_downloads(&self) {
            self.engine.poll_download_progress();
        }

        pub fn apply(&mut self, req: BrowserRequest) {
            match req {
                BrowserRequest::Open { tab, url } => {
                    self.engine.create(tab, &url);
                }
                BrowserRequest::OpenPrivate { tab, url } => {
                    self.engine.create_with_privacy(tab, &url, true);
                }
                BrowserRequest::Navigate { tab, url } => {
                    self.wake(tab);
                    self.engine.navigate(tab, &url)
                }
                BrowserRequest::GoBack(tab) => self.engine.go_back(tab),
                BrowserRequest::GoForward(tab) => self.engine.go_forward(tab),
                BrowserRequest::Reload(tab) => self.engine.reload(tab),
                BrowserRequest::Close(tab) => self.engine.close(tab),
                BrowserRequest::Activate(tab) => self.wake(tab),
                BrowserRequest::Eval { tab, script } => self.engine.eval(tab, &script),
                BrowserRequest::RefreshCookies(tab) => self.engine.refresh_cookies(tab),
                BrowserRequest::OpenWebInspector(tab) => self.engine.open_web_inspector(tab),
                BrowserRequest::Print(tab) => self.engine.print(tab),
                BrowserRequest::Screenshot { tab, done } => self.engine.screenshot(tab, done),
                BrowserRequest::CancelDownload { tab, id } => self.engine.cancel_download(tab, id),
            }
        }

        pub fn wake(&mut self, tab: TabId) {
            if !self.engine.is_awake(tab) {
                self.engine.wake(tab);
            }
        }

        pub fn after_load(&mut self, tab: TabId) {
            self.engine.after_load(tab);
        }

        pub fn sleep(&mut self, tab: TabId, scroll: (f64, f64)) {
            self.engine.sleep(tab, scroll);
        }

        pub fn snapshot(&self, tab: TabId) -> Option<Vec<u8>> {
            self.engine.slept(tab).and_then(|s| s.snapshot_jpeg)
        }

        pub fn set_zoom(&self, tab: TabId, zoom: f64) {
            self.engine.set_zoom(tab, zoom);
        }

        pub fn set_user_agent(&self, tab: TabId, ua: Option<&str>) {
            self.engine.set_user_agent(tab, ua);
        }

        pub fn set_appearance(&mut self, dark: Option<bool>) {
            self.engine.set_appearance(dark);
        }

        /// Select the next (or previous) match; the answer arrives as
        /// [`WebEvent::Found`].
        pub fn find(&self, tab: TabId, text: &str, backwards: bool, case_sensitive: bool) {
            let events = self.events.clone();
            self.engine.find(tab, text, backwards, case_sensitive, move |found| events.borrow_mut().push(WebEvent::Found { tab, found }));
        }

        /// A restored session tab: asleep until activated.
        pub fn restore_asleep(&mut self, tab: TabId, url: &str, scroll_y: f64) {
            self.engine.restore_asleep(tab, url, scroll_y);
        }

        /// The remembered scroll offset of a sleeping tab.
        pub fn slept_scroll_y(&self, tab: TabId) -> Option<f64> {
            self.engine.slept(tab).map(|s| s.scroll_y)
        }

        /// Position web views over the given GPUI bounds; hide the rest.
        pub fn layout(&self, placements: &[(TabId, Bounds<Pixels>)]) {
            let Some(shell) = &self.shell else { return };
            let rects: Vec<(TabId, NSRect)> = placements
                .iter()
                .map(|(t, b)| {
                    let (x, y, w, h) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
                    (*t, NSRect::new(NSPoint::new(x as f64, y as f64), NSSize::new(w as f64, h as f64)))
                })
                .collect();
            self.engine.show(&rects, shell);
        }

        pub fn focus_page(&self, tab: TabId) -> bool {
            self.engine.focus_page(tab)
        }

        pub fn focus_shell(&self) {
            if let Some(s) = &self.shell {
                Engine::focus_shell(s);
            }
        }

        /// Route a standard Edit command to the focused WKWebView.
        pub fn edit(&self, action: EditAction) {
            let command = match action {
                EditAction::Undo => EditCommand::Undo,
                EditAction::Redo => EditCommand::Redo,
                EditAction::Cut => EditCommand::Cut,
                EditAction::Copy => EditCommand::Copy,
                EditAction::Paste => EditCommand::Paste,
                EditAction::SelectAll => EditCommand::SelectAll,
            };
            self.engine.edit(command);
        }

        pub fn minimize(&self) {
            self.engine.minimize();
        }

        /// Give an installed web app its own Dock icon while sharing the
        /// installed Relay binary (and therefore its cookies and session).
        pub fn set_application_icon(&self, path: &std::path::Path) {
            self.engine.set_application_icon(path);
        }

        pub fn take_events(&self) -> Vec<WebEvent> {
            std::mem::take(&mut *self.events.borrow_mut())
        }

        pub fn load_extension(&self, dir: &std::path::Path, id: &str, done: impl Fn(Result<String, String>) + 'static) {
            if let Some(c) = self.engine.extension_controller() {
                relay_extensions::mac::load_extension(self.mtm, c, dir, id, done);
            } else {
                done(Err("extensions require macOS 15.4+".into()));
            }
        }

        /// List non-secret `(username, path)` Keychain candidates for `url`.
        pub fn list_login_candidates(&self, url: &str) -> Vec<(String, String)> {
            let Ok((origin, path)) = relay_extensions::login::Origin::parse_with_path(url) else {
                return vec![];
            };
            let metas = relay_extensions::mac::list_logins(&origin);
            let matched = relay_extensions::login::match_candidates(
                &origin,
                &path,
                &metas,
                relay_extensions::login::MAX_CANDIDATES,
            );
            matched
                .into_iter()
                .filter_map(|c| {
                    let meta = metas.iter().find(|m| m.id == c.id)?;
                    Some((c.username, meta.path.clone()))
                })
                .collect()
        }

        /// Explicitly fill one confirmed Keychain login directly into the isolated
        /// `relay.login` content world. Password never leaves `WebHost`.
        pub fn fill_login(&self, tab: TabId, url: &str, form_idx: i64, username: &str, path: &str) -> Result<(), String> {
            let origin = relay_extensions::login::Origin::parse(url).map_err(|e| e.to_string())?;
            // D3: bind to the live native origin at fill time, not the focus-time URL.
            let live = self.engine.current_url(tab).and_then(|u| relay_extensions::login::Origin::parse(&u).ok());
            if live.as_ref() != Some(&origin) {
                return Err("The page changed; fill cancelled".into());
            }
            let pw = relay_extensions::mac::read_login_password(&origin, username, path, keychain_profile().as_deref()).map_err(keychain_message)?;
            self.engine.fill_login(tab, form_idx, username, pw.expose());
            Ok(())
        }

        /// Save the confirmed native login capture for `tab` to Keychain and drop it.
        pub fn accept_save_login(&self, tab: TabId) -> Result<String, String> {
            let Some(pending) = self.engine.take_pending_login(tab) else {
                return Err("No pending login to save".into());
            };
            let (origin, path) = relay_extensions::login::Origin::parse_with_path(&pending.url).map_err(|e| e.to_string())?;
            let captured = relay_extensions::login::CapturedLogin::new(
                origin,
                pending.username.clone(),
                &path,
                relay_extensions::login::Password::new(pending.password.as_str()),
            );
            relay_extensions::mac::save_login(&captured, keychain_profile().as_deref()).map_err(keychain_message)?;
            Ok(pending.username)
        }

        pub fn decline_save_login(&self, tab: TabId) {
            self.engine.drop_pending_login(tab);
        }

        /// When saving is off, the engine must not read a submitted password
        /// at all; fill-only builds still receive focus offers.
        pub fn set_login_capture(&mut self, enabled: bool) {
            self.engine.set_login_capture(enabled);
        }

        /// A media key pressed while the app was active, if any.
        pub fn take_media_key(&self) -> Option<&'static str> {
            self.engine.take_media_key()
        }

        /// Remove cookies, cache, and local storage for every site.
        pub fn clear_site_data(&self) {
            self.engine.clear_site_data();
        }

        /// Ask macOS to make Relay the handler for http(s) links.
        pub fn set_default_browser(&self) {
            self.engine.set_default_browser();
        }

        /// A URL delivered by Handoff, if any.
        pub fn take_handoff(&self) -> Option<String> {
            relay_webkit::mac::take_received_handoff()
        }

        /// Seed remembered camera/mic decisions at startup.
        pub fn set_media_permissions(&mut self, entries: Vec<(String, i64, bool)>) {
            self.engine.set_media_permissions(entries);
        }

        /// Advertise the active page for Handoff, or clear it with `None`.
        pub fn set_handoff(&mut self, url: Option<&str>, title: &str) {
            self.engine.set_handoff(url, title);
        }

        /// Choose the download directory and whether each file asks first.
        pub fn set_downloads(&mut self, dir: std::path::PathBuf, ask: bool) {
            self.engine.set_downloads(dir, ask);
        }

        /// PNG bytes for an extension's action icon, if available.
        pub fn extension_icon(&self, id: &str) -> Option<Vec<u8>> {
            self.engine.extension_controller().and_then(|c| relay_extensions::mac::extension_icon(c, id))
        }

        /// Show an extension's action popup, if it declares one.
        pub fn open_extension_popup(&mut self, id: &str) -> bool {
            let Some(web) = self.engine.extension_controller().and_then(|c| relay_extensions::mac::extension_popup(c, id)) else {
                return false;
            };
            self.engine.show_extension_popup(web);
            true
        }

        /// The options-page URL for an installed extension, if it has one.
        pub fn extension_options_url(&self, id: &str) -> Option<String> {
            self.engine.extension_controller().and_then(|c| relay_extensions::mac::extension_options_url(c, id))
        }

        /// Toggle the native always-on-top video panel for a tab.
        pub fn toggle_float_panel(&mut self, tab: TabId) {
            self.engine.toggle_float_panel(tab);
        }

        /// Finish opening a panel once the page reported its video URL.
        pub fn poll_float_panel(&mut self) {
            self.engine.poll_float_panel();
        }

        /// Forget every remembered camera/mic decision in this process.
        pub fn clear_media_permissions(&mut self) {
            self.engine.set_media_permissions(Vec::new());
        }

        /// Render this tab as a PDF and ask where to save it.
        pub fn export_pdf(&self, tab: TabId) {
            self.engine.export_pdf(tab);
        }

        pub fn delete_saved_login(&self, url: &str, username: &str, path: &str) -> Result<bool, String> {
            let origin = relay_extensions::login::Origin::parse(url).map_err(|e| e.to_string())?;
            relay_extensions::mac::delete_login(&origin, username, path, keychain_profile().as_deref()).map_err(keychain_message)
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    /// No WebKit off-Mac: requests are accepted and dropped.
    pub struct WebHost;

    impl WebHost {
        pub fn new(_hub: SharedHub, _window: &Window) -> Self {
            WebHost
        }
        pub fn renders_pages(&self) -> bool {
            false
        }
        pub fn load_content_rules(&self, _json: &str) {}
        pub fn poll_downloads(&self) {}
        pub fn clear_content_rules(&self) {}
        pub fn load_content_rules_and_reload(&self, _json: &str, _tab: TabId) {}
        pub fn apply(&mut self, req: BrowserRequest) {
            if let BrowserRequest::Screenshot { done, .. } = req {
                done(Err("screenshots need macOS WebKit".into()));
            }
        }
        pub fn wake(&mut self, _tab: TabId) {}
        pub fn after_load(&mut self, _tab: TabId) {}
        pub fn sleep(&mut self, _tab: TabId, _scroll: (f64, f64)) {}
        pub fn snapshot(&self, _tab: TabId) -> Option<Vec<u8>> {
            None
        }
        pub fn restore_asleep(&mut self, _tab: TabId, _url: &str, _scroll_y: f64) {}
        pub fn set_zoom(&self, _tab: TabId, _zoom: f64) {}
        pub fn set_user_agent(&self, _tab: TabId, _ua: Option<&str>) {}
        pub fn set_appearance(&mut self, _dark: Option<bool>) {}
        pub fn find(&self, _tab: TabId, _text: &str, _backwards: bool, _case_sensitive: bool) {}
        pub fn slept_scroll_y(&self, _tab: TabId) -> Option<f64> {
            None
        }
        pub fn layout(&self, _placements: &[(TabId, Bounds<Pixels>)]) {}
        pub fn focus_page(&self, _tab: TabId) -> bool {
            true
        }
        pub fn focus_shell(&self) {}
        pub fn edit(&self, _action: EditAction) {}
        pub fn minimize(&self) {}
        pub fn set_application_icon(&self, _path: &std::path::Path) {}
        pub fn take_events(&self) -> Vec<WebEvent> {
            vec![]
        }
        pub fn load_extension(&self, _dir: &std::path::Path, _id: &str, done: impl Fn(Result<String, String>) + 'static) {
            done(Err("extensions need WebKit (macOS)".into()));
        }
        pub fn list_login_candidates(&self, _url: &str) -> Vec<(String, String)> {
            vec![]
        }
        pub fn fill_login(&self, _tab: TabId, _url: &str, _form_idx: i64, _username: &str, _path: &str) -> Result<(), String> {
            Err("Keychain autofill requires macOS".into())
        }
        pub fn accept_save_login(&self, _tab: TabId) -> Result<String, String> {
            Err("Keychain save requires macOS".into())
        }
        pub fn decline_save_login(&self, _tab: TabId) {}
        pub fn set_login_capture(&mut self, _enabled: bool) {}
        pub fn clear_site_data(&self) {}
        pub fn take_media_key(&self) -> Option<&'static str> { None }
        pub fn set_default_browser(&self) {}
        pub fn set_media_permissions(&mut self, _entries: Vec<(String, i64, bool)>) {}
        pub fn clear_media_permissions(&mut self) {}
        pub fn set_downloads(&mut self, _dir: std::path::PathBuf, _ask: bool) {}
        pub fn set_handoff(&mut self, _url: Option<&str>, _title: &str) {}
        pub fn take_handoff(&self) -> Option<String> { None }
        pub fn toggle_float_panel(&mut self, _tab: TabId) {}
        pub fn extension_options_url(&self, _id: &str) -> Option<String> { None }
        pub fn open_extension_popup(&mut self, _id: &str) -> bool { false }
        pub fn extension_icon(&self, _id: &str) -> Option<Vec<u8>> { None }
        pub fn poll_float_panel(&mut self) {}
        pub fn export_pdf(&self, _tab: TabId) {}
        pub fn delete_saved_login(&self, _url: &str, _username: &str, _path: &str) -> Result<bool, String> {
            Ok(false)
        }
    }
}

/// A Keychain failure the user can act on. Denied means they cancelled the
/// Name profiles get their own Keychain security domain so a login saved in
/// one profile cannot surface in another. Default keeps the historical domain.
#[cfg(target_os = "macos")]
fn keychain_profile() -> Option<String> {
    std::env::var("RELAY_PROFILE").ok().filter(|p| !p.is_empty()).map(|p| format!("relay-profile:{p}"))
}

/// system prompt; Unavailable means the item/keychain could not be used.
#[cfg(target_os = "macos")]
fn keychain_message(failure: relay_extensions::login::SaveFailure) -> String {
    use relay_extensions::login::SaveFailure;
    match failure {
        SaveFailure::Denied => "Keychain access was denied".to_string(),
        SaveFailure::Unavailable => "The Keychain is unavailable".to_string(),
        SaveFailure::InvalidItem => "That saved item is not a usable login".to_string(),
        SaveFailure::Other => "The Keychain rejected the request".to_string(),
    }
}

pub use imp::WebHost;
