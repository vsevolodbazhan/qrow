use crate::support::{
    TestApp, assert_workspace_divider_alignment, assert_workspace_header_alignment, bounds_of,
    labelled, offline_profile, value,
};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{InputEvent as _, TestAppContext, px};
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
                    let search = bounds_of(window, "Search relations");
                    let run = bounds_of(window, "run");
                    assert!(
                        (search.center().y - run.center().y).abs() <= gpui_kit::px(0.5),
                        "The relation search {search:?} must share the Run button centerline {run:?}"
                    );
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

#[gpui_kit::test]
fn a_full_tab_strip_scrolls_with_the_wheel(cx: &mut TestAppContext) {
    let profile = offline_profile("Many tabs");
    let tabs: Vec<_> = (1..=10)
        .map(|number| SavedTab::new(number, Some(profile.id)))
        .collect();
    let workspace = Workspace {
        tabs,
        profiles: vec![profile],
        ..Workspace::default()
    };
    let app = TestApp::launch(cx, workspace);
    cx.simulate_window_resize(app.window, gpui_kit::size(px(900.), px(770.)));
    app.settle(cx);
    let strip = app.update(cx, |window, _| bounds_of(window, "Query 2").center());
    // A horizontal swipe and a vertical wheel both scroll the strip.
    for delta in [
        gpui_kit::point(px(-120.), px(0.)),
        gpui_kit::point(px(0.), px(-120.)),
    ] {
        let before = app.update(cx, |window, _| bounds_of(window, "Query 1").left());
        app.update(cx, |window, cx| {
            window.dispatch_event(
                gpui_kit::ScrollWheelEvent {
                    position: strip,
                    delta: gpui_kit::ScrollDelta::Pixels(delta),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        });
        app.settle(cx);
        let after = app.update(cx, |window, _| bounds_of(window, "Query 1").left());
        assert_eq!(
            after,
            before - px(120.),
            "The wheel {delta:?} must scroll the tab strip"
        );
    }
    // When the wheel stops, the strip snaps so that a tab starts exactly at
    // its start edge.
    let strip_left = app.update(cx, |window, _| {
        bounds_of(window, "Query 1").left() + px(240.)
    });
    cx.executor().advance_clock(Duration::from_millis(300));
    app.settle(cx);
    app.settle(cx);
    app.update(cx, |window, _| assert_no_tab_cut_at(window, strip_left, 10));
    app.click(cx, "new-tab");
    app.settle(cx);
    app.update(cx, |window, _| {
        let tab = bounds_of(window, "Query 11");
        let new_tab = bounds_of(window, "new-tab");
        assert!(
            tab.right() <= new_tab.left(),
            "The new tab {tab:?} must scroll into view before {new_tab:?}"
        );
    });
}

/// No query tab "Query 1" to "Query {count}" starts before `edge` and ends
/// after it.
fn assert_no_tab_cut_at(window: &gpui_kit::Window, edge: gpui_kit::Pixels, count: usize) {
    for number in 1..=count {
        let title = format!("Query {number}");
        // A tab outside the strip is not drawn.
        if let Some(tab) = labelled(window, &title) {
            let tab = tab.bounds();
            assert!(
                !(tab.left() < edge - px(0.5) && tab.right() > edge + px(0.5)),
                "{title} {tab:?} is cut at the strip edge {edge:?}"
            );
        }
    }
}

/// A query tab that an edge of the strip cuts draws no close button.
fn assert_cut_tabs_hide_close(window: &gpui_kit::Window, edge: gpui_kit::Pixels, count: usize) {
    let end = bounds_of(window, "new-tab").left();
    for number in 1..=count {
        let title = format!("Query {number}");
        let Some(tab) = labelled(window, &title) else {
            continue;
        };
        let tab = tab.bounds();
        if tab.left() < edge - px(0.5) || tab.right() > end + px(0.5) {
            assert!(
                labelled(window, &format!("Close {title}")).is_none_or(|close| !close.visible()),
                "{title} {tab:?} is cut by the strip, so it must not draw its close button"
            );
        }
    }
}

/// The bounds of the selected query tab and of the strip that shows it,
/// from the first tab edge after the sidebar to the new-tab button.
fn selected_tab_in_strip(app: &TestApp, cx: &mut TestAppContext, title: &str) {
    app.update(cx, |window, _| {
        let tab = bounds_of(window, title);
        let strip_left = bounds_of(window, "add-connection").right();
        let new_tab = bounds_of(window, "new-tab");
        assert!(
            tab.left() >= strip_left && tab.right() <= new_tab.left(),
            "The selected tab {title} {tab:?} must be inside the strip, after {strip_left:?} and before {new_tab:?}"
        );
    });
}

#[gpui_kit::test]
fn the_selected_tab_stays_in_view_when_the_assistant_opens(cx: &mut TestAppContext) {
    let (directory, codex) = crate::support::assistant::FakeCodex::new();
    let profile = offline_profile("Two tabs");
    let workspace = codex.workspace(Workspace {
        tabs: vec![
            SavedTab {
                title: "Route overview".into(),
                ..SavedTab::new(1, Some(profile.id))
            },
            SavedTab::new(1, Some(profile.id)),
        ],
        active_tab: 1,
        profiles: vec![profile],
        ..Workspace::default()
    });
    let app = TestApp::launch_in(
        cx,
        directory,
        workspace,
        crate::support::MemoryCredentials::default(),
    );
    cx.simulate_window_resize(app.window, gpui_kit::size(px(900.), px(770.)));
    app.settle(cx);
    selected_tab_in_strip(&app, cx, "Query 1");
    for _ in 0..2 {
        app.dispatch(cx, qrow::ui::ToggleAssistant);
        app.settle(cx);
        app.settle(cx);
        selected_tab_in_strip(&app, cx, "Query 1");
        // Two tabs fit without a scroll, so the other tab shows too.
        app.update(cx, |window, _| {
            let other = bounds_of(window, "Route overview");
            assert!(
                other.left() >= bounds_of(window, "add-connection").right(),
                "Route overview {other:?} must not scroll away"
            );
        });
    }
}

#[gpui_kit::test]
fn a_renamed_selected_tab_stays_in_view(cx: &mut TestAppContext) {
    let profile = offline_profile("Long title");
    let id = profile.id;
    let tabs: Vec<_> = (1..=7)
        .map(|number| SavedTab::new(number, Some(id)))
        .collect();
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs,
            active_tab: 6,
            profiles: vec![profile],
            ..Workspace::default()
        },
    );
    app.settle(cx);
    selected_tab_in_strip(&app, cx, "Query 7");
    let title = "Daily bookings by route and gate for the last quarter";
    rename(&app, cx, "Query 7");
    app.fill(cx, "rename-tab-name", title);
    app.click(cx, "rename-tab");
    app.wait_gone(cx, "rename-tab-name");
    app.settle(cx);
    app.settle(cx);
    selected_tab_in_strip(&app, cx, title);
}

