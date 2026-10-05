//! Read-only views of the dbt project of a connection for the assistant: the
//! state of the project in the workspace context, a short summary of each
//! relation that the tab SQL names, and the details of one relation.
//!
//! Each view is bounded. Lineage and test targets name the catalog relation
//! when one matches, otherwise the dbt unique ID.

use crate::{
    catalog::Catalog,
    dbt::worker::ManifestState,
    dbt::{
        Entry, Index, Kind, Test,
        matching::{self, CatalogNames, Match},
    },
    model::DbtProject,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

/// The largest dbt part of one relation in a tool result.
pub const MAX_DESCRIBE_BYTES: usize = 24 * 1024;
/// The largest dbt summary of one relation in the workspace context.
pub const MAX_SUMMARY_BYTES: usize = 1024;
/// The longest description of a relation or a column that a view gives.
const MAX_DESCRIPTION_BYTES: usize = 2 * 1024;
/// The longest description in a summary.
const MAX_SUMMARY_DESCRIPTION_BYTES: usize = 400;
/// The most columns, tests, parents, or children that a view starts with.
const MAX_ITEMS: usize = 200;

/// The dbt project of a connection, with its index and catalog.
pub struct Project<'a> {
    pub index: &'a Index,
    pub project: &'a DbtProject,
    names: Option<CatalogNames>,
    /// When Qrow made the index.
    pub refreshed: Option<SystemTime>,
    /// Whether the manifest changed after Qrow made the index.
    pub changed: bool,
}

impl<'a> Project<'a> {
    /// `catalog` is the catalog of the connection, if Qrow has it.
    pub fn new(
        index: &'a Index,
        project: &'a DbtProject,
        catalog: Option<&Catalog>,
        refreshed: Option<SystemTime>,
        changed: bool,
    ) -> Self {
        Self {
            index,
            project,
            names: catalog.map(CatalogNames::of),
            refreshed,
            changed,
        }
    }

    /// The dbt resource of a catalog relation.
    pub fn entry_for(&self, schema: &str, relation: &str) -> Option<u32> {
        matching::entry_for(self.index, self.project, schema, relation)
    }

    /// The catalog relation of `position`, like `core.orders`, or its unique
    /// ID when no relation matches.
    fn name(&self, position: u32) -> String {
        let entry = self.index.entry(position);
        match self
            .names
            .as_ref()
            .map(|names| matching::find(self.index, entry, self.project, names))
        {
            Some(Match::Relation { schema, relation }) => format!("{schema}.{relation}"),
            _ => entry.unique_id.to_string(),
        }
    }

    fn manifest(&self) -> Value {
        json!({
            "generated_at": self.index.generated_at,
            "dbt_version": self.index.dbt_version,
            "refreshed_at": self.refreshed.and_then(seconds),
            "changed": self.changed,
        })
    }

    fn test(&self, test: &Test) -> Value {
        let mut value = json!({
            "test": self.index.symbol(test.name),
            "column": test.column,
        });
        if !test.values.is_empty() {
            value["values"] = json!(test.values);
        }
        if test.to_text.is_some() {
            value["to"] = json!(
                test.to
                    .map(|to| self.name(to))
                    .or_else(|| test.to_text.as_deref().map(str::to_owned))
            );
            value["field"] = json!(test.field);
        }
        if let Some(arguments) = &test.arguments {
            value["arguments"] = serde_json::from_str(arguments).unwrap_or(Value::Null);
        }
        value
    }

    /// The details of the resource at `position`, in at most `budget`
    /// bytes. Lists that do not fit are shorter and say so.
    pub fn describe(&self, position: u32, budget: usize) -> Value {
        let index = self.index;
        let entry = index.entry(position);
        let columns: Vec<Value> = entry
            .columns
            .iter()
            .filter(|column| !column.description.is_empty() || column.data_type.is_some())
            .map(|column| {
                json!({
                    "name": column.name,
                    "description": cut(&column.description, MAX_DESCRIPTION_BYTES),
                    "data_type": column.data_type.map(|data_type| index.symbol(data_type)),
                })
            })
            .collect();
        let tests: Vec<Value> = index
            .tests(position)
            .iter()
            .map(|test| self.test(test))
            .collect();
        let parents: Vec<String> = entry.parents.iter().map(|p| self.name(*p)).collect();
        let children: Vec<String> = index
            .children(position)
            .iter()
            .map(|child| self.name(*child))
            .collect();
        let mut limit = MAX_ITEMS;
        loop {
            let take = |count: usize| count.min(limit);
            let value = json!({
                "unique_id": entry.unique_id,
                "resource_type": entry.kind.name(),
                "name": entry.name,
                "materialized": entry.materialized.map(|m| index.symbol(m)),
                "description": cut(&entry.description, MAX_DESCRIPTION_BYTES),
                "tags": entry.tags.iter().map(|tag| index.symbol(*tag)).collect::<Vec<_>>(),
                "path": entry.path,
                "source_name": entry.source_name.map(|name| index.symbol(name)),
                "columns": &columns[..take(columns.len())],
                "columns_truncated": take(columns.len()) < columns.len(),
                "tests": &tests[..take(tests.len())],
                "tests_truncated": take(tests.len()) < tests.len(),
                "parents": &parents[..take(parents.len())],
                "parents_truncated": take(parents.len()) < parents.len(),
                "children": &children[..take(children.len())],
                "children_truncated": take(children.len()) < children.len(),
                "manifest": self.manifest(),
            });
            if size(&value) <= budget || limit == 0 {
                return value;
            }
            limit /= 2;
        }
    }

