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
use gpui_kit::base::FocusableExt as _;
use gpui_kit::base::{
    SelectableText, StyledExt as _, TextSelectionEvent, TextSelectionHandle, input::Rope,
};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    clipboard::Clipboard,
    h_flex,
    highlighter::{HighlightTheme, SyntaxHighlighter},
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement as _,
    tag::Tag,
    text::{TextView, TextViewState, TextViewStyle},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, Context, Entity, HighlightStyle, Hsla, IntoElement, ListAlignment, ListOffset,
    ListState, Render, Role, SharedString, StyleRefinement, Subscription, TestSupportExt as _,
    Window, div, list, percentage, prelude::*, px, relative, rems,
};
use std::{cell::RefCell, collections::HashMap, ops::Range, sync::Arc};
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
        self.open_dbt_details_sheet(view, window, cx);
    }

    /// Open the sheet of the details `view`.
    fn open_dbt_details_sheet(
        &mut self,
        view: Entity<DbtDetailsView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let weak = cx.weak_entity();
        window.open_sheet(cx, move |sheet, window, cx| {
            let close = weak.clone();
            let details = view.read(cx);
            let title = details.title.clone();
            let back = details.history.last().map(|visit| visit.title.clone());
            // The title fills the title bar, so that the back button stands
            // next to the close button of the sheet.
            let title_bar = h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .pr_1()
                .child(div().flex_1().min_w_0().truncate().child(title))
                .when_some(back, |bar, back| {
                    let view = view.clone();
                    let label = format!("Back to {back}");
                    bar.child(
                        Button::new("dbt-details-back")
                            .ghost()
                            .small()
                            .icon(IconName::ChevronLeft)
                            .tooltip(label.clone())
                            .accessibility_label(label)
                            .on_click(move |_, window, cx| {
                                view.update(cx, |view, cx| {
                                    view.back(window, cx);
                                });
                            }),
                    )
                });
            sheet
                .size(
                    (window.viewport_size().width * SHEET_WIDTH)
                        .clamp(px(MIN_SHEET_WIDTH), px(MAX_SHEET_WIDTH)),
                )
                .title(title_bar)
                .child(view.clone())
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |this, cx| this.dbt_details_closed(cx));
                })
        });
    }

    /// The user closed the details sheet. The way back through the lineage
    /// closes with it: only the back button goes back.
    fn dbt_details_closed(&mut self, cx: &mut Context<Self>) {
        self.dbt_details = None;
        cx.notify();
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
    /// Whether each part below the SQL is open, by [`Part::position`].
    open: [bool; 4],
    /// The resources that the user came from through the lineage, the last
    /// one on top.
    history: Vec<Visit>,
    /// The place of a resource that the user went back to, while the reads
    /// of its open SQL parts go on. Without their rows, the list can stop
    /// above the place, or not have the row of the place.
    pending_anchor: Option<(Row, gpui_kit::Pixels)>,
    /// The Markdown of the description and of the column descriptions. A
    /// text view without a state of its own parses its text again after a
    /// frame off the screen, and parses a large text in the background. A
    /// description that scrolls back in then is empty for a moment, and the
    /// list moves the rows below it. The sheet keeps the Markdown of the
    /// resources that the user can go back to, so that they show at once.
    markdown: RefCell<HashMap<MarkdownKey, Markdown>>,
    _subscription: gpui_kit::Subscription,
}

/// A resource that the user left through the lineage, and how the sheet
/// showed it: the column filter, the open parts, and the place in the list
/// where the user clicked.
struct Visit {
    unique_id: String,
    title: SharedString,
    filter: String,
    sql_open: [bool; 2],
    open: [bool; 4],
    anchor: Option<(Row, gpui_kit::Pixels)>,
}

/// What a Markdown text of the sheet belongs to: the description of a
/// resource, or the description of one of its columns.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct MarkdownKey {
    unique_id: String,
    column: Option<SharedString>,
}

/// The Markdown of a row, and its parse.
struct Markdown {
    text: SharedString,
    state: Entity<TextViewState>,
    _changes: Subscription,
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
    Missing,
    Facts,
    Description,
    Sql(SqlKind),
    SqlChunk(SqlKind, usize),
    /// The header of a part below the SQL. The header of the tests also
    /// shows the tests, and the header of the columns the column filter.
    Part(Part),
    Column(usize),
    NoColumns,
    Parent(usize),
    Child(usize),
}

