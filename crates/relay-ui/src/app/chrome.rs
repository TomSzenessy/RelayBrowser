//! Primary shell chrome: tab strip, folded rail, panes, start/welcome views, banners, and status bar.

use super::*;

/// Deterministic letter-monogram fallback badge when a host has no cached favicon.
fn monogram_icon(url: &str, title: &str, size_px: f32) -> gpui::AnyElement {
    // Dark enough for white 9-10 px text at WCAG AA (4.5:1).
    const PALETTE: [u32; 8] = [
        0x1e3a8a, // indigo
        0x4c1d95, // violet
        0x065f46, // emerald
        0x1e40af, // blue
        0x7f1d1d, // red
        0x7c2d12, // orange
        0x831843, // rose
        0x14532d, // green
    ];
    let host = host_of(url).unwrap_or_default();
    let clean_host = host.strip_prefix("www.").unwrap_or(&host);
    let initial = clean_host
        .chars()
        .find(|c| c.is_ascii_alphanumeric())
        .or_else(|| title.chars().find(|c| c.is_alphanumeric()))
        .unwrap_or('·')
        .to_ascii_uppercase()
        .to_string();
    let hash = clean_host.bytes().fold(0usize, |acc, b| acc.wrapping_mul(31).wrapping_add(b as usize));
    let bg = gpui::rgb(PALETTE[hash % PALETTE.len()]);
    div()
        .size(px(size_px))
        .flex_shrink_0()
        .rounded(px((size_px * 0.25).max(3.)))
        .bg(bg)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px((size_px * 0.62).max(9.)))
        .text_color(gpui::rgb(0xffffff))
        .child(SharedString::from(initial))
        .into_any_element()
}

/// Apple-style geometric sidebar toggle icon (`sidebar.left`, `sidebar.right`,
/// or horizontal top/bottom variant) matching macOS native panel chrome.
pub(super) fn sidebar_icon(edge: tabstrip::Placement, active: bool, t: Theme) -> gpui::AnyElement {
    let fg = if active { t.text } else { t.muted };
    let bar = if active { t.text } else { t.muted.opacity(0.65) };
    let frame = div()
        .w(px(15.))
        .h(px(12.))
        .rounded(px(3.))
        .border_1()
        .border_color(fg)
        .overflow_hidden()
        .flex();
    match edge {
        tabstrip::Placement::Left => frame
            .flex_row()
            .child(div().w(px(4.5)).h_full().bg(bar))
            .child(div().flex_1().h_full())
            .into_any_element(),
        tabstrip::Placement::Right => frame
            .flex_row()
            .child(div().flex_1().h_full())
            .child(div().w(px(4.5)).h_full().bg(bar))
            .into_any_element(),
        tabstrip::Placement::Top => frame
            .flex_col()
            .child(div().w_full().h(px(4.)).bg(bar))
            .child(div().flex_1().w_full())
            .into_any_element(),
        tabstrip::Placement::Bottom => frame
            .flex_col()
            .child(div().flex_1().w_full())
            .child(div().w_full().h(px(4.)).bg(bar))
            .into_any_element(),
    }
}

impl RelayApp {
    pub(super) fn shortcut_for(&self, action: ActionId) -> SharedString {
        SharedString::from(keymap::shortcut_label(self.keymap.key(action)))
    }

    pub(super) fn site_icon(&self, url: &str, title: &str, size_px: f32) -> gpui::AnyElement {
        match host_of(url).and_then(|host| self.favicons.get(&host)).and_then(|icon| icon.clone()) {
            Some(icon) => img(icon).size(px(size_px)).flex_shrink_0().rounded(px(3.)).into_any_element(),
            None => monogram_icon(url, title, size_px),
        }
    }

