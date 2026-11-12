use crate::support::{TestApp, bounds_of, label, value};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{TestAppContext, px};
use qrow::{
    build_info,
    model::{CopyFormat, Workspace},
    ui::{OpenAbout, OpenSettings},
};
use std::time::Duration;

/// A version line reads `0.1.0`, `0.1.0 (dcc75d4fd874)`, or, for a release
/// build, `0.1.0-nightly.20260921.7 (dcc75d4fd874)`.
fn is_version(text: &str) -> bool {
    let (version, commit) = text.split_once(' ').unwrap_or((text, ""));
    let (number, channel) = version.split_once('-').unwrap_or((version, ""));
    let parts: Vec<_> = number.split('.').collect();
    let numeric = parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    // A SemVer pre-release: dot-separated, nonempty ASCII alphanumeric identifiers.
    let channel = version == number
        || channel.split('.').all(|identifier| {
            !identifier.is_empty()
                && identifier
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    let commit = commit.trim_matches(|c| c == '(' || c == ')');
    numeric
        && channel
        && (commit.is_empty()
            || (commit.len() == 12
                && commit
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())))
}

#[test]
fn version_lines_include_release_channels() {
    for line in [
        "0.4.0",
        "0.4.0 (6ca918597182)",
        "0.4.0-nightly.20261107.39 (6ca918597182)",
        "1.0.0-rc.1",
    ] {
        assert!(is_version(line), "{line}");
    }
    for line in [
        "0.4",
        "0.4.0-",
        "0.4.0-nightly..39",
        "0.4.0-night_ly",
        "0.4.0 (6CA918597182)",
        "0.4.0 (6ca9185)",
    ] {
        assert!(!is_version(line), "{line}");
    }
}

#[gpui_kit::test]
fn about_shows_the_version_and_closes_with_escape(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    for _ in 0..2 {
        app.dispatch(cx, OpenAbout);
        app.wait_for(cx, "about-copyright");
        app.update(cx, |window, _| {
            assert_eq!(
                label(window, "about-copyright").as_deref(),
                Some(build_info::COPYRIGHT)
            );
            let version = label(window, "about-version").unwrap_or_default();
            assert!(is_version(&version), "Not a version line: {version}");
            assert_eq!(version, build_info::version_label());
        });
        app.press(cx, "escape");
        app.wait_gone(cx, "about-copyright");
    }
}

/// The saved setting, after Qrow applied it.
fn wait_saved(
    app: &TestApp,
    cx: &mut TestAppContext,
    what: &str,
    check: impl Fn(&Workspace) -> bool,
) {
    app.wait_until(cx, what, Duration::from_secs(10), |_, _| {
        check(&app.saved())
    });
}

fn left_edge(app: &TestApp, cx: &mut TestAppContext, id: &str) -> f32 {
    app.update(cx, |window, _| f32::from(bounds_of(window, id).origin.x))
}

#[gpui_kit::test]
fn export_replay_limit_is_saved_and_zero_disables_retention(cx: &mut TestAppContext) {
    let field = "setting-export-replay-limit";
    let app = TestApp::launch(cx, Workspace::default());
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "replay");
    app.wait_for(cx, field);
    app.update(cx, |window, _| {
        assert_eq!(value(window, field).as_deref(), Some("2048"));
        // The unit follows the value, as in the connection export fields.
        let input = window.find(field);
        let unit = window.find("setting-export-replay-limit-unit");
        assert!(unit.visible());
        assert!(unit.bounds().left() > input.bounds().left());
        assert!(unit.bounds().left() - input.bounds().left() < window.rem_size() * 4.);
    });
    app.fill(cx, field, "0");
    // Settings apply as they change; ⌘W closes the dialog.
    app.press(cx, "secondary-w");
    app.wait_gone(cx, "reset-settings");
    wait_saved(&app, cx, "disabled replay retention", |workspace| {
        workspace.settings.export_replay_limit_mib == 0
    });
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "replay");
    app.wait_for(cx, field);
    app.update(cx, |window, _| {
        assert_eq!(value(window, field).as_deref(), Some("0"));
    });
    // Enter commits the value and clamps it to the maximum.
    app.fill(cx, field, "2000000");
    app.press(cx, "enter");
    app.update(cx, |window, _| {
        assert_eq!(value(window, field).as_deref(), Some("1048576"));
    });
    app.fill(cx, field, "300");
    app.press(cx, "secondary-w");
    wait_saved(&app, cx, "replay retention", |workspace| {
        workspace.settings.export_replay_limit_mib == 300
    });
}

