//! The dbt details sheet of a table in the schema tree: the full
//! description, the SQL, the tests, the columns, and the lineage of its dbt
//! resource.
//!
//! A large model has hundreds of columns and children, so the sheet keeps
//! its rows as owned data and shows them in a virtual list: a frame lays out
//! only the rows on screen.

use super::{Qrow, dbt::resource_label};
use crate::dbt::{
    Index, Kind, Test, contains_folded,
    worker::{ManifestState, Refresher},
};
use crate::model::DbtProject;
use gpui_kit::base::{SelectableText, StyledExt as _, TextSelectionEvent, TextSelectionHandle};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    clipboard::Clipboard,
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement as _,
    tag::Tag,
    text::{TextView, TextViewStyle},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, Context, Entity, Hsla, IntoElement, ListAlignment, ListState, Render, Role,
    SharedString, StyleRefinement, Subscription, TestSupportExt as _, Window, div, list,
    percentage, prelude::*, px, relative, rems,
};
use std::{ops::Range, sync::Arc};
use uuid::Uuid;

/// The part of the window width that the sheet takes, and its limits in
/// pixels. The sheet of GPUI Kit cannot be resized.
const SHEET_WIDTH: f32 = 0.45;
const MIN_SHEET_WIDTH: f32 = 420.;
const MAX_SHEET_WIDTH: f32 = 760.;
/// The lines of SQL in one row of the list. The SQL scrolls with the rest
/// of the sheet, and the list lays out only the rows on screen.
const SQL_CHUNK_LINES: usize = 30;
/// The line height of names and descriptions, relative to the text size.
const TEXT_LINE_HEIGHT: f32 = 1.5;
/// How far above and below the screen the list lays out rows, so that they
/// do not appear late during a scroll.
const OVERDRAW: f32 = 600.;

impl Qrow {
    /// Open the details sheet of the dbt resource `unique_id` of `profile`.
    pub(super) fn open_dbt_details(
        &mut self,
        profile: Uuid,
        unique_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (state, project) = self.dbt_details_source(profile);
        let refresher = self.dbt.refresher();
        let code_font: SharedString = self.settings.editor_font_family.clone().into();
        let unique_id = unique_id.to_owned();
        let view = cx.new(|cx| {
            DbtDetailsView::new(
                profile, unique_id, state, project, refresher, code_font, window, cx,
            )
        });
        self.dbt_details = Some(view.clone());
        let weak = cx.weak_entity();
        window.open_sheet(cx, move |sheet, window, cx| {
            let close = weak.clone();
            let title = view.read(cx).title.clone();
            sheet
                .size(
                    (window.viewport_size().width * SHEET_WIDTH)
                        .clamp(px(MIN_SHEET_WIDTH), px(MAX_SHEET_WIDTH)),
                )
                .title(title)
                .child(view.clone())
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |this, cx| {
                        this.dbt_details = None;
                        cx.notify();
                    });
                })
        });
    }

    /// The manifest state and the dbt project of `profile`.
    fn dbt_details_source(
        &self,
        profile: Uuid,
    ) -> (Option<Arc<ManifestState>>, Option<DbtProject>) {
        let project = self
            .profiles
            .iter()
            .find(|candidate| candidate.id == profile)
            .and_then(|profile| profile.dbt.clone());
        (self.dbt.state(profile).cloned(), project)
    }

    /// Give the sheet the new state of its manifest.
    pub(super) fn dbt_details_changed(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.dbt_details.clone() else {
            return;
        };
        let (state, project) = self.dbt_details_source(view.read(cx).profile);
        view.update(cx, |view, cx| view.set_source(state, project, cx));
    }
}

/// The sheet content: the rows of one dbt resource.
pub(super) struct DbtDetailsView {
    profile: Uuid,
    unique_id: String,
    state: Option<Arc<ManifestState>>,
    project: Option<DbtProject>,
    refresher: Refresher,
    code_font: SharedString,
    title: SharedString,
    /// The data of the resource, or `None` when the manifest does not have
    /// it.
    data: Option<Data>,
    filter: Entity<InputState>,
    /// The positions of the columns that match the filter.
    matching: Vec<usize>,
    rows: Vec<Row>,
    /// The list measures all rows, so that the scrollbar has the right
    /// size. A large model takes one long frame to measure.
    list: ListState,
    /// Whether the facts tell that the manifest changed since the read.
    shown_changed: bool,
    /// The rem size of the last frame. Row heights follow it.
    rem_size: Option<gpui_kit::Pixels>,
    /// The compiled SQL part, then the raw SQL part.
    sql: [SqlPart; 2],
    /// The unique IDs and the titles of the resources that the user came
    /// from through the lineage, the last one on top.
    history: Vec<(String, SharedString)>,
    _subscription: gpui_kit::Subscription,
}

