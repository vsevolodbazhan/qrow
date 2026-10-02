//! Read-only views of a cached schema catalog for the assistant tools and the
//! workspace context. Each view is bounded, and it tells when Qrow read the
//! data and whether the data is older than the refresh period.

use super::broker::MAX_TOOL_OUTPUT_BYTES;
use crate::catalog::{Catalog, Relation, RelationKind, Scope};
use crate::model::{CatalogSettings, glob_match};
use serde::Serialize;
use serde_json::{Value, json};

/// The schemas or relations that one tool call lists by default.
pub const DEFAULT_PAGE: usize = 200;
/// The most schemas or relations that one tool call lists.
pub const MAX_PAGE: usize = 1_000;
/// The largest part of the workspace context that lists the columns of the
/// relations that the tab SQL names.
pub const MAX_REFERENCED_BYTES: usize = 16 * 1024;
/// The space that the envelope of a tool result can use.
const ENVELOPE_BYTES: usize = 1024;

/// Why a view has no data. The tool decides to read it or to tell the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Missing {
    /// Qrow never read the schema list. Only a connection refresh reads it.
    Schemas,
    /// The schema list does not have the schema.
    SchemaNotFound,
    /// The settings of the connection hide the schema.
    SchemaHidden,
    /// Qrow did not read the relations of the schema.
    Relations(Scope),
    /// The relation list of the schema does not have the relation.
    RelationNotFound,
    /// Qrow did not read the columns of the relation.
    Columns(Scope),
}

impl Missing {
    /// The refresh that reads the missing data, if a refresh can.
    pub fn scope(&self) -> Option<Scope> {
        match self {
            Missing::Schemas => Some(Scope::Connection),
            Missing::Relations(scope) | Missing::Columns(scope) => Some(scope.clone()),
            _ => None,
        }
    }

    /// The error code and message for the model.
    pub fn error(&self) -> (&'static str, &'static str) {
        match self {
            Missing::Schemas => (
                "not_cached",
                "Qrow has not read the schemas of this connection, and no tab of the connection is connected. Ask the user to refresh the connection in the Connections sidebar, or run SHOW SCHEMAS with run_selected_tab_query.",
            ),
            Missing::SchemaNotFound => (
                "not_found",
                "The cached schema list does not have this schema. Check the name with list_schemas. The schema can be new; then ask the user to refresh the connection.",
            ),
            Missing::SchemaHidden => (
                "schema_hidden",
                "The Show schemas or Hide schemas settings of this connection hide this schema. Ask the user to change them, or run SHOW TABLES with run_selected_tab_query.",
            ),
            Missing::Relations(_) => (
                "not_cached",
                "Qrow has not read the relations of this schema, and no tab of the connection is connected. Ask the user to refresh the schema in the Connections sidebar, or run SHOW TABLES with run_selected_tab_query.",
            ),
            Missing::RelationNotFound => (
                "not_found",
                "The cached relation list of this schema does not have this relation. Check the name with list_relations. The relation can be new; then ask the user to refresh the schema.",
            ),
            Missing::Columns(_) => (
                "not_cached",
                "Qrow has not read the columns of this relation, and no tab of the connection is connected. Ask the user to refresh it in the Connections sidebar, or run DESCRIBE with run_selected_tab_query.",
            ),
        }
    }
}

/// Whether data that Qrow read at `fetched_at` is older than the refresh
/// period of `settings`. Data that Qrow never read is stale.
pub fn stale(fetched_at: Option<u64>, settings: &CatalogSettings, now: u64) -> bool {
    fetched_at.is_none_or(|at| now.saturating_sub(at) > u64::from(settings.refresh_minutes) * 60)
}

fn kind(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::Table => "table",
        RelationKind::View => "view",
    }
}

fn fits(value: &Value) -> bool {
    serde_json::to_vec(value)
        .is_ok_and(|bytes| bytes.len() + ENVELOPE_BYTES <= MAX_TOOL_OUTPUT_BYTES)
}

