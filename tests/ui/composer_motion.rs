//! Composer motion keeps text current and responds to interrupted size changes.
use crate::support::assistant::{FakeCodex, composer_text};
use crate::support::{MemoryCredentials, TestApp, bounds_of};
use gpui_kit::component::{
    Root,
    input::{Textarea, TextareaState},
};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
    Window, div, px, size,
};
use qrow::model::Workspace;
use std::{cell::Cell, rc::Rc, time::Duration};

fn advance(app: &TestApp, cx: &mut TestAppContext, millis: u64) {
    cx.executor().advance_clock(Duration::from_millis(millis));
    app.update(cx, |window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
    });
}

#[gpui_kit::test]
fn composer_resize_grows_reverses_and_respects_reduced_motion(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let mut workspace = codex.workspace(Workspace::default());
    workspace.settings.assistant_smooth_resize = true;
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_assistant(cx);
    app.show_conversation(cx);
    let height = |cx: &mut TestAppContext| {
        app.update(cx, |window, _| {
            bounds_of(window, "assistant-composer").size.height
        })
    };
    let empty = height(cx);
    cx.update(|cx| cx.set_reduce_motion(false));
    let eight = "1\n2\n3\n4\n5\n6\n7\n8";
    app.type_message(cx, eight);
    assert_eq!(
        app.update(cx, |window, _| composer_text(window)),
        Some(eight.into())
    );
    assert_eq!(height(cx), empty);
    advance(&app, cx, 40);
    let middle = height(cx);
    assert!(
        middle > empty,
        "The scheduled frame must advance the height"
    );
    advance(&app, cx, 200);
    let full = height(cx);
    assert!(full > middle);
    // Clear mid-growth and reverse from the current height, without a jump.
    app.type_message(cx, &"line\n".repeat(60));
    advance(&app, cx, 25);
    let interrupted = height(cx);
    assert!(interrupted > full);
    app.fill_labelled(cx, "Assistant Message", "");
    assert_eq!(height(cx), interrupted);
    advance(&app, cx, 25);
    assert!(height(cx) < interrupted && height(cx) > empty);
    advance(&app, cx, 200);
    assert_eq!(height(cx), empty);
    app.update(cx, |window, cx| {
        assert_eq!(window.simulate_next_frame(cx), 0)
    });
    cx.update(|cx| cx.set_reduce_motion(true));
    app.type_message(cx, eight);
    assert_eq!(height(cx), full);
    advance(&app, cx, 200);
    assert_eq!(height(cx), full);
    // A window resize snaps to the current row cap while content is changing.
    cx.update(|cx| cx.set_reduce_motion(false));
    app.type_message(cx, &"line\n".repeat(60));
    advance(&app, cx, 20);
    let before = height(cx);
    cx.simulate_window_resize(app.window, size(px(1280.), px(1000.)));
    app.settle(cx);
    let resized = height(cx);
    assert!(resized > before);
    advance(&app, cx, 200);
    assert_eq!(height(cx), resized);
}

struct CaretProbe {
    input: Entity<TextareaState>,
    enabled: Rc<Cell<bool>>,
}
impl Render for CaretProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            Textarea::new(&self.input)
                .smooth_caret(self.enabled.get())
                .w_full(),
        )
    }
}

#[gpui_kit::test]
fn composer_caret_eases_without_delaying_text(cx: &mut TestAppContext) {
    cx.update(qrow::ui::init);
    cx.update(|cx| cx.set_reduce_motion(false));
    let enabled = Rc::new(Cell::new(true));
    let mut input = None;
    let window = cx.open_window(size(px(640.), px(480.)), |window, cx| {
        let state = cx.new(|cx| {
            TextareaState::new(window, cx)
                .default_value("message here")
                .auto_grow(3, 8)
        });
        state.update(cx, |state, cx| {
            state.set_selected_range(0..0, cx);
            state.focus(window, cx);
        });
        input = Some(state.clone());
        let probe = cx.new(|_| CaretProbe {
            input: state,
            enabled: enabled.clone(),
        });
        Root::new(probe, window, cx)
    });
    let input = input.unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.activate_window();
        input.update(cx, |state, cx| state.focus(window, cx));
        window.press("left", cx);
    })
    .unwrap();
    cx.run_until_parked();
    let frame = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            input.read(cx).caret_presentation_bounds().unwrap()
        })
        .unwrap()
    };
    let start = frame(cx);
    cx.update_window(window.into(), |_, window, cx| window.press("right", cx))
        .unwrap();
    assert_eq!(frame(cx), start);
    cx.executor().advance_clock(Duration::from_millis(35));
    let middle = frame(cx);
    assert!(middle.origin.x > start.origin.x);
    cx.executor().advance_clock(Duration::from_millis(100));
    let end = frame(cx);
    assert!(end.origin.x > middle.origin.x);
    cx.update_window(window.into(), |_, window, cx| {
        window.input("x", cx);
        assert_eq!(input.read(cx).text().to_string(), "mxessage here");
    })
    .unwrap();
    frame(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    frame(cx);
    cx.update_window(window.into(), |_, window, cx| window.press("right", cx))
        .unwrap();
    let immediate = frame(cx);
    cx.executor().advance_clock(Duration::from_millis(35));
    assert_eq!(frame(cx), immediate);
    cx.update(|cx| cx.set_reduce_motion(false));
    enabled.set(false);
    cx.update_window(window.into(), |_, window, cx| window.press("right", cx))
        .unwrap();
    let disabled = frame(cx);
    assert!(disabled.origin.x > immediate.origin.x);
    cx.executor().advance_clock(Duration::from_millis(35));
    assert_eq!(frame(cx), disabled);
}