    fn nav_icon_button(
        &self,
        id: &'static str,
        label: &'static str,
        tooltip_label: &'static str,
        action: Option<ActionId>,
        active: bool,
        cmd: Command,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = self.theme;
        let shortcut = action.map(|a| self.shortcut_for(a)).unwrap_or_default();
        let tip: SharedString = tooltip_label.into();
        div()
            .id(id)
            .w(px(24.))
            .h(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .text_size(px(13.))
            .text_color(if active { t.accent } else { t.muted })
            .cursor_pointer()
            .hover(|s| s.bg(t.hover).text_color(t.text))
            .tooltip(move |_, cx| {
                let label = tip.clone();
                let sc = shortcut.clone();
                cx.new(|_| ShortcutTooltip { label, shortcut: sc, theme: t }).into()
            })
            .on_click(cx.listener(move |app, _, _, cx| {
                app.run_command(cmd, cx);
                cx.notify();
            }))
            .child(label)
            .into_any_element()
    }

    /// Current-space pill; opens the space prompt.
    fn space_chip(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let dark = t.background.l < 0.5;
        let look = self.current_space_look();
        let shortcut = self.shortcut_for(ActionId::Spaces);
        let (name, count) = {
            let h = self.hub.lock();
            let name = h.current_space().to_string();
            (if name.is_empty() { "Default".to_string() } else { name }, h.space_tab_ids().len())
        };
        let dot = look.accent_dot(dark);
        div()
            .id("space-chip")
            .h(px(22.))
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .rounded(px(5.))
            .bg(t.well)
            .text_size(px(10.))
            .text_color(t.text)
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .tooltip(move |_, cx| {
                let sc = shortcut.clone();
                cx.new(|_| ShortcutTooltip { label: "Spaces".into(), shortcut: sc, theme: t }).into()
            })
            .on_click(cx.listener(|app, _, _, cx| app.open_space_prompt(cx)))
            .when_some(dot, |d, c| d.child(div().size(px(6.)).rounded_full().bg(c)))
            .child(look.icon.glyph())
            .when(!(self.tab_icons_only && self.strip.placement.is_vertical()), |d| {
                d.child(SharedString::from(name)).child(div().text_color(t.muted).child(SharedString::from(count.to_string())))
            })
            .into_any_element()
    }

    pub(super) fn render_tab_bar(&self, stacked: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let strip = self.strip;
        let (active, user, agents, shortcut_map) = {
            let h = self.hub.lock();
            let space = h.current_space().to_string();
            let active = h.active().filter(|id| h.tab(*id).is_some_and(|t| t.space == space));
            let mut user = vec![];
            let mut agents = vec![];
            for view in h.tab_views().into_iter().filter(|v| v.space == space) {
                if view.owner == Owner::Agent { agents.push(view) } else { user.push(view) }
            }
            let ids = h.space_tab_ids();
            let mut map: HashMap<TabId, usize> = HashMap::new();
            for (idx, id) in ids.iter().take(8).enumerate() {
                map.insert(*id, idx + 1);
            }
            if ids.len() >= 9 {
                if let Some(&last) = ids.last() {
                    map.insert(last, 9);
                }
            }
            (active, user, agents, map)
        };
        let bookmarked = self.active_is_bookmarked();
        let show_nums = self.cmd_held && self.show_tab_numbers;
        let badge_for = |id: TabId| -> Option<usize> {
            show_nums.then(|| shortcut_map.get(&id).copied()).flatten()
        };
        let pinned: Vec<&TabView> = user.iter().filter(|v| v.pinned).collect();
        let loose: Vec<&TabView> = user.iter().filter(|v| !v.pinned).collect();
        let mut ungrouped: Vec<&TabView> = Vec::new();
        let mut groups: Vec<(String, Vec<&TabView>)> = Vec::new();
        for tab in &loose {
            match &tab.group {
                None => ungrouped.push(tab),
                Some(name) => match groups.iter_mut().find(|(n, _)| n == name) {
                    Some((_, rows)) => rows.push(tab),
                    None => groups.push((name.clone(), vec![tab])),
                },
            }
        }
        let vertical = strip.placement.is_vertical();
        let icons_only = vertical && self.tab_icons_only && !stacked;
        let drag_grip = div()
            .id("tabs-drag-handle")
            .px_1()
            .h(px(22.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .text_size(px(10.))
            .text_color(t.muted)
            .cursor_grab()
            .hover(|s| s.bg(t.hover).text_color(t.text))
            .tooltip(move |_, cx| {
                cx.new(|_| ShortcutTooltip {
                    label: "Drag to dock Tabs on any edge".into(),
                    shortcut: "⌘⇧S".into(),
                    theme: t,
                })
                .into()
            })
            .on_drag(PanelDrag(PanelKind::Tabs), move |_, _, _, cx| {
                cx.new(|_| PanelDragGhost { label: "Tabs".into(), theme: t })
            })
            .child("⋮⋮");

        let mut bar = div().flex();
        if vertical {
            let is_left = strip.placement == tabstrip::Placement::Left;
            bar = bar
                .flex_col()
                .gap_1()
                .px_2()
                .pb_2()
                .pt(if stacked { px(4.) } else if is_left { px(34.) } else { px(8.) })
                .when(stacked, |d| d.w_full().flex_1().min_h_0())
                .when(!stacked, |d| d.w(px(if icons_only { 54. } else { strip.width() })).h_full());
            if icons_only {
                bar = bar.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .gap_0p5()
                        .pb_1()
                        .child(self.nav_icon_button("nav-reload", "↻", "Reload", Some(ActionId::Reload), false, Command::Reload, cx))
                        .child(self.fold_button(strip.placement, cx)),
                );
                bar = bar.child(self.space_chip(cx));
            } else {
                // Draggable navigation + bookmark + Apple sidebar toggle header row.
                let mut header_left = div()
                    .flex()
                    .items_center()
                    .gap_0p5()
                    .child(drag_grip)
                    .child(self.nav_icon_button("nav-back", "←", "Back", Some(ActionId::Back), false, Command::Back, cx))
                    .child(self.nav_icon_button("nav-forward", "→", "Forward", Some(ActionId::Forward), false, Command::Forward, cx))
                    .child(self.nav_icon_button("nav-reload", "↻", "Reload", Some(ActionId::Reload), false, Command::Reload, cx));
                if !is_left {
                    header_left = div()
                        .flex()
                        .items_center()
                        .gap_0p5()
                        .child(self.fold_button(strip.placement, cx))
                        .child(header_left);
                }
                let mut header_right = div()
                    .flex()
                    .items_center()
                    .gap_0p5()
                    .child(self.nav_icon_button(
                        "nav-bookmark",
                        if bookmarked { "★" } else { "☆" },
                        if bookmarked { "Remove Bookmark" } else { "Bookmark Page" },
                        Some(ActionId::ToggleBookmark),
                        bookmarked,
                        Command::ToggleBookmark,
                        cx,
                    ));
                if is_left {
                    header_right = header_right.child(self.fold_button(strip.placement, cx));
                }
                bar = bar.child(
                    div()
                        .id("tabs-header-bar")
                        .flex()
                        .items_center()
                        .justify_between()
                        .pb_1()
                        .on_drag(PanelDrag(PanelKind::Tabs), move |_, _, _, cx| {
                            cx.new(|_| PanelDragGhost { label: "Tabs".into(), theme: t })
                        })
                        .child(header_left)
                        .child(header_right),
                );
                bar = bar.child(self.space_chip(cx));
            }
            // Arc-style Favorites grid for pinned tabs.
            if !pinned.is_empty() {
                let mut grid = div().flex().flex_wrap().gap_1p5().pb_1();
                for tab in &pinned {
                    grid = grid.child(self.pin_tile(tab, active, badge_for(tab.id), cx));
                }
                bar = bar.child(grid);
                if let Some(menu_id) = self.tab_menu.filter(|id| pinned.iter().any(|p| p.id == *id)) {
                    bar = bar.child(self.tab_context_menu(menu_id, true, cx));
                }
            }
            // Subtle divider + Clear button between pinned favorites and loose tabs.
            if !pinned.is_empty() || !loose.is_empty() {
                bar = bar.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_1()
                        .py_0p5()
                        .child(div().flex_1().h(px(1.)).bg(t.border))
                        .when(!loose.is_empty(), |d| {
                            d.child(
                                div()
                                    .id("clear-loose-tabs")
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(4.))
                                    .text_size(px(10.))
                                    .text_color(t.muted)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(t.hover).text_color(t.text))
                                    .tooltip(move |_, cx| {
                                        cx.new(|_| ShortcutTooltip {
                                            label: "Close unpinned tabs".into(),
                                            shortcut: "".into(),
                                            theme: t,
                                        })
                                        .into()
                                    })
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.clear_loose_tabs(cx);
                                    }))
                                    .child("Clear"),
                            )
                        }),
                );
            }
            let mut list = div().id("tab-list-scroll").flex_1().min_h_0().flex().flex_col().gap_0p5().overflow_y_scroll();
            for tab in &ungrouped {
                list = list.child(self.tab_row(tab, active, true, badge_for(tab.id), cx));
            }
            for (name, rows) in &groups {
                if icons_only {
                    for tab in rows {
                        list = list.child(self.tab_row(tab, active, true, badge_for(tab.id), cx));
                    }
                } else {
                    list = list.child(self.group_header(name, rows.len(), true, cx));
                    if !self.collapsed_groups.contains(name) {
                        for tab in rows {
                            list = list.child(self.tab_row(tab, active, true, badge_for(tab.id), cx));
                        }
                    }
                }
            }
            if !agents.is_empty() {
                list = list.child(div().mt_2().px_2().pb_1().text_size(px(10.)).text_color(t.agent).child("AGENT"));
                for tab in &agents {
                    list = list.child(self.tab_row(tab, active, true, badge_for(tab.id), cx));
                }
            }
            bar.child(list).child(self.new_tab_button(true, cx)).into_any_element()
        } else {
            let is_top = strip.placement == tabstrip::Placement::Top;
            let mut bar = div()
                .id("tabs-header-horizontal")
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .pl(if stacked { px(4.) } else if is_top { px(78.) } else { px(10.) })
                .pr_2()
                .when(stacked, |d| d.flex_1().min_w_0().h_full())
                .when(!stacked, |d| d.h(px(strip.height())).w_full())
                .on_drag(PanelDrag(PanelKind::Tabs), move |_, _, _, cx| {
                    cx.new(|_| PanelDragGhost { label: "Tabs".into(), theme: t })
                })
                .child(drag_grip)
                .child(self.nav_icon_button("nav-back", "←", "Back", Some(ActionId::Back), false, Command::Back, cx))
                .child(self.nav_icon_button("nav-forward", "→", "Forward", Some(ActionId::Forward), false, Command::Forward, cx))
                .child(self.nav_icon_button("nav-reload", "↻", "Reload", Some(ActionId::Reload), false, Command::Reload, cx));
            bar = bar.child(self.space_chip(cx));
            for tab in &pinned {
                bar = bar.child(self.tab_row(tab, active, false, badge_for(tab.id), cx));
            }
            for tab in &ungrouped {
                bar = bar.child(self.tab_row(tab, active, false, badge_for(tab.id), cx));
            }
            for (name, rows) in &groups {
                bar = bar.child(self.group_header(name, rows.len(), false, cx));
                if !self.collapsed_groups.contains(name) {
                    for tab in rows {
                        bar = bar.child(self.tab_row(tab, active, false, badge_for(tab.id), cx));
                    }
                }
            }
            if !agents.is_empty() {
                bar = bar.child(div().px_1().text_size(px(10.)).text_color(t.agent).child("AGENT"));
                for tab in &agents {
                    bar = bar.child(self.tab_row(tab, active, false, badge_for(tab.id), cx));
                }
            }
            bar.child(self.new_tab_button(false, cx))
                .child(div().flex_1())
                .child(self.nav_icon_button(
                    "nav-bookmark",
                    if bookmarked { "★" } else { "☆" },
                    if bookmarked { "Remove Bookmark" } else { "Bookmark Page" },
                    Some(ActionId::ToggleBookmark),
                    bookmarked,
                    Command::ToggleBookmark,
                    cx,
                ))
                .child(self.fold_button(strip.placement, cx))
                .into_any_element()
        }
    }

    /// Compact Arc-style Favorites tile for a pinned tab in the vertical sidebar.
    fn pin_tile(&self, tab: &TabView, active: Option<TabId>, shortcut_num: Option<usize>, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let id = tab.id;
        let is_active = active == Some(id);
        div()
            .id(("pin-tile", id as usize))
            .relative()
            .w(px(45.))
            .h(px(if self.compact_tabs { 30. } else { 34. }))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .bg(if is_active { t.active } else { t.raised })
            .when(is_active, |d| d.shadow_sm())
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .on_click(cx.listener(move |app, _, _, cx| {
                app.activate(id);
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Middle, cx.listener(move |app, _, _, cx| {
                app.close_tab(id, cx);
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |app, _, _, cx| {
                app.tab_menu = if app.tab_menu == Some(id) { None } else { Some(id) };
                cx.notify();
            }))
            .child(if self.tab_glyphs { self.site_icon(&tab.url, &tab.title, 16.) } else { monogram_icon(&tab.url, &tab.title, 16.) })
            .when(tab.playing_audio, |d| {
                d.child(div().absolute().bottom(px(2.)).right(px(3.)).text_size(px(9.)).text_color(t.accent).child("♪"))
            })
            .when_some(shortcut_num, |d, n| {
                d.child(
                    div()
                        .absolute()
                        .top(px(2.))
                        .left(px(3.))
                        .px_1()
                        .rounded(px(3.))
                        .bg(t.accent)
                        .text_size(px(9.))
                        .line_height(px(11.))
                        .text_color(t.on_accent)
                        .child(SharedString::from(n.to_string())),
                )
            })
            .into_any_element()
    }

    fn tab_context_menu(&self, id: TabId, pinned: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let grouped = self.hub.lock().tab(id).and_then(|tab| tab.group.clone());
        let item = |suffix: &'static str, label: &'static str, danger: bool| {
            div()
                .id(SharedString::from(format!("tab-menu-{id}-{suffix}")))
                .px_2()
                .py_1()
                .rounded(px(4.))
                .text_size(px(11.))
                .text_color(if danger { t.error } else { t.text })
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .child(label)
        };
        div()
            .id(("tab-ctx-menu", id as usize))
            .mx_1()
            .my_0p5()
            .p_1()
            .rounded(px(6.))
            .bg(t.raised)
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_0p5()
            .child(item("group", "Move to Group…", false).on_click(cx.listener(move |app, _, _, cx| {
                app.open_group_prompt(id, cx);
                cx.notify();
            })))
            .when(grouped.is_some(), |d| {
                d.child(item("ungroup", "Remove from Group", false).on_click(cx.listener(move |app, _, _, cx| {
                    app.remove_tab_from_group(id, cx);
                })))
            })
            .child(item("pin", if pinned { "Unpin Tab" } else { "Pin to Favorites" }, false).on_click(cx.listener(move |app, _, _, cx| {
                app.toggle_pin_for(id);
                cx.notify();
            })))
            .child(item("dup", "Duplicate Tab", false).on_click(cx.listener(move |app, _, _, cx| {
                app.duplicate_tab(id);
                cx.notify();
            })))
            .child(item("sleep", "Sleep Tab", false).on_click(cx.listener(move |app, _, _, cx| {
                app.sleep_tab_now(id, cx);
            })))
            .child(item("mute", "Mute / Unmute Tab", false).on_click(cx.listener(move |app, _, _, cx| {
                app.toggle_mute(id);
                cx.notify();
            })))
            .child(item("others", "Close Other Tabs", false).on_click(cx.listener(move |app, _, _, cx| {
                app.close_other_tabs(id, cx);
                cx.notify();
            })))
            .child(item("close", "Close Tab", true).on_click(cx.listener(move |app, _, _, cx| {
                app.close_tab(id, cx);
                cx.notify();
            })))
            .into_any_element()
    }

    /// A right-clicked tab in the horizontal strip has no column to grow in;
    /// paint its menu as a floating panel over the page instead.
    pub(super) fn render_floating_tab_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let t = self.theme;
        let id = self.tab_menu?;
        let pinned = self.hub.lock().tab(id)?.pinned;
        Some(
            div()
                .id("floating-tab-menu")
                .absolute()
                .top(px(42.))
                .left(px(82.))
                .w(px(220.))
                .rounded(px(9.))
                .bg(t.raised)
                .shadow_lg()
                .on_mouse_down_out(cx.listener(|app, _, _, cx| {
                    app.tab_menu = None;
                    cx.notify();
                }))
                .child(self.tab_context_menu(id, pinned, cx))
                .into_any_element(),
        )
    }

    /// A heading for one named group: chevron, colour dot, tinted heading, name, and count.
    /// Left click folds/unfolds; right click opens the group name & colour prompt.
    fn group_header(&self, name: &str, count: usize, vertical: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let dark = t.background.l < 0.5;
        let collapsed = self.collapsed_groups.contains(name);
        let color = resolve_group_color(&self.group_colors, name);
        let owned = name.to_string();
        let edit_name = name.to_string();
        div()
            .id(SharedString::from(format!("group-{name}")))
            .flex()
            .items_center()
            .gap_1p5()
            .h(px(24.))
            .px_2()
            .rounded(px(5.))
            .bg(color.tint(dark))
            .when(!vertical, |d| d.max_w(px(190.)))
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .on_click(cx.listener(move |app, _, _, cx| {
                app.toggle_group_collapsed(&owned, cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |app, _, _, cx| {
                app.open_group_edit_prompt(&edit_name, cx);
            }))
            .child(div().text_size(px(10.)).text_color(t.muted).child(if collapsed { "▸" } else { "▾" }))
            .child(div().size(px(7.)).rounded_full().bg(color.dot(dark)))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(11.)).text_color(t.text).child(SharedString::from(name.to_string())))
            .child(div().text_size(px(10.)).text_color(t.muted).child(SharedString::from(count.to_string())))
            .into_any_element()
    }

    fn tab_row(
        &self,
        tab: &TabView,
        active: Option<TabId>,
        vertical: bool,
        shortcut_num: Option<usize>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let t = self.theme;
        let id = tab.id;
        let is_active = active == Some(id);
        let agent = tab.owner == Owner::Agent;
        let title: SharedString = if tab.title.is_empty() { palette::short_url(&tab.url).into() } else { tab.title.clone().into() };
        let show_close = is_active || self.hovered_tab == Some(id) || self.always_show_close;
        let icons_only = vertical && self.tab_icons_only;
        let mut row = div()
            .id(("tab", id as usize))
            .flex()
            .items_center()
            .when(icons_only, |d| d.justify_center().px_0())
            .gap_1p5()
            .h(px(if self.compact_tabs { 24. } else { 28. }))
            .px_2()
            .rounded(px(6.))
            .text_size(px(if self.compact_tabs { 12. } else { 13. }))
            .cursor_pointer()
            .text_color(if tab.sleeping { t.muted } else { t.text })
            .hover(|s| s.bg(t.hover))
            .on_hover(cx.listener(move |app, hovered: &bool, _, cx| {
                let next = hovered.then_some(id);
                if app.hovered_tab != next {
                    app.hovered_tab = next;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |app, _, _, cx| {
                app.activate(id);
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Middle, cx.listener(move |app, _, _, cx| {
                app.close_tab(id, cx);
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |app, _, _, cx| {
                app.tab_menu = if app.tab_menu == Some(id) { None } else { Some(id) };
                cx.notify();
            }));
        if let Some(n) = shortcut_num {
            row = row.child(
                div()
                    .size(px(15.))
                    .flex_shrink_0()
                    .rounded(px(4.))
                    .bg(t.accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(10.))
                    .text_color(t.on_accent)
                    .child(SharedString::from(n.to_string())),
            );
        } else if self.tab_glyphs {
            row = row.child(self.site_icon(&tab.url, &tab.title, if icons_only { 17. } else { 15. }));
        } else {
            row = row.child(monogram_icon(&tab.url, &tab.title, if icons_only { 17. } else { 15. }));
        }
        if !icons_only {
            let title_el = if vertical {
                div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(title)
            } else {
                div().max_w(px(160.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(title)
            };
            row = row.child(title_el);
        }
        if agent {
            row = row.bg(t.agent_bg);
        }
        if is_active {
            row = row.bg(t.active);
        }
        if !icons_only && tab.private {
            row = row.child(
                div()
                    .px_1()
                    .rounded(px(3.))
                    .bg(t.agent_bg)
                    .text_size(px(9.))
                    .text_color(t.agent)
                    .child("Private"),
            );
        }
        if !icons_only {
            if tab.challenge.is_some() {
                row = row.child(div().text_color(t.warn).child("⚠"));
            }
            if tab.playing_audio {
                row = row.child(div().text_color(t.accent).text_size(px(11.)).child("♪"));
            }
            if tab.pinned && !vertical {
                row = row.child(div().text_color(t.muted).text_size(px(10.)).child("•"));
            }
            if tab.sleeping {
                row = row.child(div().text_color(t.muted).text_size(px(10.)).child("zz"));
            }
        }
        if icons_only {
            return row.into_any_element();
        }
        let row = row.child(
            div()
                .id(("close-tab", id as usize))
                .w(px(18.))
                .h(px(18.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .text_size(px(14.))
                .line_height(px(14.))
                .text_color(t.muted)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover).text_color(t.text))
                .when(!show_close, |d| d.invisible())
                .on_click(cx.listener(move |app, _, _, cx| {
                    cx.stop_propagation();
                    app.close_tab(id, cx);
                    cx.notify();
                }))
                .child("×"),
        );
        if vertical && self.tab_menu == Some(id) {
            div().flex().flex_col().child(row).child(self.tab_context_menu(id, tab.pinned, cx)).into_any_element()
        } else {
            row.into_any_element()
        }
    }

    fn new_tab_button(&self, vertical: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let shortcut = self.shortcut_for(ActionId::NewTab);
        let button = div()
            .id("new-tab")
            .flex()
            .items_center()
            .rounded(px(6.))
            .text_size(px(12.))
            .text_color(t.muted)
            .cursor_pointer()
            .hover(|s| s.bg(t.hover).text_color(t.text))
            .tooltip(move |_, cx| {
                let sc = shortcut.clone();
                cx.new(|_| ShortcutTooltip { label: "New Tab".into(), shortcut: sc, theme: t }).into()
            })
            .on_click(cx.listener(|app, _, _, cx| {
                app.open_palette(None, true);
                cx.notify();
            }));
        if vertical && self.tab_icons_only {
            button.w(px(24.)).h(px(24.)).justify_center().child("+").into_any_element()
        } else if vertical {
            let sc = self.shortcut_for(ActionId::NewTab);
            button.px_2().h(px(28.)).justify_between().child("+ New tab").child(div().text_size(px(10.)).child(sc)).into_any_element()
        } else {
            button.w(px(24.)).h(px(24.)).justify_center().child("+").into_any_element()
        }
    }

    fn fold_button(&self, edge: tabstrip::Placement, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let shortcut = self.shortcut_for(ActionId::ToggleTabStrip);
        div()
            .id("fold-tab-strip")
            .w(px(24.))
            .h(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .tooltip(move |_, cx| {
                let sc = shortcut.clone();
                cx.new(|_| ShortcutTooltip { label: "Hide Tab Strip".into(), shortcut: sc, theme: t }).into()
            })
            .on_click(cx.listener(|app, _, _, cx| {
                app.toggle_tab_strip(cx);
            }))
            .child(sidebar_icon(edge, true, t))
            .into_any_element()
    }

    /// Edge-matched Apple-style sidebar reveal buttons whenever the Tab Strip
    /// or Agent panel is closed. Each toggle sits on its own panel's edge in
    /// the top shell strip (above the rounded browser card), never crossing
    /// sides and never overlapping WKWebView.
    pub(super) fn render_corner_toggles(&self, show_strip: bool, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let t = self.theme;
        let need_tabs = show_strip && self.strip.folded;
        let need_agent = !self.inspector_open;
        if !need_tabs && !need_agent {
            return vec![];
        }
        let mut left_group: Vec<gpui::AnyElement> = Vec::new();
        let mut right_group: Vec<gpui::AnyElement> = Vec::new();

        if need_tabs {
            let sc = self.shortcut_for(ActionId::ToggleTabStrip);
            let btn = div()
                .id("unfold-tab-strip")
                .w(px(26.))
                .h(px(22.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .tooltip(move |_, cx| {
                    let sc = sc.clone();
                    cx.new(|_| ShortcutTooltip { label: "Show Tab Strip".into(), shortcut: sc, theme: t }).into()
                })
                .on_click(cx.listener(|app, _, _, cx| {
                    app.toggle_tab_strip(cx);
                }))
                .child(sidebar_icon(self.strip.placement, false, t))
                .into_any_element();
            if self.strip.placement == tabstrip::Placement::Right {
                right_group.push(btn);
            } else {
                left_group.push(btn);
            }
        }

        if need_agent {
            let sc = self.shortcut_for(ActionId::AskAi);
            let has_agent_tabs = self.hub.lock().tabs().any(|tab| tab.owner == Owner::Agent);
            let paused = self.hub.agents_paused();
            let btn = div()
                .id("reveal-agent-panel")
                .relative()
                .w(px(26.))
                .h(px(22.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .tooltip(move |_, cx| {
                    let sc = sc.clone();
                    cx.new(|_| ShortcutTooltip { label: "Show Agent".into(), shortcut: sc, theme: t }).into()
                })
                .on_click(cx.listener(|app, _, _, cx| {
                    app.toggle_agent_sidebar(cx);
                }))
                .child(sidebar_icon(self.agent_edge, false, t))
                .when(paused || has_agent_tabs, |d| {
                    d.child(
                        div()
                            .absolute()
                            .top(px(2.))
                            .right(px(2.))
                            .size(px(5.))
                            .rounded_full()
                            .bg(if paused { t.warn } else { t.agent }),
                    )
                })
                .into_any_element();
            if self.agent_edge == tabstrip::Placement::Left {
                left_group.push(btn);
            } else {
                right_group.push(btn);
            }
        }

        let mut out = Vec::new();
        if !left_group.is_empty() {
            let mut row = div().absolute().top(px(6.)).left(px(78.)).flex().items_center().gap_1();
            for el in left_group {
                row = row.child(el);
            }
            out.push(row.into_any_element());
        }
        if !right_group.is_empty() {
            let mut row = div().absolute().top(px(6.)).right(px(10.)).flex().items_center().gap_1();
            for el in right_group {
                row = row.child(el);
            }
            out.push(row.into_any_element());
        }
        out
    }

    /// Full-window translucent dock overlay shown while dragging Tabs or Agent.
    /// Supports docking to Left, Right, Top, or Bottom — and splits an occupied
    /// edge into two stack zones so panels can sit one above another or side-by-side.
    pub(super) fn render_dock_drop_overlay(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let strip_open = !self.strip.folded;
        let agent_open = self.inspector_open;
        let occupied = |edge: tabstrip::Placement| -> Option<PanelKind> {
            let has_tabs = strip_open && self.strip.placement == edge;
            let has_agent = agent_open && self.agent_edge == edge;
            match (has_tabs, has_agent) {
                (true, false) => Some(PanelKind::Tabs),
                (false, true) => Some(PanelKind::Agent),
                _ => None,
            }
        };

        let make_zone = |id: &'static str, title: &'static str, edge: tabstrip::Placement, stack: Option<StackOrder>, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex_1()
                .min_w_0()
                .min_h_0()
                .rounded(px(12.))
                .border_2()
                .border_color(t.border)
                .bg(t.raised.opacity(0.75))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(12.))
                .text_color(t.muted)
                .drag_over::<PanelDrag>(move |s, _, _, _| s.bg(t.accent.opacity(0.24)).border_color(t.accent).text_color(t.text))
                .on_drop(cx.listener(move |app, drag: &PanelDrag, _, cx| {
                    let resolved_stack = stack.map(|s| match (drag.0, s) {
                        (PanelKind::Tabs, StackOrder::TabsFirst) | (PanelKind::Agent, StackOrder::AgentFirst) => s,
                        (PanelKind::Tabs, StackOrder::AgentFirst) => StackOrder::AgentFirst,
                        (PanelKind::Agent, StackOrder::TabsFirst) => StackOrder::TabsFirst,
                    });
                    app.dock_panel(drag.0, edge, resolved_stack, cx);
                }))
                .child(title)
        };

        let vertical_edge = |edge: tabstrip::Placement, cx: &mut Context<Self>| -> gpui::AnyElement {
            let label = edge.label();
            match occupied(edge) {
                Some(PanelKind::Tabs) => div()
                    .w(px(190.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(make_zone(
                        if edge == tabstrip::Placement::Left { "drop-left-above" } else { "drop-right-above" },
                        "Stack Above",
                        edge,
                        Some(StackOrder::AgentFirst),
                        cx,
                    ))
                    .child(make_zone(
                        if edge == tabstrip::Placement::Left { "drop-left-below" } else { "drop-right-below" },
                        "Stack Below",
                        edge,
                        Some(StackOrder::TabsFirst),
                        cx,
                    ))
                    .into_any_element(),
                Some(PanelKind::Agent) => div()
                    .w(px(190.))
                    .h_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(make_zone(
                        if edge == tabstrip::Placement::Left { "drop-left-above" } else { "drop-right-above" },
                        "Stack Above",
                        edge,
                        Some(StackOrder::TabsFirst),
                        cx,
                    ))
                    .child(make_zone(
                        if edge == tabstrip::Placement::Left { "drop-left-below" } else { "drop-right-below" },
                        "Stack Below",
                        edge,
                        Some(StackOrder::AgentFirst),
                        cx,
                    ))
                    .into_any_element(),
                None => div()
                    .w(px(190.))
                    .h_full()
                    .flex()
                    .child(make_zone(
                        if edge == tabstrip::Placement::Left { "drop-left" } else { "drop-right" },
                        label,
                        edge,
                        None,
                        cx,
                    ))
                    .into_any_element(),
            }
        };

        let horizontal_edge = |edge: tabstrip::Placement, cx: &mut Context<Self>| -> gpui::AnyElement {
            let label = edge.label();
            match occupied(edge) {
                Some(PanelKind::Tabs) => div()
                    .w_full()
                    .h(px(76.))
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(make_zone(
                        if edge == tabstrip::Placement::Top { "drop-top-left" } else { "drop-bottom-left" },
                        "Dock Left",
                        edge,
                        Some(StackOrder::AgentFirst),
                        cx,
                    ))
                    .child(make_zone(
                        if edge == tabstrip::Placement::Top { "drop-top-right" } else { "drop-bottom-right" },
                        "Dock Right",
                        edge,
                        Some(StackOrder::TabsFirst),
                        cx,
                    ))
                    .into_any_element(),
                Some(PanelKind::Agent) => div()
                    .w_full()
                    .h(px(76.))
                    .flex()
                    .flex_row()
                    .gap_2()
                    .child(make_zone(
                        if edge == tabstrip::Placement::Top { "drop-top-left" } else { "drop-bottom-left" },
                        "Dock Left",
                        edge,
                        Some(StackOrder::TabsFirst),
                        cx,
                    ))
                    .child(make_zone(
                        if edge == tabstrip::Placement::Top { "drop-top-right" } else { "drop-bottom-right" },
                        "Dock Right",
                        edge,
                        Some(StackOrder::AgentFirst),
                        cx,
                    ))
                    .into_any_element(),
                None => div()
                    .w_full()
                    .h(px(76.))
                    .flex()
                    .child(make_zone(
                        if edge == tabstrip::Placement::Top { "drop-top" } else { "drop-bottom" },
                        label,
                        edge,
                        None,
                        cx,
                    ))
                    .into_any_element(),
            }
        };

        div()
            .id("dock-drop-overlay")
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .bg(t.scrim)
            .p_4()
            .pt(px(36.))
            .flex()
            .flex_col()
            .gap_3()
            .child(horizontal_edge(tabstrip::Placement::Top, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .gap_3()
                    .child(vertical_edge(tabstrip::Placement::Left, cx))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child("Drop on any edge to dock or stack"),
                    )
                    .child(vertical_edge(tabstrip::Placement::Right, cx)),
            )
            .child(horizontal_edge(tabstrip::Placement::Bottom, cx))
            .into_any_element()
    }

    fn render_pane(&self, tab: TabId, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let placements = self.placements.clone();
        let web = self.web.clone();
        let (url, sleeping, has_dom, title, load_error) = {
            let h = self.hub.lock();
            h.tab(tab)
                .map(|s| (s.url.clone(), s.sleeping, s.dom.root().is_some(), s.title.clone(), s.load_error.clone()))
                .unwrap_or_default()
        };
        let renders = web.borrow().renders_pages();
        let mut pane = div().relative().flex_1().h_full().bg(t.page).overflow_hidden();
        if let Some(snap) = self.snapshots.get(&tab).filter(|_| !has_dom && load_error.is_none()) {
            pane = pane.child(img(snap.clone()).size_full().opacity(0.6));
        } else if !renders {
            pane = pane.child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .text_color(t.muted)
                    .child(div().text_size(px(15.)).text_color(t.text).child(SharedString::from(if title.is_empty() { palette::short_url(&url) } else { title })))
                    .child(div().text_size(px(12.)).child(SharedString::from(url)))
                    .child(div().text_size(px(12.)).child("WebKit renders this page on macOS. This build has no WebKit.")),
            );
        } else if let Some(error) = load_error.filter(|_| !has_dom && !sleeping) {
            pane = pane.child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .p_8()
                    .child(div().text_size(px(16.)).text_color(t.text).child("This page did not load"))
                    .child(div().max_w(px(520.)).text_size(px(12.)).text_color(t.muted).child(SharedString::from(error)))
                    .child(
                        div()
                            .id(("retry-page", tab as usize))
                            .px_3()
                            .py_1p5()
                            .rounded(px(7.))
                            .bg(t.raised)
                            .shadow_sm()
                            .text_size(px(13.))
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.web.borrow_mut().apply(BrowserRequest::Reload(tab));
                                app.toast("Retrying…");
                                cx.notify();
                            }))
                            .child("Try again"),
                    ),
            );
        } else if !has_dom && !sleeping {
            pane = pane.child(div().absolute().top_0().left_0().right_0().h(px(2.)).bg(t.accent));
        }
        // Report this pane's bounds so the native web view tracks the layout.
        pane.child(
            canvas(
                move |bounds, _, _| bounds,
                move |_, bounds, _, _| {
                    let mut p = placements.borrow_mut();
                    p.retain(|(id, _)| *id != tab);
                    p.push((tab, bounds));
                    web.borrow().layout(&p);
                },
            )
            .absolute()
            .size_full(),
        )
    }

    /// An optional one-line address bar above the page. Clicking it opens the
    /// full palette with the URL selected, so typing replaces the address
    /// without a second shortcut.
    pub(super) fn render_address_bar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let url = self
            .active()
            .and_then(|id| self.hub.lock().tab(id).map(|tab| tab.url.clone()))
            .unwrap_or_default();
        let secure = url.starts_with("https://");
        let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(&url);
        let (host, path) = match rest.split_once('/') {
            Some((h, p)) => (h.to_string(), format!("/{p}")),
            None => (rest.to_string(), String::new()),
        };
        let target = url.clone();
        div()
            .id("address-bar")
            .h(px(30.))
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .bg(t.surface)
            .text_size(px(12.))
            .cursor_pointer()
            .on_click(cx.listener(move |app, _, _, cx| {
                if target.is_empty() {
                    app.open_palette(None, false);
                } else {
                    app.open_palette(Some(target.clone()), false);
                    if let Some(palette) = app.palette.as_mut() {
                        palette.select_all();
                    }
                }
                cx.notify();
            }))
            .when(!url.is_empty(), |d| d.child(self.site_icon(&url, &host, 14.)))
            .child(div().text_size(px(10.)).text_color(if secure { t.accent } else { t.warn }).child(if secure { "🔒" } else { "⚠" }))
            .child(div().text_color(t.text).child(SharedString::from(host)))
            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(t.muted).child(SharedString::from(path)))
            .child(div().text_size(px(10.)).text_color(t.muted).child("⌘L"))
            .into_any_element()
    }

    /// Enabled extensions as a compact right-aligned strip; clicking an icon
    /// opens its action popup, and the gear opens Settings → Extensions.
    pub(super) fn render_extension_toolbar(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let exts: Vec<_> = self
            .store
            .as_ref()?
            .extensions()
            .ok()?
            .into_iter()
            .filter(|e| e.enabled)
            .collect();
        if exts.is_empty() {
            return None;
        }
        let t = self.theme;
        let mut bar = div().id("extension-toolbar").h(px(24.)).w_full().flex().items_center().justify_end().gap_1().px_2().bg(t.surface);
        for ext in exts {
            let id = ext.id;
            let letter = ext.name.chars().next().unwrap_or('?').to_uppercase().collect::<String>();
            let icon = self.ext_icons.get(&id).cloned();
            let badge = self.ext_badges.get(&id).cloned();
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("ext-tool-{id}")))
                    .relative()
                    .size(px(22.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(5.))
                    .bg(t.well)
                    .text_size(px(10.))
                    .text_color(t.text)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |app, _, _, cx| app.open_extension_popup(&id, cx)))
                    .child(match icon {
                        Some(icon) => img(icon).size(px(16.)).into_any_element(),
                        None => div().child(SharedString::from(letter)).into_any_element(),
                    })
                    .when_some(badge, |d, b| {
                        let (bg, fg) = if b.unread {
                            (t.accent, t.on_accent)
                        } else {
                            (t.raised, t.text)
                        };
                        d.child(
                            div()
                                .absolute()
                                .bottom(px(-2.))
                                .right(px(-3.))
                                .px(px(3.))
                                .rounded_full()
                                .bg(bg)
                                .border_1()
                                .border_color(t.surface)
                                .text_size(px(8.))
                                .line_height(px(10.))
                                .text_color(fg)
                                .child(SharedString::from(b.text)),
                        )
                    }),
            );
        }
        bar = bar.child(
            div()
                .id("ext-tool-settings")
                .size(px(20.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .text_size(px(11.))
                .text_color(t.muted)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover).text_color(t.text))
                .on_click(cx.listener(|app, _, _, cx| {
                    app.settings_open = true;
                    app.settings_tab = SettingsTab::Extensions;
                    app.web.borrow().focus_shell();
                    cx.notify();
                }))
                .child("⚙"),
        );
        Some(bar.into_any_element())
    }

    /// The optional bookmarks bar: saved pages as one quiet row under the
    /// tab strip. Off by default; Settings → Tabs turns it on.
    pub(super) fn render_bookmarks_bar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let dark = t.background.l < 0.5;
        let bar_bg = self.current_space_look().strip_background(dark, t.surface);
        let mut bar = div()
            .id("bookmarks-bar")
            .h(px(24.))
            .w_full()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .bg(bar_bg)
            .overflow_hidden();
        if self.bookmarks_bar.is_empty() {
            bar = bar.child(div().text_size(px(11.)).text_color(t.muted).child("No bookmarks yet · ⌘D adds one"));
        }
        for (i, (title, url)) in self.bookmarks_bar.iter().enumerate() {
            let target = url.clone();
            let label = if title.trim().is_empty() { palette::short_url(url) } else { title.clone() };
            bar = bar.child(
                div()
                    .id(("bookmark-bar", i))
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_1p5()
                    .py_0p5()
                    .rounded(px(5.))
                    .when(!self.bookmarks_icons_only, |d| d.max_w(px(180.)))
                    .overflow_hidden()
                    .cursor_pointer()
                    .text_size(px(11.))
                    .text_color(t.text)
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.open_url(target.clone(), false);
                        cx.notify();
                    }))
                    .child(self.site_icon(url, title, 13.))
                    .when(!self.bookmarks_icons_only, |d| {
                        d.child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(SharedString::from(label)))
                    }),
            );
        }
        bar.into_any_element()
    }

    pub(super) fn render_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let Some(active) = self.active() else {
            return self.render_start_page(cx);
        };
        let mut row = div().flex_1().h_full().flex();
        row = row.child(self.render_pane(active, cx));
        let split_tab = self.split.filter(|s| *s != active && self.hub.lock().tab(*s).is_some());
        if let Some(s) = split_tab {
            row = row.child(div().w(px(6.)).h_full().bg(t.background)).child(self.render_pane(s, cx));
        }
        row.into_any_element()
    }

    /// The new-tab page: a quiet start field and the sections of `startpage`.
    /// A brand-new profile falls back to the welcome card inside the page.
    fn render_start_page(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let dark = t.background.l < 0.5;
        let has_wallpaper = self.start_bg_image.is_some();
        // No wordmark: the window is the browser, and a blank page should be
        // quiet. The search field is the only thing on it.
        let mut foreground = div()
            .relative()
            .flex_1()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(80.))
            .px_6()
            .overflow_hidden()
            .when(has_wallpaper, |d| {
                // Contrast scrim over arbitrary user photography so text and
                // controls stay legible in both light and dark modes.
                d.bg(gpui::rgba(if dark { 0x16171ab8 } else { 0xf6f6f7cc }))
            })
            .child(self.start_search(cx));
        if let Some((title, body)) = &self.agent_widget {
            foreground = foreground.child(
                div()
                    .id("agent-widget")
                    .mt_4()
                    .w_full()
                    .max_w(px(560.))
                    .px_4()
                    .py_3()
                    .rounded(px(10.))
                    .bg(t.raised)
                    .shadow_sm()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_size(px(12.)).text_color(t.accent).child(SharedString::from(title.clone())))
                    .child(div().text_size(px(13.)).text_color(t.text).child(SharedString::from(body.clone()))),
            );
        }
        let body = if self.start_page.is_empty() { self.render_welcome(cx).into_any_element() } else { self.start_grid(cx) };
        foreground = foreground.child(
            div()
                .id("start-body")
                .flex_1()
                .w_full()
                .flex()
                .items_start()
                .justify_center()
                .overflow_y_scroll()
                .child(body),
        );
        let mut container = div()
            .relative()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(self.start_bg.background(dark, t.page));
        if let Some(wallpaper) = &self.start_bg_image {
            container = container.child(
                img(wallpaper.clone())
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            );
        }
        container.child(foreground).into_any_element()
    }

    fn start_search(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        div()
            .id("start-search")
            .w_full()
            .max_w(px(560.))
            .h(px(44.))
            .mt_5()
            .px_4()
            .flex()
            .items_center()
            .justify_between()
            .rounded(px(12.))
            .bg(t.well)
            .shadow_sm()
            .text_size(px(14.))
            .text_color(t.muted)
            .cursor_pointer()
            .hover(|s| s.bg(t.raised))
            .on_click(cx.listener(|app, _, _, cx| {
                app.open_palette(None, false);
                cx.notify();
            }))
            .child("Search or enter an address")
            .child(div().text_size(px(11.)).text_color(t.muted).child("⌘K"))
            .into_any_element()
    }

    fn start_grid(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut left = div().flex().flex_col().gap_6().flex_1().min_w_0().overflow_hidden();
        let mut right = div().flex().flex_col().gap_6().flex_1().min_w_0().overflow_hidden();
        for (i, (section, items)) in self.start_page.sections.iter().enumerate() {
            let card = self.start_section(*section, items, cx);
            if i % 2 == 0 {
                left = left.child(card);
            } else {
                right = right.child(card);
            }
        }
        div()
            .flex()
            .gap_8()
            .mt_8()
            .pb_8()
            .w_full()
            .max_w(px(880.))
            .child(left)
            .child(right)
            .into_any_element()
    }

    fn start_section(&self, section: Section, items: &[StartItem], cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let is_bookmarks = section == Section::Bookmarks;
        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .px_2()
            .pb_1()
            .child(div().text_size(px(11.)).text_color(t.muted).child(section.title().to_uppercase()))
            .when(is_bookmarks, |d| {
                d.child(
                    div()
                        .id("start-manage-bookmarks")
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(4.))
                        .text_size(px(10.))
                        .text_color(t.muted)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover).text_color(t.text))
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.run_command(Command::ShowBookmarks, cx);
                            cx.notify();
                        }))
                        .child("Manage"),
                )
            });
        let mut col = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_3()
            .rounded(px(12.))
            .bg(t.surface.opacity(0.75))
            .min_w_0()
            .overflow_hidden()
            .child(header);
        if items.is_empty() {
            col = col.child(div().px_2().py_1().text_size(px(12.)).text_color(t.muted).child("Nothing yet"));
        }
        for (i, item) in items.iter().enumerate() {
            let url = item.url.clone();
            let rm_url = item.url.clone();
            let mut row = div()
                .id(SharedString::from(format!("start-{}-{i}", section.as_str())))
                .flex()
                .items_center()
                .gap_2()
                .h(px(30.))
                .px_2()
                .rounded(px(6.))
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.open_url(url.clone(), false);
                    cx.notify();
                }))
                .child(self.site_icon(&item.url, &item.title, 16.))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(13.))
                        .text_color(t.text)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(SharedString::from(item.title.clone())),
                )
                .child(
                    div()
                        .max_w(px(120.))
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(t.muted)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(SharedString::from(item.note.clone())),
                );
            if is_bookmarks {
                row = row.child(
                    div()
                        .id(("start-rm-bm", i))
                        .w(px(18.))
                        .h(px(18.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .text_size(px(13.))
                        .text_color(t.muted)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover).text_color(t.error))
                        .on_click(cx.listener(move |app, _, _, cx| {
                            cx.stop_propagation();
                            app.remove_bookmark_url(&rm_url, cx);
                        }))
                        .child("×"),
                );
            }
            col = col.child(row);
        }
        col.into_any_element()
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let button = |id: &'static str, label: &'static str, cmd: Command, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded(px(8.))
                .bg(t.well)
                .text_size(px(12.))
                .text_color(t.text)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.run_command(cmd, cx);
                    cx.notify();
                }))
                .child(label)
        };
        let safari_btn = div()
            .id("welcome-import-safari")
            .px_3()
            .py_1p5()
            .rounded(px(8.))
            .bg(t.well)
            .text_size(px(12.))
            .text_color(t.text)
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .on_click(cx.listener(|app, _, _, cx| app.import_safari_export_file(cx)))
            .child("Import Safari…");
        div()
            .w_full()
            .max_w(px(560.))
            .mt_8()
            .mb_8()
            .flex()
            .flex_col()
            .gap_3()
            .p_5()
            .rounded(px(14.))
            .bg(t.surface)
            .shadow_md()
            .child(div().text_size(px(15.)).text_color(t.text).child("Get started"))
            .child(div().text_size(px(13.)).text_color(t.muted).child(
                "Import bookmarks and history from another browser, or connect an AI agent to the live page.",
            ))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(button("import", "Import Chrome / Arc", Command::ImportChrome, cx))
                    .child(safari_btn)
                    .child(button("mcp", "Copy MCP setup", Command::CopyMcpSetup, cx)),
            )
            .child(
                div()
                    .pt_1()
                    .text_size(px(11.))
                    .text_color(t.muted)
                    .child("⌘K search or open URL · ⌘T new tab · ⌘, settings"),
            )
    }


    pub(super) fn document_title(&self) -> String {
        let h = self.hub.lock();
        match h.active().and_then(|a| h.tab(a)) {
            Some(t) if !t.title.is_empty() => {
                if self.app_mode { t.title.clone() } else { format!("{} — Relay", t.title) }
            }
            Some(t) => {
                if self.app_mode { palette::short_url(&t.url) } else { format!("{} — Relay", palette::short_url(&t.url)) }
            }
            None => self.app_name.clone().unwrap_or_else(|| "Relay".to_string()),
        }
    }


    pub(super) fn challenge_handoff(&self) -> Option<(TabId, relay_bridge::Challenge)> {
        let active = self.active()?;
        let tab = self.hub.lock();
        let t = tab.tab(active)?;
        t.challenge.clone().map(|c| (active, c))
    }

    pub(super) fn render_challenge_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let (tab_id, challenge) = self.challenge_handoff().unwrap_or((0, relay_bridge::Challenge::Captcha));
        div()
            .id("challenge-banner")
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_2()
            .bg(t.warn.opacity(0.16))
            .text_size(px(12.))
            .text_color(t.text)
            .child(div().flex_1().child(SharedString::from(format!(
                "Agent is waiting for you — finish the {} in this tab, then continue",
                match challenge {
                    relay_bridge::Challenge::Captcha => "CAPTCHA",
                    relay_bridge::Challenge::TwoFactor => "two-factor check",
                }
            ))))
            .child(
                div()
                    .id("challenge-continue")
                    .px_2()
                    .py_0p5()
                    .rounded(px(5.))
                    .bg(t.warn.opacity(0.14))
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.hub.update(|h| {
                            if let Some(tab) = h.tab_mut(tab_id) {
                                tab.challenge = None;
                            }
                        });
                        app.toast("Agent may continue");
                        app.focus_active_page();
                        cx.notify();
                    }))
                    .child("Continue"),
            )
    }

    pub(super) fn render_agent_banner(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div()
            .flex()
            .items_center()
            .justify_between()
            .h(px(30.))
            .px_3()
            .bg(t.agent_bg)
            .text_size(px(12.))
            .text_color(t.text)
            .child(div().child("Agents paused — page automation is stopped"))
            .child(
                div()
                    .id("resume-agents")
                    .px_2()
                    .py_0p5()
                    .rounded(px(5.))
                    .bg(t.agent_bg)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.hub.set_agents_paused(false);
                        app.toast("Agents resumed");
                        cx.notify();
                    }))
                    .child("Resume"),
            )
    }

    pub(super) fn render_login_fill_banner(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (tab, url, form_idx, candidates) = self.login_fill_prompt.as_ref()?;
        if self.active() != Some(*tab) {
            return None;
        }
        let t = self.theme;
        let tab = *tab;
        let form_idx = *form_idx;
        let host = host_of(url).unwrap_or_else(|| "this site".to_string());
        let mut bar = div()
            .id("login-fill-banner")
            .flex()
            .items_center()
            .gap_2()
            .h(px(34.))
            .px_3()
            .bg(t.raised)
            .shadow_sm()
            .text_size(px(12.))
            .text_color(t.text)
            .child(div().text_color(t.muted).child(SharedString::from(format!("Keychain login for {host}:"))));
        for (idx, (username, path)) in candidates.iter().enumerate() {
            let u = username.clone();
            let p = path.clone();
            let url_c = url.clone();
            let label = if username.is_empty() { "(saved password)".to_string() } else { username.clone() };
            bar = bar.child(
                div()
                    .id(("login-fill-opt", idx))
                    .px_2p5()
                    .py_0p5()
                    .rounded(px(5.))
                    .bg(t.active)
                    .text_color(t.text)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        let res = app.web.borrow().fill_login(tab, &url_c, form_idx, &u, &p);
                        app.login_fill_prompt = None;
                        match res {
                            Ok(()) => app.toast("Filling password…"),
                            Err(e) => app.toast(e),
                        }
                        app.focus_active_page();
                        cx.notify();
                    }))
                    .child(SharedString::from(label)),
            );
        }
        bar = bar.child(div().flex_1()).child(
            div()
                .id("login-fill-dismiss")
                .px_2()
                .py_0p5()
                .rounded(px(4.))
                .text_color(t.muted)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover).text_color(t.text))
                .on_click(cx.listener(|app, _, _, cx| {
                    app.login_fill_prompt = None;
                    app.focus_active_page();
                    cx.notify();
                }))
                .child("Dismiss"),
        );
        Some(bar.into_any_element())
    }

    pub(super) fn render_login_save_banner(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let (tab, url, username) = self.login_save_prompt.as_ref()?;
        if self.active() != Some(*tab) {
            return None;
        }
        let t = self.theme;
        let tab = *tab;
        let origin_str = relay_extensions::login::Origin::parse(url).map(|o| o.to_string()).unwrap_or_default();
        let host = host_of(url).unwrap_or_else(|| "this site".to_string());
        let user_c = username.clone();
        let url_c = url.clone();
        let display_user = if username.is_empty() { "this account" } else { username.as_str() };
        let existing = self
            .store
            .as_ref()
            .and_then(|s| {
                let (origin, path) = relay_extensions::login::Origin::parse_with_path(url).ok()?;
                s.has_saved_login(origin.host(), username, &path).ok()
            })
            .unwrap_or(false);
        Some(
            div()
                .id("login-save-banner")
                .flex()
                .items_center()
                .gap_2()
                .h(px(34.))
                .px_3()
                .bg(t.raised)
                .shadow_sm()
                .text_size(px(12.))
                .text_color(t.text)
                .child(div().flex_1().child(SharedString::from(format!("Save login for {display_user} on {host} to macOS Keychain?"))))
                .child(
                    div()
                        .id("login-save-accept")
                        .px_2p5()
                        .py_0p5()
                        .rounded(px(5.))
                        .bg(t.active)
                        .text_color(t.text)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover))
                        .on_click(cx.listener(move |app, _, _, cx| {
                            let res = app.web.borrow().accept_save_login(tab);
                            app.login_save_prompt = None;
                            match res {
                                Ok(saved_user) => {
                                    if let Some(store) = &app.store {
                                        if let Ok((origin, path)) = relay_extensions::login::Origin::parse_with_path(&url_c) {
                                            let _ = store.record_saved_login(origin.host(), &saved_user, &path, now_unix());
                                        }
                                    }
                                    app.toast("Saved login to macOS Keychain")
                                }
                                Err(e) => app.toast(e),
                            }
                            cx.notify();
                        }))
                        .child(if existing { "Replace" } else { "Save" }),
                )
                .child(
                    div()
                        .id("login-save-decline")
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.))
                        .text_color(t.muted)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover).text_color(t.text))
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.web.borrow().decline_save_login(tab);
                            if !origin_str.is_empty() {
                                app.declined_login_saves.insert((origin_str.clone(), user_c.clone()));
                            }
                            app.login_save_prompt = None;
                            cx.notify();
                        }))
                        .child("Not now"),
                )
                .into_any_element(),
        )
    }

    pub(super) fn render_status(&self, _at_top: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let (url, title, hover, asleep, agents) = {
            let h = self.hub.lock();
            let active = h.active().and_then(|a| h.tab(a));
            let url = active.map(|x| x.url.clone()).unwrap_or_default();
            let title = active.map(|x| x.title.clone()).unwrap_or_default();
            let hover = active.and_then(|x| x.hover_url.clone());
            (
                url,
                title,
                hover,
                h.tabs().filter(|x| x.sleeping).count(),
                h.tabs().filter(|x| x.owner == Owner::Agent).count(),
            )
        };
        let site_host = crate::clean_url::host_of(&url);
        let status_text = match (&self.hints, &self.toast) {
            // Native web views cover GPUI overlays, so hint status lives here.
            (Some(h), _) => format!("Link hints: type a label{} · Esc cancels", if h.typed.is_empty() { String::new() } else { format!(" ({})", h.typed.to_uppercase()) }),
            (None, Some((m, _))) => m.clone(),
            (None, None) => hover.map(|h| format!("→ {h}")).unwrap_or_else(|| url.clone()),
        };
        let mut meta_parts: Vec<String> = Vec::new();
        if asleep > 0 {
            meta_parts.push(format!("{asleep} asleep"));
        }
        if agents > 0 {
            meta_parts.push(format!("{agents} agent"));
        }
        let meta = meta_parts.join(" · ");
        let show_icon = self.hints.is_none() && self.toast.is_none() && !url.is_empty();
        div()
            .h(px(24.))
            .flex()
            .items_center()
            .px_3()
            .gap_2()
            .text_size(px(11.))
            .text_color(t.muted)
            .bg(t.surface)
            .when(show_icon, |d| d.child(self.site_icon(&url, &title, 13.)))
            .child(
                div()
                    .id("status-site")
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when(!site_host.is_empty(), |d| d.cursor_pointer().hover(|s| s.text_color(t.text)))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        if !site_host.is_empty() {
                            app.open_site_card(site_host.clone());
                            cx.notify();
                        }
                    }))
                    .child(SharedString::from(status_text)),
            )
            .when(!meta.is_empty(), |d| {
                d.child(div().text_size(px(10.)).text_color(t.muted).child(SharedString::from(meta)))
            })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn monogram_letters_have_white_contrast() {
        fn luminance(rgb: u32) -> f64 {
            let channel = |v: u32| {
                let c = (v & 0xff) as f64 / 255.0;
                if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
            };
            // The palette constants are 0xRRGGBB, not 0xAARRGGBB.
            0.2126 * channel(rgb >> 16) + 0.7152 * channel(rgb >> 8) + 0.0722 * channel(rgb)
        }
        for color in [0x1e3a8a, 0x4c1d95, 0x065f46, 0x1e40af, 0x7f1d1d, 0x7c2d12, 0x831843, 0x14532d] {
            let ratio = 1.05 / (luminance(color) + 0.05);
            assert!(ratio >= 4.5, "{color:#08x} has white contrast {ratio:.2}:1");
        }
    }
}