/// Build a page of `items` with `page`, halving the page until it fits in a
/// tool result.
fn bounded_page<T>(
    items: &[T],
    offset: usize,
    limit: usize,
    page: impl Fn(&[T], Option<usize>) -> Value,
) -> Value {
    let start = offset.min(items.len());
    let mut count = limit.clamp(1, MAX_PAGE).min(items.len() - start);
    loop {
        let end = start + count;
        let next = (end < items.len()).then_some(end);
        let value = page(&items[start..end], next);
        if fits(&value) || count <= 1 {
            return value;
        }
        count /= 2;
    }
}

/// Find a schema by its name, or by its name in another letter case.
fn schema_name<'a>(catalog: &'a Catalog, name: &str) -> Option<&'a String> {
    catalog
        .schemas
        .get_key_value(name)
        .map(|(key, _)| key)
        .or_else(|| {
            catalog
                .schemas
                .keys()
                .find(|key| key.eq_ignore_ascii_case(name))
        })
}

/// The schema list of `catalog`, from `offset`.
pub fn schemas(
    catalog: &Catalog,
    settings: &CatalogSettings,
    now: u64,
    offset: usize,
    limit: usize,
) -> Result<Value, Missing> {
    if catalog.fetched_at.is_none() && catalog.schemas.is_empty() {
        return Err(Missing::Schemas);
    }
    let entries: Vec<(&String, _)> = catalog.schemas.iter().collect();
    Ok(bounded_page(&entries, offset, limit, |page, next| {
        json!({
            "fetched_at": catalog.fetched_at,
            "stale": stale(catalog.fetched_at, settings, now),
            "error": catalog.error,
            "schema_count": entries.len(),
            "offset": offset,
            "next_offset": next,
            "schemas": page.iter().map(|(name, schema)| json!({
                "name": name,
                "relation_count": schema.relations.as_ref().map(|relations| relations.len()),
                "fetched_at": schema.fetched_at,
                "error": schema.error,
            })).collect::<Vec<_>>(),
        })
    }))
}

/// The relations of `schema` whose names match `pattern`, from `offset`.
pub fn relations(
    catalog: &Catalog,
    settings: &CatalogSettings,
    now: u64,
    schema: &str,
    pattern: Option<&str>,
    offset: usize,
    limit: usize,
) -> Result<Value, Missing> {
    let name = find_schema(catalog, settings, schema)?;
    let node = catalog.schema(name).unwrap();
    let Some(relations) = &node.relations else {
        return Err(Missing::Relations(Scope::Schema(name.clone())));
    };
    let pattern = pattern.map(str::trim).filter(|pattern| !pattern.is_empty());
    let entries: Vec<(&String, &Relation)> = relations
        .iter()
        .filter(|(relation, _)| pattern.is_none_or(|pattern| glob_match(pattern, relation)))
        .map(|(relation, node)| (relation, node.as_ref()))
        .collect();
    Ok(bounded_page(&entries, offset, limit, |page, next| {
        json!({
            "schema": name,
            "pattern": pattern,
            "fetched_at": node.fetched_at,
            "stale": stale(node.fetched_at, settings, now),
            "error": node.error,
            "relation_count": entries.len(),
            "offset": offset,
            "next_offset": next,
            "relations": page.iter().map(|(relation, node)| json!({
                "name": relation,
                "kind": kind(node.kind),
                "comment": node.comment,
                "columns_loaded": node.columns.is_some(),
            })).collect::<Vec<_>>(),
        })
    }))
}

fn find_schema<'a>(
    catalog: &'a Catalog,
    settings: &CatalogSettings,
    schema: &str,
) -> Result<&'a String, Missing> {
    if catalog.fetched_at.is_none() && catalog.schemas.is_empty() {
        return Err(Missing::Schemas);
    }
    match schema_name(catalog, schema) {
        Some(name) => Ok(name),
        None if !settings.shows(schema) => Err(Missing::SchemaHidden),
        None => Err(Missing::SchemaNotFound),
    }
}

