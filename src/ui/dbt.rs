//! The dbt projects of connections: the worker that keeps the index of each
//! manifest, and the dbt group of Connection Settings.

use super::{ProfileEditor, Qrow, connection_form};
use crate::{
    activity::ActivityEntry,
    dbt::{
        Index, Kind,
        matching::{self, CatalogNames, Match},
        worker::{self, DbtWorker, Event, ManifestState, Use, manifest_key},
    },
    model::{DbtProject, DbtRefresh, parse_schema_rules},
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Sizable as _,
    button::{Button, ButtonVariants as _},
    form::{Field, Form},
    h_flex,
    input::{Input, InputState, Textarea, TextareaState},
    select::{SearchableVec, Select, SelectState},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, Context, Entity, IntoElement, PathPromptOptions, Role, SharedString,
    TestSupportExt as _, Window, div, prelude::*,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

/// The most resources without a match that the form lists.
const MAX_UNMATCHED_ROWS: usize = 200;

const AUTOMATIC: &str = "Automatic";
const MANUAL: &str = "Manual";

/// The dbt worker and the state of each manifest.
pub(super) struct DbtProjects {
    worker: DbtWorker,
    /// The state of each manifest, by key.
    states: HashMap<PathBuf, Arc<ManifestState>>,
    /// The manifest key of each connection with a dbt project.
    keys: HashMap<Uuid, PathBuf>,
    /// The projects that the worker has: connection, manifest, automatic.
    configured: Vec<(Uuid, String, bool)>,
}

impl DbtProjects {
    /// `directory` keeps the saved indexes, or `None` to keep them only in
    /// memory.
    pub(super) fn new(directory: Option<PathBuf>, wake: async_channel::Sender<()>) -> Self {
        // The worker thread keeps only a weak sender. When the thread ends,
        // it then cannot close the channel and wake the UI task from another
        // thread.
        let wake = wake.downgrade();
        let worker = DbtWorker::new(
            directory,
            Arc::new(move || {
                if let Some(wake) = wake.upgrade() {
                    let _ = wake.try_send(());
                }
            }),
        );
        Self {
            worker,
            states: HashMap::new(),
            keys: HashMap::new(),
            configured: Vec::new(),
        }
    }

    /// The state of the manifest of `profile`.
    pub(super) fn state(&self, profile: Uuid) -> Option<&Arc<ManifestState>> {
        self.states.get(self.keys.get(&profile)?)
    }
}

impl Qrow {
    /// Give the dbt worker the manifests of the connections. Nothing happens
    /// when they did not change.
    pub(super) fn sync_dbt(&mut self) {
        let configured: Vec<(Uuid, String, bool)> = self
            .profiles
            .iter()
            .filter_map(|profile| {
                let dbt = profile.dbt.as_ref()?;
                Some((
                    profile.id,
                    dbt.manifest.clone(),
                    dbt.refresh == DbtRefresh::Automatic,
                ))
            })
            .collect();
        if configured == self.dbt.configured {
            return;
        }
        self.dbt.keys = configured
            .iter()
            .map(|(id, manifest, _)| (*id, manifest_key(Path::new(manifest))))
            .collect();
        let keys = &self.dbt.keys;
        self.dbt
            .states
            .retain(|key, _| keys.values().any(|k| k == key));
        self.dbt.worker.configure(
            configured
                .iter()
                .map(|(_, manifest, automatic)| Use {
                    manifest: manifest.into(),
                    automatic: *automatic,
                })
                .collect(),
        );
        self.dbt.configured = configured;
    }

    /// Apply the events of the dbt worker. A refresh goes to the Activity of
    /// each connection that uses the manifest.
    pub(super) fn drain_dbt(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        let mut activity = Vec::new();
        for event in self.dbt.worker.events.try_iter() {
            changed = true;
            match event {
                Event::State(state) => {
                    if self.dbt.keys.values().any(|key| *key == state.path) {
                        self.dbt.states.insert(state.path.clone(), state);
                    }
                }
                Event::Log {
                    manifest,
                    severity,
                    text,
                } => {
                    for (profile, key) in &self.dbt.keys {
                        if *key == manifest {
                            activity.push((*profile, ActivityEntry::new(severity, text.clone())));
                        }
                    }
                }
            }
        }
        for (profile, entry) in activity {
            self.record_activity(profile, entry, cx);
        }
        changed
    }

    /// Read the manifest of `profile` again.
    pub(super) fn refresh_dbt(&mut self, profile: Uuid) {
        if let Some(dbt) = self
            .profiles
            .iter()
            .find(|candidate| candidate.id == profile)
            .and_then(|profile| profile.dbt.as_ref())
        {
            self.dbt.worker.refresh(Path::new(&dbt.manifest));
        }
    }

    /// The dbt project of `profile` for the assistant, when Qrow has an
    /// index of its manifest. `catalog` is the catalog of the connection.
    pub(super) fn dbt_project<'a>(
        &self,
        profile: &'a crate::model::Profile,
        state: Option<&'a Arc<ManifestState>>,
        catalog: Option<&crate::catalog::Catalog>,
    ) -> Option<crate::assistant::dbt::Project<'a>> {
        let project = profile.dbt.as_ref()?;
        let state = state?;
        let index = state.index.as_ref()?;
        Some(crate::assistant::dbt::Project::new(
            index,
            project,
            catalog.filter(|_| profile.catalog.browses()),
            state.refreshed,
            !state.is_current(),
        ))
    }

    /// Whether Qrow parses the manifest of `profile` now.
    pub(super) fn dbt_parsing(&self, profile: Uuid) -> bool {
        self.dbt.state(profile).is_some_and(|state| state.parsing)
    }

    /// Let the user choose a manifest file for the form.
    pub(super) fn choose_dbt_manifest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(form) = &this.form {
                    form.dbt.manifest.update(cx, |input, cx| {
                        input.set_value(path.to_string_lossy().into_owned(), window, cx)
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn render_dbt(&self, form: &ProfileEditor, cx: &mut Context<Self>) -> AnyElement {
        let saving = form.saving.is_some();
        let profile = form.profile.id;
        let typed = expand_home(form.dbt.manifest.read(cx).value().trim());
        // The form shows the state of the saved manifest only while the
        // field still has its path.
        let saved = self
            .profiles
            .iter()
            .find(|candidate| candidate.id == profile)
            .and_then(|profile| profile.dbt.as_ref())
            .filter(|dbt| {
                !typed.is_empty()
                    && manifest_key(Path::new(&dbt.manifest)) == manifest_key(Path::new(&typed))
            });
        let state = saved.and_then(|_| self.dbt.state(profile)).cloned();
        let status = manifest_status(&typed, saved.is_some(), state.as_deref());
        let parsing = state.as_ref().is_some_and(|state| state.parsing);
        let automatic = refresh_choice(&form.dbt.refresh, cx) == DbtRefresh::Automatic;
        let rules = parse_schema_rules(form.dbt.rules.read(cx).value().as_ref());
        let matches = match (&state, rules) {
            (_, Err(error)) => Some(Matches {
                text: error.to_string(),
                unmatched: 0,
                rows: vec![],
            }),
            (Some(state), Ok(rules)) => state.index.as_ref().map(|index| {
                let project = DbtProject {
                    manifest: typed.clone(),
                    refresh: DbtRefresh::Manual,
                    schema_mapping: rules,
                };
                self.matches(profile, index, &project, form.dbt.unmatched_open)
            }),
            _ => None,
        };
        let muted = cx.theme().muted_foreground;
        Form::vertical()
            .w_full()
            .child(
                Field::new().label("Manifest").child(
                    v_flex()
                        .w_full()
                        .gap_1()
                        .child(
                            h_flex()
                                .w_full()
                                .gap_2()
                                .child(
                                    Input::new(&form.dbt.manifest)
                                        .id("connection-dbt-manifest")
                                        .flex_1()
                                        .disabled(saving)
                                        .aria_label("Manifest"),
                                )
                                .child(
                                    Button::new("connection-dbt-choose")
                                        .label("Choose…")
                                        .disabled(saving)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.choose_dbt_manifest(window, cx)
                                        })),
                                )
                                .when(saved.is_some(), |row| {
                                    row.child(
                                        Button::new("connection-dbt-refresh-now")
                                            .label("Refresh")
                                            .disabled(saving || parsing)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.refresh_dbt(profile);
                                                cx.notify();
                                            })),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .id("connection-dbt-status")
                                .test_support()
                                .role(Role::Status)
                                .aria_label(status.clone())
                                .text_sm()
                                .text_color(muted)
                                .child(status),
                        ),
                ),
            )
            .child(
                Field::new()
                    .label("Manifest Refresh")
                    .child(
                        Select::new(&form.dbt.refresh)
                            .id("connection-dbt-refresh")
                            .w_full()
                            .disabled(saving)
                            .accessibility_label("Manifest Refresh"),
                    )
                    .description(if automatic {
                        "Reads the manifest again when dbt writes it."
                    } else {
                        "Reads the manifest again only when you select Refresh."
                    }),
            )
            .child(
                Field::new()
                    .label("Schema Mapping")
                    .child(
                        // The textarea has no element ID setter in GPUI Kit
                        // 0.6.6, so this element gives tests one.
                        div()
                            .id("connection-dbt-rules")
                            .test_support()
                            .w_full()
                            .child(
                                Textarea::new(&form.dbt.rules)
                                    .w_full()
                                    .disabled(saving)
                                    .font_family("Menlo")
                                    .aria_label("Schema Mapping"),
                            ),
                    )
                    .description(
                        "Rules like dbt_dev_* = * or analytics = prod. The first match applies.",
                    ),
            )
            .when_some(matches, |form_element, matches| {
                let Matches {
                    text,
                    unmatched,
                    rows,
                } = matches;
                form_element.child(
                    Field::new().label("Tables").child(
                        v_flex()
                            .w_full()
                            .gap_1()
                            .child(
                                h_flex()
                                    .w_full()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("connection-dbt-matches")
                                            .test_support()
                                            .role(Role::Status)
                                            .aria_label(text.clone())
                                            .flex_1()
                                            .min_w_0()
                                            .text_sm()
                                            .child(text),
                                    )
                                    .when(unmatched > 0, |row| {
                                        row.child(
                                            Button::new("connection-dbt-unmatched")
                                                .small()
                                                .ghost()
                                                .label(if form.dbt.unmatched_open {
                                                    "Hide unmatched"
                                                } else {
                                                    "Show unmatched"
                                                })
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    if let Some(form) = &mut this.form {
                                                        form.dbt.unmatched_open =
                                                            !form.dbt.unmatched_open;
                                                    }
                                                    cx.notify();
                                                })),
                                        )
                                    }),
                            )
                            .when(!rows.is_empty(), |list| {
                                list.child(
                                    v_flex()
                                        .id("connection-dbt-unmatched-list")
                                        .test_support()
                                        .w_full()
                                        .gap_0p5()
                                        .text_sm()
                                        .font_family("Menlo")
                                        .text_color(muted)
                                        .children(rows.into_iter().enumerate().map(
                                            |(number, row)| {
                                                div()
                                                    .id(("connection-dbt-unmatched-row", number))
                                                    .test_support()
                                                    .role(Role::ListItem)
                                                    .aria_label(SharedString::from(row.clone()))
                                                    .w_full()
                                                    .truncate()
                                                    .child(row)
                                            },
                                        )),
                                )
                            }),
                    ),
                )
            })
            .into_any_element()
    }

    /// The match summary of a connection, with the rows of the resources
    /// without a match when `rows` is true.
    fn matches(&self, profile: Uuid, index: &Index, project: &DbtProject, rows: bool) -> Matches {
        let note = |text: &str| Matches {
            text: text.into(),
            unmatched: 0,
            rows: vec![],
        };
        if !self.profile_browses(profile) {
            return note("Turn on Schema Refresh to match models with tables.");
        }
        let Some(catalog) = self
            .catalog
            .catalog(profile)
            .filter(|catalog| !catalog.schemas.is_empty())
        else {
            return note("Refresh the schemas of the connection to match models with tables.");
        };
        let summary = matching::summary(index, project, &CatalogNames::of(catalog));
        let mut text = format!(
            "{} of {} models and sources match tables in the catalog.",
            group(summary.matched),
            group(summary.total)
        );
        match summary.not_loaded {
            0 => {}
            1 => text.push_str(" 1 is in a schema without loaded tables."),
            count => text.push_str(&format!(
                " {} are in schemas without loaded tables.",
                group(count)
            )),
        }
        let mut list = Vec::new();
        if rows {
            list = summary
                .unmatched
                .iter()
                .take(MAX_UNMATCHED_ROWS)
                .map(|(position, schema, found)| {
                    let entry = index.entry(*position);
                    let reason = match found {
                        Match::NoSchema => "no schema",
                        _ => "no table",
                    };
                    format!(
                        "{} {}: {schema}.{} ({reason})",
                        entry.kind.name(),
                        entry.name,
                        entry.identifier
                    )
                })
                .collect();
            if summary.unmatched.len() > MAX_UNMATCHED_ROWS {
                list.push(format!(
                    "And {} more.",
                    group(summary.unmatched.len() - MAX_UNMATCHED_ROWS)
                ));
            }
        }
        Matches {
            text,
            unmatched: summary.unmatched.len(),
            rows: list,
        }
    }

    fn profile_browses(&self, profile: Uuid) -> bool {
        self.profiles
            .iter()
            .any(|candidate| candidate.id == profile && candidate.catalog.browses())
    }
}