/// The data of a resource that the rows show, read from the index once.
struct Data {
    facts: Vec<(&'static str, SharedString)>,
    description: Option<SharedString>,
    /// Whether the manifest has the compiled SQL and the raw SQL of the
    /// resource. `None` for a resource without SQL, like a source.
    sql: Option<SqlKinds>,
    tests: Vec<TestView>,
    columns: Vec<ColumnView>,
    parents: Vec<Linked>,
    children: Vec<Linked>,
}

struct ColumnView {
    name: SharedString,
    data_type: Option<SharedString>,
    description: SharedString,
    tests: Vec<TestView>,
}

/// A parent or a child: its table or unique ID, its materialization or
/// kind, and the unique ID that opens its details.
struct Linked {
    name: SharedString,
    label: SharedString,
    unique_id: String,
}

/// A test, with its arguments as names and values.
#[derive(Clone, Debug, PartialEq)]
struct TestView {
    name: SharedString,
    arguments: Vec<(SharedString, SharedString)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Row {
    /// The button back to the resource that the user came from.
    Back,
    Missing,
    Facts,
    Description,
    Tests,
    ColumnsTitle,
    Column(usize),
    NoColumns,
    ParentsTitle,
    Parent(usize),
    ChildrenTitle,
    Child(usize),
    Sql(SqlKind),
    SqlChunk(SqlKind, usize),
}

/// The two SQL parts of a resource.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SqlKind {
    Compiled,
    Raw,
}

impl SqlKind {
    const ALL: [SqlKind; 2] = [SqlKind::Compiled, SqlKind::Raw];

    fn title(self) -> &'static str {
        match self {
            SqlKind::Compiled => "Compiled SQL",
            SqlKind::Raw => "Raw SQL",
        }
    }

    /// The part of the element IDs of the part.
    fn id(self) -> &'static str {
        match self {
            SqlKind::Compiled => "compiled",
            SqlKind::Raw => "raw",
        }
    }

    fn position(self) -> usize {
        self as usize
    }

    /// What the part shows, or why the manifest does not have it.
    fn note(self, available: bool) -> &'static str {
        match (self, available) {
            (SqlKind::Compiled, true) => "The SQL that dbt compiled, without Jinja.",
            (SqlKind::Compiled, false) => {
                "The manifest has no compiled SQL. dbt adds it when it compiles the project, \
                 for example with dbt compile or dbt build, but not with dbt parse."
            }
            (SqlKind::Raw, true) => "The SQL of the model file, with Jinja.",
            (SqlKind::Raw, false) => "The manifest has no raw SQL of this resource.",
        }
    }
}

/// Which SQL the manifest has for a resource.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SqlKinds {
    compiled: bool,
    raw: bool,
}

impl SqlKinds {
    fn has(self, kind: SqlKind) -> bool {
        match kind {
            SqlKind::Compiled => self.compiled,
            SqlKind::Raw => self.raw,
        }
    }
}

/// One SQL part: whether it is open, and its SQL.
#[derive(Default)]
struct SqlPart {
    open: bool,
    sql: Sql,
    /// Counts the reads of the SQL, so that the result of an old read is
    /// dropped.
    read: u64,
}

impl SqlPart {
    /// Forget the SQL, because it belongs to an old index or another
    /// resource. A read in progress ends without a result.
    fn reset(&mut self) {
        self.read += 1;
        self.sql = Sql::Unread;
    }
}

/// The SQL of the resource. The sheet reads it from the manifest when the
/// user opens the SQL part.
#[derive(Default)]
enum Sql {
    #[default]
    Unread,
    Reading,
    /// The manifest changed after Qrow read it, so the positions of the SQL
    /// are wrong. Qrow reads the manifest again first.
    Refreshing,
    Read(SqlText),
    Failed(String),
}

/// SQL that the sheet read, in rows of [`SQL_CHUNK_LINES`] lines. Each row
/// is a selectable run in reading order, so a selection can go across them.
struct SqlText {
    full: SharedString,
    chunks: Vec<SharedString>,
    /// The selection of each row. A change of a selection draws the sheet
    /// again, so that a drag shows the selection while it grows.
    selections: Vec<TextSelectionHandle>,
    _selection_changes: Vec<Subscription>,
}

impl SqlText {
    fn new(sql: String, cx: &mut Context<DbtDetailsView>) -> Self {
        let lines: Vec<&str> = sql.lines().collect();
        let chunks: Vec<SharedString> = lines
            .chunks(SQL_CHUNK_LINES)
            .map(|chunk| SharedString::from(chunk.join("\n")))
            .collect();
        let selections: Vec<TextSelectionHandle> = chunks
            .iter()
            .map(|chunk| TextSelectionHandle::new(chunk.clone(), cx))
            .collect();
        let view = cx.weak_entity();
        let selection_changes = selections
            .iter()
            .map(|selection| {
                let view = view.clone();
                selection.subscribe(
                    move |event, cx| {
                        if matches!(event, TextSelectionEvent::SelectionChanged(_)) {
                            let _ = view.update(cx, |_, cx| cx.notify());
                        }
                    },
                    cx,
                )
            })
            .collect();
        Self {
            full: sql.into(),
            chunks,
            selections,
            _selection_changes: selection_changes,
        }
    }
}

