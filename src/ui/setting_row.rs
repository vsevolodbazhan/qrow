use super::*;
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::component::{Icon, h_flex};

#[derive(Clone, Copy)]
pub(super) struct Rows;

impl Rows {
    pub(super) fn dialog_width(window: &Window, rems: f32) -> Pixels {
        let rem = window.rem_size();
        (rem * rems).min(window.viewport_size().width - rem * 4.)
    }
}

/// The line height of a row message, as the description above it.
const MESSAGE_LINE_REMS: f32 = 1.25;

/// The kind of a message under the label of a Settings row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Error,
    Warning,
    Info,
}

impl Tone {
    pub(super) fn color(self, cx: &App) -> Hsla {
        match self {
            Self::Error => cx.theme().danger,
            Self::Warning => cx.theme().warning,
            Self::Info => cx.theme().info,
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Error => Icon::new(IconName::CircleX),
            Self::Warning => Icon::new(AssetIconName::TriangleAlert),
            Self::Info => Icon::new(IconName::Info),
        }
    }
}

/// A message under the label of a Settings row: an icon and the text, in
/// the color of `tone`. Screen readers announce it as an alert.
pub(super) fn row_message(
    id: impl Into<SharedString>,
    tone: Tone,
    text: SharedString,
    cx: &App,
) -> impl IntoElement {
    h_flex()
        .id(id.into())
        .test_support()
        .role(Role::Alert)
        .aria_label(text.clone())
        .w_full()
        .items_start()
        .gap_1()
        .text_sm()
        .line_height(rems(MESSAGE_LINE_REMS))
        .text_color(tone.color(cx))
        .child(
            // The icon sits on the first line of a wrapped message.
            div()
                .h(rems(MESSAGE_LINE_REMS))
                .flex()
                .items_center()
                .child(tone.icon().xsmall()),
        )
        .child(div().flex_1().min_w_0().child(text))
}