#[gpui_kit::test]
fn the_tab_strip_snaps_to_a_tab_after_a_resize(cx: &mut TestAppContext) {
    let profile = offline_profile("Snap");
    let tabs: Vec<_> = (1..=10)
        .map(|number| SavedTab::new(number, Some(profile.id)))
        .collect();
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs,
            profiles: vec![profile],
            ..Workspace::default()
        },
    );
    cx.simulate_window_resize(app.window, gpui_kit::size(px(900.), px(770.)));
    app.settle(cx);
    let (strip_left, position) = app.update(cx, |window, _| {
        let first = bounds_of(window, "Query 1");
        (first.left(), bounds_of(window, "Query 2").center())
    });
    // A wheel stops between two tab starts, and the strip snaps.
    app.update(cx, |window, cx| {
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position,
                delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(-200.), px(0.))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    });
    cx.executor().advance_clock(Duration::from_millis(300));
    app.settle(cx);
    app.settle(cx);
    app.update(cx, |window, _| {
        assert!(
            bounds_of(window, "Query 1").left() < strip_left,
            "The strip must stay scrolled"
        );
        assert_no_tab_cut_at(window, strip_left, 10);
    });
    // At the end of the range, the last tab meets the end of the strip, with
    // no room after it.
    app.update(cx, |window, cx| {
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position,
                delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(-5000.), px(0.))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    });
    cx.executor().advance_clock(Duration::from_millis(300));
    app.settle(cx);
    app.settle(cx);
    app.update(cx, |window, _| {
        assert_cut_tabs_hide_close(window, strip_left, 10);
        let last = bounds_of(window, "Query 10");
        let new_tab = bounds_of(window, "new-tab");
        assert!(
            last.right() <= new_tab.left() && new_tab.left() - last.right() < px(24.),
            "The last tab {last:?} must end at the strip end, before {new_tab:?}"
        );
    });
    // A tab that the left edge cuts at the end of the range has its title
    // past the edge, so the strip pins the title to the edge when two
    // characters and an ellipsis fit. Some widths cut a narrow part, some a
    // wide part.
    let (mut narrow, mut wide) = (0, 0);
    for width in (900..=1200).step_by(10).map(|width| width as f32) {
        cx.simulate_window_resize(app.window, gpui_kit::size(px(width), px(770.)));
        app.settle(cx);
        app.update(cx, |window, cx| {
            window.dispatch_event(
                gpui_kit::ScrollWheelEvent {
                    position,
                    delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(-5000.), px(0.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        });
        cx.executor().advance_clock(Duration::from_millis(300));
        app.settle(cx);
        app.settle(cx);
        app.update(cx, |window, _| {
            let cut = (1..=10)
                .filter_map(|number| labelled(window, &format!("Query {number}")))
                .map(|tab| tab.bounds())
                .find(|tab| tab.left() < strip_left - px(0.5) && tab.right() > strip_left + px(0.5));
            // Two characters, an ellipsis, and the tab padding fit in 45 pt
            // of the test text system.
            let Some(cut) = cut.filter(|cut| cut.right() - strip_left >= px(45.)) else {
                return;
            };
            let titles = gpui_kit::component::tab::pinned_tab_titles();
            assert!(
                titles.iter().any(|(text, bounds)| {
                    text.starts_with('Q') && bounds.left() >= strip_left && bounds.right() <= cut.right()
                }),
                "At {width} pt, the cut tab {cut:?} must show its title inside the strip: {titles:?}"
            );
            if cut.right() - strip_left < px(60.) {
                narrow += 1;
            } else {
                wide += 1;
            }
        });
    }
    assert!(
        narrow > 0 && wide > 0,
        "The widths must cut 45 to 60 pt and over 60 pt of a tab: {narrow} and {wide} cases"
    );
    // A wider window snaps again, and the selected tab stays in view.
    for width in [1280., 1000., 900.] {
        cx.simulate_window_resize(app.window, gpui_kit::size(px(width), px(770.)));
        app.settle(cx);
        app.settle(cx);
        app.update(cx, |window, _| {
            assert_no_tab_cut_at(window, strip_left, 10);
            assert_cut_tabs_hide_close(window, strip_left, 10);
        });
        selected_tab_in_strip(&app, cx, "Query 1");
    }
}

#[gpui_kit::test]
fn each_new_tab_shows_in_full(cx: &mut TestAppContext) {
    let profile = offline_profile("New tabs");
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![SavedTab::new(1, Some(profile.id))],
            profiles: vec![profile],
            ..Workspace::default()
        },
    );
    app.settle(cx);
    for number in 2..=10 {
        app.dispatch(cx, qrow::ui::NewTab);
        app.settle(cx);
        app.settle(cx);
        selected_tab_in_strip(&app, cx, &format!("Query {number}"));
    }
}

