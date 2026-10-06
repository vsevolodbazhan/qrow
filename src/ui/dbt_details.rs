//! The dbt details sheet of a table in the schema tree: the full
//! description, the SQL, the tests, the lineage, and the columns of its dbt
//! resource.

use super::{Qrow, dbt::resource_label};
use crate::dbt::{Index, Kind, Test, contains_folded};
use gpui_kit::base::{SelectableText, StyledExt as _};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    clipboard::Clipboard,
    collapsible::Collapsible,
    h_flex,
    input::{Input, InputEvent, InputState},
    tag::Tag,
    text::{TextView, TextViewStyle},
    v_flex,
};
use gpui_kit::{
    AnyElement, Context, Entity, Hsla, IntoElement, Role, SharedString, TestSupportExt as _,
    Window, div, percentage, prelude::*, px, rems,
};
use uuid::Uuid;

/// The most columns that the sheet shows at one time. The filter finds the
/// others.
const MAX_COLUMN_ROWS: usize = 200;
/// The most parents or children that the sheet shows.
const MAX_LINEAGE_ROWS: usize = 200;
/// The part of the window width that the sheet takes, and its limits in
/// pixels. The sheet of GPUI Kit cannot be resized.
const SHEET_WIDTH: f32 = 0.45;
const MIN_SHEET_WIDTH: f32 = 420.;
const MAX_SHEET_WIDTH: f32 = 760.;
/// The height of the SQL box. Longer SQL scrolls in it.
const SQL_HEIGHT: f32 = 420.;

/// The dbt resource that the sheet shows.
pub(super) struct DbtDetails {
    profile: Uuid,
    unique_id: String,
    filter: Entity<InputState>,
    /// Whether the SQL part is open.
    sql_open: bool,
    sql: Sql,
    /// Counts the reads of the SQL, so that the result of an old read is
    /// dropped.
    sql_read: u64,
    _subscription: gpui_kit::Subscription,
}

/// The SQL of the resource. Qrow reads it from the manifest when the user
/// opens the SQL part.
enum Sql {
    Unread,
    Reading,
    /// The manifest changed after Qrow read it, so the positions of the SQL
    /// are wrong. Qrow reads the manifest again first.
    Refreshing,
    Read(SharedString),
    Failed(String),
}

/// The result of a read of the SQL in the background.
enum SqlRead {
    Read(String),
    Changed,
    Failed(String),
}

impl Qrow {
    /// Open the details sheet of the dbt resource `unique_id` of `profile`.
    pub(super) fn open_dbt_details(
        &mut self,
        profile: Uuid,
        unique_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter columns"));
        let subscription = cx.subscribe(&filter, |_, _, _: &InputEvent, cx| cx.notify());
        self.dbt_details = Some(DbtDetails {
            profile,
            unique_id: unique_id.to_owned(),
            filter,
            sql_open: false,
            sql: Sql::Unread,
            sql_read: 0,
            _subscription: subscription,
        });
        let weak = cx.weak_entity();
        window.open_sheet(cx, move |sheet, window, cx| {
            let close = weak.clone();
            let parts = weak
                .update(cx, |this, cx| this.dbt_details_content(cx))
                .ok()
                .flatten();
            let sheet = sheet
                .size(
                    (window.viewport_size().width * SHEET_WIDTH)
                        .clamp(px(MIN_SHEET_WIDTH), px(MAX_SHEET_WIDTH)),
                )
                .on_close(move |_, _, cx| {
                    let _ = close.update(cx, |this, cx| {
                        this.dbt_details = None;
                        cx.notify();
                    });
                });
            match parts {
                Some((title, content)) => sheet.title(title).child(content),
                None => sheet,
            }
        });
    }

