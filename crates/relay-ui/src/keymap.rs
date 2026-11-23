//! Editable keyboard shortcuts: one action table, default keystrokes, user
//! overrides, conflict/reserved validation, and the GPUI bindings built from
//! them. `app::key_bindings()` delegates here so the default map has one owner.

use crate::app::{
    Back, CloseTab, Copy, CopyCleanUrl, CopyMarkdown, Cut, CycleAgentPolicy, Find, Forward, LinkHints, Minimize,
    NewTab, NextTab, NextTabStripPosition, OpenLocation, OpenPalette, Paste, PrevTab, Quit, Redo, ReopenClosedTab, Reload,
    SelectAll, SelectTab1, SelectTab2, SelectTab3, SelectTab4, SelectTab5, SelectTab6, SelectTab7, SelectTab8, SelectTab9, ShowBookmarks,
    ShowHistory, ToggleAgents, ToggleBookmark, ToggleInspector, TogglePin, ToggleSettings, ToggleSplit, ToggleTabStrip, Undo, WebInspector,
    ZoomIn, ZoomOut, ZoomReset,
};
use gpui::{KeyBinding, Keystroke};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActionId {
    OpenPalette,
    OpenLocation,
    NewTab,
    CloseTab,
    ReopenClosedTab,
    Reload,
    Back,
    Forward,
    NextTab,
    PrevTab,
    ToggleInspector,
    WebInspector,
    ToggleSplit,
    ToggleSettings,
    LinkHints,
    CopyCleanUrl,
    CopyMarkdown,
    ToggleBookmark,
    TogglePin,
    Find,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    Quit,
    SelectTab1,
    SelectTab2,
    SelectTab3,
    SelectTab4,
    SelectTab5,
    SelectTab6,
    SelectTab7,
    SelectTab8,
    SelectTab9,
    ToggleTabStrip,
    NextTabStripPosition,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Undo,
    Redo,
    Minimize,
    ShowHistory,
    ShowBookmarks,
    ToggleAgents,
    CycleAgentPolicy,
}