impl Row {
    /// Whether the row is the header of a part that opens.
    fn is_header(self) -> bool {
        matches!(self, Row::Sql(_) | Row::Part(_))
    }

    /// The part below the SQL that the row is in, if any.
    fn part(self) -> Option<Part> {
        match self {
            Row::Part(part) => Some(part),
            Row::Column(_) | Row::NoColumns => Some(Part::Columns),
            Row::Parent(_) => Some(Part::Parents),
            Row::Child(_) => Some(Part::Children),
            _ => None,
        }
    }
}

/// The parts below the SQL. They are closed until the user opens them, so
/// that the user can go to a part without a scroll through the others, and
/// the sheet does not lay out the rows of a closed part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Tests,
    Columns,
    Parents,
    Children,
}

impl Part {
    const ALL: [Part; 4] = [Part::Tests, Part::Columns, Part::Parents, Part::Children];

    fn title(self) -> &'static str {
        match self {
            Part::Tests => "Tests",
            Part::Columns => "Columns",
            Part::Parents => "Parents",
            Part::Children => "Children",
        }
    }

    /// The part of the element IDs of the part.
    fn id(self) -> &'static str {
        match self {
            Part::Tests => "tests",
            Part::Columns => "columns",
            Part::Parents => "parents",
            Part::Children => "children",
        }
    }

    fn position(self) -> usize {
        self as usize
    }
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
    Read(Box<SqlText>),
    Failed(String),
}

/// SQL that the sheet read, in rows of [`SQL_CHUNK_LINES`] lines. Each row
/// is a selectable run in reading order, so a selection can go across them.
struct SqlText {
    full: SharedString,
    chunks: Vec<SqlChunk>,
    /// The selection of each row. A change of a selection draws the sheet
    /// again, so that a drag shows the selection while it grows.
    selections: Vec<TextSelectionHandle>,
    _selection_changes: Vec<Subscription>,
    /// The syntax tree of the full SQL. A row takes its part of the tree, so
    /// that a comment or a string that starts in an earlier row has its
    /// color.
    highlighter: SyntaxHighlighter,
    /// The highlight theme of the highlights of the rows.
    theme: Option<Arc<HighlightTheme>>,
}

/// One row of SQL: its text, its place in the full SQL, and its syntax
/// highlights once a frame shows it.
struct SqlChunk {
    text: SharedString,
    range: Range<usize>,
    highlights: Option<Highlights>,
}

/// Styles of byte ranges of a row of SQL.
type Highlights = Arc<[(Range<usize>, HighlightStyle)]>;

/// SQL and its syntax tree, which the read makes in the background.
struct ParsedSql {
    text: String,
    highlighter: SyntaxHighlighter,
}

impl ParsedSql {
    fn new(sql: &str) -> Self {
        let text = display_sql(sql);
        let mut highlighter = SyntaxHighlighter::new("sql");
        highlighter.update(None, &Rope::from_str(&text), None);
        Self { text, highlighter }
    }
}

impl SqlText {
    fn new(sql: ParsedSql, cx: &mut Context<DbtDetailsView>) -> Self {
        let chunks: Vec<SqlChunk> = chunk_ranges(&sql.text)
            .into_iter()
            .map(|range| SqlChunk {
                text: sql.text[range.clone()].to_owned().into(),
                range,
                highlights: None,
            })
            .collect();
        let selections: Vec<TextSelectionHandle> = chunks
            .iter()
            .map(|chunk| TextSelectionHandle::new(chunk.text.clone(), cx))
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
            full: sql.text.into(),
            chunks,
            selections,
            _selection_changes: selection_changes,
            highlighter: sql.highlighter,
            theme: None,
        }
    }

    /// The syntax highlights of the row `position` in the colors of
    /// `theme`, relative to the row.
    fn highlights(&mut self, position: usize, theme: &Arc<HighlightTheme>) -> Highlights {
        if !self
            .theme
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, theme))
        {
            // The theme changed, and with it the colors.
            for chunk in &mut self.chunks {
                chunk.highlights = None;
            }
            self.theme = Some(theme.clone());
        }
        let chunk = &mut self.chunks[position];
        chunk
            .highlights
            .get_or_insert_with(|| {
                let styles = self.highlighter.styles(&chunk.range, theme.as_ref());
                relative_highlights(styles, &chunk.range).into()
            })
            .clone()
    }
}

