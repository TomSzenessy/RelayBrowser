//! Editable keyboard shortcuts: one action table, default keystrokes, user
//! overrides, conflict/reserved validation, and the GPUI bindings built from
//! them. `app::key_bindings()` delegates here so the default map has one owner.

use gpui::{KeyBinding, Keystroke};
use std::collections::BTreeMap;

/// One row of the action table: stable id, Settings label, default keystroke,
/// and the GPUI action type that carries it. Add an action by adding one row to
/// `action_table!`; the enum, `ALL`, every accessor, and the bindings follow.
#[derive(Debug, Clone, Copy)]
struct ActionSpec {
    id: ActionId,
    name: &'static str,
    label: &'static str,
    key: &'static str,
    #[cfg_attr(not(test), allow(dead_code))]
    action_name: fn() -> &'static str,
    binding: fn(&str) -> KeyBinding,
}

fn action_name<A: gpui::Action>() -> &'static str {
    A::name_for_type()
}

fn binding<A: gpui::Action + Default>(keystroke: &str) -> KeyBinding {
    KeyBinding::new(keystroke, A::default(), None)
}

macro_rules! action_table {
    ($($variant:ident => $id:literal, $label:literal, $key:literal;)*) => {
        /// Stable identity of a keyboard action, also the settings/MCP id.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum ActionId { $($variant),* }

        impl ActionId {
            /// Every action, in Settings → Keyboard shortcuts order.
            pub const ALL: &'static [ActionId] = &[$(ActionId::$variant),*];

            fn spec(self) -> &'static ActionSpec {
                static SPECS: &[ActionSpec] = &[$(
                    ActionSpec {
                        id: ActionId::$variant,
                        name: $id,
                        label: $label,
                        key: $key,
                        action_name: action_name::<crate::app::$variant>,
                        binding: binding::<crate::app::$variant>,
                    }
                ),*];
                SPECS.iter().find(|spec| spec.id == self).expect("every ActionId has a spec row")
            }

            pub fn as_str(self) -> &'static str {
                self.spec().name
            }

            pub fn parse(value: &str) -> Option<Self> {
                Self::ALL.iter().copied().find(|a| a.as_str() == value)
            }

            pub fn label(self) -> &'static str {
                self.spec().label
            }

            pub fn default_key(self) -> &'static str {
                self.spec().key
            }

            /// The `actions!` type name, which links a binding back to this row.
            #[cfg_attr(not(test), allow(dead_code))]
            pub fn action_name(self) -> &'static str {
                (self.spec().action_name)()
            }

            fn binding(self, keystroke: &str) -> KeyBinding {
                (self.spec().binding)(keystroke)
            }
        }
    };
}

