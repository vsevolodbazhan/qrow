//! Interruptible sidebar motion and content transitions that preserve input.
use gpui_kit::base::{Presence, TestSupportExt as _, Transition, transition};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, Size,
    Styled, Window, div, px,
};
use std::time::{Duration, Instant};

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

/// Clip fixed-width content while opening. Remove controls when closing so
/// focus traversal and accessibility cannot reach the exiting pane.
pub(super) fn sidebar_panel(
    id: &'static str,
    shown: bool,
    width: Pixels,
    full_width: Pixels,
    content: AnyElement,
    cx: &App,
) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .relative()
        .w(width)
        .h_full()
        .flex_shrink_0()
        .min_h_0()
        .overflow_hidden()
        .bg(cx.theme().sidebar)
        .when(shown, |el| {
            el.child(
                div()
                    .w(full_width)
                    .h_full()
                    .flex_shrink_0()
                    .opacity((width / full_width).clamp(0., 1.))
                    .child(content),
            )
        })
        .when(!shown, |el| {
            el.child(
                div()
                    .absolute()
                    .inset_0()
                    .occlude()
                    .capture_any_mouse_down(|_, window, cx| {
                        window.prevent_default();
                        cx.stop_propagation();
                    })
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation()),
            )
        })
}

#[derive(Clone, Copy, PartialEq)]
struct SidebarGeometry {
    viewport: Size<Pixels>,
    rem: Pixels,
}

/// Keep one channel mounted at zero width so closing can reverse mid-flight.
/// A resize follows the pointer or window immediately.
pub(super) fn sidebar_width(
    channel: &'static str,
    shown: bool,
    width: Pixels,
    enabled: bool,
    resizing: bool,
    window: &mut Window,
    cx: &mut App,
) -> Pixels {
    let geometry = SidebarGeometry {
        viewport: window.viewport_size(),
        rem: window.rem_size(),
    };
    let previous =
        window.use_keyed_state((ElementId::from(channel), "geometry"), cx, |_, _| geometry);
    let geometry_changed = *previous.read(cx) != geometry;
    if geometry_changed {
        previous.update(cx, |previous, _| *previous = geometry);
    }
    let duration = if enabled && !resizing && !geometry_changed {
        cx.theme().motion_tokens().duration_normal
    } else {
        Duration::ZERO
    };
    transition(
        ElementId::from((ElementId::from(channel), "width")),
        if shown { width } else { px(0.) },
        Transition::new(duration).easing(cx.theme().motion_tokens().easing_move.clone()),
        window,
        cx,
    )
}

#[derive(Clone, PartialEq)]
struct ContentMotion {
    key: String,
    revision: u64,
    from: f32,
    started: Instant,
}
impl ContentMotion {
    fn sample(&self, now: Instant, duration: Duration) -> f32 {
        if self.from == 1. || duration.is_zero() {
            return 1.;
        }
        let elapsed = now.saturating_duration_since(self.started);
        let time = (elapsed.as_secs_f32() / duration.as_secs_f32()).min(1.);
        self.from + (1. - self.from) * (1. - (1. - time).powi(3))
    }
}

#[derive(Clone, Copy)]
pub(super) struct ContentSample {
    progress: f32,
    distance: Pixels,
}
impl ContentSample {
    pub(super) fn opacity(self) -> f32 {
        0.75 + 0.25 * self.progress
    }
    pub(super) fn offset(self) -> Pixels {
        self.distance * (1. - self.progress)
    }
}

