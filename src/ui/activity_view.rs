//! Activity: the log of the background work of a connection, over the main
//! area of the window. The status bar button, ⇧⌘U, and the connection menu
//! open it.

use super::output::timestamp_label;
use super::{Qrow, button_pair::button_pair, panel_empty_state};
use crate::activity::{Activity, ActivityEntry, TRIMMED_TEXT};
use gpui_kit::base::SelectableText;
use gpui_kit::component::{
    ActiveTheme, Disableable as _, IconName, IndexPath, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    message_scroller::{MessageScroller, MessageScrollerState},
    select::{SearchableVec, Select, SelectEvent, SelectItem, SelectState},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::{collections::HashSet, ops::Range};
use uuid::Uuid;

actions!(qrow_activity, [CloseActivity]);

/// The key context of the open view.
pub(super) const CONTEXT: &str = "Activity";

/// A connection in the connection list of the view.
#[derive(Clone)]
struct ConnectionItem {
    id: Uuid,
    label: SharedString,
}

impl SelectItem for ConnectionItem {
    type Value = Uuid;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Uuid {
        &self.id
    }
}

type ConnectionSelect = Entity<SelectState<SearchableVec<ConnectionItem>>>;

/// One row of the list.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Row {
    /// The log removed its oldest entries.
    Trimmed,
    /// The entry at this position of the log.
    Entry(usize),
}

/// The text settings of the rows. A change measures the rows again.
#[derive(Clone, PartialEq)]
pub(super) struct Typography {
    pub family: SharedString,
    pub size: Pixels,
    pub line_height: f32,
}

pub(super) enum ActivityViewEvent {
    /// Show the query tab with this ID.
    ShowTab(Uuid),
    Closed,
}

impl EventEmitter<ActivityViewEvent> for ActivityView {}

pub(super) struct ActivityView {
    activity: Activity,
    open: bool,
    /// The connection whose log shows.
    shown: Option<Uuid>,
    errors_only: bool,
    rows: Vec<Row>,
    scroller: Entity<MessageScrollerState>,
    connection: ConnectionSelect,
    /// The connections, by name, for the connection list.
    connections: Vec<(Uuid, String)>,
    /// The closed tabs. A row of a closed tab has no Show Tab button.
    closed_tabs: HashSet<Uuid>,
    /// Whether the unseen error counts of the connection list changed.
    labels_stale: bool,
    typography: Option<Typography>,
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    _subscriptions: [Subscription; 2],
}

