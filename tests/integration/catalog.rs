//! The catalog worker against a fake connector that serves metadata from memory.
use anyhow::Result;
use qrow::{
    catalog::{
        Catalog, CatalogConfig, CatalogWorker, Event, MINUTE, RelationKind, Request, Scope, Status,
        refresh_due,
    },
    connector::{Cancellation, Connector, MetadataRequest, QueryError, QueryState, Session},
    model::{Batch, CatalogRefresh, CatalogSettings, Column, Profile, Row},
    storage,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, UNIX_EPOCH},
};
use uuid::Uuid;
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
    broken_relation_lists: Mutex<Vec<String>>,
    /// Column requests for these relations fail.
    broken_relations: Mutex<Vec<String>>,
    /// Requests stay running until they are cancelled.
    block: AtomicBool,
    /// Requests for this schema stay running until they are cancelled.
    block_schema: Mutex<Option<String>>,
    /// A status call for this schema does not return until the request is
    /// cancelled, like a call that waits for a slow server.
    hang_schema: Mutex<Option<String>>,
    /// Requests for this schema return rows without an end.
    endless_schema: Mutex<Option<String>>,
    /// How long opening a session takes.
    connect_delay: Mutex<Duration>,
    /// How long the cancel transport takes.
    cancel_delay: Mutex<Duration>,
    /// How many requests the worker cancelled.
    cancels: AtomicUsize,
    refuse: AtomicBool,
    block_after_schema_list: AtomicBool,
    request_started: Mutex<Option<std::sync::mpsc::Sender<MetadataRequest>>>,
    /// The user of each session, in order.
    users: Mutex<Vec<String>>,
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
    fn connect(&self, profile: &Profile, password: Zeroizing<String>) -> Result<Box<dyn Session>> {
        assert_eq!(password.as_str(), "synthetic-password");
        self.0.connects.fetch_add(1, Ordering::SeqCst);
        self.0.users.lock().unwrap().push(profile.username.clone());
        std::thread::sleep(*self.0.connect_delay.lock().unwrap());
        anyhow::ensure!(!self.0.refuse.load(Ordering::SeqCst), "Connection refused");
        Ok(Box::new(FakeSession {
            server: self.0.clone(),
            columns: vec![],
            rows: vec![],
            cancelled: Arc::new(AtomicBool::new(false)),
            schema: None,
        }))
    }
}

struct FakeSession {
    server: Arc<Server>,
    columns: Vec<Column>,
    rows: Vec<Row>,
    cancelled: Arc<AtomicBool>,
    /// The schema of the request in progress.
    schema: Option<String>,
}

