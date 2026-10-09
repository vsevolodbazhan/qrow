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
    Scope, Seed, Status, catalog_key, qualified_name,
};
use gpui_kit::assets::IconName as AssetIconName;
use gpui_kit::base::FocusableExt as _;
use gpui_kit::base::{
    ElementExt as _, Tree, TreeEntry, TreeEntryState, TreeEvent, TreeItem, TreeState,
};
use gpui_kit::component::{
    Icon,
    button::ButtonCustomVariant,
    h_flex,
    scroll::ScrollableElement as _,
    spinner::Spinner,
    text::{TextView, TextViewState, TextViewStyle},
    tooltip::Tooltip,
    v_flex,
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
/// The gap between the columns of a tree row, in pixels at UI scale 1.
const ROW_GAP: f32 = 4.;
/// The trailing lane shared by catalog status icons and the header action.
const STATUS_SLOT_WIDTH: f32 = 28.;
/// The opacity of the accent fill of the current connection row.
const CURRENT_CONNECTION_FILL: f32 = 0.18;
/// The most label widths that the tree keeps between frames.
const MAX_LABEL_WIDTHS: usize = 4096;

#[derive(Clone)]
struct DragConnection {
    id: Uuid,
    name: String,
    database_type: crate::model::DatabaseType,
}

impl Render for DragConnection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_2()
            .px_3()
            .py_1()
            .bg(cx.theme().popover)
            .text_color(cx.theme().popover_foreground)
            .border_1()
            .border_color(cx.theme().border)
            .rounded(cx.theme().radius)
            .child(
                Icon::default()
                    .path(crate::assets::connection_icon(self.database_type))
                    .size_4(),
            )
            .child(self.name.clone())
    }
}