/// The result of a read of the SQL in the background.
enum SqlRead {
    Read(String),
    Changed,
    Failed(String),
}

impl DbtDetailsView {
    #[allow(clippy::too_many_arguments)]
    fn new(
        profile: Uuid,
        unique_id: String,
        state: Option<Arc<ManifestState>>,
        project: Option<DbtProject>,
        refresher: Refresher,
        code_font: SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter columns"));
        let subscription = cx.subscribe(&filter, |this, _, _: &InputEvent, cx| {
            this.filter_changed(cx)
        });
        let mut view = Self {
            profile,
            title: unique_id.clone().into(),
            unique_id,
            state,
            project,
            refresher,
            code_font,
            data: None,
            filter,
            matching: Vec::new(),
            rows: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(OVERDRAW)).measure_all(),
            shown_changed: false,
            rem_size: None,
            sql: Default::default(),
            history: Vec::new(),
            _subscription: subscription,
        };
        view.rebuild(cx);
        view
    }

    /// Read the data of the resource from the index again, and show it from
    /// the top.
    fn rebuild(&mut self, cx: &App) {
        self.data = match (&self.state, &self.project) {
            (Some(state), Some(project)) => data(state, project, &self.unique_id),
            _ => None,
        };
        self.title = self
            .state
            .as_ref()
            .and_then(|state| {
                let index = state.index.as_ref()?;
                Some(index.entry(index.find(&self.unique_id)?).name.to_string())
            })
            .unwrap_or_else(|| self.unique_id.clone())
            .into();
        self.shown_changed = self.manifest_changed();
        self.matching = self.matching_columns(cx);
        self.rows = self.make_rows();
        self.list.reset(self.rows.len());
        self.keep_filter_rendered(cx);
    }

    fn matching_columns(&self, cx: &App) -> Vec<usize> {
        let query = self.filter.read(cx).value().trim().to_owned();
        let Some(data) = &self.data else {
            return Vec::new();
        };
        data.columns
            .iter()
            .enumerate()
            .filter(|(_, column)| {
                query.is_empty()
                    || contains_folded(&column.name, &query)
                    || contains_folded(&column.description, &query)
            })
            .map(|(position, _)| position)
            .collect()
    }

    fn make_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if !self.history.is_empty() {
            rows.push(Row::Back);
        }
        let Some(data) = &self.data else {
            rows.push(Row::Missing);
            return rows;
        };
        rows.extend([Row::Facts, Row::Description]);
        if data.sql.is_some() {
            for kind in SqlKind::ALL {
                rows.push(Row::Sql(kind));
                let part = &self.sql[kind.position()];
                if let (true, Sql::Read(sql)) = (part.open, &part.sql) {
                    rows.extend((0..sql.chunks.len()).map(|chunk| Row::SqlChunk(kind, chunk)));
                }
            }
        }
        rows.extend([Row::Tests, Row::ColumnsTitle]);
        if self.matching.is_empty() {
            rows.push(Row::NoColumns);
        } else {
            rows.extend(self.matching.iter().copied().map(Row::Column));
        }
        // The lineage is below the columns: the columns are read more often.
        rows.push(Row::ParentsTitle);
        rows.extend((0..data.parents.len()).map(Row::Parent));
        rows.push(Row::ChildrenTitle);
        rows.extend((0..data.children.len()).map(Row::Child));
        rows
    }

    /// The positions of the column rows in `rows`.
    fn column_rows(rows: &[Row]) -> Range<usize> {
        let start = rows
            .iter()
            .position(|row| matches!(row, Row::Column(_) | Row::NoColumns))
            .unwrap_or(rows.len());
        let end = rows[start..]
            .iter()
            .position(|row| !matches!(row, Row::Column(_) | Row::NoColumns))
            .map_or(rows.len(), |length| start + length);
        start..end
    }

    /// Show the columns that match the new filter. Only the column rows
    /// change, so the list keeps its place.
    fn filter_changed(&mut self, cx: &mut Context<Self>) {
        let matching = self.matching_columns(cx);
        if matching == self.matching {
            return;
        }
        self.matching = matching;
        let old = Self::column_rows(&self.rows);
        self.rows = self.make_rows();
        let new = Self::column_rows(&self.rows);
        self.list.splice(old, new.len());
        // A splice leaves the new rows unmeasured, and the scrollbar counts
        // them with no height.
        self.list.remeasure_items(new);
        cx.notify();
    }

    /// Take a new state of the manifest or a new dbt project.
    fn set_source(
        &mut self,
        state: Option<Arc<ManifestState>>,
        project: Option<DbtProject>,
        cx: &mut Context<Self>,
    ) {
        let index = |state: &Option<Arc<ManifestState>>| {
            state.as_ref().and_then(|state| state.index.clone())
        };
        let new_index = match (index(&self.state), index(&state)) {
            (Some(old), Some(new)) => !Arc::ptr_eq(&old, &new),
            (None, None) => false,
            _ => true,
        } || project != self.project;
        self.state = state;
        self.project = project;
        if new_index {
            // The SQL of the old index is old.
            for part in &mut self.sql {
                part.reset();
            }
            self.rebuild(cx);
            for kind in SqlKind::ALL {
                if self.sql[kind.position()].open {
                    self.read_sql(kind, cx);
                }
            }
            cx.notify();
            return;
        }
        for kind in SqlKind::ALL {
            if matches!(self.sql[kind.position()].sql, Sql::Refreshing)
                && let Some(state) = self.state.clone()
                && !state.parsing
            {
                if state.is_current() {
                    self.read_sql(kind, cx);
                } else if let Some(error) = &state.error {
                    self.sql[kind.position()].sql = Sql::Failed(error.to_string());
                    self.sql_changed(kind);
                }
            }
        }
        // The facts tell whether the manifest changed since the read. A
        // remeasure measures all rows again, so it happens only on a change.
        let changed = self.manifest_changed();
        if changed != self.shown_changed {
            self.shown_changed = changed;
            if let Some(facts) = self.rows.iter().position(|row| *row == Row::Facts) {
                self.list.remeasure_items(facts..facts + 1);
            }
        }
        cx.notify();
    }

    /// Give the list the focus handle of the filter, so that the list
    /// keeps the filter row and its keyboard input when the user scrolls
    /// it out of view.
    fn keep_filter_rendered(&self, cx: &App) {
        if let Some(title) = self.rows.iter().position(|row| *row == Row::ColumnsTitle) {
            let focus = gpui_kit::Focusable::focus_handle(self.filter.read(cx), cx);
            self.list.splice_focusable(title..title + 1, [Some(focus)]);
        }
    }

    /// Whether the manifest file changed after Qrow read it.
    fn manifest_changed(&self) -> bool {
        self.state.as_ref().is_some_and(|state| !state.is_current())
    }

    /// Show the details of `unique_id` from the top, as the sheet shows a
    /// resource when it opens. `back` tells that the user goes back to it.
    fn show(&mut self, unique_id: String, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !back {
            let current = (self.unique_id.clone(), self.title.clone());
            self.history.push(current);
        }
        self.unique_id = unique_id;
        // The SQL and the column filter belong to the old resource.
        for part in &mut self.sql {
            part.reset();
            part.open = false;
        }
        self.filter
            .update(cx, |filter, cx| filter.set_value("", window, cx));
        self.rebuild(cx);
        cx.notify();
    }

    /// The button back to the resource that the user came from.
    fn back_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some((_, title)) = self.history.last() else {
            return div().into_any_element();
        };
        h_flex()
            .pb_2()
            .child(
                Button::new("dbt-details-back")
                    .ghost()
                    .small()
                    .icon(IconName::ChevronLeft)
                    .label(format!("Back to {title}"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some((unique_id, _)) = this.history.pop() {
                            this.show(unique_id, true, window, cx);
                        }
                    })),
            )
            .into_any_element()
    }

    /// Open or close the SQL part `kind`.
    fn toggle_sql(&mut self, kind: SqlKind, cx: &mut Context<Self>) {
        let current = self.state.as_ref().is_some_and(|state| state.is_current());
        let part = &mut self.sql[kind.position()];
        part.open = !part.open;
        // A manifest that changed since the read has other SQL, and a failed
        // read can work now.
        let stale = match &part.sql {
            Sql::Unread | Sql::Failed(_) => true,
            Sql::Read(_) => !current,
            Sql::Reading | Sql::Refreshing => false,
        };
        if part.open && stale {
            self.read_sql(kind, cx);
        }
        self.sql_changed(kind);
        cx.notify();
    }

    /// The SQL part `kind` changed: its header has a new height, and the
    /// rows of SQL came or went.
    fn sql_changed(&mut self, kind: SqlKind) {
        let Some(sql) = self.rows.iter().position(|row| *row == Row::Sql(kind)) else {
            return;
        };
        let chunks = |rows: &[Row]| {
            rows[sql + 1..]
                .iter()
                .take_while(|row| matches!(row, Row::SqlChunk(..)))
                .count()
        };
        let old = chunks(&self.rows);
        self.rows = self.make_rows();
        let new = chunks(&self.rows);
        self.list.splice(sql + 1..sql + 1 + old, new);
        self.list.remeasure_items(sql..sql + 1 + new);
    }

    /// Read the SQL of the part `kind` in the background.
    fn read_sql(&mut self, kind: SqlKind, cx: &mut Context<Self>) {
        let Some(state) = self.state.clone() else {
            return;
        };
        let span = state.index.as_ref().and_then(|index| {
            let entry = index.entry(index.find(&self.unique_id)?);
            match kind {
                SqlKind::Compiled => entry.compiled_code,
                SqlKind::Raw => entry.raw_code,
            }
        });
        // The part tells that the manifest has no such SQL.
        let Some(span) = span else {
            return;
        };
        let part = &mut self.sql[kind.position()];
        part.read += 1;
        if !state.is_current() {
            self.refresher.refresh(&state.path);
            part.sql = Sql::Refreshing;
            self.sql_changed(kind);
            return;
        }
        part.sql = Sql::Reading;
        let read = part.read;
        self.sql_changed(kind);
        // Compiled SQL can be large, and the file can be on a slow disk.
        let task = cx.background_executor().spawn(async move {
            let result = crate::dbt::read_sql(&state.path, span);
            // dbt can write the file during the read, which can also make
            // the read fail.
            if !state.is_current() {
                return SqlRead::Changed;
            }
            match result {
                Ok(sql) => SqlRead::Read(sql),
                Err(error) => SqlRead::Failed(error.to_string()),
            }
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.sql[kind.position()].read != read {
                    return;
                }
                match result {
                    SqlRead::Changed => this.read_sql(kind, cx),
                    SqlRead::Read(sql) => {
                        let text = SqlText::new(sql, cx);
                        this.sql[kind.position()].sql = Sql::Read(text);
                    }
                    SqlRead::Failed(error) => this.sql[kind.position()].sql = Sql::Failed(error),
                }
                this.sql_changed(kind);
                cx.notify();
            });
        })
        .detach();
    }

    fn render_row(&mut self, position: usize, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let Some(row) = self.rows.get(position).copied() else {
            return div().into_any_element();
        };
        if row == Row::Back {
            return self.back_row(cx);
        }
        let Some(data) = &self.data else {
            let text = format!("The dbt manifest does not have {} now.", self.unique_id);
            return div()
                .id("dbt-details")
                .test_support()
                .role(Role::Label)
                .aria_label(text.clone())
                .text_sm()
                .text_color(muted)
                .child(text)
                .into_any_element();
        };
        match row {
            Row::Missing | Row::Back => div().into_any_element(),
            Row::Facts => {
                let changed = self.shown_changed;
                v_flex()
                    .id("dbt-details")
                    .test_support()
                    .gap_1()
                    .children(data.facts.iter().map(|(label, value)| {
                        let value = if *label == "Manifest" && changed {
                            SharedString::from(format!("{value} (changed since the last read)"))
                        } else {
                            value.clone()
                        };
                        h_flex()
                            .gap_3()
                            .items_start()
                            .text_sm()
                            .child(
                                div()
                                    .w(px(120.))
                                    .flex_shrink_0()
                                    .text_color(muted)
                                    .child(*label),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("dbt-details-{label}")))
                                    .test_support()
                                    .role(Role::Label)
                                    .aria_label(value.clone())
                                    .flex_1()
                                    .min_w_0()
                                    .child(value),
                            )
                    }))
                    .into_any_element()
            }
            Row::Description => {
                let content = match &data.description {
                    None => muted_text("No description.", muted),
                    Some(description) => div()
                        .id("dbt-details-description")
                        .test_support()
                        .role(Role::Label)
                        .aria_label(description.clone())
                        .child(markdown(
                            format!("dbt-description-{}", self.unique_id),
                            description.clone(),
                        ))
                        .into_any_element(),
                };
                section("Description", content).into_any_element()
            }
            Row::Sql(kind) => match data.sql {
                Some(kinds) => self.sql_part(kind, kinds.has(kind), cx),
                None => div().into_any_element(),
            },
            Row::SqlChunk(kind, position) => self.sql_chunk(kind, position, cx),
            Row::Tests => {
                let content = if data.tests.is_empty() {
                    muted_text("No tests of the table.", muted)
                } else {
                    tests_element("dbt-details-tests", &data.tests, &self.code_font, muted)
                };
                section("Table Tests", content).into_any_element()
            }
            Row::ColumnsTitle => section(
                &format!("Columns ({})", data.columns.len()),
                Input::new(&self.filter)
                    .id("dbt-details-filter")
                    .small()
                    .cleanable(true)
                    .aria_label("Filter columns")
                    .into_any_element(),
            )
            .into_any_element(),
            Row::NoColumns => div()
                .pt_3()
                .child(muted_text(
                    if data.columns.is_empty() {
                        "The dbt project does not document the columns."
                    } else {
                        "No columns match the filter."
                    },
                    muted,
                ))
                .into_any_element(),
            Row::Column(position) => {
                let column = &data.columns[position];
                let id = format!("dbt-details-column-{}", column.name);
                v_flex()
                    .id(SharedString::from(id.clone()))
                    .test_support()
                    .role(Role::Label)
                    .aria_label(column.name.clone())
                    .pt_3()
                    .gap_1()
                    .text_sm()
                    // The name and the description share a line height, so
                    // the space above and below the description is even.
                    .line_height(relative(TEXT_LINE_HEIGHT))
                    .child(
                        h_flex()
                            .id(SharedString::from(format!("{id}-header")))
                            .test_support()
                            .gap_2()
                            .child(div().font_semibold().child(column.name.clone()))
                            .when_some(column.data_type.clone(), |row, data_type| {
                                row.child(div().text_color(muted).child(data_type))
                            }),
                    )
                    .when(!column.description.is_empty(), |row| {
                        row.child(
                            div()
                                .id(SharedString::from(format!("{id}-text")))
                                .test_support()
                                .child(markdown(
                                    format!("{id}-description"),
                                    column.description.clone(),
                                )),
                        )
                    })
                    .when(!column.tests.is_empty(), |row| {
                        // A tag has no leading above it, so it moves down by
                        // the leading under the text above it.
                        row.child(div().mt_1().child(tests_element(
                            &format!("{id}-tests"),
                            &column.tests,
                            &self.code_font,
                            muted,
                        )))
                    })
                    .into_any_element()
            }
            Row::ParentsTitle => lineage_title("Parents", data.parents.len(), muted),
            Row::ChildrenTitle => lineage_title("Children", data.children.len(), muted),
            Row::Parent(position) => linked_row(
                ("dbt-details-parent", position),
                &data.parents[position],
                muted,
                cx,
            ),
            Row::Child(position) => linked_row(
                ("dbt-details-child", position),
                &data.children[position],
                muted,
                cx,
            ),
        }
    }

    /// A SQL part: a header that opens it, then the SQL with a copy button.
    /// `available` tells whether the manifest has this SQL; when it does
    /// not, the part tells why.
    fn sql_part(&self, kind: SqlKind, available: bool, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let part = &self.sql[kind.position()];
        let open = part.open;
        let title = kind.title();
        let header = Button::new(SharedString::from(format!(
            "dbt-details-{}-sql-toggle",
            kind.id()
        )))
        .ghost()
        .small()
        .w_full()
        .justify_start()
        // The chevron lines up with the titles of the other parts.
        .px_0()
        .accessibility_label(title)
        .child(
            h_flex()
                .w_full()
                .gap_1p5()
                .child(
                    Icon::new(IconName::ChevronRight)
                        .xsmall()
                        .text_color(muted)
                        .rotate(percentage(if open { 0.25 } else { 0. })),
                )
                .child(div().text_sm().font_semibold().child(title)),
        )
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_sql(kind, cx)));
        let body = open.then(|| match &part.sql {
            _ if !available => div()
                .id(SharedString::from(format!(
                    "dbt-details-{}-sql-note",
                    kind.id()
                )))
                .test_support()
                .role(Role::Label)
                .aria_label(kind.note(false))
                .text_xs()
                .text_color(muted)
                .child(kind.note(false))
                .into_any_element(),
            Sql::Unread | Sql::Reading => muted_text("Reading the SQL…", muted),
            Sql::Refreshing => muted_text(
                "The dbt manifest changed. Qrow reads it again, then shows the SQL.",
                muted,
            ),
            Sql::Failed(error) => muted_text(&format!("Could not read the SQL: {error}"), muted),
            Sql::Read(sql) => h_flex()
                .id(SharedString::from(format!("dbt-details-{}-sql", kind.id())))
                .test_support()
                .role(Role::Label)
                .aria_label(sql.full.clone())
                .gap_2()
                .text_xs()
                .text_color(muted)
                .child(div().flex_1().child(kind.note(true)))
                .child(
                    Clipboard::new(SharedString::from(format!(
                        "dbt-details-copy-{}-sql",
                        kind.id()
                    )))
                    .value(sql.full.clone())
                    .tooltip("Copy SQL"),
                )
                .into_any_element(),
        });
        v_flex()
            .pt_4()
            .gap_2()
            .child(header)
            .children(body)
            .into_any_element()
    }

    /// The rows of SQL form one box. They scroll with the sheet instead of
    /// in a box of their own.
    fn sql_chunk(&self, kind: SqlKind, position: usize, cx: &mut Context<Self>) -> AnyElement {
        let Sql::Read(sql) = &self.sql[kind.position()].sql else {
            return div().into_any_element();
        };
        let (Some(text), Some(selection)) =
            (sql.chunks.get(position), sql.selections.get(position))
        else {
            return div().into_any_element();
        };
        let last = position + 1 == sql.chunks.len();
        div()
            .w_full()
            .px_2()
            .bg(cx.theme().secondary)
            .border_color(cx.theme().border)
            .border_x_1()
            .when(position == 0, |chunk| {
                chunk.mt_1().pt_2().border_t_1().rounded_t_md()
            })
            .when(last, |chunk| chunk.pb_2().border_b_1().rounded_b_md())
            .font_family(self.code_font.clone())
            .text_xs()
            .child(
                SelectableText::with_handle(
                    SharedString::from(format!("dbt-sql-{}-{position}", kind.id())),
                    selection.clone(),
                    text.clone(),
                )
                // The compiled SQL comes before the raw SQL.
                .document_order(((kind.position() as u64) << 32) + position as u64),
            )
            .into_any_element()
    }
}

