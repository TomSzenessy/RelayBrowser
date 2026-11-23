//! Wire contract between `js/bridge.js` and Rust. Every message the page posts
//! deserializes into [`PageMessage`]; every command Rust sends serializes from
//! [`Command`]. The page is untrusted: everything here is data, never code.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Wire protocol version. `js/bridge.js` sends the same value in `hello`.
pub const PROTOCOL_VERSION: u32 = 1;

/// Compact numeric element id assigned by the bridge (`data-relay-id`).
pub type NodeId = u32;

/// A lightweight DOM element as streamed by the bridge.
/// Attribute-only update for an existing node: body class/theme toggles and
/// other attribute mutations do not need a full subtree resend.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomAttrUpdate {
    pub id: NodeId,
    #[serde(default)]
    pub attrs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DomNode {
    pub id: NodeId,
    pub tag: String,
    #[serde(default)]
    pub attrs: BTreeMap<String, String>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub interactive: bool,
    #[serde(default)]
    pub children: Vec<DomNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleLevel {
    Debug,
    Log,
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetworkEntry {
    pub id: String,
    /// `fetch`, `xhr`, or `resource` (PerformanceObserver).
    pub api: String,
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub status: Option<u16>,
    #[serde(default)]
    pub status_text: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<f64>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub request_body: Option<String>,
    #[serde(default)]
    pub response_body: Option<String>,
    #[serde(default)]
    pub resource_type: Option<String>,
    #[serde(default)]
    pub transfer_size: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
    /// Page timestamp (ms since epoch).
    #[serde(default)]
    pub t: f64,
}

impl NetworkEntry {
    /// Failed requests: transport errors and HTTP >= 400.
    pub fn failed(&self) -> bool {
        self.error.is_some() || self.status.is_some_and(|s| s == 0 || s >= 400)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConsoleEntry {
    pub level: ConsoleLevel,
    pub args: Vec<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub t: f64,
}

impl ConsoleEntry {
    pub fn message(&self) -> String {
        self.args.join(" ")
    }
}

/// Human-in-the-loop prompts that pause attached agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Challenge {
    Captcha,
    TwoFactor,
}

/// Messages posted by `bridge.js`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PageMessage {
    Hello { url: String, version: u32 },
    Page { url: String, #[serde(default)] title: String },
    DomSnapshot { root: Option<DomNode> },
    DomPatch { nodes: Vec<DomNode>, #[serde(default)] removed: Vec<NodeId> },
    DomAttrs { nodes: Vec<DomAttrUpdate> },
    Console(ConsoleEntry),
    Network(NetworkEntry),
    Challenge { challenge: Option<Challenge> },
    Media { playing: bool },
    /// A browser hotkey pressed while the page had keyboard focus (e.g. `f`
    /// for link hints); WebKit, not GPUI, receives keys then.
    Hotkey { key: String },
    /// Markdown rendering of the page's main content (reply to `Op::Markdown`).
    Markdown { text: String },
    Result(CommandResult),
}

impl PageMessage {
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

/// Reply to a [`Command`], matched by id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandResult {
    pub id: u64,
    pub ok: bool,
    #[serde(default)]
    pub value: Value,
    #[serde(default)]
    pub error: Option<String>,
}

/// Actions Rust asks the bridge to perform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Snapshot,
    Highlight { target: NodeId },
    Unhighlight,
    EditStyle { target: NodeId, css: String },
    ScrollIntoView { target: NodeId },
    Click { target: NodeId },
    Type { target: NodeId, text: String, #[serde(default)] submit: bool, #[serde(default = "yes")] clear: bool },
    Query { selector: String },
    FindText { text: String },
    ReadText { #[serde(skip_serializing_if = "Option::is_none")] target: Option<NodeId> },
    Scroll { x: f64, y: f64 },
    ScrollTo { x: f64, y: f64 },
    PageInfo,
    /// Label interactive elements for keyboard clicking: `[id, label]` pairs.
    ShowHints { hints: Vec<(NodeId, String)> },
    HideHints,
    /// Convert the main content (article/main/body) to Markdown, capped at
    /// 200 KB. The string is both the command result and a `markdown` message.
    Markdown,
    /// Read the page's web app manifest and its best PNG icon. The reply is
    /// untrusted data: `relay-ui` validates origin, size, and image format
    /// before it writes anything to disk.
    WebApp,
    /// Read the page's icon as a small PNG/JPEG/WebP/GIF data URL. Same
    /// untrusted-data rules as `WebApp`; SVG/ICO are refused.
    Favicon,
    /// Dispatch a key press to the focused element (or `<body>`). Modifiers
    /// are `ctrl`, `alt`, `shift`, `meta`; text entry stays `type_text`.
    PressKey { key: String, #[serde(default)] modifiers: Vec<String> },
    /// Read localStorage/sessionStorage with size and secret-name caps.
    Storage,
    /// Evaluate one JavaScript expression in the page and return its JSON
    /// value. Gated by the shell's `agent_eval` setting and the Allow policy.
    Eval { script: String },
}

fn yes() -> bool {
    true
}

impl Op {
    /// Ops that change the page; refused while a tab waits for a human.
    pub fn mutates(&self) -> bool {
        matches!(self, Op::Click { .. } | Op::Type { .. } | Op::EditStyle { .. } | Op::PressKey { .. } | Op::Eval { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Command {
    pub id: u64,
    #[serde(flatten)]
    pub op: Op,
}

impl Command {
    /// JavaScript to evaluate in the page. The payload is JSON, so it cannot
    /// break out of the call expression.
    pub fn to_script(&self) -> String {
        let json = serde_json::to_string(self).expect("commands always serialize");
        format!("window.__relay && window.__relay.exec({json}), void 0")
    }
}

/// The bridge script, injected at document start.
pub const BRIDGE_JS: &str = include_str!("../../../js/bridge.js");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_page_message_kind() {
        let cases = [
            r#"{"kind":"hello","url":"https://a.test/","version":1,"t":1}"#,
            r#"{"kind":"page","url":"https://a.test/","title":"A","t":1}"#,
            r#"{"kind":"dom_snapshot","root":{"id":1,"tag":"body","attrs":{},"text":"","interactive":false,"children":[]}}"#,
            r#"{"kind":"dom_patch","nodes":[],"removed":[3,4]}"#,
            r#"{"kind":"dom_attrs","nodes":[{"id":2,"attrs":{"class":"dark"}}]}"#,
            r#"{"kind":"console","level":"warn","args":["x"],"t":5}"#,
            r#"{"kind":"network","id":"f1","api":"fetch","method":"GET","url":"https://a.test/api","status":500,"duration_ms":3.2}"#,
            r#"{"kind":"challenge","challenge":"two_factor"}"#,
            r#"{"kind":"challenge","challenge":null}"#,
            r#"{"kind":"media","playing":true}"#,
            r#"{"kind":"hotkey","key":"f"}"#,
            r##"{"kind":"markdown","text":"# T\n","t":2}"##,
            r#"{"kind":"result","id":7,"ok":true,"value":{"tag":"button"},"error":null}"#,
        ];
        for c in cases {
            PageMessage::parse(c).unwrap_or_else(|e| panic!("{c}: {e}"));
        }
    }

    #[test]
    fn command_script_is_a_single_json_call() {
        let cmd = Command { id: 3, op: Op::Type { target: 9, text: "a\")</script>;alert(1)//".into(), submit: true, clear: true } };
        let s = cmd.to_script();
        assert!(s.starts_with("window.__relay && window.__relay.exec({"));
        let json = s.trim_start_matches("window.__relay && window.__relay.exec(").trim_end_matches("), void 0");
        let v: Value = serde_json::from_str(json).unwrap();
        assert_eq!(v["op"], "type");
        assert_eq!(v["target"], 9);
        assert_eq!(v["id"], 3);
    }

    #[test]
    fn markdown_op_round_trips_and_is_read_only() {
        let op: Op = serde_json::from_str(r#"{"op":"markdown"}"#).unwrap();
        assert_eq!(op, Op::Markdown);
        assert!(!op.mutates());
        let s = Command { id: 4, op }.to_script();
        assert!(s.contains(r#""op":"markdown""#), "{s}");
        match PageMessage::parse(r#"{"kind":"markdown","text":"hi"}"#).unwrap() {
            PageMessage::Markdown { text } => assert_eq!(text, "hi"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn web_app_op_round_trips_and_is_read_only() {
        let op: Op = serde_json::from_str(r#"{"op":"web_app"}"#).unwrap();
        assert_eq!(op, Op::WebApp);
        assert!(!op.mutates());
        let s = Command { id: 5, op }.to_script();
        assert!(s.contains(r#""op":"web_app""#), "{s}");
        let op: Op = serde_json::from_str(r#"{"op":"favicon"}"#).unwrap();
        assert_eq!(op, Op::Favicon);
        assert!(!op.mutates());
        let s = Command { id: 6, op }.to_script();
        assert!(s.contains(r#""op":"favicon""#), "{s}");
    }

    #[test]
    fn failed_requests() {
        let mut e: NetworkEntry = serde_json::from_str(r#"{"id":"1","api":"fetch","method":"GET","url":"u","status":200}"#).unwrap();
        assert!(!e.failed());
        e.status = Some(502);
        assert!(e.failed());
        e.status = None;
        e.error = Some("offline".into());
        assert!(e.failed());
    }
}
