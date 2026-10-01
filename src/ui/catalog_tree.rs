//! The connections sidebar. Each connection is the root of a tree of its
//! schemas, relations, and columns.
//!
//! Qrow owns the expansion state and rebuilds the tree items when a catalog,
//! the profile list, the expansion, or the search changes. Each catalog
//! worker loads the cache of its connection on first use, so the tree does
//! not read files or open sessions at startup.

use super::*;
use crate::catalog::{
    Catalog, CatalogWorker, Event as CatalogEvent, RelationKind, Scope, Status, qualified_name,
    quote_identifier,
};
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::base::{Tree, TreeEntry, TreeEntryState, TreeEvent, TreeItem, TreeState};
use gpui_kit::component::{
    Icon, button::ButtonCustomVariant, h_flex, scroll::ScrollableElement as _, spinner::Spinner,
    tooltip::Tooltip, v_flex,
};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
};

/// The most schemas and relations that a search shows.
pub(super) const MAX_SEARCH_MATCHES: usize = 500;
/// Separates the names in a node ID. Names cannot contain it in practice.
const SEPARATOR: char = '\u{1f}';
/// The height of every tree row. The virtual list needs one height for all rows.
const ROW_HEIGHT: f32 = 28.;

/// What a tree row shows. The tree item ID is the key of its node.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Node {
    Connection(Uuid),
    Schema {
        profile: Uuid,
        name: String,
        relations: Option<usize>,
        loading: bool,
        error: Option<String>,
    },
    Relation {
        profile: Uuid,
        schema: String,
        name: String,
        kind: RelationKind,
        comment: Option<String>,
        loading: bool,
        error: Option<String>,
    },
    Column {
        profile: Uuid,
        schema: String,
        relation: String,
        name: String,
        data_type: String,
        comment: Option<String>,
    },
    /// A row that explains the state of its parent, with an optional refresh.
    Notice {
        profile: Uuid,
        text: String,
        tone: Tone,
        refresh: Option<Scope>,
    },
}