action_table! {
    OpenPalette => "palette", "Command palette", "secondary-k";
    OpenLocation => "location", "Open location", "secondary-l";
    NewTab => "new-tab", "New tab", "secondary-t";
    CloseTab => "close-tab", "Close tab", "secondary-w";
    ReopenClosedTab => "reopen-tab", "Reopen closed tab", "secondary-shift-t";
    Reload => "reload", "Reload page", "secondary-r";
    Back => "back", "Go back", "secondary-[";
    Forward => "forward", "Go forward", "secondary-]";
    NextTab => "next-tab", "Next recent tab", "ctrl-tab";
    PrevTab => "prev-tab", "Previous recent tab", "ctrl-shift-tab";
    ToggleInspector => "inspector", "Toggle Inspector", "secondary-alt-i";
    WebInspector => "web-inspector", "WebKit Web Inspector", "secondary-alt-u";
    ToggleSplit => "split", "Toggle split view", "secondary-\\";
    ToggleSettings => "settings", "Settings", "secondary-,";
    LinkHints => "hints", "Link hints", "secondary-j";
    CopyCleanUrl => "copy-clean-url", "Copy clean URL", "secondary-shift-c";
    CopyMarkdown => "copy-markdown", "Copy page as Markdown", "secondary-shift-m";
    ToggleBookmark => "bookmark", "Bookmark this page", "secondary-d";
    TogglePin => "pin", "Pin / unpin tab", "secondary-alt-p";
    Find => "find", "Find in page", "secondary-f";
    ToggleFloatVideo => "float-video", "Float video", "secondary-shift-f";
    ToggleFloatPanel => "float-panel", "Float video in panel", "secondary-alt-shift-f";
    AskAi => "ask-ai", "Ask AI", "secondary-shift-a";
    ShowTabSwitcher => "tab-switcher", "Tab switcher", "secondary-alt-t";
    FindNext => "find-next", "Find next", "secondary-g";
    FindPrevious => "find-prev", "Find previous", "secondary-shift-g";
    ZoomIn => "zoom-in", "Zoom in", "secondary-=";
    ZoomOut => "zoom-out", "Zoom out", "secondary--";
    ZoomReset => "zoom-reset", "Actual size", "secondary-0";
    Quit => "quit", "Quit Relay", "secondary-q";
    SelectTab1 => "tab-1", "Select tab 1", "secondary-1";
    SelectTab2 => "tab-2", "Select tab 2", "secondary-2";
    SelectTab3 => "tab-3", "Select tab 3", "secondary-3";
    SelectTab4 => "tab-4", "Select tab 4", "secondary-4";
    SelectTab5 => "tab-5", "Select tab 5", "secondary-5";
    SelectTab6 => "tab-6", "Select tab 6", "secondary-6";
    SelectTab7 => "tab-7", "Select tab 7", "secondary-7";
    SelectTab8 => "tab-8", "Select tab 8", "secondary-8";
    SelectTab9 => "tab-9", "Select tab 9", "secondary-9";
    ToggleTabStrip => "fold-tabs", "Fold / unfold tab strip", "secondary-s";
    NextTabStripPosition => "move-tabs", "Move tab strip", "secondary-shift-s";
    Cut => "cut", "Cut", "secondary-x";
    Copy => "copy", "Copy", "secondary-c";
    Paste => "paste", "Paste", "secondary-v";
    SelectAll => "select-all", "Select all", "secondary-a";
    Undo => "undo", "Undo", "secondary-z";
    Redo => "redo", "Redo", "secondary-shift-z";
    Minimize => "minimize", "Minimize window", "secondary-m";
    ShowHistory => "history", "Show history", "secondary-y";
    ShowBookmarks => "bookmarks", "Show bookmarks", "secondary-shift-b";
    ToggleAgents => "pause-agents", "Pause / resume agents", "secondary-shift-.";
    CycleAgentPolicy => "agent-policy", "Cycle agent access on this site", "secondary-alt-a";
    ToggleReader => "reading-mode", "Reading mode", "secondary-shift-r";
    TogglePictureInPicture => "picture-in-picture", "Picture in Picture", "secondary-shift-p";
    ShowDownloads => "downloads", "Show downloads", "secondary-shift-j";
    NewPrivateTab => "new-private-tab", "New private tab", "secondary-shift-n";
    Print => "print", "Print page", "secondary-p";
    ToggleBottomBar => "bottom-bar", "Cycle status bar (Top / Bottom / Hidden)", "secondary-alt-b";
    FoldAllGroups => "fold-groups", "Fold all tab groups", "secondary-alt-[";
    UnfoldAllGroups => "unfold-groups", "Unfold all tab groups", "secondary-alt-]";
    Spaces => "spaces", "Switch space…", "secondary-shift-e";
}

/// The headings Settings → Keyboard shortcuts draws, in order. A new action
/// must be assigned to exactly one group; the test below fails otherwise.
pub const SHORTCUT_GROUPS: [&str; 6] = ["Essentials", "Tabs", "Page", "Window", "Agents", "Editing"];