impl ActionId {
    pub const ALL: [ActionId; 46] = [
        ActionId::OpenPalette, ActionId::OpenLocation, ActionId::NewTab, ActionId::CloseTab, ActionId::ReopenClosedTab,
        ActionId::Reload, ActionId::Back, ActionId::Forward, ActionId::NextTab, ActionId::PrevTab, ActionId::ToggleInspector,
        ActionId::WebInspector, ActionId::ToggleSplit, ActionId::ToggleSettings, ActionId::LinkHints, ActionId::CopyCleanUrl,
        ActionId::CopyMarkdown, ActionId::ToggleBookmark, ActionId::TogglePin, ActionId::Find, ActionId::ZoomIn, ActionId::ZoomOut,
        ActionId::ZoomReset, ActionId::Quit, ActionId::SelectTab1, ActionId::SelectTab2, ActionId::SelectTab3, ActionId::SelectTab4,
        ActionId::SelectTab5, ActionId::SelectTab6, ActionId::SelectTab7, ActionId::SelectTab8, ActionId::SelectTab9,
        ActionId::ToggleTabStrip, ActionId::NextTabStripPosition, ActionId::Cut, ActionId::Copy, ActionId::Paste, ActionId::SelectAll,
        ActionId::Undo, ActionId::Redo, ActionId::Minimize, ActionId::ShowHistory, ActionId::ShowBookmarks, ActionId::ToggleAgents,
        ActionId::CycleAgentPolicy,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ActionId::OpenPalette => "palette",
            ActionId::OpenLocation => "location",
            ActionId::NewTab => "new-tab",
            ActionId::CloseTab => "close-tab",
            ActionId::ReopenClosedTab => "reopen-tab",
            ActionId::Reload => "reload",
            ActionId::Back => "back",
            ActionId::Forward => "forward",
            ActionId::NextTab => "next-tab",
            ActionId::PrevTab => "prev-tab",
            ActionId::ToggleInspector => "inspector",
            ActionId::WebInspector => "web-inspector",
            ActionId::ToggleSplit => "split",
            ActionId::ToggleSettings => "settings",
            ActionId::LinkHints => "hints",
            ActionId::CopyCleanUrl => "copy-clean-url",
            ActionId::CopyMarkdown => "copy-markdown",
            ActionId::ToggleBookmark => "bookmark",
            ActionId::TogglePin => "pin",
            ActionId::Find => "find",
            ActionId::ZoomIn => "zoom-in",
            ActionId::ZoomOut => "zoom-out",
            ActionId::ZoomReset => "zoom-reset",
            ActionId::Quit => "quit",
            ActionId::SelectTab1 => "tab-1",
            ActionId::SelectTab2 => "tab-2",
            ActionId::SelectTab3 => "tab-3",
            ActionId::SelectTab4 => "tab-4",
            ActionId::SelectTab5 => "tab-5",
            ActionId::SelectTab6 => "tab-6",
            ActionId::SelectTab7 => "tab-7",
            ActionId::SelectTab8 => "tab-8",
            ActionId::SelectTab9 => "tab-9",
            ActionId::ToggleTabStrip => "fold-tabs",
            ActionId::NextTabStripPosition => "move-tabs",
            ActionId::Cut => "cut",
            ActionId::Copy => "copy",
            ActionId::Paste => "paste",
            ActionId::SelectAll => "select-all",
            ActionId::Undo => "undo",
            ActionId::Redo => "redo",
            ActionId::Minimize => "minimize",
            ActionId::ShowHistory => "history",
            ActionId::ShowBookmarks => "bookmarks",
            ActionId::ToggleAgents => "pause-agents",
            ActionId::CycleAgentPolicy => "agent-policy",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == value)
    }

    pub fn label(self) -> &'static str {
        match self {
            ActionId::OpenPalette => "Command palette",
            ActionId::OpenLocation => "Open location",
            ActionId::NewTab => "New tab",
            ActionId::CloseTab => "Close tab",
            ActionId::ReopenClosedTab => "Reopen closed tab",
            ActionId::Reload => "Reload page",
            ActionId::Back => "Go back",
            ActionId::Forward => "Go forward",
            ActionId::NextTab => "Next tab",
            ActionId::PrevTab => "Previous tab",
            ActionId::ToggleInspector => "Toggle Inspector",
            ActionId::WebInspector => "WebKit Web Inspector",
            ActionId::ToggleSplit => "Toggle split view",
            ActionId::ToggleSettings => "Settings",
            ActionId::LinkHints => "Link hints",
            ActionId::CopyCleanUrl => "Copy clean URL",
            ActionId::CopyMarkdown => "Copy page as Markdown",
            ActionId::ToggleBookmark => "Bookmark this page",
            ActionId::TogglePin => "Pin / unpin tab",
            ActionId::Find => "Find in page",
            ActionId::ZoomIn => "Zoom in",
            ActionId::ZoomOut => "Zoom out",
            ActionId::ZoomReset => "Actual size",
            ActionId::Quit => "Quit Relay",
            ActionId::SelectTab1 => "Select tab 1",
            ActionId::SelectTab2 => "Select tab 2",
            ActionId::SelectTab3 => "Select tab 3",
            ActionId::SelectTab4 => "Select tab 4",
            ActionId::SelectTab5 => "Select tab 5",
            ActionId::SelectTab6 => "Select tab 6",
            ActionId::SelectTab7 => "Select tab 7",
            ActionId::SelectTab8 => "Select tab 8",
            ActionId::SelectTab9 => "Select tab 9",
            ActionId::ToggleTabStrip => "Fold / unfold tab strip",
            ActionId::NextTabStripPosition => "Move tab strip",
            ActionId::Cut => "Cut",
            ActionId::Copy => "Copy",
            ActionId::Paste => "Paste",
            ActionId::SelectAll => "Select all",
            ActionId::Undo => "Undo",
            ActionId::Redo => "Redo",
            ActionId::Minimize => "Minimize window",
            ActionId::ShowHistory => "Show history",
            ActionId::ShowBookmarks => "Show bookmarks",
            ActionId::ToggleAgents => "Pause / resume agents",
            ActionId::CycleAgentPolicy => "Cycle agent access on this site",
        }
    }

    pub fn default_key(self) -> &'static str {
        match self {
            ActionId::OpenPalette => "secondary-k",
            ActionId::OpenLocation => "secondary-l",
            ActionId::NewTab => "secondary-t",
            ActionId::CloseTab => "secondary-w",
            ActionId::ReopenClosedTab => "secondary-shift-t",
            ActionId::Reload => "secondary-r",
            ActionId::Back => "secondary-[",
            ActionId::Forward => "secondary-]",
            ActionId::NextTab => "ctrl-tab",
            ActionId::PrevTab => "ctrl-shift-tab",
            ActionId::ToggleInspector => "secondary-alt-i",
            ActionId::WebInspector => "secondary-alt-u",
            ActionId::ToggleSplit => "secondary-\\",
            ActionId::ToggleSettings => "secondary-,",
            ActionId::LinkHints => "secondary-j",
            ActionId::CopyCleanUrl => "secondary-shift-c",
            ActionId::CopyMarkdown => "secondary-shift-m",
            ActionId::ToggleBookmark => "secondary-d",
            ActionId::TogglePin => "secondary-alt-p",
            ActionId::Find => "secondary-f",
            ActionId::ZoomIn => "secondary-=",
            ActionId::ZoomOut => "secondary--",
            ActionId::ZoomReset => "secondary-0",
            ActionId::Quit => "secondary-q",
            ActionId::SelectTab1 => "secondary-1",
            ActionId::SelectTab2 => "secondary-2",
            ActionId::SelectTab3 => "secondary-3",
            ActionId::SelectTab4 => "secondary-4",
            ActionId::SelectTab5 => "secondary-5",
            ActionId::SelectTab6 => "secondary-6",
            ActionId::SelectTab7 => "secondary-7",
            ActionId::SelectTab8 => "secondary-8",
            ActionId::SelectTab9 => "secondary-9",
            ActionId::ToggleTabStrip => "secondary-s",
            ActionId::NextTabStripPosition => "secondary-shift-s",
            ActionId::Cut => "secondary-x",
            ActionId::Copy => "secondary-c",
            ActionId::Paste => "secondary-v",
            ActionId::SelectAll => "secondary-a",
            ActionId::Undo => "secondary-z",
            ActionId::Redo => "secondary-shift-z",
            ActionId::Minimize => "secondary-m",
            ActionId::ShowHistory => "secondary-y",
            ActionId::ShowBookmarks => "secondary-shift-b",
            ActionId::ToggleAgents => "secondary-shift-.",
            ActionId::CycleAgentPolicy => "secondary-alt-a",
        }
    }

    fn binding(self, keystroke: &str) -> KeyBinding {
        match self {
            ActionId::OpenPalette => KeyBinding::new(keystroke, OpenPalette, None),
            ActionId::OpenLocation => KeyBinding::new(keystroke, OpenLocation, None),
            ActionId::NewTab => KeyBinding::new(keystroke, NewTab, None),
            ActionId::CloseTab => KeyBinding::new(keystroke, CloseTab, None),
            ActionId::ReopenClosedTab => KeyBinding::new(keystroke, ReopenClosedTab, None),
            ActionId::Reload => KeyBinding::new(keystroke, Reload, None),
            ActionId::Back => KeyBinding::new(keystroke, Back, None),
            ActionId::Forward => KeyBinding::new(keystroke, Forward, None),
            ActionId::NextTab => KeyBinding::new(keystroke, NextTab, None),
            ActionId::PrevTab => KeyBinding::new(keystroke, PrevTab, None),
            ActionId::ToggleInspector => KeyBinding::new(keystroke, ToggleInspector, None),
            ActionId::WebInspector => KeyBinding::new(keystroke, WebInspector, None),
            ActionId::ToggleSplit => KeyBinding::new(keystroke, ToggleSplit, None),
            ActionId::ToggleSettings => KeyBinding::new(keystroke, ToggleSettings, None),
            ActionId::LinkHints => KeyBinding::new(keystroke, LinkHints, None),
            ActionId::CopyCleanUrl => KeyBinding::new(keystroke, CopyCleanUrl, None),
            ActionId::CopyMarkdown => KeyBinding::new(keystroke, CopyMarkdown, None),
            ActionId::ToggleBookmark => KeyBinding::new(keystroke, ToggleBookmark, None),
            ActionId::TogglePin => KeyBinding::new(keystroke, TogglePin, None),
            ActionId::Find => KeyBinding::new(keystroke, Find, None),
            ActionId::ZoomIn => KeyBinding::new(keystroke, ZoomIn, None),
            ActionId::ZoomOut => KeyBinding::new(keystroke, ZoomOut, None),
            ActionId::ZoomReset => KeyBinding::new(keystroke, ZoomReset, None),
            ActionId::Quit => KeyBinding::new(keystroke, Quit, None),
            ActionId::SelectTab1 => KeyBinding::new(keystroke, SelectTab1, None),
            ActionId::SelectTab2 => KeyBinding::new(keystroke, SelectTab2, None),
            ActionId::SelectTab3 => KeyBinding::new(keystroke, SelectTab3, None),
            ActionId::SelectTab4 => KeyBinding::new(keystroke, SelectTab4, None),
            ActionId::SelectTab5 => KeyBinding::new(keystroke, SelectTab5, None),
            ActionId::SelectTab6 => KeyBinding::new(keystroke, SelectTab6, None),
            ActionId::SelectTab7 => KeyBinding::new(keystroke, SelectTab7, None),
            ActionId::SelectTab8 => KeyBinding::new(keystroke, SelectTab8, None),
            ActionId::SelectTab9 => KeyBinding::new(keystroke, SelectTab9, None),
            ActionId::ToggleTabStrip => KeyBinding::new(keystroke, ToggleTabStrip, None),
            ActionId::NextTabStripPosition => KeyBinding::new(keystroke, NextTabStripPosition, None),
            ActionId::Cut => KeyBinding::new(keystroke, Cut, None),
            ActionId::Copy => KeyBinding::new(keystroke, Copy, None),
            ActionId::Paste => KeyBinding::new(keystroke, Paste, None),
            ActionId::SelectAll => KeyBinding::new(keystroke, SelectAll, None),
            ActionId::Undo => KeyBinding::new(keystroke, Undo, None),
            ActionId::Redo => KeyBinding::new(keystroke, Redo, None),
            ActionId::Minimize => KeyBinding::new(keystroke, Minimize, None),
            ActionId::ShowHistory => KeyBinding::new(keystroke, ShowHistory, None),
            ActionId::ShowBookmarks => KeyBinding::new(keystroke, ShowBookmarks, None),
            ActionId::ToggleAgents => KeyBinding::new(keystroke, ToggleAgents, None),
            ActionId::CycleAgentPolicy => KeyBinding::new(keystroke, CycleAgentPolicy, None),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keymap {
    overrides: BTreeMap<ActionId, String>,
}

impl Keymap {
    pub fn from_setting(raw: Option<&str>) -> Self {
        let mut map = Self::default();
        for pair in raw.unwrap_or("").split(';') {
            let Some((id, key)) = pair.split_once('=') else { continue };
            let (Some(id), Ok(parsed)) = (ActionId::parse(id.trim()), Keystroke::parse(key.trim())) else { continue };
            let key = canonical(&parsed);
            if validate_key(&key).is_ok() {
                map.overrides.insert(id, key);
            }
        }
        map
    }

    pub fn key(&self, id: ActionId) -> &str {
        self.overrides.get(&id).map(String::as_str).unwrap_or_else(|| id.default_key())
    }

    pub fn bindings(&self) -> Vec<KeyBinding> {
        ActionId::ALL.into_iter().map(|id| id.binding(self.key(id))).collect()
    }

    pub fn set(&mut self, id: ActionId, keystroke: &str) -> Result<(), String> {
        let parsed = Keystroke::parse(keystroke).map_err(|_| format!("{keystroke:?} is not a shortcut"))?;
        let key = canonical(&parsed);
        validate_key(&key)?;
        if let Some(other) = ActionId::ALL.into_iter().find(|other| *other != id && same_key(self.key(*other), &key)) {
            return Err(format!("{} already uses {}", other.label(), display_key(&key)));
        }
        if same_key(&key, "secondary-q") && id != ActionId::Quit {
            return Err("⌘Q is reserved for Quit".to_string());
        }
        self.overrides.insert(id, key);
        Ok(())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn reset(&mut self, id: ActionId) {
        self.overrides.remove(&id);
    }

    pub fn reset_all(&mut self) {
        self.overrides.clear();
    }

    pub fn to_setting(&self) -> String {
        self.overrides.iter().map(|(id, key)| format!("{}={key}", id.as_str())).collect::<Vec<_>>().join(";")
    }
}

pub fn canonical(k: &Keystroke) -> String {
    let mut parts = vec![];
    if k.modifiers.platform {
        parts.push("secondary");
    }
    if k.modifiers.control {
        parts.push("ctrl");
    }
    if k.modifiers.alt {
        parts.push("alt");
    }
    if k.modifiers.shift {
        parts.push("shift");
    }
    parts.push(k.key.as_str());
    parts.join("-")
}

/// Compare before/after parsing so `secondary` and an explicit `ctrl` are
/// recognized as the same key on Linux (both become control).
fn same_key(a: &str, b: &str) -> bool {
    match (Keystroke::parse(a), Keystroke::parse(b)) {
        (Ok(a), Ok(b)) => a.modifiers == b.modifiers && a.key.eq_ignore_ascii_case(&b.key),
        _ => a.eq_ignore_ascii_case(b),
    }
}

pub fn validate_key(keystroke: &str) -> Result<(), String> {
    let parsed = Keystroke::parse(keystroke).map_err(|_| format!("{keystroke:?} is not a shortcut"))?;
    if !parsed.modifiers.platform && !parsed.modifiers.control {
        return Err("Use ⌘ or Control plus a key; a bare key belongs to the page".to_string());
    }
    Ok(())
}

/// Modifier-only presses return `None` so the recorder keeps waiting.
pub fn from_keystroke(k: &Keystroke) -> Option<String> {
    if matches!(k.key.as_str(), "shift" | "control" | "alt" | "platform" | "fn") {
        return None;
    }
    Some(canonical(k))
}

pub fn display_key(keystroke: &str) -> String {
    keystroke.replace("secondary-", "⌘").replace("ctrl-", "⌃").replace("alt-", "⌥").replace("shift-", "⇧")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_every_action_once() {
        let map = Keymap::default();
        let keys: std::collections::HashSet<&str> = ActionId::ALL.iter().map(|id| map.key(*id)).collect();
        assert_eq!(keys.len(), ActionId::ALL.len(), "default shortcuts are unique");
        assert_eq!(map.bindings().len(), 46);
        for id in ActionId::ALL {
            assert!(Keystroke::parse(map.key(id)).is_ok(), "{}", map.key(id));
        }
    }

    #[test]
    fn overrides_round_trip_and_reject_conflicts() {
        let mut map = Keymap::default();
        map.set(ActionId::Find, "ctrl-alt-f").unwrap();
        assert_eq!(map.key(ActionId::Find), "ctrl-alt-f");
        let raw = map.to_setting();
        assert!(raw.contains("find=ctrl-alt-f"), "{raw}");
        assert_eq!(Keymap::from_setting(Some(&raw)).key(ActionId::Find), "ctrl-alt-f");
        let new_tab = map.key(ActionId::NewTab).to_string();
        assert!(map.set(ActionId::Find, &new_tab).unwrap_err().contains("already uses"));
        assert!(map.set(ActionId::Find, "f").is_err(), "bare keys are page input");
        assert!(map.set(ActionId::Find, "secondary-q").is_err(), "reserved Quit");
        map.reset(ActionId::Find);
        assert_eq!(map.key(ActionId::Find), ActionId::Find.default_key());
        map.reset_all();
        assert_eq!(map, Keymap::default());
    }

    #[test]
    fn captured_modifiers_become_gpui_syntax() {
        let k: Keystroke = Keystroke::parse("ctrl-shift-k").unwrap();
        assert_eq!(from_keystroke(&k).as_deref(), Some("ctrl-shift-k"));
        let only = Keystroke { key: "shift".into(), key_char: None, modifiers: Default::default() };
        assert_eq!(from_keystroke(&only), None);
        assert_eq!(display_key("secondary-shift-s"), "⌘⇧s");
    }
}
