//! Primary shell chrome: tab strip, folded rail, panes, start/welcome views, banners, and status bar.

use super::*;

impl RelayApp {
    pub(super) fn render_tab_bar(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let strip = self.strip;
        if strip.folded {
            return self.render_folded_strip(cx);
        }
        let (active, user, agents) = {
            let h = self.hub.lock();
            let active = h.active();
            let mut user = vec![];
            let mut agents = vec![];
            for view in h.tab_views() {
                if view.owner == Owner::Agent { agents.push(view) } else { user.push(view) }
            }
            (active, user, agents)
        };
        let vertical = strip.placement.is_vertical();
        let mut bar = div().flex();
        if vertical {
            bar = bar
                .flex_col()
                .gap_0p5()
                .p_2()
                .w(px(strip.width()))
                .h_full()
                .bg(t.surface)
                .border_color(t.border);
            bar = if strip.placement == tabstrip::Placement::Left { bar.border_r_1() } else { bar.border_l_1() };
            bar = bar.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .pb_2()
                    .child(div().text_size(px(11.)).text_color(t.muted).child("TABS"))
                    .child(self.fold_button(if strip.placement == tabstrip::Placement::Left { "«" } else { "»" }, cx)),
            );
            for tab in &user {
                bar = bar.child(self.tab_row(tab, active, true, cx));
            }
            if !agents.is_empty() {
                bar = bar.child(div().mt_3().px_2().pb_1().text_size(px(11.)).text_color(t.agent).child("AGENT"));
                for tab in &agents {
                    bar = bar.child(self.tab_row(tab, active, true, cx));
                }
            }
            bar.child(div().flex_1()).child(self.new_tab_button(true, cx)).into_any_element()
        } else {
            bar = bar
                .flex_row()
                .items_center()
                .gap_1()
                .px_2()
                .h(px(strip.height()))
                .w_full()
                .bg(t.surface)
                .border_b_1()
                .border_color(t.border)
                .child(div().text_size(px(11.)).text_color(t.muted).child("TABS"));
            for tab in &user {
                bar = bar.child(self.tab_row(tab, active, false, cx));
            }
            if !agents.is_empty() {
                bar = bar.child(div().px_1().text_size(px(10.)).text_color(t.agent).child("AGENT"));
                for tab in &agents {
                    bar = bar.child(self.tab_row(tab, active, false, cx));
                }
            }
            bar.child(self.new_tab_button(false, cx))
                .child(div().flex_1())
                .child(self.fold_button("⌃", cx))
                .into_any_element()
        }
    }

    fn tab_row(&self, tab: &TabView, active: Option<TabId>, vertical: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let id = tab.id;
        let is_active = active == Some(id);
        let agent = tab.owner == Owner::Agent;
        let title: SharedString = if tab.title.is_empty() { palette::short_url(&tab.url).into() } else { tab.title.clone().into() };
        let show_close = is_active || self.hovered_tab == Some(id);
        let mut row = div()
            .id(("tab", id as usize))
            .flex()
            .items_center()
            .gap_1()
            .h(px(28.))
            .px_2()
            .rounded(px(6.))
            .text_size(px(13.))
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
            }));
        let icon = host_of(&tab.url).and_then(|host| self.favicons.get(&host)).and_then(|icon| icon.clone());
        row = row.child(match icon {
            Some(icon) => img(icon).size(px(14.)).rounded(px(3.)).into_any_element(),
            None => div()
                .size(px(8.))
                .rounded_full()
                .bg(if agent { t.agent } else if is_active { t.accent } else { t.border })
                .into_any_element(),
        });
        let title_el = if vertical {
            div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(title)
        } else {
            div().max_w(px(170.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(title)
        };
        row = row.child(title_el);
        if agent {
            row = row.bg(t.agent_bg);
        }
        if is_active {
            row = row.bg(t.active);
        }
        if tab.challenge.is_some() {
            row = row.child(div().text_color(t.warn).child("⚠"));
        }
        if tab.playing_audio {
            row = row.child(div().text_color(t.muted).text_size(px(11.)).child("♪"));
        }
        if tab.pinned {
            row = row.child(div().text_color(t.muted).text_size(px(11.)).child("pin"));
        }
        if tab.sleeping {
            row = row.child(div().text_color(t.muted).text_size(px(11.)).child("zz"));
        }
        row.child(
            div()
                .id(("close-tab", id as usize))
                .w(px(18.))
                .h(px(18.))
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
        )
        .into_any_element()
    }

    fn new_tab_button(&self, vertical: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let button = div()
            .id("new-tab")
            .flex()
            .items_center()
            .rounded(px(6.))
            .text_size(px(12.))
            .text_color(t.muted)
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .on_click(cx.listener(|app, _, _, cx| {
                app.open_palette(None, true);
                cx.notify();
            }));
        if vertical {
            button.px_2().h(px(28.)).child("+ New tab   ⌘T").into_any_element()
        } else {
            button.w(px(26.)).h(px(26.)).justify_center().child("+").into_any_element()
        }
    }

    fn fold_button(&self, label: &'static str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        div()
            .id("fold-tab-strip")
            .w(px(20.))
            .h(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .text_size(px(12.))
            .text_color(t.muted)
            .cursor_pointer()
            .hover(|s| s.bg(t.hover).text_color(t.text))
            .on_click(cx.listener(|app, _, _, cx| {
                app.toggle_tab_strip(cx);
            }))
            .child(label)
            .into_any_element()
    }

    /// The folded strip is a thin rail: visible, clickable, and never in the
    /// way. Clicking anywhere on it brings the tabs back.
    pub(super) fn render_folded_strip(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let strip = self.strip;
        let (arrow, vertical) = match strip.placement {
            tabstrip::Placement::Left => ("›", true),
            tabstrip::Placement::Right => ("‹", true),
            tabstrip::Placement::Top => ("⌄", false),
        };
        let rail = div()
            .id("unfold-tab-strip")
            .bg(t.surface)
            .border_color(t.border)
            .cursor_pointer()
            .flex()
            .items_center()
            .justify_center()
            .on_click(cx.listener(|app, _, _, cx| {
                app.toggle_tab_strip(cx);
            }))
            .child(div().text_size(px(if vertical { 11. } else { 10. })).text_color(t.muted).child(arrow));
        if vertical {
            let rail = rail.w(px(strip.width())).h_full();
            match strip.placement {
                tabstrip::Placement::Left => rail.border_r_1(),
                _ => rail.border_l_1(),
            }
            .into_any_element()
        } else {
            rail.w_full().h(px(strip.height())).border_b_1().into_any_element()
        }
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
                            .rounded(px(6.))
                            .border_1()
                            .border_color(t.border)
                            .bg(t.surface)
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

    pub(super) fn render_content(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let Some(active) = self.active() else {
            return self.render_start_page(cx);
        };
        let mut row = div().flex_1().h_full().flex();
        row = row.child(self.render_pane(active, cx));
        if let Some(s) = self.split.filter(|s| *s != active && self.hub.lock().tab(*s).is_some()) {
            row = row.child(div().w(px(1.)).h_full().bg(t.border)).child(self.render_pane(s, cx));
        }
        row.into_any_element()
    }

    /// The new-tab page: a quiet start field and the sections of `startpage`.
    /// A brand-new profile falls back to the welcome card inside the page.
    fn render_start_page(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let title = self.app_name.clone().unwrap_or_else(|| "Relay".to_string());
        let page = div()
            .flex_1()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(56.))
            .px_6()
            .bg(t.page)
            .overflow_hidden()
            .child(div().text_size(px(28.)).text_color(t.text).child(SharedString::from(title)))
            .child(self.start_search(cx));
        let mut page = page;
        if let Some((title, body)) = &self.agent_widget {
            page = page.child(
                div()
                    .id("agent-widget")
                    .mt_4()
                    .w(px(560.))
                    .px_4()
                    .py_3()
                    .rounded(px(8.))
                    .bg(t.surface)
                    .border_1()
                    .border_color(t.border)
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_size(px(12.)).text_color(t.accent).child(SharedString::from(title.clone())))
                    .child(div().text_size(px(13.)).text_color(t.text).child(SharedString::from(body.clone()))),
            );
        }
        let body = if self.start_page.is_empty() { self.render_welcome(cx).into_any_element() } else { self.start_grid(cx) };
        page.child(div().id("start-body").flex_1().w_full().flex().justify_center().overflow_y_scroll().child(body)).into_any_element()
    }

    fn start_search(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        div()
            .id("start-search")
            .w(px(560.))
            .h(px(44.))
            .mt_5()
            .px_4()
            .flex()
            .items_center()
            .justify_between()
            .rounded(px(10.))
            .border_1()
            .border_color(t.border)
            .bg(t.surface)
            .text_size(px(14.))
            .text_color(t.muted)
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .on_click(cx.listener(|app, _, _, cx| {
                app.open_palette(None, false);
                cx.notify();
            }))
            .child("Search or enter an address")
            .child(div().text_size(px(11.)).child("⌘K"))
            .into_any_element()
    }

    fn start_grid(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut left = div().flex().flex_col().gap_6().flex_1();
        let mut right = div().flex().flex_col().gap_6().flex_1();
        for (i, (section, items)) in self.start_page.sections.iter().enumerate() {
            let card = self.start_section(*section, items, cx);
            if i % 2 == 0 {
                left = left.child(card);
            } else {
                right = right.child(card);
            }
        }
        div().flex().gap_6().mt_8().w(px(680.)).child(left).child(right).into_any_element()
    }

    fn start_section(&self, section: Section, items: &[StartItem], cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let mut col = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().px_2().text_size(px(11.)).text_color(t.muted).child(section.title().to_uppercase()));
        if items.is_empty() {
            col = col.child(div().px_2().py_1().text_size(px(12.)).text_color(t.muted).child("Nothing yet"));
        }
        for (i, item) in items.iter().enumerate() {
            let url = item.url.clone();
            col = col.child(
                div()
                    .id(SharedString::from(format!("start-{}-{i}", section.as_str())))
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(26.))
                    .px_2()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.open_url(url.clone(), false);
                        cx.notify();
                    }))
                    .child(match host_of(&item.url).and_then(|host| self.favicons.get(&host)).and_then(|icon| icon.clone()) {
                        Some(icon) => img(icon).size(px(14.)).rounded(px(3.)).into_any_element(),
                        None => div().size(px(14.)).into_any_element(),
                    })
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.))
                            .text_color(t.text)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(item.title.clone())),
                    )
                    .child(
                        div()
                            .max_w(px(150.))
                            .text_size(px(11.))
                            .text_color(t.muted)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(item.note.clone())),
                    ),
            );
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
                .rounded(px(6.))
                .border_1()
                .border_color(t.border)
                .bg(t.surface)
                .text_size(px(13.))
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.run_command(cmd, cx);
                    cx.notify();
                }))
                .child(label)
        };
        div()
            .w(px(560.))
            .mt_8()
            .flex()
            .flex_col()
            .gap_3()
            .p_6()
            .rounded(px(10.))
            .bg(t.surface)
            .border_1()
            .border_color(t.border)
            .child(div().text_size(px(15.)).text_color(t.text).child("A browser that agents can read"))
            .child(div().text_size(px(13.)).text_color(t.muted).child("Import your Chrome bookmarks and history, or connect Claude Code, Codex, OpenCode, or Zed to the live page."))
            .child(div().flex().gap_2().child(button("import", "Import from Chrome", Command::ImportChrome, cx)).child(button("mcp", "Copy MCP setup", Command::CopyMcpSetup, cx)))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .font_family("Menlo")
                    .child(SharedString::from(format!("Agents: {}\nSocket: {}", self.mcp_setup, self.mcp_socket))),
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
            .border_b_1()
            .border_color(t.warn)
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
                    .rounded(px(4.))
                    .border_1()
                    .border_color(t.warn)
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
            .border_t_1()
            .border_color(t.agent)
            .text_size(px(12.))
            .text_color(t.text)
            .child(div().child("Agents paused — page automation is stopped"))
            .child(
                div()
                    .id("resume-agents")
                    .px_2()
                    .py_0p5()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(t.agent)
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

    pub(super) fn render_status(&self) -> impl IntoElement {
        let t = self.theme;
        let (url, total, asleep, agents) = {
            let h = self.hub.lock();
            let url = h.active().and_then(|a| h.tab(a)).map(|x| x.url.clone()).unwrap_or_default();
            (url, h.tabs().count(), h.tabs().filter(|x| x.sleeping).count(), h.tabs().filter(|x| x.owner == Owner::Agent).count())
        };
        let mut right = format!("{total} tabs");
        if asleep > 0 {
            right.push_str(&format!(" · {asleep} asleep"));
        }
        if agents > 0 {
            right.push_str(&format!(" · {agents} agent"));
        }
        if self.mcp_enabled {
            right.push_str(" · agents: relay mcp");
        } else {
            right.push_str(" · mcp: off");
        }
        div()
            .h(px(24.))
            .flex()
            .items_center()
            .px_3()
            .gap_3()
            .text_size(px(11.))
            .text_color(t.muted)
            .bg(t.surface)
            .border_t_1()
            .border_color(t.border)
            .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(SharedString::from(match (&self.hints, &self.toast) {
                // Native web views cover GPUI overlays, so hint status lives here.
                (Some(h), _) => format!("Link hints: type a label{} · Esc cancels", if h.typed.is_empty() { String::new() } else { format!(" ({})", h.typed.to_uppercase()) }),
                (None, Some((m, _))) => m.clone(),
                (None, None) => url,
            })))
            .child(SharedString::from(right))
    }
}
