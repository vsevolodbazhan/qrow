//! Application-owned policy for short content entrances. Layout and input stay immediate.
use gpui_kit::base::{Presence, Transition};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{App, ElementId, Window};
use std::time::Duration;

pub(super) fn fade_in(
    channel: &'static str,
    key: String,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    let duration = if enabled {
        cx.theme().motion_tokens().duration_fast
    } else {
        Duration::ZERO
    };
    Presence::new(
        ElementId::NamedChild(ElementId::from(channel).into(), key.into()),
        true,
    )
    .transition(Transition::new(duration))
    .sample(window, cx)
    .progress
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{AppContext as _, Context, IntoElement, Render, TestAppContext, div, px, size};
    use std::{cell::Cell, rc::Rc};

    struct Probe {
        key: Rc<Cell<usize>>,
        enabled: Rc<Cell<bool>>,
        opacity: Rc<Cell<f32>>,
    }
    impl Render for Probe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.opacity.set(fade_in(
                "probe-fade",
                self.key.get().to_string(),
                self.enabled.get(),
                window,
                cx,
            ));
            div()
        }
    }

    #[gpui_kit::test]
    fn panel_fades_are_opt_in_bounded_and_respect_reduced_motion(cx: &mut TestAppContext) {
        cx.update(super::super::init);
        cx.update(|cx| cx.set_reduce_motion(false));
        let key = Rc::new(Cell::new(0));
        let enabled = Rc::new(Cell::new(false));
        let opacity = Rc::new(Cell::new(-1.));
        let window = cx.open_window(size(px(200.), px(100.)), |_, _| Probe {
            key: key.clone(),
            enabled: enabled.clone(),
            opacity: opacity.clone(),
        });
        let render = |cx: &mut TestAppContext| {
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
            opacity.get()
        };
        assert_eq!(render(cx), 1.);
        enabled.set(true);
        assert_eq!(
            render(cx),
            1.,
            "enabling does not fade content already shown"
        );
        key.set(1);
        assert_eq!(render(cx), 0.);
        cx.executor().advance_clock(Duration::from_millis(60));
        cx.update_window(window.into(), |_, window, cx| {
            assert!(window.simulate_next_frame(cx) > 0);
            window.draw(cx).clear(cx);
        })
        .unwrap();
        assert!(opacity.get() > 0. && opacity.get() < 1.);
        cx.executor().advance_clock(Duration::from_millis(100));
        cx.update_window(window.into(), |_, window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
            assert_eq!(window.simulate_next_frame(cx), 0);
        })
        .unwrap();
        assert_eq!(opacity.get(), 1.);
        cx.update(|cx| cx.set_reduce_motion(true));
        key.set(2);
        assert_eq!(render(cx), 1.);
        cx.update(|cx| cx.set_reduce_motion(false));
        enabled.set(false);
        key.set(3);
        assert_eq!(render(cx), 1.);
    }
}
