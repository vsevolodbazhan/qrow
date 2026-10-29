use crate::support::*;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ClipboardItem, Modifiers, MouseButton, TestAppContext};
use std::time::Duration;

fn demo(cx: &mut TestAppContext) -> TestApp {
    let app = TestApp::launch_demo(cx);
    app.wait_cell(cx, 0, 1, "BKK → SIN");
    app
}

fn open(app: &TestApp, cx: &mut TestAppContext) {
    app.click(cx, "export-results");
    app.wait_until(
        cx,
        "the export form",
        Duration::from_secs(2),
        |window, _| window.try_find("export-save").is_some(),
    );
}

fn copy(app: &TestApp, cx: &mut TestAppContext) -> String {
    cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
    app.click(cx, "export-copy");
    app.wait_until(cx, "copied CSV", Duration::from_secs(5), |_, cx| {
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .is_some_and(|text| text != "unchanged")
    });
    cx.read_from_clipboard().unwrap().text().unwrap()
}

fn select(app: &TestApp, cx: &mut TestAppContext) {
    for (row, column, shift) in [(1_usize, 1_usize, false), (3, 3, true)] {
        app.update(cx, |window, cx| {
            let cell = find_in(window, ("row", row), ("cell", column)).unwrap();
            pointer_click_with(
                window,
                &cell,
                MouseButton::Left,
                Modifiers {
                    shift,
                    ..Modifiers::default()
                },
                cx,
            );
        });
        app.settle(cx);
    }
}

#[gpui_kit::test]
fn export_is_disabled_before_a_result_has_columns(cx: &mut TestAppContext) {
    let app = TestApp::launch(cx, qrow::model::Workspace::default());
    app.click(cx, "export-results");
    app.update(cx, |window, _| {
        assert!(window.try_find("export-save").is_none())
    });
}

#[gpui_kit::test]
fn export_selection_presets_copy_and_saved_options(cx: &mut TestAppContext) {
    let app = demo(cx);
    select(&app, cx);
    open(&app, cx);
    app.select(cx, "export-preset", "Excel (semicolon)");
    let text = copy(&app, cx);
    assert_eq!(
        text,
        "\u{feff}route;carrier;departures\r\nLHR → JFK;British Airways;3820\r\nHEL → ARN;Singapore Airlines;3819\r\nCDG → FCO;Finnair;3818\r\n"
    );
    open(&app, cx);
    app.update(cx, |window, _| {
        assert_eq!(
            value(window, "export-preset").as_deref(),
            Some("Excel (semicolon)")
        )
    });
    app.select(cx, "export-null", "NULL");
    app.update(cx, |window, _| {
        assert_eq!(value(window, "export-preset").as_deref(), Some("Custom"))
    });
    let text = copy(&app, cx);
    assert!(
        text.starts_with("\u{feff}route;carrier;departures\r\n"),
        "{text}"
    );
    open(&app, cx);
    app.update(cx, |window, _| {
        assert_eq!(value(window, "export-null").as_deref(), Some("NULL"))
    });
    app.click(cx, "export-cancel");
}

#[gpui_kit::test]
fn export_downloaded_rows_includes_other_pages_and_headers(cx: &mut TestAppContext) {
    let app = demo(cx);
    select(&app, cx);
    open(&app, cx);
    app.select(cx, "export-rows", "Downloaded rows (2250)");
    let text = copy(&app, cx);
    let mut reader = csv::Reader::from_reader(text.as_bytes());
    assert_eq!(reader.headers().unwrap().len(), 141);
    let records = reader.records().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(records.len(), 2250);
    assert_eq!(&records[0][0], "BKK → SIN");
    assert_eq!(&records[2249][2], "1572");
}

#[gpui_kit::test]
fn export_save_uses_the_native_prompt_and_remembers_the_directory(cx: &mut TestAppContext) {
    let app = demo(cx);
    select(&app, cx);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("result.csv");
    std::fs::write(&path, "old").unwrap();
    open(&app, cx);
    app.click(cx, "export-save");
    assert!(cx.did_prompt_for_new_path());
    cx.simulate_new_path_selection(|_| Some(path.clone()));
    app.wait_until(cx, "saved CSV", Duration::from_secs(5), |_, _| {
        std::fs::read_to_string(&path)
            .is_ok_and(|text| text.starts_with("route,carrier,departures\r\n"))
    });
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        csv::Reader::from_reader(text.as_bytes()).records().count(),
        3
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    app.wait_until(
        cx,
        "dismissed export",
        Duration::from_secs(2),
        |window, _| window.try_find("export-save").is_none(),
    );
    open(&app, cx);
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|suggested| {
        assert_eq!(suggested, directory.path());
        None
    });
    app.settle(cx);
    app.click(cx, "export-cancel");
}

#[gpui_kit::test]
fn cancelled_save_prompt_keeps_options_and_escape_restores_table_focus(cx: &mut TestAppContext) {
    let app = demo(cx);
    select(&app, cx);
    open(&app, cx);
    app.click(cx, "export-save");
    cx.simulate_new_path_selection(|_| None);
    app.wait_until(
        cx,
        "available export actions",
        Duration::from_secs(2),
        |window, _| window.find("export-copy").disabled() != Some(true),
    );
    app.press(cx, "escape");
    app.wait_until(
        cx,
        "dismissed export",
        Duration::from_secs(2),
        |window, _| window.try_find("export-save").is_none(),
    );
    // Copy after dismissal still acts on the selected cells.
    app.press(cx, "cmd-c");
    assert!(
        cx.read_from_clipboard()
            .unwrap()
            .text()
            .unwrap()
            .starts_with("LHR → JFK\t")
    );
}