/// The columns and details of one relation.
pub fn relation(
    catalog: &Catalog,
    settings: &CatalogSettings,
    now: u64,
    schema: &str,
    relation: &str,
) -> Result<Value, Missing> {
    let schema = find_schema(catalog, settings, schema)?;
    let Some(relations) = &catalog.schema(schema).unwrap().relations else {
        return Err(Missing::Relations(Scope::Schema(schema.clone())));
    };
    let (name, node) = relations
        .get_key_value(relation)
        .or_else(|| {
            relations
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(relation))
        })
        .ok_or(Missing::RelationNotFound)?;
    let Some(columns) = &node.columns else {
        return Err(Missing::Columns(Scope::Relation(
            schema.clone(),
            name.clone(),
        )));
    };
    let mut count = columns.len();
    loop {
        let value = json!({
            "schema": schema,
            "relation": name,
            "kind": kind(node.kind),
            "comment": node.comment,
            "fetched_at": node.fetched_at,
            "stale": stale(node.fetched_at, settings, now),
            "error": node.error,
            "column_count": columns.len(),
            "columns_truncated": count < columns.len(),
            "columns": columns[..count].iter().map(|column| json!({
                "name": column.name,
                "data_type": column.data_type,
                "comment": column.comment,
            })).collect::<Vec<_>>(),
        });
        if fits(&value) || count == 0 {
            return Ok(value);
        }
        count /= 2;
    }
}

/// The catalog of the connection of a conversation, in the workspace context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CatalogContext {
    pub connection_id: uuid::Uuid,
    /// Whether the connection browses schemas. Without it, the catalog tools
    /// have no data.
    pub browsing: bool,
    /// The name of the shared catalog that the connection uses.
    pub shared_catalog: Option<String>,
    /// Whether Qrow has the cached catalog in memory.
    pub loaded: bool,
    pub fetched_at: Option<u64>,
    pub stale: bool,
    pub schema_count: usize,
    pub relation_count: usize,
    /// The cached columns of the relations that the tab SQL names.
    pub referenced_relations: Vec<ReferencedRelation>,
    pub referenced_relations_truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReferencedRelation {
    pub schema: String,
    pub relation: String,
    pub kind: &'static str,
    pub columns: Vec<ReferencedColumn>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReferencedColumn {
    pub name: String,
    pub data_type: String,
}

impl CatalogContext {
    /// The context of a connection. `catalog` is `None` while Qrow has not
    /// loaded the cache. `sql` is the SQL of the tab, and `default_schema`
    /// the initial database of the connection.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        connection_id: uuid::Uuid,
        browsing: bool,
        shared_catalog: Option<String>,
        catalog: Option<&Catalog>,
        settings: &CatalogSettings,
        sql: &str,
        default_schema: &str,
        now: u64,
    ) -> Self {
        let catalog = catalog.filter(|_| browsing);
        let (referenced_relations, referenced_relations_truncated) = catalog
            .map(|catalog| referenced_relations(catalog, sql, default_schema))
            .unwrap_or_default();
        Self {
            connection_id,
            browsing,
            shared_catalog,
            loaded: catalog.is_some(),
            fetched_at: catalog.and_then(|catalog| catalog.fetched_at),
            stale: stale(
                catalog.and_then(|catalog| catalog.fetched_at),
                settings,
                now,
            ),
            schema_count: catalog.map_or(0, |catalog| catalog.schemas.len()),
            relation_count: catalog.map_or(0, Catalog::relation_count),
            referenced_relations,
            referenced_relations_truncated,
        }
    }
}

