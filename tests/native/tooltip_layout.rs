//! Run on the process main thread: AppKit owns the native text backend.
#[cfg(target_os = "macos")]
#[path = "../support/mod.rs"]
mod support;

#[cfg(target_os = "macos")]
fn main() {
    use gpui_kit::{TestAppContext, TestDispatcher};
    use qrow::model::{SavedTab, Workspace};
    use std::time::Duration;
    use support::{
        MemoryCredentials, TestApp, assert_tooltip_header_center, assert_tooltip_metadata_rows,
        assistant::FakeCodex, bounds_of, label, offline_profile,
    };

    let text_system = gpui_kit::platform::current_platform(true).text_system();
    assert!(
        text_system
            .all_font_names()
            .iter()
            .any(|name| name == "Menlo")
    );
    for font in [".SystemUIFont", "Menlo"] {
        for ui_scale in [0.75, 1., 1.5] {
            for display_scale in [1., 2.] {
                println!("tooltip-layout: {font} UI {ui_scale} display {display_scale}");
                let mut cx = TestAppContext::build_with_text_system(
                    TestDispatcher::new(0),
                    None,
                    text_system.clone(),
                );
                let name = "W".repeat(60);
                let profile = offline_profile(&name);
                let (directory, codex) = FakeCodex::new();
                let mut workspace = Workspace {
                    tabs: vec![SavedTab::new(1, Some(profile.id))],
                    profiles: vec![profile],
                    ..Workspace::default()
                };
                workspace.settings.ui_font_family = font.into();
                workspace.settings.ui_scale = ui_scale;
                workspace.settings.assistant = codex.settings().assistant;
                let app =
                    TestApp::launch_in(&mut cx, directory, workspace, MemoryCredentials::default());
                cx.simulate_window_scale_factor_change(app.window, display_scale);
                app.settle(&mut cx);
                for (target, title, secondary) in [
                    (name.as_str(), name.as_str(), "status-tooltip-status"),
                    ("New Tab", "New Tab", "status-tooltip-shortcut"),
                    ("Run", "Run Query", "status-tooltip-shortcut"),
                    (
                        "Toggle Sidebar",
                        "Toggle Sidebar",
                        "status-tooltip-shortcut",
                    ),
                    ("Close Query 1", "Close Tab", "status-tooltip-shortcut"),
                    ("Activity", "Activity", "status-tooltip-shortcut"),
                    ("Toggle Assistant", "Assistant", "status-tooltip-shortcut"),
                ] {
                    app.hover_labelled(&mut cx, target);
                    cx.executor().advance_clock(Duration::from_millis(800));
                    app.settle(&mut cx);
                    app.update(&mut cx, |window, cx| {
                        assert_eq!(window.scale_factor(), display_scale);
                        assert_eq!(
                            label(window, "status-tooltip-title").as_deref(),
                            Some(title)
                        );
                        assert_tooltip_header_center(window, cx, secondary);
                        if title == name {
                            assert_tooltip_metadata_rows(window);
                        }
                        let popup = bounds_of(window, "status-tooltip");
                        for id in ["status-tooltip-title", secondary] {
                            let text = bounds_of(window, id);
                            assert!(text.right() <= popup.right(), "{id} overflows the tooltip");
                            assert!(
                                text.bottom() <= popup.bottom(),
                                "{id} overflows the tooltip"
                            );
                        }
                    });
                }
            }
        }
    }
    println!("tooltip-layout: 84 native font and display scale checks passed");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