/// The SQL as the sheet shows it: without the empty lines at the start and
/// at the end, which compiled SQL often has where the model file has its
/// configuration, and with Unix line ends.
fn display_sql(sql: &str) -> String {
    let sql = sql.replace("\r\n", "\n");
    let lines: Vec<&str> = sql.split('\n').collect();
    let Some(first) = lines.iter().position(|line| !line.trim().is_empty()) else {
        return String::new();
    };
    let last = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .unwrap_or(first);
    lines[first..=last].join("\n")
}

/// The byte ranges of the rows of `text`, each [`SQL_CHUNK_LINES`] lines
/// without the line end between two rows.
fn chunk_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut lines = 0;
    for (position, byte) in text.bytes().enumerate() {
        if byte == b'\n' {
            lines += 1;
            if lines == SQL_CHUNK_LINES {
                ranges.push(start..position);
                start = position + 1;
                lines = 0;
            }
        }
    }
    if start < text.len() {
        ranges.push(start..text.len());
    }
    ranges
}

/// `styles` of the full SQL, relative to the row at `range`.
fn relative_highlights(
    styles: Vec<(Range<usize>, HighlightStyle)>,
    range: &Range<usize>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    styles
        .into_iter()
        .filter_map(|(style_range, style)| {
            let start = style_range.start.max(range.start);
            let end = style_range.end.min(range.end);
            (start < end).then(|| (start - range.start..end - range.start, style))
        })
        .collect()
}

