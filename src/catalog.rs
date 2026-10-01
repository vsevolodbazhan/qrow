//! The cached schemas, relations, and columns of a connection.
//!
//! Schemas and relations are behind `Arc`, so a snapshot for the UI copies
//! only the maps that a refresh changed.

use crate::model::{CatalogSettings, Column, Profile, Row};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

mod worker;

pub use worker::{CatalogWorker, Event, Status};

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

/// The part of the catalog that a refresh reads again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The schema list and the relations of each schema, without columns.
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
    pub profile: Uuid,
    pub identity: CatalogIdentity,
    /// When Qrow last read the schema list. `None` means never.
    pub fetched_at: Option<u64>,
    /// The error of the last connection refresh. Errors are not saved.
    #[serde(skip)]
    pub error: Option<String>,
    pub schemas: BTreeMap<String, Arc<Schema>>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Schema {
    /// When Qrow last read the relation list.
    pub fetched_at: Option<u64>,
    #[serde(skip)]
    pub error: Option<String>,
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
    pub fn new(profile: &Profile) -> Self {
        Self {
            version: CATALOG_VERSION,
            profile: profile.id,
            identity: CatalogIdentity::of(profile),
            fetched_at: None,
            error: None,
            schemas: BTreeMap::new(),
        }
    }

    /// Return whether this cache describes the catalog of `profile`.
    pub fn describes(&self, profile: &Profile) -> bool {
        self.profile == profile.id && self.identity == CatalogIdentity::of(profile)
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
        self.fetched_at = Some(at);
        self.error = None;
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
                                columns: kept.and_then(|kept| kept.columns.clone()),
                            };
                            (entry.name, Arc::new(relation))
                        })
                        .collect(),
                );
                node.fetched_at = Some(at);
                node.error = None;
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
                                columns: None,
                            })
                        });
                        let relation = Arc::make_mut(relation);
                        relation.kind = entry.kind;
                        relation.comment = entry.comment;
                        relation.error = None;
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
        }
    }

    /// Record the error of a refresh on the node of `scope`.
    pub fn set_error(&mut self, scope: &Scope, message: String) {
        match scope {
            Scope::Connection => self.error = Some(message),
            Scope::Schema(schema) => {
                if let Some(node) = self.schemas.get_mut(schema) {
                    Arc::make_mut(node).error = Some(message);
                }
            }
            Scope::Relation(schema, relation) => {
                if let Some(node) = self
                    .schemas
                    .get_mut(schema)
                    .and_then(|node| Arc::make_mut(node).relations.as_mut())
                    .and_then(|relations| relations.get_mut(relation))
                {
                    Arc::make_mut(node).error = Some(message);
                }
            }
        }
    }
}

/// Quote a Spark SQL identifier with backticks when it is not a plain name.
pub fn quote_identifier(name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain {
        name.to_owned()
    } else {
        format!("`{}`", name.replace('`', "``"))
    }
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
        catalog.set_error(&Scope::Schema("sales".into()), "denied".into());
        catalog.set_error(
            &Scope::Relation("sales".into(), "orders".into()),
            "broken".into(),
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
        catalog.apply_relations("sales", None, vec![table("orders")], 2);
        assert_eq!(catalog.schema("sales").unwrap().error, None);
        catalog.apply_columns("sales", None, BTreeMap::new(), 2);
        assert_eq!(catalog.relation("sales", "orders").unwrap().error, None);
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
    fn names_are_quoted_only_when_they_need_it() {
        assert_eq!(qualified_name("sales_eu", "Orders2"), "sales_eu.Orders2");
        assert_eq!(qualified_name("2024", "a-b"), "`2024`.`a-b`");
        assert_eq!(quote_identifier("odd`name"), "`odd``name`");
        assert_eq!(quote_identifier(""), "``");
    }

    #[test]
    fn scopes_cover_their_own_relations_only() {
        let schema = Scope::Schema("a".into());
        assert!(schema.covers(&Scope::Relation("a".into(), "t".into())));
        assert!(!schema.covers(&Scope::Relation("b".into(), "t".into())));
        // A connection refresh does not read columns.
        assert!(!Scope::Connection.covers(&schema));
        assert!(Scope::Connection.covers(&Scope::Connection));
    }

    #[test]
    fn the_cache_round_trips_without_errors_and_detects_other_connections() {
        let mut catalog = loaded();
        catalog.set_error(&Scope::Connection, "offline".into());
        let restored: Catalog =
            serde_json::from_slice(&serde_json::to_vec(&catalog).unwrap()).unwrap();
        assert_eq!(restored.error, None);
        assert_eq!(restored.schemas, catalog.schemas);
        let mut profile = Profile {
            id: catalog.profile,
            ..Profile::default()
        };
        assert!(catalog.describes(&profile));
        profile.database = "other".into();
        assert!(catalog.describes(&profile));
        profile.host = "elsewhere".into();
        assert!(!catalog.describes(&profile));
    }
}
