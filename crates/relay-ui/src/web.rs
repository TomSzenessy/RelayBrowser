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
                })
            });
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

        /// Select the next (or previous) match; the answer arrives as
        /// [`WebEvent::Found`].
        pub fn find(&self, tab: TabId, text: &str, backwards: bool) {
            let events = self.events.clone();
            self.engine.find(tab, text, backwards, move |found| events.borrow_mut().push(WebEvent::Found { tab, found }));
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
        pub fn find(&self, _tab: TabId, _text: &str, _backwards: bool) {}
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
    }
}

pub use imp::WebHost;
