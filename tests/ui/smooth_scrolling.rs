//! Discrete wheel steps ease while pixel gestures and direct manipulation stay immediate.
use gpui_kit::component::{
    Root,
    input::{Editor, EditorState},
};
use gpui_kit::test::{TestSupportExt as _, TestWindowExt as _};
use gpui_kit::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    ScrollDelta, Styled, TestAppContext, Window, div, point, px, size,
};
use std::{cell::Cell, rc::Rc, time::Duration};

struct Probe {
    editor: Entity<EditorState>,
    enabled: Rc<Cell<bool>>,
}
impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().id("scroll-probe").test_support().size_full().child(
            Editor::new(&self.editor)
                .smooth_scrolling(self.enabled.get())
                .size_full(),
        )
    }
}

#[gpui_kit::test]
fn smooth_scrolling_accumulates_wheel_steps_and_stops_after_settling(cx: &mut TestAppContext) {
    cx.update(qrow::ui::init);
    cx.update(|cx| cx.set_reduce_motion(false));
    let enabled = Rc::new(Cell::new(false));
    let mut editor = None;
    let window = cx.open_window(size(px(640.), px(180.)), |window, cx| {
        let state =
            cx.new(|cx| EditorState::new(window, cx).default_value("select 1;\n".repeat(200)));
        state.update(cx, |state, cx| state.set_selected_range(0..0, cx));
        editor = Some(state.clone());
        let probe = cx.new(|_| Probe {
            editor: state,
            enabled: enabled.clone(),
        });
        Root::new(probe, window, cx)
    });
    let editor = editor.unwrap();
    let offset = |cx: &mut TestAppContext| editor.read_with(cx, |state, _| state.scroll_offset());
    let wheel = |cx: &mut TestAppContext, delta| {
        cx.update_window(window.into(), |_, window, cx| {
            window.scroll("scroll-probe", delta, cx)
        })
        .unwrap();
    };
    let advance = |cx: &mut TestAppContext, millis| {
        cx.executor().advance_clock(Duration::from_millis(millis));
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        })
        .unwrap();
    };
    wheel(cx, ScrollDelta::Lines(point(0., -2.)));
    let direct = offset(cx).y;
    assert!(direct < px(0.));
    enabled.set(true);
    wheel(cx, ScrollDelta::Lines(point(0., -2.)));
    assert_eq!(offset(cx).y, direct);
    advance(cx, 30);
    let middle = offset(cx).y;
    assert!(middle < direct);
    // A second event adds to the pending destination, rather than losing distance.
    wheel(cx, ScrollDelta::Lines(point(0., -2.)));
    advance(cx, 150);
    assert!((offset(cx).y - direct * 3.).abs() < px(0.01));
    cx.update_window(window.into(), |_, window, cx| {
        assert_eq!(
            window.simulate_next_frame(cx),
            0,
            "settled scrolling has no frame demand"
        );
    })
    .unwrap();

    // Reversal cancels queued distance on that axis and follows the new direction.
    wheel(cx, ScrollDelta::Lines(point(0., -8.)));
    advance(cx, 20);
    let reversing = offset(cx).y;
    wheel(cx, ScrollDelta::Lines(point(0., 1.)));
    advance(cx, 30);
    assert!(
        offset(cx).y > reversing,
        "an upward wheel step must move upward"
    );
    advance(cx, 150);

    // Pixel-based trackpad events cancel pending wheel travel immediately.
    wheel(cx, ScrollDelta::Lines(point(0., -4.)));
    let before_pixel = offset(cx);
    wheel(cx, ScrollDelta::Pixels(point(px(0.), px(-7.))));
    assert_eq!(offset(cx).y, before_pixel.y - px(7.));
    let after_pixel = offset(cx);
    advance(cx, 150);
    assert_eq!(offset(cx), after_pixel);

    cx.update(|cx| cx.set_reduce_motion(true));
    wheel(cx, ScrollDelta::Lines(point(0., -2.)));
    assert!(offset(cx).y < after_pixel.y);
    let reduced = offset(cx);
    advance(cx, 150);
    assert_eq!(offset(cx), reduced);
    cx.update(|cx| cx.set_reduce_motion(false));

    // Disabling the setting settles its pending destination and stops scheduling.
    wheel(cx, ScrollDelta::Lines(point(0., -2.)));
    enabled.set(false);
    cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
        .unwrap();
    advance(cx, 10);
    let switched_off = offset(cx);
    advance(cx, 150);
    assert_eq!(offset(cx), switched_off);
    enabled.set(true);

    // Typing cancels queued travel before the input moves its caret into view.
    wheel(cx, ScrollDelta::Lines(point(0., -2.)));
    cx.update_window(window.into(), |_, window, cx| {
        window.activate_window();
        editor.update(cx, |state, cx| state.focus(window, cx));
        window.press("a", cx);
    })
    .unwrap();
    let after_typing = offset(cx);
    advance(cx, 150);
    assert_eq!(offset(cx), after_typing);

    // An explicit viewport change wins over a queued animation.
    wheel(cx, ScrollDelta::Lines(point(0., -2.)));
    cx.update_window(window.into(), |_, window, cx| {
        editor.update(cx, |state, cx| {
            state.set_scroll_offset(point(px(0.), px(0.)), cx)
        });
        window.render_frame(cx);
    })
    .unwrap();
    advance(cx, 150);
    assert_eq!(offset(cx).y, px(0.));

    // Boundaries remain clamped even when the wheel target is far past the end.
    wheel(cx, ScrollDelta::Lines(point(0., -10000.)));
    advance(cx, 150);
    let bottom = offset(cx);
    wheel(cx, ScrollDelta::Lines(point(0., -10000.)));
    advance(cx, 150);
    assert_eq!(offset(cx), bottom);
}
