//! Range selection and copy in the results table, with the demo result.
//!
//! Demo row `i` has the route `ROUTES[i % 6]`, a null carrier when `i % 7 == 0`
//! and otherwise `CARRIERS[i % 3]`, and `3821 - i` departures.
use crate::support::*;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{ClipboardItem, Modifiers, MouseButton, TestAppContext};
use std::time::Duration;

const ROUTES: [&str; 6] = [
    "BKK → SIN",
    "LHR → JFK",
    "HEL → ARN",
    "CDG → FCO",
    "DXB → BKK",
    "NRT → ICN",
];
const CARRIERS: [&str; 3] = ["Finnair", "British Airways", "Singapore Airlines"];

/// The first three result columns of demo row `i`, tab-separated.
fn first_columns(i: usize) -> String {
    let carrier = if i.is_multiple_of(7) {
        "NULL"
    } else {
        CARRIERS[i % 3]
    };
    format!("{}\t{carrier}\t{}", ROUTES[i % 6], 3821 - i)
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Modifiers::default()
    }
}

/// Clicks the cell in page row `row` and table column `column`. Column 0 is
/// the row number column.
fn click_cell(
    app: &TestApp,
    cx: &mut TestAppContext,
    row: usize,
    column: usize,
    button: MouseButton,
    modifiers: Modifiers,
) {
    app.update(cx, |window, cx| {
        let element = find_in(window, ("row", row), ("cell", column))
            .unwrap_or_else(|| panic!("No cell at row {row}, column {column}"));
        pointer_click_with(window, &element, button, modifiers, cx);
    });
    app.settle(cx);
}

fn click_header(app: &TestApp, cx: &mut TestAppContext, column: usize, modifiers: Modifiers) {
    app.update(cx, |window, cx| {
        let element = window.find(("column-header", column));
        pointer_click_with(window, &element, MouseButton::Left, modifiers, cx);
    });
    app.settle(cx);
}

/// Presses ⌘C and returns the clipboard text.
fn copy(app: &TestApp, cx: &mut TestAppContext) -> String {
    cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
    app.press(cx, "cmd-c");
    app.settle(cx);
    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default()
}

fn demo(cx: &mut TestAppContext) -> TestApp {
    let app = TestApp::launch_demo(cx);
    app.wait_until(
        cx,
        "the demo result",
        Duration::from_secs(10),
        |window, _| cell(window, 0, 1).is_some(),
    );
    app
}

#[gpui_kit::test]
fn click_shift_click_and_shift_arrows_select_a_range(cx: &mut TestAppContext) {
    let app = demo(cx);

    // One cell copies its raw value.
    click_cell(&app, cx, 1, 1, MouseButton::Left, Modifiers::default());
    assert_eq!(copy(&app, cx), ROUTES[1]);

    // Shift-click extends from the anchor to a rectangle of three rows and
    // three columns. The copy is tab-separated, without a header.
    click_cell(&app, cx, 3, 3, MouseButton::Left, shift());
    assert_eq!(
        copy(&app, cx),
        (1..=3).map(first_columns).collect::<Vec<_>>().join("\n")
    );

    // Shift+arrows move the focus corner; the anchor stays.
    app.press(cx, "shift-down");
    app.press(cx, "shift-left");
    let expected: Vec<_> = (1..=4)
        .map(|i| {
            let fields: Vec<_> = first_columns(i).split('\t').map(str::to_owned).collect();
            fields[..2].join("\t")
        })
        .collect();
    assert_eq!(copy(&app, cx), expected.join("\n"));

    // A plain arrow moves the focus and selects only that cell.
    app.press(cx, "down");
    assert_eq!(copy(&app, cx), CARRIERS[5 % 3]);
    app.press(cx, "right");
    assert_eq!(copy(&app, cx), (3821 - 5).to_string());
    app.press(cx, "up");
    assert_eq!(copy(&app, cx), (3821 - 4).to_string());

    // Escape clears the selection, so ⌘C leaves the clipboard unchanged.
    app.press(cx, "escape");
    assert_eq!(copy(&app, cx), "unchanged");
}

