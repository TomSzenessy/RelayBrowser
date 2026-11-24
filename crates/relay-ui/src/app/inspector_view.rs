//! Right-hand Agent and native Rust Inspector sidebar rendering.

use super::*;

impl RelayApp {
    pub(super) fn render_inspector(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let has_agent_tabs = self.hub.lock().tabs().any(|tab| tab.owner == Owner::Agent);
        let mut head = div()
            .flex()
            .items_center()
            .gap_1()
            .px_3()
            .h(px(36.))
            .border_b_1()
            .border_color(t.border);
        for (tab, label) in [(SidebarTab::Agent, "✦ Agent"), (SidebarTab::Inspector, "Inspector")] {
            let on = self.sidebar_tab == tab;
            head = head.child(
                div()
                    .id(if tab == SidebarTab::Agent { "sidebar-agent" } else { "sidebar-inspector" })
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .px_2p5()
                    .py_1()
                    .rounded(px(6.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .text_color(if on { t.text } else { t.muted })
                    .when(on, |d| d.bg(if tab == SidebarTab::Agent { t.agent_bg } else { t.active }))
                    .hover(|s| s.text_color(t.text))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.sidebar_tab = tab;
                        if tab != SidebarTab::Agent {
                            app.agent_input_focused = false;
                        }
                        app.hub.set_sidebar(app.inspector_open, tab.as_str());
                        cx.notify();
                    }))
                    .child(label)
                    .when(tab == SidebarTab::Agent && has_agent_tabs, |d| {
                        d.child(div().size(px(6.)).rounded_full().bg(t.agent))
                    }),
            );
        }
        head = head.child(div().flex_1()).child(
            div()
                .id("sidebar-collapse")
                .w(px(22.))
                .h(px(22.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .text_size(px(14.))
                .text_color(t.muted)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover).text_color(t.text))
                .on_click(cx.listener(|app, _, _, cx| {
                    app.inspector_open = false;
                    app.agent_input_focused = false;
                    app.hub.set_sidebar(false, app.sidebar_tab.as_str());
                    app.focus_active_page();
                    cx.notify();
                }))
                .child("×"),
        );
        let body = match self.sidebar_tab {
            SidebarTab::Agent => self.render_agent_pane(cx),
            SidebarTab::Inspector => div()
                .id("sidebar-body")
                .flex_1()
                .overflow_y_scroll()
                .font_family("Menlo")
                .text_size(px(12.))
                .child(self.render_inspector_pane(cx))
                .into_any_element(),
        };
        div()
            .w(px(360.))
            .h_full()
            .flex()
            .flex_col()
            .bg(t.surface)
            .border_l_1()
            .border_color(t.border)
            .child(head)
            .child(body)
    }

    fn render_agent_pane(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let (host, policy) = self
            .active()
            .and_then(|tab| self.hub.lock().tab(tab).map(|t| t.url.clone()))
            .and_then(|url| relay_bridge::host_of(&url))
            .map(|host| {
                let policy = self.hub.agent_policy_for_host(&host);
                (host, policy)
            })
            .unwrap_or_else(|| ("No active site".to_string(), relay_bridge::AgentPolicy::Allow));
        let paused = self.hub.agents_paused();
        let agent_tabs: Vec<(TabId, String, String, bool)> = self
            .hub
            .lock()
            .tabs()
            .filter(|tab| tab.owner == Owner::Agent)
            .map(|tab| (tab.id, tab.title.clone(), tab.url.clone(), tab.challenge.is_some()))
            .collect();
        let clients = self.store.as_ref().and_then(|s| s.mcp_clients().ok()).unwrap_or_default();
        let recent_audit = self.store.as_ref().and_then(|s| s.agent_audit(5).ok()).unwrap_or_default();
        let granted = clients.iter().filter(|c| c.granted).count();

        // Compact live status + per-site policy bar at the top.
        let status_card = div()
            .mx_3()
            .mt_3()
            .p_2p5()
            .rounded(px(9.))
            .bg(t.raised)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(div().size(px(7.)).rounded_full().bg(if paused {
                                t.warn
                            } else if self.mcp_enabled {
                                t.agent
                            } else {
                                t.muted
                            }))
                            .child(div().text_size(px(12.)).text_color(t.text).child(if paused {
                                "Agents paused"
                            } else if self.mcp_enabled {
                                "Agents ready"
                            } else {
                                "MCP off"
                            }))
                            .when(!clients.is_empty(), |d| {
                                d.child(div().text_size(px(10.)).text_color(t.muted).child(SharedString::from(format!(
                                    "· {} client{}",
                                    clients.len(),
                                    if clients.len() == 1 { "" } else { "s" }
                                ))))
                            }),
                    )
                    .child(
                        div()
                            .id(if paused { "agent-resume" } else { "agent-pause" })
                            .px_2()
                            .py_0p5()
                            .rounded(px(5.))
                            .bg(if paused { t.active } else { t.well })
                            .text_size(px(11.))
                            .text_color(if paused { t.accent } else { t.text })
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(|app, _, _, cx| app.toggle_agents(cx)))
                            .child(if paused { "Resume" } else { "Pause" }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(11.))
                            .text_color(t.muted)
                            .child(SharedString::from(host)),
                    )
                    .child(
                        div()
                            .id("agent-cycle-policy")
                            .px_2()
                            .py_0p5()
                            .rounded(px(5.))
                            .bg(t.well)
                            .text_size(px(10.))
                            .text_color(t.text)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(|app, _, _, cx| app.cycle_agent_policy(cx)))
                            .child(SharedString::from(format!("Site: {}", policy.label()))),
                    ),
            );

        // Live agent-owned tabs section (only when agents have open tabs).
        let mut agent_tabs_section = div().flex().flex_col().gap_1().mx_3().mt_2();
        if !agent_tabs.is_empty() {
            agent_tabs_section = agent_tabs_section.child(
                div().text_size(px(10.)).text_color(t.agent).child(SharedString::from(format!("AGENT TABS ({})", agent_tabs.len()))),
            );
            for (id, title, url, needs_human) in &agent_tabs {
                let tid = *id;
                let label = if title.is_empty() { palette::short_url(url) } else { title.clone() };
                agent_tabs_section = agent_tabs_section.child(
                    div()
                        .id(("sidebar-agent-tab", tid as usize))
                        .flex()
                        .items_center()
                        .gap_2()
                        .h(px(26.))
                        .px_2()
                        .rounded(px(6.))
                        .bg(t.agent_bg)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover))
                        .on_click(cx.listener(move |app, _, _, cx| {
                            app.activate(tid);
                            cx.notify();
                        }))
                        .child(self.site_icon(url, title, 14.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(11.))
                                .text_color(t.text)
                                .child(SharedString::from(label)),
                        )
                        .when(*needs_human, |d| d.child(div().text_size(px(11.)).text_color(t.warn).child("⚠"))),
                );
            }
        }

        // Integrated side-by-side AI Page Assistant.
        let chat = self.ai_chat.as_ref();
        let has_messages = chat.is_some_and(|c| !c.messages.is_empty());
        let assistant_header = div()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .pt_3()
            .pb_1()
            .child(div().text_size(px(11.)).text_color(t.muted).child("PAGE ASSISTANT"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(has_messages, |d| {
                        d.child(
                            div()
                                .id("ai-new")
                                .px_1p5()
                                .py_0p5()
                                .rounded(px(4.))
                                .text_size(px(10.))
                                .text_color(t.muted)
                                .cursor_pointer()
                                .hover(|s| s.bg(t.hover).text_color(t.text))
                                .on_click(cx.listener(|app, _, _, cx| app.new_ai_chat(cx)))
                                .child("New"),
                        )
                    })
                    .child(
                        div()
                            .id("agent-ai-settings")
                            .px_1p5()
                            .py_0p5()
                            .rounded(px(4.))
                            .text_size(px(10.))
                            .text_color(t.muted)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover).text_color(t.text))
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.settings_open = true;
                                app.settings_tab = SettingsTab::Ai;
                                app.agent_input_focused = false;
                                cx.notify();
                            }))
                            .child(if self.ai_page_context { "Context: on" } else { "URL only" }),
                    ),
            );

        let mut messages = div()
            .id("agent-ai-messages")
            .flex_1()
            .overflow_y_scroll()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_2();

        if !self.ai_has_key {
            messages = messages.child(
                div()
                    .p_3()
                    .rounded(px(9.))
                    .bg(t.raised)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_size(px(12.)).text_color(t.text).child("Chat alongside any page"))
                    .child(div().text_size(px(11.)).text_color(t.muted).child(
                        "Connect OpenAI, Ollama, LM Studio, or any OpenAI-compatible model to ask questions or summarize the active tab.",
                    ))
                    .child(
                        div().flex().child(
                            div()
                                .id("agent-configure-ai")
                                .px_2p5()
                                .py_1()
                                .rounded(px(6.))
                                .bg(t.accent)
                                .text_size(px(11.))
                                .text_color(gpui::rgb(0xffffff))
                                .cursor_pointer()
                                .hover(|s| s.opacity(0.9))
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.settings_open = true;
                                    app.settings_tab = SettingsTab::Ai;
                                    app.agent_input_focused = false;
                                    cx.notify();
                                }))
                                .child("Configure AI in Settings…"),
                        ),
                    ),
            );
        } else if let Some(chat) = chat {
            if chat.messages.is_empty() {
                let copy = if self.ai_page_context {
                    "Ask about the active page side-by-side. URL, title, and up to 20,000 chars of visible text are sent to your configured endpoint."
                } else {
                    "Ask about the active page side-by-side. Only URL, title, and your question are sent (page text is off in Settings)."
                };
                messages = messages
                    .child(div().text_size(px(11.)).text_color(t.muted).child(copy))
                    .child(
                        div().flex().child(
                            div()
                                .id("ai-summarize")
                                .px_2p5()
                                .py_1()
                                .rounded(px(6.))
                                .bg(t.well)
                                .text_size(px(11.))
                                .text_color(t.text)
                                .cursor_pointer()
                                .hover(|s| s.bg(t.hover))
                                .on_click(cx.listener(|app, _, _, cx| app.summarize_page(cx)))
                                .child("Summarize this page"),
                        ),
                    );
            }
            for (index, (user, text)) in chat.messages.iter().enumerate() {
                messages = messages.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_2p5()
                        .rounded(px(8.))
                        .bg(if *user { t.well } else { t.raised })
                        .child(div().text_size(px(9.)).text_color(t.muted).child(if *user { "YOU" } else { "ASSISTANT" }))
                        .child(div().text_size(px(12.)).text_color(t.text).child(SharedString::from(text.clone())))
                        .when(!*user, |d| {
                            let snippet = text.clone();
                            d.child(
                                div().flex().child(
                                    div()
                                        .id(SharedString::from(format!("ai-copy-{index}")))
                                        .mt_0p5()
                                        .text_size(px(10.))
                                        .text_color(t.muted)
                                        .cursor_pointer()
                                        .hover(|s| s.text_color(t.text))
                                        .on_click(cx.listener(move |app, _, _, cx| app.copy_ai_text(snippet.clone(), cx)))
                                        .child("Copy"),
                                ),
                            )
                        }),
                );
            }
            if chat.busy {
                messages = messages.child(div().text_size(px(11.)).text_color(t.muted).child("Thinking…"));
            }
            if let Some(error) = &chat.error {
                messages = messages.child(div().text_size(px(11.)).text_color(t.error).child(SharedString::from(error.clone())));
            }
        }

        // Collapsible MCP & Activity drawer so technical config stays quiet until needed.
        let mut mcp_drawer = div()
            .border_t_1()
            .border_color(t.border)
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .id("agent-mcp-toggle")
                    .flex()
                    .items_center()
                    .justify_between()
                    .cursor_pointer()
                    .hover(|s| s.text_color(t.text))
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.agent_mcp_expanded = !app.agent_mcp_expanded;
                        cx.notify();
                    }))
                    .child(div().text_size(px(11.)).text_color(t.muted).child(if self.agent_mcp_expanded {
                        "▾ MCP & recent activity"
                    } else {
                        "▸ MCP & recent activity"
                    }))
                    .child(div().text_size(px(10.)).text_color(t.muted).child(SharedString::from(if !recent_audit.is_empty() {
                        format!("{} recent", recent_audit.len())
                    } else {
                        shell_write_summary(self.shell_control, granted)
                    }))),
            );
        if self.agent_mcp_expanded {
            if !recent_audit.is_empty() {
                let mut log = div().flex().flex_col().gap_0p5();
                for row in &recent_audit {
                    let host_label = if row.host.is_empty() { "shell" } else { row.host.as_str() };
                    log = log.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .text_size(px(10.))
                            .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(t.text).child(SharedString::from(format!("{} · {host_label}", row.tool))))
                            .child(div().text_color(if row.outcome == "ok" { t.muted } else { t.warn }).child(SharedString::from(row.outcome.clone()))),
                    );
                }
                mcp_drawer = mcp_drawer.child(log);
            }
            for c in &clients {
                let name = c.name.clone();
                let is_granted = c.granted;
                mcp_drawer = mcp_drawer.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(div().text_size(px(11.)).text_color(t.text).child(c.name.clone()))
                        .child(
                            div()
                                .id(SharedString::from(format!("agent-grant-{}", c.name)))
                                .px_2()
                                .py_0p5()
                                .rounded(px(4.))
                                .bg(t.well)
                                .text_size(px(10.))
                                .text_color(if is_granted { t.accent } else { t.text })
                                .cursor_pointer()
                                .hover(|s| s.bg(t.hover))
                                .child(shell_grant_action(is_granted))
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    if let Some(store) = &app.store {
                                        let _ = store.set_mcp_client_granted(&name, !is_granted, now_unix());
                                    }
                                    cx.notify();
                                })),
                        ),
                );
            }
            mcp_drawer = mcp_drawer
                .child(
                    div()
                        .id("agent-copy-setup")
                        .px_2()
                        .py_1()
                        .rounded(px(5.))
                        .bg(t.well)
                        .font_family("Menlo")
                        .text_size(px(10.))
                        .text_color(t.muted)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover).text_color(t.text))
                        .on_click(cx.listener(|app, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(app.mcp_setup.clone()));
                            app.toast("Copied the MCP setup command");
                            cx.notify();
                        }))
                        .child(SharedString::from(format!("Copy: {}", self.mcp_setup))),
                )
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(t.muted)
                        .child("Page content is untrusted data. Pause and site policy gate every MCP tool call."),
                );
        }

        // Docked chat composer at the bottom of the Agent sidebar (when AI key is set).
        let composer = if self.ai_has_key {
            let input_text = chat.map(|c| c.input.as_str()).unwrap_or("");
            let busy = chat.is_some_and(|c| c.busy);
            Some(
                div()
                    .p_3()
                    .border_t_1()
                    .border_color(t.border)
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("agent-chat-input")
                            .flex_1()
                            .min_w_0()
                            .h(px(32.))
                            .px_2p5()
                            .flex()
                            .items_center()
                            .rounded(px(7.))
                            .bg(t.well)
                            .when(self.agent_input_focused, |d| d.border_1().border_color(t.accent))
                            .text_size(px(12.))
                            .text_color(if input_text.is_empty() { t.muted } else { t.text })
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .cursor_text()
                            .on_click(cx.listener(|app, _, _, cx| app.focus_agent_input(cx)))
                            .child(SharedString::from(if input_text.is_empty() {
                                "Ask about this page…".to_string()
                            } else {
                                input_text.to_string()
                            }))
                            .when(self.agent_input_focused, |d| d.child(div().ml_0p5().text_color(t.accent).child("▏"))),
                    )
                    .child(if busy {
                        div()
                            .id("ai-stop")
                            .px_2p5()
                            .py_1()
                            .rounded(px(6.))
                            .bg(t.well)
                            .text_size(px(11.))
                            .text_color(t.text)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(|app, _, _, cx| app.stop_ai(cx)))
                            .child("Stop")
                            .into_any_element()
                    } else {
                        div()
                            .id("ai-send")
                            .px_2p5()
                            .py_1()
                            .rounded(px(6.))
                            .bg(t.accent)
                            .text_size(px(11.))
                            .text_color(gpui::rgb(0xffffff))
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.9))
                            .on_click(cx.listener(|app, _, _, cx| app.send_ai(cx)))
                            .child("Send")
                            .into_any_element()
                    }),
            )
        } else {
            None
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(status_card)
            .when(!agent_tabs.is_empty(), |d| d.child(agent_tabs_section))
            .child(assistant_header)
            .child(messages)
            .child(mcp_drawer)
            .when_some(composer, |d, c| d.child(c))
            .into_any_element()
    }

    fn render_inspector_pane(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let mut tabs = div().flex().gap_1().px_2().h(px(32.)).items_center();
        for p in Panel::ALL {
            let on = p == self.panel;
            tabs = tabs.child(
                div()
                    .id(p.title())
                    .px_2()
                    .py_0p5()
                    .rounded(px(4.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .text_color(if on { t.text } else { t.muted })
                    .when(on, |d| d.bg(t.active))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.panel = p;
                        if p == Panel::Cookies {
                            if let Some(tab) = app.active() {
                                app.web.borrow_mut().apply(BrowserRequest::RefreshCookies(tab));
                            }
                        }
                        cx.notify();
                    }))
                    .child(p.title()),
            );
        }
        tabs = tabs.child(div().flex_1());
        if self.panel == Panel::Network {
            let failed_only = self.network_failed_only;
            tabs = tabs
                .child(
                    div()
                        .id("network-filter")
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.))
                        .text_size(px(11.))
                        .cursor_pointer()
                        .text_color(if failed_only { t.error } else { t.muted })
                        .when(failed_only, |d| d.bg(t.active))
                        .hover(|s| s.bg(t.hover))
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.network_failed_only = !app.network_failed_only;
                            cx.notify();
                        }))
                        .child(if failed_only { "Failed" } else { "All" }),
                )
                .child(
                    div()
                        .id("network-clear")
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.))
                        .text_size(px(11.))
                        .cursor_pointer()
                        .text_color(t.muted)
                        .hover(|s| s.bg(t.hover).text_color(t.text))
                        .on_click(cx.listener(|app, _, _, cx| {
                            if let Some(tab) = app.active() {
                                app.hub.update(|h| {
                                    h.clear_network(tab);
                                });
                                app.selected_network = None;
                                app.inspector_cache = None;
                                cx.notify();
                            }
                        }))
                        .child("Clear"),
                );
        } else if self.panel == Panel::Console {
            let level = self.console_min;
            let active_filter = level != ConsoleLevel::Log;
            tabs = tabs
                .child(
                    div()
                        .id("console-filter")
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.))
                        .text_size(px(11.))
                        .cursor_pointer()
                        .text_color(match level {
                            ConsoleLevel::Error => t.error,
                            ConsoleLevel::Warn => t.warn,
                            _ => t.muted,
                        })
                        .when(active_filter, |d| d.bg(t.active))
                        .hover(|s| s.bg(t.hover))
                        .on_click(cx.listener(|app, _, _, cx| {
                            app.console_min = next_console_min(app.console_min);
                            cx.notify();
                        }))
                        .child(console_filter_label(level)),
                )
                .child(
                    div()
                        .id("console-clear")
                        .px_2()
                        .py_0p5()
                        .rounded(px(4.))
                        .text_size(px(11.))
                        .cursor_pointer()
                        .text_color(t.muted)
                        .hover(|s| s.bg(t.hover).text_color(t.text))
                        .on_click(cx.listener(|app, _, _, cx| {
                            if let Some(tab) = app.active() {
                                app.hub.update(|h| {
                                    h.clear_console(tab);
                                });
                                app.inspector_cache = None;
                                cx.notify();
                            }
                        }))
                        .child("Clear"),
                );
        } else if self.panel == Panel::Cookies {
            tabs = tabs.child(
                div()
                    .id("cookies-refresh")
                    .px_2()
                    .py_0p5()
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .cursor_pointer()
                    .text_color(t.muted)
                    .hover(|s| s.bg(t.hover).text_color(t.text))
                    .on_click(cx.listener(|app, _, _, cx| {
                        if let Some(tab) = app.active() {
                            app.web.borrow_mut().apply(BrowserRequest::RefreshCookies(tab));
                            cx.notify();
                        }
                    }))
                    .child("Refresh"),
            );
        }
        let body = self.render_panel_body(cx);
        let detail = self.render_inspector_detail(cx);
        div()
            .flex()
            .flex_col()
            .h_full()
            .child(tabs)
            .child(div().id("inspector-body").flex_1().overflow_y_scroll().p_1().text_size(px(12.)).child(body))
            .when_some(detail, |d, drawer| d.child(drawer))
            .into_any_element()
    }

    fn build_inspector_rows(&self, tab: TabId) -> InspectorRows {
        let h = self.hub.lock();
        let Some(tab) = h.tab(tab) else { return InspectorRows::Empty("No tab") };
        match self.panel {
            Panel::Elements => {
                let rows = self.tree.rows(tab, 1500);
                if rows.is_empty() { InspectorRows::Empty("Waiting for page…") } else { InspectorRows::Elements(rows) }
            }
            Panel::Network => {
                let rows = if self.network_failed_only {
                    inspector::network_rows_filtered(tab, "", true)
                } else {
                    inspector::network_rows(tab, "")
                };
                if rows.is_empty() {
                    let empty = if self.network_failed_only { "No failed requests" } else { "No requests yet" };
                    InspectorRows::Empty(empty)
                } else {
                    InspectorRows::Network(rows.into_iter().take(500).collect())
                }
            }
            Panel::Console => {
                let rows = inspector::console_rows(tab, self.console_min);
                if rows.is_empty() {
                    InspectorRows::Empty("Console is empty")
                } else {
                    InspectorRows::Console(rows.into_iter().rev().take(500).collect())
                }
            }
            Panel::Cookies => {
                if tab.cookies.is_empty() {
                    InspectorRows::Empty("No cookies for this site")
                } else {
                    InspectorRows::Cookies(tab.cookies.clone())
                }
            }
        }
    }

    /// Build the panel rows only when the version, tab, panel, expansion, or
    /// filters changed; painting a frame reuses the cached strings.
    fn inspector_rows(&mut self) -> &InspectorRows {
        let tab = self.active();
        let valid = self.inspector_cache.as_ref().is_some_and(|c| {
            c.version == self.seen_version
                && c.tab == tab.unwrap_or(0)
                && c.panel == self.panel
                && c.tree_epoch == self.tree_epoch
                && c.console_min == self.console_min
                && c.failed_only == self.network_failed_only
        });
        if !valid {
            let rows = match tab {
                Some(tab) => self.build_inspector_rows(tab),
                None => InspectorRows::Empty("No tab"),
            };
            self.inspector_cache = Some(InspectorCache {
                version: self.seen_version,
                tab: tab.unwrap_or(0),
                panel: self.panel,
                tree_epoch: self.tree_epoch,
                console_min: self.console_min,
                failed_only: self.network_failed_only,
                rows,
            });
        }
        &self.inspector_cache.as_ref().expect("cache was just filled").rows
    }

    fn render_panel_body(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let Some(tab_id) = self.active() else {
            return div().p_2().text_color(t.muted).child("No tab").into_any_element();
        };
        let load_seq = self.hub.lock().tab(tab_id).map(|s| s.load_seq).unwrap_or(0);
        let selected_el = self.selected_element.and_then(|(tid, seq, id)| (tid == tab_id && seq == load_seq).then_some(id));
        let selected_net = self.selected_network.as_ref().and_then(|(tid, seq, id)| (*tid == tab_id && *seq == load_seq).then(|| id.clone()));
        let rows = self.inspector_rows();
        let mut col = div().flex().flex_col();
        match rows {
            InspectorRows::Empty(message) => return div().p_2().text_color(t.muted).child(*message).into_any_element(),
            InspectorRows::Elements(rows) => {
                for r in rows {
                    let id = r.id;
                    let has_children = r.has_children;
                    let selected = selected_el == Some(id);
                    let marker = if !has_children { "  " } else if r.expanded { "▾ " } else { "▸ " };
                    col = col.child(
                        div()
                            .id(("el", id as usize))
                            .pl(px(4. + 12. * r.depth as f32))
                            .h(px(18.))
                            .whitespace_nowrap()
                            .text_color(t.code)
                            .when(selected, |d| d.bg(t.active))
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_hover(cx.listener(move |app, hovered: &bool, _, _| {
                                app.eval_op(tab_id, if *hovered { Op::Highlight { target: id } } else { Op::Unhighlight });
                            }))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.selected_element = Some((tab_id, load_seq, id));
                                if has_children {
                                    app.tree.toggle(id);
                                    app.tree_epoch = app.tree_epoch.wrapping_add(1);
                                }
                                cx.notify();
                            }))
                            .child(SharedString::from(format!("{marker}{}", r.label))),
                    );
                }
            }
            InspectorRows::Network(rows) => {
                for (i, r) in rows.iter().enumerate() {
                    let req_id = r.id.clone();
                    let selected = selected_net.as_deref() == Some(req_id.as_str());
                    col = col.child(
                        div()
                            .id(("net", i))
                            .flex()
                            .gap_2()
                            .h(px(18.))
                            .px_1()
                            .cursor_pointer()
                            .when(selected, |d| d.bg(t.active))
                            .hover(|s| s.bg(t.hover))
                            .text_color(if r.failed { t.error } else { t.text })
                            .on_click(cx.listener(move |app, _, _, cx| {
                                if app.selected_network.as_ref().is_some_and(|(tid, seq, id)| *tid == tab_id && *seq == load_seq && id == &req_id) {
                                    app.selected_network = None;
                                } else {
                                    app.selected_network = Some((tab_id, load_seq, req_id.clone()));
                                }
                                cx.notify();
                            }))
                            .child(div().w(px(36.)).child(SharedString::from(r.method.clone())))
                            .child(div().w(px(32.)).child(SharedString::from(r.status.clone())))
                            .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(SharedString::from(r.url.clone())))
                            .child(div().text_color(t.muted).child(SharedString::from(r.time.clone()))),
                    );
                }
            }
            InspectorRows::Console(rows) => {
                for (i, r) in rows.iter().enumerate() {
                    let color = match r.level {
                        ConsoleLevel::Error => t.error,
                        ConsoleLevel::Warn => t.warn,
                        _ => t.text,
                    };
                    let copy_text = r.text.clone();
                    col = col.child(
                        div()
                            .id(("con", i))
                            .px_1()
                            .py_0p5()
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .text_color(color)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy_text.clone()));
                                app.toast("Copied console message");
                                cx.notify();
                            }))
                            .child(SharedString::from(r.text.clone()))
                            .when(!r.source.is_empty(), |d| d.child(div().text_color(t.muted).child(SharedString::from(r.source.clone())))),
                    );
                }
            }
            InspectorRows::Cookies(rows) => {
                for (i, c) in rows.iter().enumerate() {
                    let pair = format!("{}={}", c.name, c.value);
                    col = col.child(
                        div()
                            .id(("ck", i))
                            .px_1()
                            .py_0p5()
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(pair.clone()));
                                app.toast("Copied cookie");
                                cx.notify();
                            }))
                            .child(SharedString::from(format!("{} = {}", c.name, truncate(&c.value, 80))))
                            .child(div().text_color(t.muted).child(SharedString::from(format!(
                                "{}{}  {}{}",
                                c.domain,
                                c.path,
                                if c.secure { "Secure " } else { "" },
                                if c.http_only { "HttpOnly" } else { "" }
                            )))),
                    );
                }
            }
        }
        col.into_any_element()
    }

    fn render_inspector_detail(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let t = self.theme;
        let tab_id = self.active()?;
        let btn = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_1p5()
                .py_0p5()
                .rounded(px(4.))
                .text_size(px(10.))
                .text_color(t.text)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .child(label)
        };
        let load_seq = self.hub.lock().tab(tab_id).map(|s| s.load_seq).unwrap_or(0);
        match self.panel {
            Panel::Elements => {
                let id = self.selected_element.and_then(|(tid, seq, id)| (tid == tab_id && seq == load_seq).then_some(id))?;
                let detail = {
                    let h = self.hub.lock();
                    inspector::element_detail(h.tab(tab_id)?, id)?
                };
                let selector = detail.selector.clone();
                let mut box_el = div()
                    .id("element-detail")
                    .bg(t.background)
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .max_h(px(190.))
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(11.))
                                    .text_color(t.code)
                                    .child(SharedString::from(format!("{} ({} children)", detail.selector, detail.child_count))),
                            )
                            .child(
                                div()
                                    .id("el-detail-close")
                                    .px_1()
                                    .cursor_pointer()
                                    .text_color(t.muted)
                                    .hover(|s| s.text_color(t.text))
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.selected_element = None;
                                        cx.notify();
                                    }))
                                    .child("×"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(btn("el-scroll", "Scroll").on_click(cx.listener(move |app, _, _, _| {
                                app.eval_op(tab_id, Op::ScrollIntoView { target: id });
                            })))
                            .child(btn("el-copy-sel", "Copy selector").on_click(cx.listener(move |app, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(selector.clone()));
                                app.toast("Copied CSS selector");
                                cx.notify();
                            })))
                            .child(btn("el-hide", "Hide").on_click(cx.listener(move |app, _, _, cx| {
                                app.eval_op(tab_id, Op::EditStyle { target: id, css: "display: none !important".into() });
                                app.toast("Hidden on this page");
                                cx.notify();
                            }))),
                    );
                if !detail.text.is_empty() {
                    box_el = box_el.child(div().text_size(px(10.)).text_color(t.text).child(SharedString::from(truncate(&detail.text, 160))));
                }
                for (k, v) in detail.attrs.iter().take(12) {
                    box_el = box_el.child(
                        div()
                            .text_size(px(10.))
                            .text_color(t.muted)
                            .child(SharedString::from(format!("{k}={}", truncate(v, 100)))),
                    );
                }
                Some(box_el.into_any_element())
            }
            Panel::Network => {
                let req_id = self.selected_network.as_ref().and_then(|(tid, seq, id)| (*tid == tab_id && *seq == load_seq).then_some(id.as_str()))?;
                let row = {
                    let cache = self.inspector_cache.as_ref()?;
                    let InspectorRows::Network(rows) = &cache.rows else { return None };
                    rows.iter().find(|r| r.id == req_id)?.clone()
                };
                let curl = inspector::network_to_curl(&row);
                let full_url = row.full_url.clone();
                let resp_copy = row.response_body.clone();
                let mut meta = format!("{} · {} · {}", row.api, row.status, row.time);
                if let Some(sz) = row.transfer_size {
                    meta.push_str(&format!(" · {}", human_bytes(sz)));
                }
                if let Some(ct) = &row.content_type {
                    meta.push_str(&format!(" · {}", truncate(ct, 40)));
                }
                let mut drawer = div()
                    .id("network-detail")
                    .bg(t.background)
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .max_h(px(230.))
                    .overflow_y_scroll()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(11.))
                                    .text_color(if row.failed { t.error } else { t.text })
                                    .child(SharedString::from(format!("{} {}", row.method, row.full_url))),
                            )
                            .child(
                                div()
                                    .id("net-detail-close")
                                    .px_1()
                                    .cursor_pointer()
                                    .text_color(t.muted)
                                    .hover(|s| s.text_color(t.text))
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.selected_network = None;
                                        cx.notify();
                                    }))
                                    .child("×"),
                            ),
                    )
                    .child(div().text_size(px(10.)).text_color(t.muted).child(SharedString::from(meta)))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .child(btn("net-copy-curl", "Copy as cURL").on_click(cx.listener(move |app, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(curl.clone()));
                                app.toast("Copied cURL command");
                                cx.notify();
                            })))
                            .child(btn("net-copy-url", "Copy URL").on_click(cx.listener(move |app, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(full_url.clone()));
                                app.toast("Copied request URL");
                                cx.notify();
                            })))
                            .when_some(resp_copy, |d, body| {
                                d.child(btn("net-copy-body", "Copy body").on_click(cx.listener(move |app, _, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(body.clone()));
                                    app.toast("Copied response body");
                                    cx.notify();
                                })))
                            }),
                    );
                if let Some(err) = &row.error {
                    drawer = drawer.child(div().text_size(px(10.)).text_color(t.error).child(SharedString::from(format!("Error: {err}"))));
                }
                if let Some(req) = row.request_body.as_deref().filter(|s| !s.is_empty()) {
                    drawer = drawer
                        .child(div().text_size(px(10.)).text_color(t.muted).child("Request body"))
                        .child(div().text_size(px(10.)).text_color(t.code).child(SharedString::from(truncate(req, 400))));
                }
                if let Some(resp) = row.response_body.as_deref().filter(|s| !s.is_empty()) {
                    drawer = drawer
                        .child(div().text_size(px(10.)).text_color(t.muted).child("Response body"))
                        .child(div().text_size(px(10.)).text_color(t.code).child(SharedString::from(truncate(resp, 600))));
                }
                Some(drawer.into_any_element())
            }
            Panel::Console | Panel::Cookies => None,
        }
    }
}