impl Node {
    /// The name that Copy gives and the name for SQL, for a node with a name.
    fn names(&self) -> Option<(String, String)> {
        match self {
            Node::Schema { name, .. } => Some((name.clone(), quote_identifier(name))),
            Node::Relation { schema, name, .. } => {
                let qualified = qualified_name(schema, name);
                Some((qualified.clone(), qualified))
            }
            Node::Column { name, .. } => Some((name.clone(), quote_identifier(name))),
            Node::Connection(_) | Node::Notice { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Muted,
    Loading,
    Error,
}

/// The catalog of one connection, as the window knows it.
#[derive(Default)]
struct CatalogConnection {
    worker: Option<CatalogWorker>,
    catalog: Option<Arc<Catalog>>,
    status: Status,
}

pub(super) struct CatalogTree {
    pub(super) state: Entity<TreeState>,
    pub(super) search: Entity<InputState>,
    expanded: HashSet<SharedString>,
    /// Rows that a search expands but that the user collapsed.
    collapsed: HashSet<SharedString>,
    connections: HashMap<Uuid, CatalogConnection>,
    nodes: Rc<HashMap<SharedString, Node>>,
    /// The workspace file. Caches are next to it. `None` keeps them in memory.
    workspace: Option<PathBuf>,
}

impl CatalogTree {
    pub(super) fn new(workspace: Option<PathBuf>, window: &mut Window, cx: &mut App) -> Self {
        Self {
            state: cx.new(|cx| TreeState::new(cx)),
            search: cx.new(|cx| InputState::new(window, cx).placeholder("Search tables…")),
            expanded: HashSet::new(),
            collapsed: HashSet::new(),
            connections: HashMap::new(),
            nodes: Rc::new(HashMap::new()),
            workspace,
        }
    }

    /// Stop the workers. Use `wait_for_shutdown` to wait for them.
    pub(super) fn shutdown(&self) {
        for connection in self.connections.values() {
            if let Some(worker) = &connection.worker {
                worker.shutdown();
            }
        }
    }

    pub(super) fn wait_for_shutdown(&self, deadline: Instant) {
        for connection in self.connections.values() {
            if let Some(worker) = &connection.worker {
                worker.wait_for_shutdown(deadline.saturating_duration_since(Instant::now()));
            }
        }
    }

    fn catalog(&self, profile: Uuid) -> Option<&Catalog> {
        self.connections.get(&profile)?.catalog.as_deref()
    }

    fn status(&self, profile: Uuid) -> Option<&Status> {
        self.connections
            .get(&profile)
            .map(|connection| &connection.status)
    }

    /// Whether a refresh of the connection is in progress or waits.
    pub(super) fn is_refreshing(&self, profile: Uuid) -> bool {
        self.status(profile).is_some_and(|status| !status.is_idle())
    }

    pub(super) fn node(&self, id: &SharedString) -> Option<&Node> {
        self.nodes.get(id)
    }
}

fn connection_id(profile: Uuid) -> SharedString {
    format!("c{SEPARATOR}{profile}").into()
}

fn schema_id(profile: Uuid, schema: &str) -> SharedString {
    format!("s{SEPARATOR}{profile}{SEPARATOR}{schema}").into()
}

fn relation_id(profile: Uuid, schema: &str, relation: &str) -> SharedString {
    format!("r{SEPARATOR}{profile}{SEPARATOR}{schema}{SEPARATOR}{relation}").into()
}

fn child_id(parent: &SharedString, suffix: &str) -> SharedString {
    format!("{parent}{SEPARATOR}{suffix}").into()
}

/// Builds the tree items and their nodes.
struct Builder<'a> {
    tree: &'a CatalogTree,
    search: String,
    nodes: HashMap<SharedString, Node>,
    matches: usize,
}

impl Builder<'_> {
    fn add(&mut self, id: SharedString, label: impl Into<SharedString>, node: Node) -> TreeItem {
        self.nodes.insert(id.clone(), node);
        TreeItem::new(id, label)
    }

    /// Add a collapsed or an expanded folder. A collapsed folder gets one
    /// placeholder child so the tree shows it as a folder.
    fn folder(
        &mut self,
        item: TreeItem,
        expanded: bool,
        profile: Uuid,
        children: impl FnOnce(&mut Self) -> Vec<TreeItem>,
    ) -> TreeItem {
        if expanded {
            let children = children(self);
            if children.is_empty() {
                return item;
            }
            item.expanded(true).children(children)
        } else {
            let id = child_id(&item.id, "placeholder");
            let placeholder = self.add(
                id,
                "",
                Node::Notice {
                    profile,
                    text: String::new(),
                    tone: Tone::Muted,
                    refresh: None,
                },
            );
            item.child(placeholder)
        }
    }

    fn notice(
        &mut self,
        parent: &SharedString,
        profile: Uuid,
        text: impl Into<String>,
        tone: Tone,
        refresh: Option<Scope>,
    ) -> TreeItem {
        let text = text.into();
        // A row can show an error and a state notice, so each tone has an ID.
        let id = child_id(
            parent,
            match tone {
                Tone::Error => "error",
                Tone::Muted | Tone::Loading => "notice",
            },
        );
        self.add(
            id,
            text.clone(),
            Node::Notice {
                profile,
                text,
                tone,
                refresh,
            },
        )
    }

    fn is_expanded(&self, id: &SharedString) -> bool {
        self.tree.expanded.contains(id)
    }

    /// A search expands a row unless the user collapsed it.
    fn search_expands(&self, id: &SharedString) -> bool {
        !self.tree.collapsed.contains(id)
    }

    fn matches(&self, name: &str) -> bool {
        name.to_lowercase().contains(&self.search)
    }

    fn connection(&mut self, profile: &Profile) -> TreeItem {
        let id = connection_id(profile.id);
        let item = self.add(
            id.clone(),
            profile.name.clone(),
            Node::Connection(profile.id),
        );
        let catalog = self.tree.catalog(profile.id);
        let searching = !self.search.is_empty();
        let has_matches = searching
            && catalog.is_some_and(|catalog| {
                catalog.schemas.iter().any(|(name, schema)| {
                    self.matches(name)
                        || schema
                            .relations
                            .as_ref()
                            .is_some_and(|relations| relations.keys().any(|r| self.matches(r)))
                })
            });
        let expanded = if searching {
            has_matches && self.search_expands(&id) || self.is_expanded(&id) && !has_matches
        } else {
            self.is_expanded(&id)
        };
        self.folder(item, expanded, profile.id, |builder| {
            builder.connection_children(profile.id, &id)
        })
    }

    fn connection_children(&mut self, profile: Uuid, id: &SharedString) -> Vec<TreeItem> {
        let mut children = vec![];
        let status = self.tree.status(profile).cloned().unwrap_or_default();
        let Some(catalog) = self.tree.catalog(profile) else {
            return vec![self.notice(id, profile, "Loading…", Tone::Loading, None)];
        };
        if status.active == Some(Scope::Connection) {
            let text = if status.total == 0 {
                "Loading schemas…".to_owned()
            } else {
                format!("Loading schemas… {}/{}", status.done, status.total)
            };
            children.push(self.notice(id, profile, text, Tone::Loading, None));
        } else if let Some(error) = &catalog.error {
            children.push(self.notice(
                id,
                profile,
                error.clone(),
                Tone::Error,
                Some(Scope::Connection),
            ));
        } else if catalog.fetched_at.is_none() && catalog.schemas.is_empty() {
            let text = if status.includes(&Scope::Connection) {
                "Waiting…"
            } else {
                "Not loaded"
            };
            children.push(self.notice(
                id,
                profile,
                text,
                Tone::Muted,
                (!status.includes(&Scope::Connection)).then_some(Scope::Connection),
            ));
        } else if catalog.schemas.is_empty() {
            children.push(self.notice(id, profile, "No schemas", Tone::Muted, None));
        }
        let searching = !self.search.is_empty();
        for (name, schema) in &catalog.schemas {
            // The match limit applies only to a search. Without a search,
            // every name matches the empty text.
            if searching && self.matches >= MAX_SEARCH_MATCHES {
                break;
            }
            let schema_matches = self.matches(name);
            let relation_matches = searching
                && schema
                    .relations
                    .as_ref()
                    .is_some_and(|relations| relations.keys().any(|r| self.matches(r)));
            if searching && !schema_matches && !relation_matches {
                continue;
            }
            if searching && schema_matches {
                self.matches += 1;
            }
            let item_id = schema_id(profile, name);
            let item = self.add(
                item_id.clone(),
                name.clone(),
                Node::Schema {
                    profile,
                    name: name.clone(),
                    relations: schema.relations.as_ref().map(BTreeMap::len),
                    loading: status.includes(&Scope::Schema(name.clone())),
                    error: schema.error.clone(),
                },
            );
            // A search shows only the matching relations of a schema that
            // matches through them. A schema that matches by name shows all.
            let filtered = searching && !schema_matches;
            let expanded = if filtered {
                self.search_expands(&item_id)
            } else {
                self.is_expanded(&item_id)
            };
            children.push(self.folder(item, expanded, profile, |builder| {
                builder.schema_children(catalog, profile, name, &item_id, filtered)
            }));
        }
        children
    }

    fn schema_children(
        &mut self,
        catalog: &Catalog,
        profile: Uuid,
        schema: &str,
        id: &SharedString,
        filtered: bool,
    ) -> Vec<TreeItem> {
        let node = catalog.schema(schema).unwrap();
        let status = self.tree.status(profile).cloned().unwrap_or_default();
        let scope = Scope::Schema(schema.into());
        let mut children = vec![];
        match (&node.relations, &node.error) {
            (None, _) if status.includes(&scope) => {
                children.push(self.notice(id, profile, "Loading…", Tone::Loading, None))
            }
            (None, Some(error)) => {
                children.push(self.notice(id, profile, error.clone(), Tone::Error, Some(scope)))
            }
            (None, None) => {
                children.push(self.notice(id, profile, "Not loaded", Tone::Muted, Some(scope)))
            }
            (Some(relations), error) => {
                if let Some(error) = error {
                    children.push(self.notice(
                        id,
                        profile,
                        error.clone(),
                        Tone::Error,
                        Some(scope),
                    ));
                } else if relations.is_empty() {
                    children.push(self.notice(id, profile, "No tables", Tone::Muted, None));
                }
                for (name, relation) in relations {
                    if filtered && !self.matches(name) {
                        continue;
                    }
                    if filtered {
                        if self.matches >= MAX_SEARCH_MATCHES {
                            break;
                        }
                        self.matches += 1;
                    }
                    let item_id = relation_id(profile, schema, name);
                    let item = self.add(
                        item_id.clone(),
                        name.clone(),
                        Node::Relation {
                            profile,
                            schema: schema.into(),
                            name: name.clone(),
                            kind: relation.kind,
                            comment: relation.comment.clone(),
                            loading: status.includes(&Scope::Relation(schema.into(), name.clone()))
                                || status.includes(&Scope::Schema(schema.into())),
                            error: relation.error.clone(),
                        },
                    );
                    let expanded = self.is_expanded(&item_id);
                    children.push(self.folder(item, expanded, profile, |builder| {
                        builder.relation_children(catalog, profile, schema, name, &item_id)
                    }));
                }
            }
        }
        children
    }

    fn relation_children(
        &mut self,
        catalog: &Catalog,
        profile: Uuid,
        schema: &str,
        relation: &str,
        id: &SharedString,
    ) -> Vec<TreeItem> {
        let node = catalog.relation(schema, relation).unwrap();
        let status = self.tree.status(profile).cloned().unwrap_or_default();
        let scope = Scope::Relation(schema.into(), relation.into());
        let loading = status.includes(&scope) || status.includes(&Scope::Schema(schema.into()));
        let mut children = vec![];
        if let Some(error) = &node.error {
            children.push(self.notice(
                id,
                profile,
                error.clone(),
                Tone::Error,
                Some(scope.clone()),
            ));
        }
        match &node.columns {
            None if loading => {
                children.push(self.notice(id, profile, "Loading…", Tone::Loading, None))
            }
            None if node.error.is_none() => {
                children.push(self.notice(id, profile, "Not loaded", Tone::Muted, Some(scope)))
            }
            None => {}
            Some(columns) if columns.is_empty() => {
                children.push(self.notice(id, profile, "No columns", Tone::Muted, None))
            }
            Some(columns) => {
                for (ix, column) in columns.iter().enumerate() {
                    let item_id = child_id(id, &format!("{ix}{SEPARATOR}{}", column.name));
                    children.push(self.add(
                        item_id,
                        format!("{} {}", column.name, column.data_type),
                        Node::Column {
                            profile,
                            schema: schema.into(),
                            relation: relation.into(),
                            name: column.name.clone(),
                            data_type: column.data_type.clone(),
                            comment: column.comment.clone(),
                        },
                    ));
                }
            }
        }
        children
    }
}

/// What a connection row shows in the current frame.
struct ConnectionRow {
    name: String,
    tooltip: String,
    busy: bool,
    refreshing: bool,
    unread_error: bool,
    active: bool,
}

impl Qrow {
    /// Rebuild the tree items. Keeps the selected row when it still exists.
    pub(super) fn rebuild_catalog_tree(&mut self, cx: &mut Context<Self>) {
        let search = self.catalog.search.read(cx).value().trim().to_lowercase();
        let mut builder = Builder {
            tree: &self.catalog,
            search,
            nodes: HashMap::new(),
            matches: 0,
        };
        let mut items: Vec<TreeItem> = self
            .profiles
            .iter()
            .map(|profile| builder.connection(profile))
            .collect();
        if builder.matches >= MAX_SEARCH_MATCHES {
            // The first row, so it shows without a scroll to the end.
            let text =
                format!("Showing the first {MAX_SEARCH_MATCHES} matches. Refine your search.");
            let notice = builder.add(
                "search-limit".into(),
                text.clone(),
                Node::Notice {
                    profile: Uuid::nil(),
                    text,
                    tone: Tone::Muted,
                    refresh: None,
                },
            );
            items.insert(0, notice);
        }
        self.catalog.nodes = Rc::new(builder.nodes);
        self.catalog.state.update(cx, |state, cx| {
            let selected = state.selected_item().map(|item| item.id.clone());
            state.set_items(items, cx);
            if let Some(selected) = selected {
                let ix = state.index_of(&selected);
                state.set_selected_index(ix, cx);
            }
        });
    }