impl ActivityView {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let connection =
            cx.new(|cx| SelectState::new(SearchableVec::new(Vec::new()), None, window, cx));
        let subscriptions = [
            cx.subscribe_in(
                &connection,
                window,
                |this, _, event: &SelectEvent<SearchableVec<ConnectionItem>>, window, cx| {
                    if let SelectEvent::Confirm(Some(id)) = event {
                        this.show(*id, window, cx);
                    }
                },
            ),
            // The jump button follows the scroll position.
            cx.observe(&scroller, |_, _, cx| cx.notify()),
        ];
        Self {
            activity: Activity::default(),
            open: false,
            shown: None,
            errors_only: false,
            rows: Vec::new(),
            scroller,
            connection,
            connections: Vec::new(),
            closed_tabs: HashSet::new(),
            labels_stale: false,
            typography: None,
            focus: cx.focus_handle(),
            previous_focus: None,
            _subscriptions: subscriptions,
        }
    }

    pub(super) fn is_open(&self) -> bool {
        self.open
    }

    pub(super) fn activity(&self) -> &Activity {
        &self.activity
    }

    /// Add `entry` to the log of `connection`.
    pub(super) fn record(
        &mut self,
        connection: Uuid,
        entry: ActivityEntry,
        cx: &mut Context<Self>,
    ) {
        let error = entry.counts();
        let removed = self.activity.record(connection, entry);
        if self.shown != Some(connection) {
            if error {
                self.labels_stale = true;
                cx.notify();
            }
            return;
        }
        if self.open {
            self.activity.mark_seen(connection);
        }
        if removed > 0 {
            // Remove the rows of the removed entries, so the other rows keep
            // their place and the reader keeps the position.
            let header = !self.errors_only;
            let (rows, gone, added) = trim_rows(&self.rows, removed, header);
            self.rows = rows;
            self.scroller
                .update(cx, |list, cx| list.splice(gone, added, cx));
        }
        if let Some(row) = self.row_of_last() {
            self.rows.push(row);
            self.scroller.update(cx, |list, cx| list.append(1, cx));
        }
        cx.notify();
    }

    /// Forget a deleted connection.
    pub(super) fn remove(&mut self, connection: Uuid, cx: &mut Context<Self>) {
        self.activity.remove(connection);
        self.connections.retain(|(id, _)| *id != connection);
        if self.shown == Some(connection) {
            self.shown = None;
            self.rebuild_rows(cx);
        }
        cx.notify();
    }

    /// Remove the Show Tab button of the rows of a closed tab.
    pub(super) fn tab_closed(&mut self, tab: Uuid, cx: &mut Context<Self>) {
        if self.closed_tabs.insert(tab) {
            cx.notify();
        }
    }

    /// Show the new unseen error counts in the connection list.
    pub(super) fn sync_labels(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.labels_stale && self.open {
            self.sync_connections(window, cx);
        }
    }

    pub(super) fn set_typography(&mut self, typography: Typography, cx: &mut Context<Self>) {
        if self.typography.as_ref() != Some(&typography) {
            self.typography = Some(typography);
            self.scroller.update(cx, |list, cx| list.remeasure(cx));
            cx.notify();
        }
    }

    /// Open the view on the log of `connection`. `connections` are all
    /// connections, by name, for the connection list.
    pub(super) fn open(
        &mut self,
        connection: Option<Uuid>,
        connections: Vec<(Uuid, String)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.connections = connections;
        if !self.open {
            self.previous_focus = window.focused(cx);
        }
        self.open = true;
        match connection {
            Some(connection) => self.show(connection, window, cx),
            None => {
                self.shown = None;
                self.sync_connections(window, cx);
                self.rebuild_rows(cx);
            }
        }
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Close the view and give the focus back to where it was.
    pub(super) fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        self.open = false;
        if let Some(focus) = self.previous_focus.take() {
            window.focus(&focus, cx);
        }
        cx.emit(ActivityViewEvent::Closed);
        cx.notify();
    }

    /// A tab selected behind Activity receives focus when Activity closes.
    pub(super) fn return_focus_to(&mut self, focus: FocusHandle) {
        self.previous_focus = Some(focus);
    }

    fn show(&mut self, connection: Uuid, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = Some(connection);
        self.activity.mark_seen(connection);
        self.sync_connections(window, cx);
        self.rebuild_rows(cx);
        cx.notify();
    }

    /// The connection list, with the unseen errors of each connection.
    fn sync_connections(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.labels_stale = false;
        let items: Vec<_> = self
            .connections
            .iter()
            .map(|(id, name)| {
                let unseen = self.activity.unseen_errors_of(*id);
                let label = match unseen {
                    0 => name.clone(),
                    1 => format!("{name} · 1 error"),
                    count => format!("{name} · {count} errors"),
                };
                ConnectionItem {
                    id: *id,
                    label: label.into(),
                }
            })
            .collect();
        let shown = self.shown;
        self.connection.update(cx, |select, cx| {
            select.set_items(SearchableVec::new(items), window, cx);
            match shown {
                Some(id) => select.set_selected_value(&id, window, cx),
                None => select.set_selected_index(None::<IndexPath>, window, cx),
            }
        });
    }

    fn set_errors_only(&mut self, errors_only: bool, cx: &mut Context<Self>) {
        if self.errors_only != errors_only {
            self.errors_only = errors_only;
            self.rebuild_rows(cx);
            cx.notify();
        }
    }

    fn matches(&self, entry: &ActivityEntry) -> bool {
        !self.errors_only || entry.is_error()
    }

    /// The row of the newest entry of the shown log, if the filter shows it.
    fn row_of_last(&self) -> Option<Row> {
        let log = self.activity.log(self.shown?)?;
        let entry = log.entries().back()?;
        self.matches(entry)
            .then(|| Row::Entry(log.entries().len() - 1))
    }

    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let mut rows = Vec::new();
        if let Some(log) = self.shown.and_then(|id| self.activity.log(id)) {
            if log.trimmed() && !self.errors_only {
                rows.push(Row::Trimmed);
            }
            rows.extend(
                log.entries()
                    .iter()
                    .enumerate()
                    .filter(|(_, entry)| self.matches(entry))
                    .map(|(index, _)| Row::Entry(index)),
            );
        }
        self.rows = rows;
        let count = self.rows.len();
        self.scroller.update(cx, |list, cx| {
            list.reset(count, cx);
            list.scroll_to_end(cx);
        });
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.shown {
            self.activity.clear(id);
            self.rebuild_rows(cx);
            cx.notify();
        }
    }

    /// Copy the rows that the filter shows.
    fn copy_all(&self, cx: &mut App) {
        let Some(log) = self.shown.and_then(|id| self.activity.log(id)) else {
            return;
        };
        let lines: Vec<_> =
            self.rows
                .iter()
                .filter_map(|row| match row {
                    Row::Trimmed => Some(TRIMMED_TEXT.to_owned()),
                    Row::Entry(position) => log.entries().get(*position).map(|entry| {
                        format!("[{}] {}", timestamp_label(entry.timestamp), entry.text)
                    }),
                })
                .collect();
        cx.write_to_clipboard(ClipboardItem::new_string(lines.join("\n")));
    }

    /// The entries that the filter shows.
    fn entry_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| matches!(row, Row::Entry(_)))
            .count()
    }

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.entry_count() == 0;
        let log_empty = self
            .shown
            .and_then(|id| self.activity.log(id))
            .is_none_or(|log| log.is_empty());
        h_flex()
            .h_10()
            .flex_shrink_0()
            .px_3()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(div().font_weight(FontWeight::MEDIUM).child("Activity"))
            // Select fills its parent, so a box gives it its width.
            .child(
                div().w_64().flex_shrink_0().child(
                    Select::new(&self.connection)
                        .id("activity-connection")
                        .small()
                        .placeholder("No connection")
                        .accessibility_label("Connection"),
                ),
            )
            .child(button_pair(
                "activity-filter",
                Button::new("activity-all")
                    .ghost()
                    .small()
                    .label("All activity")
                    .selected(!self.errors_only)
                    .toggled(!self.errors_only)
                    .on_click(cx.listener(|this, _, _, cx| this.set_errors_only(false, cx))),
                Button::new("activity-errors")
                    .ghost()
                    .small()
                    .label("Errors only")
                    .selected(self.errors_only)
                    .toggled(self.errors_only)
                    .on_click(cx.listener(|this, _, _, cx| this.set_errors_only(true, cx))),
                cx,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(match self.entry_count() {
                        1 => "1 entry".to_owned(),
                        count => format!("{count} entries"),
                    }),
            )
            .child(div().flex_1())
            .child(
                Button::new("activity-copy-all")
                    .ghost()
                    .small()
                    .label("Copy All")
                    .disabled(empty)
                    .accessibility_label("Copy All Activity")
                    .on_click(cx.listener(|this, _, _, cx| this.copy_all(cx))),
            )
            .child(
                Button::new("activity-clear")
                    .ghost()
                    .small()
                    .label("Clear")
                    .disabled(log_empty)
                    .accessibility_label("Clear Activity")
                    .on_click(cx.listener(|this, _, _, cx| this.clear(cx))),
            )
            .child(
                Button::new("activity-close")
                    .ghost()
                    .small()
                    .icon(IconName::Close)
                    .tooltip("Close · Esc")
                    .accessibility_label("Close Activity")
                    .on_click(cx.listener(|this, _, window, cx| this.close(window, cx))),
            )
    }

    fn list(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.rows.is_empty() {
            let message = match (self.shown, self.errors_only) {
                (None, _) => "No connections",
                (Some(_), false) => "No activity yet",
                (Some(_), true) => "No errors",
            };
            return panel_empty_state(message, cx).into_any_element();
        }
        let view = cx.weak_entity();
        // GPUI Kit's MessageScroller does not register its root for tests;
        // tests find the list through this container.
        div()
            .id("activity-list")
            .test_support()
            .flex_1()
            .min_h_0()
            .child(
                MessageScroller::new(
                    "activity-rows",
                    self.scroller.clone(),
                    move |index, _, cx| {
                        let Some(view) = view.upgrade() else {
                            return div().into_any_element();
                        };
                        view.read(cx).row(index, &view, cx)
                    },
                )
                .with_jump_button_label("Jump to Latest")
                .with_jump_button_renderer(|button| button.accessibility_label("Jump to Latest"))
                .with_list_style(StyleRefinement::default().py_2())
                .with_row_style(StyleRefinement::default().px_3().pb_0()),
            )
            .into_any_element()
    }

    fn row(&self, index: usize, view: &Entity<Self>, cx: &App) -> AnyElement {
        let typography = self.typography.clone().unwrap_or(Typography {
            family: cx.theme().mono_font_family.clone(),
            size: cx.theme().font_size,
            line_height: 1.2,
        });
        let text = |el: Div| {
            el.w_full()
                .font_family(typography.family.clone())
                .text_size(typography.size)
                .line_height(relative(typography.line_height))
                .whitespace_normal()
        };
        let entry = match self.rows.get(index) {
            Some(Row::Entry(position)) => self
                .shown
                .and_then(|id| self.activity.log(id))
                .and_then(|log| log.entries().get(*position)),
            Some(Row::Trimmed) => {
                return text(div())
                    .id("activity-trimmed")
                    .child(SelectableText::new("text", TRIMMED_TEXT))
                    .into_any_element();
            }
            None => None,
        };
        let Some(entry) = entry else {
            return div().into_any_element();
        };
        let (first_line, rest) = entry
            .text
            .split_once('\n')
            .map_or((entry.text.as_str(), None), |(first, rest)| {
                (first, Some(rest))
            });
        let header = format!("[{}] {first_line}", timestamp_label(entry.timestamp));
        let order = entry.id() * 2;
        let tab = entry.tab.filter(|tab| !self.closed_tabs.contains(tab));
        let error = entry.is_error().then(|| entry.text.clone());
        h_flex()
            .id(("activity-entry", entry.id()))
            .test_support()
            .w_full()
            .items_start()
            .gap_2()
            .child(
                text(v_flex())
                    .flex_1()
                    .min_w_0()
                    .when(entry.is_error(), |el| el.text_color(cx.theme().danger))
                    .child(SelectableText::new("header", header).document_order(order))
                    .when_some(rest, |el, rest| {
                        el.child(
                            SelectableText::new("body", rest.to_owned()).document_order(order + 1),
                        )
                    }),
            )
            .when_some(error, |el, error| {
                el.child(
                    Button::new(("activity-copy", entry.id()))
                        .ghost()
                        .xsmall()
                        .label("Copy")
                        .accessibility_label("Copy Error")
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(error.clone()));
                        }),
                )
            })
            .when_some(tab, |el, tab| {
                let view = view.downgrade();
                el.child(
                    Button::new(("activity-show-tab", entry.id()))
                        .ghost()
                        .xsmall()
                        .label("Show Tab")
                        .accessibility_label("Show Tab")
                        .on_click(move |_, _, cx| {
                            let _ = view.update(cx, |_, cx| {
                                cx.emit(ActivityViewEvent::ShowTab(tab));
                            });
                        }),
                )
            })
            .into_any_element()
    }
}