/// The title in the tooltip that shows after the pointer rests on the tab
/// labelled `label`, or `None`.
fn tab_tooltip(app: &TestApp, cx: &mut TestAppContext, label: &str) -> Option<String> {
    use gpui_kit::MouseMoveEvent;
    let away = app.update(cx, |window, _| bounds_of(window, "sql-editor").center());
    for (target, wait) in [(None, true), (Some(label), false)] {
        app.update(cx, |window, cx| {
            let position = target.map_or(away, |label| {
                // The status dot has the label of its tab too; the tab is wider.
                let tab = crate::support::elements(window)
                    .into_iter()
                    .filter(|element| element.label() == Some(label))
                    .map(|element| element.bounds())
                    .max_by(|a, b| a.size.width.partial_cmp(&b.size.width).unwrap())
                    .unwrap_or_else(|| panic!("No tab {label}"));
                gpui_kit::point(tab.left() + px(40.), tab.center().y)
            });
            window.dispatch_event(
                MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers: Default::default(),
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        });
        cx.executor().advance_clock(Duration::from_millis(800));
        app.settle(cx);
        if wait {
            app.wait_gone(cx, "tooltip");
        }
    }
    app.update(cx, |window, _| {
        window
            .try_find("tooltip")
            .and_then(|_| crate::support::label(window, "status-tooltip-title"))
    })
}

#[gpui_kit::test]
fn every_tab_shows_its_full_title_on_hover(cx: &mut TestAppContext) {
    const LONG: &str = "Ежедневная стоимость моделей Ривенделла по кластерам";
    let profile = offline_profile("Tab tooltips");
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![
                tab(profile.id, LONG, "SELECT 1"),
                SavedTab {
                    id: Uuid::new_v4(),
                    ..SavedTab::new(2, Some(profile.id))
                },
            ],
            active_tab: 0,
            profiles: vec![profile],
            ..Workspace::default()
        },
    );
    app.settle(cx);
    // The selected tab, and a tab that is not selected.
    assert_eq!(tab_tooltip(&app, cx, LONG).as_deref(), Some(LONG));
    assert_eq!(tab_tooltip(&app, cx, "Query 2").as_deref(), Some("Query 2"));
}