/// The names in `sql` that can be relations: `a.b`, `a.b.c` (the last two
/// parts), and single names. Comments and string literals are skipped.
/// Backticks quote a name.
fn names(sql: &str) -> Vec<Vec<String>> {
    let chars: Vec<char> = sql.chars().collect();
    let mut names: Vec<Vec<String>> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    // Whether the last part of `current` ends with a dot.
    let mut dot = false;
    let mut i = 0;
    let finish = |current: &mut Vec<String>, names: &mut Vec<Vec<String>>| {
        if !current.is_empty() {
            names.push(std::mem::take(current));
        }
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '-' && chars.get(i + 1) == Some(&'-') {
            finish(&mut current, &mut names);
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            finish(&mut current, &mut names);
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i += 2;
            continue;
        }
        if c == '\'' || c == '"' {
            finish(&mut current, &mut names);
            i += 1;
            while i < chars.len() && chars[i] != c {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        let part = if c == '`' {
            let start = i + 1;
            let mut end = start;
            while end < chars.len() && chars[end] != '`' {
                end += 1;
            }
            i = end + 1;
            Some(
                chars[start..end.min(chars.len())]
                    .iter()
                    .collect::<String>(),
            )
        } else if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            Some(chars[start..i].iter().collect::<String>())
        } else {
            None
        };
        match part {
            Some(part) => {
                if !current.is_empty() && !dot {
                    finish(&mut current, &mut names);
                }
                current.push(part);
                dot = false;
            }
            None if c == '.' && !current.is_empty() && !dot => {
                dot = true;
                i += 1;
            }
            None => {
                finish(&mut current, &mut names);
                dot = false;
                if !c.is_alphanumeric() {
                    i += 1;
                } else {
                    // A number: skip it, so its digits are not a name.
                    while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '.') {
                        i += 1;
                    }
                }
            }
        }
    }
    finish(&mut current, &mut names);
    names
}

