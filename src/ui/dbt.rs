//! The dbt projects of connections: the worker that keeps the index of each
//! manifest, and the dbt group of Connection Settings.

use super::{ProfileEditor, Qrow, connection_form};
use crate::{
    activity::ActivityEntry,
    dbt::{
        Entry, Index, Kind,
        matching::{self, CatalogNames},
        worker::{self, DbtWorker, Event, ManifestState, Use, manifest_key},
    },
    model::{DbtProject, DbtRefresh, parse_schema_rules},
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IndexPath, Sizable as _,
    button::Button,
    form::{Field, Form},
    h_flex,
    input::{Input, InputState, Textarea, TextareaState},
    progress::Progress,
    select::{SearchableVec, Select, SelectState},
    v_flex,
};
use gpui_kit::{
    AnyElement, App, Context, Entity, IntoElement, PathPromptOptions, Role, TestSupportExt as _,
    Window, div, prelude::*,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

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
    /// The schema tree shows dbt data of projects that changed.
    tree_stale: bool,
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
            tree_stale: false,
        }
    }

    /// Read the manifest with the key `path` again.
    pub(super) fn refresh_path(&self, path: &Path) {
        self.worker.refresh(path);
    }

    /// A handle that asks the worker to read manifests again.
    pub(super) fn refresher(&self) -> worker::Refresher {
        self.worker.refresher()
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
        self.dbt.tree_stale = true;
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
        let mut rebuild = false;
        let mut activity = Vec::new();
        for event in self.dbt.worker.events.try_iter() {
            changed = true;
            match event {
                Event::State(state) => {
                    let previous = self.dbt.states.get(&state.path);
                    let same = match (previous.and_then(|p| p.index.as_ref()), &state.index) {
                        (Some(old), Some(new)) => Arc::ptr_eq(old, new),
                        (None, None) => true,
                        _ => false,
                    };
                    rebuild |= !same;
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
        // The schema tree marks the tables of the new index, and of a new
        // schema mapping.
        if rebuild || std::mem::take(&mut self.dbt.tree_stale) {
            self.rebuild_catalog_tree(cx);
        }
        if changed {
            self.dbt_details_changed(cx);
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
    /// index of its manifest.
    pub(super) fn dbt_project<'a>(
        &self,
        profile: &'a crate::model::Profile,
        state: Option<&'a Arc<ManifestState>>,
    ) -> Option<crate::assistant::dbt::Project<'a>> {
        let project = profile.dbt.as_ref()?;
        let state = state?;
        let index = state.index.as_ref()?;
        Some(crate::assistant::dbt::Project::new(
            index,
            project,
            state.refreshed,
            !state.is_current(),
        ))
    }

    /// The dbt resources of the tables of each connection with a dbt index.
    pub(super) fn dbt_lookups(&self) -> HashMap<Uuid, DbtLookup> {
        self.profiles
            .iter()
            .filter_map(|profile| {
                let project = profile.dbt.as_ref()?;
                let index = self.dbt.state(profile.id)?.index.clone()?;
                let relations = matching::relations(&index, project);
                Some((profile.id, DbtLookup { index, relations }))
            })
            .collect()
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
            (_, Err(error)) => Some(MatchResult::Error(error.to_string())),
            (Some(state), Ok(rules)) => state.index.as_ref().map(|index| {
                let project = DbtProject {
                    manifest: typed.clone(),
                    refresh: DbtRefresh::Manual,
                    schema_mapping: rules,
                };
                self.matches(profile, index, &project)
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
            .when_some(matches, |form_element, result| {
                let line = |id: &'static str, text: String, color| {
                    div()
                        .id(id)
                        .test_support()
                        .role(Role::Status)
                        .aria_label(text.clone())
                        .text_sm()
                        .text_color(color)
                        .child(text)
                };
                let result = match result {
                    MatchResult::Note(text) => {
                        line("connection-dbt-matches", text, muted).into_any_element()
                    }
                    MatchResult::Error(text) => {
                        line("connection-dbt-matches", text, cx.theme().danger).into_any_element()
                    }
                    MatchResult::Summary { matched, total } => {
                        let percent = percent(matched, total);
                        v_flex()
                            .w_full()
                            .gap_1()
                            .child(
                                // Progress has no element ID setter for
                                // tests, so this element gives tests one.
                                div()
                                    .id("connection-dbt-match-ratio")
                                    .test_support()
                                    .w_full()
                                    .child(
                                        Progress::new("connection-dbt-match-ratio-bar")
                                            .xsmall()
                                            .value(percent as f32)
                                            .accessibility_label(
                                                "dbt resources that matched the catalog",
                                            ),
                                    ),
                            )
                            // The percentage ends with the bar.
                            .child(
                                h_flex()
                                    .w_full()
                                    .items_start()
                                    .justify_between()
                                    .gap_2()
                                    .child(
                                        line(
                                            "connection-dbt-matches",
                                            format!(
                                                "{} of {} dbt resources matched the catalog",
                                                group(matched),
                                                group(total)
                                            ),
                                            muted,
                                        )
                                        .min_w_0(),
                                    )
                                    .child(
                                        line(
                                            "connection-dbt-match-percent",
                                            format!("{percent}%"),
                                            muted,
                                        )
                                        .flex_shrink_0(),
                                    ),
                            )
                            .into_any_element()
                    }
                };
                // The summary follows the schema mapping, so it needs no
                // label of its own.
                form_element.child(Field::new().child(result))
            })
            .into_any_element()
    }

    /// The match summary of a connection.
    fn matches(&self, profile: Uuid, index: &Index, project: &DbtProject) -> MatchResult {
        let note = |text: &str| MatchResult::Note(text.into());
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
        MatchResult::Summary {
            matched: summary.matched,
            total: summary.total,
        }
    }

    fn profile_browses(&self, profile: Uuid) -> bool {
        self.profiles
            .iter()
            .any(|candidate| candidate.id == profile && candidate.catalog.browses())
    }
}

/// What the form can tell about the matches.
enum MatchResult {
    /// The counts of the resources with a match and of all resources.
    Summary { matched: usize, total: usize },
    /// What the user must do before the form can match the resources.
    Note(String),
    /// The schema mapping rules are not valid.
    Error(String),
}

/// The share of the resources with a match, in whole percent. A share that
/// is not zero does not show as 0%, and a share that is not all does not
/// show as 100%.
fn percent(matched: usize, total: usize) -> usize {
    if total == 0 {
        return 0;
    }
    let percent = (matched * 100 + total / 2) / total;
    match percent {
        0 if matched > 0 => 1,
        100 if matched < total => 99,
        percent => percent,
    }
}

/// What the schema tree shows for a table that a dbt resource builds.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct DbtBadge {
    pub(super) unique_id: String,
    /// The text after the name, like `dbt incremental` or `dbt source`.
    pub(super) detail: String,
    /// The start of the dbt description, for the tooltip, unless the
    /// catalog comment has the same text.
    pub(super) description: Option<String>,
    /// Whether the tooltip has only a part of the description.
    pub(super) description_cut: bool,
}

/// The longest dbt description in a tooltip, in characters. The details
/// sheet has the full text.
const TOOLTIP_DESCRIPTION_CHARS: usize = 240;

/// The dbt resources of the tables of one connection.
pub(super) struct DbtLookup {
    index: Arc<Index>,
    relations: HashMap<(String, String), u32>,
}

impl DbtLookup {
    /// The badge of the table `relation` in `schema`, which has the catalog
    /// comment `comment`.
    pub(super) fn badge(
        &self,
        schema: &str,
        relation: &str,
        comment: Option<&str>,
    ) -> Option<DbtBadge> {
        let key = (schema.to_lowercase(), relation.to_lowercase());
        let entry = self.index.entry(*self.relations.get(&key)?);
        let detail = format!("dbt {}", resource_label(&self.index, entry));
        let description = entry.description.trim();
        let (description, description_cut) =
            if description.is_empty() || comment.map(str::trim) == Some(description) {
                (None, false)
            } else {
                let (summary, cut) = summary(description, TOOLTIP_DESCRIPTION_CHARS);
                (Some(summary), cut)
            };
        Some(DbtBadge {
            unique_id: entry.unique_id.to_string(),
            detail,
            description,
            description_cut,
        })
    }
}

/// The materialization of a model, like `incremental`, or the kind of
/// another resource, like `seed` or `source`.
pub(super) fn resource_label<'a>(index: &'a Index, entry: &'a Entry) -> &'a str {
    match (entry.kind, entry.materialized) {
        (Kind::Model, Some(materialized)) => index.symbol(materialized),
        (kind, _) => kind.name(),
    }
}

/// The first paragraph of `text` on one line, cut at a word to `limit`
/// characters, and whether it drops a part of `text`. The tooltip shows it
/// as Markdown, so the cut is outside a code span, a link, and strong text
/// when the text has such a place.
fn summary(text: &str, limit: usize) -> (String, bool) {
    let text = text.trim();
    let paragraph = text.split("\n\n").next().unwrap_or_default();
    let words: Vec<&str> = paragraph.split_whitespace().collect();
    let mut summary = String::new();
    let mut taken = 0;
    // The length and the word count of the longest part that is complete
    // Markdown.
    let mut complete = (0, 0);
    for word in &words {
        let length = summary.chars().count() + usize::from(taken > 0) + word.chars().count();
        if length > limit {
            break;
        }
        if taken > 0 {
            summary.push(' ');
        }
        summary.push_str(word);
        taken += 1;
        if !markdown_open(&summary) {
            complete = (summary.len(), taken);
        }
    }
    if taken < words.len() && complete.1 > 0 {
        summary.truncate(complete.0);
        taken = complete.1;
    }
    let mut cut = taken < words.len() || paragraph.len() < text.len();
    // A first word longer than the limit is cut inside the word.
    if taken == 0 && !words.is_empty() {
        summary = words[0].chars().take(limit).collect();
        cut = true;
    }
    if cut {
        summary.push('…');
    }
    (summary, cut)
}

/// Whether `text` ends inside a code span, a link, or strong text.
fn markdown_open(text: &str) -> bool {
    // The length of the backtick run that opened the code span.
    let mut code: Option<usize> = None;
    let mut strong = false;
    // In the text of a link, then in its target.
    let (mut link_text, mut link_target) = (false, false);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '`' => {
                let mut run = 1;
                while chars.peek() == Some(&'`') {
                    chars.next();
                    run += 1;
                }
                // Only a run of the same length closes a code span.
                code = match code {
                    None => Some(run),
                    Some(open) if open == run => None,
                    open => open,
                };
            }
            _ if code.is_some() => {}
            '\\' => {
                chars.next();
            }
            // Double underscores are common in identifiers, such as
            // silver__orders, where they are not emphasis.
            '*' if chars.peek() == Some(&'*') => {
                chars.next();
                strong = !strong;
            }
            '[' if !link_target => link_text = true,
            ']' if link_text => {
                link_text = false;
                link_target = chars.peek() == Some(&'(');
            }
            ')' if link_target => link_target = false,
            _ => {}
        }
    }
    code.is_some() || strong || link_text || link_target
}