    /// A short summary of the resource at `position` for the workspace
    /// context: its description, the columns with `unique` or `not_null`
    /// tests, and its relationships.
    pub fn summary(&self, position: u32) -> Value {
        let index = self.index;
        let entry = index.entry(position);
        let tests = index.tests(position);
        let columns_with = |name: &str| -> Vec<&str> {
            let mut columns: Vec<&str> = tests
                .iter()
                .filter(|test| index.symbol(test.name) == name)
                .filter_map(|test| test.column.as_deref())
                .collect();
            columns.dedup();
            columns
        };
        let relationships: Vec<Value> = tests
            .iter()
            .filter(|test| test.to_text.is_some())
            .map(|test| {
                json!({
                    "column": test.column,
                    "to": test.to.map(|to| self.name(to)).or_else(|| test.to_text.as_deref().map(str::to_owned)),
                    "field": test.field,
                })
            })
            .collect();
        let mut limit = relationships.len();
        let mut description = MAX_SUMMARY_DESCRIPTION_BYTES;
        loop {
            let value = json!({
                "unique_id": entry.unique_id,
                "description": cut(&entry.description, description),
                "unique": columns_with("unique"),
                "not_null": columns_with("not_null"),
                "relationships": &relationships[..limit],
                "relationships_truncated": limit < relationships.len(),
            });
            if size(&value) <= MAX_SUMMARY_BYTES || (limit == 0 && description == 0) {
                return value;
            }
            if limit > 0 {
                limit /= 2;
            } else {
                description /= 2;
            }
        }
    }

    /// The state of the project in the workspace context.
    pub fn context(&self) -> DbtContext {
        let count = |kind| {
            self.index
                .entries()
                .iter()
                .filter(|entry: &&Entry| entry.kind == kind)
                .count()
        };
        let summary = self
            .names
            .as_ref()
            .map(|names| matching::summary(self.index, self.project, names));
        DbtContext {
            project: Some(self.index.project.to_string()),
            dbt_version: Some(self.index.dbt_version.to_string()),
            manifest_generated_at: Some(self.index.generated_at.to_string()),
            refreshed_at: self.refreshed.and_then(seconds),
            manifest_changed: self.changed,
            models: Some(count(Kind::Model)),
            sources: Some(count(Kind::Source)),
            tests: Some(self.index.test_count()),
            has_compiled_sql: Some(self.index.has_compiled_code),
            relations: summary.as_ref().map(|summary| summary.total),
            matched: summary.as_ref().map(|summary| summary.matched),
            reading: false,
            error: None,
        }
    }
}

/// The dbt project of the connection of a conversation, in the workspace
/// context.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DbtContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dbt_version: Option<String>,
    /// When dbt wrote the manifest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_generated_at: Option<String>,
    /// When Qrow read the manifest, in seconds since 1970.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refreshed_at: Option<u64>,
    /// The manifest changed after Qrow read it. A refresh can follow.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub manifest_changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub models: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tests: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_compiled_sql: Option<bool>,
    /// The models, seeds, snapshots, and sources that are relations, and
    /// those that match a catalog relation, when Qrow has the catalog.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relations: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<usize>,
    /// Qrow reads the manifest for the first time.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub reading: bool,
    /// Why Qrow has no current data of the manifest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The state of the dbt project of a connection. `project` is `None` while
/// Qrow has no index of the manifest.
pub fn context(state: &ManifestState, project: Option<&Project>) -> DbtContext {
    let mut context = project.map(Project::context).unwrap_or_default();
    context.reading = state.parsing && project.is_none();
    context.error = state.error.as_ref().map(ToString::to_string);
    context
}

