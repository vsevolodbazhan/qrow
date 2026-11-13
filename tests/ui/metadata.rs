//! Related values have separate layout and accessible names.
use crate::support::{
    TestApp, assert_tooltip_header_center, assert_tooltip_metadata_rows, bounds_of, label,
    offline_profile,
};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, px, size};
use qrow::model::{SavedTab, Workspace};
use qrow::ui::{DecreaseUiScale, IncreaseUiScale};
use std::time::Duration;

fn assert_toolbar_fits(app: &TestApp, cx: &mut TestAppContext) {
    app.update(cx, |window, _| {
        let toolbar = bounds_of(window, "query-footer");
        assert_eq!(
            toolbar.size.height,
            window.pixel_snap(window.rem_size() * 2.5)
        );
        let mut right = toolbar.left();
        for id in [
            "result-count",
            "result-partial",
            "result-details",
            "export-results",
            "copy-result-query",
            "pin-result",
            "page-label",
            "previous-page",
            "next-page",
        ] {
            if window.try_find(id).is_none() {
                continue;
            }
            let child = bounds_of(window, id);
            assert!(
                child.left() >= right && child.right() <= toolbar.right(),
                "{id} overlaps or extends outside the toolbar: {child:?}, {toolbar:?}"
            );
            assert!(child.top() >= toolbar.top() && child.bottom() <= toolbar.bottom());
            right = child.right();
        }
    });
}

#[gpui_kit::test]
fn result_metadata_adapts_to_pane_width_and_ui_scale(cx: &mut TestAppContext) {
    for (scale, action_count, increase) in [(0.75, 3, false), (1., 0, true), (1.5, 5, true)] {
        let app = TestApp::launch_demo(cx);
        for _ in 0..action_count {
            if increase {
                app.dispatch(cx, IncreaseUiScale);
            } else {
                app.dispatch(cx, DecreaseUiScale);
            }
        }
        let mut pagination_width = None;
        // 75% is the lower limit; three decreases clamp the scale to it.
        // A narrow window narrows the sidebar first, so the narrowest width
        // leaves the results pane about as wide as a 500-point window did
        // with a full sidebar.
        for width in [1900., 850., 700., 450., 1900.] {
            cx.simulate_window_resize(app.window, size(px(width * scale), px(650. * scale)));
            app.settle(cx);
            assert_toolbar_fits(&app, cx);
            let compact = app.update(cx, |window, _| {
                let compact = window.try_find("result-details").is_some();
                if width == 1900. {
                    assert!(!compact, "A wide panel shows all result metadata");
                } else if width <= 700. {
                    assert!(compact, "A narrow panel uses the details popover");
                }
                let current_width = bounds_of(window, "next-page").right()
                    - bounds_of(window, "previous-page").left();
                let expected_width = *pagination_width.get_or_insert(current_width);
                assert_eq!(
                    current_width, expected_width,
                    "pagination keeps its icon width"
                );
                for (id, name) in [
                    ("previous-page", "Previous page"),
                    ("next-page", "Next page"),
                ] {
                    assert_eq!(label(window, id).as_deref(), Some(name));
                }
                if !compact {
                    assert_eq!(
                        label(window, "result-range").as_deref(),
                        Some("Rows 1–1,000")
                    );
                    assert_eq!(
                        label(window, "result-loaded").as_deref(),
                        Some("2,250 loaded")
                    );
                    assert_eq!(
                        label(window, "result-columns").as_deref(),
                        Some("141 columns")
                    );
                    assert!(
                        label(window, "result-elapsed")
                            .unwrap()
                            .starts_with("Elapsed: ")
                    );
                } else {
                    assert_eq!(
                        label(window, "result-details").as_deref(),
                        Some("Show result details")
                    );
                    assert!(window.try_find("result-loaded").is_none());
                    assert!(window.try_find("result-columns").is_none());
                    assert!(window.try_find("result-elapsed").is_none());
                    if width == 450. {
                        assert!(window.try_find("result-range").is_none());
                    }
                }
                compact
            });
            if compact {
                app.click(cx, "result-details");
                app.update(cx, |window, _| {
                    assert_eq!(
                        label(window, "result-detail-loaded").as_deref(),
                        Some("Loaded Rows: 2,250")
                    );
                    assert_eq!(
                        label(window, "result-detail-columns").as_deref(),
                        Some("Columns: 141")
                    );
                    assert!(
                        label(window, "result-detail-elapsed")
                            .unwrap()
                            .starts_with("Query Duration: ")
                    );
                    let content = bounds_of(window, "result-details-content");
                    assert!(
                        content.left() >= px(0.) && content.right() <= window.viewport_size().width
                    );
                });
                app.press(cx, "escape");
                app.update(cx, |window, _| {
                    assert!(window.try_find("result-details-content").is_none())
                });
            }
        }
    }
}