/// The rows that stay after a log removed its oldest `removed` entries, with
/// the positions of the remaining entries. Also returns the range of `rows`
/// to remove and the number of rows to insert there: the header line, when
/// `header` asks for it and `rows` do not have it yet.
fn trim_rows(rows: &[Row], removed: usize, header: bool) -> (Vec<Row>, Range<usize>, usize) {
    let had_header = rows.first() == Some(&Row::Trimmed);
    let start = usize::from(had_header);
    let gone = rows[start..]
        .iter()
        .take_while(|row| matches!(row, Row::Entry(position) if *position < removed))
        .count();
    let added = usize::from(header && !had_header);
    let mut kept = Vec::with_capacity(rows.len() - gone + added);
    if had_header || added > 0 {
        kept.push(Row::Trimmed);
    }
    kept.extend(rows[start + gone..].iter().map(|row| match row {
        Row::Entry(position) => Row::Entry(position - removed),
        Row::Trimmed => Row::Trimmed,
    }));
    (kept, start..start + gone, added)
}

impl Focusable for ActivityView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for ActivityView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("activity")
            .test_support()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &CloseActivity, window, cx| this.close(window, cx)))
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .child(self.header(cx))
            .child(self.list(cx))
    }
}

impl Qrow {
    /// Open Activity, or close it when it is open. It opens on the
    /// connection with the newest unseen error, else on the connection of
    /// the active tab.
    pub(super) fn toggle_activity(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.activity.read(cx).is_open() {
            self.activity.update(cx, |view, cx| view.close(window, cx));
        } else {
            self.open_activity(None, window, cx);
        }
    }

