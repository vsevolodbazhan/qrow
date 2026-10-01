//! The catalog worker against a fake connector that serves metadata from memory.
use anyhow::Result;
use qrow::{
    catalog::{Catalog, CatalogWorker, Event, RelationKind, Scope, Status},
    connector::{Cancellation, Connector, MetadataRequest, QueryError, QueryState, Session},
    model::{Batch, CatalogSettings, Column, Profile, Row},
    storage,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

type Tables = BTreeMap<String, BTreeMap<String, (&'static str, Vec<&'static str>)>>;

#[derive(Default)]
struct Server {
    tables: Mutex<Tables>,
    connects: AtomicUsize,
    closes: AtomicUsize,
    requests: Mutex<Vec<MetadataRequest>>,
    /// Column requests for these schemas fail like a schema with a broken view.
    broken_schemas: Mutex<Vec<String>>,
    /// Column requests for these relations fail.
    broken_relations: Mutex<Vec<String>>,
    /// Requests stay running until they are cancelled.
    block: AtomicBool,
    refuse: AtomicBool,
}

impl Server {
    fn with(tables: &[(&str, &str, &'static str, &[&'static str])]) -> Arc<Self> {
        let server = Self::default();
        server.set(tables);
        Arc::new(server)
    }

    fn set(&self, tables: &[(&str, &str, &'static str, &[&'static str])]) {
        let mut map = Tables::new();
        for (schema, table, kind, columns) in tables {
            let schema = map.entry((*schema).into()).or_default();
            if !table.is_empty() {
                schema.insert((*table).into(), (*kind, columns.to_vec()));
            }
        }
        *self.tables.lock().unwrap() = map;
    }

    fn requests(&self) -> Vec<MetadataRequest> {
        self.requests.lock().unwrap().clone()
    }
}

struct Fake(Arc<Server>);
impl Connector for Fake {
    fn connect(&self, _: &Profile, password: Zeroizing<String>) -> Result<Box<dyn Session>> {
        assert_eq!(password.as_str(), "synthetic-password");
        self.0.connects.fetch_add(1, Ordering::SeqCst);
        anyhow::ensure!(!self.0.refuse.load(Ordering::SeqCst), "Connection refused");
        Ok(Box::new(FakeSession {
            server: self.0.clone(),
            columns: vec![],
            rows: vec![],
            cancelled: Arc::new(AtomicBool::new(false)),
        }))
    }
}

struct FakeSession {
    server: Arc<Server>,
    columns: Vec<Column>,
    rows: Vec<Row>,
    cancelled: Arc<AtomicBool>,
}

struct Cancel(Arc<AtomicBool>);
impl Cancellation for Cancel {
    fn cancel(&self) -> Result<()> {
        self.0.store(true, Ordering::SeqCst);
        Ok(())
    }
}

fn names(names: &[&str]) -> Vec<Column> {
    names
        .iter()
        .map(|name| Column {
            name: (*name).into(),
            data_type: "STRING".into(),
        })
        .collect()
}

