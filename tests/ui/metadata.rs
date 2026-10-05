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
            "panel-switcher",
            "result-count",
            "result-details",
            "page-label",
            "pagination-buttons",
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
        // 75% is the lower limit; three decreases clamp the scale to it.
        for width in [1900., 850., 700., 560., 1900.] {
            cx.simulate_window_resize(app.window, size(px(width * scale), px(650. * scale)));
            app.settle(cx);
            assert_toolbar_fits(&app, cx);
            app.update(cx, |window, _| {
                if width == 1900. {
                    assert!(window.try_find("result-details").is_none());
                    assert_eq!(
                        label(window, "result-range").as_deref(),
                        Some("Rows 1–1000")
                    );
                    assert_eq!(
                        label(window, "result-loaded").as_deref(),
                        Some("2250 loaded")
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
                        Some("Result Details")
                    );
                    assert!(window.try_find("result-loaded").is_none());
                    assert!(window.try_find("result-columns").is_none());
                    assert!(window.try_find("result-elapsed").is_none());
                    if width == 560. {
                        assert!(window.try_find("result-range").is_none());
                    }
                }
            });
            if width < 1900. {
                app.click(cx, "result-details");
                app.update(cx, |window, _| {
                    assert_eq!(
                        label(window, "result-detail-loaded").as_deref(),
                        Some("Loaded rows: 2250")
                    );
                    assert_eq!(
                        label(window, "result-detail-columns").as_deref(),
                        Some("Columns: 141")
                    );
                    assert!(
                        label(window, "result-detail-elapsed")
                            .unwrap()
                            .starts_with("Query duration: ")
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
        ["Visible rows", "Loaded rows", "Columns", "Query duration"]
    );
    cx.simulate_window_resize(app.window, size(px(850.), px(650.)));
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
    cx.simulate_window_resize(app.window, size(px(850.), px(650.)));
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
            Some("Visible rows: 1–1000")
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
            Some("Visible rows: 1001–2000")
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
    for (target, title, shortcut) in [("new-tab", "New Tab", "⌘T"), ("run", "Run Query", "⌘⏎")]
    {
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
            Some("⌘B")
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
