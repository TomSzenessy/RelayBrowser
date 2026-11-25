//! Dockable Agent panel (`Web` AI Companion, native `API` chat, and local `MCP`
//! agent controls). Can be dragged to Left, Right, Top, or Bottom, or stacked
//! with the Tab Strip. Native WebKit Web Inspector (`⌘⌥I` / right-click → Inspect)
//! replaces the old monospace Inspector tab.

use super::*;

impl RelayApp {
    pub(super) fn render_inspector(&mut self, stacked: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let edge = self.agent_edge;
        let vertical = edge.is_vertical();
        let has_agent_tabs = self.hub.lock().tabs().any(|tab| tab.owner == Owner::Agent);
        let hide_shortcut = self.shortcut_for(ActionId::AskAi);

        let drag_grip = div()
            .id("agent-drag-handle")
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
                    label: "Drag to dock Agent on any edge".into(),
                    shortcut: "".into(),
                    theme: t,
                })
                .into()
            })
            .on_drag(PanelDrag(PanelKind::Agent), move |_, _, _, cx| {
                cx.new(|_| PanelDragGhost { label: "Agent".into(), theme: t })
            })
            .child("⋮⋮");

        let mut switcher = div().flex().items_center().gap_0p5().p(px(2.)).rounded(px(8.)).bg(t.well);
        for mode in AgentMode::ALL {
            let on = self.agent_mode == mode;
            let tip_text: SharedString = match mode {
                AgentMode::Web => "Claude, ChatGPT, or Gemini using your login".into(),
                AgentMode::Api => "Direct API chat (Claude, OpenAI, Ollama, LM Studio)".into(),
                AgentMode::Mcp => "Local coding agents connected via MCP".into(),
            };
            switcher = switcher.child(
                div()
                    .id(SharedString::from(format!("agent-mode-{}", mode.as_str())))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2p5()
                    .py_1()
                    .rounded(px(6.))
                    .text_size(px(11.))
                    .cursor_pointer()
                    .text_color(if on { t.text } else { t.muted })
                    .when(on, |d| d.bg(t.surface).shadow_sm())
                    .hover(|s| s.text_color(t.text))
                    .tooltip(move |_, cx| {
                        let label = tip_text.clone();
                        cx.new(|_| ShortcutTooltip { label, shortcut: "".into(), theme: t }).into()
                    })
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.set_agent_mode(mode, cx);
                    }))
                    .child(mode.label())
                    .when(mode == AgentMode::Mcp && has_agent_tabs, |d| {
                        d.child(div().size(px(6.)).rounded_full().bg(t.agent))
                    }),
            );
        }

        let settings_btn = div()
            .id("agent-header-settings")
            .w(px(24.))
            .h(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .text_size(px(12.))
            .text_color(t.muted)
            .cursor_pointer()
            .hover(|s| s.bg(t.hover).text_color(t.text))
            .tooltip(move |_, cx| {
                cx.new(|_| ShortcutTooltip {
                    label: "AI & Agent Settings".into(),
                    shortcut: "⌘,".into(),
                    theme: t,
                })
                .into()
            })
            .on_click(cx.listener(|app, _, _, cx| {
                app.open_connect(cx);
            }))
            .child("⚙");

        let collapse_btn = div()
            .id("sidebar-collapse")
            .w(px(24.))
            .h(px(24.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .tooltip(move |_, cx| {
                let sc = hide_shortcut.clone();
                cx.new(|_| ShortcutTooltip { label: "Hide Agent".into(), shortcut: sc, theme: t }).into()
            })
            .on_click(cx.listener(|app, _, _, cx| {
                app.toggle_agent_sidebar(cx);
            }))
            .child(chrome::sidebar_icon(edge, true, t));

        let head = div()
            .id("agent-panel-header")
            .flex()
            .items_center()
            .justify_between()
            .gap_1()
            .px_2()
            .pt(if stacked { px(4.) } else if vertical { px(34.) } else if edge == tabstrip::Placement::Top { px(32.) } else { px(6.) })
            .pb_1p5()
            .on_drag(PanelDrag(PanelKind::Agent), move |_, _, _, cx| {
                cx.new(|_| PanelDragGhost { label: "Agent".into(), theme: t })
            })
            .child(div().flex().items_center().gap_1().child(drag_grip).child(switcher))
            .child(div().flex().items_center().gap_0p5().child(settings_btn).child(collapse_btn));

        let body = match self.agent_mode {
            AgentMode::Web => self.render_agent_web_pane(cx),
            AgentMode::Api => self.render_agent_api_pane(cx),
            AgentMode::Mcp => self.render_agent_mcp_pane(cx),
        };

        let mut panel = div().flex().flex_col().overflow_hidden();
        if stacked {
            panel = panel.flex_1().min_w_0().min_h_0().w_full().h_full();
        } else if vertical {
            panel = panel.w(px(self.agent_width)).h_full();
        } else {
            panel = panel.w_full().h(px(self.agent_height));
        }
        panel.child(head).child(body).into_any_element()
    }

    fn render_agent_web_pane(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let provider = self.web_ai_provider;
        let url = provider.url().to_string();
        let copy_sc = self.shortcut_for(ActionId::CopyMarkdown);

        let mut providers = div().flex().items_center().gap_1();
        for p in WebAiProvider::ALL {
            let on = provider == p;
            providers = providers.child(
                div()
                    .id(SharedString::from(format!("web-ai-{}", p.as_str())))
                    .px_2()
                    .py_0p5()
                    .rounded(px(6.))
                    .text_size(px(11.))
                    .cursor_pointer()
                    .text_color(if on { t.text } else { t.muted })
                    .bg(if on { t.active } else { t.well })
                    .hover(|s| s.bg(t.hover).text_color(t.text))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.set_web_ai_provider(p, cx);
                    }))
                    .child(p.label()),
            );
        }

        let open_url = url.clone();
        let toolbar = div()
            .flex()
            .items_center()
            .justify_between()
            .gap_1()
            .px_2()
            .pb_1p5()
            .child(providers)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .id("web-ai-copy-page")
                            .px_2()
                            .py_0p5()
                            .rounded(px(6.))
                            .bg(t.well)
                            .text_size(px(10.))
                            .text_color(t.text)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .tooltip(move |_, cx| {
                                let sc = copy_sc.clone();
                                cx.new(|_| ShortcutTooltip {
                                    label: "Copy active page as Markdown to paste into chat".into(),
                                    shortcut: sc,
                                    theme: t,
                                })
                                .into()
                            })
                            .on_click(cx.listener(|app, _, _, cx| {
                                app.copy_markdown(cx);
                            }))
                            .child("Copy Page"),
                    )
                    .child(
                        div()
                            .id("web-ai-back")
                            .w(px(22.))
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(t.well)
                            .text_size(px(11.))
                            .text_color(t.muted)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover).text_color(t.text))
                            .tooltip(move |_, cx| {
                                cx.new(|_| ShortcutTooltip { label: "Back".into(), shortcut: "".into(), theme: t }).into()
                            })
                            .on_click(cx.listener(|app, _, _, _| {
                                app.web.borrow().companion_go_back();
                            }))
                            .child("←"),
                    )
                    .child(
                        div()
                            .id("web-ai-reload")
                            .w(px(22.))
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(t.well)
                            .text_size(px(11.))
                            .text_color(t.muted)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover).text_color(t.text))
                            .tooltip(move |_, cx| {
                                cx.new(|_| ShortcutTooltip { label: "Reload".into(), shortcut: "".into(), theme: t }).into()
                            })
                            .on_click(cx.listener(|app, _, _, _| {
                                app.web.borrow().companion_reload();
                            }))
                            .child("↻"),
                    )
                    .child(
                        div()
                            .id("web-ai-popout")
                            .w(px(22.))
                            .h(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.))
                            .bg(t.well)
                            .text_size(px(11.))
                            .text_color(t.muted)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover).text_color(t.text))
                            .tooltip(move |_, cx| {
                                cx.new(|_| ShortcutTooltip {
                                    label: "Open in a full browser tab".into(),
                                    shortcut: "".into(),
                                    theme: t,
                                })
                                .into()
                            })
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.open_url(open_url.clone(), true);
                                cx.notify();
                            }))
                            .child("↗"),
                    ),
            );

        let web = self.web.clone();
        let renders = web.borrow().renders_pages();
        let hide_companion = self.has_modal_overlay() || cx.has_active_drag() || self.resizing.is_some();
        let view_host = div()
            .flex_1()
            .min_h_0()
            .mx_2()
            .mb_2()
            .rounded(px(10.))
            .border_1()
            .border_color(t.border.opacity(0.45))
            .bg(t.surface)
            .overflow_hidden()
            .relative()
            .when(!renders, |d| {
                d.flex().items_center().justify_center().p_4().child(
                    div()
                        .text_size(px(12.))
                        .text_color(t.muted)
                        .child(SharedString::from(format!("{} Web AI renders inside WebKit on macOS.", provider.label()))),
                )
            })
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), _window, _cx| {
                        if hide_companion {
                            web.borrow().layout_companion(None);
                        } else {
                            web.borrow_mut().set_companion_url(&url);
                            web.borrow().layout_companion(Some(bounds));
                        }
                    },
                )
                .size_full(),
            );

        div().flex_1().min_h_0().flex().flex_col().child(toolbar).child(view_host).into_any_element()
    }

    fn render_agent_api_pane(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let chat = self.ai_chat.as_ref();
        let has_messages = chat.is_some_and(|c| !c.messages.is_empty());
        let is_claude = relay_extensions::ai::is_anthropic_endpoint(&self.ai_endpoint);
        let provider_badge: SharedString = if is_claude {
            format!("Claude · {}", self.ai_model).into()
        } else {
            self.ai_model.clone().into()
        };

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .px_2p5()
            .pb_1()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_size(px(10.))
                    .text_color(t.muted)
                    .child(provider_badge),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
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
                                app.agent_input_focused = false;
                                app.open_connect(cx);
                            }))
                            .child(if self.ai_page_context { "Context: on" } else { "URL only" }),
                    ),
            );

        let mut messages = div()
            .id("agent-ai-messages")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_2()
            .py_1()
            .flex()
            .flex_col()
            .gap_2();

        if !self.ai_has_key {
            messages = messages.child(
                div()
                    .p_3()
                    .rounded(px(10.))
                    .bg(t.surface)
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(div().text_size(px(12.)).text_color(t.text).child("Direct API & Local Model Chat"))
                    .child(div().text_size(px(11.)).text_color(t.muted).child(
                        "Connect Anthropic Claude, OpenAI, OpenRouter, Ollama, or LM Studio with automatic page context — or switch to Web mode to use your Claude / ChatGPT login with no API key.",
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id("agent-configure-ai")
                                    .px_2p5()
                                    .py_1()
                                    .rounded(px(6.))
                                    .bg(t.accent)
                                    .text_size(px(11.))
                                    .text_color(t.on_accent)
                                    .cursor_pointer()
                                    .hover(|s| s.opacity(0.9))
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.agent_input_focused = false;
                                        app.open_connect(cx);
                                    }))
                                    .child("Configure API…"),
                            )
                            .child(
                                div()
                                    .id("agent-switch-web")
                                    .px_2p5()
                                    .py_1()
                                    .rounded(px(6.))
                                    .bg(t.well)
                                    .text_size(px(11.))
                                    .text_color(t.text)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(t.hover))
                                    .on_click(cx.listener(|app, _, _, cx| {
                                        app.set_agent_mode(AgentMode::Web, cx);
                                    }))
                                    .child("Use Web AI"),
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

        let input_text = chat.map(|c| c.input.as_str()).unwrap_or("");
        let busy = chat.is_some_and(|c| c.busy);
        let composer = if self.ai_has_key {
            Some(
                div()
                    .mx_2()
                    .mb_2()
                    .mt_1()
                    .p_2()
                    .rounded(px(10.))
                    .bg(t.surface)
                    .when(self.agent_input_focused, |d| d.border_1().border_color(t.accent))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .id("agent-ai-input")
                            .flex_1()
                            .min_w_0()
                            .min_h(px(24.))
                            .flex()
                            .items_center()
                            .cursor_text()
                            .on_click(cx.listener(|app, _, _, cx| app.focus_agent_input(cx)))
                            .text_size(px(12.))
                            .text_color(if input_text.is_empty() { t.muted } else { t.text })
                            .child(SharedString::from(if input_text.is_empty() {
                                if self.agent_input_focused {
                                    "Ask about this page… ▏".to_string()
                                } else {
                                    "Click to ask about this page…".to_string()
                                }
                            } else if self.agent_input_focused {
                                format!("{input_text}▏")
                            } else {
                                input_text.to_string()
                            })),
                    )
                    .child(
                        div()
                            .id(if busy { "ai-stop" } else { "ai-send" })
                            .px_2p5()
                            .py_1()
                            .rounded(px(6.))
                            .bg(if busy { t.well } else { t.accent })
                            .text_size(px(11.))
                            .text_color(if busy { t.text } else { t.on_accent })
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.9))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                if busy {
                                    app.stop_ai(cx);
                                } else {
                                    app.send_ai(cx);
                                }
                            }))
                            .child(if busy { "Stop" } else { "Send" }),
                    ),
            )
        } else {
            None
        };

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(header)
            .child(messages)
            .when_some(composer, |d, c| d.child(c))
            .into_any_element()
    }

    fn render_agent_mcp_pane(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let pause_sc = self.shortcut_for(ActionId::ToggleAgents);
        let policy_sc = self.shortcut_for(ActionId::CycleAgentPolicy);
        let active_site = self
            .active()
            .and_then(|tab| self.hub.lock().tab(tab).map(|t| (t.url.clone(), t.title.clone())))
            .and_then(|(url, title)| {
                let host = relay_bridge::host_of(&url)?;
                let policy = self.hub.agent_policy_for_host(&host);
                Some((url, title, host, policy))
            });
        let paused = self.hub.agents_paused();
        let agent_tabs: Vec<(TabId, String, String, bool)> = self
            .hub
            .lock()
            .tabs()
            .filter(|tab| tab.owner == Owner::Agent)
            .map(|tab| (tab.id, tab.title.clone(), tab.url.clone(), tab.challenge.is_some()))
            .collect();
        let clients = self.store.as_ref().and_then(|s| s.mcp_clients().ok()).unwrap_or_default();
        let granted = clients.iter().filter(|c| c.granted).count();
        let shell_summary = shell_write_summary(self.shell_control, granted);
        let recent_audit = self.store.as_ref().and_then(|s| s.agent_audit(8).ok()).unwrap_or_default();

        let mut status_card = div()
            .p_2p5()
            .rounded(px(10.))
            .bg(t.surface)
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
                                "MCP Paused"
                            } else if self.mcp_enabled {
                                "MCP Server Ready"
                            } else {
                                "MCP Server Off"
                            }))
                            .child(div().text_size(px(10.)).text_color(t.muted).child(SharedString::from(format!("· shell: {shell_summary}")))),
                    )
                    .child(
                        div()
                            .id(if paused { "agent-resume" } else { "agent-pause" })
                            .px_2()
                            .py_0p5()
                            .rounded(px(6.))
                            .bg(if paused { t.active } else { t.well })
                            .text_size(px(11.))
                            .text_color(if paused { t.accent } else { t.text })
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .tooltip(move |_, cx| {
                                let sc = pause_sc.clone();
                                cx.new(|_| ShortcutTooltip {
                                    label: if paused { "Resume all agents".into() } else { "Pause all agents".into() },
                                    shortcut: sc,
                                    theme: t,
                                })
                                .into()
                            })
                            .on_click(cx.listener(|app, _, _, cx| app.toggle_agents(cx)))
                            .child(if paused { "Resume" } else { "Pause" }),
                    ),
            );
        if let Some((url, title, host, policy)) = active_site {
            status_card = status_card.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .child(self.site_icon(&url, &title, 14.))
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
                            ),
                    )
                    .child(
                        div()
                            .id("agent-cycle-policy")
                            .px_2()
                            .py_0p5()
                            .rounded(px(6.))
                            .bg(t.well)
                            .text_size(px(10.))
                            .text_color(t.text)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .tooltip(move |_, cx| {
                                let sc = policy_sc.clone();
                                cx.new(|_| ShortcutTooltip {
                                    label: "Cycle site policy (Read / Allow / Deny)".into(),
                                    shortcut: sc,
                                    theme: t,
                                })
                                .into()
                            })
                            .on_click(cx.listener(|app, _, _, cx| app.cycle_agent_policy(cx)))
                            .child(SharedString::from(policy.label())),
                    ),
            );
        }

        let mut list = div()
            .id("agent-mcp-scroll")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .px_2()
            .pb_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(status_card);

        if !agent_tabs.is_empty() {
            let mut tabs_card = div()
                .p_2p5()
                .rounded(px(10.))
                .bg(t.surface)
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_size(px(10.)).text_color(t.agent).child(SharedString::from(format!("AGENT TABS ({})", agent_tabs.len()))));
            for (id, title, url, needs_human) in &agent_tabs {
                let tid = *id;
                let label = if title.is_empty() { palette::short_url(url) } else { title.clone() };
                tabs_card = tabs_card.child(
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
            list = list.child(tabs_card);
        }

        let mut clients_card = div()
            .p_2p5()
            .rounded(px(10.))
            .bg(t.surface)
            .flex()
            .flex_col()
            .gap_1p5()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(px(10.)).text_color(t.muted).child("LOCAL CODING AGENTS"))
                    .child(
                        div()
                            .id("agent-connect")
                            .px_2()
                            .py_0p5()
                            .rounded(px(5.))
                            .bg(t.accent)
                            .text_size(px(10.))
                            .text_color(t.on_accent)
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.9))
                            .on_click(cx.listener(|app, _, _, cx| app.open_connect(cx)))
                            .child("Setup Guide…"),
                    ),
            );
        if clients.is_empty() {
            clients_card = clients_card.child(
                div()
                    .text_size(px(11.))
                    .text_color(t.muted)
                    .child("Connect Claude Code, Codex, Cursor, Zed, VS Code, or Gemini CLI via MCP so they can read DOM, network, console, and automate tabs. Click Setup Guide… for step-by-step instructions."),
            );
        } else {
            for client in clients {
                let name = client.name.clone();
                let granted = client.granted;
                clients_card = clients_card.child(
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
                                .text_color(t.text)
                                .child(SharedString::from(client.name)),
                        )
                        .child(
                            div()
                                .id(SharedString::from(format!("sidebar-grant-{name}")))
                                .px_1p5()
                                .py_0p5()
                                .rounded(px(4.))
                                .bg(if granted { t.active } else { t.well })
                                .text_size(px(10.))
                                .text_color(if granted { t.accent } else { t.muted })
                                .cursor_pointer()
                                .hover(|s| s.bg(t.hover).text_color(t.text))
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    if let Some(store) = &app.store {
                                        let _ = store.set_mcp_client_granted(&name, !granted, now_unix());
                                    }
                                    cx.notify();
                                }))
                                .child(shell_grant_action(granted)),
                        ),
                );
            }
        }
        list = list.child(clients_card);

        if !recent_audit.is_empty() {
            let mut audit_card = div()
                .p_2p5()
                .rounded(px(10.))
                .bg(t.surface)
                .flex()
                .flex_col()
                .gap_1()
                .child(div().text_size(px(10.)).text_color(t.muted).child("RECENT MCP ACTIVITY"));
            for entry in recent_audit {
                let detail = if entry.host.is_empty() { entry.tool } else { format!("{} · {}", entry.tool, entry.host) };
                audit_card = audit_card.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .text_size(px(10.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(t.text)
                                .child(SharedString::from(detail)),
                        )
                        .child(div().text_color(if entry.outcome == "ok" { t.muted } else { t.warn }).child(SharedString::from(entry.outcome))),
                );
            }
            list = list.child(audit_card);
        }

        list.into_any_element()
    }
}
