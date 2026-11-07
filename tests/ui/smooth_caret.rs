//! The editor animates paint geometry while text and logical caret stay current.
use gpui_kit::component::{
    Root,
    input::{Editor, EditorState},
};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, ParentElement, Render, Styled, TestAppContext,
    Window, div, px, size,
};
use std::{cell::Cell, rc::Rc, time::Duration};

struct Probe {
    editor: Entity<EditorState>,
    enabled: Rc<Cell<bool>>,
}
impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            Editor::new(&self.editor)
                .smooth_caret(self.enabled.get())
                .size_full(),
        )
    }
}

#[gpui_kit::test]
fn smooth_caret_animates_paint_and_respects_reduced_motion(cx: &mut TestAppContext) {
    cx.update(qrow::ui::init);
    cx.update(|cx| cx.set_reduce_motion(false));
    let enabled = Rc::new(Cell::new(true));
    let mut editor = None;
    let window = cx.open_window(size(px(640.), px(480.)), |window, cx| {
        let state = cx
            .new(|cx| EditorState::new(window, cx).default_value("select value from t\nselect 2"));
        state.update(cx, |state, cx| {
            state.set_selected_range(0..0, cx);
            state.focus(window, cx);
        });
        editor = Some(state.clone());
        let probe = cx.new(|_| Probe {
            editor: state,
            enabled: enabled.clone(),
        });
        Root::new(probe, window, cx)
    });
    let editor = editor.unwrap();
    cx.run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.activate_window();
        editor.update(cx, |state, cx| state.focus(window, cx));
    })
    .unwrap();
    cx.run_until_parked();
    let frame = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            editor.read(cx).caret_presentation_bounds().unwrap()
        })
        .unwrap()
    };
    cx.update_window(window.into(), |_, window, cx| window.press("left", cx))
        .unwrap();
    let start = frame(cx);
    cx.update_window(window.into(), |_, window, cx| window.press("right", cx))
        .unwrap();
    assert_eq!(
        frame(cx),
        start,
        "movement begins at the last painted position"
    );
    cx.executor().advance_clock(Duration::from_millis(35));
    let middle = frame(cx);
    assert!(middle.origin.x > start.origin.x);
    cx.executor().advance_clock(Duration::from_millis(100));
    let end = frame(cx);
    assert!(end.origin.x > middle.origin.x);
    assert_eq!(frame(cx), end, "a completed transition stays settled");

    // Interrupt from the sampled position, without resetting to an old endpoint.
    cx.update_window(window.into(), |_, window, cx| window.press("right", cx))
        .unwrap();
    frame(cx);
    cx.executor().advance_clock(Duration::from_millis(20));
    let interrupted = frame(cx);
    cx.update_window(window.into(), |_, window, cx| window.press("right", cx))
        .unwrap();
    assert_eq!(frame(cx), interrupted);

    // Text changes synchronously, including while the painted caret is moving.
    cx.update_window(window.into(), |_, window, cx| {
        window.press("a", cx);
        assert!(editor.read(cx).text().to_string().starts_with("selaect"));
    })
    .unwrap();
    frame(cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    let reduced = frame(cx);
    cx.update_window(window.into(), |_, window, cx| window.press("right", cx))
        .unwrap();
    let immediate = frame(cx);
    assert!(immediate.origin.x > reduced.origin.x);
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

    enabled.set(true);
    cx.update_window(window.into(), |_, _, cx| {
        editor.update(cx, |state, cx| state.set_selected_range(21..21, cx));
    })
    .unwrap();
    let next_line = frame(cx);
    assert!(
        next_line.origin.y > disabled.origin.y,
        "line changes snap immediately"
    );
    cx.executor().advance_clock(Duration::from_millis(35));
    assert_eq!(frame(cx), next_line);

    // Let the scheduled callbacks drive repainting, without a forced refresh.
    cx.update_window(window.into(), |_, window, cx| window.press("right", cx))
        .unwrap();
    let before_callback = frame(cx);
    cx.executor().advance_clock(Duration::from_millis(20));
    cx.update_window(window.into(), |_, window, cx| {
        assert!(window.simulate_next_frame(cx) > 0);
        window.draw(cx).clear(cx);
        assert!(
            editor
                .read(cx)
                .caret_presentation_bounds()
                .unwrap()
                .origin
                .x
                > before_callback.origin.x
        );
    })
    .unwrap();
    cx.executor().advance_clock(Duration::from_millis(100));
    cx.update_window(window.into(), |_, window, cx| {
        window.simulate_next_frame(cx);
        window.draw(cx).clear(cx);
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "settled caret schedules no further frames"
        );
    })
    .unwrap();
}
