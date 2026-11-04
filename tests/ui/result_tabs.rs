//! Result tabs: runs replace the unpinned result, and pinned results stay.
use crate::support::{TestApp, label, labelled};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ClipboardItem, TestAppContext};

/// The labels of the result tabs, in order.
fn result_tabs(app: &TestApp, cx: &mut TestAppContext) -> Vec<String> {
    app.update(cx, |window, _| {
        (1..=20)
            .filter_map(|id| label(window, format!("result-tab-{id}")))
            .collect()
    })
}

/// The ID of the result tab that Results shows.
fn shown(app: &TestApp, cx: &mut TestAppContext) -> Option<u64> {
    app.update(cx, |window, _| {
        (1..=20).find(|id| {
            window
                .try_find(format!("result-tab-{id}"))
                .is_some_and(|tab| {
                    tab.path()
                        .iter()
                        .any(|part| part.to_string() == "results-panel-tab")
                })
        })
    })
}

/// Replaces the demo SQL and leaves the cursor at its end.
fn write_sql(app: &TestApp, cx: &mut TestAppContext, sql: &str) {
    app.update(cx, |window, cx| {
        window.click("sql-editor", cx);
        window.press("cmd-a", cx);
        window.input(sql, cx);
    });
    app.settle(cx);
}

fn run(app: &TestApp, cx: &mut TestAppContext) {
    app.click(cx, "run");
    app.settle(cx);
}

fn pin_label(app: &TestApp, cx: &mut TestAppContext) -> String {
    app.update(cx, |window, _| {
        label(window, "pin-result").unwrap_or_default()
    })
}

fn copied(cx: &mut TestAppContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|item| item.text())
}

#[gpui_kit::test]
fn a_run_replaces_the_unpinned_result_and_keeps_pinned_ones(cx: &mut TestAppContext) {
    let app = TestApp::launch_demo(cx);
    app.settle(cx);
    assert_eq!(result_tabs(&app, cx), ["Result 1"]);
    assert_eq!(shown(&app, cx), Some(1));

    // An unpinned result is replaced in place.
    run(&app, cx);
    assert_eq!(result_tabs(&app, cx), ["Result 1"]);

    app.click(cx, "pin-result");
    app.settle(cx);
    app.update(cx, |window, _| {
        assert!(labelled(window, "Result 1, pinned").is_some());
        assert_eq!(label(window, "pin-result").as_deref(), Some("Unpin result"));
    });
    run(&app, cx);
    assert_eq!(result_tabs(&app, cx), ["Result 1", "Result 2"]);
    assert_eq!(shown(&app, cx), Some(2));
    run(&app, cx);
    assert_eq!(result_tabs(&app, cx), ["Result 1", "Result 2"]);

    // A run from a pinned result replaces the unpinned one, not the shown one.
    app.click(cx, "result-tab-1");
    app.settle(cx);
    assert_eq!(shown(&app, cx), Some(1));
    run(&app, cx);
    assert_eq!(result_tabs(&app, cx), ["Result 1", "Result 2"]);
    assert_eq!(shown(&app, cx), Some(2));

    // Unpinning lets the next run replace the shown result.
    app.click(cx, "result-tab-1");
    app.click(cx, "pin-result");
    app.settle(cx);
    run(&app, cx);
    assert_eq!(result_tabs(&app, cx), ["Result 1", "Result 2"]);
    assert_eq!(shown(&app, cx), Some(1));

    // Logs stays a tab beside the results.
    app.click(cx, "output-panel-tab");
    app.wait_for(cx, "output-copy-all");
    app.click(cx, "result-tab-2");
    app.wait_gone(cx, "output-copy-all");
    assert_eq!(shown(&app, cx), Some(2));
}

