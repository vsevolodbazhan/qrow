//! One entry of the tab Logs or of Activity: time, severity, source, message,
//! and actions in fixed lanes, so that entries scan as rows. Time and SQL use
//! the Logs font; the message and the source use the interface font.

use gpui_kit::base::{SelectableText, TextSelectionEvent, TextSelectionHandle};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::time::SystemTime;

use crate::logs::timestamp_label;

/// The selection order of the parts of one entry.
const PARTS: u64 = 4;
/// The hover group of a row, for actions that show on hover.
pub(super) const ROW_GROUP: &str = "log-row";

/// The selection of one part of a row, kept while the row is drawn. A change
/// of it draws the list again, so that a drag shows the selection while it
/// grows.
struct PartSelection {
    handle: TextSelectionHandle,
    text: SharedString,
    _changes: Subscription,
}

/// The selectable text `text` of the part `key` of the current row.
fn selectable(
    key: &'static str,
    text: impl Into<SharedString>,
    order: u64,
    window: &mut Window,
    cx: &mut App,
) -> SelectableText {
    let text = text.into();
    let state = window.use_keyed_state(key, cx, |_, cx| {
        let handle = TextSelectionHandle::new(text.clone(), cx);
        let this = cx.weak_entity();
        let changes = handle.subscribe(
            move |event, cx| {
                if matches!(event, TextSelectionEvent::SelectionChanged(_)) {
                    let _ = this.update(cx, |_, cx| cx.notify());
                }
            },
            cx,
        );
        PartSelection {
            handle,
            text: text.clone(),
            _changes: changes,
        }
    });
    let handle = state.update(cx, |state, cx| {
        if state.text != text {
            state.handle.set_fallback_copy_text(text.clone(), cx);
            state.text = text.clone();
        }
        state.handle.clone()
    });
    SelectableText::with_handle(key, handle, text).document_order(order)
}

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
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (header, body) = match self.message.split_once('\n') {
            Some((header, body)) => (header.to_owned(), Some(body.to_owned())),
            None => (self.message.to_string(), None),
        };
        let clock = self.time.map(|time| {
            let label = timestamp_label(time);
            // The date is the same for nearly all entries of a session; the
            // copied text keeps it.
            let clock = label
                .split_once(' ')
                .map_or(label.clone(), |(_, t)| t.into());
            format!("{clock} ")
        });
        let (clock, header, body) = window.with_id(self.id.clone(), |window| {
            (
                clock.map(|clock| selectable("time", clock, self.order, window, cx)),
                selectable("header", header, self.order + 1, window, cx),
                body.map(|body| selectable("body", body, self.order + 2, window, cx)),
            )
        });
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        // Lanes that hold one line share the height of an extra-small button,
        // so rows with and without actions have the same height.
        let lane = || h_flex().h_5().flex_shrink_0();
        let time = clock.map(|clock| {
            lane()
                .font_family(self.font_family.clone())
                .text_color(muted)
                .child(clock)
        });
        h_flex()
            .id(self.id)
            .group(ROW_GROUP)
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
            .child(lane().w_4().justify_center().when(self.error, |el| {
                el.child(
                    Icon::new(IconName::CircleX)
                        .xsmall()
                        .text_color(theme.danger),
                )
            }))
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
                    .child(div().min_h_5().flex().items_center().child(header))
                    .when_some(body, |el, body| {
                        el.child(
                            div()
                                .id("sql")
                                .test_support()
                                .mt_1()
                                .mb_1()
                                .px_2()
                                .py_1()
                                .rounded(theme.radius)
                                .bg(theme.muted)
                                .font_family(self.font_family.clone())
                                .text_color(theme.foreground)
                                .child(body),
                        )
                    }),
            )
            .child(lane().gap_1().children(self.actions))
    }
}