#[gpui_kit::test]
fn settings_restore_defaults_sits_at_the_right_like_the_dialog_actions(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    app.dispatch(cx, OpenSettings);
    app.wait_for(cx, "reset-settings");
    app.update(cx, |window, _| {
        let button = bounds_of(window, "reset-settings");
        let width = window.viewport_size().width;
        assert!(
            width - button.right() < button.left(),
            "Restore defaults must sit at the right: {button:?} in {width:?}"
        );
    });
}

#[gpui_kit::test]
fn copy_format_is_saved(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "clipboard");
    app.wait_for(cx, "setting-copy-format");
    app.select(cx, "setting-copy-format", "JSON");
    // Settings apply as they change; ⌘W closes the dialog.
    app.press(cx, "secondary-w");
    app.wait_gone(cx, "reset-settings");
    wait_saved(&app, cx, "the copy format", |workspace| {
        workspace.settings.copy_format == CopyFormat::Json
    });
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "clipboard");
    app.wait_for(cx, "setting-copy-format");
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "setting-copy-format").as_deref(),
            Some("JSON")
        );
    });
}

#[gpui_kit::test]
fn settings_change_sizes_fonts_and_keyword_case_and_restore_defaults(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    app.dispatch(cx, OpenSettings);
    app.wait_for(cx, "reset-settings");
    app.update(cx, |window, _| {
        assert_eq!(value(window, "setting-theme").as_deref(), Some("System"));
    });
    assert_eq!(app.stepper(cx, "setting-ui-scale"), "100");

    // At 110%, the description of the interface font is wider than its
    // column. It wraps, and the picker keeps the left edge of the page.
    app.step(cx, "setting-ui-scale", "increment", "110");
    let theme = left_edge(&app, cx, "setting-theme");
    let font = left_edge(&app, cx, "setting-ui-font-family");
    assert!(
        (font - theme).abs() <= 1.,
        "The UI font picker starts at {font}, not {theme}"
    );
    app.step(cx, "setting-ui-scale", "decrement", "100");

    app.select(cx, "setting-ui-font-family", "Menlo");
    wait_saved(&app, cx, "the UI font", |w| {
        w.settings.ui_font_family == "Menlo"
    });
    app.select(cx, "setting-ui-font-family", "System Font");

    // The search brings the assistant typography to the top of the page, where
    // the dialog footer does not cover it.
    app.fill_labelled(cx, "Search...", "messages");
    app.wait_for(cx, "setting-assistant-font-size");
    assert_eq!(app.stepper(cx, "setting-assistant-font-size"), "14");
    app.step(cx, "setting-assistant-font-size", "increment", "15");
    app.select(cx, "setting-assistant-font-family", "Menlo");
    wait_saved(&app, cx, "the assistant font", |w| {
        w.settings.assistant_font_family == "Menlo"
    });
    app.select(cx, "setting-assistant-font-family", "System Font");
    wait_saved(&app, cx, "the system assistant font", |w| {
        w.settings.assistant_font_family == ".SystemUIFont"
    });

    app.fill_labelled(cx, "Search...", "editor");
    app.wait_for(cx, "setting-editor-font-size");
    assert_eq!(app.stepper(cx, "setting-editor-font-size"), "13");
    app.step(cx, "setting-editor-font-size", "increment", "14");
    app.select(cx, "setting-editor-font-family", "System Font");
    // SQL Keyword Case is an Editor setting with a long description.
    let family = left_edge(&app, cx, "setting-editor-font-family");
    let case = left_edge(&app, cx, "setting-sql-keyword-case");
    assert!(
        (case - family).abs() <= 1.,
        "SQL Keyword Case starts at {case}, not {family}"
    );
    // The search shows the setting at the top of the page, where the pointer
    // can reach it.
    app.fill_labelled(cx, "Search...", "keyword");
    app.wait_for(cx, "setting-sql-keyword-case");
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "setting-sql-keyword-case").as_deref(),
            Some("Uppercase")
        );
    });
    app.select(cx, "setting-sql-keyword-case", "Lowercase");

    app.fill_labelled(cx, "Search...", "logs");
    app.wait_for(cx, "setting-logs-font-size");
    assert_eq!(app.stepper(cx, "setting-logs-font-size"), "13");
    app.select(cx, "setting-logs-font-family", "System Font");

    app.fill_labelled(cx, "Search...", "");
    app.click(cx, "reset-settings");
    app.fill_labelled(cx, "Search...", "messages");
    app.wait_until(
        cx,
        "the default sizes",
        Duration::from_secs(10),
        |window, _| {
            window
                .within("setting-assistant-font-size")
                .try_find("value")
                .and_then(|v| v.value().map(str::to_owned))
                .as_deref()
                == Some("14")
        },
    );
    app.fill_labelled(cx, "Search...", "editor");
    app.wait_for(cx, "setting-editor-font-size");
    assert_eq!(app.stepper(cx, "setting-editor-font-size"), "13");
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "setting-sql-keyword-case").as_deref(),
            Some("Uppercase")
        );
    });
    // Settings apply as they change; ⌘W closes the dialog.
    app.press(cx, "secondary-w");
    app.wait_gone(cx, "reset-settings");
    wait_saved(&app, cx, "the restored default fonts", |w| {
        w.settings.editor_font_size == 13.
            && w.settings.assistant_font_size == 14.
            && w.settings.editor_font_family == "JetBrains Mono"
            && w.settings.logs_font_family == "JetBrains Mono"
            && w.settings.ui_font_family == "IBM Plex Sans"
            && w.settings.assistant_font_family == "IBM Plex Sans"
    });
}

