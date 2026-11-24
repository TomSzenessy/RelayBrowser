//! Native macOS menu bar and global key-binding entry points.

use super::*;

/// Every global shortcut. `secondary` is ⌘ on macOS and Ctrl elsewhere.
///
/// These bindings are also what the native menu bar shows as key
/// equivalents: on macOS the menu receives them before the focused
/// WKWebView, which is why shortcuts work while a page has focus.
pub fn key_bindings() -> Vec<KeyBinding> {
    crate::keymap::Keymap::default().bindings()
}

pub fn app_menus() -> Vec<Menu> {
    app_menus_named(None)
}

pub(super) fn app_menus_named(app_name: Option<&str>) -> Vec<Menu> {
    fn item(name: &'static str, action: impl gpui::Action) -> MenuItem {
        MenuItem::action(name, action)
    }
    let sep = MenuItem::separator;
    vec![
        Menu {
            name: app_name.map(|s| SharedString::from(s.to_string())).unwrap_or_else(|| "Relay".into()),
            items: vec![
                item("Settings…", ToggleSettings),
                item("Pause / Resume Agents", ToggleAgents),
                item("Cycle Agent Access on This Site", CycleAgentPolicy),
                sep(),
                item("Quit Relay", Quit),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                item("Command Palette…", OpenPalette),
                item("Open Location…", OpenLocation),
                sep(),
                item("New Tab", NewTab),
                item("New Private Tab", NewPrivateTab),
                item("Close Tab", CloseTab),
                item("Reopen Closed Tab", ReopenClosedTab),
                sep(),
                item("Print…", Print),
                item("Export as PDF…", ExportPdf),
                sep(),
                item("Install This Site as an App…", InstallWebApp),
                item("Remove This Site's App", RemoveWebApp),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                item("Undo", Undo),
                item("Redo", Redo),
                sep(),
                item("Cut", Cut),
                item("Copy", Copy),
                item("Paste", Paste),
                item("Select All", SelectAll),
                sep(),
                item("Find in Page…", Find),
                item("Find Next", FindNext),
                item("Find Previous", FindPrevious),
            ],
        },
        Menu {
            name: "View".into(),
            items: vec![
                item("Fold / Unfold Tab Strip", ToggleTabStrip),
                item("Move Tab Strip", NextTabStripPosition),
                sep(),
                item("Tabs on the Left", TabsOnLeft),
                item("Tabs on Top", TabsOnTop),
                item("Tabs on the Right", TabsOnRight),
                item("Fold All Tab Groups", FoldAllGroups),
                item("Unfold All Tab Groups", UnfoldAllGroups),
                item("Spaces…", Spaces),
                sep(),
                item("Show / Hide Bottom Bar", ToggleBottomBar),
                sep(),
                item("Reading Mode", ToggleReader),
                item("Picture in Picture", TogglePictureInPicture),
                item("Float Video", ToggleFloatVideo),
                sep(),
                item("Downloads…", ShowDownloads),
                item("Reload Page", Reload),
                item("Zoom In", ZoomIn),
                item("Zoom Out", ZoomOut),
                item("Actual Size", ZoomReset),
                sep(),
                item("Toggle Inspector", ToggleInspector),
                item("WebKit Web Inspector", WebInspector),
                item("Toggle Split View", ToggleSplit),
            ],
        },
        Menu {
            name: "History".into(),
            items: vec![item("Back", Back), item("Forward", Forward), sep(), item("Show All History…", ShowHistory)],
        },
        Menu {
            name: "Bookmarks".into(),
            items: vec![
                item("Bookmark This Page", ToggleBookmark),
                item("Pin / Unpin Tab", TogglePin),
                sep(),
                item("Show All Bookmarks…", ShowBookmarks),
                sep(),
                item("Copy Clean URL", CopyCleanUrl),
                item("Copy Page as Markdown", CopyMarkdown),
            ],
        },
        Menu {
            name: "Tab".into(),
            items: vec![
                item("Next Tab", NextTab),
                item("Previous Tab", PrevTab),
                item("Link Hints", LinkHints),
                sep(),
                item("Select Tab 1", SelectTab1),
                item("Select Tab 2", SelectTab2),
                item("Select Tab 3", SelectTab3),
                item("Select Tab 4", SelectTab4),
                item("Select Tab 5", SelectTab5),
                item("Select Tab 6", SelectTab6),
                item("Select Tab 7", SelectTab7),
                item("Select Tab 8", SelectTab8),
                item("Select Tab 9", SelectTab9),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![item("Minimize", Minimize)],
        },
        Menu {
            name: "Help".into(),
            items: vec![item("Relay on GitHub", Help)],
        },
    ]
}