    /// Start the catalog worker of `profile` if it does not exist. The worker
    /// loads the cache; it opens a session only for a refresh.
    fn ensure_catalog(&mut self, profile: Uuid) {
        if self.demo {
            self.catalog
                .connections
                .entry(profile)
                .or_insert_with(|| CatalogConnection {
                    worker: None,
                    catalog: Some(Arc::new(demo_catalog(profile))),
                    status: Status::default(),
                });
            return;
        }
        let Some(saved) = self.profiles.iter().find(|p| p.id == profile).cloned() else {
            return;
        };
        let connection = self.catalog.connections.entry(profile).or_default();
        if connection.worker.is_some() {
            return;
        }
        let wake = self.wake.clone();
        let cache = self
            .catalog
            .workspace
            .as_deref()
            .filter(|_| self.saver.is_some())
            .map(|workspace| storage::catalog_path(workspace, profile));
        connection.worker = Some(CatalogWorker::new(
            saved,
            cache,
            Arc::new(move || {
                let _ = wake.try_send(());
            }),
            self.credentials.clone(),
        ));
    }

    /// The demo shows the tree of its first connection open to one table.
    pub(super) fn expand_demo_catalog(&mut self) {
        let Some(profile) = self.profiles.first().map(|profile| profile.id) else {
            return;
        };
        self.ensure_catalog(profile);
        self.catalog.expanded.extend([
            connection_id(profile),
            schema_id(profile, "avia"),
            relation_id(profile, "avia", "bookings"),
        ]);
    }

