//! Modal overlays: Library, Downloads, Connect MCP, Shortcuts, Settings, and Find bar.

use super::*;

impl RelayApp {
    pub(super) fn render_library(&self, lib: &Library, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut list = div().id("library-list").flex().flex_col().py_1().max_h(px(400.)).overflow_y_scroll();
        if lib.rows.is_empty() {
            list = list.child(div().px_4().py_6().text_size(px(13.)).text_color(t.muted).child(lib.mode.empty()));
        }
        for (i, row) in lib.rows.iter().enumerate() {
            let on = i == lib.selected;
            list = list.child(
                div()
                    .id(("lib", i))
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(px(36.))
                    .px_3()
                    .mx_1()
                    .rounded(px(6.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.active))
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.open_library_row(i);
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .overflow_hidden()
                            .child(div().text_size(px(13.)).text_color(t.text).whitespace_nowrap().text_ellipsis().child(SharedString::from(row.title.clone())))
                            .child(div().text_size(px(11.)).text_color(t.muted).whitespace_nowrap().text_ellipsis().child(SharedString::from(row.note.clone()))),
                    )
                    .when(lib.mode == Mode::Bookmarks, |d| {
                        d.child(
                            div()
                                .id(("lib-move", i))
                                .px_1()
                                .h(px(20.))
                                .flex()
                                .items_center()
                                .rounded(px(4.))
                                .text_size(px(10.))
                                .text_color(t.muted)
                                .cursor_pointer()
                                .hover(|s| s.bg(t.hover).text_color(t.text))
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    cx.stop_propagation();
                                    let note = app.library.as_ref().and_then(|l| l.rows.get(i)).map(|r| r.note.clone()).unwrap_or_default();
                                    if let Some(lib) = app.library.as_mut() {
                                        lib.folder_input = Some(if note == "Bookmark" { String::new() } else { note });
                                    }
                                    cx.notify();
                                }))
                                .child("move"),
                        )
                    })
                    .child(
                        div()
                            .id(("lib-del", i))
                            .w(px(20.))
                            .h(px(20.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .text_size(px(14.))
                            .text_color(t.muted)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover).text_color(t.error))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                cx.stop_propagation();
                                app.delete_library_row(i, cx);
                            }))
                            .child("×"),
                    ),
            );
        }
        let title = if lib.query.is_empty() { format!("Search {}", lib.mode.title().to_lowercase()) } else { lib.query.clone() };
        let panel = div()
            .id("library")
            .w(px(680.))
            .flex()
            .flex_col()
            .rounded(px(10.))
            .bg(t.surface)
            .border_1()
            .border_color(t.border)
            .shadow_lg()
            .overflow_hidden()
            .on_mouse_down_out(cx.listener(|app, _, _, _| app.close_library()))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .h(px(40.))
                    .px_4()
                    .border_b_1()
                    .border_color(t.border)
                    .child(div().text_size(px(14.)).text_color(t.text).child(lib.mode.title()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .when(lib.mode == Mode::Bookmarks, |d| {
                                d.child(
                                    div()
                                        .id("lib-add")
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(t.border)
                                        .text_size(px(11.))
                                        .text_color(t.text)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover))
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            app.bookmark_current_in_library(cx);
                                        }))
                                        .child("+ Bookmark this page"),
                                )
                            })
                            .child(div().text_size(px(11.)).text_color(t.muted).child(lib.mode.hint())),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(px(42.))
                    .px_4()
                    .border_b_1()
                    .border_color(t.border)
                    .text_size(px(14.))
                    .text_color(if lib.query.is_empty() { t.muted } else { t.text })
                    .child(SharedString::from(title))
                    .child(div().w(px(1.5)).h(px(18.)).ml_0p5().bg(t.accent)),
            );
        let panel = if let Some(input) = &lib.folder_input {
            panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(38.))
                    .px_4()
                    .border_b_1()
                    .border_color(t.border)
                    .text_size(px(12.))
                    .text_color(t.text)
                    .child("Move to folder:")
                    .child(div().flex_1().child(SharedString::from(if input.is_empty() { "top level".to_string() } else { input.clone() })))
                    .child(div().text_color(t.muted).child("↩ saves · esc cancels")),
            )
        } else {
            panel
        };
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(80.)).bg(t.scrim).child(panel.child(list))
    }


    pub(super) fn render_downloads(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let downloads = self.hub.lock().downloads_in_panel_order();
        let active = downloads.iter().filter(|d| DownloadGroup::of(&d.state) == DownloadGroup::Active).count();
        let finished = downloads.len() - active;
        let mut rows = div().flex().flex_col();
        if downloads.is_empty() {
            rows = rows.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .px_4()
                    .py_8()
                    .child(div().text_size(px(13.)).text_color(t.text).child("No downloads yet"))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(t.muted)
                            .child(SharedString::from(format!("Downloads save to {FOLDER_LABEL}"))),
                    ),
            );
        }
        let mut shown: Option<DownloadGroup> = None;
        for (idx, d) in downloads.into_iter().enumerate() {
            let group = DownloadGroup::of(&d.state);
            if shown != Some(group) {
                let count = match group {
                    DownloadGroup::Active => active,
                    DownloadGroup::Finished => finished,
                };
                rows = rows.child(
                    div()
                        .px_4()
                        .pt_3()
                        .pb_1()
                        .text_size(px(10.))
                        .text_color(t.muted)
                        .child(SharedString::from(format!("{} ({count})", group.label().to_uppercase()))),
                );
                shown = Some(group);
            }
            let selected = idx == self.downloads_selected;
            let state = match &d.state {
                relay_bridge::DownloadState::Downloading => match (d.received, d.total) {
                    (Some(received), Some(total)) if total > 0 => {
                        format!("Downloading… {} / {} ({:.0}%)", human_bytes(received), human_bytes(total), received as f64 * 100.0 / total as f64)
                    }
                    _ => "Downloading…".to_string(),
                },
                relay_bridge::DownloadState::Done => "Done".to_string(),
                relay_bridge::DownloadState::Failed(e) => format!("Failed: {e}"),
            };
            let path = d.path.clone();
            let destination = match path.as_deref() {
                Some(path) => display_path(path),
                None => format!("Saving to {FOLDER_LABEL}…"),
            };
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("download-{}", d.id)))
                    .flex()
                    .items_center()
                    .gap_3()
                    .min_h(px(52.))
                    .px_4()
                    .py_1()
                    .border_b_1()
                    .border_color(t.border)
                    .when(selected, |el| el.bg(t.hover))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .overflow_hidden()
                            .child(div().text_size(px(12.)).text_color(t.text).whitespace_nowrap().text_ellipsis().child(SharedString::from(d.filename)))
                            .child(div().text_size(px(10.)).text_color(t.muted).whitespace_nowrap().text_ellipsis().child(SharedString::from(format!("{state} · {}", d.url))))
                            .child(div().text_size(px(10.)).text_color(t.muted).whitespace_nowrap().text_ellipsis().child(SharedString::from(destination))),
                    )
                    .child(
                        div()
                            .when(matches!(d.state, relay_bridge::DownloadState::Failed(_)), |el| {
                                el.child(
                                    div()
                                        .id(SharedString::from(format!("download-retry-{}", d.id)))
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(t.border)
                                        .text_size(px(10.))
                                        .text_color(t.text)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover))
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            app.web.borrow_mut().apply(BrowserRequest::Navigate { tab: d.tab, url: d.url.clone() });
                                            app.toast("Retrying download");
                                            cx.notify();
                                        }))
                                        .child("Retry"),
                                )
                            })
                            .when(d.state == relay_bridge::DownloadState::Downloading, |el| {
                                el.child(
                                    div()
                                        .id(SharedString::from(format!("download-cancel-{}", d.id)))
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(t.border)
                                        .text_size(px(10.))
                                        .text_color(t.text)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover))
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            app.web.borrow_mut().apply(BrowserRequest::CancelDownload { tab: d.tab, id: d.id });
                                            app.toast("Download cancelled");
                                            cx.notify();
                                        }))
                                        .child("Cancel"),
                                )
                            })
                            .when(path.is_some(), |el| {
                                el.child(
                                    div()
                                        .id(SharedString::from(format!("download-reveal-{}", d.id)))
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(t.border)
                                        .text_size(px(10.))
                                        .text_color(t.text)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover))
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            if let Some(path) = &path {
                                                app.reveal_download(path, cx);
                                            } else {
                                                cx.notify();
                                            }
                                        }))
                                        .child("Reveal"),
                                )
                            }),
                    ),
            );
        }
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(80.)).bg(t.scrim).child(
            div()
                .id("downloads")
                .w(px(680.))
                .flex()
                .flex_col()
                .rounded(px(10.))
                .bg(t.surface)
                .border_1()
                .border_color(t.border)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down_out(cx.listener(|app, _, _, _| app.close_downloads()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(44.))
                        .px_4()
                        .border_b_1()
                        .border_color(t.border)
                        .child(div().text_size(px(15.)).text_color(t.text).child("Downloads"))
                        .child(
                            div()
                                .id("downloads-clear")
                                .px_2()
                                .py_0p5()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(t.border)
                                .text_size(px(11.))
                                .text_color(t.muted)
                                .cursor_pointer()
                                .hover(|s| s.bg(t.hover))
                                .on_click(cx.listener(|app, _, _, cx| {
                                    let n = app.hub.update(|h| h.clear_finished_downloads());
                                    app.toast(format!("Cleared {n} finished downloads"));
                                    cx.notify();
                                }))
                                .child("Clear finished"),
                        ),
                )
                .child(rows),
        )
    }

    pub(super) fn render_connect(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut rows = div().id("connect-list").flex().flex_col().max_h(px(420.)).overflow_y_scroll();
        for &(client, detected) in &self.connect_detected {
            rows = rows.child(
                div()
                    .id(SharedString::from(format!("connect-{}", client.id())))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .min_h(px(46.))
                    .px_4()
                    .py_1()
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(div().text_size(px(13.)).text_color(t.text).child(client.name()))
                                    .when(detected, |d| {
                                        d.child(
                                            div()
                                                .px_1p5()
                                                .py_0p5()
                                                .rounded(px(4.))
                                                .bg(t.active)
                                                .text_size(px(10.))
                                                .text_color(t.accent)
                                                .child("Detected"),
                                        )
                                    }),
                            )
                            .child(div().text_size(px(10.)).text_color(t.muted).child(client.target())),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("connect-copy-{}", client.id())))
                            .px_2()
                            .py_1()
                            .rounded(px(5.))
                            .border_1()
                            .border_color(t.border)
                            .text_size(px(11.))
                            .text_color(t.text)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                let command = mcp_config::relay_command();
                                cx.write_to_clipboard(ClipboardItem::new_string(client.snippet(&command)));
                                app.toast(format!("Copied the {} config", client.name()));
                                cx.notify();
                            }))
                            .child("Copy config"),
                    ),
            );
        }
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(80.)).bg(t.scrim).child(
            div()
                .id("connect")
                .w(px(620.))
                .flex()
                .flex_col()
                .rounded(px(10.))
                .bg(t.surface)
                .border_1()
                .border_color(t.border)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down_out(cx.listener(|app, _, _, _| app.close_connect()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(44.))
                        .px_4()
                        .border_b_1()
                        .border_color(t.border)
                        .child(div().text_size(px(15.)).text_color(t.text).child("Connect an agent"))
                        .child(div().text_size(px(11.)).text_color(t.muted).child("Copy the text; Relay changes no files")),
                )
                .child(rows),
        )
    }

    pub(super) fn render_shortcuts(&self, editor: &ShortcutEditor, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut list = div().id("shortcuts-list").flex().flex_col().max_h(px(460.)).overflow_y_scroll();
        for (i, id) in ActionId::ALL.iter().copied().enumerate() {
            let recording = editor.recording == Some(id);
            list = list.child(
                div()
                    .id(("shortcut", i))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .h(px(32.))
                    .px_4()
                    .border_b_1()
                    .border_color(t.border)
                    .text_size(px(12.))
                    .when(recording, |d| d.bg(t.active))
                    .child(div().text_color(t.text).child(id.label()))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_color(t.muted).child(SharedString::from(keymap::display_key(self.keymap.key(id)))))
                            .child(
                                div()
                                    .id(("shortcut-change", i))
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(4.))
                                    .border_1()
                                    .border_color(if recording { t.accent } else { t.border })
                                    .cursor_pointer()
                                    .hover(|s| s.bg(t.hover))
                                    .on_click(cx.listener(move |app, _, _, cx| {
                                        app.start_recording(id, cx);
                                    }))
                                    .child(if recording { "Press keys…" } else { "Change" }),
                            ),
                    ),
            );
        }
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(60.)).bg(t.scrim).child(
            div()
                .id("shortcuts")
                .w(px(640.))
                .flex()
                .flex_col()
                .rounded(px(10.))
                .bg(t.surface)
                .border_1()
                .border_color(t.border)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down_out(cx.listener(|app, _, _, _| app.close_shortcuts()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(44.))
                        .px_4()
                        .border_b_1()
                        .border_color(t.border)
                        .child(div().text_size(px(15.)).text_color(t.text).child("Keyboard shortcuts"))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .id("shortcuts-reset")
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .border_1()
                                        .border_color(t.border)
                                        .text_size(px(11.))
                                        .text_color(t.muted)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover))
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            app.keymap.reset_all();
                                            app.apply_keymap(cx);
                                            app.toast("Shortcuts reset to defaults");
                                            cx.notify();
                                        }))
                                        .child("Reset all"),
                                )
                                .child(div().text_size(px(11.)).text_color(t.muted).child(if editor.recording.is_some() { "Press a shortcut · Esc cancels" } else { "Esc closes" })),
                        ),
                )
                .child(list),
        )
    }

    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let pill = |id: &'static str, label: SharedString, danger: bool| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded(px(6.))
                .border_1()
                .border_color(if danger { t.error } else { t.border })
                .bg(t.background)
                .text_size(px(12.))
                .text_color(if danger { t.error } else { t.text })
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .child(label)
        };
        let row = |title: &'static str, subtitle: Option<&'static str>, control: gpui::AnyElement| {
            let mut left = div().flex().flex_col().gap_0p5().child(div().text_size(px(13.)).text_color(t.text).child(title));
            if let Some(sub) = subtitle {
                left = left.child(div().text_size(px(11.)).text_color(t.muted).child(sub));
            }
            div().flex().items_center().justify_between().px_4().py_2p5().border_b_1().border_color(t.border).child(left).child(control)
        };
        let sleep_label: SharedString = if self.sleep_minutes == 0 { "Off".into() } else { format!("{} min", self.sleep_minutes).into() };
        let clear_label: SharedString = if self.confirm_clear_history { "Confirm clear history".into() } else { "Clear history".into() };
        let search_btn = pill("settings-search", palette::search_label(&self.search).into(), false)
            .on_click(cx.listener(|app, _, _, cx| {
                let idx = palette::SEARCH_PRESETS
                    .iter()
                    .position(|(_, url)| *url == app.search)
                    .map(|i| (i + 1) % palette::SEARCH_PRESETS.len())
                    .unwrap_or(0);
                app.search = palette::SEARCH_PRESETS[idx].1.to_string();
                if let Some(store) = &app.store {
                    let _ = store.set_setting("search", &app.search);
                }
                cx.notify();
            }))
            .into_any_element();
        let blocking_btn = pill("settings-blocking", if self.blocking { "On" } else { "Off" }.into(), false)
            .on_click(cx.listener(|app, _, _, cx| {
                app.blocking = !app.blocking;
                if let Some(store) = &app.store {
                    let _ = store.set_setting("blocking", if app.blocking { "on" } else { "off" });
                }
                if app.blocking {
                    app.web.borrow().load_content_rules(&rules_json(app.store.as_ref()));
                } else {
                    app.web.borrow().clear_content_rules();
                }
                cx.notify();
            }))
            .into_any_element();
        let sleep_btn = pill("settings-sleep", sleep_label, false)
            .on_click(cx.listener(|app, _, _, cx| {
                const PRESETS: [u64; 5] = [5, 15, 30, 60, 0];
                let idx = PRESETS.iter().position(|&m| m == app.sleep_minutes).map(|i| (i + 1) % PRESETS.len()).unwrap_or(0);
                app.sleep_minutes = PRESETS[idx];
                app.sleep_policy.idle_after = Duration::from_secs(app.sleep_minutes * 60);
                if let Some(store) = &app.store {
                    let _ = store.set_setting("sleep_minutes", &app.sleep_minutes.to_string());
                }
                cx.notify();
            }))
            .into_any_element();
        let mcp_btn = pill("settings-mcp", if self.mcp_enabled { "On" } else { "Off" }.into(), false)
            .on_click(cx.listener(|app, _, _, cx| {
                app.mcp_enabled = !app.mcp_enabled;
                if let Some(store) = &app.store {
                    let _ = store.set_setting("mcp", if app.mcp_enabled { "on" } else { "off" });
                }
                cx.notify();
            }))
            .into_any_element();
        let eval_btn = pill("settings-agent-eval", if self.agent_eval { "On" } else { "Off" }.into(), self.agent_eval)
            .on_click(cx.listener(|app, _, _, cx| {
                app.agent_eval = !app.agent_eval;
                app.hub.set_agent_eval(app.agent_eval);
                if let Some(store) = &app.store {
                    let _ = store.set_setting("agent_eval", if app.agent_eval { "on" } else { "off" });
                }
                cx.notify();
            }))
            .into_any_element();
        let connect_btn = pill("settings-connect", "Set up…".into(), false)
            .on_click(cx.listener(|app, _, _, cx| app.open_connect(cx)))
            .into_any_element();
        let shell_btn = pill("settings-shell-control", if self.shell_control { "On" } else { "Off" }.into(), self.shell_control)
            .on_click(cx.listener(|app, _, _, cx| {
                app.shell_control = !app.shell_control;
                if let Some(store) = &app.store {
                    let _ = store.set_setting("shell_control", if app.shell_control { "on" } else { "off" });
                }
                cx.notify();
            }))
            .into_any_element();
        let clients = self.store.as_ref().and_then(|s| s.mcp_clients().ok()).unwrap_or_default();
        let mut client_section = div().flex().flex_col().gap_2().child(row(
            "Connected agents",
            Some("Per-client shell writes; the global switch above overrides every grant"),
            div().into_any_element(),
        ));
        if clients.is_empty() {
            client_section = client_section.child(div().text_size(px(11.)).text_color(t.muted).child("No MCP agent has connected yet."));
        }
        for c in clients {
            let name = c.name.clone();
            let granted = c.granted;
            let action = shell_grant_action(granted);
            let btn = div()
                .id(SharedString::from(format!("grant-{}", c.name)))
                .px_2()
                .py_0p5()
                .rounded(px(4.))
                .border_1()
                .border_color(t.border)
                .text_size(px(11.))
                .text_color(if granted { t.accent } else { t.text })
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .child(action)
                .on_click(cx.listener(move |app, _, _, cx| {
                    if let Some(store) = &app.store {
                        let _ = store.set_mcp_client_granted(&name, !granted, now_unix());
                    }
                    cx.notify();
                }))
                .into_any_element();
            client_section = client_section.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .py_2p5()
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .child(div().text_size(px(13.)).text_color(t.text).child(c.name))
                            .child(div().text_size(px(11.)).text_color(t.muted).child(if c.granted {
                                "May plan/apply shell changes"
                            } else {
                                "Connected; no shell writes"
                            })),
                    )
                    .child(btn),
            );
        }
        let strip_position = pill("settings-tabs-position", self.strip.placement.label().into(), false)
            .on_click(cx.listener(|app, _, _, cx| {
                app.rotate_tab_strip(cx);
            }))
            .into_any_element();
        let strip_folded = pill(
            "settings-tabs-folded",
            if self.strip.folded { "Folded" } else { "Open" }.into(),
            false,
        )
        .on_click(cx.listener(|app, _, _, cx| {
            app.toggle_tab_strip(cx);
        }))
        .into_any_element();
        let strip_control = div().flex().items_center().gap_1().child(strip_position).child(strip_folded).into_any_element();
        let clear_btn = pill("settings-clear-history", clear_label, self.confirm_clear_history)
            .on_click(cx.listener(|app, _, _, cx| {
                if !app.confirm_clear_history {
                    app.confirm_clear_history = true;
                } else {
                    app.confirm_clear_history = false;
                    match app.store.as_ref().map(|s| s.clear_history().and_then(|n| s.clear_session().map(|_| n))) {
                        Some(Ok(n)) => {
                            app.saved_session.clear();
                            app.refresh_start_page();
                            app.toast(format!("Cleared {n} history entries and the saved session"))
                        }
                        Some(Err(e)) => app.toast(format!("Clear history failed: {e}")),
                        None => app.toast("Profile store unavailable"),
                    }
                }
                cx.notify();
            }))
            .into_any_element();
        let mut theme_control = div().flex().items_center().gap_1();
        for choice in [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark] {
            let id = match choice {
                ThemeChoice::System => "settings-theme-system",
                ThemeChoice::Light => "settings-theme-light",
                ThemeChoice::Dark => "settings-theme-dark",
            };
            theme_control = theme_control.child(
                pill(id, choice.label().into(), false)
                    .when(choice == self.theme_choice, |d| d.bg(t.active))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.set_theme_choice(choice);
                        cx.notify();
                    })),
            );
        }
        let mut accent_control = div().flex().items_center().gap_1();
        for choice in AccentChoice::ALL {
            let id = match choice {
                AccentChoice::Blue => "settings-accent-blue",
                AccentChoice::Purple => "settings-accent-purple",
                AccentChoice::Green => "settings-accent-green",
                AccentChoice::Orange => "settings-accent-orange",
            };
            accent_control = accent_control.child(
                pill(id, choice.label().into(), false)
                    .when(choice == self.accent_choice, |d| d.bg(t.active))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.set_accent_choice(choice);
                        cx.notify();
                    })),
            );
        }
        let theme_control = theme_control.into_any_element();
        let accent_control = accent_control.into_any_element();
        let shortcuts_btn = pill("settings-shortcuts", "Edit…".into(), false)
            .on_click(cx.listener(|app, _, _, cx| app.open_shortcuts(cx)))
            .into_any_element();
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(90.)).bg(t.scrim).child(
            div()
                .id("settings")
                .w(px(520.))
                .flex()
                .flex_col()
                .rounded(px(10.))
                .bg(t.surface)
                .border_1()
                .border_color(t.border)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down_out(cx.listener(|app, _, _, cx| {
                    app.close_settings();
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .h(px(44.))
                        .px_4()
                        .border_b_1()
                        .border_color(t.border)
                        .child(div().text_size(px(15.)).text_color(t.text).child("Settings"))
                        .child(div().text_size(px(12.)).text_color(t.muted).child("Esc")),
                )
                .child(row("Keyboard shortcuts", Some("Click Change, then press the new keys"), shortcuts_btn))
                .child(row("Appearance", Some("System follows macOS"), theme_control))
                .child(row("Accent", None, accent_control))
                .child(row("Search engine", None, search_btn))
                .child(row("Ad & tracker blocking", None, blocking_btn))
                .child(row("Tab sleep timeout", None, sleep_btn))
                .child(row("Tab strip", Some("⌘S folds · ⇧⌘S moves"), strip_control))
                .child(row("Local MCP server", Some("(restart to apply)"), mcp_btn))
                .child(row("Agent JavaScript (eval_js)", Some("On only for Allow sites; agents can read page data"), eval_btn))
                .child(row("Agent shell changes (all clients)", Some("Allow plan/apply for theme, settings, shortcuts, and sidebar"), shell_btn))
                .child(client_section)
                .child(row("Connect an agent", Some("Claude Code, Codex, OpenCode, Cursor, Zed"), connect_btn))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_4()
                        .py_2p5()
                        .child(div().text_size(px(13.)).text_color(t.text).child("Browsing history"))
                        .child(clear_btn),
                ),
        )
    }

    /// What the window title should say: the active page, or Relay itself.
    /// A web app window is just its page; the bundle already carries its name.

    pub(super) fn render_find(&self, f: &FindBar) -> impl IntoElement {
        let t = self.theme;
        let (text, color) = if f.query.is_empty() {
            ("Find in page…".to_string(), t.muted)
        } else {
            (f.query.clone(), t.text)
        };
        let status = match f.found {
            Some(false) => "No matches",
            Some(true) => "↩ next · ⇧↩ previous",
            None => "esc to close",
        };
        div().absolute().top(px(10.)).right(px(16.)).child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .w(px(340.))
                .h(px(34.))
                .px_3()
                .rounded(px(8.))
                .bg(t.surface)
                .border_1()
                .border_color(if f.found == Some(false) { t.warn } else { t.border })
                .shadow_lg()
                .text_size(px(13.))
                .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(color).child(SharedString::from(text)))
                .child(div().text_size(px(11.)).text_color(if f.found == Some(false) { t.warn } else { t.muted }).child(status)),
        )
    }

}
