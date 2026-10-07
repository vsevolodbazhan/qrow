use crate::support::{
    TestApp, assert_workspace_divider_alignment, assert_workspace_header_alignment, bounds_of,
    offline_profile, value,
};
use gpui_kit::TestAppContext;
use qrow::model::{SavedTab, Workspace};
use std::time::Duration;
use uuid::Uuid;

#[gpui_kit::test]
fn sidebar_headers_align_with_query_tabs_at_each_scale_and_width(cx: &mut TestAppContext) {
    for scale in [0.75, 1., 1.1, 1.25, 1.5] {
        let profile = offline_profile("Header alignment");
        let mut workspace = Workspace {
            tabs: vec![SavedTab::new(1, Some(profile.id))],
            profiles: vec![profile],
            ..Workspace::default()
        };
        workspace.settings.ui_scale = scale;
        let app = TestApp::launch(cx, workspace);
        for display_scale in [1., 2.] {
            cx.simulate_window_scale_factor_change(app.window, display_scale);
            for width in [850., 1280.] {
                cx.simulate_window_resize(
                    app.window,
                    gpui_kit::size(gpui_kit::px(width), gpui_kit::px(720.)),
                );
                app.settle(cx);
                app.update(cx, |window, cx| {
                    assert_workspace_header_alignment(window, "add-connection");
                    assert_workspace_divider_alignment(window, cx);
                });
                app.click(cx, "show-sign-ins");
                app.update(cx, |window, cx| {
                    assert_workspace_header_alignment(window, "add-sign-in");
                    assert_workspace_divider_alignment(window, cx);
                });
                app.click(cx, "show-connections");
            }
        }
    }
}

#[gpui_kit::test]
fn divider_drag_targets_resize_from_both_edges_and_release(cx: &mut TestAppContext) {
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{
        InputEvent as _, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, point, px,
    };

    for scale in [0.75, 1., 1.1, 1.25, 1.5] {
        let profile = offline_profile("Resize");
        let mut workspace = Workspace {
            tabs: vec![SavedTab::new(1, Some(profile.id))],
            profiles: vec![profile],
            ..Workspace::default()
        };
        workspace.settings.ui_scale = scale;
        let app = TestApp::launch(cx, workspace);
        cx.simulate_window_resize(app.window, gpui_kit::size(px(1280.), px(1400.)));
        for display in [1., 2.] {
            cx.simulate_window_scale_factor_change(app.window, display);
            app.settle(cx);
            for horizontal in [true, false] {
                for edge in [0., 0.5, 1.] {
                    let divider = if horizontal {
                        "sidebar-divider"
                    } else {
                        "editor-divider"
                    };
                    let handle = if horizontal {
                        "sidebar-splitter"
                    } else {
                        "editor-splitter"
                    };
                    let before = app.update(cx, |window, cx| {
                        let before = bounds_of(window, divider);
                        let target = bounds_of(window, handle);
                        let inset = px(0.25 / display);
                        let start = if horizontal {
                            point(
                                target.left() + inset + (target.size.width - 2. * inset) * edge,
                                target.center().y,
                            )
                        } else {
                            point(
                                target.center().x,
                                target.top() + inset + (target.size.height - 2. * inset) * edge,
                            )
                        };
                        window.dispatch_event(
                            MouseDownEvent {
                                button: MouseButton::Left,
                                position: start,
                                modifiers: Default::default(),
                                click_count: 1,
                                first_mouse: false,
                            }
                            .to_platform_input(),
                            cx,
                        );
                        // Reverse on the second display scale to stay away from
                        // the pane width limits throughout the matrix.
                        let distance = px(if display == 1. { 20. } else { -20. });
                        let end = if horizontal {
                            start + point(distance, px(0.))
                        } else {
                            start + point(px(0.), distance)
                        };
                        window.dispatch_event(
                            MouseMoveEvent {
                                position: end,
                                pressed_button: Some(MouseButton::Left),
                                modifiers: Default::default(),
                            }
                            .to_platform_input(),
                            cx,
                        );
                        window.dispatch_event(
                            MouseUpEvent {
                                button: MouseButton::Left,
                                position: end,
                                modifiers: Default::default(),
                                click_count: 1,
                            }
                            .to_platform_input(),
                            cx,
                        );
                        window.render_frame(cx);
                        before
                    });
                    app.settle(cx);
                    app.update(cx, |window, cx| {
                        let after = bounds_of(window, divider);
                        let distance = px(if display == 1. { 20. } else { -20. });
                        assert_eq!(if horizontal { after.left() - before.left() } else { after.top() - before.top() }, distance, "The full drag target works: {handle}, edge {edge}, UI {scale}, display {display}");
                        assert_workspace_divider_alignment(window, cx);
                    });
                }
            }
        }
        // A released drag does not consume the next click or obstruct a dialog.
        app.click(cx, "new-tab");
        app.wait_until(cx, "new tab", Duration::from_secs(2), |_, _| {
            app.saved().tabs.len() == 2
        });
        app.dispatch(cx, qrow::ui::OpenSettings);
        app.wait_for(cx, "setting-ui-scale");
        app.press(cx, "escape");
        app.wait_gone(cx, "setting-ui-scale");
    }
}

fn tab(profile: Uuid, title: &str, sql: &str) -> SavedTab {
    SavedTab {
        title: title.into(),
        sql: sql.into(),
        ..SavedTab::new(1, Some(profile))
    }
}