/// The dbt fields of Connection Settings.
pub(super) struct DbtForm {
    pub(super) manifest: Entity<InputState>,
    pub(super) refresh: connection_form::ChoiceSelect,
    pub(super) rules: Entity<TextareaState>,
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
    fn the_match_share_shows_some_and_not_all() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(0, 10), 0);
        assert_eq!(percent(10, 10), 100);
        assert_eq!(percent(26, 2_244), 1);
        assert_eq!(percent(1, 2_244), 1);
        assert_eq!(percent(2_243, 2_244), 99);
        assert_eq!(percent(10, 16), 63);
    }

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
    fn a_table_badge_names_the_materialization_and_the_description() {
        let value = serde_json::json!({
            "metadata": {"dbt_schema_version": "https://schemas.getdbt.com/dbt/manifest/v12.json"},
            "nodes": {
                "model.lake.orders": {
                    "unique_id": "model.lake.orders", "resource_type": "model", "name": "orders",
                    "schema": "dev_core", "alias": "orders", "relation_name": "x",
                    "config": {"materialized": "incremental"},
                    "description": format!("  {}  ", "Long text. ".repeat(40)),
                    "raw_code": "select 1",
                },
            },
            "sources": {
                "source.lake.raw.orders": {
                    "unique_id": "source.lake.raw.orders", "resource_type": "source",
                    "name": "orders", "schema": "raw", "source_name": "raw",
                },
            },
        });
        let index = Arc::new(crate::dbt::parse(&serde_json::to_vec(&value).unwrap()).unwrap());
        let project = DbtProject {
            manifest: "/m".into(),
            refresh: DbtRefresh::Manual,
            schema_mapping: parse_schema_rules("dev_* = *").unwrap(),
        };
        let relations = matching::relations(&index, &project);
        let lookup = DbtLookup { index, relations };
        let badge = lookup.badge("CORE", "Orders", None).unwrap();
        assert_eq!(badge.unique_id, "model.lake.orders");
        assert_eq!(badge.detail, "dbt incremental");
        assert!(badge.description_cut);
        let description = badge.description.unwrap();
        // The cut is after a whole word.
        let start = description.strip_suffix('…').unwrap();
        assert!("Long text. ".repeat(40).starts_with(&format!("{start} ")));
        assert!(description.chars().count() <= TOOLTIP_DESCRIPTION_CHARS + 1);
        // A catalog comment with the same text needs no second line.
        let comment = "Long text. ".repeat(40);
        let same = lookup.badge("core", "orders", Some(&comment)).unwrap();
        assert_eq!(same.description, None);
        let source = lookup.badge("raw", "orders", None).unwrap();
        assert_eq!(source.detail, "dbt source");
        assert_eq!(source.description, None);
        assert!(lookup.badge("dev_core", "orders", None).is_none());
    }

    #[test]
    fn a_summary_has_the_first_paragraph_on_one_line() {
        assert_eq!(summary("Short.", 20), ("Short.".to_owned(), false));
        assert_eq!(
            summary("One\ntwo  three.", 20),
            ("One two three.".to_owned(), false)
        );
        assert_eq!(
            summary("First part.\n\nSecond part.", 20),
            ("First part.…".to_owned(), true)
        );
        assert_eq!(
            summary("Words that do not fit", 12),
            ("Words that…".to_owned(), true)
        );
        assert_eq!(summary("Ünïcödé_wörd", 4), ("Ünïc…".to_owned(), true));
    }

    #[test]
    fn a_summary_does_not_cut_markdown() {
        // A code span, a link, and strong text stay whole or go.
        assert_eq!(
            summary("Uses `a long code span` here", 14),
            ("Uses…".to_owned(), true)
        );
        assert_eq!(
            summary("See [the guide](https://example.com/guide) now", 20),
            ("See…".to_owned(), true)
        );
        assert_eq!(
            summary("Very **important text** here", 18),
            ("Very…".to_owned(), true)
        );
        assert_eq!(
            summary("Has `code` and more words", 14),
            ("Has `code` and…".to_owned(), true)
        );
        // Identifiers with underscores are not emphasis.
        assert_eq!(
            summary("silver__orders has prices", 20),
            ("silver__orders has…".to_owned(), true)
        );
        // A code span closes only with as many backticks as it opened with.
        assert_eq!(
            summary("Uses ``a ` b c`` and more", 13),
            ("Uses…".to_owned(), true)
        );
        assert_eq!(
            summary("Uses ``a ` b`` and more", 18),
            ("Uses ``a ` b`` and…".to_owned(), true)
        );
        // Text that is all one open span keeps the first words.
        assert_eq!(
            summary("`one two three four`", 9),
            ("`one two…".to_owned(), true)
        );
    }

    #[test]
    fn counts_have_digit_groups() {
        assert_eq!(group(0), "0");
        assert_eq!(group(1_884), "1,884");
        assert_eq!(group(1_234_567), "1,234,567");
    }
}