    /// Ask the worker of `profile` to read `scope` again.
    pub(super) fn refresh_catalog(&mut self, profile: Uuid, scope: Scope, cx: &mut Context<Self>) {
        self.ensure_catalog(profile);
        if let Some(worker) = self
            .catalog
            .connections
            .get(&profile)
            .and_then(|connection| connection.worker.as_ref())
        {
            worker.refresh(scope);
        }
        cx.notify();
    }

    pub(super) fn cancel_catalog_refresh(&mut self, profile: Uuid) {
        if let Some(worker) = self
            .catalog
            .connections
            .get(&profile)
            .and_then(|connection| connection.worker.as_ref())
        {
            worker.cancel();
        }
    }

    /// Whether a tab of `profile` has a live session. A refresh then uses
    /// the engine that the tab already started.
    fn catalog_warm(&self, profile: Uuid) -> bool {
        self.tabs
            .iter()
            .any(|tab| tab.worker_profile == Some(profile) && tab.connected)
    }

    /// Applies what the catalog workers sent since the last tick.
    pub(super) fn drain_catalogs(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        let mut activities = Vec::new();
        for (profile, connection) in &mut self.catalog.connections {
            let Some(worker) = &connection.worker else {
                continue;
            };
            activities.extend(worker.activities.try_iter().map(|event| (*profile, event)));
            for event in worker.events.try_iter() {
                changed = true;
                match event {
                    CatalogEvent::Catalog(catalog) => connection.catalog = Some(catalog),
                    CatalogEvent::Status(status) => connection.status = status,
                }
            }
        }
        // A refresh belongs to a connection. Its Logs entries go to the tab
        // that the connection shows. They do not mark an unread error,
        // because the tree shows refresh errors.
        for (profile, event) in activities {
            if let Some(index) = self.active_tab_for_profile(profile) {
                Self::record_activity(&mut self.tabs[index], event);
                changed = true;
            }
        }
        if changed {
            self.rebuild_catalog_tree(cx);
        }
        changed
    }

