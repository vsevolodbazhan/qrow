use gpui_kit::component::{ActiveTheme, badge::Badge, button::Button};
use gpui_kit::{AnyElement, App, IntoElement, ParentElement, Styled, div};

/// One shared priority for the dots of tabs, connections, and conversations.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum DotStatus {
    Connected,
    Ready,
    Working,
    Error,
    Attention,
}

impl DotStatus {
    fn color(self, cx: &App) -> gpui_kit::Hsla {
        let theme = cx.theme();
        match self {
            Self::Connected => theme.info.opacity(0.4),
            Self::Working => theme.info,
            Self::Ready => theme.success,
            Self::Error => theme.danger,
            Self::Attention => theme.warning,
        }
    }

    pub(super) fn dot(self, cx: &App) -> AnyElement {
        Badge::new()
            .dot()
            .color(self.color(cx))
            .child(div().size_1p5())
            .into_any_element()
    }

    pub(super) fn on_button(status: Option<Self>, button: Button, cx: &App) -> AnyElement {
        match status {
            Some(status) => Badge::new()
                .dot()
                .color(status.color(cx))
                .child(button)
                .into_any_element(),
            None => button.into_any_element(),
        }
    }
}
