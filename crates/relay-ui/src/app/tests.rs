use super::*;
use gpui::Action;
use std::collections::HashSet;

fn saved(url: &str, pinned: bool, active: bool, scroll_y: f64) -> SessionTab {
    SessionTab { url: url.into(), title: format!("T {url}"), pinned, scroll_y, active, position: 0, group: String::new() }
}

#[test]
fn restore_recreates_tabs_in_order_with_pins_and_activates_the_saved_tab() {
    let hub = SharedHub::new();
    let saved = vec![
        saved("https://a.example/", true, false, 0.0),
        saved("about:blank", false, false, 0.0),
        saved("https://b.example/", false, true, 340.0),
    ];
    let (restored, active) = hub.update(|h| restore_into(h, &saved));
    assert_eq!(restored.len(), 2, "non-web URLs are not restored");
    assert_eq!(restored[1].1, "https://b.example/");
    assert_eq!(restored[1].2, 340.0);
    assert_eq!(active, Some(restored[1].0));
    let h = hub.lock();
    assert_eq!(h.active(), active);
    let tabs: Vec<_> = h.tabs().map(|t| (t.url.clone(), t.pinned, t.title.clone())).collect();
    assert_eq!(tabs[0], ("https://a.example/".into(), true, "T https://a.example/".into()));
    assert_eq!(tabs[1].0, "https://b.example/");
}

#[test]
fn restore_without_a_saved_active_tab_activates_the_first() {
    let hub = SharedHub::new();
    let two = [saved("https://a.example/", false, false, 0.0), saved("https://b.example/", false, false, 0.0)];
    let (restored, active) = hub.update(|h| restore_into(h, &two));
    assert_eq!(active, Some(restored[0].0));
    let (none, no_active) = SharedHub::new().update(|h| restore_into(h, &[]));
    assert!(none.is_empty() && no_active.is_none());
}

#[test]
fn snapshot_keeps_user_web_tabs_in_order_and_round_trips() {
    let hub = SharedHub::new();
    let a = hub.update(|h| {
        let a = h.create_tab("https://a.example/", Owner::User);
        h.create_tab("https://agent.example/", Owner::Agent);
        let b = h.create_tab("https://b.example/", Owner::User);
        h.tab_mut(b).unwrap().pinned = true;
        h.tab_mut(a).unwrap().sleeping = true;
        h.set_active(b);
        a
    });
    let snap = session_snapshot(&hub.lock(), |t| (t == a).then_some(512.0));
    assert_eq!(snap.len(), 2, "agent tabs are not saved");
    assert!(snap.iter().all(|s| s.url != "https://agent.example/"));
    assert_eq!((snap[0].scroll_y, snap[0].active, snap[0].position), (512.0, false, 0));
    assert_eq!((snap[1].pinned, snap[1].active, snap[1].position), (true, true, 1));

    let fresh = SharedHub::new();
    let (_, active) = fresh.update(|h| restore_into(h, &snap));
    let again = session_snapshot(&fresh.lock(), |_| None);
    let key = |v: &[SessionTab]| v.iter().map(|s| (s.url.clone(), s.pinned, s.active)).collect::<Vec<_>>();
    assert_eq!(key(&again), key(&snap));
    assert_eq!(fresh.lock().active(), active);
}

#[test]
fn only_a_real_page_load_counts_as_loaded() {
    assert!(!page_loaded_since(None, 0), "created or restored-and-woken tab on the blank document");
    assert!(page_loaded_since(None, 1), "first hello");
    assert!(!page_loaded_since(Some(1), 1), "already handled");
    assert!(page_loaded_since(Some(1), 2), "next navigation");
}

#[test]
fn title_changes_do_not_count_as_visits() {
    let rec = RecordedVisit { load_seq: 7, title: "First".into() };
    assert_eq!(visit_decision(7, "First", Some(&rec)), VisitDecision::Ignore);
    assert_eq!(visit_decision(7, "Second", Some(&rec)), VisitDecision::TitleOnly);
    assert_eq!(visit_decision(7, "Third", Some(&rec)), VisitDecision::TitleOnly, "one visit per load, not per title");
    assert_eq!(visit_decision(8, "Second", Some(&rec)), VisitDecision::Record, "a new load records again");
    assert_eq!(visit_decision(0, "Second", None), VisitDecision::Ignore);
    assert_eq!(visit_decision(1, "Second", None), VisitDecision::Record);
}