#[gpui_kit::test]
fn copy_query_copies_the_statement_that_made_the_result(cx: &mut TestAppContext) {
    let app = TestApp::launch_demo(cx);
    write_sql(&app, cx, "SELECT 1;\nSELECT 2");
    run(&app, cx);
    app.click(cx, "copy-result-query");
    assert_eq!(copied(cx).as_deref(), Some("SELECT 2"));

    // The copied query does not follow later edits of the editor.
    app.click(cx, "pin-result");
    write_sql(&app, cx, "SELECT 3");
    run(&app, cx);
    app.click(cx, "result-tab-1");
    app.settle(cx);
    app.click(cx, "copy-result-query");
    assert_eq!(copied(cx).as_deref(), Some("SELECT 2"));

    app.context_menu_labelled(cx, "Result 2");
    app.choose(cx, "popup-menu", "Copy query");
    app.settle(cx);
    assert_eq!(copied(cx).as_deref(), Some("SELECT 3"));
}

#[gpui_kit::test]
fn a_rename_pins_the_result_and_rejects_taken_names(cx: &mut TestAppContext) {
    let app = TestApp::launch_demo(cx);
    app.settle(cx);
    app.context_menu_labelled(cx, "Result 1");
    app.choose(cx, "popup-menu", "Rename…");
    app.wait_for(cx, "rename-result-name");
    app.fill(cx, "rename-result-name", "Routes");
    app.click(cx, "rename-result");
    app.wait_gone(cx, "rename-result-name");
    app.update(cx, |window, _| {
        assert!(labelled(window, "Routes, pinned").is_some());
    });

    // The pinned, renamed result stays when the next run opens a new one.
    run(&app, cx);
    assert_eq!(result_tabs(&app, cx), ["Routes", "Result 2"]);

    app.context_menu_labelled(cx, "Result 2");
    app.choose(cx, "popup-menu", "Rename…");
    app.wait_for(cx, "rename-result-name");
    app.fill(cx, "rename-result-name", "Routes");
    app.click(cx, "rename-result");
    app.settle(cx);
    app.wait_for(cx, "rename-result-name");
    app.click(cx, "cancel-result");
    app.wait_gone(cx, "rename-result-name");
    assert_eq!(result_tabs(&app, cx), ["Routes", "Result 2"]);

    // A new result tab skips a name that a renamed result tab took.
    app.context_menu_labelled(cx, "Result 2");
    app.choose(cx, "popup-menu", "Rename…");
    app.wait_for(cx, "rename-result-name");
    app.fill(cx, "rename-result-name", "Result 3");
    app.click(cx, "rename-result");
    app.wait_gone(cx, "rename-result-name");
    run(&app, cx);
    assert_eq!(result_tabs(&app, cx), ["Routes", "Result 3", "Result 4"]);
}

#[gpui_kit::test]
fn closing_results_keeps_one_and_pins_are_limited(cx: &mut TestAppContext) {
    let app = TestApp::launch_demo(cx);
    app.settle(cx);
    // Closing the last result tab leaves an empty replacement.
    app.click(cx, "close-result-1");
    app.settle(cx);
    assert_eq!(result_tabs(&app, cx), ["Result 2"]);
    assert_eq!(shown(&app, cx), Some(2));
    // An empty result has no toolbar, so no query to copy.
    app.update(cx, |window, _| {
        assert!(!crate::support::present(window, &"copy-result-query".into()))
    });

    for count in 1..=10 {
        run(&app, cx);
        app.click(cx, "pin-result");
        app.settle(cx);
        assert_eq!(pin_label(&app, cx), "Unpin result");
        assert_eq!(result_tabs(&app, cx).len(), count);
    }
    // The next run opens an eleventh result tab, which cannot be pinned.
    run(&app, cx);
    assert_eq!(result_tabs(&app, cx).len(), 11);
    app.click(cx, "pin-result");
    app.settle(cx);
    assert_eq!(pin_label(&app, cx), "Pin result");

    // Closing a result tab shows its neighbor.
    app.click(cx, "result-tab-5");
    app.click(cx, "close-result-5");
    app.settle(cx);
    assert_eq!(result_tabs(&app, cx).len(), 10);
    assert_eq!(shown(&app, cx), Some(6));
    // A pinned result can always be unpinned.
    app.click(cx, "pin-result");
    app.settle(cx);
    assert_eq!(pin_label(&app, cx), "Pin result");
}