    /// Open Activity on `connection`, or on the connection that
    /// [`Self::toggle_activity`] chooses.
    pub(super) fn open_activity(
        &mut self,
        connection: Option<Uuid>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let connection = connection
            .or_else(|| self.activity.read(cx).activity().newest_unseen())
            .or_else(|| {
                self.tabs
                    .iter()
                    .rev()
                    .find(|tab| tab.panel.unread_error)
                    .and_then(|tab| tab.worker_profile.or(tab.saved.profile))
            })
            .or_else(|| self.tabs.get(self.active).and_then(|tab| tab.saved.profile))
            .or_else(|| self.profiles.first().map(|profile| profile.id));
        let connections = self
            .profiles
            .iter()
            .map(|profile| (profile.id, profile.name.clone()))
            .collect();
        self.sync_activity_typography(cx);
        self.activity.update(cx, |view, cx| {
            view.open(connection, connections, window, cx);
        });
        cx.notify();
    }

    /// Give Activity the Logs font settings at the current UI scale. A
    /// change measures its rows again.
    pub(super) fn sync_activity_typography(&self, cx: &mut Context<Self>) {
        let typography = Typography {
            family: self.settings.logs_font_family.clone().into(),
            size: self.ui_px(self.settings.logs_font_size),
            line_height: self.settings.logs_line_height,
        };
        self.activity
            .update(cx, |view, cx| view.set_typography(typography, cx));
    }