impl Render for DbtDetailsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The interface scale changes the rem size but not the width, and
        // the list measures rows again only on a new width.
        let rem_size = window.rem_size();
        if self
            .rem_size
            .replace(rem_size)
            .is_some_and(|old| old != rem_size)
        {
            self.list.remeasure();
        }
        let rows = list(
            self.list.clone(),
            // GPUI's list lays a row out at the width of its content, so
            // each row takes the full width. The padding keeps the row out
            // from under the scrollbar.
            cx.processor(|this, position, _, cx| {
                div()
                    .w_full()
                    .min_w_0()
                    .pr_3()
                    .child(this.render_row(position, cx))
                    .into_any_element()
            }),
        )
        .size_full()
        .pb_4();
        // The sheet body scrolls its children, so the list takes the height
        // of the body and scrolls by itself.
        div()
            .id("dbt-details-list")
            .size_full()
            .child(rows)
            .vertical_scrollbar(&self.list)
    }
}

/// The data of `unique_id` in the index of `state`.
fn data(state: &ManifestState, project: &DbtProject, unique_id: &str) -> Option<Data> {
    let index = state.index.as_ref()?;
    let position = index.find(unique_id)?;
    let project =
        crate::assistant::dbt::Project::new(index, project, state.refreshed, !state.is_current());
    let entry = index.entry(position);
    let tests = index.tests(position);
    let shared = |text: &str| SharedString::from(text.to_owned());

    let mut facts = vec![("Resource", shared(entry.kind.name()))];
    if let Some(materialized) = entry.materialized {
        facts.push(("Materialization", shared(index.symbol(materialized))));
    }
    if let Some(relation) = project.relation(entry) {
        facts.push(("Relation", relation.into()));
    }
    facts.push(("Unique ID", shared(&entry.unique_id)));
    if !entry.tags.is_empty() {
        let tags: Vec<&str> = entry.tags.iter().map(|tag| index.symbol(*tag)).collect();
        facts.push(("Tags", tags.join(", ").into()));
    }
    facts.push(("Manifest", shared(&index.generated_at)));

    let description = entry.description.trim();
    let sql = match (entry.kind, entry.compiled_code, entry.raw_code) {
        (Kind::Source, _, _) | (_, None, None) => None,
        (_, compiled, raw) => Some(SqlKinds {
            compiled: compiled.is_some(),
            raw: raw.is_some(),
        }),
    };
    let view = |test: &Test| test_view(index, &project, test);
    let columns = entry
        .columns
        .iter()
        .map(|column| ColumnView {
            name: shared(&column.name),
            data_type: column.data_type.map(|symbol| shared(index.symbol(symbol))),
            description: shared(column.description.trim()),
            tests: tests
                .iter()
                .filter(|test| {
                    test.column
                        .as_deref()
                        .is_some_and(|name| name.eq_ignore_ascii_case(&column.name))
                })
                .map(view)
                .collect(),
        })
        .collect();
    let linked = |positions: &[u32]| -> Vec<Linked> {
        positions
            .iter()
            .map(|position| {
                let entry = index.entry(*position);
                Linked {
                    name: project
                        .relation(entry)
                        .unwrap_or_else(|| entry.unique_id.to_string())
                        .into(),
                    label: shared(resource_label(index, entry)),
                    unique_id: entry.unique_id.to_string(),
                }
            })
            .collect()
    };
    Some(Data {
        facts,
        description: (!description.is_empty()).then(|| shared(description)),
        sql,
        tests: tests
            .iter()
            .filter(|test| test.column.is_none())
            .map(view)
            .collect(),
        columns,
        parents: linked(&entry.parents),
        children: linked(index.children(position)),
    })
}

