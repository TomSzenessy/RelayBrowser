//! The web-content host behind the shell. On macOS it drives the WebKit
//! `Engine` inside GPUI's NSView; elsewhere it is a no-op stand-in so the
//! shell (and every portable test) builds and runs on Linux, with a
//! placeholder where pages would render.

use gpui::{Bounds, Pixels, Window};
use relay_bridge::{BrowserRequest, SharedHub, TabId};

/// Things WebKit tells the shell (only the macOS engine produces them).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq)]
pub enum WebEvent {
    OpenInNewTab { opener: TabId, url: String },
    Slept(TabId),
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use objc2::rc::Retained;
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSView;
    use objc2_foundation::{NSPoint, NSRect, NSSize};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use relay_webkit::mac::{Engine, EngineEvent};
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
            let shell = window.window_handle().ok().and_then(|h| match h.as_raw() {
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

        pub fn focus_page(&self, tab: TabId) {
            self.engine.focus_page(tab);
        }

        pub fn focus_shell(&self) {
            if let Some(s) = &self.shell {
                Engine::focus_shell(s);
            }
        }

        pub fn take_events(&self) -> Vec<WebEvent> {
            std::mem::take(&mut *self.events.borrow_mut())
        }

        pub fn load_extension(&self, dir: &std::path::Path, id: &str, done: impl Fn(Result<String, String>) + 'static) {
            if let Some(c) = self.engine.extension_controller() {
                relay_extensions::mac::load_extension(self.mtm, c, dir, id, done);
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
        pub fn apply(&mut self, _req: BrowserRequest) {}
        pub fn wake(&mut self, _tab: TabId) {}
        pub fn after_load(&mut self, _tab: TabId) {}
        pub fn sleep(&mut self, _tab: TabId, _scroll: (f64, f64)) {}
        pub fn snapshot(&self, _tab: TabId) -> Option<Vec<u8>> {
            None
        }
        pub fn layout(&self, _placements: &[(TabId, Bounds<Pixels>)]) {}
        pub fn focus_page(&self, _tab: TabId) {}
        pub fn focus_shell(&self) {}
        pub fn take_events(&self) -> Vec<WebEvent> {
            vec![]
        }
        pub fn load_extension(&self, _dir: &std::path::Path, _id: &str, done: impl Fn(Result<String, String>) + 'static) {
            done(Err("extensions need WebKit (macOS)".into()));
        }
    }
}

pub use imp::WebHost;
