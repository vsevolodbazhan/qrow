//! The connections sidebar. Each connection is the root of a tree of its
//! schemas, relations, and columns.
//!
//! Qrow owns the expansion state and rebuilds the tree items when a catalog,
//! the profile list, the expansion, or the search changes. Each catalog
//! worker loads the cache of its catalog on first use, so the tree does not
//! read files or open sessions at startup. Connections that share a catalog
//! use one worker and show the same data, each in its own tree.

use super::*;
use crate::catalog::{
    Catalog, CatalogConfig, CatalogIdentity, CatalogWorker, Event as CatalogEvent, RelationKind,
    Scope, Seed, Status, catalog_key, qualified_name, quote_identifier,
};
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::base::{
    ElementExt as _, Tree, TreeEntry, TreeEntryState, TreeEvent, TreeItem, TreeState,
};
use gpui_kit::component::{
    Icon, button::ButtonCustomVariant, h_flex, scroll::ScrollableElement as _, spinner::Spinner,
    tooltip::Tooltip, v_flex,
};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
};

/// The most schemas and relations that a search shows.
pub(super) const MAX_SEARCH_MATCHES: usize = 500;
/// Separates the names in a node ID. Names cannot contain it in practice.
const SEPARATOR: char = '\u{1f}';
/// The height of every tree row. The virtual list needs one height for all rows.
const ROW_HEIGHT: f32 = 30.;
/// The most label widths that the tree keeps between frames.
const MAX_LABEL_WIDTHS: usize = 4096;

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

/// The catalog of one connection, or of the connections that share it, as
/// the window knows it.
#[derive(Default)]
struct CatalogConnection {
    worker: Option<CatalogWorker>,
    catalog: Option<Arc<Catalog>>,
    status: Status,
    /// The members with a live session, as the worker last received them.
    live: HashSet<Uuid>,
}

pub(super) struct CatalogTree {
    pub(super) state: Entity<TreeState>,
    pub(super) search: Entity<InputState>,
    expanded: HashSet<SharedString>,
    /// Rows that a search expands but that the user collapsed.
    collapsed: HashSet<SharedString>,
    /// The catalogs by their key: a profile ID or a shared catalog ID.
    connections: HashMap<Uuid, CatalogConnection>,
    /// The catalog key of each connection.
    keys: HashMap<Uuid, Uuid>,
    /// The refreshes of each connection, from the status of its catalog.
    statuses: HashMap<Uuid, Status>,
    nodes: Rc<HashMap<SharedString, Node>>,
    /// The laid-out width of each label in the last frame, to know which
    /// names the rows truncate.
    widths: Rc<RefCell<HashMap<SharedString, Pixels>>>,
    /// The current tooltip of each row, for its open tooltip view.
    tips: Rc<RefCell<HashMap<SharedString, RowTip>>>,
    /// The list around the tree. The tree shows its selection only while
    /// it has the focus.
    focus: FocusHandle,
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
            keys: HashMap::new(),
            statuses: HashMap::new(),
            nodes: Rc::new(HashMap::new()),
            widths: Rc::new(RefCell::new(HashMap::new())),
            tips: Rc::new(RefCell::new(HashMap::new())),
            focus: cx.focus_handle(),
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

    /// The catalog key of `profile`.
    fn key(&self, profile: Uuid) -> Uuid {
        self.keys.get(&profile).copied().unwrap_or(profile)
    }

    fn connection(&self, profile: Uuid) -> Option<&CatalogConnection> {
        self.connections.get(&self.key(profile))
    }

    fn worker(&self, profile: Uuid) -> Option<&CatalogWorker> {
        self.connection(profile)?.worker.as_ref()
    }

    fn catalog(&self, profile: Uuid) -> Option<&Catalog> {
        self.connection(profile)?.catalog.as_deref()
    }

    /// The refreshes that `profile` runs or waits for. The refreshes of
    /// other members of a shared catalog are not included.
    fn status(&self, profile: Uuid) -> Option<&Status> {
        self.statuses.get(&profile)
    }

    /// Give each member of the catalog `key` its part of `status`.
    fn set_status(&mut self, key: Uuid, status: Status) {
        for (member, _) in self.keys.iter().filter(|(_, k)| **k == key) {
            self.statuses.insert(*member, status.of_member(*member));
        }
        if let Some(connection) = self.connections.get_mut(&key) {
            connection.status = status;
        }
    }