/// A titled part of the sheet.
fn section(title: &str, content: AnyElement) -> impl IntoElement {
    v_flex()
        .pt_5()
        .gap_2()
        .child(div().text_sm().font_semibold().child(title.to_owned()))
        .child(content)
}

fn lineage_title(title: &str, count: usize, muted: Hsla) -> AnyElement {
    v_flex()
        .pt_5()
        .gap_2()
        .child(
            div()
                .text_sm()
                .font_semibold()
                .child(format!("{title} ({count})")),
        )
        .when(count == 0, |part| part.child(muted_text("None.", muted)))
        .into_any_element()
}

/// A parent or a child. A click shows its details in the sheet.
fn linked_row(
    id: impl Into<gpui_kit::ElementId>,
    linked: &Linked,
    muted: Hsla,
    cx: &mut Context<DbtDetailsView>,
) -> AnyElement {
    let unique_id = linked.unique_id.clone();
    Button::new(id)
        .ghost()
        .small()
        .w_full()
        .justify_start()
        .accessibility_label(format!("{}, {}", linked.name, linked.label))
        .child(
            h_flex()
                .w_full()
                .min_w_0()
                .gap_2()
                .text_sm()
                .child(div().min_w_0().truncate().child(linked.name.clone()))
                .child(div().text_color(muted).child(linked.label.clone())),
        )
        .on_click(
            cx.listener(move |this, _, window, cx| this.show(unique_id.clone(), false, window, cx)),
        )
        .into_any_element()
}