impl Session for FakeSession {
    fn execute(&mut self, _: &str) -> Result<Arc<dyn Cancellation>> {
        unreachable!("the catalog worker runs no SQL")
    }
    fn execute_metadata(&mut self, request: &MetadataRequest) -> Result<Arc<dyn Cancellation>> {
        self.server.requests.lock().unwrap().push(request.clone());
        self.cancelled = Arc::new(AtomicBool::new(false));
        let tables = self.server.tables.lock().unwrap();
        let text = |value: &str| Some(value.to_owned());
        // Like HiveServer2, `_` in a name is a pattern that matches any character.
        let like = |pattern: &str, value: &str| {
            pattern.len() == value.len()
                && pattern
                    .chars()
                    .zip(value.chars())
                    .all(|(p, v)| p == '_' || p == v)
        };
        match request {
            MetadataRequest::Schemas => {
                self.columns = names(&["TABLE_SCHEM", "TABLE_CATALOG"]);
                self.rows = tables
                    .keys()
                    .map(|schema| vec![text(schema), None])
                    .collect();
            }
            MetadataRequest::Relations { schema, relation } => {
                self.columns = names(&[
                    "TABLE_CAT",
                    "TABLE_SCHEM",
                    "TABLE_NAME",
                    "TABLE_TYPE",
                    "REMARKS",
                ]);
                self.rows = tables
                    .iter()
                    .filter(|(name, _)| like(schema, name))
                    .flat_map(|(name, relations)| {
                        relations
                            .iter()
                            .map(move |(table, (kind, _))| (name, table, kind))
                    })
                    .filter(|(_, table, _)| relation.as_deref().is_none_or(|r| like(r, table)))
                    .map(|(name, table, kind)| {
                        vec![
                            None,
                            text(name),
                            text(table),
                            text(kind),
                            text(&format!("About {table}")),
                        ]
                    })
                    .collect();
            }
            MetadataRequest::Columns { schema, relation } => {
                let broken = match relation {
                    None => self.server.broken_schemas.lock().unwrap().contains(schema),
                    Some(relation) => self
                        .server
                        .broken_relations
                        .lock()
                        .unwrap()
                        .contains(relation),
                };
                if broken {
                    return Err(QueryError("View definition is invalid".into()).into());
                }
                self.columns = names(&[
                    "TABLE_SCHEM",
                    "TABLE_NAME",
                    "COLUMN_NAME",
                    "TYPE_NAME",
                    "REMARKS",
                    "ORDINAL_POSITION",
                ]);
                self.rows = tables
                    .iter()
                    .filter(|(name, _)| like(schema, name))
                    .flat_map(|(name, relations)| {
                        relations
                            .iter()
                            .map(move |(table, (_, columns))| (name, table, columns))
                    })
                    .filter(|(_, table, _)| relation.as_deref().is_none_or(|r| like(r, table)))
                    .flat_map(|(name, table, columns)| {
                        columns
                            .iter()
                            .enumerate()
                            .rev()
                            .map(move |(index, column)| {
                                vec![
                                    text(name),
                                    text(table),
                                    text(column),
                                    text("INT"),
                                    None,
                                    text(&(index + 1).to_string()),
                                ]
                            })
                    })
                    .collect();
            }
        }
        Ok(Arc::new(Cancel(self.cancelled.clone())))
    }
    fn poll(&mut self) -> Result<QueryState> {
        Ok(if self.cancelled.load(Ordering::SeqCst) {
            QueryState::Cancelled
        } else if self.server.block.load(Ordering::SeqCst) {
            QueryState::Running
        } else {
            QueryState::Finished { has_results: true }
        })
    }
    fn columns(&mut self) -> Result<Vec<Column>> {
        Ok(self.columns.clone())
    }
    fn fetch(&mut self, count: usize) -> Result<Batch> {
        // Small batches check that the worker reads until an empty batch.
        let take = count.min(2).min(self.rows.len());
        Ok(Batch {
            rows: self.rows.drain(..take).collect(),
        })
    }
    fn close_operation(&mut self) -> Result<()> {
        Ok(())
    }
    fn execute_keep_alive(&mut self, _: &str) -> Result<Arc<dyn Cancellation>> {
        unreachable!()
    }
    fn close_keep_alive(&mut self) -> Result<()> {
        unreachable!()
    }
    fn close(&mut self) -> Result<()> {
        self.server.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct Harness {
    worker: CatalogWorker,
    server: Arc<Server>,
    catalog: Option<Arc<Catalog>>,
    status: Status,
}

impl Harness {
    fn new(server: Arc<Server>, profile: Profile, cache: Option<std::path::PathBuf>) -> Self {
        let worker = CatalogWorker::with_connector(
            profile,
            cache,
            Arc::new(|| {}),
            Arc::new(Fake(server.clone())),
            Arc::new(|_| Ok(Zeroizing::new("synthetic-password".into()))),
        );
        let mut harness = Self {
            worker,
            server,
            catalog: None,
            status: Status::default(),
        };
        harness.wait(|h| h.catalog.is_some());
        harness
    }

    /// Apply events until `done` is true.
    fn wait(&mut self, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(self) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.worker.events.recv_timeout(remaining) {
                Ok(Event::Catalog(catalog)) => self.catalog = Some(catalog),
                Ok(Event::Status(status)) => self.status = status,
                Err(_) => panic!("timed out; status {:?}", self.status),
            }
        }
    }

    /// Refresh `scope` and wait until the worker is idle again.
    fn refresh(&mut self, scope: Scope) {
        self.worker.refresh(scope.clone());
        self.wait(|h| h.status.includes(&scope));
        self.wait(|h| h.status.is_idle());
    }

    fn catalog(&self) -> &Catalog {
        self.catalog.as_ref().unwrap()
    }

    fn columns(&self, schema: &str, relation: &str) -> Option<Vec<String>> {
        self.catalog()
            .relation(schema, relation)?
            .columns
            .as_ref()
            .map(|columns| columns.iter().map(|c| c.name.clone()).collect())
    }
}

fn profile() -> Profile {
    Profile {
        name: "Catalog".into(),
        host: "127.0.0.1".into(),
        username: "synthetic-user".into(),
        ..Profile::default()
    }
}

fn warehouse() -> Arc<Server> {
    Server::with(&[
        ("sales", "orders", "TABLE", &["id", "total"]),
        ("sales", "daily", "VIEW", &["day"]),
        ("sales_tmp", "scratch", "TABLE", &["x"]),
        ("salesx", "other", "TABLE", &["y"]),
        ("empty", "", "", &[]),
    ])
}

#[test]
fn connection_refresh_reads_filtered_schemas_and_relations_without_columns() {
    let mut profile = profile();
    profile.catalog = CatalogSettings {
        include: vec![],
        exclude: vec!["*_tmp".into()],
    };
    let mut h = Harness::new(warehouse(), profile, None);
    h.refresh(Scope::Connection);
    let catalog = h.catalog();
    assert_eq!(
        catalog.schemas.keys().collect::<Vec<_>>(),
        ["empty", "sales", "salesx"]
    );
    assert!(catalog.fetched_at.is_some());
    // `sales` is not a pattern for `salesx`, but `_` would be: names stay exact.
    let sales: Vec<_> = catalog
        .schema("sales")
        .unwrap()
        .relations
        .as_ref()
        .unwrap()
        .keys()
        .collect();
    assert_eq!(sales, ["daily", "orders"]);
    assert_eq!(
        catalog.relation("sales", "daily").unwrap().kind,
        RelationKind::View
    );
    assert_eq!(
        catalog
            .relation("sales", "orders")
            .unwrap()
            .comment
            .as_deref(),
        Some("About orders")
    );
    assert_eq!(h.columns("sales", "orders"), None);
    assert!(
        catalog
            .schema("empty")
            .unwrap()
            .relations
            .as_ref()
            .unwrap()
            .is_empty()
    );
    assert!(
        !h.server
            .requests()
            .iter()
            .any(|r| matches!(r, MetadataRequest::Columns { .. }))
    );
    // One session for the whole pass, closed when the queue is empty.
    assert_eq!(h.server.connects.load(Ordering::SeqCst), 1);
    assert_eq!(h.server.closes.load(Ordering::SeqCst), 1);
}

#[test]
fn schema_refresh_reads_columns_and_falls_back_to_each_relation() {
    let server = warehouse();
    let mut h = Harness::new(server.clone(), profile(), None);
    h.refresh(Scope::Connection);
    h.refresh(Scope::Schema("sales".into()));
    assert_eq!(h.columns("sales", "orders").unwrap(), ["id", "total"]);
    assert_eq!(h.columns("sales", "daily").unwrap(), ["day"]);

    server.broken_schemas.lock().unwrap().push("sales".into());
    server.broken_relations.lock().unwrap().push("daily".into());
    server.set(&[
        ("sales", "orders", "TABLE", &["id", "total", "currency"]),
        ("sales", "daily", "VIEW", &["day"]),
    ]);
    h.refresh(Scope::Schema("sales".into()));
    assert_eq!(
        h.columns("sales", "orders").unwrap(),
        ["id", "total", "currency"]
    );
    let daily = h.catalog().relation("sales", "daily").unwrap();
    assert_eq!(daily.error.as_deref(), Some("View definition is invalid"));
    // The earlier columns stay available beside the error.
    assert_eq!(h.columns("sales", "daily").unwrap(), ["day"]);
    assert_eq!(h.catalog().schema("sales").unwrap().error, None);
}

#[test]
fn relation_refresh_updates_one_relation_or_removes_a_dropped_one() {
    let server = warehouse();
    let mut h = Harness::new(server.clone(), profile(), None);
    h.refresh(Scope::Connection);
    h.refresh(Scope::Relation("sales".into(), "orders".into()));
    assert_eq!(h.columns("sales", "orders").unwrap(), ["id", "total"]);
    assert_eq!(h.columns("sales", "daily"), None);

    server.set(&[("sales", "daily", "VIEW", &["day"])]);
    h.refresh(Scope::Relation("sales".into(), "orders".into()));
    assert!(h.catalog().relation("sales", "orders").is_none());
    assert!(h.catalog().relation("sales", "daily").is_some());
}

#[test]
fn an_unavailable_server_marks_the_refreshes_and_does_not_retry_each_schema() {
    let server = warehouse();
    let mut h = Harness::new(server.clone(), profile(), None);
    server.refuse.store(true, Ordering::SeqCst);
    h.worker.refresh(Scope::Connection);
    h.worker.refresh(Scope::Schema("sales".into()));
    h.wait(|h| h.catalog().error.is_some() && h.status.is_idle());
    assert_eq!(h.catalog().error.as_deref(), Some("Connection refused"));
    assert_eq!(server.connects.load(Ordering::SeqCst), 1);
}

#[test]
fn cancel_stops_a_running_request_and_keeps_the_catalog() {
    let server = warehouse();
    let mut h = Harness::new(server.clone(), profile(), None);
    h.refresh(Scope::Connection);
    let before = h.catalog().clone();
    let earlier = server.requests().len();
    server.block.store(true, Ordering::SeqCst);
    h.worker.refresh(Scope::Schema("sales".into()));
    h.worker.refresh(Scope::Schema("salesx".into()));
    h.wait(|h| h.status.active == Some(Scope::Schema("sales".into())));
    h.worker.cancel();
    h.wait(|h| h.status.is_idle());
    assert_eq!(h.catalog().schemas, before.schemas);
    assert_eq!(h.catalog().schema("sales").unwrap().error, None);
    assert!(!h.server.requests()[earlier..].iter().any(|r| matches!(
        r,
        MetadataRequest::Relations { schema, .. } if schema == "salesx"
    )));
    assert_eq!(
        server.closes.load(Ordering::SeqCst),
        server.connects.load(Ordering::SeqCst)
    );

    // The next refresh works again.
    server.block.store(false, Ordering::SeqCst);
    h.refresh(Scope::Schema("sales".into()));
    assert!(h.columns("sales", "orders").is_some());
}

#[test]
fn a_conditional_refresh_reads_only_a_catalog_that_was_never_read() {
    let directory = tempfile::tempdir().unwrap();
    let profile = profile();
    let path = storage::catalog_path(&directory.path().join("workspace.json"), profile.id);
    let server = warehouse();
    let mut h = Harness::new(server.clone(), profile.clone(), Some(path.clone()));
    h.worker.refresh_if_unloaded();
    h.wait(|h| h.catalog().fetched_at.is_some() && h.status.is_idle());
    assert_eq!(server.connects.load(Ordering::SeqCst), 1);
    h.worker.shutdown();
    h.worker.wait_for_shutdown(Duration::from_secs(5));

    // A cached catalog does not need a session.
    let server = warehouse();
    let h = Harness::new(server.clone(), profile, Some(path));
    h.worker.refresh_if_unloaded();
    h.worker.shutdown();
    h.worker.wait_for_shutdown(Duration::from_secs(5));
    assert_eq!(server.connects.load(Ordering::SeqCst), 0);
}

#[test]
fn a_waiting_schema_refresh_absorbs_its_relation_refreshes() {
    let server = warehouse();
    let mut h = Harness::new(server.clone(), profile(), None);
    h.refresh(Scope::Connection);
    server.block.store(true, Ordering::SeqCst);
    h.worker.refresh(Scope::Schema("salesx".into()));
    h.wait(|h| h.status.active.is_some());
    h.worker
        .refresh(Scope::Relation("sales".into(), "orders".into()));
    h.worker.refresh(Scope::Schema("sales".into()));
    h.worker
        .refresh(Scope::Relation("sales".into(), "daily".into()));
    h.worker.refresh(Scope::Schema("sales".into()));
    h.wait(|h| h.status.queued == [Scope::Schema("sales".into())]);
    server.block.store(false, Ordering::SeqCst);
    h.wait(|h| h.status.is_idle());
    assert_eq!(h.columns("sales", "daily").unwrap(), ["day"]);
}

#[test]
fn the_cache_restores_without_a_session_and_profile_changes_update_it() {
    let directory = tempfile::tempdir().unwrap();
    let profile = profile();
    let path = storage::catalog_path(&directory.path().join("workspace.json"), profile.id);
    {
        let mut h = Harness::new(warehouse(), profile.clone(), Some(path.clone()));
        h.refresh(Scope::Connection);
        h.refresh(Scope::Relation("sales".into(), "orders".into()));
        h.worker.shutdown();
        h.worker.wait_for_shutdown(Duration::from_secs(5));
    }
    assert!(path.exists());

    let server = warehouse();
    let mut h = Harness::new(server.clone(), profile.clone(), Some(path.clone()));
    assert_eq!(h.columns("sales", "orders").unwrap(), ["id", "total"]);
    assert_eq!(server.connects.load(Ordering::SeqCst), 0);

    // A filter change hides schemas at once.
    let mut filtered = profile.clone();
    filtered.catalog.include = vec!["sales".into()];
    h.worker.update_profile(filtered.clone());
    h.wait(|h| h.catalog().schemas.len() == 1);

    // Another server clears the catalog and its file.
    let mut moved = filtered;
    moved.host = "elsewhere".into();
    h.worker.update_profile(moved);
    h.wait(|h| h.catalog().schemas.is_empty());
    assert!(!path.exists());

    // A cache of another server is not used.
    let mut h = Harness::new(warehouse(), profile.clone(), Some(path.clone()));
    h.refresh(Scope::Connection);
    let mut other = profile;
    other.username = "someone-else".into();
    let restored = Harness::new(warehouse(), other, Some(path.clone()));
    assert!(restored.catalog().schemas.is_empty());

    h.worker.delete();
    h.worker.wait_for_shutdown(Duration::from_secs(5));
    assert!(!path.exists());
}
