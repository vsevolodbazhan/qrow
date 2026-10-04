use gpui_kit::component::{ActiveTheme, h_flex, kbd::Kbd, tooltip::Tooltip, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::rc::Rc;

/// The object name leads. Status and connection details use secondary text.
#[derive(Clone, PartialEq)]
pub(super) struct StatusTooltip {
    title: SharedString,
    status: SharedString,
    detail: Option<SharedString>,
    error: Option<SharedString>,
}

impl FluentBuilder for StatusTooltip {}

impl StatusTooltip {
    pub(super) fn new(title: impl Into<SharedString>, status: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            status: status.into(),
            detail: None,
            error: None,
        }
    }

    pub(super) fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub(super) fn error(mut self, error: impl Into<SharedString>) -> Self {
        self.error = Some(error.into());
        self
    }

    pub(super) fn build(
        &self,
        action: Option<&dyn Action>,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyView {
        let content = self.clone();
        let shortcut = action
            .and_then(|action| {
                window
                    .highest_precedence_binding_for_action_in_context(action, KeyContext::default())
            })
            .and_then(|binding| {
                binding
                    .keystrokes()
                    .first()
                    .map(|key| key.as_keystroke().clone())
            });
        let status_below = action.is_some();
        Tooltip::element(move |window, cx| {
            let secondary = |id, text: SharedString| {
                div()
                    .id(id)
                    .test_support()
                    .aria_label(text.clone())
                    .text_xs()
                    .font_weight(FontWeight::NORMAL)
                    .text_color(cx.theme().secondary_foreground)
                    .child(text)
            };
            let mut title = div()
                .id("status-tooltip-title")
                .test_support()
                .flex_1()
                .min_w_0()
                .aria_label(content.title.clone())
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .child(content.title.clone());
            let trailing = if status_below {
                shortcut.clone().map(|shortcut| {
                    let label: SharedString = Kbd::format(&shortcut).into();
                    let element = div()
                        .id("status-tooltip-shortcut")
                        .test_support()
                        .flex_shrink_0()
                        .aria_label(label.clone())
                        .text_xs()
                        .font_weight(FontWeight::NORMAL)
                        .text_color(cx.theme().secondary_foreground)
                        .child(Kbd::new(shortcut).appearance(false));
                    (label, element)
                })
            } else {
                Some((
                    content.status.clone(),
                    secondary("status-tooltip-status", content.status.clone()).flex_shrink_0(),
                ))
            };
            let (title, trailing) = match trailing {
                Some((text, mut element)) => {
                    let title_baseline =
                        first_line_baseline(&content.title, &title.style().text, window, cx);
                    let trailing_baseline =
                        first_line_baseline(&text, &element.style().text, window, cx);
                    let baseline = title_baseline.max(trailing_baseline);
                    (
                        title.mt(baseline - title_baseline),
                        Some(element.mt(baseline - trailing_baseline)),
                    )
                }
                None => (title, None),
            };
            v_flex()
                .id("status-tooltip")
                .test_support()
                .max_w_96()
                .whitespace_normal()
                .gap_1()
                .child(
                    h_flex()
                        .min_w_0()
                        .items_start()
                        .justify_between()
                        .gap_3()
                        .child(title)
                        .children(trailing),
                )
                .when(status_below, |tooltip| {
                    tooltip.child(secondary("status-tooltip-status", content.status.clone()))
                })
                .when_some(content.detail.clone(), |tooltip, detail| {
                    tooltip.child(secondary("status-tooltip-detail", detail))
                })
                .when_some(content.error.clone(), |tooltip, error| {
                    tooltip.child(
                        div()
                            .id("status-tooltip-error")
                            .test_support()
                            .aria_label(error.clone())
                            .child(error),
                    )
                })
        })
        .py_1()
        .build(window, cx)
    }

    /// GPUI keeps an open tooltip across trigger renders. Observe its owner
    /// so the title and status follow changes without another pointer move.
    pub(super) fn live<T: 'static>(
        owner: WeakEntity<T>,
        action: Option<&dyn Action>,
        content: impl Fn(&T, &App) -> Option<Self> + 'static,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let action = action.map(|action| Rc::new(action.boxed_clone()));
        let content = Rc::new(content);
        move |_, cx| {
            cx.new(|cx| LiveStatusTooltip {
                _subscription: owner
                    .upgrade()
                    .map(|owner| cx.observe(&owner, |_, _, cx| cx.notify())),
                owner: owner.clone(),
                action: action.clone(),
                content: content.clone(),
                shown: None,
            })
            .into()
        }
    }
}

/// GPUI flex baselines use box bottoms. Match the baselines used to paint text,
/// with each font's shaped metrics and the current snapped line height. Snap
/// the painted baselines before taking their difference, so the margin is a
/// whole device pixel and does not round independently from the glyphs.
fn first_line_baseline(
    text: &SharedString,
    refinement: &TextStyleRefinement,
    window: &mut Window,
    cx: &App,
) -> Pixels {
    let mut style = window.text_style();
    style.font_family = cx.theme().font_family.clone();
    style.refine(refinement);
    let first: SharedString = text
        .split('\n')
        .next()
        .unwrap_or_default()
        .to_owned()
        .into();
    let size = style.font_size.to_pixels(window.rem_size());
    let line =
        window
            .text_system()
            .shape_line(first.clone(), size, &[style.to_run(first.len())], None);
    let line_height =
        window.pixel_snap(style.line_height.to_pixels(size.into(), window.rem_size()));
    window.pixel_snap((line_height - line.ascent - line.descent) / 2. + line.ascent)
}

type TooltipContent<T> = Rc<dyn Fn(&T, &App) -> Option<StatusTooltip>>;

struct LiveStatusTooltip<T> {
    owner: WeakEntity<T>,
    action: Option<Rc<Box<dyn Action>>>,
    content: TooltipContent<T>,
    shown: Option<(StatusTooltip, AnyView)>,
    _subscription: Option<Subscription>,
}

impl<T: 'static> Render for LiveStatusTooltip<T> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let content = self
            .owner
            .upgrade()
            .and_then(|owner| (self.content)(owner.read(cx), cx));
        match content {
            Some(content) => {
                if self
                    .shown
                    .as_ref()
                    .is_none_or(|(shown, _)| *shown != content)
                {
                    let action = self.action.as_ref().map(|action| action.as_ref().as_ref());
                    let view = content.build(action, window, cx);
                    self.shown = Some((content, view));
                }
                div().children(self.shown.as_ref().map(|(_, view)| view.clone()))
            }
            None => {
                self.shown = None;
                div()
            }
        }
    }
}