fn seconds(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

fn size(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

/// `text` cut to at most `limit` bytes, with `…` at the end when it is cut.
fn cut(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit.saturating_sub('…'.len_utf8());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        assistant::catalog::{CatalogContext, MAX_REFERENCED_BYTES},
        catalog::{CatalogColumn, RelationEntry, RelationKind},
        model::{CatalogSettings, DbtRefresh, SchemaRule, SchemaRuleKind},
    };
    use std::collections::BTreeMap;

    /// Orders in dev_core, built from a raw source, with tests that refer
    /// to customers, a model without a table in the catalog.
    fn manifest(description: &str) -> Index {
        let value = json!({
            "metadata": {
                "dbt_schema_version": "https://schemas.getdbt.com/dbt/manifest/v12.json",
                "dbt_version": "1.12.5", "generated_at": "2026-10-01T08:00:00Z",
                "project_name": "lake", "adapter_type": "spark",
            },
            "nodes": {
                "model.lake.orders": {
                    "unique_id": "model.lake.orders", "resource_type": "model", "name": "orders",
                    "schema": "dev_core", "alias": "orders", "relation_name": "`dev_core`.`orders`",
                    "description": description, "tags": ["finance"],
                    "config": {"materialized": "table"},
                    "columns": {
                        "id": {"name": "id", "description": "The key."},
                        "customer_id": {"name": "customer_id", "description": "", "data_type": "bigint"},
                        "note": {"name": "note", "description": ""},
                    },
                    "depends_on": {"nodes": ["source.lake.raw.orders"]},
                    "original_file_path": "models/orders.sql",
                },
                "model.lake.customers": {
                    "unique_id": "model.lake.customers", "resource_type": "model", "name": "customers",
                    "schema": "dev_core", "alias": "customers", "relation_name": "`dev_core`.`customers`",
                    "config": {"materialized": "table"},
                    "depends_on": {"nodes": ["model.lake.orders"]},
                },
                "test.lake.unique_orders_id": {
                    "unique_id": "test.lake.unique_orders_id", "resource_type": "test",
                    "attached_node": "model.lake.orders", "column_name": "id",
                    "test_metadata": {"name": "unique", "kwargs": {"column_name": "id"}},
                },
                "test.lake.not_null_orders_id": {
                    "unique_id": "test.lake.not_null_orders_id", "resource_type": "test",
                    "attached_node": "model.lake.orders", "column_name": "id",
                    "test_metadata": {"name": "not_null", "kwargs": {"column_name": "id"}},
                },
                "test.lake.relationships_orders_customer_id": {
                    "unique_id": "test.lake.relationships_orders_customer_id", "resource_type": "test",
                    "attached_node": "model.lake.orders", "column_name": "customer_id",
                    "depends_on": {"nodes": ["model.lake.customers", "model.lake.orders"]},
                    "test_metadata": {"name": "relationships", "kwargs": {
                        "column_name": "customer_id", "to": "ref('customers')", "field": "id"}},
                },
                "test.lake.accepted_values_orders_note": {
                    "unique_id": "test.lake.accepted_values_orders_note", "resource_type": "test",
                    "attached_node": "model.lake.orders", "column_name": "note",
                    "test_metadata": {"name": "accepted_values", "kwargs": {
                        "column_name": "note", "values": ["a", "b"]}},
                },
            },
            "sources": {
                "source.lake.raw.orders": {
                    "unique_id": "source.lake.raw.orders", "resource_type": "source", "name": "orders",
                    "schema": "raw", "identifier": "orders", "source_name": "raw",
                },
            },
        });
        crate::dbt::parse(&serde_json::to_vec(&value).unwrap()).unwrap()
    }

    fn project() -> DbtProject {
        DbtProject {
            manifest: "/lake/target/manifest.json".into(),
            refresh: DbtRefresh::Manual,
            schema_mapping: vec![SchemaRule {
                kind: SchemaRuleKind::Prefix,
                from: "dev_".into(),
                to: String::new(),
            }],
        }
    }

    fn catalog() -> Catalog {
        let mut catalog = Catalog::empty(uuid::Uuid::nil(), None);
        catalog.apply_schemas(
            vec!["core".into(), "raw".into()],
            &CatalogSettings::default(),
            1,
        );
        for (schema, name) in [("core", "Orders"), ("raw", "orders")] {
            let entry = RelationEntry {
                name: name.into(),
                kind: RelationKind::Table,
                comment: None,
            };
            catalog.apply_relations(schema, None, vec![entry], 1);
        }
        catalog.apply_columns(
            "core",
            Some("Orders"),
            BTreeMap::from([(
                "Orders".into(),
                vec![CatalogColumn {
                    name: "id".into(),
                    data_type: "bigint".into(),
                    comment: None,
                }],
            )]),
            1,
        );
        catalog
    }

    #[test]
    fn a_described_relation_has_its_dbt_meaning_and_lineage() {
        let index = manifest("One row for each order.");
        let (project, catalog) = (project(), catalog());
        let dbt = Project::new(&index, &project, Some(&catalog), None, false);
        let position = dbt.entry_for("CORE", "orders").unwrap();
        let value = dbt.describe(position, MAX_DESCRIBE_BYTES);
        assert_eq!(value["unique_id"], "model.lake.orders");
        assert_eq!(value["materialized"], "table");
        assert_eq!(value["description"], "One row for each order.");
        assert_eq!(value["tags"], json!(["finance"]));
        // Columns without a description or a type do not count.
        assert_eq!(
            value["columns"],
            json!([
                {"name": "id", "description": "The key.", "data_type": null},
                {"name": "customer_id", "description": "", "data_type": "bigint"},
            ])
        );
        assert_eq!(
            value["tests"],
            json!([
                {"test": "unique", "column": "id"},
                {"test": "not_null", "column": "id"},
                {"test": "relationships", "column": "customer_id",
                 "to": "model.lake.customers", "field": "id"},
                {"test": "accepted_values", "column": "note", "values": ["a", "b"]},
            ])
        );
        // A parent with a table has its catalog name, and a child without
        // one has its unique ID.
        assert_eq!(value["parents"], json!(["raw.orders"]));
        assert_eq!(value["children"], json!(["model.lake.customers"]));
        assert_eq!(value["manifest"]["generated_at"], "2026-10-01T08:00:00Z");
        assert_eq!(value["manifest"]["changed"], false);

        // A small budget cuts the lists and tells so.
        let small = dbt.describe(position, 600);
        assert!(size(&small) <= 600 || small["tests"].as_array().unwrap().is_empty());
        assert_eq!(small["tests_truncated"], true);
    }

    #[test]
    fn a_summary_has_keys_and_relationships_in_a_small_budget() {
        let index = manifest(&"Long text. ".repeat(500));
        let (project, catalog) = (project(), catalog());
        let dbt = Project::new(&index, &project, Some(&catalog), None, true);
        let position = dbt.entry_for("core", "orders").unwrap();
        let summary = dbt.summary(position);
        assert!(size(&summary) <= MAX_SUMMARY_BYTES, "{summary}");
        assert_eq!(summary["unique"], json!(["id"]));
        assert_eq!(summary["not_null"], json!(["id"]));
        assert_eq!(
            summary["relationships"],
            json!([{"column": "customer_id", "to": "model.lake.customers", "field": "id"}])
        );
        assert!(summary["description"].as_str().unwrap().ends_with('…'));

        let context = dbt.context();
        assert_eq!(context.models, Some(2));
        assert_eq!(context.sources, Some(1));
        assert_eq!(context.tests, Some(4));
        assert_eq!((context.relations, context.matched), (Some(3), Some(2)));
        assert!(context.manifest_changed);
    }

    #[test]
    fn the_workspace_context_adds_dbt_summaries_to_referenced_relations() {
        let index = manifest("Orders.");
        let (project, catalog) = (project(), catalog());
        let dbt = Project::new(&index, &project, Some(&catalog), None, false);
        let context = CatalogContext::new(
            uuid::Uuid::nil(),
            true,
            None,
            Some(&catalog),
            &CatalogSettings::default(),
            "SELECT id FROM core.orders",
            "core",
            1,
            Some(&dbt),
        );
        let [relation] = context.referenced_relations.as_slice() else {
            panic!("one relation");
        };
        assert_eq!(
            relation.dbt.as_ref().unwrap()["unique_id"],
            "model.lake.orders"
        );
        assert!(size(&serde_json::to_value(&context).unwrap()) < MAX_REFERENCED_BYTES);

        // A manifest that Qrow reads for the first time, or that failed.
        let state = ManifestState {
            parsing: true,
            ..ManifestState::default()
        };
        let reading = super::context(&state, None);
        assert!(reading.reading && reading.models.is_none());
        let state = ManifestState {
            error: Some(crate::dbt::worker::ManifestError::NotFound),
            ..ManifestState::default()
        };
        let failed = serde_json::to_value(super::context(&state, Some(&dbt))).unwrap();
        assert_eq!(
            failed["error"],
            "Manifest not found: run dbt parse in the project"
        );
        assert_eq!(failed["models"], 2);
    }
}