/// The cached relations that `sql` names, with their columns, in at most
/// `MAX_REFERENCED_BYTES`. Returns whether some did not fit. The match is
/// best-effort: a name that is also a column or an alias can match a table.
pub fn referenced_relations(
    catalog: &Catalog,
    sql: &str,
    default_schema: &str,
) -> (Vec<ReferencedRelation>, bool) {
    let mut found: Vec<ReferencedRelation> = Vec::new();
    let mut bytes = 0;
    let mut truncated = false;
    for name in names(sql) {
        let (schema, relation) = match name.as_slice() {
            [.., schema, relation] => (schema.as_str(), relation.as_str()),
            [relation] => (default_schema, relation.as_str()),
            [] => continue,
        };
        let Some(schema) = schema_name(catalog, schema) else {
            continue;
        };
        let Some(relations) = &catalog.schema(schema).unwrap().relations else {
            continue;
        };
        let Some((relation, node)) = relations.get_key_value(relation).or_else(|| {
            relations
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(relation))
        }) else {
            continue;
        };
        let Some(columns) = &node.columns else {
            continue;
        };
        if found
            .iter()
            .any(|known| known.schema == *schema && known.relation == *relation)
        {
            continue;
        }
        let entry = ReferencedRelation {
            schema: schema.clone(),
            relation: relation.clone(),
            kind: kind(node.kind),
            columns: columns
                .iter()
                .map(|column| ReferencedColumn {
                    name: column.name.clone(),
                    data_type: column.data_type.clone(),
                })
                .collect(),
        };
        let size = serde_json::to_vec(&entry).map_or(usize::MAX, |encoded| encoded.len());
        if bytes + size > MAX_REFERENCED_BYTES {
            truncated = true;
            continue;
        }
        bytes += size;
        found.push(entry);
    }
    (found, truncated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{CatalogColumn, RelationEntry};
    use std::collections::BTreeMap;

    const NOW: u64 = 100_000;

    fn settings() -> CatalogSettings {
        CatalogSettings {
            refresh_minutes: 60,
            ..CatalogSettings::default()
        }
    }

    fn catalog() -> Catalog {
        let mut catalog = Catalog::empty(uuid::Uuid::nil(), None);
        let at = NOW - 60;
        catalog.apply_schemas(
            vec!["sales".into(), "hr".into(), "empty".into()],
            &CatalogSettings::default(),
            at,
        );
        let table = |name: &str, kind| RelationEntry {
            name: name.into(),
            kind,
            comment: Some(format!("About {name}")),
        };
        catalog.apply_relations(
            "sales",
            None,
            vec![
                table("orders", RelationKind::Table),
                table("order_items", RelationKind::Table),
                table("daily", RelationKind::View),
            ],
            at,
        );
        catalog.apply_relations("empty", None, vec![], at);
        let column = |name: &str, data_type: &str| CatalogColumn {
            name: name.into(),
            data_type: data_type.into(),
            comment: None,
        };
        catalog.apply_columns(
            "sales",
            Some("orders"),
            BTreeMap::from([(
                "orders".into(),
                vec![column("id", "BIGINT"), column("total", "DECIMAL(10,2)")],
            )]),
            at,
        );
        catalog
    }

    #[test]
    fn data_is_stale_after_the_refresh_period_or_when_never_read() {
        assert!(!stale(Some(NOW - 3600), &settings(), NOW));
        assert!(stale(Some(NOW - 3601), &settings(), NOW));
        assert!(stale(None, &settings(), NOW));
    }

    #[test]
    fn schemas_page_through_the_list_with_counts() {
        let catalog = catalog();
        let all = schemas(&catalog, &settings(), NOW, 0, DEFAULT_PAGE).unwrap();
        assert_eq!(all["schema_count"], 3);
        assert_eq!(all["next_offset"], Value::Null);
        assert_eq!(all["stale"], false);
        let names: Vec<_> = all["schemas"]
            .as_array()
            .unwrap()
            .iter()
            .map(|schema| schema["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["empty", "hr", "sales"]);
        assert_eq!(all["schemas"][1]["relation_count"], Value::Null);
        assert_eq!(all["schemas"][2]["relation_count"], 3);

        let page = schemas(&catalog, &settings(), NOW, 1, 1).unwrap();
        assert_eq!(page["schemas"][0]["name"], "hr");
        assert_eq!(page["next_offset"], 2);

        let never = Catalog::empty(uuid::Uuid::nil(), None);
        assert_eq!(
            schemas(&never, &settings(), NOW, 0, 10),
            Err(Missing::Schemas)
        );
    }

    #[test]
    fn relations_match_a_pattern_and_tell_what_is_missing() {
        let catalog = catalog();
        let matched =
            relations(&catalog, &settings(), NOW, "Sales", Some("ORDER*"), 0, 10).unwrap();
        assert_eq!(matched["schema"], "sales");
        assert_eq!(matched["relation_count"], 2);
        assert_eq!(matched["relations"][0]["name"], "order_items");
        assert_eq!(matched["relations"][0]["columns_loaded"], false);
        assert_eq!(matched["relations"][1]["columns_loaded"], true);
        assert_eq!(matched["relations"][1]["kind"], "table");

        assert_eq!(
            relations(&catalog, &settings(), NOW, "hr", None, 0, 10),
            Err(Missing::Relations(Scope::Schema("hr".into())))
        );
        assert_eq!(
            relations(&catalog, &settings(), NOW, "finance", None, 0, 10),
            Err(Missing::SchemaNotFound)
        );
        let hiding = CatalogSettings {
            exclude: vec!["fin*".into()],
            ..settings()
        };
        assert_eq!(
            relations(&catalog, &hiding, NOW, "finance", None, 0, 10),
            Err(Missing::SchemaHidden)
        );
    }

    #[test]
    fn a_relation_has_its_columns_or_the_refresh_that_reads_them() {
        let catalog = catalog();
        let orders = relation(&catalog, &settings(), NOW, "sales", "ORDERS").unwrap();
        assert_eq!(orders["relation"], "orders");
        assert_eq!(orders["columns"][1]["data_type"], "DECIMAL(10,2)");
        assert_eq!(orders["columns_truncated"], false);
        assert_eq!(orders["comment"], "About orders");
        assert_eq!(
            relation(&catalog, &settings(), NOW, "sales", "daily"),
            Err(Missing::Columns(Scope::Relation(
                "sales".into(),
                "daily".into()
            )))
        );
        assert_eq!(
            relation(&catalog, &settings(), NOW, "sales", "nothing"),
            Err(Missing::RelationNotFound)
        );
        assert_eq!(Missing::Schemas.scope(), Some(Scope::Connection));
        assert!(Missing::RelationNotFound.scope().is_none());
    }

    #[test]
    fn large_views_stay_inside_the_tool_output_limit() {
        let mut catalog = catalog();
        let entries: Vec<RelationEntry> = (0..MAX_PAGE)
            .map(|index| RelationEntry {
                name: format!("relation_{index:04}"),
                kind: RelationKind::Table,
                comment: Some("x".repeat(200)),
            })
            .collect();
        catalog.apply_relations("hr", None, entries, NOW);
        let page = relations(&catalog, &settings(), NOW, "hr", None, 0, MAX_PAGE).unwrap();
        assert!(fits(&page));
        let listed = page["relations"].as_array().unwrap().len();
        assert!(listed < MAX_PAGE);
        assert_eq!(page["next_offset"], listed);

        let wide: Vec<CatalogColumn> = (0..2_000)
            .map(|index| CatalogColumn {
                name: format!("column_{index}"),
                data_type: "STRING".into(),
                comment: Some("y".repeat(100)),
            })
            .collect();
        catalog.apply_columns(
            "hr",
            Some("relation_0000"),
            BTreeMap::from([("relation_0000".into(), wide)]),
            NOW,
        );
        let described = relation(&catalog, &settings(), NOW, "hr", "relation_0000").unwrap();
        assert!(fits(&described));
        assert_eq!(described["columns_truncated"], true);
        assert_eq!(described["column_count"], 2_000);
    }

    #[test]
    fn the_context_names_the_cached_relations_of_the_tab_sql() {
        let catalog = catalog();
        let sql = "-- orders from sales.daily\nSELECT o.id, 'sales.order_items' AS note\nFROM `sales`.Orders o JOIN daily d ON 1 = 1.5 /* hr.x */";
        let (found, truncated) = referenced_relations(&catalog, sql, "sales");
        assert!(!truncated);
        // `daily` has no cached columns, and comments and strings do not count.
        let names: Vec<_> = found.iter().map(|r| r.relation.as_str()).collect();
        assert_eq!(names, ["orders"]);
        assert_eq!(found[0].columns[0].name, "id");

        let context = CatalogContext::new(
            uuid::Uuid::nil(),
            true,
            Some("Lake".into()),
            Some(&catalog),
            &settings(),
            sql,
            "sales",
            NOW,
        );
        assert!(context.loaded);
        assert_eq!(context.schema_count, 3);
        assert_eq!(context.relation_count, 3);
        assert!(!context.stale);
        let off = CatalogContext::new(
            uuid::Uuid::nil(),
            false,
            None,
            Some(&catalog),
            &settings(),
            sql,
            "sales",
            NOW,
        );
        assert!(!off.loaded);
        assert!(off.referenced_relations.is_empty());
    }

    #[test]
    fn names_follow_qualified_and_quoted_identifiers() {
        assert_eq!(
            names("SELECT a.b, `c d`.e FROM x.y.z WHERE 1.5 > 2"),
            vec![
                vec!["SELECT".to_owned()],
                vec!["a".into(), "b".into()],
                vec!["c d".into(), "e".into()],
                vec!["FROM".into()],
                vec!["x".into(), "y".into(), "z".into()],
                vec!["WHERE".into()],
            ]
        );
    }
}
