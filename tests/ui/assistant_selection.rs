//! Text selection in the transcript while the transcript scrolls.
use crate::support::assistant::FakeCodex;
use crate::support::{MemoryCredentials, TestApp, bounds_of, elements};
use gpui_kit::InputEvent as _;
use gpui_kit::base::TextSelection;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    ScrollDelta, ScrollWheelEvent, TestAppContext, Window, point, px,
};
use qrow::model::Workspace;
use std::time::Duration;

const REPLIES: usize = 4;

/// A conversation with `REPLIES` replies of 40 lines each, shown at its end.
fn long_conversation(cx: &mut TestAppContext) -> TestApp {
    let (directory, codex) = FakeCodex::new();
    let workspace = codex.workspace(Workspace::default());
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_assistant(cx);
    app.show_conversation(cx);
    for reply in 1..=REPLIES {
        app.send(cx, &format!("Show many lines {reply}"));
        app.wait_reply(cx, &format!("Line 40: synthetic assistant text {reply}"));
        app.wait_idle(cx);
    }
    app.settle(cx);
    app
}

/// The label and the top of each transcript entry that the list draws.
fn entries(window: &Window) -> Vec<(String, f32)> {
    let mut entries: Vec<_> = elements(window)
        .into_iter()
        .filter(|element| {
            element
                .path()
                .last()
                .is_some_and(|id| format!("{id:?}").contains("assistant-entry-"))
        })
        .filter_map(|element| {
            Some((
                element.label()?.to_owned(),
                f32::from(element.bounds().top()),
            ))
        })
        .collect();
    entries.sort_by(|a, b| a.1.total_cmp(&b.1));
    entries
}

/// How far the entries that two frames both draw moved between them.
fn moved(before: &[(String, f32)], after: &[(String, f32)]) -> Vec<f32> {
    before
        .iter()
        .filter_map(|(label, top)| {
            after
                .iter()
                .find(|(other, _)| other == label)
                .map(|(_, other)| other - top)
        })
        .collect()
}

fn press(window: &mut Window, position: Point<Pixels>, cx: &mut gpui_kit::App) {
    window.activate_window();
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
    window.render_frame(cx);
}

fn drag_to(window: &mut Window, position: Point<Pixels>, cx: &mut gpui_kit::App) {
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
}

fn release(window: &mut Window, position: Point<Pixels>, cx: &mut gpui_kit::App) {
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
    window.render_frame(cx);
}

fn wheel(window: &mut Window, position: Point<Pixels>, dy: Pixels, cx: &mut gpui_kit::App) {
    window.dispatch_event(
        ScrollWheelEvent {
            position,
            delta: ScrollDelta::Pixels(point(px(0.), dy)),
            ..Default::default()
        }
        .to_platform_input(),
        cx,
    );
    window.render_frame(cx);
}

/// Asserts that `selected` holds the text from the start of the drag, in
/// the last reply, back to the text in reply `first`, in document order.
fn assert_selected_back_to(selected: &str, first: usize) {
    // The drag starts above the last lines of the last reply.
    assert!(
        !selected.contains(&format!("Line 40: synthetic assistant text {REPLIES}")),
        "the selection lost its start: {selected:?}"
    );
    let mut previous = 0;
    for reply in first..=REPLIES {
        let text = if reply == first {
            format!("Line 40: synthetic assistant text {reply}")
        } else {
            format!("Line 1: synthetic assistant text {reply}")
        };
        let position = selected
            .find(&text)
            .unwrap_or_else(|| panic!("the selection does not have {text:?}: {selected:?}"));
        assert!(
            position >= previous,
            "{text:?} is out of order: {selected:?}"
        );
        previous = position;
        if reply > first {
            assert!(
                selected.contains(&format!("Show many lines {reply}")),
                "the selection skipped message {reply}: {selected:?}"
            );
        }
    }
}

#[gpui_kit::test]
fn a_drag_selection_follows_the_pointer_while_the_wheel_scrolls(cx: &mut TestAppContext) {
    let app = long_conversation(cx);
    let step = px(60.);
    let pointer = app.update(cx, |window, cx| {
        let list = bounds_of(window, "assistant-transcript");
        let pointer = point(list.left() + px(200.), list.top() + px(200.));
        // Leave the end of the transcript, which stops following new text.
        wheel(window, pointer, step, cx);
        press(
            window,
            point(list.left() + px(40.), list.bottom() - px(80.)),
            cx,
        );
        drag_to(window, pointer, cx);
        let selected = TextSelection::selected_text(window, cx);
        assert!(
            selected.contains("synthetic"),
            "the drag did not select: {selected:?}"
        );
        pointer
    });
    // Scroll up through more than two replies with the button held.
    let mut length = 0;
    for _ in 0..60 {
        app.update(cx, |window, cx| {
            let before = entries(window);
            wheel(window, pointer, step, cx);
            let moved = moved(&before, &entries(window));
            assert!(
                moved
                    .iter()
                    .all(|delta| (delta - f32::from(step)).abs() < 0.5),
                "the transcript did not follow the wheel: {moved:?}"
            );
            let selected = TextSelection::selected_text(window, cx).len();
            assert!(
                selected > length,
                "the selection did not follow the pointer: {selected} after {length}"
            );
            length = selected;
        });
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
    }
    let selected = app.update(cx, |window, cx| {
        release(window, pointer, cx);
        TextSelection::selected_text(window, cx)
    });
    assert_selected_back_to(&selected, 2);
}

#[gpui_kit::test]
fn a_drag_past_the_transcript_scrolls_and_extends_the_selection(cx: &mut TestAppContext) {
    let app = long_conversation(cx);
    let above = app.update(cx, |window, cx| {
        let list = bounds_of(window, "assistant-transcript");
        press(
            window,
            point(list.left() + px(40.), list.bottom() - px(80.)),
            cx,
        );
        drag_to(
            window,
            point(list.left() + px(200.), list.top() + px(200.)),
            cx,
        );
        // Above the transcript: the drag scrolls it up.
        let above = point(list.left() + px(200.), list.top() - px(40.));
        drag_to(window, above, cx);
        above
    });
    // About 130 ticks reach the top. The rest check that it stays there.
    for _ in 0..240 {
        cx.executor().advance_clock(Duration::from_millis(16));
        cx.run_until_parked();
        app.update(cx, |window, cx| window.render_frame(cx));
    }
    let selected = app.update(cx, |window, cx| {
        assert!(
            entries(window)
                .iter()
                .any(|(label, _)| label == "You: Show many lines 1"),
            "auto-scroll did not stay at the start of the conversation"
        );
        release(window, above, cx);
        TextSelection::selected_text(window, cx)
    });
    assert_selected_back_to(&selected, 1);
}