#[gpui_kit::test]
fn headers_row_numbers_and_select_all_select_whole_lines(cx: &mut TestAppContext) {
    let app = demo(cx);

    // A column header selects that column on the current page.
    click_header(&app, cx, 3, Modifiers::default());
    let text = copy(&app, cx);
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 1000);
    assert_eq!(lines[0], "3821");
    assert_eq!(lines[999], (3821 - 999).to_string());

    // Shift-click on another header extends to the columns between.
    click_header(&app, cx, 1, shift());
    let text = copy(&app, cx);
    assert_eq!(text.lines().count(), 1000);
    assert_eq!(text.lines().next(), Some(first_columns(0).as_str()));

    // A row number selects the whole row: all 141 result columns.
    click_cell(&app, cx, 2, 0, MouseButton::Left, Modifiers::default());
    let text = copy(&app, cx);
    assert_eq!(text.split('\t').count(), 141);
    assert!(text.starts_with(&first_columns(2)));

    // ⌘A selects the current page.
    app.press(cx, "cmd-a");
    let text = copy(&app, cx);
    assert_eq!(text.lines().count(), 1000);
    assert_eq!(text.lines().next().unwrap().split('\t').count(), 141);
}

#[gpui_kit::test]
fn the_context_menu_copies_the_selection_and_a_page_change_clears_it(cx: &mut TestAppContext) {
    let app = demo(cx);
    click_cell(&app, cx, 1, 1, MouseButton::Left, Modifiers::default());
    click_cell(&app, cx, 2, 2, MouseButton::Left, shift());

    // A right-click inside the range keeps it.
    click_cell(&app, cx, 2, 1, MouseButton::Right, Modifiers::default());
    app.wait_for(cx, "popup-menu");
    cx.write_to_clipboard(ClipboardItem::new_string("unchanged".into()));
    app.choose(cx, "popup-menu", "Copy selection");
    app.settle(cx);
    let expected: Vec<_> = (1..=2)
        .map(|i| {
            first_columns(i)
                .split('\t')
                .take(2)
                .collect::<Vec<_>>()
                .join("\t")
        })
        .collect();
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some(expected.join("\n"))
    );

    // The open menu keeps its own keys: Down moves within the menu, and
    // Escape closes the menu without clearing the selection.
    click_cell(&app, cx, 2, 2, MouseButton::Right, Modifiers::default());
    app.wait_for(cx, "popup-menu");
    app.press(cx, "down");
    app.press(cx, "escape");
    app.wait_gone(cx, "popup-menu");
    // The selection stays, and ⌘C copies it.
    assert_eq!(copy(&app, cx), expected.join("\n"));

    // A right-click outside selects that cell.
    click_cell(&app, cx, 4, 3, MouseButton::Right, Modifiers::default());
    app.wait_for(cx, "popup-menu");
    app.choose(cx, "popup-menu", "Copy cell");
    app.settle(cx);
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some((3821 - 4).to_string())
    );

    // Another page clears the selection.
    click_cell(&app, cx, 1, 1, MouseButton::Left, Modifiers::default());
    click_cell(&app, cx, 3, 3, MouseButton::Left, shift());
    app.click(cx, "next-page");
    app.settle(cx);
    assert_eq!(copy(&app, cx), "unchanged");
}

#[gpui_kit::test]
fn a_null_in_a_copied_range_is_written_as_null(cx: &mut TestAppContext) {
    let app = demo(cx);
    click_cell(&app, cx, 0, 1, MouseButton::Left, Modifiers::default());
    app.press(cx, "shift-right");
    assert_eq!(copy(&app, cx), format!("{}\tNULL", ROUTES[0]));
}