#[test]
fn compat_reloads_only_when_a_compat_site_used_the_default_agent() {
    assert_eq!(compat_change(false, true), CompatChange::Enable);
    assert_eq!(compat_change(true, true), CompatChange::None, "after the one reload, no loop");
    assert_eq!(compat_change(true, false), CompatChange::Disable);
    assert_eq!(compat_change(false, false), CompatChange::None);
    assert!(SAFARI_USER_AGENT.contains("Version/") && SAFARI_USER_AGENT.ends_with("Safari/605.1.15"));
}

#[test]
fn zoom_steps_through_presets_and_clamps() {
    assert_eq!(zoom_step(1.0, 1), 1.1);
    assert_eq!(zoom_step(1.0, -1), 0.9);
    assert_eq!(zoom_step(1.3, 1), 1.5, "off-preset values move to the next preset");
    assert_eq!(zoom_step(1.3, -1), 1.25);
    assert_eq!(zoom_step(3.0, 1), 3.0);
    assert_eq!(zoom_step(0.5, -1), 0.5);
    assert_eq!(zoom_step(2.5, 0), 1.0);
    assert_eq!(zoom_step(f64::NAN, 1), 1.1);
}

#[test]
fn byte_size_reads_naturally() {
    assert_eq!(byte_size(820), "820 B");
    assert_eq!(byte_size(1024), "1 KB");
    assert_eq!(byte_size(14_000), "14 KB");
}

#[test]
fn download_directory_setting_expands_home_and_rejects_relative() {
    let home = std::env::var("HOME").unwrap_or_default();
    assert_eq!(download_dir_setting(Some("/tmp/relay-downloads")), std::path::PathBuf::from("/tmp/relay-downloads"));
    if !home.is_empty() {
        assert_eq!(download_dir_setting(Some("~/Downloads/Relay")), std::path::PathBuf::from(&home).join("Downloads/Relay"));
    }
    assert_eq!(download_dir_setting(Some("relative/path")), relay_webkit::downloads::folder());
    assert_eq!(download_dir_setting(None), relay_webkit::downloads::folder());
}

#[test]
fn media_permissions_round_trip_and_skip_bad_entries() {
    let parsed = parse_media_permissions("a.test|0|1;b.test|1|0;bad;c.test|x|1;d.test|0|2");
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed.get(&("a.test".into(), 0)), Some(&true));
    assert_eq!(parsed.get(&("b.test".into(), 1)), Some(&false));
    assert_eq!(encode_media_permissions(&parsed), "a.test|0|1;b.test|1|0");
    assert!(parse_media_permissions("").is_empty());
}

#[test]
fn bookmarks_bar_optional_visibility() {
    assert!(!bookmarks_bar_shown(false, false, true));
    assert!(bookmarks_bar_shown(true, false, true));
    assert!(bookmarks_bar_shown(true, true, false));
    assert!(!bookmarks_bar_shown(true, true, true));
}

#[test]
fn opt_in_settings_default_off() {
    assert!(!setting_opt_in(None));
    assert!(!setting_opt_in(Some("off")));
    assert!(setting_opt_in(Some("on")));
}

#[test]
fn collapsed_group_setting_ignores_blanks_and_duplicates() {
    let parsed = collapsed_group_names(Some(" Work, Read,,Work , "));
    assert_eq!(parsed.len(), 2);
    assert!(parsed.contains("Work") && parsed.contains("Read"));
    assert!(collapsed_group_names(None).is_empty());
}

