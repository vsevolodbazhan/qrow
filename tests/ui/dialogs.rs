use crate::support::{TestApp, bounds_of, label, value};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;
use qrow::{
    build_info,
    model::Workspace,
    ui::{OpenAbout, OpenSettings},
};
use std::time::Duration;

/// A version line reads `0.1.0` or `0.1.0 (dcc75d4fd874)`.
fn is_version(text: &str) -> bool {
    let (number, commit) = text.split_once(' ').unwrap_or((text, ""));
    let parts: Vec<_> = number.split('.').collect();
    let numeric = parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    let commit = commit.trim_matches(|c| c == '(' || c == ')');
    numeric
        && (commit.is_empty()
            || (commit.len() == 12
                && commit
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())))
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
    let app = TestApp::launch(cx, Workspace::default());
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "replay");
    app.wait_for(cx, "setting-export-replay-limit");
    assert_eq!(app.stepper(cx, "setting-export-replay-limit"), "2048");
    for expected in ["1792", "1536", "1280", "1024", "768", "512", "256", "0"] {
        app.step(cx, "setting-export-replay-limit", "decrement", expected);
    }
    app.click(cx, "save-settings");
    app.wait_gone(cx, "save-settings");
    wait_saved(&app, cx, "disabled replay retention", |workspace| {
        workspace.settings.export_replay_limit_mib == 0
    });
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "replay");
    app.wait_for(cx, "setting-export-replay-limit");
    assert_eq!(app.stepper(cx, "setting-export-replay-limit"), "0");
    app.step(cx, "setting-export-replay-limit", "increment", "256");
    app.click(cx, "save-settings");
    wait_saved(&app, cx, "replay retention", |workspace| {
        workspace.settings.export_replay_limit_mib == 256
    });
}

#[gpui_kit::test]
fn settings_change_sizes_fonts_and_keyword_case_and_restore_defaults(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, Workspace::default());
    app.dispatch(cx, OpenSettings);
    app.wait_for(cx, "save-settings");
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
    app.click(cx, "save-settings");
    app.wait_gone(cx, "save-settings");
    let settings = app.saved().settings;
    assert_eq!(settings.editor_font_size, 13.);
    assert_eq!(settings.assistant_font_size, 14.);
    assert_eq!(settings.editor_font_family, "Menlo");
}