#[gpui_kit::test]
fn result_metadata_tooltips_match_detail_labels(cx: &mut TestAppContext) {
    let app = TestApp::launch_demo(cx);
    let mut titles = Vec::new();
    for (tag, detail) in [
        ("result-range", "result-detail-range"),
        ("result-loaded", "result-detail-loaded"),
        ("result-columns", "result-detail-columns"),
        ("result-elapsed", "result-detail-elapsed"),
    ] {
        app.update(cx, |window, cx| window.hover(tag, cx));
        cx.executor().advance_clock(Duration::from_millis(800));
        app.settle(cx);
        let title = app.update(cx, |window, _| {
            label(window, "status-tooltip-title").expect("metadata tag has a tooltip")
        });
        titles.push((detail, title));
    }
    assert_eq!(
        titles
            .iter()
            .map(|(_, title)| title.as_str())
            .collect::<Vec<_>>(),
        ["Visible Rows", "Loaded Rows", "Columns", "Query Duration"]
    );
    cx.simulate_window_resize(app.window, size(px(700.), px(650.)));
    app.settle(cx);
    app.click(cx, "result-details");
    app.update(cx, |window, _| {
        for (detail, title) in titles {
            let row = label(window, detail).expect("detail row has an accessible label");
            assert_eq!(row.split_once(": ").unwrap().0, title);
        }
    });
}

#[gpui_kit::test]
fn result_details_support_keyboard_dismissal_and_page_updates(cx: &mut TestAppContext) {
    let app = TestApp::launch_demo(cx);
    cx.simulate_window_resize(app.window, size(px(700.), px(650.)));
    app.settle(cx);
    for _ in 0..40 {
        if app.update(cx, |window, _| {
            window.find("result-details").focused() == Some(true)
        }) {
            break;
        }
        // The SQL editor uses Tab for indentation. Enter the native control
        // focus traversal before testing the popover's keyboard commands.
        app.update(cx, |window, cx| {
            window.focus_next(cx);
            window.render_frame(cx);
        });
    }
    app.update(cx, |window, _| {
        assert_eq!(window.find("result-details").focused(), Some(true))
    });
    app.press(cx, "enter");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "result-detail-range").as_deref(),
            Some("Visible Rows: 1–1,000")
        );
    });
    app.press(cx, "escape");
    app.update(cx, |window, _| {
        assert!(window.try_find("result-details-content").is_none());
        assert_eq!(window.find("result-details").focused(), Some(true));
    });
    app.press(cx, "space");
    app.update(cx, |window, _| {
        assert!(window.try_find("result-details-content").is_some());
    });
    app.press(cx, "escape");
    app.click(cx, "previous-page");
    app.update(cx, |window, _| {
        assert_eq!(label(window, "page-label").as_deref(), Some("Page 1"))
    });
    app.click(cx, "next-page");
    app.update(cx, |window, _| {
        assert_eq!(label(window, "page-label").as_deref(), Some("Page 2"))
    });
    app.click(cx, "result-details");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "result-detail-range").as_deref(),
            Some("Visible Rows: 1,001–2,000")
        )
    });
    // Outside clicks dismiss without consuming the target command.
    app.click(cx, "next-page");
    app.update(cx, |window, _| {
        assert!(window.try_find("result-details-content").is_none());
        assert_eq!(label(window, "page-label").as_deref(), Some("Page 3"));
    });
}

