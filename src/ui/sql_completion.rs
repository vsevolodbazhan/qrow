//! Tab providers use immutable cache indexes. Only an explicit note action can
//! ask the catalog worker to refresh metadata.
use super::*;
use crate::{
    catalog::{Catalog, CatalogIdentity, RefreshOutcome, RefreshResult, Scope, catalog_key},
    model::DatabaseType,
    sql::analysis::{self, CatalogIndex, CompletionConfig, CompletionKind},
};
use gpui_kit::base::input::lsp_types as lsp;
use gpui_kit::component::input::{CompletionNote, CompletionProvider, Rope, RopeExt};
use std::{cell::RefCell, rc::Rc};

pub(super) type IndexJobs = HashMap<Uuid, (Arc<Catalog>, Task<()>)>;

fn same_schemas(a: &Catalog, b: &Catalog) -> bool {
    a.schemas.len() == b.schemas.len()
        && a.schemas
            .iter()
            .zip(&b.schemas)
            .all(|((a, old), (b, new))| a == b && Arc::ptr_eq(old, new))
}

#[derive(Clone, PartialEq)]
struct Anchor {
    sql: Rope,
    cursor: usize,
    context_revision: u64,
}

struct SourceState {
    enabled: bool,
    context_revision: u64,
    dirty: bool,
    config: CompletionConfig,
    index: Arc<CatalogIndex>,
    refreshes: Vec<Scope>,
    result_revision: u64,
    results: Vec<(u64, RefreshResult)>,
    target: Option<(Uuid, CatalogIdentity)>,
    pending: Option<(Scope, u64)>,
    request: Option<Anchor>,
    note: Option<CompletionNote>,
}

#[derive(Clone)]
pub(super) struct Source {
    owner: WeakEntity<Qrow>,
    tab: Uuid,
    state: Rc<RefCell<SourceState>>,
}

impl Source {
    pub(super) fn new(owner: WeakEntity<Qrow>, tab: Uuid) -> Rc<Self> {
        Rc::new(Self {
            owner,
            tab,
            state: Rc::new(RefCell::new(SourceState {
                enabled: true,
                context_revision: 0,
                dirty: false,
                config: CompletionConfig::new(
                    DatabaseType::Kyuubi,
                    "default",
                    "",
                    Default::default(),
                    Default::default(),
                ),
                index: Arc::new(CatalogIndex::default()),
                refreshes: vec![],
                result_revision: 0,
                results: vec![],
                target: None,
                pending: None,
                request: None,
                note: None,
            })),
        })
    }

    fn response(
        &self,
        text: &Rope,
        cursor: usize,
        prepared: Option<(analysis::CompletionBatch, Vec<lsp::CompletionItem>)>,
    ) -> (
        usize,
        String,
        Vec<lsp::CompletionItem>,
        Option<CompletionNote>,
    ) {
        let mut state = self.state.borrow_mut();
        if !state.enabled {
            state.note = None;
            return (cursor, String::new(), vec![], None);
        }
        let anchor = Anchor {
            sql: text.clone(),
            cursor,
            context_revision: state.context_revision,
        };
        if state.request.as_ref() != Some(&anchor) {
            state.pending = None;
        }
        state.request = Some(anchor.clone());
        let (batch, items) = prepared.unwrap_or_else(|| {
            let batch = analysis::complete(&text.to_string(), cursor, &state.index, &state.config);
            let items = completion_items(text, &batch);
            (batch, items)
        });
        let note = batch.notes().first().map(|first| {
            let scope = first.refresh().cloned();
            let pending = state
                .pending
                .as_ref()
                .filter(|(pending, _)| Some(pending) == scope.as_ref());
            let result = pending.and_then(|(pending, after)| {
                state
                    .results
                    .iter()
                    .rev()
                    .find(|(revision, result)| revision > after && result.scope().covers(pending))
                    .map(|(_, result)| result.outcome())
            });
            let loading = scope.as_ref().is_some_and(|scope| {
                state.refreshes.iter().any(|active| active.covers(scope))
                    || pending.is_some() && result.is_none()
            });
            let mut message = if loading {
                "Loading catalog metadata…".to_string()
            } else {
                match result {
                    Some(RefreshOutcome::Failed(error)) => {
                        format!("Catalog refresh failed. {error}")
                    }
                    Some(RefreshOutcome::Cancelled) => "Catalog refresh was cancelled.".into(),
                    Some(RefreshOutcome::Complete) if first.is_missing_relation() => first
                        .not_found_text()
                        .unwrap_or_else(|| first.text().into()),
                    _ => first.text().to_string(),
                }
            };
            if batch.notes().len() > 1 {
                message.push_str(&format!(" and {} more", batch.notes().len() - 1));
            }
            let note = CompletionNote::new(message).loading(loading);
            if let Some(scope) = scope.filter(|_| !loading) {
                let owner = self.owner.clone();
                let tab = self.tab;
                note.action("Refresh", move |_, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        this.refresh_completion(tab, &anchor, scope.clone(), cx)
                    });
                })
            } else {
                note
            }
        });
        state.note = note.clone();
        (
            batch.replacement().start,
            batch.prefix().into(),
            items,
            note,
        )
    }
}