    /// Follows the expansion that the user changed in the tree. Expanding a
    /// row without data reads it when the connection has a live session.
    pub(super) fn catalog_expansion_changed(&mut self, event: &TreeEvent, cx: &mut Context<Self>) {
        let searching = !self.catalog.search.read(cx).value().trim().is_empty();
        let (id, expanded) = match event {
            TreeEvent::Expanded(id) => (id.clone(), true),
            TreeEvent::Collapsed(id) => (id.clone(), false),
        };
        if expanded {
            self.catalog.expanded.insert(id.clone());
            self.catalog.collapsed.remove(&id);
        } else {
            self.catalog.expanded.remove(&id);
            if searching {
                self.catalog.collapsed.insert(id.clone());
            }
        }
        if expanded && let Some(node) = self.catalog.node(&id).cloned() {
            match node {
                Node::Connection(profile) => {
                    self.ensure_catalog(profile);
                    if self.catalog_warm(profile)
                        && let Some(worker) = self
                            .catalog
                            .connections
                            .get(&profile)
                            .and_then(|connection| connection.worker.as_ref())
                    {
                        worker.refresh_if_unloaded();
                    }
                }
                Node::Schema {
                    profile,
                    name,
                    relations: None,
                    loading: false,
                    error: None,
                } if self.catalog_warm(profile) => {
                    self.refresh_catalog(profile, Scope::Schema(name), cx);
                }
                Node::Relation {
                    profile,
                    schema,
                    name,
                    loading: false,
                    error: None,
                    ..
                } if self.catalog_warm(profile)
                    && self
                        .catalog
                        .catalog(profile)
                        .and_then(|catalog| catalog.relation(&schema, &name))
                        .is_some_and(|relation| relation.columns.is_none()) =>
                {
                    self.refresh_catalog(profile, Scope::Relation(schema, name), cx);
                }
                _ => {}
            }
        }
        self.rebuild_catalog_tree(cx);
        cx.notify();
    }

    /// A search reads every cache, so it can find tables in any connection.
    pub(super) fn catalog_search_changed(&mut self, cx: &mut Context<Self>) {
        self.catalog.collapsed.clear();
        if !self.catalog.search.read(cx).value().trim().is_empty() {
            for index in 0..self.profiles.len() {
                self.ensure_catalog(self.profiles[index].id);
            }
        }
        self.rebuild_catalog_tree(cx);
        cx.notify();
    }

    /// Gives an edited profile to its catalog worker.
    pub(super) fn catalog_profile_saved(&mut self, profile: &Profile, cx: &mut Context<Self>) {
        if let Some(worker) = self
            .catalog
            .connections
            .get(&profile.id)
            .and_then(|connection| connection.worker.as_ref())
        {
            worker.update_profile(profile.clone());
        }
        self.rebuild_catalog_tree(cx);
    }

    /// Stops the worker of a deleted profile and deletes its cache.
    pub(super) fn catalog_profile_deleted(&mut self, profile: Uuid, cx: &mut Context<Self>) {
        let worker = self
            .catalog
            .connections
            .remove(&profile)
            .and_then(|connection| connection.worker);
        match worker {
            Some(worker) => worker.delete(),
            None => {
                if let Some(workspace) = self.catalog.workspace.clone().filter(|_| !self.demo) {
                    // File work stays off the window thread.
                    std::thread::spawn(move || {
                        storage::delete_catalog(&storage::catalog_path(&workspace, profile))
                    });
                }
            }
        }
        self.rebuild_catalog_tree(cx);
    }

    /// Inserts `text` at the cursor of the active query tab.
    fn insert_into_editor(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.tabs[self.active].input.clone();
        input.update(cx, |state, cx| {
            state.replace(text, window, cx);
            state.focus(window, cx);
        });
    }

    /// The names of the row that the tree selects.
    fn selected_catalog_names(&self, cx: &App) -> Option<(String, String)> {
        let state = self.catalog.state.read(cx);
        self.catalog.node(&state.selected_item()?.id)?.names()
    }

    /// Copies the name of the selected row, like Copy Name in its menu.
    pub(super) fn copy_catalog_name(&mut self, cx: &mut Context<Self>) {
        if let Some((name, _)) = self.selected_catalog_names(cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(name));
        }
    }

