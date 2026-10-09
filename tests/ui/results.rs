//! Range selection and copy in the results table, with the demo result.
//!
//! Demo row `i` has the route `ROUTES[i % 6]`, a null carrier when `i % 7 == 0`
//! and otherwise `CARRIERS[i % 3]`, and `3821 - i` departures.
use crate::support::*;
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    ClipboardItem, InputEvent, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, Point, TestAppContext, point, px,
};
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

fn begin_drag(app: &TestApp, cx: &mut TestAppContext, row: usize, column: usize) {
    app.update(cx, |window, cx| {
        window.activate_window();
        let element = find_in(window, ("row", row), ("cell", column)).unwrap();
        assert!(element.visible(), "drag start cell must be visible");
        let position = element.bounds().center();
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        assert!(
            window.captured_hitbox().is_some(),
            "the result drag must capture the pointer"
        );
        window.render_frame(cx);
    });
}

fn move_drag(app: &TestApp, cx: &mut TestAppContext, position: Point<Pixels>) {
    app.update(cx, |window, cx| {
        window.dispatch_event(
            MouseMoveEvent {
                position,
                pressed_button: Some(MouseButton::Left),
                modifiers: Modifiers::default(),
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    });
}

fn end_drag(app: &TestApp, cx: &mut TestAppContext, position: Point<Pixels>) {
    app.update(cx, |window, cx| {
        window.dispatch_event(
            MouseUpEvent {
                button: MouseButton::Left,
                position,
                modifiers: Modifiers::default(),
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        assert!(window.captured_hitbox().is_none());
        window.render_frame(cx);
    });
    app.settle(cx);
}

#[gpui_kit::test]
fn dragging_selects_a_rectangle_in_both_directions(cx: &mut TestAppContext) {
    let app = demo(cx);
    for (start, end) in [
        ((1usize, 1usize), (3usize, 3usize)),
        ((3usize, 3usize), (1usize, 1usize)),
    ] {
        let target = app.update(cx, |window, _| {
            find_in(window, ("row", end.0), ("cell", end.1))
                .unwrap()
                .bounds()
                .center()
        });
        begin_drag(&app, cx, start.0, start.1);
        move_drag(&app, cx, target);
        end_drag(&app, cx, target);
        assert_eq!(
            copy(&app, cx),
            (1..=3).map(first_columns).collect::<Vec<_>>().join("\n")
        );
    }
}

#[gpui_kit::test]
fn drag_release_outside_stops_scrolling_and_does_not_follow_the_pointer(cx: &mut TestAppContext) {
    let app = demo(cx);
    begin_drag(&app, cx, 1, 1);
    let outside = app.update(cx, |window, _| {
        let bounds = find_in(window, ("row", 1usize), ("cell", 1usize))
            .unwrap()
            .bounds();
        point(bounds.left() - px(500.), bounds.top() - px(500.))
    });
    move_drag(&app, cx, outside);
    end_drag(&app, cx, outside);
    assert_eq!(copy(&app, cx), format!("{}\n{}", ROUTES[0], ROUTES[1]));
    let target = app.update(cx, |window, _| {
        find_in(window, ("row", 4usize), ("cell", 3usize))
            .unwrap()
            .bounds()
            .center()
    });
    move_drag(&app, cx, target);
    for _ in 0..10 {
        app.settle(cx);
    }
    assert_eq!(copy(&app, cx), format!("{}\n{}", ROUTES[0], ROUTES[1]));
}

#[gpui_kit::test]
fn a_held_drag_scrolls_both_axes_and_escape_releases_it(cx: &mut TestAppContext) {
    let app = demo(cx);
    let (outside, last_row, last_col) = app.update(cx, |window, _| {
        let cells: Vec<_> = elements(window)
            .into_iter()
            .filter(|e| e.role() == Some(gpui_kit::Role::Cell) && e.visible())
            .collect();
        let right = cells.iter().map(|e| e.bounds().right()).max().unwrap();
        let bottom = cells.iter().map(|e| e.bounds().bottom()).max().unwrap();
        let last_row = (0..1000)
            .filter(|r| cell(window, *r, 1).is_some())
            .max()
            .unwrap();
        let last_col = (1..142)
            .filter(|c| cell(window, 1, *c).is_some())
            .max()
            .unwrap();
        (point(right + px(25.), bottom + px(25.)), last_row, last_col)
    });
    begin_drag(&app, cx, 1, 1);
    move_drag(&app, cx, outside);
    for _ in 0..20 {
        app.settle(cx);
    }
    end_drag(&app, cx, outside);
    let text = copy(&app, cx);
    assert!(
        text.lines().count() > last_row,
        "vertical scrolling extends past the original viewport"
    );
    assert!(
        text.lines().next().unwrap().split('\t').count() > last_col,
        "horizontal scrolling extends past the original viewport"
    );
    // Escape must release capture without letting an active timer select again.
    app.press(cx, "cmd-home");
    app.press(cx, "home");
    app.settle(cx);
    let row = app.update(cx, |window, _| {
        (0..1000)
            .find(|r| find_in(window, ("row", *r), ("cell", 1usize)).is_some_and(|e| e.visible()))
            .unwrap()
    });
    begin_drag(&app, cx, row, 1);
    app.press(cx, "escape");
    app.update(cx, |window, _| assert!(window.captured_hitbox().is_none()));
    for _ in 0..5 {
        app.settle(cx);
    }
    assert_eq!(copy(&app, cx), "unchanged");
}

#[gpui_kit::test]
fn changing_tabs_during_a_drag_releases_capture_and_stops_the_old_table(cx: &mut TestAppContext) {
    let app = demo(cx);
    begin_drag(&app, cx, 1, 1);
    let target = app.update(cx, |window, _| {
        find_in(window, ("row", 3usize), ("cell", 3usize))
            .unwrap()
            .bounds()
            .center()
    });
    move_drag(&app, cx, target);
    app.press(cx, "cmd-t");
    for _ in 0..5 {
        app.settle(cx);
    }
    app.update(cx, |window, _| assert!(window.captured_hitbox().is_none()));
    // Mouse-up belongs to the new tab. Return to the old result and verify its rectangle.
    end_drag(&app, cx, target);
    app.click_labelled(cx, "Route overview");
    app.settle(cx);
    click_cell(&app, cx, 2, 2, MouseButton::Right, Modifiers::default());
    app.choose(cx, "popup-menu", "Copy selection");
    app.settle(cx);
    assert_eq!(
        cx.read_from_clipboard().unwrap().text().unwrap(),
        (1..=3).map(first_columns).collect::<Vec<_>>().join("\n")
    );
}

#[gpui_kit::test]
fn a_new_query_during_a_drag_releases_capture(cx: &mut TestAppContext) {
    let app = demo(cx);
    begin_drag(&app, cx, 1, 1);
    app.press(cx, "cmd-enter");
    for _ in 0..5 {
        app.settle(cx);
    }
    app.update(cx, |window, _| assert!(window.captured_hitbox().is_none()));
}

#[gpui_kit::test]
fn a_held_drag_scrolls_back_to_the_first_row_and_column(cx: &mut TestAppContext) {
    let app = demo(cx);
    click_cell(&app, cx, 1, 1, MouseButton::Left, Modifiers::default());
    for _ in 0..30 {
        app.press(cx, "down");
        app.settle(cx);
    }
    for _ in 0..14 {
        app.press(cx, "right");
        app.settle(cx);
    }
    app.settle(cx);
    begin_drag(&app, cx, 30, 10);
    let outside = point(px(-1000.), px(-1000.));
    move_drag(&app, cx, outside);
    for _ in 0..40 {
        app.settle(cx);
    }
    end_drag(&app, cx, outside);
    let text = copy(&app, cx);
    assert_eq!(text.lines().count(), 31);
    assert_eq!(text.lines().next().unwrap().split('\t').count(), 10);
    assert!(text.starts_with(&first_columns(0)));
}

#[gpui_kit::test]
fn dragging_after_column_resize_uses_the_new_cell_bounds(cx: &mut TestAppContext) {
    let app = demo(cx);
    let (start, width) = app.update(cx, |window, cx| {
        let bounds = window.find(("col-header", 1usize)).bounds();
        let start = point(bounds.right() - px(2.), bounds.center().y);
        window.dispatch_event(
            MouseDownEvent {
                button: MouseButton::Left,
                position: start,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
        (start, bounds.size.width)
    });
    let target = start + point(px(80.), px(0.));
    move_drag(&app, cx, start + point(px(20.), px(0.)));
    move_drag(&app, cx, target);
    end_drag(&app, cx, target);
    app.update(cx, |window, _| {
        assert!(window.find(("col-header", 1usize)).bounds().size.width > width + px(60.))
    });
    let target = app.update(cx, |window, _| {
        find_in(window, ("row", 3usize), ("cell", 3usize))
            .unwrap()
            .bounds()
            .center()
    });
    begin_drag(&app, cx, 1, 1);
    move_drag(&app, cx, target);
    end_drag(&app, cx, target);
    assert_eq!(
        copy(&app, cx),
        (1..=3).map(first_columns).collect::<Vec<_>>().join("\n")
    );
}