impl CompletionProvider for Source {
    fn completions(
        &self,
        text: &Rope,
        offset: usize,
        _: lsp::CompletionContext,
        _: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<lsp::CompletionResponse>> {
        self.state.borrow_mut().note = None;
        let source = self.clone();
        let text = text.clone();
        let revision = self.state.borrow().context_revision;
        let executor = cx.background_executor().clone();
        cx.spawn(async move |_| {
            // Dropping the superseded provider task also cancels its debounce.
            executor.timer(Duration::from_millis(80)).await;
            loop {
                let (index, config) = {
                    let state = source.state.borrow();
                    if state.context_revision != revision {
                        return Ok(lsp::CompletionResponse::Array(vec![]));
                    }
                    (state.index.clone(), state.config.clone())
                };
                let sql = text.clone();
                let request_index = index.clone();
                let batch = executor
                    .spawn(async move {
                        let batch = analysis::complete(&sql.to_string(), offset, &index, &config);
                        let items = completion_items(&sql, &batch);
                        (batch, items)
                    })
                    .await;
                if source.state.borrow().context_revision != revision {
                    return Ok(lsp::CompletionResponse::Array(vec![]));
                }
                if !Arc::ptr_eq(&source.state.borrow().index, &request_index) {
                    continue;
                }
                return Ok(lsp::CompletionResponse::Array(
                    source.response(&text, offset, Some(batch)).2,
                ));
            }
        })
    }
    fn completion_note(&self) -> Option<CompletionNote> {
        self.state.borrow().note.clone()
    }
    fn is_completion_trigger(&self, _: usize, text: &str, _: &mut App) -> bool {
        self.state.borrow().enabled
            && text.chars().count() <= 1
            && text
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '`' | '"' | ' '))
    }
}

fn completion_items(text: &Rope, batch: &analysis::CompletionBatch) -> Vec<lsp::CompletionItem> {
    let replacement = lsp::Range::new(
        text.offset_to_position(batch.replacement().start),
        text.offset_to_position(batch.replacement().end),
    );
    batch
        .items()
        .iter()
        .map(|item| lsp::CompletionItem {
            label: item.label().into(),
            detail: item.detail().map(str::to_owned),
            filter_text: Some(batch.prefix().into()),
            kind: Some(match item.kind() {
                CompletionKind::Schema => lsp::CompletionItemKind::MODULE,
                CompletionKind::Relation => lsp::CompletionItemKind::CLASS,
                CompletionKind::Column => lsp::CompletionItemKind::FIELD,
                CompletionKind::Keyword => lsp::CompletionItemKind::KEYWORD,
                CompletionKind::Function => lsp::CompletionItemKind::FUNCTION,
            }),
            documentation: Some(lsp::Documentation::MarkupContent(lsp::MarkupContent {
                kind: lsp::MarkupKind::Markdown,
                value: escape_markdown(item.documentation()),
            })),
            text_edit: Some(lsp::CompletionTextEdit::Edit(lsp::TextEdit {
                range: replacement,
                new_text: item.insert().into(),
            })),
            ..Default::default()
        })
        .collect()
}