fn muted_text(text: &str, color: Hsla) -> AnyElement {
    div()
        .text_sm()
        .text_color(color)
        .child(text.to_owned())
        .into_any_element()
}

/// A dbt description as Markdown. Its headings stay below the titles of the
/// sheet, and its code blocks have the size of the SQL part instead of the
/// larger code size of the theme.
fn markdown(id: String, text: SharedString) -> TextView {
    TextView::markdown(SharedString::from(id), text)
        .style(
            TextViewStyle::default()
                .paragraph_gap(rems(0.5))
                .heading_font_size(|_, base| base)
                .code_block(StyleRefinement::default().text_xs().p_2()),
        )
        .selectable(true)
        .text_sm()
        .line_height(relative(TEXT_LINE_HEIGHT))
        .min_w_0()
        .max_w_full()
}

fn test_view(index: &Index, project: &crate::assistant::dbt::Project, test: &Test) -> TestView {
    let mut arguments: Vec<(SharedString, SharedString)> = Vec::new();
    if !test.values.is_empty() {
        arguments.push(("values".into(), test.values.join(", ").into()));
    }
    if let Some(text) = &test.to_text {
        let target = test
            .to
            .map(|to| {
                let entry = index.entry(to);
                project
                    .relation(entry)
                    .unwrap_or_else(|| entry.unique_id.to_string())
            })
            .unwrap_or_else(|| text.to_string());
        arguments.push(("to".into(), target.into()));
        if let Some(field) = &test.field {
            arguments.push(("field".into(), field.to_string().into()));
        }
    }
    if let Some(text) = &test.arguments {
        arguments.extend(
            argument_pairs(text)
                .into_iter()
                .map(|(name, value)| (name.into(), value.into())),
        );
    }
    TestView {
        name: index.symbol(test.name).to_owned().into(),
        arguments,
    }
}