#[gpui_kit::test]
fn copy_as_uses_the_selection_and_the_tsv_preset(cx: &mut TestAppContext) {
    let app = demo(cx);
    select(&app, cx);
    app.update(cx, |window, cx| {
        let cell = find_in(window, ("row", 2_usize), ("cell", 2_usize)).unwrap();
        pointer_click_with(window, &cell, MouseButton::Right, Modifiers::default(), cx);
    });
    app.settle(cx);
    cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
    app.choose_in_submenu(cx, "Copy as", "TSV");
    app.wait_until(cx, "TSV clipboard", Duration::from_secs(3), |_, cx| {
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .is_some_and(|text| text.starts_with("route\tcarrier\tdepartures\n"))
    });
    assert_eq!(
        cx.read_from_clipboard().unwrap().text().unwrap(),
        "route\tcarrier\tdepartures\nLHR → JFK\tBritish Airways\t3820\nHEL → ARN\tSingapore Airlines\t3819\nCDG → FCO\tFinnair\t3818\n"
    );
}

#[gpui_kit::test]
fn enter_after_open_starts_save_instead_of_dismissing_the_export(cx: &mut TestAppContext) {
    let app = demo(cx);
    open(&app, cx);
    app.press(cx, "enter");
    assert!(cx.did_prompt_for_new_path());
    app.update(cx, |window, _| {
        assert!(window.try_find("export-save").is_some())
    });
    cx.simulate_new_path_selection(|_| None);
    app.settle(cx);
    app.click(cx, "export-cancel");
}

#[gpui_kit::test]
fn custom_null_validation_and_saved_value_follow_the_separator(cx: &mut TestAppContext) {
    let app = demo(cx);
    select(&app, cx);
    open(&app, cx);
    app.select(cx, "export-null", "Custom");
    app.fill(cx, "export-null-text", "a,b");
    app.settle(cx);
    app.wait_for(cx, "export-error");
    cx.write_to_clipboard(ClipboardItem::new_string("preserved".into()));
    app.click(cx, "export-save");
    assert!(!cx.did_prompt_for_new_path());
    app.click(cx, "export-copy");
    assert_eq!(
        cx.read_from_clipboard().unwrap().text().as_deref(),
        Some("preserved")
    );
    app.press(cx, "enter");
    assert!(!cx.did_prompt_for_new_path());
    app.select(cx, "export-separator", "Pipe");
    let text = copy(&app, cx);
    assert!(text.starts_with("route|carrier|departures\r\n"));
    open(&app, cx);
    app.update(cx, |window, _| {
        assert_eq!(value(window, "export-null").as_deref(), Some("Custom"));
        assert_eq!(value(window, "export-null-text").as_deref(), Some("a,b"));
    });
    app.select(cx, "export-separator", "Comma");
    app.fill(cx, "export-null-text", "未知😀");
    app.settle(cx);
    let text = copy(&app, cx);
    assert!(text.starts_with("route,carrier,departures\r\n"));
}

#[gpui_kit::test]
fn copy_as_csv_includes_headers_after_a_headerless_tsv_export(cx: &mut TestAppContext) {
    let app = demo(cx);
    select(&app, cx);
    open(&app, cx);
    app.select(cx, "export-preset", "Tab-separated");
    app.click(cx, "export-header");
    assert!(copy(&app, cx).starts_with("LHR → JFK\t"));
    app.update(cx, |window, cx| {
        let cell = find_in(window, ("row", 2usize), ("cell", 2usize)).unwrap();
        pointer_click_with(window, &cell, MouseButton::Right, Modifiers::default(), cx);
    });
    app.settle(cx);
    cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
    app.choose_in_submenu(cx, "Copy as", "CSV");
    app.wait_until(cx, "CSV with headers", Duration::from_secs(3), |_, cx| {
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .is_some_and(|text| text.starts_with("route,carrier,departures\n"))
    });
}

#[gpui_kit::test]
fn quit_with_an_active_export_keeps_the_writer_when_declined(cx: &mut TestAppContext) {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let app = demo(cx);
    let cancel = Arc::new(AtomicBool::new(false));
    let guard = cx.update(|cx| cx.global::<qrow::export::Jobs>().register(cancel.clone()));
    app.dispatch(cx, qrow::ui::Quit);
    app.wait_for(cx, "keep-working");
    app.click(cx, "keep-working");
    app.wait_gone(cx, "keep-working");
    assert!(!cancel.load(Ordering::Relaxed));
    drop(guard);
}

#[gpui_kit::test]
fn confirmed_quit_cancels_an_export_before_waiting_for_cleanup(cx: &mut TestAppContext) {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let app = demo(cx);
    let cancel = Arc::new(AtomicBool::new(false));
    let guard = cx.update(|cx| cx.global::<qrow::export::Jobs>().register(cancel.clone()));
    let worker_cancel = cancel.clone();
    let worker = std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !worker_cancel.load(Ordering::Relaxed) {
            assert!(
                std::time::Instant::now() < deadline,
                "Quit did not cancel the writer"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        drop(guard);
    });
    app.dispatch(cx, qrow::ui::Quit);
    app.wait_for(cx, "quit-anyway");
    app.click(cx, "quit-anyway");
    worker.join().unwrap();
    assert!(cancel.load(Ordering::Relaxed));
    assert_eq!(
        cx.update(|cx| cx.global::<qrow::export::Jobs>().active_count()),
        0
    );
}