/// The match summary that the form shows.
struct Matches {
    text: String,
    /// The number of resources without a match.
    unmatched: usize,
    /// The resources without a match, when the list is open.
    rows: Vec<String>,
}

/// The dbt fields of Connection Settings.
pub(super) struct DbtForm {
    pub(super) manifest: Entity<InputState>,
    pub(super) refresh: connection_form::ChoiceSelect,
    pub(super) rules: Entity<TextareaState>,
    pub(super) unmatched_open: bool,
    _subscriptions: Vec<gpui_kit::Subscription>,
}

impl DbtForm {
    pub(super) fn new(
        project: Option<&DbtProject>,
        window: &mut Window,
        cx: &mut Context<Qrow>,
    ) -> Self {
        let manifest = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("/path/to/project/target/manifest.json")
                .default_value(project.map(|p| p.manifest.clone()).unwrap_or_default())
        });
        let refresh = project.map_or(DbtRefresh::Automatic, |project| project.refresh);
        let refresh = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(vec![AUTOMATIC.to_owned(), MANUAL.to_owned()]),
                Some(IndexPath::default().row(usize::from(refresh == DbtRefresh::Manual))),
                window,
                cx,
            )
        });
        let rules = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .default_value(
                    project
                        .map(|project| crate::model::format_schema_rules(&project.schema_mapping))
                        .unwrap_or_default(),
                )
        });
        // The status and the match summary follow the fields.
        let subscriptions = vec![
            cx.subscribe(
                &manifest,
                |_, _, _: &gpui_kit::component::input::InputEvent, cx| cx.notify(),
            ),
            cx.subscribe(
                &rules,
                |_, _, _: &gpui_kit::component::input::InputEvent, cx| cx.notify(),
            ),
            cx.subscribe(
                &refresh,
                |_, _, _: &gpui_kit::component::select::SelectEvent<SearchableVec<String>>, cx| {
                    cx.notify()
                },
            ),
        ];
        Self {
            manifest,
            refresh,
            rules,
            unmatched_open: false,
            _subscriptions: subscriptions,
        }
    }

    /// The dbt project of the fields. An empty manifest field removes the
    /// project.
    pub(super) fn project(&self, cx: &App) -> anyhow::Result<Option<DbtProject>> {
        let manifest = expand_home(self.manifest.read(cx).value().trim());
        let rules = self.rules.read(cx).value().to_string();
        if manifest.is_empty() {
            anyhow::ensure!(
                rules.trim().is_empty(),
                "Choose the manifest.json file of the dbt project, or remove the schema mapping."
            );
            return Ok(None);
        }
        Ok(Some(DbtProject {
            manifest,
            refresh: refresh_choice(&self.refresh, cx),
            schema_mapping: parse_schema_rules(&rules)?,
        }))
    }
}