/// Change content immediately, without fading through an empty viewport. Fast
/// switches continue from the sampled appearance; editing settles it at once.
pub(super) fn content(
    channel: &'static str,
    key: String,
    revision: u64,
    enabled: bool,
    window: &mut Window,
    cx: &mut App,
) -> ContentSample {
    let now = cx.background_executor().now();
    let duration = cx.theme().motion_tokens().duration_normal;
    let state = window.use_keyed_state((ElementId::from(channel), "content"), cx, |_, _| {
        ContentMotion {
            key: key.clone(),
            revision,
            from: 1.,
            started: now,
        }
    });
    let before = state.read(cx).clone();
    let mut next = before.clone();
    let current = before.sample(now, duration);
    if !enabled || cx.reduce_motion() || (before.key == key && before.revision != revision) {
        if before.key != key || before.revision != revision || before.from != 1. {
            next = ContentMotion {
                key,
                revision,
                from: 1.,
                started: now,
            };
        }
    } else if before.key != key {
        next = ContentMotion {
            key,
            revision,
            from: if current < 1. { current } else { 0. },
            started: now,
        };
    }
    let progress = next.sample(now, duration);
    if next != before {
        state.update(cx, |state, _| *state = next);
    }
    if progress < 1. {
        window.request_animation_frame();
    }
    ContentSample {
        progress,
        distance: cx
            .theme()
            .motion_tokens()
            .distance_short
            .to_pixels(window.rem_size()),
    }
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

    struct RefinementProbe {
        shown: Rc<Cell<bool>>,
        width: Rc<Cell<f32>>,
        dragging: Rc<Cell<bool>>,
        key: Rc<Cell<usize>>,
        revision: Rc<Cell<u64>>,
        enabled: Rc<Cell<bool>>,
        sampled_width: Rc<Cell<Pixels>>,
        sampled_content: Rc<Cell<(f32, Pixels)>>,
    }
    impl Render for RefinementProbe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.sampled_width.set(sidebar_width(
                "probe-sidebar",
                self.shown.get(),
                px(self.width.get()),
                self.enabled.get(),
                self.dragging.get(),
                window,
                cx,
            ));
            let cue = content(
                "probe-content",
                self.key.get().to_string(),
                self.revision.get(),
                self.enabled.get(),
                window,
                cx,
            );
            self.sampled_content.set((cue.opacity(), cue.offset()));
            div()
        }
    }

    #[gpui_kit::test]
    fn sidebar_and_content_motion_reverse_settle_and_stop_at_idle(cx: &mut TestAppContext) {
        cx.update(super::super::init);
        cx.update(|cx| cx.set_reduce_motion(false));
        let shown = Rc::new(Cell::new(true));
        let width = Rc::new(Cell::new(240.));
        let dragging = Rc::new(Cell::new(false));
        let key = Rc::new(Cell::new(0));
        let revision = Rc::new(Cell::new(0));
        let enabled = Rc::new(Cell::new(true));
        let sampled_width = Rc::new(Cell::new(px(-1.)));
        let sampled_content = Rc::new(Cell::new((-1., px(-1.))));
        let window = cx.open_window(size(px(640.), px(480.)), |_, _| RefinementProbe {
            shown: shown.clone(),
            width: width.clone(),
            dragging: dragging.clone(),
            key: key.clone(),
            revision: revision.clone(),
            enabled: enabled.clone(),
            sampled_width: sampled_width.clone(),
            sampled_content: sampled_content.clone(),
        });
        let render = |cx: &mut TestAppContext| {
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
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
        render(cx);
        assert_eq!(sampled_width.get(), px(240.));
        assert_eq!(sampled_content.get(), (1., px(0.)));
        shown.set(false);
        key.set(1);
        render(cx);
        assert_eq!(sampled_width.get(), px(240.));
        assert_eq!(
            sampled_content.get().0,
            0.75,
            "content never starts with a blank frame"
        );
        advance(cx, 40);
        let halfway_width = sampled_width.get();
        let halfway_content = sampled_content.get();
        assert!(halfway_width > px(0.) && halfway_width < px(240.));
        assert!(halfway_content.0 > 0.75 && halfway_content.0 < 1.);
        shown.set(true);
        key.set(2);
        render(cx);
        assert!((sampled_width.get() - halfway_width).abs() < px(0.01));
        assert_eq!(
            sampled_content.get(),
            halfway_content,
            "rapid content changes continue from the sampled appearance"
        );
        revision.set(1);
        render(cx);
        assert_eq!(
            sampled_content.get(),
            (1., px(0.)),
            "editing settles the editor appearance immediately"
        );
        advance(cx, 250);
        assert_eq!(sampled_width.get(), px(240.));
        cx.update_window(window.into(), |_, window, cx| {
            assert_eq!(window.simulate_next_frame(cx), 0)
        })
        .unwrap();
        dragging.set(true);
        width.set(300.);
        render(cx);
        assert_eq!(
            sampled_width.get(),
            px(300.),
            "splitter dragging stays direct"
        );
        dragging.set(false);
        shown.set(false);
        render(cx);
        cx.simulate_window_resize(window.into(), size(px(600.), px(480.)));
        render(cx);
        assert_eq!(
            sampled_width.get(),
            px(0.),
            "window resizing settles geometry immediately"
        );
        shown.set(true);
        key.set(3);
        cx.update(|cx| cx.set_reduce_motion(true));
        render(cx);
        assert_eq!(sampled_width.get(), px(300.));
        assert_eq!(sampled_content.get(), (1., px(0.)));
        cx.update(|cx| cx.set_reduce_motion(false));
        shown.set(false);
        key.set(4);
        render(cx);
        enabled.set(false);
        render(cx);
        assert_eq!(sampled_width.get(), px(0.));
        assert_eq!(sampled_content.get(), (1., px(0.)));
        advance(cx, 250);
        cx.update_window(window.into(), |_, window, cx| {
            assert_eq!(window.simulate_next_frame(cx), 0)
        })
        .unwrap();
    }
}