#[test]
fn favicon_reply_is_validated_before_decoding() {
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lU2l4wAAAABJRU5ErkJggg==";
    assert!(favicon_image(&serde_json::json!({"available": true, "mime": "image/png", "data": format!("data:image/png;base64,{png}")})).is_some());
    assert!(favicon_image(&serde_json::json!({"available": false, "mime": "image/png", "data": format!("data:image/png;base64,{png}")})).is_none());
    let svg = "PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIxNiIgaGVpZ2h0PSIxNiI+PHJlY3Qgd2lkdGg9IjE2IiBoZWlnaHQ9IjE2IiBmaWxsPSIjNWI4Y2ZmIi8+PC9zdmc+";
    assert!(favicon_image(&serde_json::json!({"available": true, "mime": "image/svg+xml", "data": format!("data:image/svg+xml;base64,{svg}")})).is_some());
    assert!(favicon_image(&serde_json::json!({"available": true, "mime": "image/svg+xml", "data": "data:image/svg+xml;base64,bm90LXN2Zw=="})).is_none(), "bytes must carry the declared format");
    assert!(favicon_image(&serde_json::json!({"available": true, "mime": "image/png", "data": "data:image/jpeg;base64,AAAA"})).is_none());
    assert!(favicon_image(&serde_json::json!({"available": true, "mime": "image/png", "data": "not-a-data-url"})).is_none());
    let huge = format!("data:image/png;base64,{}", "A".repeat(400 * 1024));
    assert!(favicon_image(&serde_json::json!({"available": true, "mime": "image/png", "data": huge})).is_none());
    assert!(!favicon_file_name("evil/../x").contains('/'));
    assert!(favicon_file_name("../../etc").chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_'));
}

#[test]
fn ico_favicons_use_the_largest_png_payload() {
    let ico = webapp::base64_decode("AAABAAEAAQEAAAEAIABGAAAAFgAAAIlQTkcNChoKAAAADUlIRFIAAAABAAAAAQgGAAAAHxXEiQAAAA1JREFUeNpj/M/AUA8ABYMCf5VNpeMAAAAASUVORK5CYII=").unwrap();
    let png = ico_png(&ico).expect("PNG-compressed ICO");
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(ico_png(b"\x00\x00\x01\x00\x00\x00").is_none());
}

#[test]
fn favicons_are_requested_after_a_load_and_retried_once_per_load() {
    assert!(favicon_due(false, 1, None, false));
    assert!(!favicon_due(true, 1, None, false), "a decoded icon is final");
    assert!(!favicon_due(false, 1, Some(1), false), "already answered this load");
    assert!(favicon_due(false, 2, Some(1), false), "a reload may bring an icon");
    assert!(!favicon_due(false, 2, Some(1), true), "never overlap requests");
}

#[test]
fn key_bindings_cover_documented_shortcuts() {
    let bindings = key_bindings();
    assert_eq!(bindings.len(), ActionId::ALL.len());
    let bound: HashSet<&'static str> = bindings.iter().map(|b| b.action().name()).collect();
    for id in ActionId::ALL {
        assert!(bound.contains(id.action_name()), "missing key binding for {}", id.action_name());
    }
}

#[test]
fn the_menu_bar_is_complete_and_has_no_duplicate_actions() {
    let menus = app_menus();
    let names: Vec<&'static str> = menus
        .iter()
        .flat_map(|m| m.items.iter())
        .filter_map(|i| match i {
            MenuItem::Action { action, .. } => Some(action.name()),
            _ => None,
        })
        .collect();
    for required in [
        NewTab.name(),
        CloseTab.name(),
        ToggleTabStrip.name(),
        NextTabStripPosition.name(),
        InstallWebApp.name(),
        RemoveWebApp.name(),
        Cut.name(),
        Copy.name(),
        Paste.name(),
        SelectAll.name(),
        Undo.name(),
        Redo.name(),
        Find.name(),
        Minimize.name(),
        Quit.name(),
    ] {
        assert!(names.contains(&required), "the menu bar is missing {required}");
    }
    for binding in key_bindings() {
        assert!(
            names.contains(&binding.action().name()),
            "{} has a key binding but no menu item, so it will not fire while WKWebView has focus",
            binding.action().name()
        );
    }
    let mut unique = names.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), names.len(), "a menu action appears twice: {names:?}");
    assert!(menus.iter().any(|m| m.name == "File"), "File menu");
    assert!(menus.iter().any(|m| m.name == "Edit"), "Edit menu");
    assert_eq!(app_menus_named(Some("Relay Shop"))[0].name, "Relay Shop");
    assert_eq!(app_menus()[0].name, "Relay");
}