/// The arguments of a test from their JSON, in the order of the manifest: a
/// text value without quotes and escapes, and another value as compact JSON.
fn argument_pairs(json: &str) -> Vec<(String, String)> {
    match serde_json::from_str::<serde_json::Value>(json) {
        Ok(serde_json::Value::Object(map)) => map
            .into_iter()
            .map(|(name, value)| {
                let value = match value {
                    serde_json::Value::String(text) => text,
                    value => value.to_string(),
                };
                (name, value)
            })
            .collect(),
        _ => vec![("arguments".to_owned(), json.to_owned())],
    }
}

/// Tests as tags. A test with arguments shows them under its tag, with the
/// values in the code font.
fn tests_element(
    id: &str,
    tests: &[TestView],
    code_font: &SharedString,
    muted: Hsla,
) -> AnyElement {
    let simple = tests.iter().filter(|test| test.arguments.is_empty());
    let detailed = tests.iter().filter(|test| !test.arguments.is_empty());
    v_flex()
        .id(SharedString::from(id.to_owned()))
        .test_support()
        .gap_2()
        .when(simple.clone().next().is_some(), |list| {
            list.child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .children(simple.map(|test| test_tag(test.name.clone()))),
            )
        })
        .children(detailed.map(|test| {
            v_flex()
                .gap_1()
                .child(h_flex().child(test_tag(test.name.clone())))
                // The arguments of one test read as one block.
                .child(
                    v_flex()
                        .pl_2()
                        .text_xs()
                        .line_height(relative(1.4))
                        .children(test.arguments.iter().map(|(name, value)| {
                            h_flex()
                                .items_start()
                                .gap_2()
                                .child(div().flex_shrink_0().text_color(muted).child(name.clone()))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .font_family(code_font.clone())
                                        .child(value.clone()),
                                )
                        })),
                )
        }))
        .into_any_element()
}

fn test_tag(name: SharedString) -> impl IntoElement {
    Tag::secondary().small().child(name)
}

#[cfg(test)]
mod tests {
    use super::argument_pairs;

    #[test]
    fn test_arguments_are_names_and_plain_values() {
        let pairs = argument_pairs(
            r#"{"filter":"{{ data_interval_filter(date_column=\"pdate\") }}","min":1,"cols":["a","b"]}"#,
        );
        assert_eq!(
            pairs,
            vec![
                (
                    "filter".to_owned(),
                    r#"{{ data_interval_filter(date_column="pdate") }}"#.to_owned()
                ),
                ("min".to_owned(), "1".to_owned()),
                ("cols".to_owned(), r#"["a","b"]"#.to_owned()),
            ]
        );
        assert_eq!(
            argument_pairs("[1]"),
            vec![("arguments".to_owned(), "[1]".to_owned())]
        );
    }
}