/// Titles and SQL of the saved tabs of a connection, in tab order.
fn saved_tabs(app: &TestApp, profile: Uuid) -> Vec<(String, String)> {
    app.saved()
        .tabs
        .into_iter()
        .filter(|tab| tab.profile == Some(profile))
        .map(|tab| (tab.title, tab.sql))
        .collect()
}

fn wait_tabs(app: &TestApp, cx: &mut TestAppContext, profile: Uuid, expected: &[(&str, &str)]) {
    let expected: Vec<_> = expected
        .iter()
        .map(|(t, s)| (t.to_string(), s.to_string()))
        .collect();
    app.wait_until(
        cx,
        &format!("tabs {expected:?}"),
        Duration::from_secs(10),
        |_, _| saved_tabs(app, profile) == expected,
    );
}

fn rename(app: &TestApp, cx: &mut TestAppContext, title: &str) {
    app.context_menu_labelled(cx, title);
    app.choose(cx, "popup-menu", "Rename…");
    app.wait_for(cx, "rename-tab-name");
}

#[gpui_kit::test]
fn duplicates_get_unique_names_and_keep_the_sql(cx: &mut TestAppContext) {
    let alpha = offline_profile("Alpha");
    let id = alpha.id;
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![tab(id, "Query 1", "SELECT 1")],
            profiles: vec![alpha],
            ..Workspace::default()
        },
    );
    for expected in ["Query 1 (Copy)", "Query 1 (Copy 2)"] {
        app.click_labelled(cx, "Query 1");
        app.context_menu_labelled(cx, "Query 1");
        app.choose(cx, "popup-menu", "Duplicate");
        app.wait_until(cx, expected, Duration::from_secs(10), |_, _| {
            saved_tabs(&app, id)
                .iter()
                .any(|(title, _)| title == expected)
        });
    }
    wait_tabs(
        &app,
        cx,
        id,
        &[
            ("Query 1", "SELECT 1"),
            ("Query 1 (Copy)", "SELECT 1"),
            ("Query 1 (Copy 2)", "SELECT 1"),
        ],
    );
}

#[gpui_kit::test]
fn a_rename_rejects_taken_and_long_names(cx: &mut TestAppContext) {
    let alpha = offline_profile("Alpha");
    let id = alpha.id;
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![
                tab(id, "Query 1", "SELECT 1"),
                tab(id, "Query 2", "SELECT 2"),
            ],
            profiles: vec![alpha],
            ..Workspace::default()
        },
    );
    rename(&app, cx, "Query 1");
    app.update(cx, |window, _| {
        assert_eq!(value(window, "rename-tab-name").as_deref(), Some("Query 1"))
    });
    // Each rejected name keeps the dialog open and the title unchanged.
    for rejected in ["Query 2".to_owned(), "x".repeat(61)] {
        app.fill(cx, "rename-tab-name", &rejected);
        app.click(cx, "rename-tab");
        app.settle(cx);
        app.wait_for(cx, "rename-tab-name");
        wait_tabs(
            &app,
            cx,
            id,
            &[("Query 1", "SELECT 1"), ("Query 2", "SELECT 2")],
        );
    }
    app.click(cx, "cancel-tab");
    app.wait_gone(cx, "rename-tab-name");

    rename(&app, cx, "Query 1");
    app.fill(cx, "rename-tab-name", "Renamed tab");
    app.click(cx, "rename-tab");
    app.wait_gone(cx, "rename-tab-name");
    wait_tabs(
        &app,
        cx,
        id,
        &[("Renamed tab", "SELECT 1"), ("Query 2", "SELECT 2")],
    );
    // The same name again closes the dialog without a change.
    rename(&app, cx, "Renamed tab");
    app.click(cx, "rename-tab");
    app.wait_gone(cx, "rename-tab-name");
    wait_tabs(
        &app,
        cx,
        id,
        &[("Renamed tab", "SELECT 1"), ("Query 2", "SELECT 2")],
    );
}

#[gpui_kit::test]
fn copy_and_move_to_another_connection(cx: &mut TestAppContext) {
    let (alpha, beta) = (offline_profile("Alpha"), offline_profile("Beta"));
    let (a, b) = (alpha.id, beta.id);
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![tab(a, "Query 1", "SELECT 'moved'"), tab(b, "Query 1", "")],
            profiles: vec![alpha, beta],
            ..Workspace::default()
        },
    );
    app.context_menu_labelled(cx, "Query 1");
    app.choose_in_submenu(cx, "Copy to connection…", "Beta");
    wait_tabs(
        &app,
        cx,
        b,
        &[("Query 1", ""), ("Query 1 (Copy)", "SELECT 'moved'")],
    );
    wait_tabs(&app, cx, a, &[("Query 1", "SELECT 'moved'")]);

    // Copy selected Beta. Return to Alpha and move its only tab.
    app.click(cx, crate::support::connection_row(a));
    app.wait_until(cx, "Alpha's tab", Duration::from_secs(10), |window, _| {
        crate::support::labelled(window, "Query 1").is_some()
    });
    app.context_menu_labelled(cx, "Query 1");
    app.choose_in_submenu(cx, "Move to connection…", "Beta");
    wait_tabs(
        &app,
        cx,
        b,
        &[
            ("Query 1", ""),
            ("Query 1 (Copy)", "SELECT 'moved'"),
            ("Query 1 (Copy 2)", "SELECT 'moved'"),
        ],
    );
    // Moving the last tab leaves the source connection with a new blank tab.
    wait_tabs(&app, cx, a, &[("Query 1", "")]);
}