#[test]
fn shell_grant_labels_and_summary_are_stable() {
    assert_eq!(shell_grant_action(false), "Grant");
    assert_eq!(shell_grant_action(true), "Revoke");
    assert_eq!(shell_write_summary(false, 0), "off");
    assert_eq!(shell_write_summary(false, 1), "1 client granted");
    assert_eq!(shell_write_summary(false, 3), "3 clients granted");
    assert_eq!(shell_write_summary(true, 0), "on (all clients)");
}

#[test]
fn shell_keybinding_setting_reaches_live_bindings_and_menus() {
    // The MCP shell path parses this same string and calls apply_keymap,
    // so the active map is what Settings/palette show without a restart.
    let keymap = Keymap::from_setting(Some("find=ctrl-alt-f"));
    let (bindings, menus) = live_keymap(&keymap, Some("Relay"));
    assert_eq!(bindings.len(), ActionId::ALL.len());
    let find_index = ActionId::ALL.iter().position(|id| *id == ActionId::Find).expect("Find action");
    let key = &bindings[find_index].keystrokes()[0];
    assert_eq!(key.key(), "f");
    assert!(key.modifiers().control && key.modifiers().alt, "ctrl-alt-f modifiers");
    assert!(
        menus.iter().flat_map(|m| m.items.iter()).any(|i| matches!(i, MenuItem::Action { action, .. } if action.name() == Find.name())),
        "the Find menu action is rebuilt too"
    );
    // A reserved/conflicting override is skipped and leaves the default.
    let skipped = Keymap::from_setting(Some("find=secondary-k"));
    assert_eq!(skipped.key(ActionId::Find), ActionId::Find.default_key());
}

#[test]
fn download_rows_show_active_then_finished_in_selection_order() {
    let hub = SharedHub::new();
    let tab = hub.update(|h| h.create_tab("https://x.test/", Owner::User));
    let (a, b, c) = hub.update(|h| {
        (h.begin_download(tab, "https://x.test/a", "a"), h.begin_download(tab, "https://x.test/b", "b"), h.begin_download(tab, "https://x.test/c", "c"))
    });
    hub.update(|h| {
        h.set_download_destination(b, "b.zip", "/Users/u/Downloads/Relay/b.zip");
        h.finish_download(b);
        h.fail_download(a, "network");
    });
    let rows = hub.lock().downloads_in_panel_order();
    let groups: Vec<_> = rows.iter().map(|d| DownloadGroup::of(&d.state)).collect();
    assert_eq!(groups, vec![DownloadGroup::Active, DownloadGroup::Finished, DownloadGroup::Finished]);
    assert_eq!(rows.iter().map(|d| d.id).collect::<Vec<_>>(), vec![c, a, b], "one order for rows, headings, and the selected index");
    assert_eq!((DownloadGroup::Active.label(), DownloadGroup::Finished.label()), ("Active", "Finished"));
    assert_eq!(rows[2].path.as_deref().map(|p| shorten_home(p, "/Users/u")).as_deref(), Some("~/Downloads/Relay/b.zip"));
}

#[test]
fn destination_rows_shorten_home_and_leave_unrelated_paths_alone() {
    assert_eq!(shorten_home("/Users/u/Downloads/Relay/a.zip", "/Users/u"), "~/Downloads/Relay/a.zip");
    assert_eq!(shorten_home("/Users/u", "/Users/u"), "~");
    assert_eq!(shorten_home("/Users/other/x", "/Users/u"), "/Users/other/x");
    assert_eq!(shorten_home("/Users/u2/x", "/Users/u"), "/Users/u2/x", "a shared prefix is not the home directory");
    assert_eq!(shorten_home("/tmp/a", ""), "/tmp/a");
}

#[cfg(not(target_os = "macos"))]
fn test_app(cx: &mut gpui::TestAppContext) -> gpui::WindowHandle<RelayApp> {
    let (_requests, requests_rx) = tokio::sync::mpsc::unbounded_channel();
    cx.add_window(move |window, cx| {
        RelayApp::new(SharedHub::new(), requests_rx, None, "test".into(), true, None, None, window, cx)
    })
}

