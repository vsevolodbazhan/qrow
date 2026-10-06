//! The dbt details sheet of a table in the schema tree: the full
//! description, the tests, the lineage, and the columns of its dbt resource.

use super::Qrow;
use crate::dbt::{Index, Test, contains_folded};
use gpui_kit::base::StyledExt as _;
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::Button,
    h_flex,
    input::{Input, InputEvent, InputState},
    text::{TextView, TextViewStyle},
    v_flex,
};
use gpui_kit::{
    AnyElement, Context, Entity, IntoElement, Role, SharedString, TestSupportExt as _, Window, div,
    prelude::*, px, rems,
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

/// The dbt resource that the sheet shows.
pub(super) struct DbtDetails {
    profile: Uuid,
    unique_id: String,
    filter: Entity<InputState>,
    _subscription: gpui_kit::Subscription,
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
            _subscription: subscription,
        });
        let weak = cx.weak_entity();
        window.open_sheet(cx, move |sheet, window, cx| {
            let close = weak.clone();
            let parts = weak
                .update(cx, |this, cx| this.dbt_details_content(window, cx))
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

    /// The title and the content of the dbt details sheet. The sheet reads
    /// the current index, so it follows a refresh of the manifest.
    fn dbt_details_content(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(SharedString, AnyElement)> {
        let details = self.dbt_details.as_ref()?;
        let (profile, unique_id) = (details.profile, details.unique_id.clone());
        let filter = details.filter.clone();
        let muted = cx.theme().muted_foreground;
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
        let has_sql = entry.kind != crate::dbt::Kind::Source
            && (entry.compiled_code.is_some() || entry.raw_code.is_some());

        let mut facts = vec![("Resource", entry.kind.name().to_owned())];
        if let Some(materialized) = entry.materialized {
            facts.push(("Materialization", index.symbol(materialized).to_owned()));
        }
        if let Some(relation) = project.relation(entry) {
            facts.push(("Relation", relation));
        }
        facts.push(("Unique ID", entry.unique_id.to_string()));
        facts.push(("Path", entry.path.to_string()));
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
                .child(
                    TextView::markdown(
                        SharedString::from(format!("dbt-description-{unique_id}")),
                        description.to_owned(),
                    )
                    // Headings of the description stay below the titles
                    // of the sheet.
                    .style(
                        TextViewStyle::default()
                            .paragraph_gap(rems(0.5))
                            .heading_font_size(|_, base| base),
                    )
                    .selectable(true)
                    .text_sm()
                    .min_w_0()
                    .max_w_full(),
                )
                .into_any_element()
        };

        let model_tests: Vec<String> = tests
            .iter()
            .filter(|test| test.column.is_none())
            .map(|test| test_text(index, &project, test))
            .collect();
        let model_tests = if model_tests.is_empty() {
            muted_text("No tests of the table.", muted)
        } else {
            v_flex()
                .gap_0p5()
                .text_sm()
                .children(model_tests.into_iter().map(|test| div().child(test)))
                .into_any_element()
        };

        let name = |position: &u32| {
            let entry = index.entry(*position);
            project
                .relation(entry)
                .unwrap_or_else(|| entry.unique_id.to_string())
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
                    h_flex()
                        .gap_2()
                        .child(div().min_w_0().child(name(position)))
                        .child(div().text_color(muted).child(entry.kind.name()))
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
            .gap_2()
            .children(matching[..shown].iter().map(|column| {
                let column_tests: Vec<String> = tests
                    .iter()
                    .filter(|test| {
                        test.column
                            .as_deref()
                            .is_some_and(|name| name.eq_ignore_ascii_case(&column.name))
                    })
                    .map(|test| test_text(index, &project, test))
                    .collect();
                let data_type = column
                    .data_type
                    .map(|symbol| index.symbol(symbol).to_owned());
                let description = column.description.trim().to_owned();
                v_flex()
                    .id(SharedString::from(format!(
                        "dbt-details-column-{}",
                        column.name
                    )))
                    .test_support()
                    .role(Role::Label)
                    .aria_label(SharedString::from(column.name.to_string()))
                    .gap_0p5()
                    .text_sm()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(div().font_semibold().child(column.name.to_string()))
                            .when_some(data_type, |row, data_type| {
                                row.child(div().text_color(muted).child(data_type))
                            }),
                    )
                    .when(!description.is_empty(), |row| row.child(description))
                    .when(!column_tests.is_empty(), |row| {
                        row.child(
                            div()
                                .text_color(muted)
                                .child(format!("Tests: {}", column_tests.join("; "))),
                        )
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

        let open_sql = has_sql.then(|| {
            let unique_id = unique_id.clone();
            Button::new("dbt-details-open-sql")
                .small()
                .outline()
                .label("Open Model SQL")
                .on_click(cx.listener(move |this, _, window, cx| {
                    window.close_sheet(cx);
                    this.open_model_sql(profile, &unique_id, window, cx);
                }))
        });

        let content = v_flex()
            .id("dbt-details")
            .test_support()
            .gap_5()
            .pb_4()
            .child(facts)
            .when_some(open_sql, |content, button| {
                content.child(h_flex().child(button))
            })
            .child(section("Description", description))
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
}

/// A titled part of the sheet.
fn section(title: &str, content: AnyElement) -> impl IntoElement {
    v_flex()
        .gap_2()
        .child(div().text_sm().font_semibold().child(title.to_owned()))
        .child(content)
}

fn muted_text(text: &str, color: gpui_kit::Hsla) -> AnyElement {
    div()
        .text_sm()
        .text_color(color)
        .child(text.to_owned())
        .into_any_element()
}

/// A test as text, like `unique`, `accepted_values: a, b`, or
/// `relationships to core.customers.id`.
fn test_text(index: &Index, project: &crate::assistant::dbt::Project, test: &Test) -> String {
    let name = index.symbol(test.name);
    if !test.values.is_empty() {
        return format!("{name}: {}", test.values.join(", "));
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
        return match &test.field {
            Some(field) => format!("{name} to {target}.{field}"),
            None => format!("{name} to {target}"),
        };
    }
    match &test.arguments {
        Some(arguments) => format!("{name} {arguments}"),
        None => name.to_owned(),
    }
}
