//! The cached schemas, relations, and columns of a connection, or of the
//! connections that share one catalog.
//!
//! Schemas and relations are behind `Arc`, so a snapshot for the UI copies
//! only the maps that a refresh changed.

use crate::model::{CatalogSettings, Column, Profile, Row, SharedCatalog, effective_catalog};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

mod worker;

pub use worker::{CatalogWorker, Event, MINUTE, Request, Seed, Status, refresh_due};

/// The cache format. Qrow discards a cache with another version.
pub const CATALOG_VERSION: u32 = 1;

/// The connection settings that select which schemas exist.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CatalogIdentity {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub parameters: BTreeMap<String, String>,
}

impl CatalogIdentity {
    pub fn of(profile: &Profile) -> Self {
        Self {
            host: profile.host.clone(),
            port: profile.port,
            username: profile.username.clone(),
            parameters: profile.parameters.clone(),
        }
    }
}

/// The key of the catalog of `profile`: the shared catalog that it uses, or
/// the profile itself.
pub fn catalog_key(profile: &Profile, shared: &[SharedCatalog]) -> Uuid {
    profile
        .shared_catalog
        .filter(|id| shared.iter().any(|catalog| catalog.id == *id))
        .unwrap_or(profile.id)
}

/// What a catalog worker needs to know about its catalog and the
/// connections that use it.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogConfig {
    /// The profile ID of a private catalog, or the ID of a shared catalog.
    pub id: Uuid,
    pub shared: bool,
    /// The connections that browse the catalog, in sidebar order. Each has
    /// the catalog settings, with its own Logs option.
    pub members: Vec<Profile>,
    /// The settings that decide what a refresh reads.
    pub settings: CatalogSettings,
    /// The member that automatic refreshes use while it has a live session.
    pub preferred: Option<Uuid>,
}

impl CatalogConfig {
    /// The private catalog of one profile.
    pub fn private(profile: Profile) -> Self {
        Self {
            id: profile.id,
            shared: false,
            settings: profile.catalog.clone(),
            members: vec![profile],
            preferred: None,
        }
    }

    /// The catalog with the key `id`, or `None` when no connection browses it.
    pub fn of(id: Uuid, profiles: &[Profile], shared: &[SharedCatalog]) -> Option<Self> {
        if let Some(catalog) = shared.iter().find(|catalog| catalog.id == id) {
            let members: Vec<Profile> = profiles
                .iter()
                .filter(|profile| profile.shared_catalog == Some(id) && profile.catalog.browses())
                .map(|profile| Profile {
                    catalog: effective_catalog(profile, shared),
                    ..profile.clone()
                })
                .collect();
            return (!members.is_empty()).then(|| Self {
                id,
                shared: true,
                members,
                settings: catalog.settings.clone(),
                preferred: catalog.preferred,
            });
        }
        profiles
            .iter()
            .find(|profile| profile.id == id && catalog_key(profile, shared) == id)
            .filter(|profile| profile.catalog.browses())
            .map(|profile| Self::private(profile.clone()))
    }

    /// The connection settings that the cache must match. A shared catalog
    /// has none: its members state that they read the same metastore.
    pub fn identity(&self) -> Option<CatalogIdentity> {
        if self.shared {
            None
        } else {
            self.members.first().map(CatalogIdentity::of)
        }
    }

    pub fn member(&self, id: Uuid) -> Option<&Profile> {
        self.members.iter().find(|member| member.id == id)
    }
}

/// The part of the catalog that a refresh reads again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The schema list, then the relations and columns of each schema.
    Connection,
    /// The relations of one schema and their columns.
    Schema(String),
    /// One relation and its columns.
    Relation(String, String),
}

impl Scope {
    /// Return whether a refresh of `self` also reads everything that `other` reads.
    pub fn covers(&self, other: &Scope) -> bool {
        match (self, other) {
            (Scope::Connection, _) => true,
            (Scope::Schema(schema), Scope::Relation(parent, _)) => schema == parent,
            _ => self == other,
        }
    }
}

