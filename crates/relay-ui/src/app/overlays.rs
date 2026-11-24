//! Modal overlays: Library, Downloads, Connect MCP, Shortcuts, Settings, and Find bar.

use super::*;

/// Shorten the home prefix for display in Settings.
fn app_downloads_label(path: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() {
        if let Some(rest) = path.strip_prefix(&home) {
            return format!("~{rest}");
        }
    }
    path.to_string()
}

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
                        .child(div().text_size(px(15.)).text_color(t.text).child("Downloads"))
                        .child(
                            div()
                                .id("downloads-clear")
                                .px_2()
                                .py_0p5()
                                .rounded(px(4.))
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
                    .child({
                        let just_copied = self
                            .connect_copied
                            .is_some_and(|(c, at)| c == client && at.elapsed() < Duration::from_secs(3));
                        div()
                            .id(SharedString::from(format!("connect-copy-{}", client.id())))
                            .flex_shrink_0()
                            .px_2p5()
                            .py_1()
                            .rounded(px(5.))
                            .bg(if just_copied { t.active } else { t.background })
                            .text_size(px(11.))
                            .text_color(if just_copied { t.accent } else { t.text })
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                let command = mcp_config::relay_command();
                                cx.write_to_clipboard(ClipboardItem::new_string(client.snippet(&command)));
                                app.connect_copied = Some((client, Instant::now()));
                                app.toast(format!("Copied the {} config", client.name()));
                                cx.notify();
                            }))
                            .child(if just_copied { "Copied ✓" } else { "Copy config" })
                    }),
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
                        .child(div().text_size(px(15.)).text_color(t.text).child("Connect an agent"))
                        .child(div().text_size(px(11.)).text_color(t.muted).child("Copy the text; Relay changes no files")),
                )
                .child(rows),
        )
    }

    pub(super) fn render_shortcuts(&self, editor: &ShortcutEditor, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let query = editor.query.trim().to_lowercase();
        let mut list = div().id("shortcuts-list").flex().flex_col().gap_0p5().max_h(px(474.)).overflow_y_scroll().px_4().pb_4();
        let mut shown = 0;
        for group in keymap::SHORTCUT_GROUPS {
            let ids: Vec<ActionId> = ActionId::ALL
                .iter()
                .copied()
                .filter(|id| {
                    id.group() == group
                        && (query.is_empty()
                            || id.label().to_lowercase().contains(&query)
                            || id.as_str().contains(&query)
                            || keymap::shortcut_label(self.keymap.key(*id)).to_lowercase().contains(&query))
                })
                .collect();
            if ids.is_empty() {
                continue;
            }
            shown += ids.len();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_2()
                    .pt_4()
                    .pb_1()
                    .child(div().text_size(px(10.)).text_color(t.muted).child(SharedString::from(group.to_uppercase())))
                    .child(div().text_size(px(10.)).text_color(t.muted).child(SharedString::from(ids.len().to_string()))),
            );
            for id in ids {
                let recording = editor.recording == Some(id);
                let custom = self.keymap.key(id) != id.default_key();
                let key = if recording { "…".to_string() } else { keymap::shortcut_label(self.keymap.key(id)) };
                let mut row = div()
                    .id(SharedString::from(format!("shortcut-{}", id.as_str())))
                    .flex()
                    .items_center()
                    .gap_3()
                    .h(px(34.))
                    .px_3()
                    .rounded(px(7.))
                    .text_size(px(13.))
                    .when(recording, |d| d.bg(t.active))
                    .hover(|s| s.bg(t.hover))
                    .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(t.text).child(id.label()))
                    .child(
                        div()
                            .w(px(78.))
                            .flex_shrink_0()
                            .flex()
                            .justify_end()
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded(px(5.))
                                    .bg(if recording { t.accent } else { t.background })
                                    .text_size(px(12.))
                                    .font_family("Menlo")
                                    .text_color(if recording { gpui::rgb(0xffffff).into() } else { t.text })
                                    .child(SharedString::from(key)),
                            ),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("shortcut-change-{}", id.as_str())))
                            .w(px(74.))
                            .flex_shrink_0()
                            .flex()
                            .justify_center()
                            .px_2()
                            .py_1()
                            .rounded(px(5.))
                            .bg(t.background)
                            .text_size(px(11.))
                            .text_color(if recording { t.accent } else { t.text })
                            .cursor_pointer()
                            .hover(|s| s.bg(t.active))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.start_recording(id, cx);
                            }))
                            .child(if recording { "Press keys…" } else { "Change" }),
                    );
                if custom {
                    row = row.child(
                        div()
                            .id(SharedString::from(format!("shortcut-reset-{}", id.as_str())))
                            .w(px(22.))
                            .h(px(22.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(5.))
                            .text_size(px(14.))
                            .text_color(t.muted)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover).text_color(t.text))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.keymap.reset(id);
                                app.apply_keymap(cx);
                                app.toast(format!("{} restored", id.label()));
                                cx.notify();
                            }))
                            .child("↺"),
                    );
                } else {
                    row = row.child(div().w(px(22.)).flex_shrink_0());
                }
                list = list.child(row);
            }
        }
        if shown == 0 {
            list = list.child(
                div().px_2().py_8().text_size(px(13.)).text_color(t.muted).child("No shortcut matches that search."),
            );
        }
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(46.)).bg(t.scrim).child(
            div()
                .id("shortcuts")
                .w(px(720.))
                .h(px(600.))
                .flex()
                .flex_col()
                .rounded(px(14.))
                .bg(t.surface)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down_out(cx.listener(|app, _, _, cx| app.close_shortcuts(cx)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .px_5()
                        .pt_5()
                        .pb_3()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(div().text_size(px(17.)).text_color(t.text).child("Keyboard shortcuts"))
                                .child(
                                    div()
                                        .id("shortcuts-reset")
                                        .px_2p5()
                                        .py_1()
                                        .rounded(px(6.))
                                        .bg(t.background)
                                        .text_size(px(11.))
                                        .text_color(t.muted)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover).text_color(t.text))
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            app.keymap.reset_all();
                                            app.apply_keymap(cx);
                                            app.toast("Shortcuts reset to defaults");
                                            cx.notify();
                                        }))
                                        .child("Reset all"),
                                ),
                        )
                        .child(div().text_size(px(11.)).text_color(t.muted).child(
                            if editor.recording.is_some() {
                                "Press the new combination · Esc cancels"
                            } else {
                                "Click Change, then press a combination. Bare keys and ⌘Q are reserved."
                            },
                        ))
                        .child(
                            div()
                                .id("shortcuts-search")
                                .mt_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .h(px(32.))
                                .px_3()
                                .rounded(px(8.))
                                .bg(t.background)
                                .text_size(px(12.))
                                .text_color(if query.is_empty() { t.muted } else { t.text })
                                .child(div().text_size(px(11.)).text_color(t.muted).child("⌕"))
                                .child(div().flex_1().child(SharedString::from(if editor.query.is_empty() {
                                    "Search shortcuts".to_string()
                                } else {
                                    editor.query.clone()
                                })))
                                .child(div().text_size(px(10.)).text_color(t.muted).child(if editor.query.is_empty() { "type to filter" } else { "esc clears" })),
                        ),
                )
                .child(list)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_5()
                        .py_2p5()
                        .bg(t.background)
                        .text_size(px(10.))
                        .text_color(t.muted)
                        .child(format!("{} shortcuts", ActionId::ALL.len()))
                        .child(if editor.recording.is_some() { "Recording" } else { "Esc closes" }),
                ),
        )
    }

    pub(super) fn render_profile_prompt(&self, prompt: &ProfilePrompt, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut chips = div().flex().flex_wrap().gap_1();
        let names = profile_names();
        for name in &names {
            if name == &self.profile_name {
                continue;
            }
            let target = name.clone();
            chips = chips.child(
                div()
                    .id(SharedString::from(format!("profile-pick-{name}")))
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .bg(t.background)
                    .text_size(px(11.))
                    .text_color(t.text)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.profile_prompt = None;
                        app.switch_profile(&target, cx);
                    }))
                    .child(SharedString::from(name.clone())),
            );
        }
        if !self.profile_name.is_empty() {
            chips = chips.child(
                div()
                    .id("profile-pick-default")
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .bg(t.background)
                    .text_size(px(11.))
                    .text_color(t.text)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.profile_prompt = None;
                        app.switch_profile("", cx);
                    }))
                    .child("Default"),
            );
        }
        let text = if prompt.value.is_empty() { "New profile name".to_string() } else { prompt.value.clone() };
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(120.)).bg(t.scrim).child(
            div()
                .id("profile-prompt")
                .w(px(420.))
                .flex()
                .flex_col()
                .gap_3()
                .p_5()
                .rounded(px(12.))
                .bg(t.raised)
                .shadow_lg()
                .on_mouse_down_out(cx.listener(|app, _, _, cx| {
                    app.close_profile_prompt();
                    cx.notify();
                }))
                .child(div().text_size(px(15.)).text_color(t.text).child("Profiles"))
                .child(div().text_size(px(11.)).text_color(t.muted).child("One profile per context. Switching relaunches Relay with that profile's tabs, cookies, logins, and extensions."))
                .child(
                    div()
                        .h(px(34.))
                        .px_3()
                        .flex()
                        .items_center()
                        .rounded(px(8.))
                        .bg(t.well)
                        .text_size(px(13.))
                        .text_color(if prompt.value.is_empty() { t.muted } else { t.text })
                        .child(SharedString::from(text))
                        .child(div().ml_0p5().text_color(t.accent).child("▏")),
                )
                .when(!names.is_empty(), |d| d.child(div().text_size(px(10.)).text_color(t.muted).child("EXISTING PROFILES")).child(chips))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1())
                        .child(
                            div()
                                .id("profile-cancel")
                                .px_3()
                                .py_1p5()
                                .rounded(px(6.))
                                .text_size(px(12.))
                                .text_color(t.muted)
                                .cursor_pointer()
                                .hover(|s| s.bg(t.hover).text_color(t.text))
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.close_profile_prompt();
                                    cx.notify();
                                }))
                                .child("Cancel"),
                        )
                        .child(
                            div()
                                .id("profile-commit")
                                .px_3()
                                .py_1p5()
                                .rounded(px(6.))
                                .bg(t.accent)
                                .text_size(px(12.))
                                .text_color(gpui::rgb(0xffffff))
                                .cursor_pointer()
                                .hover(|s| s.opacity(0.9))
                                .on_click(cx.listener(|app, _, _, cx| app.save_profile_prompt(cx)))
                                .child("Create & switch"),
                        ),
                ),
        )
    }

    pub(super) fn render_space_prompt(&self, prompt: &SpacePrompt, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let current = self.hub.lock().current_space().to_string();
        let names = self.hub.lock().space_names();
        let mut chips = div().flex().flex_wrap().gap_1();
        for name in &names {
            if name == &current {
                continue;
            }
            let label = if name.is_empty() { "Default".to_string() } else { name.clone() };
            let target = name.clone();
            chips = chips.child(
                div()
                    .id(SharedString::from(format!("space-pick-{name}")))
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .bg(t.background)
                    .text_size(px(11.))
                    .text_color(t.text)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.space_prompt = None;
                        app.switch_space(target.clone(), cx);
                    }))
                    .child(SharedString::from(label)),
            );
        }
        let text = if prompt.value.is_empty() { "New space name".to_string() } else { prompt.value.clone() };
        let wanted = super::Hub::normalize_space_name(&prompt.value).unwrap_or_default();
        let can_rename = !current.is_empty() && !wanted.is_empty() && wanted != current;
        let can_delete = !current.is_empty();
        let delete_label = if prompt.confirm_delete { "Confirm delete" } else { "Delete space" };
        let delete_btn = div()
            .id("space-delete")
            .px_3()
            .py_1p5()
            .rounded(px(6.))
            .text_size(px(12.))
            .text_color(if prompt.confirm_delete { t.warn } else { t.muted })
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .on_click(cx.listener(|app, _, _, cx| {
                if app.space_prompt.as_ref().is_some_and(|p| p.confirm_delete) {
                    app.delete_current_space(cx);
                } else if let Some(prompt) = app.space_prompt.as_mut() {
                    prompt.confirm_delete = true;
                    cx.notify();
                }
            }))
            .child(delete_label);
        let rename_btn = div()
            .id("space-rename")
            .px_3()
            .py_1p5()
            .rounded(px(6.))
            .text_size(px(12.))
            .text_color(t.accent)
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .on_click(cx.listener(move |app, _, _, cx| {
                if let Some(prompt) = &app.space_prompt {
                    let value = prompt.value.clone();
                    app.rename_current_space(value, cx);
                }
            }))
            .child("Rename current");
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(120.)).bg(t.scrim).child(
            div()
                .id("space-prompt")
                .w(px(380.))
                .flex()
                .flex_col()
                .gap_3()
                .p_5()
                .rounded(px(12.))
                .bg(t.raised)
                .shadow_lg()
                .on_mouse_down_out(cx.listener(|app, _, _, cx| {
                    app.close_space_prompt();
                    cx.notify();
                }))
                .child(div().text_size(px(15.)).text_color(t.text).child("Spaces"))
                .child(div().text_size(px(11.)).text_color(t.muted).child("Each space keeps its own tabs and pins. Empty means Default."))
                .child(
                    div()
                        .h(px(34.))
                        .px_3()
                        .flex()
                        .items_center()
                        .rounded(px(8.))
                        .bg(t.well)
                        .text_size(px(13.))
                        .text_color(if prompt.value.is_empty() { t.muted } else { t.text })
                        .child(SharedString::from(text))
                        .child(div().ml_0p5().text_color(t.accent).child("▏")),
                )
                .when(!names.is_empty(), |d| d.child(div().text_size(px(10.)).text_color(t.muted).child("EXISTING SPACES")).child(chips))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .when(can_delete, |d| d.child(delete_btn))
                        .when(can_rename, |d| d.child(rename_btn))
                        .child(div().flex_1())
                        .child(
                            div()
                                .id("space-cancel")
                                .px_3()
                                .py_1p5()
                                .rounded(px(6.))
                                .text_size(px(12.))
                                .text_color(t.muted)
                                .cursor_pointer()
                                .hover(|s| s.bg(t.hover).text_color(t.text))
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.close_space_prompt();
                                    cx.notify();
                                }))
                                .child("Cancel"),
                        )
                        .child(
                            div()
                                .id("space-commit")
                                .px_3()
                                .py_1p5()
                                .rounded(px(6.))
                                .bg(t.accent)
                                .text_size(px(12.))
                                .text_color(gpui::rgb(0xffffff))
                                .cursor_pointer()
                                .hover(|s| s.opacity(0.9))
                                .on_click(cx.listener(|app, _, _, cx| app.save_space_prompt(cx)))
                                .child("Switch"),
                        ),
                ),
        )
    }

    pub(super) fn render_group_prompt(&self, prompt: &GroupPrompt, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let current = self.hub.lock().tab(prompt.tab).and_then(|tab| tab.group.clone());
        let groups = self.hub.lock().tab_groups();
        let mut chips = div().flex().flex_wrap().gap_1();
        for name in &groups {
            if current.as_deref() == Some(name.as_str()) {
                continue;
            }
            let target = name.clone();
            chips = chips.child(
                div()
                    .id(SharedString::from(format!("group-pick-{name}")))
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .bg(t.background)
                    .text_size(px(11.))
                    .text_color(t.text)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        if let Some(prompt) = &app.group_prompt {
                            app.hub.update(|h| h.set_tab_group(prompt.tab, Some(&target)));
                        }
                        app.group_prompt = None;
                        app.toast(format!("Moved tab to {target}"));
                        app.focus_active_page();
                        cx.notify();
                    }))
                    .child(SharedString::from(name.clone())),
            );
        }
        let text = if prompt.value.is_empty() { "Group name".to_string() } else { prompt.value.clone() };
        let commit_label = if current.is_some() && prompt.value.trim().is_empty() { "Remove" } else { "Move" };
        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(120.)).bg(t.scrim).child(
            div()
                .id("group-prompt")
                .w(px(380.))
                .flex()
                .flex_col()
                .gap_3()
                .p_5()
                .rounded(px(12.))
                .bg(t.raised)
                .shadow_lg()
                .on_mouse_down_out(cx.listener(|app, _, _, cx| {
                    app.close_group_prompt();
                    cx.notify();
                }))
                .child(div().text_size(px(15.)).text_color(t.text).child("Move tab to a group"))
                .child(div().text_size(px(11.)).text_color(t.muted).child("Name it, or pick an existing group. Clearing the name removes it."))
                .child(
                    div()
                        .h(px(34.))
                        .px_3()
                        .flex()
                        .items_center()
                        .rounded(px(8.))
                        .bg(t.well)
                        .text_size(px(13.))
                        .text_color(if prompt.value.is_empty() { t.muted } else { t.text })
                        .child(SharedString::from(text))
                        .child(div().ml_0p5().text_color(t.accent).child("▏")),
                )
                .when(!groups.is_empty(), |d| d.child(div().text_size(px(10.)).text_color(t.muted).child("EXISTING GROUPS")).child(chips))
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            div()
                                .id("group-cancel")
                                .px_3()
                                .py_1p5()
                                .rounded(px(6.))
                                .text_size(px(12.))
                                .text_color(t.muted)
                                .cursor_pointer()
                                .hover(|s| s.bg(t.hover).text_color(t.text))
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.close_group_prompt();
                                    cx.notify();
                                }))
                                .child("Cancel"),
                        )
                        .child(
                            div()
                                .id("group-commit")
                                .px_3()
                                .py_1p5()
                                .rounded(px(6.))
                                .bg(t.accent)
                                .text_size(px(12.))
                                .text_color(gpui::rgb(0xffffff))
                                .cursor_pointer()
                                .hover(|s| s.opacity(0.9))
                                .on_click(cx.listener(|app, _, _, cx| app.save_group_prompt(cx)))
                                .child(commit_label),
                        ),
                ),
        )
    }

    pub(super) fn render_site_card(&self, host: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let prefs = self.store.as_ref().and_then(|s| s.site_pref(host).ok()).unwrap_or_default();
        let policy = self.hub.agent_policy_for_host(host);
        let zoom = (prefs.zoom * 100.0).round() as i32;
        let forget_camera = host.to_string();
        let forget_mic = host.to_string();
        let permission = |kind: i64| match self.media_permissions.get(&(host.to_string(), kind)) {
            Some(true) => "Allowed",
            Some(false) => "Blocked",
            None => "Ask",
        };
        let button = |id: &'static str, label: String, active: bool| {
            div()
                .id(id)
                .px_2()
                .py_1()
                .rounded(px(5.))
                .bg(if active { t.active } else { t.well })
                .text_size(px(11.))
                .text_color(if active { t.text } else { t.muted })
                .cursor_pointer()
                .hover(|s| s.bg(t.hover).text_color(t.text))
                .child(SharedString::from(label))
        };
        let row = |label: &'static str, control: gpui::AnyElement| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .py_1p5()
                .child(div().text_size(px(12.)).text_color(t.text).child(label))
                .child(control)
        };
        div().absolute().top_0().left_0().size_full().bg(t.scrim).flex().justify_end().pt(px(44.)).pr(px(10.)).child(
            div()
                .id("site-card")
                .w(px(340.))
                .flex()
                .flex_col()
                .gap_1()
                .p_4()
                .rounded(px(12.))
                .bg(t.raised)
                .shadow_lg()
                .on_mouse_down_out(cx.listener(|app, _, _, cx| {
                    app.close_site_card();
                    cx.notify();
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().flex().flex_col().child(div().text_size(px(14.)).text_color(t.text).child(SharedString::from(host.to_string()))).child(div().text_size(px(10.)).text_color(t.muted).child("Site settings")))
                        .child(
                            div()
                                .id("site-close")
                                .px_2()
                                .cursor_pointer()
                                .text_color(t.muted)
                                .hover(|s| s.text_color(t.text))
                                .on_click(cx.listener(|app, _, _, cx| {
                                    app.close_site_card();
                                    cx.notify();
                                }))
                                .child("×"),
                        ),
                )
                .child(
                    row(
                        "Zoom",
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(button("site-zoom-out", "−".into(), false).on_click(cx.listener(|app, _, _, cx| { app.zoom(-1); cx.notify(); })))
                            .child(div().w(px(38.)).text_size(px(11.)).text_color(t.text).child(SharedString::from(format!("{zoom}%"))))
                            .child(button("site-zoom-in", "+".into(), false).on_click(cx.listener(|app, _, _, cx| { app.zoom(1); cx.notify(); })))
                            .child(button("site-zoom-reset", "Reset".into(), zoom == 100).on_click(cx.listener(|app, _, _, cx| { app.zoom(0); cx.notify(); })))
                            .into_any_element(),
                    ),
                )
                .child(row(
                    "Blocking",
                    button("site-blocking", if prefs.blocking_off { "Off".into() } else { "On".into() }, prefs.blocking_off)
                        .on_click(cx.listener(|app, _, _, cx| { app.toggle_site_blocking(); cx.notify(); }))
                        .into_any_element(),
                ))
                .child(row(
                    "Agents",
                    button("site-agents", policy.label().to_string(), policy != relay_bridge::AgentPolicy::default())
                        .on_click(cx.listener(|app, _, _, cx| { app.cycle_agent_policy(cx); cx.notify(); }))
                        .into_any_element(),
                ))
                .child(row(
                    "Compatibility",
                    button("site-compat", if prefs.compat_mode { "Safari UA".into() } else { "Default".into() }, prefs.compat_mode)
                        .on_click(cx.listener(|app, _, _, cx| { app.toggle_compat_mode(); cx.notify(); }))
                        .into_any_element(),
                ))
                .child(row(
                    "Camera",
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_size(px(11.)).text_color(t.muted).child(permission(0)))
                        .child(button("site-forget-camera", "Forget".into(), false).on_click(cx.listener(move |app, _, _, cx| app.forget_host_permissions(&forget_camera, cx))))
                        .into_any_element(),
                ))
                .child(row(
                    "Microphone",
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_size(px(11.)).text_color(t.muted).child(permission(1)))
                        .child(button("site-forget-mic", "Forget".into(), false).on_click(cx.listener(move |app, _, _, cx| app.forget_host_permissions(&forget_mic, cx))))
                        .into_any_element(),
                )),
        )
    }

    pub(super) fn render_settings(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let btn = |id: &'static str, label: SharedString, danger: bool| {
            div()
                .id(id)
                .px_3()
                .py_1p5()
                .rounded(px(6.))
                .bg(t.background)
                .text_size(px(12.))
                .text_color(if danger { t.error } else { t.text })
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .child(label)
        };
        let switch_dyn = |id: SharedString, on: bool, danger: bool| {
            let track_bg = if on {
                if danger { t.error } else { t.accent }
            } else {
                t.well
            };
            div()
                .id(id)
                .w(px(36.))
                .h(px(20.))
                .p(px(2.))
                .rounded_full()
                .bg(track_bg)
                .flex()
                .items_center()
                .when(on, |d| d.justify_end())
                .cursor_pointer()
                .child(div().size(px(16.)).rounded_full().bg(gpui::rgb(0xffffff)).shadow_sm())
        };
        let switch = |id: &'static str, on: bool, danger: bool| switch_dyn(id.into(), on, danger);
        let row = |title: &'static str, subtitle: Option<&'static str>, control: gpui::AnyElement| {
            let mut left = div()
                .flex()
                .flex_col()
                .gap_0p5()
                .flex_1()
                .min_w_0()
                .pr_4()
                .child(div().text_size(px(13.)).text_color(t.text).child(title));
            if let Some(sub) = subtitle {
                left = left.child(div().text_size(px(11.)).text_color(t.muted).child(sub));
            }
            div()
                .flex()
                .items_center()
                .justify_between()
                .px_4()
                .py_3()
                .child(left)
                .child(control)
        };
        let card = |content: gpui::Div| {
            content
                .rounded(px(8.))
                .bg(t.background)
                .overflow_hidden()
        };

        let mut nav = div()
            .w(px(184.))
            .flex_shrink_0()
            .h_full()
            .bg(t.background)
            .p_3()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().px_2().pb_2().text_size(px(14.)).text_color(t.text).child("Settings"));
        for tab in SettingsTab::ALL {
            let active = self.settings_tab == tab;
            nav = nav.child(
                div()
                    .id(SharedString::from(format!("settings-nav-{}", tab.label())))
                    .px_2p5()
                    .py_1p5()
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .text_color(if active { t.text } else { t.muted })
                    .when(active, |d| d.bg(t.active))
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover).text_color(t.text))
                    .on_click(cx.listener(move |app, _, _, cx| {
                        app.settings_tab = tab;
                        app.settings_dropdown = None;
                        cx.notify();
                    }))
                    .child(tab.label()),
            );
        }

        let sleep_label: SharedString = if self.sleep_minutes == 0 {
            "Never".into()
        } else {
            format!("{} min", self.sleep_minutes).into()
        };
        let pane_content: gpui::AnyElement = match self.settings_tab {
            SettingsTab::General => {
                let search_open = self.settings_dropdown == Some(SettingsDropdown::SearchEngine);
                let sleep_open = self.settings_dropdown == Some(SettingsDropdown::SleepTimeout);
                let search_trigger = div()
                    .id("settings-search")
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .min_w(px(140.))
                    .justify_between()
                    .rounded(px(6.))
                    .bg(t.surface)
                    .text_size(px(12.))
                    .text_color(t.text)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.settings_dropdown = if app.settings_dropdown == Some(SettingsDropdown::SearchEngine) {
                            None
                        } else {
                            Some(SettingsDropdown::SearchEngine)
                        };
                        cx.notify();
                    }))
                    .child(palette::search_label(&self.search))
                    .child(div().text_size(px(10.)).text_color(t.muted).child(if search_open { "▴" } else { "▾" }))
                    .into_any_element();

                let mut search_menu = div().flex().flex_col().px_4().py_2().gap_0p5().bg(t.well);
                for (name, url) in palette::SEARCH_PRESETS {
                    let selected = self.search == *url;
                    let target_url = (*url).to_string();
                    search_menu = search_menu.child(
                        div()
                            .id(SharedString::from(format!("settings-search-opt-{name}")))
                            .flex()
                            .items_center()
                            .justify_between()
                            .px_3()
                            .py_1p5()
                            .rounded(px(5.))
                            .text_size(px(12.))
                            .text_color(t.text)
                            .when(selected, |d| d.bg(t.active))
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.search = target_url.clone();
                                app.settings_dropdown = None;
                                if let Some(store) = &app.store {
                                    let _ = store.set_setting("search", &app.search);
                                }
                                cx.notify();
                            }))
                            .child(*name)
                            .child(div().text_color(t.accent).child(if selected { "✓" } else { "" })),
                    );
                }

                let sleep_trigger = div()
                    .id("settings-sleep")
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1p5()
                    .min_w(px(140.))
                    .justify_between()
                    .rounded(px(6.))
                    .bg(t.surface)
                    .text_size(px(12.))
                    .text_color(t.text)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.settings_dropdown = if app.settings_dropdown == Some(SettingsDropdown::SleepTimeout) {
                            None
                        } else {
                            Some(SettingsDropdown::SleepTimeout)
                        };
                        cx.notify();
                    }))
                    .child(sleep_label)
                    .child(div().text_size(px(10.)).text_color(t.muted).child(if sleep_open { "▴" } else { "▾" }))
                    .into_any_element();

                const SLEEP_OPTS: [(u64, &str); 5] = [(5, "5 min"), (15, "15 min"), (30, "30 min"), (60, "60 min"), (0, "Never")];
                let mut sleep_menu = div().flex().flex_col().px_4().py_2().gap_0p5().bg(t.well);
                for (mins, label) in SLEEP_OPTS {
                    let selected = self.sleep_minutes == mins;
                    sleep_menu = sleep_menu.child(
                        div()
                            .id(SharedString::from(format!("settings-sleep-opt-{mins}")))
                            .flex()
                            .items_center()
                            .justify_between()
                            .px_3()
                            .py_1p5()
                            .rounded(px(5.))
                            .text_size(px(12.))
                            .text_color(t.text)
                            .when(selected, |d| d.bg(t.active))
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.sleep_minutes = mins;
                                app.sleep_policy.idle_after = Duration::from_secs(mins * 60);
                                app.settings_dropdown = None;
                                if let Some(store) = &app.store {
                                    let _ = store.set_setting("sleep_minutes", &mins.to_string());
                                }
                                cx.notify();
                            }))
                            .child(label)
                            .child(div().text_color(t.accent).child(if selected { "✓" } else { "" })),
                    );
                }

                let profile_btn = btn("settings-profile-switch", "Switch…".into(), false)
                    .on_click(cx.listener(|app, _, _, cx| app.open_profile_prompt(cx)))
                    .into_any_element();
                let profile_label = if self.profile_name.is_empty() { "Default".to_string() } else { self.profile_name.clone() };
                let mut general_card = div().flex().flex_col();
                general_card = general_card.child(row("Profile", Some("Each profile has its own tabs, data, logins, and extensions; switching relaunches Relay"), profile_btn));
                general_card = general_card.child(row("Current profile", Some("The window's profile name"), div().text_size(px(12.)).text_color(t.text).child(SharedString::from(profile_label)).into_any_element()));
                general_card = general_card.child(row("Search engine", Some("Used for non-URL queries in the ⌘K palette"), search_trigger));
                if search_open {
                    general_card = general_card.child(search_menu);
                }
                general_card = general_card.child(row("Tab sleep timeout", Some("Unload idle background tabs to save RAM"), sleep_trigger));
                if sleep_open {
                    general_card = general_card.child(sleep_menu);
                }
                let sync_choose = btn("settings-sync-choose", "Choose…".into(), false)
                    .on_click(cx.listener(|app, _, _, cx| app.choose_sync_folder(cx)))
                    .into_any_element();
                let sync_now = btn("settings-sync-now", "Sync now".into(), false)
                    .on_click(cx.listener(|app, _, _, cx| app.sync_now(cx)))
                    .into_any_element();
                let sync_control = div()
                    .max_w(px(340.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(10.))
                            .font_family("Menlo")
                            .text_color(t.muted)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(if self.sync_folder.is_empty() { "Off".to_string() } else { app_downloads_label(&self.sync_folder) }),
                    )
                    .child(sync_choose)
                    .child(sync_now)
                    .into_any_element();
                let backup_btn = btn("settings-backup", "Back up…".into(), false)
                    .on_click(cx.listener(|app, _, _, cx| app.backup_profile(cx)))
                    .into_any_element();
                let restore_btn = btn("settings-restore", "Restore…".into(), false)
                    .on_click(cx.listener(|app, _, _, cx| app.restore_profile(cx)))
                    .into_any_element();
                let safari_btn = btn("settings-import-safari", "Import Safari".into(), false)
                    .on_click(cx.listener(|app, _, _, cx| app.import_safari(cx)))
                    .into_any_element();
                let import_btn = btn("settings-import-chrome", "Import from Chrome".into(), false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.run_command(Command::ImportChrome, cx);
                        cx.notify();
                    }))
                    .into_any_element();
                general_card = general_card.child(row("Browser bookmarks & history", Some("Chrome, Brave, Edge, Arc, Dia, or Chromium"), import_btn));
                general_card = general_card.child(row("Safari bookmarks & history", Some("Needs Full Disk Access in System Settings → Privacy & Security"), safari_btn));
                general_card = general_card.child(row("Sync folder", Some("Merge this profile on launch, then rewrite it every minute (Dropbox/iCloud)"), sync_control));
                general_card = general_card.child(row("Back up profile", Some("Write bookmarks, history, and settings to a private JSON file"), backup_btn));
                general_card = general_card.child(row("Restore profile", Some("Merge a Relay backup; local settings win"), restore_btn));
                let default_btn = btn("settings-default-browser", "Make default".into(), false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        if app.web.borrow().renders_pages() {
                            app.web.borrow().set_default_browser();
                            app.toast("Asking macOS to make Relay the default browser…");
                        } else {
                            app.toast("Setting the default browser needs macOS");
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                general_card = general_card.child(row("Default browser", Some("Open http and https links from other apps in Relay"), default_btn));
                let ask_downloads_sw = switch("settings-downloads-ask", self.downloads_ask, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.downloads_ask = !app.downloads_ask;
                        app.persist_downloads();
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("downloads_ask", if app.downloads_ask { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let choose_downloads = btn("settings-downloads-choose", "Choose…".into(), false)
                    .on_click(cx.listener(|app, _, _, cx| app.choose_download_folder(cx)))
                    .into_any_element();
                let downloads_path = div()
                    .max_w(px(300.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(10.))
                            .font_family("Menlo")
                            .text_color(t.muted)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(SharedString::from(app_downloads_label(&self.downloads_dir))),
                    )
                    .child(choose_downloads)
                    .into_any_element();
                general_card = general_card.child(row("Downloads folder", Some("Set with relay config set downloads_dir PATH"), downloads_path));
                general_card = general_card.child(row("Ask where to save each file", Some("Show a save panel for every download"), ask_downloads_sw));
                let sections = self.start_sections();
                for (id, section) in [
                    ("settings-start-recent", Section::Recent),
                    ("settings-start-frequent", Section::Frequent),
                    ("settings-start-apps", Section::Apps),
                    ("settings-start-bookmarks", Section::Bookmarks),
                ] {
                    let on = sections.contains(&section);
                    let sw = switch(id, on, false)
                        .on_click(cx.listener(move |app, _, _, cx| app.toggle_start_section(section, cx)))
                        .into_any_element();
                    general_card = general_card.child(row(section.title(), Some("Show this section on the new-tab page"), sw));
                }
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_size(px(16.)).text_color(t.text).child("General"))
                    .child(card(general_card))
                    .into_any_element()
            }
            SettingsTab::Appearance => {
                let mut theme_seg = div().flex().items_center().p_0p5().rounded(px(7.)).bg(t.well);
                for choice in [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark] {
                    let id = match choice {
                        ThemeChoice::System => "settings-theme-system",
                        ThemeChoice::Light => "settings-theme-light",
                        ThemeChoice::Dark => "settings-theme-dark",
                    };
                    let active = choice == self.theme_choice;
                    theme_seg = theme_seg.child(
                        div()
                            .id(id)
                            .px_3()
                            .py_1()
                            .rounded(px(5.))
                            .text_size(px(12.))
                            .text_color(if active { t.text } else { t.muted })
                            .when(active, |d| d.bg(t.active))
                            .cursor_pointer()
                            .hover(|s| s.text_color(t.text))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.set_theme_choice(choice);
                                cx.notify();
                            }))
                            .child(choice.label()),
                    );
                }
                let mut accent_row = div().flex().items_center().gap_2();
                for choice in AccentChoice::ALL {
                    let id = match choice {
                        AccentChoice::Blue => "settings-accent-blue",
                        AccentChoice::Purple => "settings-accent-purple",
                        AccentChoice::Green => "settings-accent-green",
                        AccentChoice::Orange => "settings-accent-orange",
                    };
                    let active = choice == self.accent_choice;
                    let (swatch_color, _) = choice.color(true);
                    accent_row = accent_row.child(
                        div()
                            .id(id)
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .px_2p5()
                            .py_1()
                            .rounded(px(6.))
                            .when(active, |d| d.bg(t.active))
                            .text_size(px(12.))
                            .text_color(t.text)
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.set_accent_choice(choice);
                                cx.notify();
                            }))
                            .child(div().size(px(10.)).rounded_full().bg(swatch_color))
                            .child(choice.label()),
                    );
                }
                let mut strip_seg = div().flex().items_center().p_0p5().rounded(px(7.)).bg(t.well);
                for placement in tabstrip::Placement::ALL {
                    let id = match placement {
                        tabstrip::Placement::Left => "settings-tabs-left",
                        tabstrip::Placement::Top => "settings-tabs-top",
                        tabstrip::Placement::Right => "settings-tabs-right",
                    };
                    let active = self.strip.placement == placement;
                    strip_seg = strip_seg.child(
                        div()
                            .id(id)
                            .px_3()
                            .py_1()
                            .rounded(px(5.))
                            .text_size(px(12.))
                            .text_color(if active { t.text } else { t.muted })
                            .when(active, |d| d.bg(t.active))
                            .cursor_pointer()
                            .hover(|s| s.text_color(t.text))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.set_tab_strip_placement(placement, cx);
                            }))
                            .child(placement.label()),
                    );
                }
                let strip_visible = switch("settings-tabs-folded", !self.strip.folded, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.toggle_tab_strip(cx);
                    }))
                    .into_any_element();
                let tab_nums_sw = switch("settings-tab-numbers", self.show_tab_numbers, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.show_tab_numbers = !app.show_tab_numbers;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("tab_numbers", if app.show_tab_numbers { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let address_sw = switch("settings-address-bar", self.show_address_bar, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.show_address_bar = !app.show_address_bar;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("address_bar", if app.show_address_bar { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let bottom_sw = switch("settings-bottom-bar", self.show_bottom_bar, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.toggle_bottom_bar();
                        cx.notify();
                    }))
                    .into_any_element();
                let appearance_card = div()
                    .flex()
                    .flex_col()
                    .child(row("Theme", Some("System follows macOS and syncs website prefers-color-scheme"), theme_seg.into_any_element()))
                    .child(row("Accent colour", None, accent_row.into_any_element()))
                    .child(row("Tab strip position", Some("Move tabs between Left, Top, and Right (⇧⌘S)"), strip_seg.into_any_element()))
                    .child(row("Show tab strip", Some("Hide to maximize page area; ⌘S toggles anytime"), strip_visible))
                    .child(row("Show tab numbers while ⌘ is held", Some("Displays 1–9 shortcut badges on tabs and favorites"), tab_nums_sw))
                    .child(row("Always show the address bar", Some("One quiet URL line above the page; click it to edit in ⌘K"), address_sw))
                    .child(row("Show the bottom bar", Some("URL, tab count and agent state; hide it for a quieter page (⌘⌥B)"), bottom_sw));
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_size(px(16.)).text_color(t.text).child("Appearance"))
                    .child(card(appearance_card))
                    .into_any_element()
            }
            SettingsTab::Tabs => {
                let restore_sw = switch("settings-restore-session", self.session_enabled, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.session_enabled = !app.session_enabled;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("restore_session", if app.session_enabled { "on" } else { "off" });
                            if !app.session_enabled {
                                let _ = store.clear_session();
                                app.saved_session.clear();
                            }
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let glyphs_sw = switch("settings-tab-glyphs", self.tab_glyphs, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.tab_glyphs = !app.tab_glyphs;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("tab_glyphs", if app.tab_glyphs { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let bm_icons_sw = switch("settings-bookmarks-icons", self.bookmarks_icons_only, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.bookmarks_icons_only = !app.bookmarks_icons_only;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("bookmarks_icons_only", if app.bookmarks_icons_only { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let bm_new_sw = switch("settings-bookmarks-newtab", self.bookmarks_new_tab_only, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.bookmarks_new_tab_only = !app.bookmarks_new_tab_only;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("bookmarks_new_tab_only", if app.bookmarks_new_tab_only { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let close_sw = switch("settings-always-close", self.always_show_close, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.always_show_close = !app.always_show_close;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("always_show_close", if app.always_show_close { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let bookmarks_sw = switch("settings-bookmarks-bar", self.show_bookmarks_bar, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.show_bookmarks_bar = !app.show_bookmarks_bar;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("bookmarks_bar", if app.show_bookmarks_bar { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let icons_only_sw = switch("settings-tab-icons-only", self.tab_icons_only, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.tab_icons_only = !app.tab_icons_only;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("tab_icons_only", if app.tab_icons_only { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let compact_sw = switch("settings-compact-tabs", self.compact_tabs, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.compact_tabs = !app.compact_tabs;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("compact_tabs", if app.compact_tabs { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let content = div()
                    .flex()
                    .flex_col()
                    .child(row("Restore tabs at launch", Some("Reopen last session's pages asleep, so the RAM is not spent until you visit them"), restore_sw))
                    .child(row("Site icons in tabs", Some("Off shows a coloured letter badge instead of each site's favicon"), glyphs_sw))
                    .child(row("Compact tab rows", Some("Shorter rows for more tabs in the same column"), compact_sw))
                    .child(row("Icons-only vertical strip", Some("A narrow rail of site icons; middle-click or right-click to close/manage"), icons_only_sw))
                    .child(row("Show the bookmarks bar", Some("A row of saved pages under the tab strip"), bookmarks_sw))
                    .child(row("Bookmarks bar: icons only", Some("Titles are hidden to keep the row short"), bm_icons_sw))
                    .child(row("Bookmarks bar only on new tabs", Some("Hidden while a page is open"), bm_new_sw))
                    .child(row("Always show tab close buttons", Some("Off: × appears on the active tab and on hover"), close_sw));
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_size(px(16.)).text_color(t.text).child("Tabs"))
                    .child(card(content))
                    .into_any_element()
            }
            SettingsTab::Browsing => {
                let mut zoom_seg = div().flex().items_center().gap_1().p_0p5().rounded(px(8.)).bg(t.well);
                for pct in [80u32, 90, 100, 110, 125, 150] {
                    let zoom = f64::from(pct) / 100.0;
                    let active = (self.default_zoom - zoom).abs() < 1e-6;
                    zoom_seg = zoom_seg.child(
                        div()
                            .id(SharedString::from(format!("settings-zoom-{pct}")))
                            .px_2p5()
                            .py_1()
                            .rounded(px(6.))
                            .text_size(px(11.))
                            .text_color(if active { t.text } else { t.muted })
                            .when(active, |d| d.bg(t.surface))
                            .cursor_pointer()
                            .hover(|s| s.bg(t.hover).text_color(t.text))
                            .on_click(cx.listener(move |app, _, _, cx| {
                                app.default_zoom = zoom;
                                if let Some(store) = &app.store {
                                    let _ = store.set_setting("default_zoom", &format!("{zoom:.2}"));
                                }
                                if let Some(tab) = app.active() {
                                    app.web.borrow().set_zoom(tab, zoom);
                                }
                                app.toast(format!("Default zoom: {pct}%"));
                                cx.notify();
                            }))
                            .child(format!("{pct}%")),
                    );
                }
                let notes = div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .text_size(px(11.))
                    .text_color(t.muted)
                    .child("⌘= and ⌘- still change one site and remember it; the default applies to sites you never zoomed.")
                    .child("Find is case-insensitive and wraps; setting case/whole-word options is not implemented yet.");
                let content = div()
                    .flex()
                    .flex_col()
                    .child(row("Default page zoom", Some("Sites without their own saved zoom open at this size"), zoom_seg.into_any_element()))
                    .child(div().px_4().py_3().child(notes));
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_size(px(16.)).text_color(t.text).child("Browsing"))
                    .child(card(content))
                    .into_any_element()
            }
            SettingsTab::Passwords => {
                let fill_sw = switch("settings-login-fill", self.login_fill_enabled, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.login_fill_enabled = !app.login_fill_enabled;
                        if !app.login_fill_enabled {
                            app.login_fill_prompt = None;
                        }
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("login_fill", if app.login_fill_enabled { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let save_sw = switch("settings-login-save", self.login_save_enabled, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.login_save_enabled = !app.login_save_enabled;
                        if !app.login_save_enabled {
                            if let Some((tab, _, _)) = app.login_save_prompt.take() {
                                app.web.borrow().decline_save_login(tab);
                            }
                        }
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("login_save", if app.login_save_enabled { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let auto_sw = switch("settings-autofill-logins", self.autofill_logins, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.autofill_logins = !app.autofill_logins;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("autofill_logins", if app.autofill_logins { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let manage = btn("settings-manage-logins", "Manage…".into(), false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.settings_tab = SettingsTab::Privacy;
                        cx.notify();
                    }))
                    .into_any_element();
                let passkeys = div()
                    .px_2p5()
                    .py_1()
                    .rounded(px(5.))
                    .bg(t.well)
                    .text_size(px(11.))
                    .text_color(t.muted)
                    .child("Not available in this build")
                    .into_any_element();
                let import_hint = div()
                    .text_size(px(10.))
                    .font_family("Menlo")
                    .text_color(t.muted)
                    .child("relay import-passwords <file.csv>")
                    .into_any_element();
                let content = div()
                    .flex()
                    .flex_col()
                    .child(row("Fill saved sign-ins", Some("Show a Keychain login when a sign-in field gets focus; the password never reaches an agent"), fill_sw))
                    .child(row("Fill a single saved login automatically", Some("Off by default: no second click when exactly one login is saved for the site"), auto_sw))
                    .child(row("Offer to save passwords", Some("After a successful sign-in, offer the macOS Keychain"), save_sw))
                    .child(row("Saved logins", Some("Delete a saved login for the site you are on, under Privacy"), manage))
                    .child(row("Passkeys", Some("Apple grants the browser credential entitlement to signed browsers; this ad-hoc build does not have it yet"), passkeys))
                    .child(row("Import Chrome passwords", Some("Writes internet passwords to the login Keychain, then deletes the CSV"), import_hint));
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_size(px(16.)).text_color(t.text).child("Passwords"))
                    .child(card(content))
                    .into_any_element()
            }
            SettingsTab::Privacy => {
                let blocking_sw = switch("settings-blocking", self.blocking, false)
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
                let cookie_sw = switch("settings-cookie-banners", self.hide_cookie_banners, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.hide_cookie_banners = !app.hide_cookie_banners;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("cookie_banners", if app.hide_cookie_banners { "on" } else { "off" });
                        }
                        if app.blocking {
                            app.web.borrow().load_content_rules(&rules_json(app.store.as_ref()));
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let clear_label: SharedString = if self.confirm_clear_history { "Confirm clear history".into() } else { "Clear history".into() };
                let clear_btn = btn("settings-clear-history", clear_label, self.confirm_clear_history)
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
                let handoff_sw = switch("settings-handoff", self.handoff, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.handoff = !app.handoff;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("handoff", if app.handoff { "on" } else { "off" });
                        }
                        if !app.handoff {
                            app.last_handoff = None;
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let forget_btn = btn("settings-forget-media", "Forget".into(), false)
                    .on_click(cx.listener(|app, _, _, cx| app.forget_media_permissions(cx)))
                    .into_any_element();
                let site_label: SharedString = if self.confirm_clear_site_data { "Confirm sign out everywhere".into() } else { "Clear cookies & site data".into() };
                let site_btn = btn("settings-clear-site-data", site_label, self.confirm_clear_site_data)
                    .on_click(cx.listener(|app, _, _, cx| {
                        if !app.confirm_clear_site_data {
                            app.confirm_clear_site_data = true;
                            app.toast("Click again to sign out of every site");
                        } else {
                            app.confirm_clear_site_data = false;
                            app.web.borrow().clear_site_data();
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let privacy_card = div()
                    .flex()
                    .flex_col()
                    .child(row("Ad & tracker blocking", Some("Native WebKit content-rule blocker"), blocking_sw))
                    .child(row("Hide cookie banners", Some("Consent-platform selectors from the blocker; reload to apply"), cookie_sw))
                    .child(row("Browsing history & saved session", Some("Removes all visited pages and open session tabs"), clear_btn))
                    .child(row("Cookies & site data", Some("Signs you out everywhere; clears cache and local storage"), site_btn))
                    .child(row("Camera & microphone choices", Some("Allow/Block decisions saved for ordinary sites"), forget_btn))
                    .child(row("Handoff current page", Some("Advertise ordinary http(s) tabs to other iCloud devices; off by default"), handoff_sw));
                let active_url = self
                    .active()
                    .and_then(|id| self.hub.lock().tab(id).map(|s| s.url.clone()))
                    .unwrap_or_default();
                let saved_here = if active_url.is_empty() {
                    vec![]
                } else {
                    let managed: std::collections::HashSet<(String, String)> = self
                        .store
                        .as_ref()
                        .and_then(|s| {
                            let origin = relay_extensions::login::Origin::parse(&active_url).ok()?;
                            s.saved_logins(origin.host()).ok()
                        })
                        .unwrap_or_default()
                        .into_iter()
                        .collect();
                    self.web
                        .borrow()
                        .list_login_candidates(&active_url)
                        .into_iter()
                        .filter(|c| managed.contains(c))
                        .collect()
                };
                let mut col = div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_size(px(16.)).text_color(t.text).child("Privacy"))
                    .child(card(privacy_card));
                if !saved_here.is_empty() {
                    let mut logins_card = div().flex().flex_col();
                    for (idx, (user, path)) in saved_here.into_iter().enumerate() {
                        let u_del = user.clone();
                        let p_del = path.clone();
                        let url_del = active_url.clone();
                        let label = if user.is_empty() { "(saved password)".to_string() } else { user };
                        logins_card = logins_card.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px_4()
                                .py_2p5()
                                .child(div().text_size(px(13.)).text_color(t.text).child(SharedString::from(label)))
                                .child(
                                    div()
                                        .id(("login-del", idx))
                                        .px_2p5()
                                        .py_1()
                                        .rounded(px(5.))
                                        .text_size(px(11.))
                                        .text_color(t.error)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover))
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            let res = app.web.borrow().delete_saved_login(&url_del, &u_del, &p_del);
                                            match res {
                                                Ok(true) => {
                                                    if let Some(store) = &app.store {
                                                        if let Ok((origin, _)) = relay_extensions::login::Origin::parse_with_path(&url_del) {
                                                            let _ = store.forget_saved_login(origin.host(), &u_del, &p_del);
                                                        }
                                                    }
                                                    app.toast("Deleted login from Keychain")
                                                }
                                                Ok(false) => app.toast("Login not found in Keychain"),
                                                Err(e) => app.toast(e),
                                            }
                                            cx.notify();
                                        }))
                                        .child("Delete"),
                                ),
                        );
                    }
                    col = col
                        .child(div().text_size(px(12.)).text_color(t.muted).child("SAVED LOGINS ON THIS SITE"))
                        .child(card(logins_card));
                }
                col.into_any_element()
            }
            SettingsTab::Extensions => {
                let installed = self.store.as_ref().and_then(|s| s.extensions().ok()).unwrap_or_default();
                let import_ext_btn = btn(
                    "settings-ext-import-chrome",
                    if self.ext_busy { "Working…".into() } else { "Import from Chrome".into() },
                    false,
                )
                .on_click(cx.listener(|app, _, _, cx| {
                    app.import_chrome_extensions(cx);
                }))
                .into_any_element();
                let paste_ext_btn = btn(
                    "settings-ext-install-clip",
                    if self.ext_busy { "Installing…".into() } else { "Install from Clipboard".into() },
                    false,
                )
                .on_click(cx.listener(|app, _, _, cx| {
                    app.install_extension_from_clipboard(cx);
                }))
                .into_any_element();
                let store_btn = btn("settings-ext-open-cws", "Open Web Store".into(), false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.close_settings();
                        app.open_url("https://chromewebstore.google.com/".to_string(), true);
                        cx.notify();
                    }))
                    .into_any_element();
                let toolbar_sw = switch("settings-extension-toolbar", self.extension_toolbar, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.extension_toolbar = !app.extension_toolbar;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("extension_toolbar", if app.extension_toolbar { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let ext_install_btn = btn("settings-ext-install-field", "Install".into(), false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.install_extension_from_input(cx);
                    }))
                    .into_any_element();
                let ext_field = div()
                    .id("settings-ext-input")
                    .flex_1()
                    .min_w_0()
                    .h(px(30.))
                    .px_3()
                    .flex()
                    .items_center()
                    .rounded(px(7.))
                    .bg(t.well)
                    .text_size(px(12.))
                    .text_color(if self.ext_input.is_empty() { t.muted } else { t.text })
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .cursor_text()
                    .on_click(cx.listener(|app, _, _, _cx| {
                        app.web.borrow().focus_shell();
                    }))
                    .child(SharedString::from(if self.ext_input.is_empty() {
                        "Chrome Web Store URL or 32-char ID".to_string()
                    } else {
                        self.ext_input.clone()
                    }));
                let actions_card = div()
                    .flex()
                    .flex_col()
                    .child(row("Show extension toolbar", Some("Enabled extensions as a compact row above the page"), toolbar_sw))
                    .child(row("Import Chrome extensions", Some("Fetches your Chrome extensions via CRX3 and loads them in WebKit (macOS 15.4+)"), import_ext_btn))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_4()
                            .py_3()
                            .child(div().flex_1().min_w_0().flex().flex_col().gap_0p5().child(div().text_size(px(13.)).text_color(t.text).child("Install by URL or ID")).child(div().text_size(px(11.)).text_color(t.muted).child("Type or paste a Chrome Web Store address, then Install; this grants every requested permission and host pattern")))
                            .child(ext_field)
                            .child(div().flex_shrink_0().child(ext_install_btn)),
                    )
                    .child(row("Install from Clipboard", Some("Copy any CWS URL or 32-char ID, then Install"), paste_ext_btn))
                    .child(row("Browse Chrome Web Store", Some("Find extensions, copy their URL, and install directly"), store_btn));
                let mut list_card = div().flex().flex_col();
                if installed.is_empty() {
                    list_card = list_card.child(
                        div()
                            .px_4()
                            .py_4()
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child("No extensions installed yet."),
                    );
                } else {
                    for ext in installed {
                        let id = ext.id.clone();
                        let path = ext.path.clone();
                        let enabled = ext.enabled;
                        let grants = relay_extensions::manifest::Manifest::read(std::path::Path::new(&path))
                            .ok()
                            .map(|m| m.permission_report().lines().join(" · "))
                            .unwrap_or_default();
                        let rm_id = id.clone();
                        let en_id = id.clone();
                        let opt_id = id.clone();
                        let pop_id = id.clone();
                        let en_sw = switch_dyn(SharedString::from(format!("ext-en-{id}")), enabled, false)
                            .on_click(cx.listener(move |app, _, _, cx| {
                                if let Some(store) = &app.store {
                                    let _ = store.set_extension_enabled(&en_id, !enabled);
                                }
                                if !enabled {
                                    app.web.borrow().load_extension(std::path::Path::new(&path), &en_id, |_| {});
                                }
                                app.toast(if enabled { "Disabled; restart to unload it" } else { "Enabled" });
                                cx.notify();
                            }))
                            .into_any_element();
                        list_card = list_card.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px_4()
                                .py_2p5()
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_0p5()
                                        .flex_1()
                                        .min_w_0()
                                        .child(div().text_size(px(13.)).text_color(t.text).child(SharedString::from(ext.name)))
                                        .child(div().text_size(px(10.)).text_color(t.muted).font_family("Menlo").overflow_hidden().whitespace_nowrap().text_ellipsis().child(SharedString::from(format!("{} · v{}", id, ext.version))))
                                        .when(!grants.is_empty(), |d| {
                                            d.child(div().text_size(px(10.)).text_color(t.muted).overflow_hidden().whitespace_nowrap().text_ellipsis().child(SharedString::from(grants)))
                                        }),
                                )
                                .child(div().flex().items_center().gap_3().child(en_sw).child(
                                    div()
                                        .id(SharedString::from(format!("ext-pop-{id}")))
                                        .px_2p5()
                                        .py_1()
                                        .rounded(px(5.))
                                        .text_size(px(11.))
                                        .text_color(t.muted)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover).text_color(t.text))
                                        .on_click(cx.listener(move |app, _, _, cx| app.open_extension_popup(&pop_id, cx)))
                                        .child("Popup"),
                                ).child(
                                    div()
                                        .id(SharedString::from(format!("ext-opt-{id}")))
                                        .px_2p5()
                                        .py_1()
                                        .rounded(px(5.))
                                        .text_size(px(11.))
                                        .text_color(t.muted)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover).text_color(t.text))
                                        .on_click(cx.listener(move |app, _, _, cx| app.open_extension_options(&opt_id, cx)))
                                        .child("Options"),
                                ).child(
                                    div()
                                        .id(SharedString::from(format!("ext-rm-{id}")))
                                        .px_2p5()
                                        .py_1()
                                        .rounded(px(5.))
                                        .text_size(px(11.))
                                        .text_color(t.error)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover))
                                        .on_click(cx.listener(move |app, _, _, cx| {
                                            app.remove_extension(&rm_id, cx);
                                        }))
                                        .child("Remove"),
                                )),
                        );
                    }
                }
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_size(px(16.)).text_color(t.text).child("Extensions"))
                    .child(card(actions_card))
                    .when(!self.ext_errors.borrow().is_empty(), |d| {
                        let errors = self.ext_errors.borrow();
                        d.child(div().text_size(px(11.)).text_color(t.error).child(SharedString::from(errors.join("\n"))))
                    })
                    .child(div().text_size(px(12.)).text_color(t.muted).child("INSTALLED"))
                    .child(card(list_card))
                    .into_any_element()
            }
            SettingsTab::Agents => {
                let mcp_sw = switch("settings-mcp", self.mcp_enabled, false)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.mcp_enabled = !app.mcp_enabled;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("mcp", if app.mcp_enabled { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let eval_sw = switch("settings-agent-eval", self.agent_eval, true)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.agent_eval = !app.agent_eval;
                        app.hub.set_agent_eval(app.agent_eval);
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("agent_eval", if app.agent_eval { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let shell_sw = switch("settings-shell-control", self.shell_control, true)
                    .on_click(cx.listener(|app, _, _, cx| {
                        app.shell_control = !app.shell_control;
                        if let Some(store) = &app.store {
                            let _ = store.set_setting("shell_control", if app.shell_control { "on" } else { "off" });
                        }
                        cx.notify();
                    }))
                    .into_any_element();
                let connect_btn = btn("settings-connect", "Set up…".into(), false)
                    .on_click(cx.listener(|app, _, _, cx| app.open_connect(cx)))
                    .into_any_element();
                let mcp_card = div()
                    .flex()
                    .flex_col()
                    .child(row("Local MCP server", Some("Restart Relay after toggling"), mcp_sw))
                    .child(row("Connect an agent", Some("9 clients: Claude Code, Codex, OpenCode, Cursor, Windsurf, Zed, …"), connect_btn))
                    .child(row("Agent JavaScript (eval_js)", Some("Enabled only on Allow sites; agents can read page data"), eval_sw))
                    .child(row("Agent shell changes", Some("Allow plan/apply for theme, settings, shortcuts, and sidebar"), shell_sw));
                let clients = self.store.as_ref().and_then(|s| s.mcp_clients().ok()).unwrap_or_default();
                let mut client_card = div().flex().flex_col();
                if clients.is_empty() {
                    client_card = client_card.child(
                        div()
                            .px_4()
                            .py_3()
                            .text_size(px(12.))
                            .text_color(t.muted)
                            .child("No MCP agent has connected yet."),
                    );
                } else {
                    for c in clients {
                        let name = c.name.clone();
                        let granted = c.granted;
                        let action = shell_grant_action(granted);
                        let gbtn = div()
                            .id(SharedString::from(format!("grant-{}", c.name)))
                            .px_2p5()
                            .py_1()
                            .rounded(px(5.))
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
                            }));
                        client_card = client_card.child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px_4()
                                .py_2p5()
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
                                .child(gbtn),
                        );
                    }
                }
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_size(px(16.)).text_color(t.text).child("Agents & MCP"))
                    .child(card(mcp_card))
                    .child(div().text_size(px(12.)).text_color(t.muted).child("CONNECTED AGENTS"))
                    .child(card(client_card))
                    .into_any_element()
            }
            SettingsTab::Shortcuts => {
                let shortcuts_btn = btn("settings-shortcuts", "Customize shortcuts…".into(), false)
                    .on_click(cx.listener(|app, _, _, cx| app.open_shortcuts(cx)))
                    .into_any_element();
                let sc_card = div()
                    .flex()
                    .flex_col()
                    .child(row("Keyboard shortcuts", Some("Remap any command; hold ⌘ to see 1–9 tab badges"), shortcuts_btn));
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_size(px(16.)).text_color(t.text).child("Shortcuts"))
                    .child(card(sc_card))
                    .into_any_element()
            }
            SettingsTab::About => {
                let home = relay_bridge::paths::relay_home();
                let info = |text: String| {
                    div()
                        .max_w(px(360.))
                        .text_size(px(10.))
                        .font_family("Menlo")
                        .text_color(t.muted)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(SharedString::from(text))
                        .into_any_element()
                };
                let source = btn("settings-source", "GitHub".into(), false)
                    .on_click(cx.listener(|app, _, _, _| app.open_help()))
                    .into_any_element();
                let content = div()
                    .flex()
                    .flex_col()
                    .child(row("Version", Some("MacOS WebKit shell with a local MCP server"), info(env!("CARGO_PKG_VERSION").to_string())))
                    .child(row("Profile", Some("History, bookmarks, settings, session, and the agent audit log"), info(home.display().to_string())))
                    .child(row("Crash log", Some("Attach this file to a bug report; it contains no page data"), info(home.join("logs/panic.log").display().to_string())))
                    .child(row("MCP socket", Some("Local agents connect here when the server is on"), info(self.mcp_socket.clone())))
                    .child(row("Source", Some("Issues, releases, and the feature matrix"), source));
                div()
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(div().text_size(px(16.)).text_color(t.text).child("About"))
                    .child(card(content))
                    .into_any_element()
            }
        };

        div().absolute().top_0().left_0().size_full().flex().justify_center().pt(px(56.)).bg(t.scrim).child(
            div()
                .id("settings")
                .w(px(860.))
                .h(px(600.))
                .flex()
                .flex_row()
                .rounded(px(14.))
                .bg(t.raised)
                .shadow_lg()
                .overflow_hidden()
                .on_mouse_down_out(cx.listener(|app, _, _, cx| {
                    app.close_settings();
                    cx.notify();
                }))
                .child(nav)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_end()
                                .h(px(36.))
                                .px_4()
                                .child(
                                    div()
                                        .id("settings-close")
                                        .px_2()
                                        .py_0p5()
                                        .rounded(px(4.))
                                        .text_size(px(11.))
                                        .text_color(t.muted)
                                        .cursor_pointer()
                                        .hover(|s| s.bg(t.hover).text_color(t.text))
                                        .on_click(cx.listener(|app, _, _, cx| {
                                            app.close_settings();
                                            cx.notify();
                                        }))
                                        .child("Esc"),
                                ),
                        )
                        .child(div().id("settings-body").flex_1().overflow_y_scroll().px_6().pb_6().child(pane_content)),
                ),
        )
    }

    /// What the window title should say: the active page, or Relay itself.
    /// A web app window is just its page; the bundle already carries its name.

    pub(super) fn render_find(&self, f: &FindBar, cx: &mut Context<Self>) -> impl IntoElement {
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
                .w(px(380.))
                .h(px(34.))
                .px_3()
                .rounded(px(8.))
                .bg(t.surface)
                .shadow_lg()
                .text_size(px(13.))
                .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().text_color(color).child(SharedString::from(text)))
                .child(
                    div()
                        .id("find-case")
                        .px_1p5()
                        .py_0p5()
                        .rounded(px(5.))
                        .bg(if f.case_sensitive { t.active } else { t.well })
                        .text_size(px(10.))
                        .text_color(if f.case_sensitive { t.accent } else { t.muted })
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover))
                        .on_click(cx.listener(|app, _, _, cx| {
                            if let Some(f) = app.find.as_mut() {
                                f.case_sensitive = !f.case_sensitive;
                            }
                            app.run_find(false);
                            cx.notify();
                        }))
                        .child("Aa"),
                )
                .child(div().text_size(px(11.)).text_color(if f.found == Some(false) { t.warn } else { t.muted }).child(status)),
        )
    }

}