impl ActionId {
    pub fn group(self) -> &'static str {
        match self {
            ActionId::OpenPalette
            | ActionId::OpenLocation
            | ActionId::NewTab
            | ActionId::CloseTab
            | ActionId::ReopenClosedTab
            | ActionId::Reload
            | ActionId::Back
            | ActionId::Forward
            | ActionId::Quit => "Essentials",
            ActionId::NextTab
            | ActionId::PrevTab
            | ActionId::SelectTab1
            | ActionId::SelectTab2
            | ActionId::SelectTab3
            | ActionId::SelectTab4
            | ActionId::SelectTab5
            | ActionId::SelectTab6
            | ActionId::SelectTab7
            | ActionId::SelectTab8
            | ActionId::SelectTab9
            | ActionId::TogglePin
            | ActionId::ToggleTabStrip
            | ActionId::NextTabStripPosition
            | ActionId::NewPrivateTab
            | ActionId::FoldAllGroups
            | ActionId::UnfoldAllGroups
            | ActionId::Spaces => "Tabs",
            ActionId::Find
            | ActionId::ToggleFloatVideo
            | ActionId::ToggleFloatPanel
            | ActionId::AskAi
            | ActionId::ShowTabSwitcher
            | ActionId::FindNext
            | ActionId::FindPrevious
            | ActionId::ZoomIn
            | ActionId::ZoomOut
            | ActionId::ZoomReset
            | ActionId::LinkHints
            | ActionId::ToggleReader
            | ActionId::TogglePictureInPicture
            | ActionId::Print
            | ActionId::CopyCleanUrl
            | ActionId::CopyMarkdown
            | ActionId::ToggleBookmark => "Page",
            ActionId::ToggleInspector
            | ActionId::WebInspector
            | ActionId::ToggleSplit
            | ActionId::ToggleSettings
            | ActionId::ToggleBottomBar
            | ActionId::Minimize
            | ActionId::ShowHistory
            | ActionId::ShowBookmarks
            | ActionId::ShowDownloads => "Window",
            ActionId::ToggleAgents | ActionId::CycleAgentPolicy => "Agents",
            ActionId::Cut | ActionId::Copy | ActionId::Paste | ActionId::SelectAll | ActionId::Undo | ActionId::Redo => "Editing",
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
            // Skip conflict/reserved overrides instead of silently accepting a
            // map that would bind two actions to one key.
            let _ = map.set(id, &key);
        }
        map
    }

    pub fn key(&self, id: ActionId) -> &str {
        self.overrides.get(&id).map(String::as_str).unwrap_or_else(|| id.default_key())
    }

    pub fn bindings(&self) -> Vec<KeyBinding> {
        ActionId::ALL.iter().map(|id| id.binding(self.key(*id))).collect()
    }

    pub fn set(&mut self, id: ActionId, keystroke: &str) -> Result<(), String> {
        let parsed = Keystroke::parse(keystroke).map_err(|_| format!("{keystroke:?} is not a shortcut"))?;
        let key = canonical(&parsed);
        validate_key(&key)?;
        if let Some(other) = ActionId::ALL.iter().find(|other| **other != id && same_key(self.key(**other), &key)) {
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

/// A shortcut as the palette and the documented map show it: modifier glyphs,
/// then the key capitalised (`⌘⇧T`, `⌃Tab`). One formatter so a shortcut shown
/// anywhere is the keystroke the binding actually uses.
pub fn shortcut_label(keystroke: &str) -> String {
    let display = display_key(keystroke);
    let key_start = display.rfind(['⌘', '⌥', '⌃', '⇧']).map_or(0, |i| i + '⌘'.len_utf8());
    let (modifiers, key) = display.split_at(key_start);
    let mut chars = key.chars();
    let key = match chars.next() {
        Some(first) if first.is_ascii_alphabetic() => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        _ => key.to_string(),
    };
    format!("{modifiers}{key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_setting_skips_reserved_and_conflicting_overrides() {
        let map = Keymap::from_setting(Some("new-tab=secondary-q;close-tab=secondary-shift-9;reload=secondary-shift-9"));
        assert_eq!(map.key(ActionId::NewTab), ActionId::NewTab.default_key(), "⌘Q stays reserved for Quit");
        assert!(map.key(ActionId::CloseTab).ends_with("shift-9"), "{}", map.key(ActionId::CloseTab));
        assert_ne!(map.key(ActionId::CloseTab), ActionId::CloseTab.default_key());
        assert_eq!(map.key(ActionId::Reload), ActionId::Reload.default_key(), "conflicting key is skipped");
    }

    #[test]
    fn shared_action_file_matches_the_keymap() {
        let raw = include_str!("../../../.agents/keymap-actions.txt");
        let mut seen = std::collections::HashSet::new();
        for line in raw.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()) {
            let (id, key) = line.split_once('|').expect("id|key");
            let id = ActionId::parse(id).unwrap_or_else(|| panic!("unknown action {id}"));
            assert_eq!(id.default_key(), key, "{id:?}");
            assert!(seen.insert(id), "the shared table lists {id:?} twice");
        }
        assert_eq!(seen.len(), ActionId::ALL.len(), "every table action is in .agents/keymap-actions.txt");
    }

    #[test]
    fn action_table_rows_are_unique_and_complete() {
        let mut ids = std::collections::HashSet::new();
        let mut labels = std::collections::HashSet::new();
        for id in ActionId::ALL {
            assert!(!id.as_str().is_empty(), "{id:?}");
            assert!(!id.label().is_empty(), "{id:?}");
            assert!(Keystroke::parse(id.default_key()).is_ok(), "{id:?} has {:?}", id.default_key());
            assert!(ids.insert(id.as_str()), "duplicate action id {}", id.as_str());
            assert!(labels.insert(id.label()), "duplicate Settings label {}", id.label());
        }
        assert_eq!(ActionId::ALL.len(), 61);
    }

    #[test]
    fn defaults_cover_every_action_once() {
        let map = Keymap::default();
        let keys: std::collections::HashSet<&str> = ActionId::ALL.iter().map(|id| map.key(*id)).collect();
        assert_eq!(keys.len(), ActionId::ALL.len(), "default shortcuts are unique");
        assert_eq!(map.bindings().len(), ActionId::ALL.len());
        for id in ActionId::ALL {
            assert!(Keystroke::parse(map.key(*id)).is_ok(), "{}", map.key(*id));
        }
    }

    #[test]
    fn key_bindings_are_built_from_the_action_table() {
        use gpui::AsKeystroke;

        let bindings = Keymap::default().bindings();
        assert_eq!(bindings.len(), ActionId::ALL.len(), "one binding per table row");
        for (id, binding) in ActionId::ALL.iter().zip(&bindings) {
            assert_eq!(binding.action().name(), id.action_name(), "binding order follows the table");
            let expected = Keystroke::parse(id.default_key()).unwrap();
            let actual = binding.keystrokes()[0].as_keystroke();
            assert_eq!(actual, &expected, "{:?} is bound to {:?} but the table says {:?}", id, actual, id.default_key());
        }
    }

    #[test]
    fn shortcut_labels_are_the_documented_form() {
        assert_eq!(shortcut_label("secondary-t"), "⌘T");
        assert_eq!(shortcut_label("secondary-shift-s"), "⌘⇧S");
        assert_eq!(shortcut_label("secondary-alt-i"), "⌘⌥I");
        assert_eq!(shortcut_label("ctrl-tab"), "⌃Tab");
        assert_eq!(shortcut_label("secondary--"), "⌘-");
        for id in ActionId::ALL {
            let label = shortcut_label(id.default_key());
            assert!(label.starts_with(['⌘', '⌃', '⌥', '⇧']), "{id:?}: {label}");
        }
    }

    #[test]
    fn design_keyboard_map_matches_the_action_table() {
        const START: &str = "<!-- keymap:start -->";
        const END: &str = "<!-- keymap:end -->";
        let doc = include_str!("../../../docs/design.md");
        let block = doc
            .split_once(START)
            .and_then(|(_, rest)| rest.split_once(END))
            .map(|(block, _)| block)
            .expect("docs/design.md has a keymap block between the keymap markers");
        let actual: Vec<&str> = block.lines().map(str::trim_end).filter(|line| !line.is_empty()).collect();

        let mut expected = vec!["| Keys | Action |".to_string(), "|---|---|".to_string()];
        expected.extend(ActionId::ALL.iter().map(|id| {
            format!("| {} | {} |", shortcut_label(id.default_key()), id.label())
        }));
        assert_eq!(
            actual,
            expected,
            "docs/design.md keyboard map drifted from the action table; copy the lines above"
        );
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
    fn every_action_has_one_settings_group() {
        let mut seen = std::collections::HashSet::new();
        for id in ActionId::ALL {
            assert!(SHORTCUT_GROUPS.contains(&id.group()), "{id:?} has group {:?}", id.group());
            seen.insert(id.group());
        }
        assert_eq!(seen.len(), SHORTCUT_GROUPS.len(), "a heading would be empty");
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