/// The result of a read of the SQL in the background.
enum SqlRead {
    Read(Box<ParsedSql>),
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
            open: [false; 4],
            history: Vec::new(),
            pending_anchor: None,
            markdown: RefCell::default(),
            _subscription: subscription,
        };
        // The user moves the list: the place that waits for a read must not
        // move it back.
        let weak = cx.weak_entity();
        view.list.set_scroll_handler(move |_, _, cx| {
            let _ = weak.update(cx, |this, _| this.pending_anchor = None);
        });
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

    /// The positions of the columns that match the filter. Columns whose
    /// name matches come first, the closest names on top. Then come the
    /// columns that match only by their description. Each group keeps the
    /// order of the manifest.
    fn matching_columns(&self, cx: &App) -> Vec<usize> {
        let query = self.filter.read(cx).value().trim().to_lowercase();
        let Some(data) = &self.data else {
            return Vec::new();
        };
        let mut ranked: Vec<(ColumnMatch, usize)> = data
            .columns
            .iter()
            .enumerate()
            .filter_map(|(position, column)| {
                column_match(&column.name, &column.description, &query).map(|rank| (rank, position))
            })
            .collect();
        // A stable sort keeps the order of the manifest in each group.
        ranked.sort_by_key(|(rank, _)| *rank);
        ranked.into_iter().map(|(_, position)| position).collect()
    }

    fn make_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
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
        // The lineage is below the columns: the columns are read more often.
        for part in Part::ALL {
            rows.push(Row::Part(part));
            if !self.open[part.position()] {
                continue;
            }
            match part {
                Part::Tests => {}
                Part::Columns if self.matching.is_empty() => rows.push(Row::NoColumns),
                Part::Columns => rows.extend(self.matching.iter().copied().map(Row::Column)),
                Part::Parents => rows.extend((0..data.parents.len()).map(Row::Parent)),
                Part::Children => rows.extend((0..data.children.len()).map(Row::Child)),
            }
        }
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
            // dbt writes the manifest again on each run. The user keeps the
            // place, and an open part shows its old SQL until the new SQL
            // is read.
            let anchor = self.scroll_anchor();
            for part in &mut self.sql {
                if part.open && matches!(part.sql, Sql::Read(_)) {
                    part.read += 1;
                } else {
                    part.reset();
                }
            }
            self.rebuild(cx);
            for kind in SqlKind::ALL {
                if self.sql[kind.position()].open {
                    self.read_sql(kind, cx);
                }
            }
            self.restore_scroll(anchor);
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

    /// The row at the top of the list, and how far the list is scrolled
    /// into it.
    fn scroll_anchor(&self) -> Option<(Row, gpui_kit::Pixels)> {
        let top = self.list.logical_scroll_top();
        let row = self.rows.get(top.item_ix)?;
        Some((*row, top.offset_in_item))
    }

    /// Scroll the list back to `anchor`, and tell whether the rows still
    /// have its row.
    fn restore_scroll(&self, anchor: Option<(Row, gpui_kit::Pixels)>) -> bool {
        let Some((row, offset_in_item)) = anchor else {
            return false;
        };
        let Some(item_ix) = self.rows.iter().position(|candidate| *candidate == row) else {
            return false;
        };
        self.list.scroll_to(ListOffset {
            item_ix,
            offset_in_item,
        });
        true
    }

    /// Give the list the focus handle of the filter, so that the list
    /// keeps the filter row and its keyboard input when the user scrolls
    /// it out of view.
    fn keep_filter_rendered(&self, cx: &App) {
        if !self.open[Part::Columns.position()] {
            return;
        }
        if let Some(title) = self
            .rows
            .iter()
            .position(|row| *row == Row::Part(Part::Columns))
        {
            let focus = gpui_kit::Focusable::focus_handle(self.filter.read(cx), cx);
            self.list.splice_focusable(title..title + 1, [Some(focus)]);
        }
    }

    /// Whether the manifest file changed after Qrow read it.
    fn manifest_changed(&self) -> bool {
        self.state.as_ref().is_some_and(|state| !state.is_current())
    }

    /// Show the details of `unique_id`, a parent or a child of the shown
    /// resource, from the top.
    fn show(&mut self, unique_id: String, window: &mut Window, cx: &mut Context<Self>) {
        self.history.push(Visit {
            unique_id: self.unique_id.clone(),
            title: self.title.clone(),
            filter: self.filter.read(cx).value().to_string(),
            sql_open: self.sql.each_ref().map(|part| part.open),
            open: self.open,
            anchor: self.scroll_anchor(),
        });
        // The lists stay open, so that the user can follow the lineage
        // further. The SQL parts close, as their SQL is read again.
        self.open(unique_id, "", [false; 2], self.open, window, cx);
    }

    /// Show the resource that the user came from again, at the place where
    /// the user left it.
    fn back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(visit) = self.history.pop() else {
            return;
        };
        self.open(
            visit.unique_id,
            &visit.filter,
            visit.sql_open,
            visit.open,
            window,
            cx,
        );
        self.restore_scroll(visit.anchor);
        // The rows of an open SQL part come after the read.
        if self.reading() {
            self.pending_anchor = visit.anchor;
        }
    }

    /// Show the details of `unique_id` from the top, with the column filter
    /// `filter`, the SQL parts in `sql_open` open, and the parts below them
    /// in `open` open.
    fn open(
        &mut self,
        unique_id: String,
        filter: &str,
        sql_open: [bool; 2],
        open: [bool; 4],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.unique_id = unique_id;
        self.open = open;
        self.pending_anchor = None;
        // The SQL belongs to the old resource. The Markdown of the
        // resources in the history stays for a way back.
        let kept: Vec<&str> = std::iter::once(self.unique_id.as_str())
            .chain(self.history.iter().map(|visit| visit.unique_id.as_str()))
            .collect();
        self.markdown
            .borrow_mut()
            .retain(|key, _| kept.contains(&key.unique_id.as_str()));
        for (part, open) in self.sql.iter_mut().zip(sql_open) {
            part.reset();
            part.open = open;
        }
        self.filter.update(cx, |state, cx| {
            state.set_value(filter.to_owned(), window, cx)
        });
        self.rebuild(cx);
        for kind in SqlKind::ALL {
            if self.sql[kind.position()].open {
                self.read_sql(kind, cx);
            }
        }
        cx.notify();
    }

    /// The Markdown view `id` of `text` in `row`, with the state that the
    /// sheet keeps while the text stays the same.
    fn markdown(
        &self,
        column: Option<&SharedString>,
        text: &SharedString,
        row: Row,
        cx: &mut Context<Self>,
    ) -> TextView {
        markdown_view(&self.markdown_state(column, text, row, cx))
    }

    /// The Markdown state of `text` in `row`: the description of the
    /// resource, or of the column `column`. The state parses the text when
    /// the sheet makes it.
    fn markdown_state(
        &self,
        column: Option<&SharedString>,
        text: &SharedString,
        row: Row,
        cx: &mut Context<Self>,
    ) -> Entity<TextViewState> {
        let key = MarkdownKey {
            unique_id: self.unique_id.clone(),
            column: column.cloned(),
        };
        let mut markdown = self.markdown.borrow_mut();
        if let Some(current) = markdown.get(&key)
            && current.text == *text
        {
            return current.state.clone();
        }
        let state = cx.new(|cx| TextViewState::markdown(text, cx));
        let changes = cx.observe(&state, move |this, _, cx| this.markdown_parsed(row, cx));
        markdown.insert(
            key,
            Markdown {
                text: text.clone(),
                state: state.clone(),
                _changes: changes,
            },
        );
        state
    }

    /// Parse the column descriptions before the user opens the columns, for
    /// example while the pointer is on their header.
    fn prefetch_columns(&self, cx: &mut Context<Self>) {
        let Some(data) = &self.data else {
            return;
        };
        for (position, column) in data.columns.iter().enumerate() {
            if !column.description.is_empty() {
                self.markdown_state(
                    Some(&column.name),
                    &column.description,
                    Row::Column(position),
                    cx,
                );
            }
        }
    }

    /// Open or close the part `part`. A closed part keeps no rows, and the
    /// closed columns keep no Markdown.
    fn toggle_part(&mut self, part: Part, cx: &mut Context<Self>) {
        // The user moves the list now.
        self.pending_anchor = None;
        let open = !self.open[part.position()];
        self.open[part.position()] = open;
        if part == Part::Columns && !open {
            let unique_id = &self.unique_id;
            self.markdown
                .borrow_mut()
                .retain(|key, _| key.column.is_none() || key.unique_id != *unique_id);
        }
        let Some(header) = self.rows.iter().position(|row| *row == Row::Part(part)) else {
            return;
        };
        let items = |rows: &[Row]| {
            rows[header + 1..]
                .iter()
                .take_while(|row| !row.is_header())
                .count()
        };
        let anchor = self.scroll_anchor();
        let old = items(&self.rows);
        self.rows = self.make_rows();
        let new = items(&self.rows);
        self.list.splice(header + 1..header + 1 + old, new);
        self.list.remeasure_items(header..header + 1 + new);
        self.keep_filter_rendered(cx);
        if !self.restore_scroll(anchor) && anchor.is_some_and(|(row, _)| row.part() == Some(part)) {
            // The rows that the user read are gone: show their header.
            self.list.scroll_to(ListOffset {
                item_ix: header,
                offset_in_item: px(0.),
            });
        }
        cx.notify();
    }

    /// The Markdown of `row` changed, for example after a parse in the
    /// background. The list measures a row on the screen in each frame,
    /// but keeps the height of a row off the screen: measure it again, so
    /// that it does not move the rows when it scrolls back in.
    fn markdown_parsed(&mut self, row: Row, cx: &mut Context<Self>) {
        let Some(position) = self.rows.iter().position(|candidate| *candidate == row) else {
            return;
        };
        let on_screen = self.list.item_is_above_viewport(position) == Some(false)
            && self.list.item_is_below_viewport(position) == Some(false);
        if !on_screen {
            self.list.remeasure_items(position..position + 1);
            cx.notify();
        }
    }

    /// Open or close the SQL part `kind`.
    fn toggle_sql(&mut self, kind: SqlKind, cx: &mut Context<Self>) {
        let current = self.state.as_ref().is_some_and(|state| state.is_current());
        // The user moves the list now.
        self.pending_anchor = None;
        let part = &mut self.sql[kind.position()];
        part.open = !part.open;
        // A closed part keeps no SQL. A pointer on its header reads the SQL
        // again before the next click.
        if !part.open {
            part.reset();
        }
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
        // A splice moves the list to the first new row when the top row is
        // one of the old rows.
        let anchor = self.scroll_anchor();
        let old = chunks(&self.rows);
        self.rows = self.make_rows();
        let new = chunks(&self.rows);
        self.list.splice(sql + 1..sql + 1 + old, new);
        self.list.remeasure_items(sql..sql + 1 + new);
        // The place of a resource that the user went back to waits only for
        // the reads that started with it.
        let pending = if self.reading() {
            self.pending_anchor
        } else {
            self.pending_anchor.take()
        };
        if !(pending.is_some() && self.restore_scroll(pending))
            && !self.restore_scroll(anchor)
            && matches!(anchor, Some((Row::SqlChunk(chunk_kind, _), _)) if chunk_kind == kind)
        {
            // The SQL that the user read is gone: show its header.
            self.list.scroll_to(ListOffset {
                item_ix: sql,
                offset_in_item: px(0.),
            });
        }
    }

    /// Whether a SQL part reads its SQL.
    fn reading(&self) -> bool {
        self.sql
            .iter()
            .any(|part| matches!(part.sql, Sql::Reading | Sql::Refreshing))
    }

    /// Read the SQL of the closed part `kind` before the user opens it, for
    /// example while the pointer is on its header. A manifest that changed
    /// waits for the click, as Qrow reads the manifest again first.
    fn prefetch_sql(&mut self, kind: SqlKind, cx: &mut Context<Self>) {
        let part = &self.sql[kind.position()];
        let current = self.state.as_ref().is_some_and(|state| state.is_current());
        if !part.open && matches!(part.sql, Sql::Unread) && current {
            self.read_sql(kind, cx);
        }
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
        let part = &mut self.sql[kind.position()];
        part.read += 1;
        // A changed manifest can have other SQL, also SQL that the old one
        // did not have, like the compiled SQL after dbt compile.
        if !state.is_current() {
            self.refresher.refresh(&state.path);
            part.sql = Sql::Refreshing;
            self.sql_changed(kind);
            return;
        }
        // The part tells that the manifest has no such SQL.
        let Some(span) = span else {
            if matches!(part.sql, Sql::Read(_)) {
                part.sql = Sql::Unread;
                self.sql_changed(kind);
            }
            return;
        };
        // SQL that the part shows stays until the new SQL replaces it.
        if !matches!(part.sql, Sql::Read(_)) {
            part.sql = Sql::Reading;
        }
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
                // A large model has thousands of lines of SQL.
                Ok(sql) => SqlRead::Read(Box::new(ParsedSql::new(&sql))),
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
                        let text = SqlText::new(*sql, cx);
                        this.sql[kind.position()].sql = Sql::Read(Box::new(text));
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
            Row::Missing => div().into_any_element(),
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
                        .child(self.markdown(None, description, Row::Description, cx))
                        .into_any_element(),
                };
                section("Description", content).into_any_element()
            }
            Row::Sql(kind) => match data.sql {
                Some(kinds) => self.sql_part(kind, kinds.has(kind), cx),
                None => div().into_any_element(),
            },
            Row::SqlChunk(kind, position) => self.sql_chunk(kind, position, cx),
            Row::Part(part) => self.part(part, data, cx),
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
                                .child(self.markdown(
                                    Some(&column.name),
                                    &column.description,
                                    Row::Column(position),
                                    cx,
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

    /// The header of the part `part` below the SQL, with the count of its
    /// items. An open part of tests also shows the tests, and an open part
    /// of columns the column filter.
    fn part(&self, part: Part, data: &Data, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let open = self.open[part.position()];
        let count = match part {
            Part::Tests => data.tests.len(),
            Part::Columns => data.columns.len(),
            Part::Parents => data.parents.len(),
            Part::Children => data.children.len(),
        };
        let header = disclosure(
            format!("dbt-details-{}-toggle", part.id()),
            format!("{} ({count})", part.title()).into(),
            open,
            muted,
        )
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_part(part, cx)))
        .when(part == Part::Columns && !open, |header| {
            header.on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if *hovered {
                    this.prefetch_columns(cx);
                }
            }))
        });
        let body = match part {
            _ if !open => None,
            Part::Tests if data.tests.is_empty() => {
                Some(muted_text("No tests of the table.", muted))
            }
            Part::Tests => Some(tests_element(
                "dbt-details-tests",
                &data.tests,
                &self.code_font,
                muted,
            )),
            Part::Columns => (!data.columns.is_empty()).then(|| {
                Input::new(&self.filter)
                    .focus_ring(false)
                    .id("dbt-details-filter")
                    .small()
                    .cleanable(true)
                    .aria_label("Filter columns")
                    .into_any_element()
            }),
            Part::Parents | Part::Children => (count == 0).then(|| muted_text("None.", muted)),
        };
        v_flex()
            .map(|header| self.group_spacing(header, Row::Part(part)))
            .gap_2()
            .child(header)
            .children(body)
            .into_any_element()
    }

    /// The parts that open are one group: the first one stands apart from
    /// the description, and a closed part stands close to the next one.
    fn group_spacing<E: Styled>(&self, header: E, row: Row) -> E {
        if self.rows.iter().find(|row| row.is_header()) == Some(&row) {
            header.pt_4()
        } else {
            header.pt_1()
        }
    }

    /// Whether the row at `position` is the last row of an open part that
    /// another part follows. Such a row ends with space before the next
    /// part.
    fn ends_open_part(&self, position: usize) -> bool {
        let Some(row) = self.rows.get(position) else {
            return false;
        };
        let open = match row {
            Row::Sql(kind) => self.sql[kind.position()].open,
            Row::Part(part) => self.open[part.position()],
            Row::SqlChunk(..) => true,
            row => row.part().is_some(),
        };
        open && self
            .rows
            .get(position + 1)
            .is_some_and(|next| next.is_header())
    }

    /// A SQL part: a header that opens it, then the SQL with a copy button.
    /// `available` tells whether the manifest has this SQL; when it does
    /// not, the part tells why.
    fn sql_part(&self, kind: SqlKind, available: bool, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let part = &self.sql[kind.position()];
        let open = part.open;
        let header = disclosure(
            format!("dbt-details-{}-sql-toggle", kind.id()),
            kind.title().into(),
            open,
            muted,
        )
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_sql(kind, cx)))
        .when(!open, |header| {
            header.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.prefetch_sql(kind, cx);
                }
            }))
        });
        let body = open.then(|| match &part.sql {
            Sql::Refreshing => muted_text(
                "The dbt manifest changed. Qrow reads it again, then shows the SQL.",
                muted,
            ),
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
            .map(|header| self.group_spacing(header, Row::Sql(kind)))
            .gap_2()
            .child(header)
            .children(body)
            .into_any_element()
    }

    /// The rows of SQL form one box. They scroll with the sheet instead of
    /// in a box of their own.
    fn sql_chunk(&mut self, kind: SqlKind, position: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().highlight_theme.clone();
        let Sql::Read(sql) = &mut self.sql[kind.position()].sql else {
            return div().into_any_element();
        };
        let (Some(chunk), Some(selection)) =
            (sql.chunks.get(position), sql.selections.get(position))
        else {
            return div().into_any_element();
        };
        let text = chunk.text.clone();
        let selection = selection.clone();
        let last = position + 1 == sql.chunks.len();
        let highlights = sql.highlights(position, &theme);
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
                    selection,
                    text,
                )
                .highlights(highlights.iter().cloned())
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
                    // An open part ends with space before the next part.
                    .when(this.ends_open_part(position), |row| row.pb_3())
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
            .test_support()
            .size_full()
            // The scrollbar does not tell the list handler that the user
            // moves the list. The capture comes before the scrollbar.
            .capture_any_mouse_down(cx.listener(|this, _, _, _| this.pending_anchor = None))
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

/// How a column matches the filter, the best match first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ColumnMatch {
    Name,
    NameStart,
    NamePart,
    Description,
}