    /// Add `entry` to the Activity of `connection`. An unseen error
    /// changes the status bar, which a closed Activity view does not redraw.
    /// Performance probes fill Activity through this.
    pub fn record_activity(&self, connection: Uuid, entry: ActivityEntry, cx: &mut Context<Self>) {
        let counts = entry.counts();
        self.activity
            .update(cx, |view, cx| view.record(connection, entry, cx));
        if counts {
            cx.notify();
        }
    }

    pub(super) fn activity_event(
        &mut self,
        event: &ActivityViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ActivityViewEvent::ShowTab(id) => {
                let Some(index) = self.tabs.iter().position(|tab| tab.saved.id == *id) else {
                    return;
                };
                self.activity.update(cx, |view, cx| view.close(window, cx));
                self.activate(index, window, cx);
            }
            ActivityViewEvent::Closed => {
                self.tabs[self.active].panel.content_visible();
                if self.assistant_transcript_visible(window, cx)
                    && let Some(thread) = self.displayed_thread()
                {
                    self.thread_run_mut(&thread).unread = None;
                }
                cx.notify();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn trimming_removes_only_the_rows_of_removed_entries() {
        use Row::{Entry, Trimmed};
        // The first trim adds the header in place of the removed rows.
        let rows = [Entry(0), Entry(1), Entry(3), Entry(4)];
        let (kept, gone, added) = trim_rows(&rows, 2, true);
        assert_eq!(kept, [Trimmed, Entry(1), Entry(2)]);
        assert_eq!((gone, added), (0..2, 1));
        // A later trim keeps the header.
        let (kept, gone, added) = trim_rows(&kept, 2, true);
        assert_eq!(kept, [Trimmed, Entry(0)]);
        assert_eq!((gone, added), (1..2, 0));
        // Errors only has no header.
        let (kept, gone, added) = trim_rows(&[Entry(5), Entry(9)], 6, false);
        assert_eq!(kept, [Entry(3)]);
        assert_eq!((gone, added), (0..1, 0));
    }
}