fn escape_markdown(text: &str) -> String {
    let mut output = String::new();
    for ch in text.chars() {
        if "\\`*_{}[]<>()!#|>+-.".contains(ch) {
            output.push('\\');
        }
        output.push(ch);
    }
    output
}

impl Qrow {
    pub(super) fn complete_sql(&mut self, _: &CompleteSql, _: &mut Window, cx: &mut Context<Self>) {
        if self.form.is_some() || self.settings_open || !self.settings.autocomplete {
            return;
        }
        let Some(tab) = self.tabs.get(self.active) else {
            return;
        };
        let editor = tab.input.clone();
        let text = Rope::from(editor.read(cx).value().as_str());
        let cursor = editor.read(cx).cursor();
        let (start, prefix, items, note) = tab.completion.response(&text, cursor, None);
        editor.update(cx, |editor, cx| {
            editor.present_completion_items_with_note(start, prefix, items, note, cx)
        });
    }

    fn refresh_completion(
        &mut self,
        id: Uuid,
        anchor: &Anchor,
        scope: Scope,
        cx: &mut Context<Self>,
    ) {
        if self.form.is_some() || self.settings_open {
            return;
        }
        let Some(tab) = self.tabs.get(self.active).filter(|tab| tab.saved.id == id) else {
            return;
        };
        let editor = tab.input.read(cx);
        if tab.completion.state.borrow().request.as_ref() != Some(anchor)
            || tab.completion.state.borrow().context_revision != anchor.context_revision
            || !editor.completion_menu_state().open
            || editor.cursor() != anchor.cursor
            || anchor.sql != editor.value().as_str()
        {
            return;
        }
        let Some(profile) = tab.saved.profile else {
            return;
        };
        {
            let mut state = tab.completion.state.borrow_mut();
            state.pending = Some((scope.clone(), self.catalog.completion_results(profile).0));
            state.dirty = true;
        }
        self.refresh_catalog(profile, scope, cx);
        self.sync_completions(cx);
    }