fn refresh_choice(select: &connection_form::ChoiceSelect, cx: &App) -> DbtRefresh {
    match select.read(cx).selected_value().map(String::as_str) {
        Some(MANUAL) => DbtRefresh::Manual,
        _ => DbtRefresh::Automatic,
    }
}

/// Replace a leading `~/` with the home folder.
fn expand_home(path: &str) -> String {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => Path::new(&home).join(rest).to_string_lossy().into_owned(),
        _ => path.to_owned(),
    }
}

/// The line below the manifest field.
fn manifest_status(typed: &str, saved: bool, state: Option<&ManifestState>) -> String {
    if typed.is_empty() {
        return "The manifest.json file that dbt writes in the target folder of the project."
            .into();
    }
    if !saved {
        return "Read after you save.".into();
    }
    let Some(state) = state else {
        return "Reading the manifest…".into();
    };
    if state.parsing {
        return "Reading the manifest…".into();
    }
    match (&state.index, &state.error) {
        (_, Some(error)) => error.to_string(),
        (Some(index), None) => describe_index(index),
        (None, None) => "Reading the manifest…".into(),
    }
}

/// "Manifest from 2026-10-01 08:00 UTC, dbt 1.12.5, 1,884 models".
pub(super) fn describe_index(index: &Index) -> String {
    let models = index
        .entries()
        .iter()
        .filter(|entry| entry.kind == Kind::Model)
        .count();
    let generated = index.generated_at.replacen('T', " ", 1);
    let generated = match (generated.get(..16), generated.ends_with('Z')) {
        (Some(minutes), true) => format!("{minutes} UTC"),
        (Some(minutes), false) => minutes.to_owned(),
        (None, _) => generated,
    };
    let mut text = String::from("Manifest");
    if !generated.is_empty() {
        text.push_str(&format!(" from {generated}"));
    }
    if !index.dbt_version.is_empty() {
        text.push_str(&format!(", dbt {}", index.dbt_version));
    }
    text.push_str(&format!(", {}", worker::plural(models, "model")));
    text
}

fn group(count: usize) -> String {
    worker::plural(count, "")
        .trim_end_matches([' ', 's'])
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_follows_the_field_and_the_manifest() {
        assert!(manifest_status("", false, None).starts_with("The manifest.json file"));
        assert_eq!(
            manifest_status("/m.json", false, None),
            "Read after you save."
        );
        assert_eq!(
            manifest_status("/m.json", true, None),
            "Reading the manifest…"
        );
        let state = ManifestState {
            error: Some(worker::ManifestError::NotFound),
            ..ManifestState::default()
        };
        assert_eq!(
            manifest_status("/m.json", true, Some(&state)),
            "Manifest not found: run dbt parse in the project"
        );
    }

    #[test]
    fn counts_have_digit_groups() {
        assert_eq!(group(0), "0");
        assert_eq!(group(1_884), "1,884");
        assert_eq!(group(1_234_567), "1,234,567");
    }
}