/// Seconds since the Unix epoch.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Catalog {
    pub version: u32,
    /// The profile of a private catalog, or the shared catalog.
    #[serde(rename = "profile")]
    pub owner: Uuid,
    /// The connection settings of a private catalog. `None` for a shared one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<CatalogIdentity>,
    /// When Qrow last read the schema list. `None` means never.
    pub fetched_at: Option<u64>,
    /// The error of the last connection refresh. Errors are not saved.
    #[serde(skip)]
    pub error: Option<String>,
    /// A connection refresh that stopped before its end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unfinished: Option<Unfinished>,
    /// The member whose connection refresh recorded `error`.
    #[serde(skip)]
    pub error_member: Option<Uuid>,
    pub schemas: BTreeMap<String, Arc<Schema>>,
}

/// A connection refresh that stopped before its end, for example at its
/// timeout or when the session failed. The next connection refresh in the
/// refresh period continues it and does not read these schemas again.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Unfinished {
    /// When the refresh started, in seconds since the Unix epoch.
    pub started: u64,
    /// The schemas that the refresh read completely.
    pub done: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Schema {
    /// When Qrow last read the relation list.
    pub fetched_at: Option<u64>,
    #[serde(skip)]
    pub error: Option<String>,
    /// The member whose refresh recorded `error`.
    #[serde(skip)]
    pub error_member: Option<Uuid>,
    /// `None` until Qrow reads the relation list.
    pub relations: Option<BTreeMap<String, Arc<Relation>>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Relation {
    pub kind: RelationKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// When Qrow last read the columns.
    pub fetched_at: Option<u64>,
    #[serde(skip)]
    pub error: Option<String>,
    /// The member whose refresh recorded `error`.
    #[serde(skip)]
    pub error_member: Option<Uuid>,
    /// `None` until Qrow reads the columns. Columns are in table order.
    pub columns: Option<Vec<CatalogColumn>>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum RelationKind {
    Table,
    View,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CatalogColumn {
    pub name: String,
    pub data_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// One relation from a relation list.
#[derive(Clone, Debug, PartialEq)]
pub struct RelationEntry {
    pub name: String,
    pub kind: RelationKind,
    pub comment: Option<String>,
}

impl Catalog {
    /// An empty catalog of `owner`, for a cache that matches `identity`.
    pub fn empty(owner: Uuid, identity: Option<CatalogIdentity>) -> Self {
        Self {
            version: CATALOG_VERSION,
            owner,
            identity,
            fetched_at: None,
            error: None,
            unfinished: None,
            error_member: None,
            schemas: BTreeMap::new(),
        }
    }

    /// An empty private catalog of `profile`.
    pub fn new(profile: &Profile) -> Self {
        Self::empty(profile.id, Some(CatalogIdentity::of(profile)))
    }

    /// Return whether this cache describes the catalog of `owner` that
    /// matches `identity`.
    pub fn describes(&self, owner: Uuid, identity: Option<&CatalogIdentity>) -> bool {
        self.owner == owner && self.identity.as_ref() == identity
    }

    pub fn schema(&self, name: &str) -> Option<&Schema> {
        self.schemas.get(name).map(Arc::as_ref)
    }

    pub fn relation(&self, schema: &str, relation: &str) -> Option<&Relation> {
        self.schema(schema)?
            .relations
            .as_ref()?
            .get(relation)
            .map(Arc::as_ref)
    }

    /// The number of relations in the loaded relation lists.
    pub fn relation_count(&self) -> usize {
        self.schemas
            .values()
            .filter_map(|schema| schema.relations.as_ref())
            .map(BTreeMap::len)
            .sum()
    }

    /// Remove the schemas that `settings` hide.
    pub fn retain(&mut self, settings: &CatalogSettings) {
        self.schemas.retain(|name, _| settings.shows(name));
        if let Some(unfinished) = &mut self.unfinished {
            unfinished
                .done
                .retain(|name| self.schemas.contains_key(name));
        }
    }

    /// Replace the schema list. Schemas that remain keep their relations.
    pub fn apply_schemas(&mut self, names: Vec<String>, settings: &CatalogSettings, at: u64) {
        let mut previous = std::mem::take(&mut self.schemas);
        self.schemas = names
            .into_iter()
            .filter(|name| settings.shows(name))
            .map(|name| {
                let schema = previous.remove(&name).unwrap_or_default();
                (name, schema)
            })
            .collect();
        if let Some(unfinished) = &mut self.unfinished {
            unfinished
                .done
                .retain(|name| self.schemas.contains_key(name));
        }
        self.fetched_at = Some(at);
        self.error = None;
        self.error_member = None;
    }

    /// Replace the relation list of `schema`, or only `relation` when given.
    /// Relations that remain keep their columns. A missing relation is removed.
    pub fn apply_relations(
        &mut self,
        schema: &str,
        relation: Option<&str>,
        entries: Vec<RelationEntry>,
        at: u64,
    ) {
        let Some(node) = self.schemas.get_mut(schema) else {
            return;
        };
        let node = Arc::make_mut(node);
        match relation {
            None => {
                let mut previous = node.relations.take().unwrap_or_default();
                node.relations = Some(
                    entries
                        .into_iter()
                        .map(|entry| {
                            let kept = previous.remove(&entry.name);
                            let relation = Relation {
                                kind: entry.kind,
                                comment: entry.comment,
                                fetched_at: kept.as_ref().and_then(|kept| kept.fetched_at),
                                error: None,
                                error_member: None,
                                columns: kept.and_then(|kept| kept.columns.clone()),
                            };
                            (entry.name, Arc::new(relation))
                        })
                        .collect(),
                );
                node.fetched_at = Some(at);
                node.error = None;
                node.error_member = None;
            }
            Some(name) => {
                // Without a relation list, one relation cannot show which others exist.
                let Some(relations) = node.relations.as_mut() else {
                    return;
                };
                match entries.into_iter().find(|entry| entry.name == name) {
                    Some(entry) => {
                        let relation = relations.entry(entry.name).or_insert_with(|| {
                            Arc::new(Relation {
                                kind: entry.kind,
                                comment: None,
                                fetched_at: None,
                                error: None,
                                error_member: None,
                                columns: None,
                            })
                        });
                        let relation = Arc::make_mut(relation);
                        relation.kind = entry.kind;
                        relation.comment = entry.comment;
                        relation.error = None;
                        relation.error_member = None;
                    }
                    None => {
                        relations.remove(name);
                    }
                }
            }
        }
    }

    /// Set the columns of `relation`, or of every relation in `schema`. A
    /// relation without column rows gets an empty column list.
    pub fn apply_columns(
        &mut self,
        schema: &str,
        relation: Option<&str>,
        mut columns: BTreeMap<String, Vec<CatalogColumn>>,
        at: u64,
    ) {
        let Some(relations) = self
            .schemas
            .get_mut(schema)
            .and_then(|node| Arc::make_mut(node).relations.as_mut())
        else {
            return;
        };
        for (name, node) in relations.iter_mut() {
            if relation.is_some_and(|relation| relation != name) {
                continue;
            }
            let node = Arc::make_mut(node);
            node.columns = Some(columns.remove(name).unwrap_or_default());
            node.fetched_at = Some(at);
            node.error = None;
            node.error_member = None;
        }
    }

    /// Record the error of a refresh on the node of `scope`. The error belongs
    /// to `member`, the connection that ran the refresh.
    pub fn set_error(&mut self, scope: &Scope, message: String, member: Uuid) {
        match scope {
            Scope::Connection => {
                self.error = Some(message);
                self.error_member = Some(member);
            }
            Scope::Schema(schema) => {
                if let Some(node) = self.schemas.get_mut(schema) {
                    let node = Arc::make_mut(node);
                    node.error = Some(message);
                    node.error_member = Some(member);
                }
            }
            Scope::Relation(schema, relation) => {
                if let Some(node) = self
                    .schemas
                    .get_mut(schema)
                    .and_then(|node| Arc::make_mut(node).relations.as_mut())
                    .and_then(|relations| relations.get_mut(relation))
                {
                    let node = Arc::make_mut(node);
                    node.error = Some(message);
                    node.error_member = Some(member);
                }
            }
        }
    }
}

impl Catalog {
    /// The error of the last connection refresh that `member` ran. A member
    /// of a shared catalog does not get the errors of the others.
    pub fn error_for(&self, member: Uuid) -> Option<&str> {
        error_for(&self.error, self.error_member, member)
    }
}

impl Schema {
    /// The error of the last refresh of this schema that `member` ran. A
    /// member of a shared catalog does not show the errors of the others.
    pub fn error_for(&self, member: Uuid) -> Option<&str> {
        error_for(&self.error, self.error_member, member)
    }
}

impl Relation {
    /// The error of the last refresh of this relation that `member` ran.
    pub fn error_for(&self, member: Uuid) -> Option<&str> {
        error_for(&self.error, self.error_member, member)
    }
}

fn error_for(error: &Option<String>, owner: Option<Uuid>, member: Uuid) -> Option<&str> {
    error
        .as_deref()
        .filter(|_| owner.is_none_or(|owner| owner == member))
}

/// Quote a Spark SQL identifier, including reserved keywords.
pub fn quote_identifier(name: &str) -> String {
    format!("`{}`", name.replace('`', "``"))
}

/// The name of a relation for SQL, with its schema.
pub fn qualified_name(schema: &str, relation: &str) -> String {
    format!(
        "{}.{}",
        quote_identifier(schema),
        quote_identifier(relation)
    )
}

/// The rows of a catalog request, with its result columns.
pub struct MetadataRows {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
}

impl MetadataRows {
    fn index(&self, name: &str) -> Result<usize> {
        self.columns
            .iter()
            .position(|column| column.name.eq_ignore_ascii_case(name))
            .with_context(|| format!("The catalog result has no {name} column"))
    }

    fn optional_index(&self, name: &str) -> Option<usize> {
        self.columns
            .iter()
            .position(|column| column.name.eq_ignore_ascii_case(name))
    }
}

fn text(row: &Row, index: usize) -> Option<&str> {
    row.get(index)
        .and_then(Option::as_deref)
        .filter(|value| !value.is_empty())
}

/// Read the schema names, without duplicates.
pub fn parse_schemas(rows: &MetadataRows) -> Result<Vec<String>> {
    let schema = rows.index("TABLE_SCHEM")?;
    let mut names: Vec<String> = rows
        .rows
        .iter()
        .filter_map(|row| text(row, schema).map(str::to_owned))
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

/// Read the relations of `schema`, or only `relation` when given.
pub fn parse_relations(
    rows: &MetadataRows,
    schema: &str,
    relation: Option<&str>,
) -> Result<Vec<RelationEntry>> {
    let schema_index = rows.index("TABLE_SCHEM")?;
    let name_index = rows.index("TABLE_NAME")?;
    let kind_index = rows.index("TABLE_TYPE")?;
    let comment_index = rows.optional_index("REMARKS");
    let mut entries: Vec<RelationEntry> = rows
        .rows
        .iter()
        .filter(|row| {
            text(row, schema_index).is_some_and(|value| value.eq_ignore_ascii_case(schema))
        })
        .filter_map(|row| {
            let name = text(row, name_index)?;
            if relation.is_some_and(|relation| !name.eq_ignore_ascii_case(relation)) {
                return None;
            }
            let kind = if text(row, kind_index)
                .is_some_and(|kind| kind.to_ascii_uppercase().contains("VIEW"))
            {
                RelationKind::View
            } else {
                RelationKind::Table
            };
            Some(RelationEntry {
                // The requested name keys the node that the user refreshed.
                name: relation.map_or_else(|| name.to_owned(), str::to_owned),
                kind,
                comment: comment_index.and_then(|index| text(row, index).map(str::to_owned)),
            })
        })
        .collect();
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    entries.dedup_by(|a, b| a.name == b.name);
    Ok(entries)
}

/// Read the columns of `schema`, grouped by relation and in table order.
pub fn parse_columns(
    rows: &MetadataRows,
    schema: &str,
    relation: Option<&str>,
) -> Result<BTreeMap<String, Vec<CatalogColumn>>> {
    let schema_index = rows.index("TABLE_SCHEM")?;
    let relation_index = rows.index("TABLE_NAME")?;
    let name_index = rows.index("COLUMN_NAME")?;
    let type_index = rows.index("TYPE_NAME")?;
    let comment_index = rows.optional_index("REMARKS");
    let position_index = rows.optional_index("ORDINAL_POSITION");
    let mut grouped: BTreeMap<String, Vec<(i64, usize, CatalogColumn)>> = BTreeMap::new();
    for (row_index, row) in rows.rows.iter().enumerate() {
        if !text(row, schema_index).is_some_and(|value| value.eq_ignore_ascii_case(schema)) {
            continue;
        }
        let (Some(owner), Some(name)) = (text(row, relation_index), text(row, name_index)) else {
            continue;
        };
        if relation.is_some_and(|relation| !owner.eq_ignore_ascii_case(relation)) {
            continue;
        }
        let owner = relation.map_or_else(|| owner.to_owned(), str::to_owned);
        let position = position_index
            .and_then(|index| text(row, index))
            .and_then(|value| value.parse().ok())
            .unwrap_or(i64::MAX);
        grouped.entry(owner).or_default().push((
            position,
            row_index,
            CatalogColumn {
                name: name.to_owned(),
                data_type: text(row, type_index).unwrap_or_default().to_owned(),
                comment: comment_index.and_then(|index| text(row, index).map(str::to_owned)),
            },
        ));
    }
    Ok(grouped
        .into_iter()
        .map(|(owner, mut columns)| {
            columns.sort_by_key(|(position, row, _)| (*position, *row));
            (
                owner,
                columns.into_iter().map(|(_, _, column)| column).collect(),
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(columns: &[&str], rows: &[&[Option<&str>]]) -> MetadataRows {
        MetadataRows {
            columns: columns
                .iter()
                .map(|name| Column {
                    name: (*name).into(),
                    data_type: "STRING".into(),
                })
                .collect(),
            rows: rows
                .iter()
                .map(|row| row.iter().map(|value| value.map(str::to_owned)).collect())
                .collect(),
        }
    }

    fn table(name: &str) -> RelationEntry {
        RelationEntry {
            name: name.into(),
            kind: RelationKind::Table,
            comment: None,
        }
    }

    fn column(name: &str) -> CatalogColumn {
        CatalogColumn {
            name: name.into(),
            data_type: "INT".into(),
            comment: None,
        }
    }

    fn loaded() -> Catalog {
        let mut catalog = Catalog::new(&Profile::default());
        let settings = CatalogSettings::default();
        catalog.apply_schemas(vec!["sales".into(), "ops".into()], &settings, 1);
        catalog.apply_relations("sales", None, vec![table("orders"), table("items")], 1);
        catalog.apply_columns(
            "sales",
            None,
            BTreeMap::from([("orders".into(), vec![column("id")])]),
            1,
        );
        catalog
    }

    #[test]
    fn parsing_keeps_only_the_requested_names_in_table_order() {
        let schemas = rows(
            &["TABLE_SCHEM", "TABLE_CATALOG"],
            &[
                &[Some("b"), None],
                &[Some("a"), None],
                &[Some("a"), None],
                &[None, None],
            ],
        );
        assert_eq!(parse_schemas(&schemas).unwrap(), ["a", "b"]);

        // `_` is a pattern character on the server, so `my_db` also returns `myxdb`.
        let relations = rows(
            &[
                "TABLE_CAT",
                "TABLE_SCHEM",
                "TABLE_NAME",
                "TABLE_TYPE",
                "REMARKS",
            ],
            &[
                &[None, Some("my_db"), Some("v"), Some("VIEW"), Some("A view")],
                &[None, Some("my_db"), Some("t"), Some("TABLE"), Some("")],
                &[None, Some("myxdb"), Some("other"), Some("TABLE"), None],
            ],
        );
        assert_eq!(
            parse_relations(&relations, "my_db", None).unwrap(),
            [
                table("t"),
                RelationEntry {
                    name: "v".into(),
                    kind: RelationKind::View,
                    comment: Some("A view".into()),
                },
            ]
        );
        assert_eq!(
            parse_relations(&relations, "my_db", Some("t")).unwrap(),
            [table("t")]
        );

        let columns = rows(
            &[
                "TABLE_SCHEM",
                "TABLE_NAME",
                "COLUMN_NAME",
                "TYPE_NAME",
                "REMARKS",
                "ORDINAL_POSITION",
            ],
            &[
                &[
                    Some("my_db"),
                    Some("t"),
                    Some("b"),
                    Some("INT"),
                    None,
                    Some("2"),
                ],
                &[
                    Some("my_db"),
                    Some("t"),
                    Some("a"),
                    Some("INT"),
                    Some("First"),
                    Some("1"),
                ],
                &[
                    Some("myxdb"),
                    Some("t"),
                    Some("c"),
                    Some("INT"),
                    None,
                    Some("1"),
                ],
            ],
        );
        let parsed = parse_columns(&columns, "my_db", None).unwrap();
        assert_eq!(
            parsed["t"]
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(parsed["t"][0].comment.as_deref(), Some("First"));
        assert!(parse_relations(&rows(&["TABLE_SCHEM"], &[]), "a", None).is_err());
    }

    #[test]
    fn a_relation_list_refresh_keeps_the_columns_of_remaining_relations() {
        let mut catalog = loaded();
        catalog.apply_relations("sales", None, vec![table("orders"), table("refunds")], 2);
        let schema = catalog.schema("sales").unwrap();
        let names: Vec<_> = schema.relations.as_ref().unwrap().keys().collect();
        assert_eq!(names, ["orders", "refunds"]);
        assert_eq!(
            catalog
                .relation("sales", "orders")
                .unwrap()
                .columns
                .as_ref()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(catalog.relation("sales", "refunds").unwrap().columns, None);
        assert_eq!(schema.fetched_at, Some(2));

        // The schema list refresh keeps the relations of remaining schemas.
        catalog.apply_schemas(vec!["sales".into()], &CatalogSettings::default(), 3);
        assert_eq!(catalog.relation_count(), 2);
        assert!(catalog.schema("ops").is_none());
    }

    #[test]
    fn a_relation_refresh_updates_or_removes_only_that_relation() {
        let mut catalog = loaded();
        catalog.apply_relations(
            "sales",
            Some("orders"),
            vec![RelationEntry {
                name: "orders".into(),
                kind: RelationKind::View,
                comment: Some("Now a view".into()),
            }],
            2,
        );
        let orders = catalog.relation("sales", "orders").unwrap();
        assert_eq!(orders.kind, RelationKind::View);
        assert!(orders.columns.is_some());
        catalog.apply_relations("sales", Some("items"), vec![], 2);
        assert!(catalog.relation("sales", "items").is_none());
        assert!(catalog.relation("sales", "orders").is_some());

        // Without a relation list, Qrow does not make a partial one.
        catalog.apply_relations("ops", Some("x"), vec![table("x")], 2);
        assert!(catalog.schema("ops").unwrap().relations.is_none());
    }

    #[test]
    fn a_schema_column_refresh_sets_every_relation() {
        let mut catalog = loaded();
        assert_eq!(
            catalog.relation("sales", "items").unwrap().columns,
            Some(vec![])
        );
        catalog.apply_columns(
            "sales",
            Some("items"),
            BTreeMap::from([("items".into(), vec![column("sku")])]),
            2,
        );
        assert_eq!(
            catalog
                .relation("sales", "items")
                .unwrap()
                .columns
                .as_ref()
                .unwrap()[0]
                .name,
            "sku"
        );
        assert_eq!(
            catalog.relation("sales", "orders").unwrap().fetched_at,
            Some(1)
        );
    }

    #[test]
    fn errors_stay_on_their_node_and_a_success_clears_them() {
        let mut catalog = loaded();
        catalog.set_error(&Scope::Schema("sales".into()), "denied".into(), Uuid::nil());
        catalog.set_error(
            &Scope::Relation("sales".into(), "orders".into()),
            "broken".into(),
            Uuid::nil(),
        );
        assert_eq!(
            catalog.schema("sales").unwrap().error.as_deref(),
            Some("denied")
        );
        assert_eq!(
            catalog
                .relation("sales", "orders")
                .unwrap()
                .error
                .as_deref(),
            Some("broken")
        );
        // Only the member that ran the refresh sees the error.
        let other = Uuid::from_u128(1);
        let sales = catalog.schema("sales").unwrap();
        assert_eq!(sales.error_for(Uuid::nil()), Some("denied"));
        assert_eq!(sales.error_for(other), None);
        let orders = catalog.relation("sales", "orders").unwrap();
        assert_eq!(orders.error_for(Uuid::nil()), Some("broken"));
        assert_eq!(orders.error_for(other), None);
        catalog.apply_relations("sales", None, vec![table("orders")], 2);
        let sales = catalog.schema("sales").unwrap();
        assert_eq!((sales.error.as_deref(), sales.error_member), (None, None));
        catalog.apply_columns("sales", None, BTreeMap::new(), 2);
        let orders = catalog.relation("sales", "orders").unwrap();
        assert_eq!((orders.error.as_deref(), orders.error_member), (None, None));
    }

    #[test]
    fn filters_apply_to_the_schema_list_and_to_the_cache() {
        let settings = CatalogSettings {
            include: vec!["sales*".into(), "OPS".into()],
            exclude: vec!["*_tmp".into()],
            ..CatalogSettings::default()
        };
        let mut catalog = Catalog::new(&Profile::default());
        catalog.apply_schemas(
            vec![
                "sales".into(),
                "sales_tmp".into(),
                "ops".into(),
                "hr".into(),
            ],
            &settings,
            1,
        );
        assert_eq!(catalog.schemas.keys().collect::<Vec<_>>(), ["ops", "sales"]);
        catalog.retain(&CatalogSettings {
            include: vec![],
            exclude: vec!["o?s".into()],
            ..CatalogSettings::default()
        });
        assert_eq!(catalog.schemas.keys().collect::<Vec<_>>(), ["sales"]);
    }

    #[test]
    fn names_are_quoted_for_sql() {
        assert_eq!(
            qualified_name("sales_eu", "Orders2"),
            "`sales_eu`.`Orders2`"
        );
        assert_eq!(qualified_name("select", "from"), "`select`.`from`");
        assert_eq!(qualified_name("2024", "a-b"), "`2024`.`a-b`");
        assert_eq!(quote_identifier("odd`name"), "`odd``name`");
        assert_eq!(quote_identifier(""), "``");
    }

    #[test]
    fn scopes_cover_their_own_relations_only() {
        let schema = Scope::Schema("a".into());
        assert!(schema.covers(&Scope::Relation("a".into(), "t".into())));
        assert!(!schema.covers(&Scope::Relation("b".into(), "t".into())));
        assert!(Scope::Connection.covers(&schema));
        assert!(Scope::Connection.covers(&Scope::Relation("b".into(), "t".into())));
        assert!(!schema.covers(&Scope::Connection));
    }

    #[test]
    fn the_cache_round_trips_without_errors_and_detects_other_connections() {
        let mut catalog = loaded();
        catalog.set_error(&Scope::Connection, "offline".into(), catalog.owner);
        let restored: Catalog =
            serde_json::from_slice(&serde_json::to_vec(&catalog).unwrap()).unwrap();
        assert_eq!(restored.error, None);
        assert_eq!(restored.error_member, None);
        assert_eq!(restored.schemas, catalog.schemas);
        let mut profile = Profile {
            id: catalog.owner,
            ..Profile::default()
        };
        let describes =
            |profile: &Profile| catalog.describes(profile.id, Some(&CatalogIdentity::of(profile)));
        assert!(describes(&profile));
        profile.database = "other".into();
        assert!(describes(&profile));
        profile.host = "elsewhere".into();
        assert!(!describes(&profile));
        // A shared catalog has no identity, so no member clears it.
        let shared = Catalog::empty(Uuid::new_v4(), None);
        assert!(shared.describes(shared.owner, None));
        assert!(!shared.describes(shared.owner, Some(&CatalogIdentity::of(&profile))));
    }

    #[test]
    fn a_shared_catalog_has_the_browsing_members_with_their_settings() {
        use crate::model::{CatalogRefresh, SharedCatalog};
        let shared = SharedCatalog {
            id: Uuid::new_v4(),
            name: "Lake".into(),
            settings: CatalogSettings {
                refresh: CatalogRefresh::Manual,
                include: vec!["sales".into()],
                ..CatalogSettings::default()
            },
            preferred: None,
        };
        let browsing = CatalogSettings {
            refresh: CatalogRefresh::Manual,
            log_refreshes: true,
            ..CatalogSettings::default()
        };
        let member = |name: &str, catalog: CatalogSettings| Profile {
            name: name.into(),
            catalog,
            shared_catalog: Some(shared.id),
            ..Profile::default()
        };
        let small = member("small", browsing.clone());
        let off = member("off", CatalogSettings::default());
        let large = member("large", browsing.clone());
        let private = Profile {
            name: "private".into(),
            catalog: browsing,
            ..Profile::default()
        };
        let profiles = vec![small.clone(), off.clone(), large, private.clone()];
        let catalogs = std::slice::from_ref(&shared);

        assert_eq!(catalog_key(&small, catalogs), shared.id);
        assert_eq!(catalog_key(&private, catalogs), private.id);
        assert_eq!(catalog_key(&small, &[]), small.id);

        let config = CatalogConfig::of(shared.id, &profiles, catalogs).unwrap();
        assert!(config.shared);
        assert_eq!(config.identity(), None);
        let members: Vec<_> = config.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(members, ["small", "large"]);
        assert_eq!(config.members[0].catalog.include, ["sales"]);
        assert!(config.members[0].catalog.log_refreshes);
        assert!(config.member(off.id).is_none());

        let config = CatalogConfig::of(private.id, &profiles, catalogs).unwrap();
        assert!(!config.shared);
        assert_eq!(config.identity(), Some(CatalogIdentity::of(&private)));
        // A member has no private catalog, and a connection that does not
        // browse has no catalog.
        assert!(CatalogConfig::of(small.id, &profiles, catalogs).is_none());
        let none = [off];
        assert!(CatalogConfig::of(shared.id, &none, catalogs).is_none());
    }

    #[test]
    fn a_member_sees_only_its_own_refreshes() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let status = Status {
            active: Some(Scope::Connection),
            runner: Some(a),
            queued: vec![
                Request {
                    member: b,
                    scope: Scope::Schema("sales".into()),
                },
                Request {
                    member: a,
                    scope: Scope::Schema("hr".into()),
                },
            ],
            done: 2,
            total: 5,
        };
        let mine = status.of_member(a);
        assert_eq!(mine.active, Some(Scope::Connection));
        assert_eq!((mine.done, mine.total), (2, 5));
        assert_eq!(mine.queued.len(), 1);
        let theirs = status.of_member(b);
        assert_eq!(theirs.active, None);
        assert_eq!((theirs.done, theirs.total), (0, 0));
        assert!(theirs.includes(&Scope::Schema("sales".into())));
        assert!(!theirs.includes(&Scope::Schema("hr".into())));
    }
}