    pub(super) fn sync_completions(&mut self, cx: &mut Context<Self>) {
        if self.settings.autocomplete {
            let profiles = self
                .tabs
                .iter()
                .filter_map(|tab| tab.saved.profile)
                .filter(|profile| !self.catalog.has_completion_source(*profile))
                .collect::<std::collections::HashSet<_>>();
            for profile in profiles {
                self.ensure_catalog(profile);
            }
        }
        let mut used = std::collections::HashSet::new();
        for (i, tab) in self.tabs.iter().enumerate() {
            let profile = self
                .profiles
                .iter()
                .find(|profile| Some(profile.id) == tab.saved.profile);
            let key = profile.map_or(Uuid::nil(), |profile| {
                catalog_key(profile, &self.shared_catalogs)
            });
            used.insert(key);
            let snapshot = profile.and_then(|profile| self.catalog.completion_snapshot(profile.id));
            let indexed = self
                .completion_indexes
                .get(&key)
                .is_some_and(|index| index.matches_snapshot(snapshot.as_ref()));
            if indexed {
                self.completion_index_jobs.remove(&key);
            }
            if !indexed {
                if let Some(snapshot) = snapshot {
                    let pending = self
                        .completion_index_jobs
                        .get(&key)
                        .is_some_and(|(pending, _)| same_schemas(pending, &snapshot));
                    if !pending {
                        let previous = self.completion_indexes.get(&key).cloned();
                        let input = snapshot.clone();
                        let work = cx.background_executor().spawn(async move {
                            CatalogIndex::new(Some(input), previous.as_deref())
                        });
                        let requested = snapshot.clone();
                        let task = cx.spawn(async move |owner, cx| {
                            let index = Arc::new(work.await);
                            let _ =
                                owner.update(cx, |this, cx| {
                                    if !this.completion_index_jobs.get(&key).is_some_and(
                                        |(pending, _)| same_schemas(pending, &requested),
                                    ) {
                                        return;
                                    }
                                    this.completion_index_jobs.remove(&key);
                                    this.completion_indexes.insert(key, index);
                                    this.sync_completions(cx);
                                    cx.notify();
                                });
                        });
                        self.completion_index_jobs.insert(key, (snapshot, task));
                    }
                    self.completion_indexes.entry(key).or_default();
                } else {
                    self.completion_index_jobs.remove(&key);
                    self.completion_indexes
                        .insert(key, Arc::new(CatalogIndex::default()));
                }
            }
            let (kind, schema, catalog, filters) = profile.map_or_else(
                || {
                    (
                        DatabaseType::Kyuubi,
                        "default".into(),
                        String::new(),
                        Default::default(),
                    )
                },
                |profile| {
                    let schema = match profile.database_type {
                        DatabaseType::Kyuubi => profile.database.clone(),
                        DatabaseType::Postgres => "public".into(),
                        DatabaseType::Trino => profile.trino_schema.clone(),
                    };
                    (
                        profile.database_type,
                        schema,
                        profile.database.clone(),
                        crate::model::effective_catalog(profile, &self.shared_catalogs),
                    )
                },
            );
            let config = CompletionConfig::new(
                kind,
                schema,
                catalog,
                filters,
                self.settings.assistant.sql_keyword_case,
            );
            let status = profile
                .map(|profile| self.catalog.completion_status(profile.id))
                .unwrap_or_default();
            let refreshes = status
                .active
                .into_iter()
                .chain(status.queued.into_iter().map(|request| request.scope))
                .collect::<Vec<_>>();
            let (result_revision, results) = profile
                .map(|profile| self.catalog.completion_results(profile.id))
                .unwrap_or_default();
            let target = profile.map(|profile| (profile.id, CatalogIdentity::of(profile)));
            let index = self.completion_indexes[&key].clone();
            let mut state = tab.completion.state.borrow_mut();
            let context_changed = state.enabled != self.settings.autocomplete
                || state.config != config
                || state.target != target;
            if context_changed {
                state.context_revision = state.context_revision.wrapping_add(1);
            }
            let changed = state.dirty
                || context_changed
                || !Arc::ptr_eq(&state.index, &index)
                || state.refreshes != refreshes
                || state.result_revision != result_revision;
            state.enabled = self.settings.autocomplete;
            state.config = config;
            state.index = index;
            state.refreshes = refreshes;
            state.result_revision = result_revision;
            state.results = results;
            state.target = target;
            state.dirty = false;
            let anchor = state.request.clone();
            drop(state);
            let editor = tab.input.read(cx);
            let open = editor.completion_menu_state().open;
            if context_changed
                || !self.settings.autocomplete
                || i != self.active
                || self.form.is_some()
                || self.settings_open
            {
                tab.input
                    .update(cx, |editor, cx| editor.dismiss_completion_overlay(cx));
                continue;
            }
            if !open {
                continue;
            }
            if !changed {
                continue;
            }
            let text = Rope::from(editor.value().as_str());
            let cursor = editor.cursor();
            if anchor
                .as_ref()
                .is_none_or(|anchor| anchor.cursor != cursor || anchor.sql != text)
            {
                continue;
            }
            let (start, prefix, items, note) = tab.completion.response(&text, cursor, None);
            tab.input.update(cx, |editor, cx| {
                editor.present_completion_items_with_note(start, prefix, items, note, cx)
            });
        }
        self.completion_indexes.retain(|key, _| used.contains(key));
        self.completion_index_jobs
            .retain(|key, _| used.contains(key));
    }
}