fn connection_drop_target(id: Uuid, after: bool, weak: WeakEntity<Qrow>) -> impl IntoElement {
    div()
        .id(SharedString::from(format!(
            "connection-drop-{}-{id}",
            if after { "after" } else { "before" }
        )))
        .test_support()
        .absolute()
        .left_0()
        .w_full()
        .h(relative(0.5))
        .map(|el| if after { el.bottom_0() } else { el.top_0() })
        .drag_over::<DragConnection>(move |style, drag, _, cx| {
            if drag.id == id {
                return style;
            }
            let style = style.border_color(cx.theme().primary);
            if after {
                style.border_b_2()
            } else {
                style.border_t_2()
            }
        })
        .on_drop(move |drag: &DragConnection, _, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.reorder_connection(drag.id, id, after, cx)
            });
        })
}

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
        /// The dbt resource that builds the relation.
        dbt: Option<super::dbt::DbtBadge>,
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
    fn names(&self, kind: crate::model::DatabaseType, catalog: &str) -> Option<(String, String)> {
        match self {
            Node::Schema { name, .. } => Some((name.clone(), kind.quote_identifier(name))),
            Node::Relation { schema, name, .. } => {
                let qualified = format!(
                    "{}.{}",
                    kind.quote_identifier(schema),
                    kind.quote_identifier(name)
                );
                let qualified = if kind == crate::model::DatabaseType::Trino {
                    format!("{}.{}", kind.quote_identifier(catalog), qualified)
                } else {
                    qualified
                };
                Some((qualified.clone(), qualified))
            }
            Node::Column { name, .. } => Some((name.clone(), kind.quote_identifier(name))),
            Node::Connection(_) | Node::Notice { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tone {
    Muted,
    Loading,
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
    /// How many times the worker reported that it has no refresh.
    idle_reports: u64,
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

    pub(super) fn catalog(&self, profile: Uuid) -> Option<&Catalog> {
        self.connection(profile)?.catalog.as_deref()
    }

    /// How many times the catalog worker of `profile` reported that it has no
    /// refresh. A refresh that Qrow asked for ended when this number grows.
    pub(super) fn idle_reports(&self, profile: Uuid) -> u64 {
        self.connection(profile)
            .map_or(0, |connection| connection.idle_reports)
    }

    /// Whether a refresh in progress or waiting, of any connection of the
    /// catalog of `profile`, reads `scope`.
    pub(super) fn reads(&self, profile: Uuid, scope: &Scope) -> bool {
        self.connection(profile).is_some_and(|connection| {
            let status = &connection.status;
            status
                .active
                .iter()
                .chain(status.queued.iter().map(|request| &request.scope))
                .any(|reading| reading.covers(scope))
        })
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
            if status.is_idle() {
                connection.idle_reports += 1;
            }
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
        self.catalog(profile)?
            .error_for(profile)
            .filter(|_| !running)
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
    /// The dbt resources of the tables of each connection.
    dbt: &'a HashMap<Uuid, super::dbt::DbtLookup>,
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
        let id = child_id(parent, "notice");
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

    fn matches_relation(&self, schema: &str, name: &str) -> bool {
        self.matches(name)
            || self.matches(&format!("{schema}.{name}"))
            || self.matches(&qualified_name(schema, name))
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
                        || schema.relations.as_ref().is_some_and(|relations| {
                            relations.keys().any(|r| self.matches_relation(name, r))
                        })
                })
            });
        let expanded = if searching {
            has_matches && self.search_expands(&id) || self.is_expanded(&id) && !has_matches
        } else {
            self.is_expanded(&id)
        };
        self.folder(item, expanded, profile.id, |builder| {
            builder.connection_children(profile.id, &id, has_matches)
        })
    }

    fn connection_children(
        &mut self,
        profile: Uuid,
        id: &SharedString,
        has_matches: bool,
    ) -> Vec<TreeItem> {
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
        } else if self.tree.connection_error(profile).is_some() {
            // The connection status opens Activity with the refresh error.
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
                && schema.relations.as_ref().is_some_and(|relations| {
                    relations.keys().any(|r| self.matches_relation(name, r))
                });
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
        if searching && children.is_empty() {
            let text = if has_matches {
                "Search limit reached"
            } else {
                "No matches"
            };
            children.push(self.notice(id, profile, text, Tone::Muted, None));
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
                children.push(self.notice(id, profile, "Loading relations…", Tone::Loading, None))
            }
            // The status dot opens Activity; keep failures out of the child list.
            (None, Some(_)) => {}
            (None, None) => {
                children.push(self.notice(id, profile, "Not loaded", Tone::Muted, Some(scope)))
            }
            (Some(relations), error) => {
                if relations.is_empty() && error.is_none() {
                    children.push(self.notice(id, profile, "No relations", Tone::Muted, None));
                }
                for (name, relation) in relations {
                    if filtered && !self.matches_relation(schema, name) {
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
                            dbt: self.dbt.get(&profile).and_then(|lookup| {
                                lookup.badge(schema, name, relation.comment.as_deref())
                            }),
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
                    let data_type = display_type(&column.data_type);
                    children.push(self.add(
                        item_id,
                        format!("{} {data_type}", column.name),
                        Node::Column {
                            profile,
                            schema: schema.into(),
                            relation: relation.into(),
                            name: column.name.clone(),
                            data_type,
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
    database_type: crate::model::DatabaseType,
    name: String,
    tooltip: StatusTooltip,
    running: bool,
    connecting: bool,
    refreshing: bool,
    unread_error: bool,
    /// The error of the last connection refresh.
    refresh_error: bool,
    unread_success: bool,
    connected: bool,
    status: Option<DotStatus>,
    assistant_states: Vec<assistant_view::ThreadStatus>,
    active: bool,
}

impl Qrow {
    fn reorder_connection(&mut self, id: Uuid, target: Uuid, after: bool, cx: &mut Context<Self>) {
        if id == target {
            return;
        }
        let Some(source_ix) = self.profiles.iter().position(|profile| profile.id == id) else {
            return;
        };
        let Some(target_ix) = self
            .profiles
            .iter()
            .position(|profile| profile.id == target)
        else {
            return;
        };
        let destination_ix = target_ix + usize::from(after) - usize::from(source_ix < target_ix);
        if source_ix == destination_ix {
            return;
        }
        let profile = self.profiles.remove(source_ix);
        self.profiles.insert(destination_ix, profile);
        self.rebuild_catalog_tree(cx);
        self.changed(cx);
    }

    pub(super) fn move_connection(&mut self, id: Uuid, down: bool, cx: &mut Context<Self>) {
        let Some(ix) = self.profiles.iter().position(|profile| profile.id == id) else {
            return;
        };
        let target_ix = if down {
            ix.checked_add(1)
        } else {
            ix.checked_sub(1)
        };
        if let Some(target) = target_ix
            .and_then(|ix| self.profiles.get(ix))
            .map(|profile| profile.id)
        {
            self.reorder_connection(id, target, down, cx);
            if let Some(ix) = self.catalog.state.read(cx).index_of(&connection_id(id)) {
                self.catalog
                    .state
                    .read(cx)
                    .scroll_handle()
                    .scroll_to_item(ix, ScrollStrategy::Nearest);
            }
        }
    }

    fn move_selected_connection(&mut self, down: bool, cx: &mut Context<Self>) {
        let selected = self
            .catalog
            .state
            .read(cx)
            .selected_item()
            .map(|item| item.id.clone());
        if let Some(Node::Connection(id)) = selected.as_ref().and_then(|id| self.catalog.node(id)) {
            self.move_connection(*id, down, cx);
        }
    }

    /// Rebuild the tree items. Keeps the selected row when it still exists.
    pub(super) fn rebuild_catalog_tree(&mut self, cx: &mut Context<Self>) {
        let search = self.catalog.search.read(cx).value().trim().to_lowercase();
        let dbt = self.dbt_lookups();
        let mut builder = Builder {
            tree: &self.catalog,
            dbt: &dbt,
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
    pub(super) fn ensure_catalog(&mut self, profile: Uuid) {
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
                    catalog: Some(Arc::new(super::demo::catalog(key))),
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
            self.connector.clone(),
            self.credential_provider(),
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
        for tab in &self.tabs {
            if let Some(worker) = &tab.worker {
                let guard = tab.worker_profile.and_then(|member| {
                    self.catalog
                        .worker(member)
                        .map(|catalog| catalog.idle_guard(member))
                });
                worker.set_idle_guard(guard);
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
    pub(super) fn catalog_warm(&self, profile: Uuid) -> bool {
        self.tabs
            .iter()
            .any(|tab| tab.worker_profile == Some(profile) && tab.connected)
    }

    /// Applies what the catalog workers sent since the last tick.
    pub(super) fn drain_catalogs(&mut self, cx: &mut Context<Self>) -> bool {
        let mut changed = false;
        let mut logs = Vec::new();
        let mut statuses = Vec::new();
        for (key, connection) in &mut self.catalog.connections {
            let Some(worker) = &connection.worker else {
                continue;
            };
            logs.extend(worker.logs.try_iter());
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
        // A refresh belongs to the connection that ran it, so its entries
        // go to the Activity of that connection.
        for (profile, event) in logs {
            self.record_activity(profile, crate::activity::from_refresh(event), cx);
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

    /// Selects the row of the connection `profile` and focuses the tree.
    pub(super) fn select_catalog_connection(
        &mut self,
        profile: Uuid,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.catalog.state.update(cx, |state, cx| {
            let ix = state.index_of(&connection_id(profile));
            state.set_selected_index(ix, cx);
            state.focus(window, cx);
        });
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

    fn catalog_node_names(&self, node: &Node) -> Option<(String, String)> {
        let id = match node {
            Node::Schema { profile, .. }
            | Node::Relation { profile, .. }
            | Node::Column { profile, .. } => *profile,
            _ => return None,
        };
        let profile = self.profiles.iter().find(|profile| profile.id == id)?;
        node.names(profile.database_type, &profile.database)
    }

    /// The names of the row that the tree selects.
    fn selected_catalog_names(&self, cx: &App) -> Option<(String, String)> {
        let state = self.catalog.state.read(cx);
        self.catalog_node_names(self.catalog.node(&state.selected_item()?.id)?)
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
        let Some((name, insert)) = self.catalog_node_names(&node) else {
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
            "Copy qualified name"
        } else {
            "Copy name"
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
        let details = match &node {
            Node::Relation {
                profile,
                dbt: Some(dbt),
                ..
            } => {
                let (profile, unique_id) = (*profile, dbt.unique_id.clone());
                Some(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.open_dbt_details(profile, &unique_id, window, cx)
                }))
            }
            _ => None,
        };
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
                let menu = menu
                    .item(PopupMenuItem::new(copy_label).on_click(copy))
                    .item(PopupMenuItem::new("Insert into editor").on_click(insert));
                match details {
                    Some(details) => menu
                        .separator()
                        .item(PopupMenuItem::new("Show dbt details").on_click(details)),
                    None => menu,
                }
            },
            window,
            cx,
        );
    }

    /// The status shown by a connection row and summarized by Activity.
    pub(super) fn connection_dot_status(&self, id: Uuid, cx: &App) -> Option<DotStatus> {
        let query_status = self
            .tabs
            .iter()
            .filter(|tab| tab.saved.profile == Some(id))
            .filter_map(|tab| {
                tab.dot_status().max(
                    self.settings
                        .assistant
                        .enabled
                        .then(|| self.tab_assistant_status(tab.saved.id))
                        .flatten()
                        .and_then(assistant_view::ThreadStatus::dot_status),
                )
            })
            .max();
        query_status.max(
            if self.activity.read(cx).activity().unseen_errors_of(id) > 0 {
                Some(DotStatus::Error)
            } else if self.catalog.is_refreshing(id) || self.external_authentication_pending(id) {
                Some(DotStatus::Working)
            } else {
                None
            },
        )
    }

    /// The Connections sidebar: its header, the search, and the tree.
    pub(super) fn connections(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let action_size = self.ui_px(STATUS_SLOT_WIDTH);
        let active = self.active_profile();
        let rows: Rc<HashMap<Uuid, ConnectionRow>> = Rc::new(
            self.profiles
                .iter()
                .map(|profile| {
                    let id = profile.id;
                    let refresh_error = self.catalog.connection_error(id);
                    let status = self.connection_dot_status(id, cx);
                    let connected = self
                        .tabs
                        .iter()
                        .any(|tab| tab.worker_profile == Some(id) && tab.connected);
                    let unread_success = self
                        .tabs
                        .iter()
                        .any(|tab| tab.saved.profile == Some(id) && tab.panel.has_unread_success());
                    let assistant_states: Vec<_> = self
                        .tabs
                        .iter()
                        .filter(|tab| tab.saved.profile == Some(id))
                        .filter_map(|tab| {
                            self.settings
                                .assistant
                                .enabled
                                .then(|| self.tab_assistant_status(tab.saved.id))
                                .flatten()
                        })
                        .collect();
                    let tooltip = StatusTooltip::new(
                        profile.name.clone(),
                        match status {
                            Some(DotStatus::Connected) => "Idle",
                            Some(DotStatus::Connecting) => "Connecting",
                            Some(DotStatus::Working) => "In use",
                            Some(DotStatus::Ready) if unread_success => "Unread result",
                            Some(DotStatus::Ready) => "Unread reply",
                            Some(DotStatus::Error) => "Unread error",
                            Some(DotStatus::Attention) => "Needs approval",
                            None => "Disconnected",
                        },
                    )
                    .metadata("Host", profile.host.clone())
                    .metadata("User", profile.username.clone());
                    (
                        id,
                        ConnectionRow {
                            database_type: profile.database_type,
                            name: profile.name.clone(),
                            tooltip,
                            connected,
                            unread_success,
                            status,
                            assistant_states,
                            refresh_error: refresh_error.is_some(),
                            running: self.tabs.iter().any(|tab| {
                                tab.worker_profile == Some(id) && tab.busy && tab.connected
                            }),
                            connecting: self.external_authentication_pending(id)
                                || self.tabs.iter().any(|tab| {
                                    tab.worker_profile == Some(id) && tab.busy && !tab.connected
                                }),
                            refreshing: self.catalog.is_refreshing(id),
                            unread_error: self.activity.read(cx).activity().unseen_errors_of(id)
                                > 0
                                || self.tabs.iter().any(|tab| {
                                    tab.saved.profile == Some(id) && tab.panel.unread_error
                                }),
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
                self.sidebar_header("Connections", cx).child(
                    Button::new("add-connection")
                        .ghost()
                        .small()
                        .w(action_size)
                        .h(action_size)
                        .flex_shrink_0()
                        .icon(IconName::Plus)
                        .accessibility_label("New connection")
                        .tooltip("New connection…")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.edit_profile(Profile::default(), true, window, cx)
                        })),
                ),
            )
            .when(!self.profiles.is_empty(), |el| {
                el.child(
                    div().p_2().flex_shrink_0().child(
                        Input::new(&self.catalog.search)
                            .focus_ring(false)
                            .small()
                            .w_full()
                            .aria_label("Search tables"),
                    ),
                )
            })
            .child(
                div()
                    .id("connections-list")
                    .test_support()
                    .track_focus(&self.catalog.focus)
                    .key_context("Connections")
                    .on_action(cx.listener(|this, _: &MoveConnectionUp, _, cx| {
                        this.move_selected_connection(false, cx)
                    }))
                    .on_action(cx.listener(|this, _: &MoveConnectionDown, _, cx| {
                        this.move_selected_connection(true, cx)
                    }))
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
    fn record_tip(
        &self,
        id: &SharedString,
        text: impl Into<RowTooltip>,
        truncation: Option<Truncation>,
    ) {
        let mut tips = self.tips.borrow_mut();
        if tips.len() >= MAX_LABEL_WIDTHS && !tips.contains_key(id) {
            tips.clear();
        }
        tips.insert(
            id.clone(),
            RowTip {
                text: text.into(),
                truncation,
            },
        );
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
#[derive(Clone, PartialEq)]
enum RowTooltip {
    Text(String),
    Tree(TreeTip),
    Status(StatusTooltip),
}

impl From<TreeTip> for RowTooltip {
    fn from(tip: TreeTip) -> Self {
        Self::Tree(tip)
    }
}

impl From<String> for RowTooltip {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<StatusTooltip> for RowTooltip {
    fn from(tooltip: StatusTooltip) -> Self {
        Self::Status(tooltip)
    }
}

struct RowTip {
    text: RowTooltip,
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
    shown: Option<(RowTooltip, AnyView)>,
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
                    let view = match &text {
                        RowTooltip::Text(text) => row_tooltip_view(text, window, cx),
                        RowTooltip::Tree(tip) => tree_tooltip_view(tip, window, cx),
                        RowTooltip::Status(tooltip) => tooltip.build(None, window, cx),
                    };
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
        .id(child_id(&id, "disclosure"))
        .test_support()
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
    // Each level indents by the disclosure and the gap after it, so the
    // disclosure of a row is under the icon of its parent. Column rows have
    // no disclosure lane, so their icon is under the icon of their relation.
    let indent = ui_px(8. + (16. + ROW_GAP) * entry.depth() as f32);
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
                    .pl(ui_px(8.))
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
        Option<TreeTip>,
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
            dbt,
            ..
        } => {
            // The dbt description adds a line. The details sheet has the
            // full text of a long description.
            let description = dbt.as_ref().and_then(|dbt| dbt.description.clone());
            let cut = dbt.as_ref().is_some_and(|dbt| dbt.description_cut);
            (
                Some(match kind {
                    RelationKind::Table => AssetIconName::Table,
                    RelationKind::View => AssetIconName::Eye,
                }),
                name.clone().into(),
                dbt.as_ref().map(|dbt| dbt.detail.clone()),
                *loading,
                error.clone(),
                row_tooltip(name, comment.as_deref(), error.as_deref()).map(|tip| {
                    tip.description(description)
                        .hint(cut.then_some(DESCRIPTION_HINT))
                }),
            )
        }
        Node::Column {
            name,
            data_type,
            comment,
            ..
        } => (
            Some(AssetIconName::Minus),
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
        Node::Relation { comment, dbt, .. } => {
            comment.is_some() || dbt.as_ref().is_some_and(|dbt| dbt.description.is_some())
        }
        Node::Column { comment, .. } => comment.is_some(),
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
    let error_status = error.as_ref().and_then(|_| match node {
        Node::Schema { profile, name, .. } | Node::Relation { profile, name, .. } => Some((
            *profile,
            format!("{name}, schema refresh error, show Activity"),
        )),
        _ => None,
    });
    let menu_id = id.clone();
    let schema_detail = matches!(node, Node::Schema { .. });
    let leaf = matches!(node, Node::Column { .. });
    let row = h_flex()
        .id(id.clone())
        .size_full()
        .pl(indent)
        .when(!schema_detail && !loading && error.is_none(), |el| {
            el.pr_2()
        })
        .gap(ui_px(ROW_GAP))
        .text_sm()
        .rounded(cx.theme().radius)
        .relative()
        .when(right_clicked, |el| el.bg(cx.theme().accent))
        .when(!right_clicked, |el| {
            el.hover(|el| el.bg(cx.theme().tokens.list_hover))
        })
        .text_color(cx.theme().sidebar_foreground)
        .when(!leaf, |el| el.child(disclosure))
        .child(
            div()
                .id(child_id(&id, "icon"))
                .test_support()
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
                .id(label_key.clone())
                .test_support()
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
                    .id(detail_key.clone())
                    .test_support()
                    .relative()
                    .flex_shrink_0()
                    .max_w(ui_px(120.))
                    .truncate()
                    .when(schema_detail, |el| {
                        el.min_w(ui_px(STATUS_SLOT_WIDTH)).flex().justify_center()
                    })
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(detail)
                    .on_prepaint(context.record_width(detail_key)),
            )
        })
        .when(loading, |el| {
            el.child(
                status_lane(child_id(&id, "busy"), ui_px(STATUS_SLOT_WIDTH))
                    .test_support()
                    .child(Spinner::new().xsmall().color(cx.theme().muted_foreground)),
            )
        })
        .when_some(error_status, |el, (profile, label)| {
            el.child(
                h_flex()
                    .id(child_id(&id, "error-status"))
                    .h_full()
                    .child(
                        status_slot(
                            child_id(&id, "error-icon").to_string(),
                            ui_px(STATUS_SLOT_WIDTH),
                            label,
                            show_activity(profile, weak),
                        )
                        .child(DotStatus::Error.dot(cx)),
                    )
                    .when(!*menu_open, |el| el.tooltip(context.live_tooltip(&id)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
            )
        })
        .when(selected, |el| el.child(focus_outline(cx)))
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

/// The tooltip view of a schema, table, or column row: its name leads, and
/// a hint is secondary. Its text is an observed element, so tests can find
/// the tooltip that a hover opens.
fn tree_tooltip_view(tip: &TreeTip, window: &mut Window, cx: &mut App) -> AnyView {
    let tip = tip.clone();
    // The parse of the short description happens once for each tooltip.
    let description = tip
        .description
        .as_deref()
        .map(|text| cx.new(|cx| TextViewState::markdown(text, cx)));
    Tooltip::element(move |_, cx| {
        // Long comments wrap instead of making the tooltip as wide as the
        // window.
        v_flex()
            .id("catalog-tooltip")
            .max_w(rems(TOOLTIP_WIDTH_REMS))
            .test_support()
            .aria_label(tip.text())
            .gap_0p5()
            .child(
                div()
                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                    .child(tip.title.clone()),
            )
            .children(tip.comment.clone())
            .when_some(description.as_ref(), |tooltip, state| {
                tooltip.child(
                    // Markdown can have content of any height, like an
                    // image, so the tooltip shows only its start.
                    div()
                        .id("catalog-tooltip-description")
                        .test_support()
                        .max_h(rems(TOOLTIP_DESCRIPTION_REMS))
                        .overflow_hidden()
                        .child(
                            TextView::new(state)
                                .style(TextViewStyle::default().paragraph_gap(rems(0.)))
                                .selectable(false),
                        ),
                )
            })
            .children(tip.error.clone())
            .when_some(tip.hint, |tooltip, hint| {
                tooltip.child(
                    div()
                        .id("catalog-tooltip-hint")
                        .test_support()
                        .aria_label(hint)
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(hint),
                )
            })
    })
    .build(window, cx)
}

/// The tooltip view of a tree row. Its text is an observed element, so tests
/// can find the tooltip that a hover opens.
fn row_tooltip_view(text: &str, window: &mut Window, cx: &mut App) -> AnyView {
    let text = SharedString::from(text.to_owned());
    Tooltip::element(move |_, _| {
        // Long comments wrap instead of making the tooltip as wide as the
        // window.
        div()
            .id("catalog-tooltip")
            .max_w(rems(TOOLTIP_WIDTH_REMS))
            .test_support()
            .aria_label(text.clone())
            .child(text.clone())
    })
    .build(window, cx)
}

/// A column type in lowercase, so that all connection types look the same.
/// Quoted names, such as a PostgreSQL type `"MyEnum"`, keep their case.
fn display_type(data_type: &str) -> String {
    let mut quote = None;
    data_type
        .chars()
        .map(|c| {
            match quote {
                Some(open) if c == open => quote = None,
                None if c == '"' || c == '`' => quote = Some(c),
                _ => {}
            }
            if quote.is_some() {
                c
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

/// The widest tree tooltip, at the standard text size.
const TOOLTIP_WIDTH_REMS: f32 = 26.;

/// The tallest dbt description in a tree tooltip. A cut description has
/// approximately six lines.
const TOOLTIP_DESCRIPTION_REMS: f32 = 12.;

/// The longest error summary in a tooltip, in characters.
const ERROR_SUMMARY_CHARS: usize = 200;

/// The first line of a catalog row error, cut to [`ERROR_SUMMARY_CHARS`].
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
    summary
}

/// The hint under a dbt description that the tooltip cuts.
const DESCRIPTION_HINT: &str = "Open dbt details to see the full description.";

/// The tooltip of a schema, table, or column row.
#[derive(Clone, Debug, PartialEq)]
struct TreeTip {
    /// The full name, which the row can truncate.
    title: String,
    comment: Option<String>,
    /// The dbt description, in Markdown.
    description: Option<String>,
    error: Option<String>,
    hint: Option<&'static str>,
}

impl TreeTip {
    fn description(mut self, description: Option<String>) -> Self {
        self.description = description;
        self
    }

    fn hint(mut self, hint: Option<&'static str>) -> Self {
        self.hint = hint;
        self
    }

    /// All the text, one part on each line.
    fn text(&self) -> String {
        [
            Some(self.title.as_str()),
            self.comment.as_deref(),
            self.description.as_deref(),
            self.error.as_deref(),
            self.hint,
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
    }
}

/// The tooltip of a tree row: its full name, which the row can truncate,
/// then its comment or its error.
fn row_tooltip(name: &str, comment: Option<&str>, error: Option<&str>) -> Option<TreeTip> {
    Some(TreeTip {
        title: name.to_owned(),
        comment: comment.map(str::to_owned),
        description: None,
        error: error.map(error_summary),
        hint: None,
    })
}

/// A fixed trailing lane for a tree status icon or count.
fn status_lane(id: SharedString, width: Pixels) -> Stateful<Div> {
    div()
        .id(id)
        .w(width)
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
}

/// A button for one status dot at the end of a catalog row. The header and
/// the list have the same side padding, so a slot as wide as the header's New
/// Connection button at the row end has the same centerline.
fn status_slot(
    id: String,
    width: Pixels,
    label: String,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
    Button::new(SharedString::from(id))
        .ghost()
        .small()
        .h_full()
        .w(width)
        .flex_shrink_0()
        .accessibility_label(label)
        .on_click(on_click)
}

/// A click on a status dot of a catalog row opens the Activity of the
/// connection, which has the details.
fn show_activity(
    id: Uuid,
    weak: &WeakEntity<Qrow>,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let weak = weak.clone();
    move |_, window, cx| {
        cx.stop_propagation();
        let _ = weak.update(cx, |this, cx| this.open_activity(Some(id), window, cx));
    }
}

/// The outline of the selected tree row while the tree has the focus. The
/// row must be `relative`, and the outline must be its last child, so that
/// hover fills of the row's controls do not cover it.
fn focus_outline(cx: &App) -> Div {
    div()
        .absolute()
        .inset_0()
        .size_full()
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().ring)
}

/// The row of a connection: the disclosure and the connection button.
fn connection_row(
    entry: &SharedString,
    id: Uuid,
    row: &ConnectionRow,
    keyboard_position: bool,
    disclosure: impl IntoElement,
    context: &RowContext,
    cx: &App,
) -> Stateful<Div> {
    let (menu_open, weak) = (context.menu_open, &context.weak);
    // As wide as the header's New Connection button.
    let slot_width = px(context.scale * STATUS_SLOT_WIDTH);
    let has_status = row.status.is_some();
    // The tooltip can change while it is open: a refresh error arrives or
    // goes away.
    context.record_tip(entry, row.tooltip.clone(), None);
    let accessibility_label = format!(
        "{}{}{}{}{}{}{}",
        row.name,
        if row.running { ", running" } else { "" },
        if row.connecting { ", connecting" } else { "" },
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
        if row.unread_success {
            ", unread query result"
        } else {
            ""
        },
        if row.refresh_error {
            ", schema refresh error"
        } else {
            ""
        }
    );
    let foreground = cx.theme().sidebar_foreground;
    // Only the current query connection paints an accent fill. An inset focus
    // outline shows the keyboard position, also without schema browsing.
    // The button itself paints no background.
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
        // The tree owns the icon and label lanes, including their padding.
        .pl_0()
        // A status slot ends at the row end. Without one, the name keeps a
        // margin from the highlight edge.
        .pr_0()
        .accessibility_label(accessibility_label.clone())
        .child(
            h_flex()
                .id(SharedString::from(format!("connection-drag-{id}")))
                .cursor(CursorStyle::OpenHand)
                .on_drag(
                    DragConnection {
                        id,
                        name: row.name.clone(),
                        database_type: row.database_type,
                    },
                    |drag, _, _, cx| cx.new(|_| drag.clone()),
                )
                .h_full()
                .w_full()
                .min_w_0()
                .text_base()
                .line_height(relative(1.25))
                .items_center()
                .gap(px(context.scale * ROW_GAP))
                .when(!has_status, |el| el.pr_3())
                .child(
                    div()
                        .id(child_id(entry, "icon"))
                        .test_support()
                        .role(Role::Image)
                        .aria_label(format!("{} database", row.database_type.label()))
                        .w(px(context.scale * 16.))
                        .flex_shrink_0()
                        .flex()
                        .justify_center()
                        .child(
                            gpui_kit::component::Icon::default()
                                .path(crate::assets::connection_icon(row.database_type))
                                .size_4(),
                        ),
                )
                .child(
                    div()
                        .id(child_id(entry, "label"))
                        .test_support()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(row.name.clone()),
                ),
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
        .gap(px(context.scale * ROW_GAP))
        .rounded(cx.theme().radius)
        .relative()
        .text_color(foreground)
        .map(|el| {
            if row.active {
                el.bg(cx.theme().primary.opacity(CURRENT_CONNECTION_FILL))
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
                .when_some(row.status, |el, status| {
                    let label = format!(
                        "{}{}{}",
                        accessibility_label,
                        if row.connected {
                            if row.running || row.connecting {
                                ", connected"
                            } else {
                                ", connected, idle"
                            }
                        } else {
                            ""
                        },
                        row.assistant_states
                            .iter()
                            .copied()
                            .collect::<std::collections::BTreeSet<_>>()
                            .into_iter()
                            .map(assistant_view::ThreadStatus::accessible_suffix)
                            .collect::<String>()
                    );
                    el.child(
                        status_slot(
                            format!("connection-status-{id}"),
                            slot_width,
                            label,
                            show_activity(id, weak),
                        )
                        .child(status.dot(cx)),
                    )
                })
                // The button selects the connection and focuses the editor.
                // The tree must not also expand the row.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
        )
        // The outline paints after the status slot, so the hover fill of the
        // slot does not cover it.
        .when(keyboard_position, |el| el.child(focus_outline(cx)))
        .child(connection_drop_target(id, false, weak.clone()))
        .child(connection_drop_target(id, true, weak.clone()))
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
    h_flex()
        .id(id.clone())
        .w_full()
        .h(px(scale * ROW_HEIGHT))
        // Text actions end at the same spine as the centers of status icons.
        .pr(px(scale * STATUS_SLOT_WIDTH / 2.))
        .gap(px(scale * ROW_GAP))
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        // A notice explains its parent rather than adding a tree level.
        // Its spinner and label use the parent's icon and label lanes.
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
                .id(child_id(id, "label"))
                .test_support()
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
                    .pr_0()
                    .label("Refresh")
                    .accessibility_label("Refresh")
                    .on_click(move |_, _, cx| {
                        let scope = scope.clone();
                        let _ =
                            weak.update(cx, |this, cx| this.refresh_catalog(profile, scope, cx));
                    }),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn an_error_summary_keeps_only_the_first_line() {
        let trace =
            "\n  Could not open the session: host unreachable\n\tat org.apache.Foo(Foo.java:1)";
        assert_eq!(
            error_summary(trace),
            "Could not open the session: host unreachable"
        );
        let long = "x".repeat(ERROR_SUMMARY_CHARS + 1);
        let summary = error_summary(&long);
        assert_eq!(summary, format!("{}…", "x".repeat(ERROR_SUMMARY_CHARS)));
    }

    #[::core::prelude::v1::test]
    fn a_column_type_shows_in_lowercase_outside_quotes() {
        assert_eq!(display_type("DECIMAL(38,2)"), "decimal(38,2)");
        assert_eq!(display_type("STRUCT<A:INT>"), "struct<a:int>");
        assert_eq!(display_type("text"), "text");
        assert_eq!(display_type("\"MyEnum\"[]"), "\"MyEnum\"[]");
        assert_eq!(display_type("STRUCT<`Key`:INT>"), "struct<`Key`:int>");
    }
}
