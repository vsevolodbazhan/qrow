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
        Tooltip::element(move |_, cx| {
            let secondary = |id, text: SharedString| {
                div()
                    .id(id)
                    .test_support()
                    .aria_label(text.clone())
                    .text_xs()
                    .text_color(cx.theme().secondary_foreground)
                    .child(text)
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
                        .items_baseline()
                        .justify_between()
                        .gap_3()
                        .child(
                            div()
                                .id("status-tooltip-title")
                                .test_support()
                                .flex_1()
                                .min_w_0()
                                .aria_label(content.title.clone())
                                .font_weight(FontWeight::MEDIUM)
                                .child(content.title.clone()),
                        )
                        .when(!status_below, |header| {
                            header.child(
                                secondary("status-tooltip-status", content.status.clone())
                                    .flex_shrink_0(),
                            )
                        })
                        .when_some(shortcut.clone(), |header, shortcut| {
                            header.child(
                                div()
                                    .id("status-tooltip-shortcut")
                                    .test_support()
                                    .flex_shrink_0()
                                    .aria_label(Kbd::format(&shortcut))
                                    .text_xs()
                                    .text_color(cx.theme().secondary_foreground)
                                    .child(Kbd::new(shortcut).appearance(false)),
                            )
                        }),
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