#[cfg(not(target_os = "macos"))]
fn key_event(key: &str) -> KeyDownEvent {
    KeyDownEvent {
        keystroke: gpui::Keystroke {
            key: key.into(),
            key_char: (key.chars().count() == 1).then(|| key.into()),
            ..Default::default()
        },
        is_held: false,
    }
}

#[cfg(not(target_os = "macos"))]
#[gpui::test]
fn palette_edit_actions_select_copy_and_cut(cx: &mut gpui::TestAppContext) {
    let window = test_app(cx);
    window
        .update(cx, |app, _window, cx| {
            app.open_palette(Some("hello world".into()), false);
            app.apply_edit(EditAction::SelectAll, cx);
            assert_eq!(app.palette.as_ref().unwrap().selection(), Some((0, 11)));
            app.apply_edit(EditAction::Copy, cx);
        })
        .unwrap();
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("hello world"));

    window
        .update(cx, |app, _window, cx| {
            app.open_palette(Some("abc".into()), false);
            if let Some(p) = app.palette.as_mut() {
                p.set_cursor(1, false);
                p.set_cursor(3, true);
            }
            app.apply_edit(EditAction::Cut, cx);
            let p = app.palette.as_ref().unwrap();
            assert_eq!((p.query.as_str(), p.cursor), ("a", 1));
        })
        .unwrap();
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("bc"));
}

#[test]
fn marked_text_run_is_underlined_without_changing_other_bytes() {
    let base = TextRun {
        len: 9,
        font: gpui::font(".SystemUIFont"),
        color: gpui::black().into(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let runs = palette_text_runs("日本語", Some(0..3), &base, gpui::blue().into());
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].len, 3);
    assert!(runs[0].underline.is_some(), "only the marked range is underlined");
    assert_eq!(runs[1].len, 6);
    assert!(runs[1].underline.is_none());
}

#[cfg(not(target_os = "macos"))]
#[gpui::test]
fn ime_replacement_tracks_utf16_and_marked_range(cx: &mut gpui::TestAppContext) {
    let window = test_app(cx);
    window
        .update(cx, |app, window, cx| {
            app.open_palette(Some("ab".into()), false);
            app.replace_and_mark_text_in_range(Some(0..2), "日本", Some(1..2), window, cx);
            {
                let palette = app.palette.as_ref().unwrap();
                assert_eq!(palette.query, "日本");
                assert_eq!(palette.marked, Some(0..6));
                assert_eq!(palette.selected_text().as_deref(), Some("本"));
            }
            let mut adjusted = None;
            assert_eq!(app.text_for_range(0..2, &mut adjusted, window, cx).as_deref(), Some("日本"));
            assert_eq!(adjusted, Some(0..2));
            assert_eq!(app.marked_text_range(window, cx), Some(0..2));
            let selection = app.selected_text_range(false, window, cx).unwrap();
            assert_eq!((selection.range, selection.reversed), (1..2, false));

            app.replace_text_in_range(Some(0..1), "X", window, cx);
            let palette = app.palette.as_ref().unwrap();
            assert_eq!(palette.query, "X本");
            assert_eq!(palette.marked, None, "a normal commit clears the marked range");
        })
        .unwrap();
}