struct Cancel(Arc<AtomicBool>, Arc<Server>);
impl Cancellation for Cancel {
    fn cancel(&self) -> Result<()> {
        self.0.store(true, Ordering::SeqCst);
        self.1.cancels.fetch_add(1, Ordering::SeqCst);
        std::thread::sleep(*self.1.cancel_delay.lock().unwrap());
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
        if let Some(sender) = self.server.request_started.lock().unwrap().as_ref() {
            let _ = sender.send(request.clone());
        }
        self.cancelled = Arc::new(AtomicBool::new(false));
        self.schema = match request {
            MetadataRequest::Schemas => None,
            MetadataRequest::Relations { schema, .. } | MetadataRequest::Columns { schema, .. } => {
                Some(schema.clone())
            }
        };
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
                if self
                    .server
                    .broken_relation_lists
                    .lock()
                    .unwrap()
                    .contains(schema)
                {
                    return Err(QueryError("Table list is unavailable".into()).into());
                }
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
        Ok(Arc::new(Cancel(
            self.cancelled.clone(),
            self.server.clone(),
        )))
    }
    fn poll(&mut self) -> Result<QueryState> {
        if self.schema.is_some() && *self.server.hang_schema.lock().unwrap() == self.schema {
            while !self.cancelled.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let blocked =
            self.schema.is_some() && *self.server.block_schema.lock().unwrap() == self.schema;
        Ok(if self.cancelled.load(Ordering::SeqCst) {
            QueryState::Cancelled
        } else if self.server.block.load(Ordering::SeqCst)
            && (!self.server.block_after_schema_list.load(Ordering::SeqCst)
                || !matches!(
                    self.server.requests.lock().unwrap().last(),
                    Some(MetadataRequest::Schemas)
                ))
            || blocked
        {
            QueryState::Running
        } else {
            QueryState::Finished { has_results: true }
        })
    }
    fn columns(&mut self) -> Result<Vec<Column>> {
        Ok(self.columns.clone())
    }
    fn fetch(&mut self, count: usize) -> Result<Batch> {
        if self.schema.is_some() && *self.server.endless_schema.lock().unwrap() == self.schema {
            std::thread::sleep(Duration::from_millis(10));
            return Ok(Batch {
                rows: vec![vec![None; self.columns.len()]],
            });
        }
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
    /// The profile that owns the private catalog of the worker.
    id: Uuid,
    server: Arc<Server>,
    catalog: Option<Arc<Catalog>>,
    status: Status,
}

impl Harness {
    fn new(server: Arc<Server>, profile: Profile, cache: Option<std::path::PathBuf>) -> Self {
        Self::timed(server, profile, cache, MINUTE)
    }

    /// A worker whose refresh period and timeout count in `minute`.
    fn timed(
        server: Arc<Server>,
        profile: Profile,
        cache: Option<std::path::PathBuf>,
        minute: Duration,
    ) -> Self {
        Self::with_config(server, CatalogConfig::private(profile), cache, minute)
    }

    /// A worker of any catalog. `id` is its first member.
    fn with_config(
        server: Arc<Server>,
        config: CatalogConfig,
        cache: Option<std::path::PathBuf>,
        minute: Duration,
    ) -> Self {
        let id = config.members[0].id;
        let worker = CatalogWorker::with_connector(
            config,
            cache,
            Arc::new(|| {}),
            Arc::new(Fake(server.clone())),
            Arc::new(|_| Ok(Zeroizing::new("synthetic-password".into()))),
            minute,
        );
        let mut harness = Self {
            worker,
            id,
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
        self.worker.refresh(self.id, scope.clone());
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
    let mut profile = Profile {
        name: "Catalog".into(),
        host: "127.0.0.1".into(),
        username: "synthetic-user".into(),
        ..Profile::default()
    };
    profile.catalog.refresh = CatalogRefresh::WhileConnected;
    profile
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
fn connection_refresh_reads_filtered_schemas_relations_and_columns() {
    let mut profile = profile();
    profile.catalog = CatalogSettings {
        include: vec![],
        exclude: vec!["*_tmp".into()],
        ..CatalogSettings::default()
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
    assert_eq!(h.columns("sales", "orders").unwrap(), ["id", "total"]);
    assert_eq!(h.columns("sales", "daily").unwrap(), ["day"]);
    assert!(
        catalog
            .schema("empty")
            .unwrap()
            .relations
            .as_ref()
            .unwrap()
            .is_empty()
    );
    // Columns are read one schema at a time, never for the whole connection.
    let columns: Vec<_> = h
        .server
        .requests()
        .into_iter()
        .filter_map(|r| match r {
            MetadataRequest::Columns { schema, relation } => Some((schema, relation)),
            _ => None,
        })
        .collect();
    assert_eq!(
        columns,
        [
            ("empty".to_owned(), None),
            ("sales".to_owned(), None),
            ("salesx".to_owned(), None)
        ]
    );
    // One session for the whole pass, closed when the queue is empty.
    assert_eq!(h.server.connects.load(Ordering::SeqCst), 1);
    assert_eq!(h.server.closes.load(Ordering::SeqCst), 1);
}

#[test]
fn a_connection_refresh_skips_schemas_hidden_while_it_runs() {
    let server = Server::with(&[
        ("a", "first", "TABLE", &["id"]),
        ("b", "hidden", "TABLE", &["id"]),
    ]);
    server.block.store(true, Ordering::SeqCst);
    server.block_after_schema_list.store(true, Ordering::SeqCst);
    let (sender, requests) = std::sync::mpsc::channel();
    *server.request_started.lock().unwrap() = Some(sender);
    let mut profile = profile();
    let mut h = Harness::new(server.clone(), profile.clone(), None);
    h.worker.refresh(h.id, Scope::Connection);
    h.wait(|h| h.status.includes(&Scope::Connection));
    assert_eq!(
        requests.recv_timeout(Duration::from_secs(10)).unwrap(),
        MetadataRequest::Schemas
    );
    assert_eq!(
        requests.recv_timeout(Duration::from_secs(10)).unwrap(),
        MetadataRequest::Relations {
            schema: "a".into(),
            relation: None,
        }
    );
    profile.catalog.exclude = vec!["b".into()];
    h.worker.configure(CatalogConfig::private(profile));
    server.block.store(false, Ordering::SeqCst);
    h.wait(|h| h.status.is_idle());
    assert!(h.catalog().schema("b").is_none());
    assert_eq!(h.columns("a", "first").unwrap(), ["id"]);
    assert!(!server.requests().iter().any(|request| matches!(request,
        MetadataRequest::Relations { schema, .. } | MetadataRequest::Columns { schema, .. }
            if schema == "b"
    )));
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
    // A relation refresh reads only its relation.
    server.set(&[
        ("sales", "orders", "TABLE", &["id", "total", "currency"]),
        ("sales", "daily", "VIEW", &["day", "count"]),
    ]);
    h.refresh(Scope::Relation("sales".into(), "orders".into()));
    assert_eq!(
        h.columns("sales", "orders").unwrap(),
        ["id", "total", "currency"]
    );
    assert_eq!(h.columns("sales", "daily").unwrap(), ["day"]);

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
    h.worker.refresh(h.id, Scope::Connection);
    h.worker.refresh(h.id, Scope::Schema("sales".into()));
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
    h.worker.refresh(h.id, Scope::Schema("sales".into()));
    h.worker.refresh(h.id, Scope::Schema("salesx".into()));
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
    h.worker.refresh_if_unloaded(h.id);
    h.wait(|h| h.catalog().fetched_at.is_some() && h.status.is_idle());
    assert_eq!(server.connects.load(Ordering::SeqCst), 1);
    h.worker.shutdown();
    h.worker.wait_for_shutdown(Duration::from_secs(5));

    // A cached catalog does not need a session.
    let server = warehouse();
    let h = Harness::new(server.clone(), profile, Some(path));
    h.worker.refresh_if_unloaded(h.id);
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
    h.worker.refresh(h.id, Scope::Schema("salesx".into()));
    h.wait(|h| h.status.active.is_some());
    h.worker
        .refresh(h.id, Scope::Relation("sales".into(), "orders".into()));
    h.worker.refresh(h.id, Scope::Schema("sales".into()));
    h.worker
        .refresh(h.id, Scope::Relation("sales".into(), "daily".into()));
    h.worker.refresh(h.id, Scope::Schema("sales".into()));
    let id = h.id;
    h.wait(|h| {
        h.status.queued
            == [Request {
                member: id,
                scope: Scope::Schema("sales".into()),
            }]
    });
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
    h.worker.configure(CatalogConfig::private(filtered.clone()));
    h.wait(|h| h.catalog().schemas.len() == 1);

    // Another server clears the catalog and its file.
    let mut moved = filtered;
    moved.host = "elsewhere".into();
    h.worker.configure(CatalogConfig::private(moved));
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

#[test]
fn logs_entries_share_one_batch_and_follow_the_profile_option() {
    let server = warehouse();
    let mut h = Harness::new(server, profile(), None);
    h.refresh(Scope::Schema("sales".into()));
    assert_eq!(h.worker.activities.try_iter().count(), 0);

    let mut logged = profile();
    logged.id = h.catalog().owner;
    logged.catalog.log_refreshes = true;
    h.worker.configure(CatalogConfig::private(logged));
    h.refresh(Scope::Connection);
    let entries: Vec<_> = h
        .worker
        .activities
        .try_iter()
        .map(|(_, event)| event)
        .collect();
    let texts: Vec<_> = entries.iter().map(|entry| entry.text.as_str()).collect();
    assert!(
        texts[0].starts_with("Started a schema refresh of the connection"),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|text| text.starts_with("Opened a session"))
    );
    assert!(
        texts
            .iter()
            .any(|text| text.starts_with("List schemas: 4 schemas (client measurement:"))
    );
    assert!(
        texts
            .iter()
            .any(|text| text
                .starts_with("List relations in sales: 2 relations (client measurement:"))
    );
    assert!(
        texts
            .last()
            .unwrap()
            .starts_with("Schema refresh completed")
    );
    let batch = entries[0].batch;
    assert!(batch.is_some());
    assert!(entries.iter().all(|entry| entry.batch == batch));
    assert!(
        entries
            .iter()
            .all(|entry| entry.connection.as_deref() == Some("Catalog"))
    );

    h.refresh(Scope::Relation("sales".into(), "orders".into()));
    let next: Vec<_> = h
        .worker
        .activities
        .try_iter()
        .map(|(_, event)| event)
        .collect();
    assert!(
        next.iter()
            .all(|entry| entry.batch.is_some() && entry.batch != batch)
    );

    // A broken view fails its relation. The refresh reports it at its end.
    // The failed request for the whole schema does not count, because the
    // requests for each relation replace it.
    h.server.broken_schemas.lock().unwrap().push("sales".into());
    h.server
        .broken_relations
        .lock()
        .unwrap()
        .push("daily".into());
    h.refresh(Scope::Schema("sales".into()));
    let entries: Vec<_> = h
        .worker
        .activities
        .try_iter()
        .map(|(_, event)| event)
        .collect();
    let last = entries.last().unwrap();
    assert!(
        last.text
            .starts_with("Schema refresh completed with 1 error"),
        "{}",
        last.text
    );
    assert_eq!(last.severity, qrow::activity::Severity::Error);
    assert!(
        entries
            .iter()
            .any(|entry| entry.text.starts_with("List columns of sales.daily failed"))
    );
}

#[test]
fn logs_entries_count_only_the_requested_names() {
    // `_` in `my_db` also matches `myxdb` on the server.
    let server = Server::with(&[
        ("my_db", "orders", "TABLE", &["id"]),
        ("myxdb", "other", "TABLE", &["a", "b"]),
    ]);
    let mut logged = profile();
    logged.catalog.log_refreshes = true;
    let mut h = Harness::new(server, logged, None);
    h.refresh(Scope::Connection);
    let texts: Vec<_> = h
        .worker
        .activities
        .try_iter()
        .map(|(_, event)| event)
        .map(|entry| entry.text)
        .collect();
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("List relations in my_db: 1 relation (")),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("List columns of all relations in my_db: 1 column (")),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.starts_with("List columns of all relations in myxdb: 2 columns (")),
        "{texts:?}"
    );
}

/// Wait until the worker cancelled a request. A cancellation runs on its own
/// thread, and the watchdog and the worker can both cancel a timed-out request.
fn wait_for_cancel(server: &Server) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while server.cancels.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "no request was cancelled");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn connects(server: &Server) -> usize {
    server.connects.load(Ordering::SeqCst)
}

/// A cache of `warehouse()` read at `at`, with `sales` read later than the
/// other schemas.
fn cached_warehouse(path: &std::path::Path, profile: &Profile, at: u64) {
    let mut h = Harness::new(warehouse(), profile.clone(), None);
    h.refresh(Scope::Connection);
    let mut catalog = h.catalog().clone();
    catalog.fetched_at = Some(at);
    for (name, schema) in catalog.schemas.iter_mut() {
        let schema = Arc::make_mut(schema);
        schema.fetched_at = Some(if name == "sales" { at + 1 } else { at });
    }
    storage::save_catalog(path, &catalog).unwrap();
}

#[test]
fn an_automatic_refresh_is_due_only_while_warm_and_after_its_period() {
    let every = CatalogSettings {
        refresh: CatalogRefresh::WhileConnected,
        ..CatalogSettings::default()
    };
    let last = UNIX_EPOCH + Duration::from_secs(1_000_000);
    let hour = Duration::from_secs(3600);
    assert_eq!(
        refresh_due(&every, true, Some(last), MINUTE),
        Some(last + hour)
    );
    // A catalog that Qrow never read is due at once.
    assert_eq!(refresh_due(&every, true, None, MINUTE), Some(UNIX_EPOCH));
    // No live session, or another choice: nothing is due.
    assert_eq!(refresh_due(&every, false, Some(last), MINUTE), None);
    for refresh in [CatalogRefresh::Manual, CatalogRefresh::Disabled] {
        let settings = CatalogSettings {
            refresh,
            ..every.clone()
        };
        assert_eq!(refresh_due(&settings, true, None, MINUTE), None);
    }
    // A period change moves the due time.
    let often = CatalogSettings {
        refresh_minutes: 5,
        ..every
    };
    assert_eq!(
        refresh_due(&often, true, Some(last), MINUTE),
        Some(last + Duration::from_secs(300))
    );
}

#[test]
fn a_warm_connection_refreshes_by_itself_and_a_cold_one_never_connects() {
    let server = warehouse();
    let mut profile = profile();
    profile.catalog.refresh = CatalogRefresh::WhileConnected;
    profile.catalog.refresh_minutes = 5;
    // One "minute" is 40 ms, so the period is 200 ms.
    let mut h = Harness::timed(
        server.clone(),
        profile.clone(),
        None,
        Duration::from_millis(40),
    );
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(connects(&server), 0, "a cold connection opened a session");

    // A catalog that Qrow never read is stale, so it is read at once.
    h.worker.set_live(h.id, true);
    h.wait(|h| h.catalog().fetched_at.is_some() && h.status.is_idle());
    assert_eq!(h.columns("sales", "orders").unwrap(), ["id", "total"]);
    // Then again after each period.
    h.wait(|_| connects(&server) >= 3);

    h.worker.set_live(h.id, false);
    // The worker applies the command before it starts another refresh.
    std::thread::sleep(Duration::from_millis(100));
    h.wait(|h| h.status.is_idle());
    let after = connects(&server);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(connects(&server), after);

    // A manual policy never refreshes by itself.
    profile.catalog.refresh = CatalogRefresh::Manual;
    h.worker.configure(CatalogConfig::private(profile));
    h.worker.set_live(h.id, true);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(connects(&server), after);
}

#[test]
fn a_fresh_cache_waits_for_its_period_and_a_stale_one_refreshes_when_warm() {
    let directory = tempfile::tempdir().unwrap();
    let profile = profile();
    let path = storage::catalog_path(&directory.path().join("workspace.json"), profile.id);
    let now = qrow::catalog::now();
    cached_warehouse(&path, &profile, now);

    // Read now, with a period of 60 minutes: no refresh is due.
    let server = warehouse();
    let h = Harness::new(server.clone(), profile.clone(), Some(path.clone()));
    h.worker.set_live(h.id, true);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(connects(&server), 0);
    h.worker.shutdown();
    h.worker.wait_for_shutdown(Duration::from_secs(5));

    // Read two hours ago: the refresh starts when the connection is warm.
    cached_warehouse(&path, &profile, now - 2 * 3600);
    let server = warehouse();
    let mut h = Harness::new(server.clone(), profile, Some(path));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(connects(&server), 0);
    h.worker.set_live(h.id, true);
    h.wait(|h| h.catalog().fetched_at.is_some_and(|at| at >= now));
    h.wait(|h| h.status.is_idle());
    assert_eq!(connects(&server), 1);
}

#[test]
fn a_connection_refresh_reads_the_oldest_schemas_first() {
    let directory = tempfile::tempdir().unwrap();
    let profile = profile();
    let path = storage::catalog_path(&directory.path().join("workspace.json"), profile.id);
    cached_warehouse(&path, &profile, 1_000);
    let server = warehouse();
    let mut h = Harness::new(server.clone(), profile, Some(path));
    h.refresh(Scope::Connection);
    let order: Vec<_> = server
        .requests()
        .into_iter()
        .filter_map(|request| match request {
            MetadataRequest::Relations {
                schema,
                relation: None,
            } => Some(schema),
            _ => None,
        })
        .collect();
    assert_eq!(order, ["empty", "sales_tmp", "salesx", "sales"]);
}

/// The schemas whose relation lists the server returned, in order.
fn relation_lists(server: &Server) -> Vec<String> {
    server
        .requests()
        .into_iter()
        .filter_map(|request| match request {
            MetadataRequest::Relations {
                schema,
                relation: None,
            } => Some(schema),
            _ => None,
        })
        .collect()
}

#[test]
fn a_stopped_connection_refresh_continues_in_the_refresh_period() {
    let server = warehouse();
    *server.block_schema.lock().unwrap() = Some("salesx".into());
    let mut profile = profile();
    profile.catalog.timeout_minutes = 2;
    profile.catalog.log_refreshes = true;
    // One "minute" is 250 ms: the timeout is 500 ms, and the refresh period
    // of 60 minutes is 15 s.
    let mut h = Harness::timed(server.clone(), profile, None, Duration::from_millis(250));
    h.worker.refresh(h.id, Scope::Connection);
    h.wait(|h| h.status.active.is_some());
    h.wait(|h| h.status.is_idle());
    assert_eq!(
        relation_lists(&server),
        ["empty", "sales", "sales_tmp", "salesx"]
    );
    let unfinished = h.catalog().unfinished.clone().unwrap();
    assert_eq!(
        unfinished.done.into_iter().collect::<Vec<_>>(),
        ["empty", "sales", "sales_tmp"]
    );
    wait_for_cancel(&server);

    // The next connection refresh reads only the schema that it did not read.
    *server.block_schema.lock().unwrap() = None;
    let before = relation_lists(&server).len();
    let _ = h.worker.activities.try_iter().count();
    h.refresh(Scope::Connection);
    assert_eq!(relation_lists(&server)[before..], ["salesx"]);
    assert_eq!(h.columns("salesx", "other").unwrap(), ["y"]);
    assert!(h.catalog().unfinished.is_none());
    let texts: Vec<_> = h
        .worker
        .activities
        .try_iter()
        .map(|(_, e)| e.text)
        .collect();
    assert!(
        texts
            .iter()
            .any(|text| text
                == "Continued a stopped schema refresh: 3 of 4 schemas were already read"),
        "{texts:?}"
    );

    // A completed refresh does not continue: the next one reads all schemas.
    let before = relation_lists(&server).len();
    h.refresh(Scope::Connection);
    assert_eq!(relation_lists(&server).len() - before, 4);
}

#[test]
fn a_continued_refresh_retries_a_schema_with_failed_reads() {
    for relation_list_fails in [false, true] {
        let server = warehouse();
        if relation_list_fails {
            server
                .broken_relation_lists
                .lock()
                .unwrap()
                .push("sales".into());
        } else {
            server.broken_schemas.lock().unwrap().push("sales".into());
            server.broken_relations.lock().unwrap().push("daily".into());
        }
        *server.block_schema.lock().unwrap() = Some("salesx".into());
        let mut profile = profile();
        profile.catalog.timeout_minutes = 2;
        let mut h = Harness::timed(server.clone(), profile, None, Duration::from_millis(250));
        h.refresh(Scope::Connection);
        assert!(
            !h.catalog()
                .unfinished
                .as_ref()
                .unwrap()
                .done
                .contains("sales")
        );
        wait_for_cancel(&server);
        server.broken_relation_lists.lock().unwrap().clear();
        server.broken_schemas.lock().unwrap().clear();
        server.broken_relations.lock().unwrap().clear();
        *server.block_schema.lock().unwrap() = None;
        let before = relation_lists(&server).len();
        h.refresh(Scope::Connection);
        let expected = if relation_list_fails {
            ["sales", "salesx"]
        } else {
            ["salesx", "sales"]
        };
        assert_eq!(relation_lists(&server)[before..], expected);
        assert_eq!(h.columns("sales", "daily").unwrap(), ["day"]);
        assert!(
            h.catalog()
                .relation("sales", "daily")
                .unwrap()
                .error
                .is_none()
        );
    }
}

#[test]
fn a_continued_refresh_rereads_a_schema_hidden_and_shown_again() {
    let server = warehouse();
    *server.block_schema.lock().unwrap() = Some("salesx".into());
    let mut profile = profile();
    profile.catalog.timeout_minutes = 2;
    let mut h = Harness::timed(
        server.clone(),
        profile.clone(),
        None,
        Duration::from_millis(250),
    );
    h.refresh(Scope::Connection);
    assert_eq!(h.columns("sales", "orders").unwrap(), ["id", "total"]);
    wait_for_cancel(&server);
    profile.catalog.exclude = vec!["sales".into()];
    h.worker.configure(CatalogConfig::private(profile.clone()));
    h.wait(|h| h.catalog().schema("sales").is_none());
    assert!(
        !h.catalog()
            .unfinished
            .as_ref()
            .unwrap()
            .done
            .contains("sales")
    );
    profile.catalog.exclude.clear();
    h.worker.configure(CatalogConfig::private(profile));
    *server.block_schema.lock().unwrap() = None;
    let before = relation_lists(&server).len();
    h.refresh(Scope::Connection);
    assert_eq!(relation_lists(&server)[before..], ["sales", "salesx"]);
    assert_eq!(h.columns("sales", "orders").unwrap(), ["id", "total"]);
    assert!(h.catalog().unfinished.is_none());
}

#[test]
fn a_timeout_stops_the_refresh_and_keeps_what_it_read() {
    let server = warehouse();
    *server.block_schema.lock().unwrap() = Some("salesx".into());
    let mut profile = profile();
    profile.catalog.timeout_minutes = 2;
    profile.catalog.log_refreshes = true;
    // One "minute" is 250 ms, so the timeout is 500 ms.
    let mut h = Harness::timed(server.clone(), profile, None, Duration::from_millis(250));
    let started = Instant::now();
    h.worker.refresh(h.id, Scope::Connection);
    h.wait(|h| h.status.active.is_some());
    h.wait(|h| h.status.is_idle());
    assert!(started.elapsed() >= Duration::from_millis(500));
    let catalog = h.catalog();
    assert_eq!(
        catalog.error.as_deref(),
        Some("Refresh stopped after 2 minutes")
    );
    // The schemas before the blocked one stay in the catalog.
    assert_eq!(h.columns("sales", "orders").unwrap(), ["id", "total"]);
    assert!(catalog.schema("empty").unwrap().relations.is_some());
    assert!(catalog.schema("salesx").unwrap().relations.is_none());
    wait_for_cancel(&server);
    assert_eq!(server.closes.load(Ordering::SeqCst), connects(&server));
    let texts: Vec<_> = h
        .worker
        .activities
        .try_iter()
        .map(|(_, event)| event)
        .map(|e| e.text)
        .collect();
    assert!(
        texts
            .last()
            .unwrap()
            .starts_with("Schema refresh stopped after 2 minutes"),
        "{texts:?}"
    );

    // A schema refresh has the same limit.
    h.worker.refresh(h.id, Scope::Schema("salesx".into()));
    h.wait(|h| h.status.active.is_some());
    h.wait(|h| h.status.is_idle());
    assert_eq!(
        h.catalog().schema("salesx").unwrap().error.as_deref(),
        Some("Refresh stopped after 2 minutes")
    );
}

#[test]
fn an_automatic_refresh_stops_when_the_connection_becomes_cold() {
    let server = warehouse();
    *server.block_schema.lock().unwrap() = Some("sales".into());
    let mut profile = profile();
    profile.catalog.log_refreshes = true;
    let mut h = Harness::new(server.clone(), profile, None);
    h.worker.set_live(h.id, true);
    h.wait(|h| h.status.active == Some(Scope::Connection));
    // `empty` goes before the blocked `sales`.
    std::thread::sleep(Duration::from_millis(200));
    h.worker.set_live(h.id, false);
    h.wait(|h| h.status.is_idle());
    assert_eq!(h.catalog().error, None);
    assert!(h.catalog().schema("empty").unwrap().relations.is_some());
    wait_for_cancel(&server);
    assert_eq!(server.closes.load(Ordering::SeqCst), connects(&server));
    let texts: Vec<_> = h
        .worker
        .activities
        .try_iter()
        .map(|(_, event)| event)
        .map(|e| e.text)
        .collect();
    assert!(
        texts[0].starts_with("Started an automatic schema refresh of the connection"),
        "{texts:?}"
    );
    assert!(
        texts
            .last()
            .unwrap()
            .starts_with("Schema refresh stopped because no tab of the connection is connected"),
        "{texts:?}"
    );
}

#[test]
fn a_timeout_cancels_a_call_that_blocks() {
    let server = warehouse();
    let mut profile = profile();
    profile.catalog.timeout_minutes = 2;
    // One "minute" is 150 ms, so the timeout is 300 ms.
    let mut h = Harness::timed(server.clone(), profile, None, Duration::from_millis(150));
    h.refresh(Scope::Connection);
    *server.hang_schema.lock().unwrap() = Some("sales".into());
    h.worker.refresh(h.id, Scope::Schema("sales".into()));
    h.wait(|h| h.status.active.is_some());
    h.wait(|h| h.status.is_idle());
    wait_for_cancel(&server);
    assert_eq!(
        h.catalog().schema("sales").unwrap().error.as_deref(),
        Some("Refresh stopped after 2 minutes")
    );
    assert_eq!(h.columns("sales", "orders").unwrap(), ["id", "total"]);
}

#[test]
fn an_automatic_refresh_stops_between_fetches_when_the_connection_becomes_cold() {
    let server = warehouse();
    *server.endless_schema.lock().unwrap() = Some("sales".into());
    let mut h = Harness::new(server.clone(), profile(), None);
    h.worker.set_live(h.id, true);
    h.wait(|h| h.status.active == Some(Scope::Connection));
    std::thread::sleep(Duration::from_millis(200));
    h.worker.set_live(h.id, false);
    h.wait(|h| h.status.is_idle());
    wait_for_cancel(&server);
    assert_eq!(h.catalog().error, None);
}

#[test]
fn a_slow_cancel_transport_does_not_block_a_cancel_from_the_window() {
    let server = warehouse();
    let mut profile = profile();
    profile.catalog.timeout_minutes = 2;
    // One "minute" is 100 ms, so the timeout is 200 ms.
    let mut h = Harness::timed(server.clone(), profile, None, Duration::from_millis(100));
    h.refresh(Scope::Connection);
    *server.hang_schema.lock().unwrap() = Some("sales".into());
    *server.cancel_delay.lock().unwrap() = Duration::from_secs(2);
    h.worker.refresh(h.id, Scope::Schema("sales".into()));
    h.wait(|h| h.status.active.is_some());
    // The watchdog is now in its slow cancellation.
    wait_for_cancel(&server);
    let started = Instant::now();
    h.worker.cancel();
    assert!(started.elapsed() < Duration::from_millis(500));
    h.wait(|h| h.status.is_idle());
}

#[test]
fn a_refresh_that_times_out_while_it_connects_sends_no_request() {
    let server = warehouse();
    let mut profile = profile();
    profile.catalog.timeout_minutes = 2;
    // One "minute" is 100 ms, so the timeout is 200 ms.
    let mut h = Harness::timed(server.clone(), profile, None, Duration::from_millis(100));
    h.refresh(Scope::Connection);
    let earlier = server.requests().len();
    *server.connect_delay.lock().unwrap() = Duration::from_millis(400);
    h.worker.refresh(h.id, Scope::Schema("sales".into()));
    h.wait(|h| h.status.active.is_some());
    h.wait(|h| h.status.is_idle());
    assert_eq!(server.requests().len(), earlier);
    assert_eq!(
        h.catalog().schema("sales").unwrap().error.as_deref(),
        Some("Refresh stopped after 2 minutes")
    );
}

fn users(server: &Server) -> Vec<String> {
    server.users.lock().unwrap().clone()
}

/// A shared catalog of the members `small` and `large`, which read the same
/// warehouse with their own users.
fn shared(preferred: Option<usize>) -> (CatalogConfig, Profile, Profile) {
    let member = |name: &str| Profile {
        name: name.into(),
        username: format!("user-{name}"),
        ..profile()
    };
    let (small, large) = (member("small"), member("large"));
    let config = CatalogConfig {
        id: Uuid::new_v4(),
        shared: true,
        members: vec![small.clone(), large.clone()],
        settings: profile().catalog,
        preferred: preferred.map(|index| [small.id, large.id][index]),
    };
    (config, small, large)
}

#[test]
fn each_member_refreshes_with_its_own_session_and_all_see_the_data() {
    let server = warehouse();
    let (config, small, large) = shared(None);
    let mut h = Harness::with_config(server.clone(), config, None, MINUTE);
    assert_eq!(h.catalog().identity, None);

    h.worker.refresh(small.id, Scope::Connection);
    h.wait(|h| h.status.runner == Some(small.id));
    h.wait(|h| h.status.is_idle());
    server.set(&[
        ("sales", "orders", "TABLE", &["id", "total"]),
        ("salesx", "other", "TABLE", &["y", "z"]),
    ]);
    h.worker.refresh(large.id, Scope::Schema("salesx".into()));
    h.wait(|h| h.status.runner == Some(large.id));
    h.wait(|h| h.status.is_idle());

    assert_eq!(users(&server), ["user-small", "user-large"]);
    assert_eq!(h.columns("sales", "orders").unwrap(), ["id", "total"]);
    assert_eq!(h.columns("salesx", "other").unwrap(), ["y", "z"]);
    // A member that is not in the catalog cannot refresh it.
    h.worker.refresh(Uuid::new_v4(), Scope::Connection);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(connects(&server), 2);
}

#[test]
fn a_refresh_of_one_member_covers_the_same_refresh_of_another() {
    let server = warehouse();
    server.block.store(true, Ordering::SeqCst);
    let (config, small, large) = shared(None);
    let mut h = Harness::with_config(server, config, None, MINUTE);
    h.worker.refresh(small.id, Scope::Connection);
    h.wait(|h| h.status.active == Some(Scope::Connection));
    h.worker.refresh(large.id, Scope::Schema("sales".into()));
    h.worker.refresh(large.id, Scope::Connection);
    std::thread::sleep(Duration::from_millis(100));
    h.wait(|h| h.status.queued.is_empty());
    // The waiting refresh of a narrower scope gives way to a wider one.
    h.worker.cancel();
    h.wait(|h| h.status.is_idle());
    h.worker.refresh(small.id, Scope::Schema("sales".into()));
    h.wait(|h| h.status.active.is_some());
    h.worker
        .refresh(large.id, Scope::Relation("salesx".into(), "other".into()));
    h.worker.refresh(small.id, Scope::Schema("salesx".into()));
    h.wait(|h| {
        h.status.queued
            == [Request {
                member: small.id,
                scope: Scope::Schema("salesx".into()),
            }]
    });
    h.worker.cancel();
    h.wait(|h| h.status.is_idle());
}

#[test]
fn stopping_one_member_leaves_the_refreshes_of_the_others() {
    let server = warehouse();
    *server.block_schema.lock().unwrap() = Some("sales".into());
    let (config, small, large) = shared(None);
    let mut h = Harness::with_config(server.clone(), config, None, MINUTE);
    *server.block_schema.lock().unwrap() = None;
    h.refresh(Scope::Connection);
    let schema = |h: &Harness, name: &str| h.catalog().schema(name).unwrap().fetched_at;
    let before = schema(&h, "sales");
    *server.block_schema.lock().unwrap() = Some("sales".into());
    h.worker.refresh(small.id, Scope::Schema("sales".into()));
    h.wait(|h| h.status.runner == Some(small.id));
    h.worker.refresh(large.id, Scope::Schema("salesx".into()));
    h.wait(|h| h.status.queued.len() == 1);

    // The waiting member stops: the running refresh continues.
    h.worker.stop(large.id);
    h.wait(|h| h.status.queued.is_empty());
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(h.status.runner, Some(small.id));
    assert_eq!(server.cancels.load(Ordering::SeqCst), 0);

    // The running member stops: the next member runs.
    h.worker.refresh(large.id, Scope::Schema("salesx".into()));
    h.wait(|h| h.status.queued.len() == 1);
    h.worker.stop(small.id);
    wait_for_cancel(&server);
    h.wait(|h| h.status.runner == Some(large.id) || h.status.is_idle());
    h.wait(|h| h.status.is_idle());
    assert_eq!(users(&server).last().unwrap(), "user-large");
    assert_eq!(schema(&h, "sales"), before);
    assert_eq!(h.columns("salesx", "other").unwrap(), ["y"]);
}

#[test]
fn a_connection_error_belongs_to_the_member_that_ran_the_refresh() {
    let server = warehouse();
    server.refuse.store(true, Ordering::SeqCst);
    let (config, small, large) = shared(None);
    let mut h = Harness::with_config(server.clone(), config, None, MINUTE);
    h.worker.refresh(small.id, Scope::Connection);
    h.wait(|h| h.catalog().error.is_some() && h.status.is_idle());
    assert_eq!(h.catalog().error_member, Some(small.id));
    server.refuse.store(false, Ordering::SeqCst);
    h.worker.refresh(large.id, Scope::Connection);
    h.wait(|h| h.catalog().error.is_none() && h.status.is_idle());
    assert!(h.catalog().fetched_at.is_some());
}

#[test]
fn automatic_refreshes_use_the_preferred_member_while_it_is_live() {
    let minute = Duration::from_millis(40);
    let (mut config, small, large) = shared(Some(1));
    config.settings.refresh = CatalogRefresh::WhileConnected;
    config.settings.refresh_minutes = 5;

    // Both members are live: the preferred one refreshes.
    let server = warehouse();
    let mut h = Harness::with_config(server.clone(), config.clone(), None, minute);
    h.worker.set_live(small.id, true);
    h.worker.set_live(large.id, true);
    h.wait(|h| h.catalog().fetched_at.is_some() && h.status.is_idle());
    assert_eq!(users(&server)[0], "user-large");
    h.worker.shutdown();

    // Without the preferred member, the first live member refreshes.
    let server = warehouse();
    let mut h = Harness::with_config(server.clone(), config, None, minute);
    h.worker.set_live(small.id, true);
    h.wait(|h| h.catalog().fetched_at.is_some() && h.status.is_idle());
    assert_eq!(users(&server)[0], "user-small");
    h.worker.set_live(small.id, false);
    std::thread::sleep(Duration::from_millis(100));
    h.wait(|h| h.status.is_idle());
    let after = connects(&server);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(connects(&server), after, "no member is live");
}

#[test]
fn a_member_that_leaves_stops_its_refreshes() {
    let server = warehouse();
    *server.block_schema.lock().unwrap() = Some("sales".into());
    let (config, small, large) = shared(None);
    let mut h = Harness::with_config(server.clone(), config.clone(), None, MINUTE);
    *server.block_schema.lock().unwrap() = None;
    h.refresh(Scope::Connection);
    *server.block_schema.lock().unwrap() = Some("sales".into());
    h.worker.refresh(large.id, Scope::Schema("sales".into()));
    h.wait(|h| h.status.runner == Some(large.id));
    let remaining = CatalogConfig {
        members: vec![small],
        ..config
    };
    h.worker.configure(remaining);
    wait_for_cancel(&server);
    h.wait(|h| h.status.is_idle());
    // The catalog stays: members do not change what it describes.
    *server.block_schema.lock().unwrap() = None;
    h.refresh(Scope::Schema("salesx".into()));
    assert_eq!(h.columns("salesx", "other").unwrap(), ["y"]);
    assert_eq!(users(&server).last().unwrap(), "user-small");
}

#[test]
fn logs_entries_name_the_member_that_ran_the_refresh() {
    let server = warehouse();
    let (mut config, small, large) = shared(None);
    config.members[0].catalog.log_refreshes = true;
    config.members[1].catalog.log_refreshes = false;
    let mut h = Harness::with_config(server, config, None, MINUTE);
    h.worker.refresh(small.id, Scope::Schema("sales".into()));
    h.wait(|h| h.status.runner == Some(small.id));
    h.wait(|h| h.status.is_idle());
    h.worker.refresh(large.id, Scope::Schema("salesx".into()));
    h.wait(|h| h.status.runner == Some(large.id));
    h.wait(|h| h.status.is_idle());
    let entries: Vec<_> = h.worker.activities.try_iter().collect();
    assert!(!entries.is_empty());
    assert!(entries.iter().all(|(member, event)| {
        *member == small.id && event.connection.as_deref() == Some("small")
    }));
}

#[test]
fn a_new_shared_catalog_starts_with_the_private_catalog_of_its_member() {
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace.json");
    let (config, small, _) = shared(None);
    let private = storage::catalog_path(&workspace, small.id);
    cached_warehouse(&private, &small, 1_000);
    let seed = qrow::catalog::Seed::File {
        path: private.clone(),
        owner: small.id,
        identity: qrow::catalog::CatalogIdentity::of(&small),
    };
    let path = storage::catalog_path(&workspace, config.id);
    let server = warehouse();
    let mut h = Harness::with_config(server.clone(), config.clone(), Some(path.clone()), MINUTE);
    h.worker.seed(seed);
    h.wait(|h| h.catalog().fetched_at == Some(1_000));
    assert_eq!(h.columns("sales", "orders").unwrap(), ["id", "total"]);
    assert_eq!(h.catalog().owner, config.id);
    assert_eq!(connects(&server), 0);
    h.worker.shutdown();
    h.worker.wait_for_shutdown(Duration::from_secs(5));
    assert!(!private.exists(), "the private cache stays");
    let saved = storage::load_catalog(&path).unwrap();
    assert!(saved.describes(config.id, None));

    // A shared catalog that was read keeps its own data.
    let mut h = Harness::with_config(warehouse(), config, Some(path), MINUTE);
    let mut other = h.catalog().clone();
    other.fetched_at = Some(5);
    h.worker.seed(qrow::catalog::Seed::Catalog(Arc::new(other)));
    std::thread::sleep(Duration::from_millis(100));
    h.wait(|h| h.status.is_idle());
    assert_eq!(h.catalog().fetched_at, Some(1_000));
}