#[gpui_kit::test]
fn smooth_caret_is_opt_in_saved_and_resettable(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    assert!(!app.saved().settings.editor_smooth_caret);
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "smooth");
    app.wait_for(cx, "editor-smooth-caret");
    app.click(cx, "editor-smooth-caret");
    wait_saved(&app, cx, "smooth caret enabled", |workspace| {
        workspace.settings.editor_smooth_caret
    });
    app.press(cx, "secondary-w");
    app.wait_gone(cx, "reset-settings");
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "smooth");
    app.wait_for(cx, "editor-smooth-caret");
    app.click(cx, "editor-smooth-caret");
    wait_saved(&app, cx, "smooth caret disabled", |workspace| {
        !workspace.settings.editor_smooth_caret
    });
    app.click(cx, "editor-smooth-caret");
    wait_saved(&app, cx, "smooth caret enabled again", |workspace| {
        workspace.settings.editor_smooth_caret
    });
    app.click(cx, "reset-settings");
    wait_saved(&app, cx, "smooth caret default", |workspace| {
        !workspace.settings.editor_smooth_caret
    });
}

#[gpui_kit::test]
fn scrolling_and_panel_motion_are_independent_opt_ins_and_resettable(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    assert!(!app.saved().settings.editor_smooth_scrolling);
    assert!(!app.saved().settings.smooth_transitions);
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "smooth");
    app.wait_for(cx, "smooth-transitions");
    app.click(cx, "smooth-transitions");
    wait_saved(&app, cx, "panel transitions enabled", |workspace| {
        workspace.settings.smooth_transitions && !workspace.settings.editor_smooth_scrolling
    });
    app.click(cx, "editor-smooth-scrolling");
    wait_saved(&app, cx, "wheel scrolling enabled", |workspace| {
        workspace.settings.smooth_transitions
            && workspace.settings.editor_smooth_scrolling
            && !workspace.settings.editor_smooth_caret
    });
    app.press(cx, "secondary-w");
    app.wait_gone(cx, "reset-settings");
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "smooth");
    app.wait_for(cx, "smooth-transitions");
    app.click(cx, "editor-smooth-scrolling");
    wait_saved(&app, cx, "wheel scrolling disabled", |workspace| {
        workspace.settings.smooth_transitions && !workspace.settings.editor_smooth_scrolling
    });
    app.click(cx, "reset-settings");
    wait_saved(&app, cx, "motion defaults", |workspace| {
        !workspace.settings.smooth_transitions && !workspace.settings.editor_smooth_scrolling
    });
}

