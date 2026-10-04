//! Related values have separate layout and accessible names.
use crate::support::{
    TestApp, assert_tooltip_header_center, assert_tooltip_metadata_rows, bounds_of, label,
    offline_profile,
};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, px, size};
use qrow::model::{SavedTab, Workspace};
use qrow::ui::IncreaseUiScale;
use std::time::Duration;

#[gpui_kit::test]
fn result_and_status_metadata_fit_at_small_window_sizes(cx: &mut TestAppContext) {
    let app = TestApp::launch_demo(cx);
    for scale in [1., 1.5] {
        if scale > 1. {
            for _ in 0..5 {
                app.dispatch(cx, IncreaseUiScale);
            }
        }
        cx.simulate_window_resize(app.window, size(px(850.), px(560.)));
        app.settle(cx);
        assert_eq!(cx.update(|cx| app.status(cx)), "Complete: Demo data");
        app.update(cx, |window, _| {
            assert!(
                label(window, "result-elapsed")
                    .unwrap()
                    .starts_with("Elapsed: ")
            );
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
            for (parent, children) in [(
                "result-count",
                &[
                    "result-range",
                    "result-loaded",
                    "result-columns",
                    "result-elapsed",
                ][..],
            )] {
                let parent = bounds_of(window, parent);
                for id in children {
                    let child = bounds_of(window, id);
                    assert!(
                        child.left() >= parent.left() && child.right() <= parent.right(),
                        "{id} extends beyond its metadata group at {scale}: {child:?}, {parent:?}"
                    );
                    assert!(child.top() >= parent.top() && child.bottom() <= parent.bottom());
                }
            }
        });
    }
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
    // The sidebar buttons have no shortcut, so the tooltip shows the title
    // alone.
    app.update(cx, |window, cx| window.hover("show-connections", cx));
    cx.executor().advance_clock(Duration::from_millis(800));
    app.settle(cx);
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "status-tooltip-title").as_deref(),
            Some("Connections")
        );
        assert!(window.try_find("status-tooltip-shortcut").is_none());
        assert!(window.try_find("status-tooltip-status").is_none());
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