#[cfg(not(target_os = "macos"))]
#[gpui::test]
fn palette_cursor_tracks_the_model_after_edits(cx: &mut gpui::TestAppContext) {
    let window = test_app(cx);
    window
        .update(cx, |app, _window, cx| {
            app.open_palette(Some("ab".into()), false);
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    let before = window
        .update(cx, |app, _window, _cx| {
            let metrics = app.palette_metrics.borrow();
            let metrics = metrics.as_ref().unwrap();
            (metrics.cursor, metrics.line.x_for_index(2))
        })
        .unwrap();
    assert_eq!(before.0, before.1, "the painted caret is at the model cursor");

    window
        .update(cx, |app, window, cx| {
            app.replace_text_in_range(None, "c", window, cx);
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |app, _window, _cx| {
            let palette = app.palette.as_ref().unwrap();
            assert_eq!((palette.query.as_str(), palette.cursor), ("abc", 3));
            let metrics = app.palette_metrics.borrow();
            let metrics = metrics.as_ref().unwrap();
            assert_eq!(metrics.cursor, metrics.line.x_for_index(3), "the caret moved with the second edit");
            assert!(metrics.cursor > px(0.));
        })
        .unwrap();
}

#[cfg(not(target_os = "macos"))]
#[gpui::test]
fn palette_render_installs_the_input_handler(cx: &mut gpui::TestAppContext) {
    let window = test_app(cx);
    window
        .update(cx, |app, _window, cx| {
            app.open_palette(Some("a".into()), false);
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    window
        .update(cx, |app, window, cx| {
            assert!(app.palette_input_active, "the palette text element must install the IME handler during paint");
            let bounds = app
                .bounds_for_range(0..1, gpui::Bounds::default(), window, cx)
                .expect("the painted line must position the IME candidate window");
            assert!(bounds.size.width > px(0.));
            assert!(app.character_index_for_point(gpui::point(bounds.origin.x + px(1.), bounds.origin.y + px(1.)), window, cx).is_some());
        })
        .unwrap();
}

#[cfg(not(target_os = "macos"))]
#[gpui::test]
fn palette_shortcuts_select_copy_and_cut(cx: &mut gpui::TestAppContext) {
    let window = test_app(cx);
    window
        .update(cx, |app, _window, cx| {
            app.open_palette(Some("hello world".into()), false);
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(*window, cx);

    visual.simulate_keystrokes("secondary-a");
    window
        .update(cx, |app, _window, _cx| {
            assert_eq!(app.palette.as_ref().unwrap().selection(), Some((0, 11)));
        })
        .unwrap();
    visual.simulate_keystrokes("secondary-c");
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("hello world"));

    visual.simulate_keystrokes("secondary-x");
    window
        .update(cx, |app, _window, _cx| {
            let palette = app.palette.as_ref().unwrap();
            assert_eq!(palette.query, "");
            assert_eq!(palette.cursor, 0);
        })
        .unwrap();
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("hello world"));
}

#[cfg(not(target_os = "macos"))]
#[gpui::test]
fn palette_click_places_the_cursor_at_the_shaped_position(cx: &mut gpui::TestAppContext) {
    let window = test_app(cx);
    window
        .update(cx, |app, _window, cx| {
            app.open_palette(Some("abc".into()), false);
            cx.notify();
        })
        .unwrap();
    cx.refresh().unwrap();
    cx.run_until_parked();
    let (bounds, x) = window
        .update(cx, |app, _window, _cx| {
            let metrics = app.palette_metrics.borrow();
            let metrics = metrics.as_ref().expect("palette must have painted");
            (metrics.bounds, metrics.line.x_for_index(2) - px(0.5))
        })
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(*window, cx);
    visual.simulate_click(point(bounds.left() + x, bounds.top() + px(4.)), gpui::Modifiers::default());
    window
        .update(cx, |app, _window, _cx| {
            assert_eq!(app.palette.as_ref().unwrap().cursor, 1, "clicking between b and c places the caret there");
        })
        .unwrap();
}

#[cfg(not(target_os = "macos"))]
#[gpui::test]
fn active_input_handler_owns_printable_keys(cx: &mut gpui::TestAppContext) {
    let window = test_app(cx);
    window
        .update(cx, |app, window, cx| {
            app.open_palette(Some("a".into()), false);
            app.palette_input_active = true;
            app.on_key(&key_event("b"), window, cx);
            assert_eq!(app.palette.as_ref().unwrap().query, "a", "key_char must not be inserted when the IME handler owns text");
        })
        .unwrap();
}

#[cfg(not(target_os = "macos"))]
#[gpui::test]
fn closed_palette_does_not_consume_editing_keys(cx: &mut gpui::TestAppContext) {
    let window = test_app(cx);
    window
        .update(cx, |app, window, cx| {
            let tab = app.hub.update(|hub| hub.create_tab("https://example.test/", Owner::User));
            app.hub.update(|hub| hub.set_active(tab));
            app.on_key(&key_event("left"), window, cx);
            app.on_key(&key_event("backspace"), window, cx);
            app.on_key(&key_event("a"), window, cx);
            assert!(app.palette.is_none(), "closed palette must not be opened by editing keys");
            assert!(app.hints.is_none());
        })
        .unwrap();
}