#[gpui_kit::test]
fn composer_motion_options_are_independent_saved_and_resettable(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    assert!(!app.saved().settings.assistant_smooth_caret);
    assert!(!app.saved().settings.assistant_smooth_resize);
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "composer");
    app.wait_for(cx, "assistant-smooth-caret");
    app.click(cx, "assistant-smooth-caret");
    wait_saved(&app, cx, "composer caret enabled", |workspace| {
        workspace.settings.assistant_smooth_caret && !workspace.settings.assistant_smooth_resize
    });
    app.click(cx, "assistant-smooth-resize");
    wait_saved(&app, cx, "composer resize enabled", |workspace| {
        workspace.settings.assistant_smooth_caret
            && workspace.settings.assistant_smooth_resize
            && !workspace.settings.editor_smooth_caret
    });
    app.press(cx, "secondary-w");
    app.wait_gone(cx, "reset-settings");
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "composer");
    app.wait_for(cx, "assistant-smooth-resize");
    app.click(cx, "assistant-smooth-caret");
    wait_saved(&app, cx, "composer caret disabled", |workspace| {
        !workspace.settings.assistant_smooth_caret && workspace.settings.assistant_smooth_resize
    });
    app.click(cx, "reset-settings");
    wait_saved(&app, cx, "composer defaults", |workspace| {
        !workspace.settings.assistant_smooth_caret && !workspace.settings.assistant_smooth_resize
    });
}

#[gpui_kit::test]
fn buffered_replies_are_opt_in_saved_and_resettable(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    assert!(!app.saved().settings.assistant_buffer_replies);
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "buffered");
    app.wait_for(cx, "assistant-buffer-replies");
    app.click(cx, "assistant-buffer-replies");
    wait_saved(&app, cx, "buffered replies enabled", |workspace| {
        workspace.settings.assistant_buffer_replies
            && !workspace.settings.smooth_transitions
            && !workspace.settings.assistant_smooth_resize
    });
    app.press(cx, "secondary-w");
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "buffered");
    app.wait_for(cx, "assistant-buffer-replies");
    app.click(cx, "reset-settings");
    wait_saved(&app, cx, "buffered replies reset", |workspace| {
        !workspace.settings.assistant_buffer_replies
    });
}

#[gpui_kit::test]
fn settings_controls_end_at_one_right_edge(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    app.dispatch(cx, OpenSettings);
    app.wait_for(cx, "smooth-transitions");
    app.update(cx, |window, _| {
        let select = bounds_of(window, "setting-theme");
        let switch = bounds_of(window, "smooth-transitions");
        assert!(
            (select.right() - switch.right()).abs() < px(1.),
            "the switch ends at {:?}, the select at {:?}",
            switch.right(),
            select.right()
        );
        assert!(switch.size.width < select.size.width / 2.);
    });
}

#[gpui_kit::test]
fn setting_descriptions_use_a_compact_line_height(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    app.dispatch(cx, OpenSettings);
    app.wait_for(cx, "smooth-transitions");
    app.update(cx, |window, _| {
        // 0.875 rem text on 1.25 rem lines, not the 1.618 default.
        let line = window.rem_size() * 1.25;
        for title in ["Theme", "Smooth Transitions", "Scale"] {
            let height = bounds_of(window, &format!("setting-help-{title}"))
                .size
                .height;
            let lines = (height / line).round();
            assert!(lines >= 1., "{title}: {height:?}");
            assert!(
                (height - line * lines).abs() < px(1.),
                "{title}: {height:?} is not {lines} lines of {line:?}"
            );
        }
    });
}