#[gpui_kit::test]
fn shortcuts_and_connection_fields_use_tooltip_parts(cx: &mut TestAppContext) {
    // Preserve punctuation in user text. Remove separators only from app copy.
    let mut profile = offline_profile("Warehouse · East");
    profile.host = "127.0.0.1".into();
    profile.username = "synthetic · user".into();
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![SavedTab::new(1, Some(profile.id))],
            profiles: vec![profile],
            ..Workspace::default()
        },
    );
    for (target, title, shortcut) in [
        (
            "new-tab",
            "New tab",
            if cfg!(target_os = "macos") {
                "⌘T"
            } else {
                "Ctrl+T"
            },
        ),
        (
            "run",
            "Run query",
            if cfg!(target_os = "macos") {
                "⌘⏎"
            } else {
                "Ctrl+Enter"
            },
        ),
    ] {
        app.update(cx, |window, cx| window.hover(target, cx));
        cx.executor().advance_clock(Duration::from_millis(800));
        app.settle(cx);
        app.update(cx, |window, cx| {
            assert_eq!(
                label(window, "status-tooltip-title").as_deref(),
                Some(title)
            );
            assert_eq!(
                label(window, "status-tooltip-shortcut").as_deref(),
                Some(shortcut)
            );
            assert!(window.try_find("status-tooltip-status").is_none());
            assert_tooltip_header_center(window, cx, "status-tooltip-shortcut");
        });
    }
    // ⌘B shows the Connections sidebar. The Sign-ins button has no shortcut.
    app.update(cx, |window, cx| window.hover("show-connections", cx));
    cx.executor().advance_clock(Duration::from_millis(800));
    app.settle(cx);
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "status-tooltip-title").as_deref(),
            Some("Connections")
        );
        assert_eq!(
            label(window, "status-tooltip-shortcut").as_deref(),
            Some(if cfg!(target_os = "macos") {
                "⌘B"
            } else {
                "Ctrl+B"
            })
        );
    });
    app.hover_labelled(cx, "Warehouse · East");
    cx.executor().advance_clock(Duration::from_millis(800));
    app.settle(cx);
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "status-tooltip-title").as_deref(),
            Some("Warehouse · East")
        );
        assert_eq!(
            label(window, "status-tooltip-Host").as_deref(),
            Some("Host: 127.0.0.1")
        );
        assert_eq!(
            label(window, "status-tooltip-User").as_deref(),
            Some("User: synthetic · user")
        );
        assert_tooltip_metadata_rows(window);
    });
}

#[cfg(target_os = "linux")]
#[gpui_kit::test]
fn application_menu_icon_lines_up_with_the_sidebar_title(cx: &mut TestAppContext) {
    let app = TestApp::launch_demo(cx);
    for zoom in [false, true] {
        if zoom {
            app.press(cx, "secondary-=");
            app.settle(cx);
        }
        app.update(cx, |window, _| {
            let button = bounds_of(window, "application-menu");
            let title = bounds_of(window, "sidebar-title");
            // The small menu icon is 0.875 rem wide, centered in the button,
            // and its ink starts 3/24 of the icon in.
            let icon = window.rem_size() * 0.875;
            let ink = button.left() + (button.size.width - icon) / 2. + icon * (3. / 24.);
            assert!(
                (ink - title.left()).abs() < px(0.5),
                "zoom {zoom}: icon ink at {ink:?}, title at {:?}",
                title.left()
            );
        });
    }
}

#[cfg(target_os = "linux")]
#[gpui_kit::test]
fn application_menu_opens_the_settings_window(cx: &mut TestAppContext) {
    let app = TestApp::launch_demo(cx);
    app.click(cx, "application-menu");
    app.wait_label(cx, "Settings…");
    app.wait_label(cx, "File");
    app.wait_label(cx, "Edit");
    app.wait_label(cx, "View");
    app.click_labelled(cx, "Settings…");
    cx.run_until_parked();
    assert!(app.settings_window(cx).is_some());
    app.press(cx, "secondary-w");
    cx.run_until_parked();
    assert!(app.settings_window(cx).is_none());
    app.click(cx, "application-menu");
    app.click_labelled(cx, "Settings…");
    cx.run_until_parked();
    assert!(app.settings_window(cx).is_some());
}

#[cfg(target_os = "linux")]
#[gpui_kit::test]
fn application_file_menu_creates_a_query_tab(cx: &mut TestAppContext) {
    let profile = offline_profile("Warehouse");
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![SavedTab::new(1, Some(profile.id))],
            profiles: vec![profile],
            ..Workspace::default()
        },
    );
    app.click(cx, "application-menu");
    app.choose_in_submenu(cx, "File", "New query tab");
    app.wait_label(cx, "Query 2");
}

#[cfg(target_os = "linux")]
#[gpui_kit::test]
fn application_edit_menu_targets_the_editor(cx: &mut TestAppContext) {
    let profile = offline_profile("Warehouse");
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![SavedTab::new(1, Some(profile.id))],
            profiles: vec![profile],
            ..Workspace::default()
        },
    );
    app.type_sql(cx, "SELECT 123");
    app.press(cx, "end");
    app.click(cx, "application-menu");
    app.choose_in_submenu(cx, "Edit", "Select all");
    app.press(cx, "secondary-c");
    assert_eq!(
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .as_deref(),
        Some("SELECT 123")
    );
}