    /// The error of the last connection refresh of `profile`, while it runs
    /// no connection refresh. A collapsed connection shows it too. A member
    /// of a shared catalog does not show the errors of the others.
    fn connection_error(&self, profile: Uuid) -> Option<&str> {
        let running = self
            .status(profile)
            .is_some_and(|status| status.active == Some(Scope::Connection));
        let catalog = self.catalog(profile)?;
        catalog
            .error
            .as_deref()
            .filter(|_| !running && catalog.error_member.is_none_or(|member| member == profile))
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
        // A connection without schema browsing is a row without children.
        if !profile.catalog.browses() {
            return item;
        }
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
        } else if let Some(error) = self.tree.connection_error(profile) {
            children.push(self.notice(
                id,
                profile,
                error.to_owned(),
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
                    error: schema.error_for(profile).map(str::to_owned),
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
        let loading = status.includes(&scope) || status.includes(&Scope::Connection);
        match (&node.relations, node.error_for(profile)) {
            (None, _) if loading => {
                children.push(self.notice(id, profile, "Loading…", Tone::Loading, None))
            }
            (None, Some(error)) => {
                children.push(self.notice(id, profile, error, Tone::Error, Some(scope)))
            }
            (None, None) => {
                children.push(self.notice(id, profile, "Not loaded", Tone::Muted, Some(scope)))
            }
            (Some(relations), error) => {
                if let Some(error) = error {
                    children.push(self.notice(id, profile, error, Tone::Error, Some(scope)));
                } else if relations.is_empty() {
                    children.push(self.notice(id, profile, "No relations", Tone::Muted, None));
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
                            error: relation.error_for(profile).map(str::to_owned),
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
        let loading = status.includes(&scope)
            || status.includes(&Scope::Schema(schema.into()))
            || status.includes(&Scope::Connection);
        let mut children = vec![];
        let error = node.error_for(profile);
        if let Some(error) = error {
            children.push(self.notice(id, profile, error, Tone::Error, Some(scope.clone())));
        }
        match &node.columns {
            None if loading => {
                children.push(self.notice(id, profile, "Loading…", Tone::Loading, None))
            }
            None if error.is_none() => {
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
    /// The error of the last connection refresh.
    refresh_error: bool,
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

    /// The catalog of `key` with its current members, or `None` when no
    /// connection browses it.
    fn catalog_config(&self, key: Uuid) -> Option<CatalogConfig> {
        CatalogConfig::of(key, &self.profiles, &self.shared_catalogs)
    }

    /// The cache file of the catalog `key`, or `None` to keep it in memory.
    fn catalog_cache(&self, key: Uuid) -> Option<PathBuf> {
        self.catalog
            .workspace
            .as_deref()
            .filter(|_| self.saver.is_some() && !self.demo)
            .map(|workspace| storage::catalog_path(workspace, key))
    }

    /// Find the catalog key of each connection again.
    pub(super) fn sync_catalog_keys(&mut self) {
        self.catalog.keys = self
            .profiles
            .iter()
            .map(|profile| (profile.id, catalog_key(profile, &self.shared_catalogs)))
            .collect();
    }

    /// Start the catalog worker of `profile` if it does not exist. The worker
    /// loads the cache; it opens a session only for a refresh.
    fn ensure_catalog(&mut self, profile: Uuid) {
        self.ensure_catalog_with_seed(profile, None);
    }

    /// Transfer a private cache before live members can start a refresh.
    fn ensure_catalog_with_seed(&mut self, profile: Uuid, seed: Option<Seed>) {
        let key = self.catalog.key(profile);
        if self.demo {
            self.catalog
                .connections
                .entry(key)
                .or_insert_with(|| CatalogConnection {
                    catalog: Some(Arc::new(demo_catalog(key))),
                    ..CatalogConnection::default()
                });
            return;
        }
        let Some(config) = self.catalog_config(key) else {
            return;
        };
        if let Some(worker) = self
            .catalog
            .connections
            .get(&key)
            .and_then(|connection| connection.worker.as_ref())
        {
            if let Some(seed) = seed {
                worker.seed(seed);
            }
            return;
        }
        let live: HashSet<Uuid> = config
            .members
            .iter()
            .map(|member| member.id)
            .filter(|member| self.catalog_warm(*member))
            .collect();
        let wake = self.wake.clone();
        let worker = CatalogWorker::new(
            config,
            self.catalog_cache(key),
            Arc::new(move || {
                let _ = wake.try_send(());
            }),
            self.credentials.clone(),
        );
        if let Some(seed) = seed {
            worker.seed(seed);
        }
        for member in &live {
            worker.set_live(*member, true);
        }
        let connection = self.catalog.connections.entry(key).or_default();
        connection.live = live;
        connection.worker = Some(worker);
    }

    /// Tell each catalog worker which members have a tab with a live
    /// session. A catalog with automatic refresh gets its worker when a
    /// member first has one, so the worker can refresh a stale catalog.
    pub(super) fn sync_catalog_warmth(&mut self) {
        for index in 0..self.profiles.len() {
            let profile = &self.profiles[index];
            let id = profile.id;
            if !profile.catalog.browses() {
                continue;
            }
            let automatic = crate::model::effective_catalog(profile, &self.shared_catalogs).refresh
                == CatalogRefresh::WhileConnected;
            let warm = self.catalog_warm(id);
            let key = self.catalog.key(id);
            match self
                .catalog
                .connections
                .get_mut(&key)
                .and_then(|connection| {
                    connection
                        .worker
                        .as_ref()
                        .map(|worker| (worker, &mut connection.live))
                }) {
                Some((worker, sent)) => {
                    if sent.contains(&id) != warm {
                        worker.set_live(id, warm);
                        if warm {
                            sent.insert(id);
                        } else {
                            sent.remove(&id);
                        }
                    }
                }
                None if warm && automatic => self.ensure_catalog(id),
                None => {}
            }
        }
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

    /// Ask the catalog worker to read `scope` again with `profile`.
    pub(super) fn refresh_catalog(&mut self, profile: Uuid, scope: Scope, cx: &mut Context<Self>) {
        self.ensure_catalog(profile);
        if let Some(worker) = self.catalog.worker(profile) {
            worker.refresh(profile, scope);
        }
        cx.notify();
    }

    /// Stop the refreshes of `profile`. The refreshes of other members of a
    /// shared catalog continue.
    pub(super) fn cancel_catalog_refresh(&mut self, profile: Uuid) {
        if let Some(worker) = self.catalog.worker(profile) {
            worker.stop(profile);
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
        let mut statuses = Vec::new();
        for (key, connection) in &mut self.catalog.connections {
            let Some(worker) = &connection.worker else {
                continue;
            };
            activities.extend(worker.activities.try_iter());
            for event in worker.events.try_iter() {
                changed = true;
                match event {
                    CatalogEvent::Catalog(catalog) => connection.catalog = Some(catalog),
                    CatalogEvent::Status(status) => statuses.push((*key, status)),
                }
            }
        }
        for (key, status) in statuses {
            self.catalog.set_status(key, status);
        }
        // A refresh belongs to the connection that ran it. Its Logs entries
        // go to each tab of that connection. They do not mark an unread
        // error, because the tree shows refresh errors.
        for (profile, event) in activities {
            for tab in self
                .tabs
                .iter_mut()
                .filter(|tab| tab.saved.profile == Some(profile))
            {
                Self::record_activity(tab, event.clone());
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
                        && let Some(worker) = self.catalog.worker(profile)
                    {
                        worker.refresh_if_unloaded(profile);
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

    /// Bring the catalog workers in line with the saved profiles and shared
    /// catalogs. `previous` is the saved profile before its change. A
    /// catalog that no connection uses any more is deleted with its cache.
    pub(super) fn sync_catalogs(&mut self, previous: Option<&Profile>, cx: &mut Context<Self>) {
        let old: HashSet<Uuid> = self.catalog.keys.values().copied().collect();
        self.sync_catalog_keys();
        let current: HashSet<Uuid> = self.catalog.keys.values().copied().collect();
        let retired: HashSet<Uuid> = old.difference(&current).copied().collect();
        // A connection that makes a new shared catalog brings its schemas.
        let seed = previous.and_then(|previous| {
            let key = self.catalog.key(previous.id);
            if !retired.contains(&previous.id) || old.contains(&key) {
                return None;
            }
            let seed = match self.catalog.connections.get(&previous.id) {
                Some(connection) => connection.catalog.clone().map(Seed::Catalog),
                None => self.catalog_cache(previous.id).map(|path| Seed::File {
                    path,
                    owner: previous.id,
                    identity: CatalogIdentity::of(previous),
                }),
            };
            Some((previous.id, seed?))
        });
        let keys: Vec<Uuid> = self.catalog.connections.keys().copied().collect();
        for key in keys {
            let config = self.catalog_config(key).filter(|_| !retired.contains(&key));
            match config {
                Some(config) => {
                    let connection = self.catalog.connections.get_mut(&key).unwrap();
                    connection
                        .live
                        .retain(|member| config.member(*member).is_some());
                    if let Some(worker) = &connection.worker {
                        worker.configure(config);
                    }
                }
                None => {
                    let connection = self.catalog.connections.remove(&key).unwrap();
                    match connection.worker {
                        Some(worker) if retired.contains(&key) => worker.delete(),
                        // Stop reading schemas. The cache file stays for a later enable.
                        Some(worker) => worker.shutdown(),
                        None => {}
                    }
                }
            }
        }
        for key in &retired {
            let seeded = matches!(&seed, Some((_, Seed::File { owner, .. })) if owner == key);
            if !seeded {
                self.delete_catalog_cache(*key);
            }
        }
        if let Some((member, seed)) = seed {
            let path = match &seed {
                Seed::File { path, .. } => Some(path.clone()),
                Seed::Catalog(_) => None,
            };
            self.ensure_catalog_with_seed(member, Some(seed));
            if self.catalog.worker(member).is_none()
                && let Some(path) = path
            {
                std::thread::spawn(move || storage::delete_catalog(&path));
            }
        }
        // Statuses of connections that left a catalog no longer apply.
        let keys = self.catalog.keys.clone();
        let connections = &self.catalog.connections;
        self.catalog.statuses = keys
            .iter()
            .filter_map(|(member, key)| {
                let status = &connections.get(key)?.status;
                Some((*member, status.of_member(*member)))
            })
            .collect();
        for index in 0..self.profiles.len() {
            let profile = &self.profiles[index];
            let id = profile.id;
            if !profile.catalog.browses() {
                // The row has no children now, so it is not expanded. A later
                // enable then starts with a collapsed connection.
                let connection = connection_id(id);
                let prefixes = Self::descendant_prefixes(id, None);
                let below = |row: &SharedString| {
                    *row == connection
                        || prefixes
                            .iter()
                            .any(|prefix| row.starts_with(prefix.as_str()))
                };
                self.catalog.expanded.retain(|row| !below(row));
                self.catalog.collapsed.retain(|row| !below(row));
            } else if self.catalog.expanded.contains(&connection_id(id)) {
                // An expanded connection shows its cache at once.
                self.ensure_catalog(id);
            }
        }
        self.rebuild_catalog_tree(cx);
    }

    /// Delete the cache file of the catalog `key`, off the window thread.
    fn delete_catalog_cache(&self, key: Uuid) {
        if let Some(path) = self.catalog_cache(key) {
            std::thread::spawn(move || storage::delete_catalog(&path));
        }
    }

    /// Inserts `text` at the cursor of the active query tab.
    fn insert_into_editor(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.tabs[self.active].input.clone();
        input.update(cx, |state, cx| {
            state.replace(text, window, cx);
            state.focus(window, cx);
        });
    }

    /// The ID prefixes of the rows below a connection, or below one of its
    /// schemas.
    fn descendant_prefixes(profile: Uuid, schema: Option<&str>) -> Vec<String> {
        match schema {
            None => vec![
                format!("s{SEPARATOR}{profile}{SEPARATOR}"),
                format!("r{SEPARATOR}{profile}{SEPARATOR}"),
            ],
            Some(schema) => vec![format!(
                "r{SEPARATOR}{profile}{SEPARATOR}{schema}{SEPARATOR}"
            )],
        }
    }

    /// Whether Collapse has rows to collapse below the connection or
    /// schema. The built tree knows which rows are open, also the rows that
    /// a search opens.
    pub(super) fn has_expanded_descendants(
        &self,
        profile: Uuid,
        schema: Option<&str>,
        cx: &App,
    ) -> bool {
        let prefixes = Self::descendant_prefixes(profile, schema);
        let state = self.catalog.state.read(cx);
        (0..).map_while(|ix| state.entry(ix)).any(|entry| {
            entry.is_expanded()
                && prefixes
                    .iter()
                    .any(|prefix| entry.item().id.starts_with(prefix.as_str()))
        })
    }

    /// Collapses every row below the connection, or below one of its schemas.
    /// The row itself stays expanded. During a search, this also collapses
    /// the schemas that the search expands.
    pub(super) fn collapse_catalog(
        &mut self,
        profile: Uuid,
        schema: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let prefixes = Self::descendant_prefixes(profile, schema);
        self.catalog.expanded.retain(|id| {
            !prefixes
                .iter()
                .any(|prefix| id.starts_with(prefix.as_str()))
        });
        let searching = !self.catalog.search.read(cx).value().trim().is_empty();
        if searching && schema.is_none() {
            let schemas: Vec<SharedString> = self
                .catalog
                .catalog(profile)
                .map(|catalog| {
                    catalog
                        .schemas
                        .keys()
                        .map(|name| schema_id(profile, name))
                        .collect()
                })
                .unwrap_or_default();
            self.catalog.collapsed.extend(schemas);
        }
        self.rebuild_catalog_tree(cx);
        cx.notify();
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
        let has_commands = scope.is_some();
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
        let collapse = match &node {
            Node::Schema { name, .. } => {
                let has_expanded = self.has_expanded_descendants(profile, Some(name), cx);
                let schema = name.clone();
                Some((
                    cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.collapse_catalog(profile, Some(&schema), cx)
                    }),
                    has_expanded,
                ))
            }
            _ => None,
        };
        self.open_context_menu(
            position,
            move |menu, _, _| {
                let menu = match refresh {
                    Some(refresh) => menu.item(
                        PopupMenuItem::new("Refresh")
                            .on_click(refresh)
                            .disabled(refreshing),
                    ),
                    None => menu,
                };
                let menu = match collapse {
                    Some((collapse, has_expanded)) => menu.item(
                        PopupMenuItem::new("Collapse")
                            .on_click(collapse)
                            .disabled(!has_expanded),
                    ),
                    None => menu,
                };
                let menu = if has_commands { menu.separator() } else { menu };
                menu.item(PopupMenuItem::new(copy_label).on_click(copy))
                    .item(PopupMenuItem::new("Insert into Editor").on_click(insert))
            },
            window,
            cx,
        );
    }

    /// The Connections sidebar: its header, the search, and the tree.
    pub(super) fn connections(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let action_size = self.ui_px(28.);
        let active = self.active_profile();
        let rows: Rc<HashMap<Uuid, ConnectionRow>> = Rc::new(
            self.profiles
                .iter()
                .map(|profile| {
                    let id = profile.id;
                    let refresh_error = self.catalog.connection_error(id);
                    (
                        id,
                        ConnectionRow {
                            name: profile.name.clone(),
                            tooltip: workspace_view::connection_tooltip(profile, refresh_error),
                            refresh_error: refresh_error.is_some(),
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
        let context = Rc::new(RowContext {
            nodes: self.catalog.nodes.clone(),
            rows,
            weak: cx.weak_entity(),
            tree: self.catalog.state.clone(),
            scale: self.settings.ui_scale,
            widths: self.catalog.widths.clone(),
            tips: self.catalog.tips.clone(),
            // A tooltip would cover the open menu.
            menu_open: self.menu.is_some(),
            focused: self.catalog.focus.contains_focused(window, cx),
        });
        let tree = self.catalog.state.clone();
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
                    .track_focus(&self.catalog.focus)
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
                                render_entry(entry, state, &context, window, cx)
                            })
                            .list_style(StyleRefinement::default().size_full())
                            .size_full(),
                    )
                    .vertical_scrollbar(&self.catalog.state.read(cx).scroll_handle().clone()),
            )
    }
}

/// What the rows of one frame share.
struct RowContext {
    nodes: Rc<HashMap<SharedString, Node>>,
    rows: Rc<HashMap<Uuid, ConnectionRow>>,
    weak: WeakEntity<Qrow>,
    tree: Entity<TreeState>,
    scale: f32,
    widths: Rc<RefCell<HashMap<SharedString, Pixels>>>,
    tips: Rc<RefCell<HashMap<SharedString, RowTip>>>,
    menu_open: bool,
    /// Whether the tree has the focus. A selection without it is not shown.
    focused: bool,
}

impl RowContext {
    /// Records the current tooltip of the row `id` for its open tooltip view.
    fn record_tip(&self, id: &SharedString, text: String, truncation: Option<Truncation>) {
        let mut tips = self.tips.borrow_mut();
        if tips.len() >= MAX_LABEL_WIDTHS && !tips.contains_key(id) {
            tips.clear();
        }
        tips.insert(id.clone(), RowTip { text, truncation });
    }

    /// A tooltip that follows the tooltip that the row `id` records.
    fn live_tooltip(
        &self,
        id: &SharedString,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let (id, tips, widths) = (id.clone(), self.tips.clone(), self.widths.clone());
        move |_, cx| {
            let (id, tips, widths) = (id.clone(), tips.clone(), widths.clone());
            cx.new(|_| LiveTooltip {
                id,
                tips,
                widths,
                shown: None,
            })
            .into()
        }
    }

    /// Records the laid-out width of the label `key` during prepaint.
    fn record_width(
        &self,
        key: SharedString,
    ) -> impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) + use<> {
        let widths = self.widths.clone();
        move |bounds, _, _| {
            let mut widths = widths.borrow_mut();
            if widths.len() >= MAX_LABEL_WIDTHS && !widths.contains_key(&key) {
                widths.clear();
            }
            widths.insert(key, bounds.size.width);
        }
    }
}

/// The labels of a row whose cut text a tooltip can show.
struct Truncation {
    /// Each label: its width key, its text, and its font size in rems.
    labels: Vec<(SharedString, String, f32)>,
}

impl Truncation {
    /// Whether the last laid-out frame cut one of the labels.
    fn is_truncated(
        &self,
        widths: &HashMap<SharedString, Pixels>,
        window: &Window,
        cx: &App,
    ) -> bool {
        self.labels.iter().any(|(key, text, rems)| {
            let Some(width) = widths.get(key).copied() else {
                return false;
            };
            let run = TextRun {
                len: text.len(),
                font: font(cx.theme().font_family.clone()),
                color: cx.theme().foreground,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let size = window.rem_size() * *rems;
            let shaped = window.text_system().shape_line(
                SharedString::from(text.clone()),
                size,
                &[run],
                None,
            );
            // Half a pixel absorbs rounding of the laid-out width.
            shaped.width > width + px(0.5)
        })
    }
}

/// The current tooltip of a row. The row writes it in each frame.
struct RowTip {
    text: String,
    /// `None` when the tooltip has a comment or an error, so it always shows.
    truncation: Option<Truncation>,
}

/// The tooltip that GPUI opens for a row. GPUI keeps an open tooltip while
/// the pointer stays on its row, so the view reads the row's current tooltip
/// in each frame: an error that arrives during a hover shows at once, and a
/// name that fits shows nothing.
struct LiveTooltip {
    id: SharedString,
    tips: Rc<RefCell<HashMap<SharedString, RowTip>>>,
    widths: Rc<RefCell<HashMap<SharedString, Pixels>>>,
    /// The tooltip on screen and its text. It changes only with the text.
    shown: Option<(String, AnyView)>,
}

impl Render for LiveTooltip {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.tips.borrow().get(&self.id).and_then(|tip| {
            tip.truncation
                .as_ref()
                .is_none_or(|truncation| truncation.is_truncated(&self.widths.borrow(), window, cx))
                .then(|| tip.text.clone())
        });
        match text {
            Some(text) => {
                if self.shown.as_ref().is_none_or(|(shown, _)| *shown != text) {
                    let view = row_tooltip_view(&text, window, cx);
                    self.shown = Some((text, view));
                }
                div().children(self.shown.as_ref().map(|(_, view)| view.clone()))
            }
            None => {
                self.shown = None;
                div()
            }
        }
    }
}

/// The tree row of `entry`. The highlight is inset in the row, so the
/// highlights of adjacent rows keep a gap.
fn render_entry(
    entry: &TreeEntry,
    state: TreeEntryState,
    context: &RowContext,
    _: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let RowContext {
        nodes,
        rows,
        weak,
        tree,
        scale,
        menu_open,
        ..
    } = context;
    let scale = *scale;
    let ui_px = |value: f32| px(scale * value);
    // The highlights follow the focus and the menu: a row stays selected in
    // the tree, but it shows that only while the tree has the focus.
    let selected = state.is_selected() && context.focused;
    let right_clicked = state.is_right_clicked() && *menu_open;
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
        return div()
            .w_full()
            .h(ui_px(ROW_HEIGHT))
            .py_0p5()
            .child(
                connection_row(&id, *profile, row, selected, disclosure, context, cx)
                    .pl(ui_px(4.))
                    .h_full()
                    .on_mouse_down(MouseButton::Left, focus),
            )
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
                .pl(indent)
                .into_any_element();
        }
        Node::Connection(_) => unreachable!(),
    };
    // Show the tooltip only when it adds text: a name that the row cuts, a
    // comment, or an error.
    let label_key = child_id(&id, "label");
    let detail_key = child_id(&id, "detail");
    let has_comment = match node {
        Node::Relation { comment, .. } | Node::Column { comment, .. } => comment.is_some(),
        _ => false,
    };
    let truncation = (!has_comment && error.is_none()).then(|| Truncation {
        labels: [(label_key.clone(), label.to_string(), 0.875)]
            .into_iter()
            .chain(
                detail
                    .clone()
                    .map(|detail| (detail_key.clone(), detail, 0.75)),
            )
            .collect(),
    });
    if let Some(text) = &tooltip {
        context.record_tip(&id, text.clone(), truncation);
    }
    let menu_id = id.clone();
    let row = h_flex()
        .id(id.clone())
        .size_full()
        .pl(indent)
        .pr_2()
        .gap_1()
        .text_sm()
        .rounded(cx.theme().radius)
        .when(selected, |el| el.bg(cx.theme().list_active))
        .when(right_clicked && !selected, |el| el.bg(cx.theme().accent))
        .when(!selected && !right_clicked, |el| {
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
        .child(
            div()
                .relative()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(label)
                .on_prepaint(context.record_width(label_key)),
        )
        .when_some(detail, |el, detail| {
            el.child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .max_w(ui_px(120.))
                    .truncate()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(detail)
                    .on_prepaint(context.record_width(detail_key)),
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
        .when(tooltip.is_some() && !*menu_open, |el| {
            el.tooltip(context.live_tooltip(&id))
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
        });
    div()
        .w_full()
        .h(ui_px(ROW_HEIGHT))
        .py_0p5()
        .child(row)
        .into_any_element()
}

/// The tooltip view of a tree row. Its text is an observed element, so tests
/// can find the tooltip that a hover opens.
fn row_tooltip_view(text: &str, window: &mut Window, cx: &mut App) -> AnyView {
    let text = SharedString::from(text.to_owned());
    Tooltip::element(move |_, _| {
        div()
            .id("catalog-tooltip")
            .test_support()
            .aria_label(text.clone())
            .child(text.clone())
    })
    .build(window, cx)
}

/// The longest error summary in a tooltip, in characters.
const ERROR_SUMMARY_CHARS: usize = 200;

/// The tooltip text of a refresh error: its first line, cut to
/// [`ERROR_SUMMARY_CHARS`]. A server error can have a long stack trace, so
/// the tooltip points to Logs, which keep the full error.
pub(super) fn error_summary(error: &str) -> String {
    let line = error
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    let mut summary: String = line.chars().take(ERROR_SUMMARY_CHARS).collect();
    if summary.len() < line.len() {
        summary.push('…');
    }
    summary.push_str("\nThe Logs of each tab of the connection show the full error.");
    summary
}

/// The tooltip of a tree row: its full name, which the row can truncate,
/// then its comment or its error.
fn row_tooltip(name: &str, comment: Option<&str>, error: Option<&str>) -> Option<String> {
    let mut text = name.to_owned();
    for line in [comment.map(str::to_owned), error.map(error_summary)]
        .into_iter()
        .flatten()
    {
        text.push('\n');
        text.push_str(&line);
    }
    Some(text)
}

/// A slot for one status icon at the end of a connection row. The header and
/// the list have the same side padding, so a slot as wide as the header's New
/// Connection button at the row end has the same centerline.
fn status_slot(id: String, width: Pixels) -> impl IntoElement + ParentElement {
    div()
        .id(SharedString::from(id))
        .test_support()
        .w(width)
        .flex_shrink_0()
        .flex()
        .justify_center()
}

/// The row of a connection: the disclosure and the connection button.
fn connection_row(
    entry: &SharedString,
    id: Uuid,
    row: &ConnectionRow,
    selected: bool,
    disclosure: Div,
    context: &RowContext,
    cx: &App,
) -> Stateful<Div> {
    let (menu_open, weak) = (context.menu_open, &context.weak);
    // As wide as the header's New Connection button.
    let slot_width = px(context.scale * 28.);
    let has_status = row.busy || row.refreshing || row.unread_error || row.refresh_error;
    // The tooltip can change while it is open: a refresh error arrives or
    // goes away.
    context.record_tip(entry, row.tooltip.clone(), None);
    let accessibility_label = format!(
        "{}{}{}{}{}",
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
        },
        if row.refresh_error {
            ", schema refresh error"
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
        // A status slot ends at the row end. Without one, the name keeps a
        // margin from the highlight edge.
        .pr_0()
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
                .when(!has_status, |el| el.pr_3())
                .child(
                    gpui_kit::component::Icon::default()
                        .path(crate::assets::SPARK_ICON)
                        .size_4()
                        .flex_shrink_0(),
                )
                .child(div().flex_1().min_w_0().truncate().child(row.name.clone()))
                .when(row.busy || row.refreshing, |el| {
                    el.child(
                        status_slot(format!("connection-busy-{id}"), slot_width)
                            .child(Spinner::new().xsmall().color(cx.theme().muted_foreground)),
                    )
                })
                // One warning icon covers an unread query error and a
                // schema refresh error. The tooltip and the label tell which.
                .when(row.unread_error || row.refresh_error, |el| {
                    el.child(
                        status_slot(format!("connection-error-{id}"), slot_width).child(
                            Icon::new(AssetIconName::TriangleAlert)
                                .small()
                                .text_color(cx.theme().danger),
                        ),
                    )
                }),
        )
        .on_click({
            let weak = weak.clone();
            move |_, window, cx| {
                let _ = weak.update(cx, |this, cx| this.switch_profile(id, window, cx));
            }
        });
    h_flex()
        .id(SharedString::from(format!("connection-{id}")))
        .when(!menu_open, |el| el.tooltip(context.live_tooltip(entry)))
        .w_full()
        .gap_0p5()
        .rounded(cx.theme().radius)
        .text_color(foreground)
        .map(|el| {
            if row.active {
                el.bg(cx.theme().sidebar_accent)
            } else if selected {
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
    let tooltip = (tone == Tone::Error).then(|| error_summary(text));
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
        // The columns of the other rows: the disclosure, the icon, and the
        // label. The spinner takes the place of an icon.
        .child(div().w(px(scale * 16.)).flex_shrink_0())
        .child(
            div()
                .w(px(scale * 16.))
                .flex_shrink_0()
                .flex()
                .justify_center()
                .when(tone == Tone::Loading, |el| {
                    el.child(Spinner::new().xsmall().color(cx.theme().muted_foreground))
                }),
        )
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
            el.tooltip(move |window, cx| row_tooltip_view(&tooltip, window, cx))
        })
}

/// A fixed catalog for the demo, which has no server.
fn demo_catalog(profile: Uuid) -> Catalog {
    use crate::catalog::{CatalogColumn, RelationEntry};
    let mut catalog = Catalog::empty(profile, None);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn an_error_summary_keeps_the_first_line_and_points_to_logs() {
        let trace =
            "\n  Could not open the session: host unreachable\n\tat org.apache.Foo(Foo.java:1)";
        assert_eq!(
            error_summary(trace),
            "Could not open the session: host unreachable\nThe Logs of each tab of the connection show the full error."
        );
        let long = "x".repeat(ERROR_SUMMARY_CHARS + 1);
        let summary = error_summary(&long);
        assert!(summary.starts_with(&format!("{}…\n", "x".repeat(ERROR_SUMMARY_CHARS))));
    }
}
