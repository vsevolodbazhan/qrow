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
    metadata: Vec<(SharedString, SharedString)>,
    error: Option<SharedString>,
    key_context: Option<SharedString>,
    shortcut_focus: Option<FocusHandle>,
}

impl FluentBuilder for StatusTooltip {}

impl StatusTooltip {
    pub(super) fn new(title: impl Into<SharedString>, status: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            status: status.into(),
            detail: None,
            metadata: Vec::new(),
            error: None,
            key_context: None,
            shortcut_focus: None,
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

    pub(super) fn metadata(
        mut self,
        label: impl Into<SharedString>,
        value: impl Into<SharedString>,
    ) -> Self {
        self.metadata.push((label.into(), value.into()));
        self
    }

    /// Resolve the shortcut from the command's binding, including dialog
    /// contexts. Use the same aligned header as status tooltips.
    pub(super) fn for_action(
        mut self,
        action: &dyn Action,
        context: Option<&str>,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        self.key_context = context.map(SharedString::new);
        let action = action.boxed_clone();
        move |window, cx| self.build(Some(action.as_ref()), window, cx)
    }

    pub(super) fn for_action_in(
        mut self,
        action: &dyn Action,
        focus: FocusHandle,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        self.shortcut_focus = Some(focus);
        self.for_action(action, None)
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
                if let Some(focus) = &self.shortcut_focus {
                    return window.highest_precedence_binding_for_action_in(action, focus);
                }
                window.highest_precedence_binding_for_action_in_context(
                    action,
                    self.key_context
                        .as_deref()
                        .and_then(|context| KeyContext::parse(context).ok())
                        .unwrap_or_default(),
                )
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
            } else if !content.status.is_empty() {
                Some((
                    content.status.clone(),
                    secondary("status-tooltip-status", content.status.clone()).flex_shrink_0(),
                ))
            } else {
                None
            };
            let (title, trailing) = match trailing {
                Some((text, mut element)) => {
                    let title_center =
                        first_line_center(&content.title, &title.style().text, window, cx);
                    let trailing_center =
                        first_line_center(&text, &element.style().text, window, cx);
                    let center = title_center.max(trailing_center);
                    (
                        title.mt(center - title_center),
                        Some(element.mt(center - trailing_center)),
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
                .when(status_below && !content.status.is_empty(), |tooltip| {
                    tooltip.child(secondary("status-tooltip-status", content.status.clone()))
                })
                .when_some(content.detail.clone(), |tooltip, detail| {
                    tooltip.child(secondary("status-tooltip-detail", detail))
                })
                .when(!content.metadata.is_empty(), |tooltip| {
                    tooltip.child(
                        v_flex()
                            .id("status-tooltip-metadata")
                            .test_support()
                            .gap_1()
                            .children(content.metadata.iter().map(|(label, value)| {
                                h_flex()
                                    .id(SharedString::from(format!("status-tooltip-{label}")))
                                    .test_support()
                                    .aria_label(format!("{label}: {value}"))
                                    .items_start()
                                    .gap_3()
                                    .text_xs()
                                    .font_weight(FontWeight::NORMAL)
                                    .child(
                                        div()
                                            .w_12()
                                            .flex_shrink_0()
                                            .text_color(cx.theme().secondary_foreground)
                                            .child(label.clone()),
                                    )
                                    .child(div().min_w_0().flex_1().child(value.clone()))
                            })),
                    )
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

/// Center smaller status and shortcut text on the title's first line. Use
/// capital-letter centers, so descenders and wrapped titles do not move the
/// alignment. Snap the painted baseline and center before taking their
/// difference, keeping the correction in whole device pixels at every scale.
fn first_line_center(
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
    let baseline = window.pixel_snap((line_height - line.ascent - line.descent) / 2. + line.ascent);
    let font = window.text_system().resolve_font(&style.font());
    let cap_height = window.text_system().cap_height(font, size);
    window.pixel_snap(baseline - cap_height / 2.)
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
