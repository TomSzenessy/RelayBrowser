//! The "One Bridge": `js/bridge.js` plus the Rust state it feeds.
//!
//! The Inspector UI and the MCP server are two frontends over [`SharedHub`];
//! the WebKit layer is its only writer of page state.

pub mod channel;
pub mod hub;
pub mod paths;
pub mod protocol;

pub use channel::{BrowserRequest, ChannelBrowser};
pub use hub::{run_op, Browser, Cookie, DomStore, Hub, HubError, Owner, SharedHub, TabId, TabState, TabSummary};
pub use protocol::{Challenge, Command, CommandResult, ConsoleEntry, ConsoleLevel, DomNode, NetworkEntry, NodeId, Op, PageMessage, BRIDGE_JS};