#[gpui_kit::test]
fn the_selected_tab_with_a_status_dot_shows_its_full_title_on_hover(cx: &mut TestAppContext) {
    use qrow::storage::Credentials as _;

    const LONG: &str = "Ежедневная стоимость моделей Ривенделла по кластерам";
    // A server that accepts the connection but never answers keeps the
    // selected tab connecting, so the tab shows a status dot.
    let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let profile = qrow::model::Profile {
        name: "Silent".into(),
        host: "127.0.0.1".into(),
        port: server.local_addr().unwrap().port(),
        username: "synthetic-user".into(),
        database: "default".into(),
        ..qrow::model::Profile::default()
    };
    let credentials = crate::support::MemoryCredentials::default();
    credentials
        .set_password(profile.id, "synthetic-password")
        .unwrap();
    let app = TestApp::launch_with(
        cx,
        Workspace {
            tabs: vec![
                tab(profile.id, LONG, "SELECT 1"),
                SavedTab {
                    id: Uuid::new_v4(),
                    ..SavedTab::new(2, Some(profile.id))
                },
            ],
            active_tab: 0,
            profiles: vec![profile],
            ..Workspace::default()
        },
        credentials,
    );
    app.update(cx, |window, cx| window.click("run", cx));
    app.wait_until(
        cx,
        "the status dot",
        Duration::from_secs(20),
        |window, _| {
            crate::support::elements(window)
                .into_iter()
                .any(|element| format!("{:?}", element.path().last()).contains("query-status-"))
        },
    );
    let label = app.update(cx, |window, _| {
        crate::support::labelled_starting(window, LONG)
            .into_iter()
            .filter_map(|element| element.label().map(str::to_owned))
            .find(|label| label != LONG && !label.starts_with("Close"))
            .expect("the label of the selected tab with its status")
    });
    assert_eq!(tab_tooltip(&app, cx, &label).as_deref(), Some(LONG));
    assert_eq!(tab_tooltip(&app, cx, "Query 2").as_deref(), Some("Query 2"));
    drop(server);
}

#[gpui_kit::test]
fn the_selected_tab_shows_its_whole_title(cx: &mut TestAppContext) {
    const LONG: &str = "Ежедневная стоимость моделей Ривенделла";
    let profile = offline_profile("Wide tabs");
    let app = TestApp::launch(
        cx,
        Workspace {
            tabs: vec![
                tab(profile.id, LONG, "SELECT 1"),
                SavedTab {
                    id: Uuid::new_v4(),
                    ..SavedTab::new(2, Some(profile.id))
                },
            ],
            active_tab: 0,
            profiles: vec![profile],
            ..Workspace::default()
        },
    );
    app.settle(cx);
    app.settle(cx);
    let tab_width = |app: &TestApp, cx: &mut TestAppContext| {
        app.update(cx, |window, _| bounds_of(window, LONG).size.width)
    };
    // Selected, the tab is wider than the cap of the other tabs.
    let selected = tab_width(&app, cx);
    assert!(selected > px(241.), "The selected tab is {selected:?} wide");
    selected_tab_in_strip(&app, cx, LONG);
    // Not selected, it shrinks back to the cap.
    app.click_labelled(cx, "Query 2");
    app.settle(cx);
    app.settle(cx);
    let unselected = tab_width(&app, cx);
    assert!(unselected <= px(241.), "The tab is {unselected:?} wide");
    // Selected again, it grows again.
    app.click_labelled(cx, LONG);
    app.settle(cx);
    app.settle(cx);
    assert_eq!(tab_width(&app, cx), selected);
}