    /// Inserts the name of the selected row, like Insert into Editor in its menu.
    pub(super) fn insert_catalog_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, insert)) = self.selected_catalog_names(cx) {
            self.insert_into_editor(insert, window, cx);
        }
    }

    pub(super) fn open_catalog_menu(
        &mut self,
        id: SharedString,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(node) = self.catalog.node(&id).cloned() else {
            return;
        };
        let Some((name, insert)) = node.names() else {
            return;
        };
        let (profile, scope) = match &node {
            Node::Schema { profile, name, .. } => (*profile, Some(Scope::Schema(name.clone()))),
            Node::Relation {
                profile,
                schema,
                name,
                ..
            } => (
                *profile,
                Some(Scope::Relation(schema.clone(), name.clone())),
            ),
            Node::Column { profile, .. } => (*profile, None),
            Node::Connection(_) | Node::Notice { .. } => return,
        };
        let refreshing = scope.as_ref().is_some_and(|scope| {
            self.catalog
                .status(profile)
                .is_some_and(|status| status.includes(scope))
        });
        let copy_label = if matches!(node, Node::Relation { .. }) {
            "Copy Qualified Name"
        } else {
            "Copy Name"
        };
        let refresh = scope.map(|scope| {
            cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.refresh_catalog(profile, scope.clone(), cx)
            })
        });
        let copy = move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            cx.write_to_clipboard(ClipboardItem::new_string(name.clone()))
        };
        let insert = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.insert_into_editor(insert.clone(), window, cx)
        });
        self.open_context_menu(
            position,
            move |menu, _, _| {
                let menu = match refresh {
                    Some(refresh) => menu
                        .item(
                            PopupMenuItem::new("Refresh")
                                .on_click(refresh)
                                .disabled(refreshing),
                        )
                        .separator(),
                    None => menu,
                };
                menu.item(PopupMenuItem::new(copy_label).on_click(copy))
                    .item(PopupMenuItem::new("Insert into Editor").on_click(insert))
            },
            window,
            cx,
        );
    }

    /// The Connections sidebar: its header, the search, and the tree.
    pub(super) fn connections(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let action_size = self.ui_px(28.);
        let active = self.active_profile();
        let rows: Rc<HashMap<Uuid, ConnectionRow>> = Rc::new(
            self.profiles
                .iter()
                .map(|profile| {
                    let id = profile.id;
                    (
                        id,
                        ConnectionRow {
                            name: profile.name.clone(),
                            tooltip: workspace_view::connection_tooltip(profile),
                            busy: self.profile_busy(id),
                            refreshing: self.catalog.is_refreshing(id),
                            unread_error: self
                                .tabs
                                .iter()
                                .any(|tab| tab.saved.profile == Some(id) && tab.panel.unread_error),
                            active: active == Some(id),
                        },
                    )
                })
                .collect(),
        );
        let nodes = self.catalog.nodes.clone();
        let tree = self.catalog.state.clone();
        let weak = cx.weak_entity();
        let scale = self.settings.ui_scale;
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                h_flex()
                    .h(self.ui_px(workspace_view::TAB_BAR_HEIGHT))
                    .flex_shrink_0()
                    .items_center()
                    .pl_3()
                    .pr_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .flex_1()
                            .text_base()
                            .font_weight(FontWeight::MEDIUM)
                            .child("Connections"),
                    )
                    .child(
                        Button::new("add-connection")
                            .ghost()
                            .small()
                            .w(action_size)
                            .h(action_size)
                            .flex_shrink_0()
                            .icon(IconName::Plus)
                            .accessibility_label("New Connection")
                            .tooltip("New Connection…")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.edit_profile(Profile::default(), true, window, cx)
                            })),
                    ),
            )
            .when(!self.profiles.is_empty(), |el| {
                el.child(
                    div().p_2().flex_shrink_0().child(
                        Input::new(&self.catalog.search)
                            .small()
                            .w_full()
                            .aria_label("Search Tables"),
                    ),
                )
            })
            .child(
                div()
                    .id("connections-list")
                    .test_support()
                    .on_action(
                        cx.listener(|this, _: &CopyCatalogName, _, cx| this.copy_catalog_name(cx)),
                    )
                    .on_action(cx.listener(|this, _: &InsertCatalogName, window, cx| {
                        this.insert_catalog_name(window, cx)
                    }))
                    .flex_1()
                    .min_h_0()
                    // Rows keep a margin from the sidebar edges. The scrollbar
                    // stays at the edge, above the rows.
                    .px_2()
                    .child(
                        Tree::new(&tree)
                            .item(move |_, entry, state, window, cx| {
                                render_entry(
                                    entry, state, &nodes, &rows, &weak, &tree, scale, window, cx,
                                )
                            })
                            .list_style(StyleRefinement::default().size_full())
                            .size_full(),
                    )
                    .vertical_scrollbar(&self.catalog.state.read(cx).scroll_handle().clone()),
            )
    }
}