    /// Open or close the SQL part of the sheet.
    fn toggle_dbt_sql(&mut self, cx: &mut Context<Self>) {
        let Some(details) = self.dbt_details.as_mut() else {
            return;
        };
        details.sql_open = !details.sql_open;
        let open = details.sql_open;
        // A manifest that changed since the read has other SQL, and a failed
        // read can work now.
        let stale = match &details.sql {
            Sql::Unread | Sql::Failed(_) => true,
            Sql::Read(_) => !self
                .dbt
                .state(details.profile)
                .is_some_and(|state| state.is_current()),
            Sql::Reading | Sql::Refreshing => false,
        };
        if open && stale {
            self.read_dbt_sql(cx);
        }
        cx.notify();
    }

    /// Read the SQL of the resource of the sheet in the background: the
    /// compiled SQL, or the raw SQL when the manifest has none.
    fn read_dbt_sql(&mut self, cx: &mut Context<Self>) {
        let Some(details) = self.dbt_details.as_ref() else {
            return;
        };
        let (profile, unique_id) = (details.profile, details.unique_id.clone());
        let Some(state) = self.dbt.state(profile).cloned() else {
            return;
        };
        let span = state.index.as_ref().and_then(|index| {
            let entry = index.entry(index.find(&unique_id)?);
            entry.compiled_code.or(entry.raw_code)
        });
        let Some(span) = span else {
            return;
        };
        let stale = !state.is_current();
        if stale {
            self.dbt.refresh_path(&state.path);
        }
        let Some(details) = self.dbt_details.as_mut() else {
            return;
        };
        details.sql_read += 1;
        if stale {
            details.sql = Sql::Refreshing;
            return;
        }
        details.sql = Sql::Reading;
        let read = details.sql_read;
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
                let current = this.dbt_details.as_ref().is_some_and(|details| {
                    details.profile == profile
                        && details.unique_id == unique_id
                        && details.sql_read == read
                });
                if !current {
                    return;
                }
                match result {
                    SqlRead::Changed => this.read_dbt_sql(cx),
                    SqlRead::Read(sql) => {
                        if let Some(details) = this.dbt_details.as_mut() {
                            details.sql = Sql::Read(sql.into());
                        }
                    }
                    SqlRead::Failed(error) => {
                        if let Some(details) = this.dbt_details.as_mut() {
                            details.sql = Sql::Failed(error);
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Follow a new state of the manifest of the sheet. A new index makes
    /// the SQL old, so the sheet reads it again.
    pub(super) fn dbt_details_changed(&mut self, new_index: bool, cx: &mut Context<Self>) {
        let Some(details) = self.dbt_details.as_mut() else {
            return;
        };
        let profile = details.profile;
        if new_index {
            details.sql_read += 1;
            details.sql = Sql::Unread;
            if details.sql_open {
                self.read_dbt_sql(cx);
            }
            return;
        }
        if !matches!(details.sql, Sql::Refreshing) {
            return;
        }
        let Some(state) = self.dbt.state(profile).cloned() else {
            return;
        };
        if state.parsing {
            return;
        }
        if state.is_current() {
            self.read_dbt_sql(cx);
        } else if let (Some(error), Some(details)) = (&state.error, self.dbt_details.as_mut()) {
            details.sql = Sql::Failed(error.to_string());
        }
    }

    /// The title and the content of the dbt details sheet. The sheet reads
    /// the current index, so it follows a refresh of the manifest.
    fn dbt_details_content(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<(SharedString, AnyElement)> {
        let details = self.dbt_details.as_ref()?;
        let (profile, unique_id) = (details.profile, details.unique_id.clone());
        let filter = details.filter.clone();
        let muted = cx.theme().muted_foreground;
        let code_font: SharedString = self.settings.editor_font_family.clone().into();
        let state = self.dbt.state(profile).cloned();
        let owner = self
            .profiles
            .iter()
            .find(|candidate| candidate.id == profile);
        let found = state.as_ref().zip(owner).and_then(|(state, owner)| {
            let index = state.index.as_ref()?;
            let position = index.find(&unique_id)?;
            let project = self.dbt_project(owner, Some(state))?;
            Some((index, position, project))
        });
        let Some((index, position, project)) = found else {
            let text = format!("The dbt manifest does not have {unique_id} now.");
            let content = div()
                .id("dbt-details")
                .test_support()
                .role(Role::Label)
                .aria_label(text.clone())
                .text_sm()
                .text_color(muted)
                .child(text)
                .into_any_element();
            return Some((unique_id.into(), content));
        };
        let entry = index.entry(position);
        let tests = index.tests(position);

        let mut facts = vec![("Resource", entry.kind.name().to_owned())];
        if let Some(materialized) = entry.materialized {
            facts.push(("Materialization", index.symbol(materialized).to_owned()));
        }
        if let Some(relation) = project.relation(entry) {
            facts.push(("Relation", relation));
        }
        facts.push(("Unique ID", entry.unique_id.to_string()));
        if !entry.tags.is_empty() {
            let tags: Vec<&str> = entry.tags.iter().map(|tag| index.symbol(*tag)).collect();
            facts.push(("Tags", tags.join(", ")));
        }
        let mut manifest = index.generated_at.to_string();
        if state.as_ref().is_some_and(|state| !state.is_current()) {
            manifest.push_str(" (changed since the last read)");
        }
        facts.push(("Manifest", manifest));
        let facts = v_flex()
            .gap_1()
            .children(facts.into_iter().map(|(label, value)| {
                h_flex()
                    .gap_3()
                    .items_start()
                    .text_sm()
                    .child(
                        div()
                            .w(px(120.))
                            .flex_shrink_0()
                            .text_color(muted)
                            .child(label),
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
            }));

        let description = entry.description.trim();
        let description = if description.is_empty() {
            muted_text("No description.", muted)
        } else {
            div()
                .id("dbt-details-description")
                .test_support()
                .role(Role::Label)
                .aria_label(description.to_owned())
                .child(markdown(
                    format!("dbt-description-{unique_id}"),
                    description,
                ))
                .into_any_element()
        };

        let sql = (entry.kind != Kind::Source)
            .then(|| {
                let code = match (entry.compiled_code, entry.raw_code) {
                    (Some(_), _) => "Compiled SQL",
                    (None, Some(_)) => "Raw SQL",
                    (None, None) => return None,
                };
                Some(self.dbt_sql_part(code, entry.compiled_code.is_none(), &code_font, cx))
            })
            .flatten();

        let view = |test: &Test| test_view(index, &project, test);
        let model_tests: Vec<TestView> = tests
            .iter()
            .filter(|test| test.column.is_none())
            .map(view)
            .collect();
        let model_tests = if model_tests.is_empty() {
            muted_text("No tests of the table.", muted)
        } else {
            tests_element("dbt-details-tests", model_tests, &code_font, muted)
        };

        let lineage = |id: &str, positions: &[u32]| -> AnyElement {
            if positions.is_empty() {
                return muted_text("None.", muted);
            }
            let shown = positions.len().min(MAX_LINEAGE_ROWS);
            v_flex()
                .id(SharedString::from(format!("dbt-details-{id}")))
                .test_support()
                .gap_0p5()
                .text_sm()
                .children(positions[..shown].iter().map(|position| {
                    let entry = index.entry(*position);
                    let name = project
                        .relation(entry)
                        .unwrap_or_else(|| entry.unique_id.to_string());
                    h_flex().gap_2().child(div().min_w_0().child(name)).child(
                        div()
                            .text_color(muted)
                            .child(resource_label(index, entry).to_owned()),
                    )
                }))
                .when(shown < positions.len(), |list| {
                    list.child(div().text_color(muted).child(format!(
                        "And {} more. The assistant can page through all of them.",
                        positions.len() - shown
                    )))
                })
                .into_any_element()
        };
        let parents = lineage("parents", &entry.parents);
        let children = lineage("children", index.children(position));

        let query = filter.read(cx).value().trim().to_owned();
        let matching: Vec<_> = entry
            .columns
            .iter()
            .filter(|column| {
                query.is_empty()
                    || contains_folded(&column.name, &query)
                    || contains_folded(&column.description, &query)
            })
            .collect();
        let shown = matching.len().min(MAX_COLUMN_ROWS);
        let columns = v_flex()
            .id("dbt-details-columns")
            .test_support()
            .gap_3()
            .children(matching[..shown].iter().map(|column| {
                let column_tests: Vec<TestView> = tests
                    .iter()
                    .filter(|test| {
                        test.column
                            .as_deref()
                            .is_some_and(|name| name.eq_ignore_ascii_case(&column.name))
                    })
                    .map(view)
                    .collect();
                let data_type = column
                    .data_type
                    .map(|symbol| index.symbol(symbol).to_owned());
                let description = column.description.trim();
                let id = format!("dbt-details-column-{}", column.name);
                v_flex()
                    .id(SharedString::from(id.clone()))
                    .test_support()
                    .role(Role::Label)
                    .aria_label(SharedString::from(column.name.to_string()))
                    .gap_1()
                    .text_sm()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().font_semibold().child(column.name.to_string()))
                            .when_some(data_type, |row, data_type| {
                                row.child(div().text_color(muted).child(data_type))
                            }),
                    )
                    .when(!description.is_empty(), |row| {
                        row.child(markdown(format!("{id}-description"), description))
                    })
                    .when(!column_tests.is_empty(), |row| {
                        row.child(tests_element(
                            &format!("{id}-tests"),
                            column_tests,
                            &code_font,
                            muted,
                        ))
                    })
            }))
            .when(matching.is_empty(), |list| {
                list.child(muted_text(
                    if entry.columns.is_empty() {
                        "The dbt project does not document the columns."
                    } else {
                        "No columns match the filter."
                    },
                    muted,
                ))
            })
            .when(shown < matching.len(), |list| {
                list.child(muted_text(
                    &format!(
                        "{shown} of {} columns. Filter to find the others.",
                        matching.len()
                    ),
                    muted,
                ))
            });

        let content = v_flex()
            .id("dbt-details")
            .test_support()
            .gap_5()
            .pb_4()
            .child(facts)
            .child(section("Description", description))
            .when_some(sql, |content, sql| content.child(sql))
            .child(section("Table Tests", model_tests))
            .child(section(
                &format!("Parents ({})", entry.parents.len()),
                parents,
            ))
            .child(section(
                &format!("Children ({})", index.children(position).len()),
                children,
            ))
            .child(section(
                &format!("Columns ({})", entry.columns.len()),
                v_flex()
                    .gap_3()
                    .child(
                        Input::new(&filter)
                            .id("dbt-details-filter")
                            .small()
                            .cleanable(true)
                            .aria_label("Filter columns"),
                    )
                    .child(columns)
                    .into_any_element(),
            ))
            .into_any_element();
        Some((entry.name.to_string().into(), content))
    }

    /// The SQL part of the sheet: a header that opens it, then the SQL with
    /// a copy button. `raw_only` tells that the manifest has no compiled SQL.
    fn dbt_sql_part(
        &self,
        title: &'static str,
        raw_only: bool,
        code_font: &SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(details) = self.dbt_details.as_ref() else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let open = details.sql_open;
        let header = Button::new("dbt-details-sql-toggle")
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
            .on_click(cx.listener(|this, _, _, cx| this.toggle_dbt_sql(cx)));
        let body = match &details.sql {
            Sql::Unread | Sql::Reading => muted_text("Reading the SQL…", muted),
            Sql::Refreshing => muted_text(
                "The dbt manifest changed. Qrow reads it again, then shows the SQL.",
                muted,
            ),
            Sql::Failed(error) => muted_text(&format!("Could not read the SQL: {error}"), muted),
            Sql::Read(sql) => v_flex()
                .gap_1()
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(muted)
                        .child(div().flex_1().child(if raw_only {
                            "The manifest has no compiled SQL. This is the SQL with Jinja."
                        } else {
                            "The SQL that dbt compiled, without Jinja."
                        }))
                        .child(
                            Clipboard::new("dbt-details-copy-sql")
                                .value(sql.clone())
                                .tooltip("Copy SQL"),
                        ),
                )
                .child(
                    div()
                        .id("dbt-details-sql")
                        .test_support()
                        .role(Role::Label)
                        .aria_label(sql.clone())
                        .max_h(px(SQL_HEIGHT))
                        .overflow_y_scroll()
                        .overflow_x_scroll()
                        .p_2()
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().secondary)
                        .font_family(code_font.clone())
                        .text_xs()
                        .child(SelectableText::new("dbt-sql", sql.clone())),
                )
                .into_any_element(),
        };
        Collapsible::new()
            .open(open)
            .w_full()
            .gap_2()
            .child(header)
            .content(body)
            .into_any_element()
    }
}

/// A titled part of the sheet.
fn section(title: &str, content: AnyElement) -> impl IntoElement {
    v_flex()
        .gap_2()
        .child(div().text_sm().font_semibold().child(title.to_owned()))
        .child(content)
}

fn muted_text(text: &str, color: Hsla) -> AnyElement {
    div()
        .text_sm()
        .text_color(color)
        .child(text.to_owned())
        .into_any_element()
}

/// A dbt description as Markdown. Its headings stay below the titles of the
/// sheet.
fn markdown(id: String, text: &str) -> TextView {
    TextView::markdown(SharedString::from(id), text.to_owned())
        .style(
            TextViewStyle::default()
                .paragraph_gap(rems(0.5))
                .heading_font_size(|_, base| base),
        )
        .selectable(true)
        .text_sm()
        .min_w_0()
        .max_w_full()
}

/// A test, with its arguments as names and values.
#[derive(Debug, PartialEq)]
struct TestView {
    name: String,
    arguments: Vec<(String, String)>,
}

fn test_view(index: &Index, project: &crate::assistant::dbt::Project, test: &Test) -> TestView {
    let mut arguments = Vec::new();
    if !test.values.is_empty() {
        arguments.push(("values".to_owned(), test.values.join(", ")));
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
        arguments.push(("to".to_owned(), target));
        if let Some(field) = &test.field {
            arguments.push(("field".to_owned(), field.to_string()));
        }
    }
    if let Some(text) = &test.arguments {
        arguments.extend(argument_pairs(text));
    }
    TestView {
        name: index.symbol(test.name).to_owned(),
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
    tests: Vec<TestView>,
    code_font: &SharedString,
    muted: Hsla,
) -> AnyElement {
    let (simple, detailed): (Vec<_>, Vec<_>) = tests
        .into_iter()
        .partition(|test| test.arguments.is_empty());
    v_flex()
        .id(SharedString::from(id.to_owned()))
        .test_support()
        .gap_2()
        .when(!simple.is_empty(), |list| {
            list.child(
                h_flex()
                    .flex_wrap()
                    .gap_1()
                    .children(simple.into_iter().map(|test| test_tag(test.name))),
            )
        })
        .children(detailed.into_iter().map(|test| {
            v_flex()
                .gap_1()
                .child(h_flex().child(test_tag(test.name)))
                .children(test.arguments.into_iter().map(|(name, value)| {
                    h_flex()
                        .items_start()
                        .gap_2()
                        .pl_2()
                        .text_xs()
                        .child(div().flex_shrink_0().text_color(muted).child(name))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .font_family(code_font.clone())
                                .child(value),
                        )
                }))
        }))
        .into_any_element()
}

fn test_tag(name: String) -> impl IntoElement {
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
