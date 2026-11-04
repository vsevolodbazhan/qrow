//! One entry of the tab Logs or of Activity: time, severity, source, message,
//! and actions in fixed lanes, so that entries scan as rows. Time and SQL use
//! the Logs font; the message and the source use the interface font.

use gpui_kit::base::SelectableText;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::time::SystemTime;

use crate::logs::timestamp_label;

/// The selection order of the parts of one entry.
const PARTS: u64 = 4;

#[derive(IntoElement)]
pub(super) struct LogRow {
    id: ElementId,
    order: u64,
    time: Option<SystemTime>,
    error: bool,
    source: Option<SharedString>,
    message: SharedString,
    font_family: SharedString,
    font_size: Pixels,
    line_height: f32,
    actions: Vec<AnyElement>,
}

impl LogRow {
    /// A row with the ID `id`. `index` gives the place of the row in the
    /// text selection of the list.
    pub(super) fn new(
        id: impl Into<ElementId>,
        index: u64,
        message: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            order: index * PARTS,
            time: None,
            error: false,
            source: None,
            message: message.into(),
            font_family: SharedString::default(),
            font_size: px(13.),
            line_height: 1.2,
            actions: Vec::new(),
        }
    }

    pub(super) fn time(mut self, time: SystemTime) -> Self {
        self.time = Some(time);
        self
    }

    pub(super) fn error(mut self, error: bool) -> Self {
        self.error = error;
        self
    }

    /// The query tab or other origin of the entry, in its own lane.
    pub(super) fn source(mut self, source: Option<impl Into<SharedString>>) -> Self {
        self.source = source.map(Into::into);
        self
    }

    pub(super) fn typography(mut self, family: SharedString, size: Pixels, height: f32) -> Self {
        self.font_family = family;
        self.font_size = size;
        self.line_height = height;
        self
    }

    pub(super) fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }
}

impl RenderOnce for LogRow {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let (header, body) = match self.message.split_once('\n') {
            Some((header, body)) => (header.to_owned(), Some(body.to_owned())),
            None => (self.message.to_string(), None),
        };
        // Lanes that hold one line share the height of an extra-small button,
        // so rows with and without actions have the same height.
        let lane = || h_flex().h_5().flex_shrink_0();
        let time = self.time.map(|time| {
            let label = timestamp_label(time);
            // The date is the same for nearly all entries of a session; the
            // copied text keeps it.
            let clock = label.split_once(' ').map_or(label.clone(), |(_, t)| t.into());
            lane()
                .font_family(self.font_family.clone())
                .text_color(muted)
                .child(
                    SelectableText::new("time", format!("{clock} ")).document_order(self.order),
                )
        });
        h_flex()
            .id(self.id)
            .w_full()
            .items_start()
            .gap_2()
            .py_1()
            .border_b_1()
            .border_color(theme.border.opacity(0.5))
            // Prose uses the interface font; time and SQL use the Logs font.
            .font_family(theme.font_family.clone())
            .text_size(self.font_size)
            .line_height(relative(self.line_height))
            .children(time)
            .child(
                lane().w_4().justify_center().when(self.error, |el| {
                    el.child(
                        Icon::new(IconName::CircleX)
                            .xsmall()
                            .text_color(theme.danger),
                    )
                }),
            )
            .when_some(self.source, |el, source| {
                el.child(
                    lane()
                        .w_32()
                        .min_w_0()
                        .overflow_hidden()
                        .text_color(muted)
                        .child(div().truncate().child(source)),
                )
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .whitespace_normal()
                    .when(self.error, |el| el.text_color(theme.danger))
                    .child(
                        div().min_h_5().flex().items_center().child(
                            SelectableText::new("header", header)
                                .document_order(self.order + 1),
                        ),
                    )
                    .when_some(body, |el, body| {
                        el.child(
                            div()
                                .mt_1()
                                .mb_1()
                                .px_2()
                                .py_1()
                                .rounded(theme.radius)
                                .bg(theme.muted)
                                .font_family(self.font_family.clone())
                                .text_color(theme.foreground)
                                .child(
                                    SelectableText::new("body", body)
                                        .document_order(self.order + 2),
                                ),
                        )
                    }),
            )
            .child(lane().gap_1().children(self.actions))
    }
}