/// The tree row of `entry`.
#[allow(clippy::too_many_arguments)]
fn render_entry(
    entry: &TreeEntry,
    state: TreeEntryState,
    nodes: &HashMap<SharedString, Node>,
    rows: &HashMap<Uuid, ConnectionRow>,
    weak: &WeakEntity<Qrow>,
    tree: &Entity<TreeState>,
    scale: f32,
    _: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let ui_px = |value: f32| px(scale * value);
    let id = entry.item().id.clone();
    let disclosure = div()
        .w(ui_px(16.))
        .flex_shrink_0()
        .flex()
        .justify_center()
        .when(entry.is_folder(), |el| {
            el.child(
                Icon::new(if entry.is_expanded() {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall()
                .text_color(cx.theme().muted_foreground),
            )
        });
    let indent = ui_px(8. + 14. * entry.depth() as f32);
    let Some(node) = nodes.get(&id) else {
        return div().h(ui_px(ROW_HEIGHT)).into_any_element();
    };
    // Clicks on rows focus the tree for keyboard navigation. They do not
    // stop propagation, so the tree also selects and expands the row.
    let focus = {
        let tree = tree.clone();
        move |_: &MouseDownEvent, window: &mut Window, cx: &mut App| {
            tree.update(cx, |tree, cx| tree.focus(window, cx));
        }
    };
    if let Node::Connection(profile) = node {
        let Some(row) = rows.get(profile) else {
            return div().into_any_element();
        };
        return connection_row(*profile, row, state, disclosure, weak, cx)
            .pl(ui_px(4.))
            .h(ui_px(ROW_HEIGHT))
            .on_mouse_down(MouseButton::Left, focus)
            .into_any_element();
    }
    let (icon, label, detail, loading, error, tooltip): (
        Option<AssetIconName>,
        SharedString,
        Option<String>,
        bool,
        Option<String>,
        Option<String>,
    ) = match node {
        Node::Schema {
            name,
            relations,
            loading,
            error,
            ..
        } => (
            Some(AssetIconName::Database),
            name.clone().into(),
            relations.map(|count| count.to_string()),
            *loading,
            error.clone(),
            row_tooltip(name, None, error.as_deref()),
        ),
        Node::Relation {
            name,
            kind,
            comment,
            loading,
            error,
            ..
        } => (
            Some(match kind {
                RelationKind::Table => AssetIconName::Table,
                RelationKind::View => AssetIconName::Eye,
            }),
            name.clone().into(),
            None,
            *loading,
            error.clone(),
            row_tooltip(name, comment.as_deref(), error.as_deref()),
        ),
        Node::Column {
            name,
            data_type,
            comment,
            ..
        } => (
            None,
            name.clone().into(),
            Some(data_type.clone()),
            false,
            None,
            row_tooltip(&format!("{name} {data_type}"), comment.as_deref(), None),
        ),
        Node::Notice {
            profile,
            text,
            tone,
            refresh,
        } => {
            if text.is_empty() {
                return div().h(ui_px(ROW_HEIGHT)).into_any_element();
            }
            return notice_row(&id, *profile, text, *tone, refresh.clone(), weak, scale, cx)
                .pl(indent + ui_px(16.))
                .into_any_element();
        }
        Node::Connection(_) => unreachable!(),
    };
    let menu_id = id.clone();
    h_flex()
        .id(id.clone())
        .w_full()
        .h(ui_px(ROW_HEIGHT))
        .pl(indent)
        .pr_2()
        .gap_1()
        .text_sm()
        .rounded(cx.theme().radius)
        .when(state.is_selected(), |el| el.bg(cx.theme().list_active))
        .when(state.is_right_clicked() && !state.is_selected(), |el| {
            el.bg(cx.theme().accent)
        })
        .when(!state.is_selected() && !state.is_right_clicked(), |el| {
            el.hover(|el| el.bg(cx.theme().tokens.list_hover))
        })
        .text_color(cx.theme().sidebar_foreground)
        .child(disclosure)
        .child(
            div()
                .w(ui_px(16.))
                .flex_shrink_0()
                .flex()
                .justify_center()
                .when_some(icon, |el, icon| {
                    el.child(
                        Icon::new(icon)
                            .xsmall()
                            .text_color(cx.theme().muted_foreground),
                    )
                }),
        )
        .child(div().flex_1().min_w_0().truncate().child(label))
        .when_some(detail, |el, detail| {
            el.child(
                div()
                    .flex_shrink_0()
                    .max_w(ui_px(120.))
                    .truncate()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(detail),
            )
        })
        .when(loading, |el| {
            el.child(Spinner::new().xsmall().color(cx.theme().muted_foreground))
        })
        .when(error.is_some(), |el| {
            el.child(
                Icon::new(AssetIconName::TriangleAlert)
                    .xsmall()
                    .text_color(cx.theme().danger),
            )
        })
        .when_some(tooltip, |el, tooltip| {
            el.tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        })
        .on_mouse_down(MouseButton::Left, focus)
        .on_mouse_down(MouseButton::Right, {
            let weak = weak.clone();
            move |event, window, cx| {
                let (id, position) = (menu_id.clone(), event.position);
                let _ = weak.update(cx, |_, cx| {
                    cx.defer_in(window, move |this, window, cx| {
                        this.open_catalog_menu(id, position, window, cx)
                    })
                });
            }
        })
        .into_any_element()
}

/// The tooltip of a tree row: its full name, which the row can truncate,
/// then its comment or its error.
fn row_tooltip(name: &str, comment: Option<&str>, error: Option<&str>) -> Option<String> {
    let mut text = name.to_owned();
    for line in [comment, error].into_iter().flatten() {
        text.push('\n');
        text.push_str(line);
    }
    Some(text)
}

/// The row of a connection: the disclosure and the connection button.
fn connection_row(
    id: Uuid,
    row: &ConnectionRow,
    state: TreeEntryState,
    disclosure: Div,
    weak: &WeakEntity<Qrow>,
    cx: &App,
) -> Stateful<Div> {
    let accessibility_label = format!(
        "{}{}{}{}",
        row.name,
        if row.busy { ", running" } else { "" },
        if row.refreshing {
            ", refreshing schemas"
        } else {
            ""
        },
        if row.unread_error {
            ", unread error"
        } else {
            ""
        }
    );
    let foreground = if row.active {
        cx.theme().sidebar_accent_foreground
    } else {
        cx.theme().sidebar_foreground
    };
    // The row draws the hover and the selection, so they also cover the
    // disclosure. The button itself paints no background.
    let button = Button::new(SharedString::from(format!("profile-{id}")))
        .custom(
            ButtonCustomVariant::new(cx)
                .color(transparent_black())
                .hover(transparent_black())
                .active(transparent_black())
                .foreground(foreground)
                .shadow(false),
        )
        .small()
        .h_full()
        .flex_1()
        .min_w_0()
        .accessibility_label(accessibility_label)
        .child(
            h_flex()
                .h_full()
                .w_full()
                .min_w_0()
                .text_base()
                .line_height(relative(1.25))
                .items_center()
                .gap_2()
                .child(
                    gpui_kit::component::Icon::default()
                        .path(crate::assets::SPARK_ICON)
                        .size_4()
                        .flex_shrink_0(),
                )
                .child(div().flex_1().min_w_0().truncate().child(row.name.clone()))
                .when(row.busy || row.refreshing, |el| {
                    el.child(
                        div()
                            // Keep the loading glyph on the header action's centerline.
                            .mr_0p5()
                            .child(Spinner::new().xsmall().color(cx.theme().muted_foreground)),
                    )
                })
                .when(row.unread_error, |el| {
                    el.child(
                        Icon::new(AssetIconName::TriangleAlert)
                            .small()
                            .text_color(cx.theme().danger),
                    )
                }),
        )
        .tooltip(format!("{}\n{}", row.name, row.tooltip))
        .on_click({
            let weak = weak.clone();
            move |_, window, cx| {
                let _ = weak.update(cx, |this, cx| this.switch_profile(id, window, cx));
            }
        });
    h_flex()
        .id(SharedString::from(format!("connection-{id}")))
        .w_full()
        .pr_1()
        .gap_0p5()
        .rounded(cx.theme().radius)
        .text_color(foreground)
        .map(|el| {
            if row.active {
                el.bg(cx.theme().sidebar_accent)
            } else if state.is_selected() {
                el.bg(cx.theme().list_active)
            } else {
                el.hover(|el| el.bg(cx.theme().tokens.list_hover))
            }
        })
        .child(disclosure)
        .child(
            h_flex()
                .h_full()
                .flex_1()
                .min_w_0()
                .child(button)
                // The button selects the connection and focuses the editor.
                // The tree must not also expand the row.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
        )
        .on_mouse_down(MouseButton::Right, {
            let weak = weak.clone();
            move |event, window, cx| {
                let position = event.position;
                let _ = weak.update(cx, |_, cx| {
                    cx.defer_in(window, move |this, window, cx| {
                        this.open_profile_menu(id, position, window, cx)
                    })
                });
            }
        })
}

/// A row that explains the state of its parent node.
#[allow(clippy::too_many_arguments)]
fn notice_row(
    id: &SharedString,
    profile: Uuid,
    text: &str,
    tone: Tone,
    refresh: Option<Scope>,
    weak: &WeakEntity<Qrow>,
    scale: f32,
    cx: &App,
) -> Stateful<Div> {
    let tooltip = (tone == Tone::Error).then(|| text.to_owned());
    h_flex()
        .id(id.clone())
        .w_full()
        .h(px(scale * ROW_HEIGHT))
        .pr_2()
        .gap_1()
        .text_sm()
        .text_color(match tone {
            Tone::Error => cx.theme().danger,
            Tone::Muted | Tone::Loading => cx.theme().muted_foreground,
        })
        .when(tone == Tone::Loading, |el| {
            el.child(Spinner::new().xsmall().color(cx.theme().muted_foreground))
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(text.lines().next().unwrap_or_default().to_owned()),
        )
        .when_some(refresh, |el, scope| {
            let weak = weak.clone();
            el.child(
                Button::new(SharedString::from(format!("{id}{SEPARATOR}refresh")))
                    .ghost()
                    .xsmall()
                    .h(px(scale * 22.))
                    .label("Refresh")
                    .accessibility_label("Refresh")
                    .on_click(move |_, _, cx| {
                        let scope = scope.clone();
                        let _ =
                            weak.update(cx, |this, cx| this.refresh_catalog(profile, scope, cx));
                    }),
            )
        })
        .when_some(tooltip, |el, tooltip| {
            el.tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        })
}

/// A fixed catalog for the demo, which has no server.
fn demo_catalog(profile: Uuid) -> Catalog {
    use crate::catalog::{CatalogColumn, RelationEntry};
    let mut catalog = Catalog::new(&Profile {
        id: profile,
        ..Profile::default()
    });
    let settings = crate::model::CatalogSettings::default();
    let at = crate::catalog::now();
    catalog.apply_schemas(vec!["avia".into(), "finance".into()], &settings, at);
    let table = |name: &str| RelationEntry {
        name: name.into(),
        kind: RelationKind::Table,
        comment: None,
    };
    catalog.apply_relations("avia", None, vec![table("bookings"), table("searches")], at);
    catalog.apply_relations("finance", None, vec![table("payments")], at);
    let column = |name: &str, data_type: &str| CatalogColumn {
        name: name.into(),
        data_type: data_type.into(),
        comment: None,
    };
    catalog.apply_columns(
        "avia",
        None,
        BTreeMap::from([
            (
                "bookings".into(),
                vec![
                    column("booking_id", "BIGINT"),
                    column("gate", "STRING"),
                    column("amount", "DECIMAL(12,2)"),
                    column("booked_at", "TIMESTAMP"),
                ],
            ),
            (
                "searches".into(),
                vec![column("search_id", "BIGINT"), column("origin", "STRING")],
            ),
        ]),
        at,
    );
    catalog
}