/// How the column `name` with `description` matches `query`, which is in
/// lowercase. An empty query matches all columns by name.
fn column_match(name: &str, description: &str, query: &str) -> Option<ColumnMatch> {
    if query.is_empty() {
        return Some(ColumnMatch::Name);
    }
    let name = name.to_lowercase();
    if name == query {
        Some(ColumnMatch::Name)
    } else if name.starts_with(query) {
        Some(ColumnMatch::NameStart)
    } else if name.contains(query) {
        Some(ColumnMatch::NamePart)
    } else if contains_folded(description, query) {
        Some(ColumnMatch::Description)
    } else {
        None
    }
}

/// The header of a part that opens on a click, with a chevron that turns
/// down when the part is open.
fn disclosure(id: String, title: SharedString, open: bool, muted: Hsla) -> Button {
    Button::new(SharedString::from(id))
        .ghost()
        .small()
        .w_full()
        .justify_start()
        // The chevron lines up with the text above the parts.
        .px_0()
        .accessibility_label(title.clone())
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
}

/// A titled part of the sheet.
fn section(title: &str, content: AnyElement) -> impl IntoElement {
    v_flex()
        .pt_5()
        .gap_2()
        .child(div().text_sm().font_semibold().child(title.to_owned()))
        .child(content)
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
        .on_click(cx.listener(move |this, _, window, cx| this.show(unique_id.clone(), window, cx)))
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
/// A Markdown text view with the state `state`.
fn markdown_view(state: &Entity<TextViewState>) -> TextView {
    TextView::new(state)
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
    use super::{
        ColumnMatch, SQL_CHUNK_LINES, argument_pairs, chunk_ranges, column_match, display_sql,
        relative_highlights,
    };
    use gpui_kit::HighlightStyle;

    #[test]
    fn columns_that_match_by_name_come_before_columns_that_match_by_description() {
        let rank = |name, description| column_match(name, description, "pdate");
        assert_eq!(rank("PDATE", ""), Some(ColumnMatch::Name));
        assert_eq!(rank("pdate_utc", ""), Some(ColumnMatch::NameStart));
        assert_eq!(rank("max_pdate", ""), Some(ColumnMatch::NamePart));
        assert_eq!(
            rank("booked_at", "Use `pdate` to filter."),
            Some(ColumnMatch::Description)
        );
        assert_eq!(rank("booked_at", "The time of the booking."), None);
        assert!(ColumnMatch::Name < ColumnMatch::NameStart);
        assert!(ColumnMatch::NameStart < ColumnMatch::NamePart);
        assert!(ColumnMatch::NamePart < ColumnMatch::Description);
        // Other letters than ASCII match in their lowercase form.
        assert_eq!(
            column_match("id", "Идентификатор бронирования", "бронирования"),
            Some(ColumnMatch::Description)
        );
        assert_eq!(column_match("any", "", ""), Some(ColumnMatch::Name));
    }

    #[test]
    fn shown_sql_has_no_empty_lines_at_the_ends() {
        assert_eq!(
            display_sql("\r\n  \n\nselect 1\r\n\n  from t \n\n \n"),
            "select 1\n\n  from t "
        );
        assert_eq!(display_sql(" \n\t\n"), "");
    }

    #[test]
    fn sql_rows_cover_the_text_without_the_line_ends_between_them() {
        let lines: Vec<String> = (0..SQL_CHUNK_LINES * 2 + 1)
            .map(|line| format!("line {line}"))
            .collect();
        let text = lines.join("\n");
        let ranges = chunk_ranges(&text);
        assert_eq!(ranges.len(), 3);
        assert_eq!(text[ranges[0].clone()], lines[..SQL_CHUNK_LINES].join("\n"));
        assert_eq!(
            text[ranges[2].clone()],
            format!("line {}", SQL_CHUNK_LINES * 2)
        );
        for pair in ranges.windows(2) {
            assert_eq!(&text[pair[0].end..pair[1].start], "\n");
        }
        assert_eq!(chunk_ranges(""), Vec::<std::ops::Range<usize>>::new());
    }

    #[test]
    fn row_highlights_are_relative_to_the_row() {
        let bold = HighlightStyle {
            font_weight: Some(gpui_kit::FontWeight::BOLD),
            ..Default::default()
        };
        let styles = vec![(0..4, bold), (8..14, bold), (20..22, bold)];
        assert_eq!(relative_highlights(styles, &(10..20)), vec![(0..4, bold)]);
    }

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
