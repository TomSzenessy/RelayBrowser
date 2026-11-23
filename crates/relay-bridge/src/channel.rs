//! A [`Browser`] that forwards requests over a channel, so background threads
//! (the MCP server) can drive WebKit, which lives on the main thread. The UI
//! drains the receiver and applies each request to the engine.

use crate::hub::{Browser, TabId};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

pub enum BrowserRequest {
    Open { tab: TabId, url: String },
    Navigate { tab: TabId, url: String },
    GoBack(TabId),
    GoForward(TabId),
    Reload(TabId),
    Close(TabId),
    Activate(TabId),
    Eval { tab: TabId, script: String },
    RefreshCookies(TabId),
    OpenWebInspector(TabId),
    Screenshot { tab: TabId, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send> },
}

#[derive(Clone)]
pub struct ChannelBrowser {
    tx: UnboundedSender<BrowserRequest>,
}

impl ChannelBrowser {
    pub fn new() -> (Self, UnboundedReceiver<BrowserRequest>) {
        let (tx, rx) = unbounded_channel();
        (Self { tx }, rx)
    }

    fn send(&self, r: BrowserRequest) {
        // The receiver only closes when the app quits; dropping is correct then.
        let _ = self.tx.send(r);
    }
}

impl Browser for ChannelBrowser {
    fn open(&self, tab: TabId, url: &str) { self.send(BrowserRequest::Open { tab, url: url.into() }) }
    fn navigate(&self, tab: TabId, url: &str) { self.send(BrowserRequest::Navigate { tab, url: url.into() }) }
    fn go_back(&self, tab: TabId) { self.send(BrowserRequest::GoBack(tab)) }
    fn go_forward(&self, tab: TabId) { self.send(BrowserRequest::GoForward(tab)) }
    fn reload(&self, tab: TabId) { self.send(BrowserRequest::Reload(tab)) }
    fn close(&self, tab: TabId) { self.send(BrowserRequest::Close(tab)) }
    fn activate(&self, tab: TabId) { self.send(BrowserRequest::Activate(tab)) }
    fn eval(&self, tab: TabId, script: String) { self.send(BrowserRequest::Eval { tab, script }) }
    fn refresh_cookies(&self, tab: TabId) { self.send(BrowserRequest::RefreshCookies(tab)) }
    fn open_web_inspector(&self, tab: TabId) { self.send(BrowserRequest::OpenWebInspector(tab)) }
    fn screenshot(&self, tab: TabId, done: Box<dyn FnOnce(Result<Vec<u8>, String>) + Send>) {
        let _ = self.tx.send(BrowserRequest::Screenshot { tab, done });
    }
}
