use super::*;
use gpui::Action;
use std::collections::HashSet;

fn saved(url: &str, pinned: bool, active: bool, scroll_y: f64) -> SessionTab {
    SessionTab { url: url.into(), title: format!("T {url}"), pinned, scroll_y, active, position: 0, group: String::new(), space: String::new() }
}

#[test]
fn restore_keeps_spaces_and_selects_the_active_tab_space() {
    let hub = SharedHub::new();
    let mut a = saved("https://default.example/", false, false, 0.0);
    a.space = String::new();
    let mut b = saved("https://work.example/", false, true, 0.0);
    b.space = "Work".into();
    let (_, active) = hub.update(|h| restore_into(h, &[a, b]));
    assert_eq!(hub.lock().tab(active.unwrap()).unwrap().space, "Work");
    assert_eq!(hub.lock().current_space(), "Work");
    let snapshot = session_snapshot(&hub.lock(), |_| None);
    assert_eq!(snapshot.iter().map(|t| t.space.as_str()).collect::<Vec<_>>(), vec!["", "Work"]);
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
fn mru_cycle_wraps_and_falls_back_to_the_first() {
    assert_eq!(mru_next(&[3, 2, 1], None, 1), Some(2));
    assert_eq!(mru_next(&[3, 2, 1], Some(3), 1), Some(2));
    assert_eq!(mru_next(&[3, 2, 1], Some(3), -1), Some(1));
    assert_eq!(mru_next(&[3, 2, 1], Some(1), 1), Some(3));
    assert_eq!(mru_next(&[], Some(1), 1), None);
}

#[test]
fn ai_config_requires_an_http_endpoint_and_keeps_the_model() {
    let cfg = ai_config("https://api.test/v1/", "gpt-test").unwrap();
    assert_eq!(cfg.endpoint, "https://api.test/v1");
    assert_eq!(cfg.model, "gpt-test");
    assert!(ai_config("file:///etc/passwd", "m").is_none());
}

#[test]
fn icloud_sync_lives_under_the_cloud_docs_folder() {
    let dir = icloud_relay_dir(std::path::Path::new("/Users/me"));
    assert_eq!(dir, std::path::Path::new("/Users/me/Library/Mobile Documents/com~apple~CloudDocs/Relay"));
}

#[test]
fn handoff_only_advertises_ordinary_web_tabs() {
    assert_eq!(handoff_target("https://a.test/", false, true, false), Some("https://a.test/"));
    assert_eq!(handoff_target("http://a.test/", false, true, false), Some("http://a.test/"));
    assert_eq!(handoff_target("https://a.test/", true, true, false), None);
    assert_eq!(handoff_target("https://a.test/", false, false, false), None);
    assert_eq!(handoff_target("https://a.test/", false, true, true), None);
    assert_eq!(handoff_target("about:blank", false, true, false), None);
}

#[test]
fn sync_file_names_are_profile_specific() {
    assert_eq!(sync_file_name(""), "relay-profile-default.json");
    assert_eq!(sync_file_name("Work"), "relay-profile-Work.json");
}

#[test]
fn profile_names_are_short_and_directory_safe() {
    assert_eq!(profile_name("Work / Client"), "Work-Client");
    assert_eq!(profile_name("  personal  "), "personal");
    assert_eq!(profile_name("a/b\\c"), "a-b-c");
    assert!(profile_name("///").is_empty());
    assert_eq!(profile_name(&"x".repeat(40)).len(), 32);
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

#[test]
fn tab_closing_and_switching_defaults_preserve_upgraded_profiles() {
    // Absent settings preserve existing behavior: start page on last tab close,
    // no multi-tab confirmation, and MRU switching order.
    assert_eq!(LastTabClose::parse(None), LastTabClose::StartPage);
    assert_eq!(LastTabClose::parse(Some("unknown")), LastTabClose::StartPage);
    assert_eq!(LastTabClose::parse(Some("quit")), LastTabClose::Quit);
    assert_eq!(LastTabClose::Quit.as_str(), "quit");
    assert_eq!(LastTabClose::StartPage.as_str(), "start_page");

    assert!(!setting_opt_in(None));
    assert!(setting_opt_in(Some("on")));
    assert!(!setting_opt_in(Some("off")));

    assert_eq!(TabSwitchOrder::parse(None), TabSwitchOrder::Mru);
    assert_eq!(TabSwitchOrder::parse(Some("bogus")), TabSwitchOrder::Mru);
    assert_eq!(TabSwitchOrder::parse(Some("strip")), TabSwitchOrder::Strip);
    assert_eq!(TabSwitchOrder::Mru.as_str(), "mru");
    assert_eq!(TabSwitchOrder::Strip.as_str(), "strip");
}

#[test]
fn tab_closing_and_switching_preferences_persist_in_store() {
    let store = Store::in_memory().unwrap();
    assert_eq!(LastTabClose::parse(store.setting("last_tab_close").unwrap().as_deref()), LastTabClose::StartPage);
    assert!(!setting_opt_in(store.setting("confirm_close_multiple").unwrap().as_deref()));
    assert_eq!(TabSwitchOrder::parse(store.setting("tab_switch_order").unwrap().as_deref()), TabSwitchOrder::Mru);

    store.set_setting("last_tab_close", LastTabClose::Quit.as_str()).unwrap();
    store.set_setting("confirm_close_multiple", "on").unwrap();
    store.set_setting("tab_switch_order", TabSwitchOrder::Strip.as_str()).unwrap();

    assert_eq!(LastTabClose::parse(store.setting("last_tab_close").unwrap().as_deref()), LastTabClose::Quit);
    assert!(setting_opt_in(store.setting("confirm_close_multiple").unwrap().as_deref()));
    assert_eq!(TabSwitchOrder::parse(store.setting("tab_switch_order").unwrap().as_deref()), TabSwitchOrder::Strip);
}

#[test]
fn tab_switch_sequence_switches_between_mru_and_strip_order() {
    let strip = vec![1, 2, 3, 4];
    let mru = vec![3, 1];
    let mru_seq = tab_switch_sequence(TabSwitchOrder::Mru, &mru, &strip);
    assert_eq!(mru_seq, vec![3, 1, 2, 4]);
    assert_eq!(mru_next(&mru_seq, Some(3), 1), Some(1));
    assert_eq!(mru_next(&mru_seq, Some(3), -1), Some(4));

    let strip_seq = tab_switch_sequence(TabSwitchOrder::Strip, &mru, &strip);
    assert_eq!(strip_seq, vec![1, 2, 3, 4]);
    assert_eq!(mru_next(&strip_seq, Some(3), 1), Some(4));
    assert_eq!(mru_next(&strip_seq, Some(1), -1), Some(4));
}

#[test]
fn multi_tab_close_confirmation_and_last_tab_quit_rules() {
    // Confirmation covers multiple tabs (>= 2) only when enabled.
    assert!(!should_confirm_multi_close(false, 5));
    assert!(!should_confirm_multi_close(true, 0));
    assert!(!should_confirm_multi_close(true, 1));
    assert!(should_confirm_multi_close(true, 2));

    // Last-tab quit obeys app_mode or explicit Quit preference, never non-last tabs.
    assert!(!should_quit_on_last_tab(false, LastTabClose::StartPage, true));
    assert!(should_quit_on_last_tab(false, LastTabClose::Quit, true));
    assert!(!should_quit_on_last_tab(false, LastTabClose::Quit, false));
    assert!(should_quit_on_last_tab(true, LastTabClose::StartPage, true));

    // Regression: pinned tabs stay open regardless of last_tab_close / confirm_close_multiple.
    let hub = SharedHub::new();
    let pinned_id = hub.update(|h| {
        let id = h.create_tab("https://pinned.example/", Owner::User);
        h.set_pinned(id, true);
        id
    });
    assert!(hub.lock().tab(pinned_id).unwrap().pinned);
    // Close-other filter never touches pinned tabs.
    let others: Vec<TabId> = hub
        .lock()
        .tabs()
        .filter(|t| !t.pinned && t.owner == Owner::User)
        .map(|t| t.id)
        .collect();
    assert!(others.is_empty(), "pinned tabs remain protected from bulk close");
}

#[test]
fn group_colors_persist_restore_and_fall_back_on_unknown_values() {
    let store = Store::in_memory().unwrap();
    let mut colors = std::collections::BTreeMap::new();
    colors.insert("Work".to_string(), theme::GroupColor::Teal);
    colors.insert("Docs".to_string(), theme::GroupColor::Pink);
    store.set_setting("group_colors", &serialize_group_colors(&colors)).unwrap();

    let loaded = parse_group_colors(store.setting("group_colors").unwrap().as_deref());
    assert_eq!(loaded.get("Work"), Some(&theme::GroupColor::Teal));
    assert_eq!(loaded.get("Docs"), Some(&theme::GroupColor::Pink));
    assert_eq!(resolve_group_color(&loaded, "Work"), theme::GroupColor::Teal);

    // Negative: unknown/deleted colour falls back deterministically without losing the group.
    let corrupted = parse_group_colors(Some(r#"{"Work":"neon","Docs":"orange","Empty":""}"#));
    assert!(!corrupted.contains_key("Work"));
    assert_eq!(corrupted.get("Docs"), Some(&theme::GroupColor::Orange));
    assert_eq!(resolve_group_color(&corrupted, "Work"), theme::GroupColor::for_group_name("Work"));
    assert!(parse_group_colors(Some("not-json")).is_empty());
}

#[test]
fn group_color_rename_and_merge_are_deterministic() {
    let mut colors = std::collections::BTreeMap::new();
    colors.insert("Work".to_string(), theme::GroupColor::Green);
    colors.insert("Archive".to_string(), theme::GroupColor::Purple);

    // Renaming Work -> Projects with no new override carries Work's colour.
    apply_group_color_change(&mut colors, Some("Work"), Some("Projects"), None, false);
    assert!(!colors.contains_key("Work"));
    assert_eq!(colors.get("Projects"), Some(&theme::GroupColor::Green));

    // Merging Projects -> Archive with no override keeps Archive's existing colour.
    apply_group_color_change(&mut colors, Some("Projects"), Some("Archive"), None, false);
    assert!(!colors.contains_key("Projects"));
    assert_eq!(colors.get("Archive"), Some(&theme::GroupColor::Purple));

    // Choosing Default on the same group clears its override.
    apply_group_color_change(&mut colors, Some("Archive"), Some("Archive"), None, false);
    assert!(!colors.contains_key("Archive"));

    // Explicit colour choice always wins and hub::rename_group merges tabs cleanly.
    apply_group_color_change(&mut colors, Some("Archive"), Some("Focus"), Some(theme::GroupColor::Orange), false);
    assert_eq!(colors.get("Focus"), Some(&theme::GroupColor::Orange));

    let hub = SharedHub::new();
    let (a, b) = hub.update(|h| {
        let a = h.create_tab("https://a.example/", Owner::User);
        let b = h.create_tab("https://b.example/", Owner::User);
        h.set_tab_group(a, Some("Old"));
        h.set_tab_group(b, Some("Old"));
        (a, b)
    });
    assert_eq!(hub.update(|h| h.rename_group("Old", Some("New"))), 2);
    assert_eq!(hub.lock().tab(a).unwrap().group.as_deref(), Some("New"));
    assert_eq!(hub.lock().tab(b).unwrap().group.as_deref(), Some("New"));
}

#[test]
fn start_background_persists_validates_local_images_and_rejects_bad_inputs() {
    let store = Store::in_memory().unwrap();
    store.set_setting("start_bg", &theme::StartBackground::Dusk.to_setting()).unwrap();
    let (bg, img) = load_start_background(store.setting("start_bg").unwrap().as_deref());
    assert_eq!(bg, theme::StartBackground::Dusk);
    assert!(img.is_none());

    // Valid local 1x1 PNG loads cleanly and persists without remote fetches.
    let dir = tempfile::tempdir().unwrap();
    let valid_png = dir.path().join("wall.png");
    let png_bytes: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a,
        0, 0, 0, 13, b'I', b'H', b'D', b'R',
        0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0, 0x90, 0x77, 0x53, 0xde,
    ];
    std::fs::write(&valid_png, png_bytes).unwrap();
    let setting = theme::StartBackground::Image(valid_png.to_string_lossy().into_owned()).to_setting();
    store.set_setting("start_bg", &setting).unwrap();
    let (bg, img) = load_start_background(store.setting("start_bg").unwrap().as_deref());
    assert_eq!(bg, theme::StartBackground::Image(valid_png.to_string_lossy().into_owned()));
    assert!(img.is_some());

    // Negative: invalid bytes, missing file, and oversized (> 8 MB) files fall back to Default.
    let bad = dir.path().join("not-image.png");
    std::fs::write(&bad, b"hello world not a png").unwrap();
    assert_eq!(load_start_background(Some(&format!("file:{}", bad.display()))).0, theme::StartBackground::Default);

    let missing = dir.path().join("missing.png");
    assert_eq!(load_start_background(Some(&format!("file:{}", missing.display()))).0, theme::StartBackground::Default);

    let huge = dir.path().join("huge.png");
    let mut big = png_bytes.to_vec();
    big.resize((MAX_WALLPAPER_BYTES as usize) + 64, 0);
    std::fs::write(&huge, &big).unwrap();
    assert_eq!(load_start_background(Some(&format!("file:{}", huge.display()))).0, theme::StartBackground::Default);

    // Remote URLs never load.
    assert_eq!(load_start_background(Some("https://example.test/bg.png")).0, theme::StartBackground::Default);
    assert_eq!(load_start_background(Some("file:https://example.test/bg.png")).0, theme::StartBackground::Default);
}

#[test]
fn status_bar_position_defaults_hidden_cycles_and_honors_legacy_setting() {
    assert_eq!(StatusBarPosition::parse(None, None), StatusBarPosition::Hidden);
    assert_eq!(StatusBarPosition::parse(None, Some("on")), StatusBarPosition::Bottom);
    assert_eq!(StatusBarPosition::parse(None, Some("off")), StatusBarPosition::Hidden);
    assert_eq!(StatusBarPosition::parse(Some("top"), Some("off")), StatusBarPosition::Top);
    assert_eq!(StatusBarPosition::parse(Some("bottom"), None), StatusBarPosition::Bottom);
    assert_eq!(StatusBarPosition::parse(Some("hidden"), Some("on")), StatusBarPosition::Hidden);
    assert_eq!(StatusBarPosition::Hidden.next(), StatusBarPosition::Top);
    assert_eq!(StatusBarPosition::Top.next(), StatusBarPosition::Bottom);
    assert_eq!(StatusBarPosition::Bottom.next(), StatusBarPosition::Hidden);

    let store = Store::in_memory().unwrap();
    store.set_setting("status_bar_position", StatusBarPosition::Top.as_str()).unwrap();
    assert_eq!(
        StatusBarPosition::parse(store.setting("status_bar_position").unwrap().as_deref(), None),
        StatusBarPosition::Top
    );
}

#[test]
fn space_looks_persist_restore_and_fall_back_on_invalid_entries() {
    let store = Store::in_memory().unwrap();
    let mut looks = std::collections::BTreeMap::new();
    looks.insert(
        "Work".to_string(),
        theme::SpaceLook { fill: theme::SpaceFill::Preset(theme::SpaceGradient::Ocean), icon: theme::SpaceIcon::Work },
    );
    looks.insert(
        "Play".to_string(),
        theme::SpaceLook {
            fill: theme::SpaceFill::Custom(theme::GroupColor::Orange, theme::GroupColor::Purple),
            icon: theme::SpaceIcon::Spark,
        },
    );
    store.set_setting("space_looks", &serialize_space_looks(&looks)).unwrap();

    let restored = parse_space_looks(store.setting("space_looks").unwrap().as_deref());
    assert_eq!(restored.get("Work"), looks.get("Work"));
    assert_eq!(restored.get("Play"), looks.get("Play"));

    // Negative: invalid/deleted fill or icon falls back cleanly without losing valid entries.
    let corrupted = parse_space_looks(Some(
        r#"{"Work":{"fill":"preset:deleted","icon":"missing"},"Focus":{"fill":"color:teal","icon":"missing"}}"#,
    ));
    assert!(!corrupted.contains_key("Work"));
    assert_eq!(
        corrupted.get("Focus"),
        Some(&theme::SpaceLook { fill: theme::SpaceFill::Color(theme::GroupColor::Teal), icon: theme::SpaceIcon::Default })
    );
    assert!(parse_space_looks(Some("not-json")).is_empty());
}

#[test]
fn bmp_dib_ico_decodes_to_valid_png_and_ssrf_favicon_urls_are_blocked() {
    // Build a valid 2x2 32-bit BGRA BMP-DIB ICO container (no embedded PNG).
    let mut dib = Vec::new();
    dib.extend_from_slice(&40u32.to_le_bytes()); // BITMAPINFOHEADER size
    dib.extend_from_slice(&2i32.to_le_bytes());  // width = 2
    dib.extend_from_slice(&4i32.to_le_bytes());  // height = 2 * 2 (XOR + AND)
    dib.extend_from_slice(&1u16.to_le_bytes());  // planes
    dib.extend_from_slice(&32u16.to_le_bytes()); // bpp = 32
    dib.extend_from_slice(&0u32.to_le_bytes());  // BI_RGB
    dib.extend_from_slice(&0u32.to_le_bytes());  // image size
    dib.extend_from_slice(&0i32.to_le_bytes());
    dib.extend_from_slice(&0i32.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes());
    // 2x2 BGRA pixels (16 bytes)
    for _ in 0..4 {
        dib.extend_from_slice(&[0xff, 0x80, 0x20, 0xff]);
    }
    // 2x2 AND mask: 4-byte aligned per row -> 8 bytes
    dib.extend_from_slice(&[0u8; 8]);

    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0, 1, 0]); // ICO header, 1 entry
    let offset = 6 + 16u32;
    ico.extend_from_slice(&[2, 2, 0, 0, 1, 0, 32, 0]);
    ico.extend_from_slice(&(dib.len() as u32).to_le_bytes());
    ico.extend_from_slice(&offset.to_le_bytes());
    ico.extend_from_slice(&dib);

    let (png, ext) = favicon_bytes_to_payload(ico).expect("BMP-DIB ICO should decode to PNG");
    assert_eq!(ext, "png");
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(favicon_dimensions(&png, "png"), Some((2, 2)));

    // Truncated / non-image payloads fail closed.
    assert!(favicon_bytes_to_payload(vec![0, 0, 1, 0, 1, 0]).is_none());
    assert!(favicon_bytes_to_payload(b"<html>not an icon</html>".to_vec()).is_none());

    // SSRF guard: only public CDN icons are allowed in the native fallback; local/private/CGNAT/IPv6/obfuscated
    // hosts are ALWAYS blocked (even when matching tab_host, since same-origin local icons load in-page).
    assert!(favicon_url_allowed("github.com", "https://github.githubassets.com/favicons/favicon.svg"));
    assert!(!favicon_url_allowed("localhost", "http://localhost:3000/favicon.ico"));
    assert!(!favicon_url_allowed("[::1]", "http://[::1]:3000/favicon.ico"));
    assert!(!favicon_url_allowed("127.0.0.1", "http://127.0.0.1:8080/favicon.ico"));
    for blocked in [
        "http://127.0.0.1/favicon.ico",
        "http://127.1/favicon.ico",
        "http://0x7f000001/favicon.ico",
        "http://2130706433/favicon.ico",
        "http://0177.0.0.1/favicon.ico",
        "http://100.64.0.1/favicon.ico",
        "http://169.254.169.254/latest/meta-data/",
        "http://192.168.1.1/icon.png",
        "http://10.0.0.1/icon.png",
        "http://[fe80::1]/icon.png",
        "http://[fc00::1]:8080/icon.png",
        "http://[::1]/icon.png",
        "http://[::ffff:127.0.0.1]/icon.png",
        "http://[::127.0.0.1]/icon.png",
        "http://[64:ff9b::127.0.0.1]/icon.png",
        "http://localhost/icon.png",
        "file:///etc/passwd",
    ] {
        assert!(!favicon_url_allowed("example.com", blocked), "must block SSRF URL: {blocked}");
    }
    assert!(favicon_ip_public("1.1.1.1".parse().unwrap()));
    assert!(!favicon_ip_public("100.64.0.1".parse().unwrap()));
    assert!(!favicon_ip_public("fe80::1".parse().unwrap()));
    assert!(!favicon_ip_public("fc00::1".parse().unwrap()));
    assert!(!favicon_ip_public("::ffff:127.0.0.1".parse().unwrap()));
    assert!(!favicon_ip_public("::127.0.0.1".parse().unwrap()));
    assert!(!favicon_ip_public("64:ff9b::127.0.0.1".parse().unwrap()));
}

#[test]
fn agent_composer_key_capture_yields_to_find_hints_modals_and_inspector() {
    assert!(agent_composer_captures_keys(true, SidebarTab::Agent, true, false, false, false));
    assert!(!agent_composer_captures_keys(false, SidebarTab::Agent, true, false, false, false), "closed sidebar");
    assert!(!agent_composer_captures_keys(true, SidebarTab::Inspector, true, false, false, false), "inspector tab");
    assert!(!agent_composer_captures_keys(true, SidebarTab::Agent, false, false, false, false), "unfocused input");
    assert!(!agent_composer_captures_keys(true, SidebarTab::Agent, true, true, false, false), "modal overlay");
    assert!(!agent_composer_captures_keys(true, SidebarTab::Agent, true, false, true, false), "find bar active");
    assert!(!agent_composer_captures_keys(true, SidebarTab::Agent, true, false, false, true), "link hints active");
}
