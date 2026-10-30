//! Completion over the cursor statement and an immutable catalog name index.
//! This module performs no I/O and has no access to sessions or credentials.

mod context;
#[cfg(test)]
mod tests;

use super::KeywordCase;
use crate::{
    catalog::{Catalog, Relation, Scope},
    model::{CatalogSettings, DatabaseType},
};
use context::{Context, Name};
use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
    sync::Arc,
};

const MAX_ITEMS: usize = 100;
pub(super) const KEYWORDS: &[&str] = &[
    "SELECT",
    "FROM",
    "WHERE",
    "JOIN",
    "LEFT",
    "RIGHT",
    "INNER",
    "FULL",
    "ON",
    "USING",
    "GROUP",
    "BY",
    "ORDER",
    "HAVING",
    "LIMIT",
    "WITH",
    "UNION",
    "ALL",
    "DISTINCT",
    "AND",
    "OR",
    "NOT",
    "NULL",
    "IS",
    "IN",
    "EXISTS",
    "BETWEEN",
    "LIKE",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "AS",
    "INSERT",
    "INTO",
    "UPDATE",
    "DELETE",
    "VALUES",
    "CREATE",
    "TABLE",
    "VIEW",
    "OVER",
    "PARTITION",
    "ROWS",
    "RANGE",
    "UNBOUNDED",
    "PRECEDING",
    "FOLLOWING",
    "CURRENT",
    "ROW",
    "TRUE",
    "FALSE",
    "EXCEPT",
    "INTERSECT",
    "ASC",
    "DESC",
    "NULLS",
    "FIRST",
    "LAST",
    "OFFSET",
    "FETCH",
    "WINDOW",
    "QUALIFY",
];
const COMMON_FUNCTIONS: &[&str] = &[
    "abs",
    "avg",
    "cast",
    "ceil",
    "coalesce",
    "concat",
    "count",
    "date_trunc",
    "floor",
    "greatest",
    "least",
    "length",
    "lower",
    "max",
    "min",
    "nullif",
    "round",
    "row_number",
    "substr",
    "substring",
    "sum",
    "trim",
    "upper",
];
const TRINO_FUNCTIONS: &[&str] = &[
    "approx_distinct",
    "array_agg",
    "array_join",
    "cardinality",
    "count_if",
    "date_add",
    "date_diff",
    "if",
    "json_extract",
    "json_extract_scalar",
    "map_agg",
    "regexp_like",
    "try",
    "try_cast",
    "unnest",
];
const POSTGRES_FUNCTIONS: &[&str] = &[
    "array_agg",
    "current_setting",
    "date_part",
    "generate_series",
    "json_agg",
    "jsonb_agg",
    "jsonb_build_object",
    "now",
    "regexp_match",
    "string_agg",
    "to_char",
    "to_date",
    "unnest",
];

struct IndexedColumn {
    position: usize,
    folded: String,
}

struct IndexedRelation {
    schema: String,
    name: String,
    folded: String,
    relation: Arc<Relation>,
    columns: Vec<IndexedColumn>,
}

/// Names derived from one catalog snapshot. Unchanged relation entries are
/// shared with the preceding index, including their folded column names.
#[derive(Default)]
pub struct CatalogIndex {
    catalog: Option<Arc<Catalog>>,
    relations: BTreeMap<(String, String), Arc<IndexedRelation>>,
    folded_relations: BTreeMap<(String, String), Arc<IndexedRelation>>,
}

impl CatalogIndex {
    pub fn new(catalog: Option<Arc<Catalog>>, previous: Option<&Self>) -> Self {
        let mut relations = BTreeMap::new();
        if let Some(catalog) = &catalog {
            for (schema, data) in &catalog.schemas {
                for (name, relation) in data.relations.iter().flatten() {
                    let key = (schema.clone(), name.clone());
                    let entry = previous
                        .and_then(|old| old.relations.get(&key))
                        .filter(|old| Arc::ptr_eq(&old.relation, relation))
                        .cloned()
                        .unwrap_or_else(|| {
                            Arc::new(IndexedRelation {
                                schema: schema.clone(),
                                name: name.clone(),
                                folded: name.to_lowercase(),
                                relation: relation.clone(),
                                columns: relation
                                    .columns
                                    .iter()
                                    .flatten()
                                    .enumerate()
                                    .map(|(position, column)| IndexedColumn {
                                        position,
                                        folded: column.name.to_lowercase(),
                                    })
                                    .collect(),
                            })
                        });
                    relations.insert(key, entry);
                }
            }
        }
        let mut folded_relations = BTreeMap::new();
        for entry in relations.values() {
            folded_relations
                .entry((entry.schema.to_lowercase(), entry.folded.clone()))
                .or_insert_with(|| entry.clone());
        }
        Self {
            catalog,
            relations,
            folded_relations,
        }
    }

    pub fn matches_snapshot(&self, catalog: Option<&Arc<Catalog>>) -> bool {
        match (self.catalog.as_ref(), catalog) {
            (None, None) => true,
            (Some(old), Some(new)) => {
                Arc::ptr_eq(old, new)
                    || old.schemas.len() == new.schemas.len()
                        && old
                            .schemas
                            .iter()
                            .zip(&new.schemas)
                            .all(|((a, old), (b, new))| a == b && Arc::ptr_eq(old, new))
            }
            _ => false,
        }
    }
}

/// The connection choices that give unqualified SQL names their context.
#[derive(Clone, PartialEq)]
pub struct CompletionConfig {
    dialect: DatabaseType,
    schema: String,
    catalog: String,
    filters: CatalogSettings,
    keyword_case: KeywordCase,
}

impl CompletionConfig {
    pub fn new(
        dialect: DatabaseType,
        schema: impl Into<String>,
        catalog: impl Into<String>,
        filters: CatalogSettings,
        keyword_case: KeywordCase,
    ) -> Self {
        Self {
            dialect,
            schema: schema.into(),
            catalog: catalog.into(),
            filters,
            keyword_case,
        }
    }

    fn keyword(&self, name: &str) -> String {
        match self.keyword_case {
            KeywordCase::Uppercase => name.to_uppercase(),
            KeywordCase::Lowercase => name.to_lowercase(),
        }
    }

    fn identifier(&self, name: &str, _quoted: bool) -> String {
        // The suggestion keyword list is deliberately small, not a dialect's
        // reserved-word registry. Quoting cached names also preserves their case.
        self.dialect.quote_identifier(name)
    }

    fn matches(&self, name: &Name, actual: &str) -> bool {
        if name.quoted && self.dialect == DatabaseType::Postgres {
            name.text == actual
        } else if self.dialect == DatabaseType::Postgres {
            name.text.to_lowercase() == actual
        } else {
            name.text.to_lowercase() == actual.to_lowercase()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Schema,
    Relation,
    Column,
    Keyword,
    Function,
}

/// A complete replacement edit in UTF-8 byte offsets, with plain-text details.
#[derive(Clone, Debug)]
pub struct Candidate {
    label: String,
    detail: Option<String>,
    documentation: String,
    insert: String,
    replacement: Range<usize>,
    kind: CompletionKind,
    rank: (usize, usize, usize),
}

impl Candidate {
    pub fn label(&self) -> &str {
        &self.label
    }
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
    pub fn documentation(&self) -> &str {
        &self.documentation
    }
    pub fn insert(&self) -> &str {
        &self.insert
    }
    pub fn replacement(&self) -> Range<usize> {
        self.replacement.clone()
    }
    pub fn kind(&self) -> CompletionKind {
        self.kind
    }
}

/// A cache limitation. Refresh is always an explicit application action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogNote {
    text: String,
    refresh: Option<Scope>,
    missing_relation: bool,
}

impl CatalogNote {
    pub fn is_missing_relation(&self) -> bool {
        self.missing_relation
    }
    pub fn not_found_text(&self) -> Option<String> {
        match &self.refresh {
            Some(Scope::Relation(schema, name)) if self.missing_relation => {
                Some(format!("{schema}.{name} was not found."))
            }
            _ => None,
        }
    }
    pub fn text(&self) -> &str {
        &self.text
    }
    pub fn refresh(&self) -> Option<&Scope> {
        self.refresh.as_ref()
    }
}

#[derive(Default)]
pub struct CompletionBatch {
    items: Vec<Candidate>,
    notes: Vec<CatalogNote>,
    prefix: String,
    replacement: Range<usize>,
}

impl CompletionBatch {
    pub fn items(&self) -> &[Candidate] {
        &self.items
    }
    pub fn notes(&self) -> &[CatalogNote] {
        &self.notes
    }
    pub fn prefix(&self) -> &str {
        &self.prefix
    }
    pub fn replacement(&self) -> Range<usize> {
        self.replacement.clone()
    }
}

struct Builder<'a> {
    prefix: String,
    context: Context,
    index: &'a CatalogIndex,
    config: &'a CompletionConfig,
    batch: CompletionBatch,
}

pub fn complete(
    sql: &str,
    cursor: usize,
    index: &CatalogIndex,
    config: &CompletionConfig,
) -> CompletionBatch {
    let Some(context) = context::scan(sql, cursor, config.dialect) else {
        return CompletionBatch::default();
    };
    let mut builder = Builder {
        prefix: context.prefix.to_lowercase(),
        batch: CompletionBatch {
            prefix: context.prefix.clone(),
            replacement: context.replacement.clone(),
            ..CompletionBatch::default()
        },
        context,
        index,
        config,
    };
    builder.collect();
    builder.batch.items.sort_by(|a, b| {
        a.rank
            .cmp(&b.rank)
            .then_with(|| a.label.cmp(&b.label))
            .then_with(|| a.documentation.cmp(&b.documentation))
    });
    builder
        .batch
        .items
        .dedup_by(|a, b| a.insert == b.insert && a.documentation == b.documentation);
    builder.batch.items.truncate(MAX_ITEMS);
    builder.batch
}

fn match_rank(prefix: &str, folded: &str) -> Option<(usize, usize)> {
    if folded.starts_with(prefix) {
        return Some((0, folded.len().saturating_sub(prefix.len())));
    }
    let mut chars = folded.chars().enumerate();
    let mut score = 0;
    for wanted in prefix.chars() {
        let (position, _) = chars.find(|(_, actual)| *actual == wanted)?;
        score += position;
    }
    Some((1, score))
}

impl<'a> Builder<'a> {
    fn add(
        &mut self,
        label: &str,
        match_name: &str,
        detail: Option<&str>,
        documentation: String,
        insert: String,
        kind: CompletionKind,
    ) {
        let priority = match kind {
            CompletionKind::Column => 0,
            CompletionKind::Relation => usize::from(!detail.is_some_and(|detail| {
                detail == "CTE" || detail.eq_ignore_ascii_case(&self.config.schema)
            })),
            CompletionKind::Keyword => 2,
            _ => 1,
        };
        let Some((tier, score)) = match_rank(&self.prefix, match_name) else {
            return;
        };
        self.batch.items.push(Candidate {
            label: label.into(),
            detail: detail.map(str::to_string),
            documentation,
            insert,
            replacement: self.context.replacement.clone(),
            kind,
            rank: (priority, tier, score),
        });
    }

    fn note(&mut self, text: String, refresh: Option<Scope>) {
        let note = CatalogNote {
            text,
            refresh,
            missing_relation: false,
        };
        if !self.batch.notes.contains(&note) {
            self.batch.notes.push(note);
        }
    }

    fn schema(&mut self, name: &Name) -> Option<String> {
        if !self.config.filters.browses() {
            self.note("The catalog is disabled for this connection.".into(), None);
            return None;
        }
        if !self.config.filters.shows(&name.text) {
            self.note(
                format!("{} is hidden by the catalog filters.", name.text),
                None,
            );
            return None;
        }
        let schema = self
            .index
            .catalog
            .as_ref()
            .and_then(|catalog| {
                catalog
                    .schemas
                    .keys()
                    .find(|actual| self.config.matches(name, actual))
            })
            .cloned();
        if schema.is_none() {
            self.note(format!("{} is not in the catalog.", name.text), None);
        }
        schema
    }

    fn collect(&mut self) {
        let qualifiers = self.context.qualifiers.clone();
        if !qualifiers.is_empty() {
            if !self.context.relation_position
                && qualifiers.len() == 1
                && let Some(relation) = self
                    .context
                    .relations
                    .iter()
                    .find(|relation| self.config.matches(&qualifiers[0], &relation.alias))
                    .cloned()
            {
                self.columns(&relation, true, &HashMap::new());
                return;
            }
            let schema_name = if qualifiers.len() == 1 {
                &qualifiers[0]
            } else if qualifiers.len() == 2
                && self.config.dialect == DatabaseType::Trino
                && qualifiers[0]
                    .text
                    .eq_ignore_ascii_case(&self.config.catalog)
            {
                &qualifiers[1]
            } else {
                // A two-part relation qualifier can refer to a cached table.
                let relation = context::Relation {
                    alias: String::new(),
                    path: qualifiers.clone(),
                    columns: None,
                };
                self.columns(&relation, true, &HashMap::new());
                return;
            };
            if let Some(schema) = self.schema(schema_name) {
                self.tables(Some(&schema));
            }
            return;
        }
        if self.context.relation_position {
            self.tables(None);
            for cte in self.context.ctes.clone() {
                self.add(
                    &cte.name,
                    &cte.name.to_lowercase(),
                    Some("CTE"),
                    format!("{}\n\nCommon table expression.", cte.name),
                    self.config.identifier(&cte.name, self.context.quoted),
                    CompletionKind::Relation,
                );
            }
        } else {
            let mut counts = HashMap::new();
            for relation in &self.context.relations {
                if let Some(columns) = &relation.columns {
                    for name in columns {
                        *counts.entry(name.to_lowercase()).or_insert(0) += 1;
                    }
                } else if let Some(entry) = self.lookup(&relation.path) {
                    for column in &entry.columns {
                        *counts.entry(column.folded.clone()).or_insert(0) += 1;
                    }
                }
            }
            for relation in self.context.relations.clone() {
                self.columns(&relation, false, &counts);
            }
            let extra = match self.config.dialect {
                DatabaseType::Kyuubi => super::FUNCTIONS,
                DatabaseType::Postgres => POSTGRES_FUNCTIONS,
                DatabaseType::Trino => TRINO_FUNCTIONS,
            };
            for function in COMMON_FUNCTIONS.iter().chain(extra) {
                let label = self.config.keyword(function);
                self.add(
                    &label,
                    function,
                    Some("function"),
                    format!("{label}\n\nBuilt-in function."),
                    label.clone(),
                    CompletionKind::Function,
                );
            }
        }
        for keyword in KEYWORDS {
            let label = self.config.keyword(keyword);
            self.add(
                &label,
                &keyword.to_lowercase(),
                Some("keyword"),
                format!("{label}\n\nSQL keyword."),
                label.clone(),
                CompletionKind::Keyword,
            );
        }
    }

    fn tables(&mut self, schema: Option<&str>) {
        if !self.config.filters.browses() {
            self.note("The catalog is disabled for this connection.".into(), None);
            return;
        }
        if let Some(schema) = schema {
            self.schema_note(schema);
        } else if self
            .index
            .catalog
            .as_ref()
            .is_none_or(|catalog| catalog.schemas.is_empty())
        {
            self.note(
                "No schema data is cached. Refresh the catalog in Connections.".into(),
                None,
            );
        }
        let visible = self
            .index
            .catalog
            .as_ref()
            .map(|catalog| {
                catalog
                    .schemas
                    .keys()
                    .filter(|name| self.config.filters.shows(name))
                    .map(String::as_str)
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();
        for entry in self.index.relations.values() {
            if !visible.contains(entry.schema.as_str())
                || schema.is_some_and(|schema| schema != entry.schema)
            {
                continue;
            }
            if match_rank(&self.prefix, &entry.folded).is_none() {
                continue;
            }
            let insert =
                if schema.is_some() || entry.schema.eq_ignore_ascii_case(&self.config.schema) {
                    self.config.identifier(&entry.name, self.context.quoted)
                } else {
                    format!(
                        "{}.{}",
                        self.config.identifier(&entry.schema, false),
                        self.config.identifier(&entry.name, self.context.quoted)
                    )
                };
            self.add(
                &entry.name,
                &entry.folded,
                Some(&entry.schema),
                documentation(
                    &format!("{}.{}", entry.schema, entry.name),
                    entry.relation.comment.as_deref(),
                ),
                insert,
                CompletionKind::Relation,
            );
        }
        if schema.is_none()
            && let Some(catalog) = &self.index.catalog
        {
            for name in catalog.schemas.keys() {
                if !self.config.filters.shows(name) {
                    continue;
                }
                self.add(
                    name,
                    &name.to_lowercase(),
                    Some("schema"),
                    format!("{name}\n\nDatabase schema."),
                    self.config.identifier(name, self.context.quoted),
                    CompletionKind::Schema,
                );
            }
        }
    }

    fn schema_note(&mut self, schema: &str) -> bool {
        let Some(data) = self
            .index
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.schemas.get(schema))
        else {
            return false;
        };
        if let Some(error) = &data.error {
            self.note(
                format!("Tables of {schema} could not be loaded. {error}"),
                Some(Scope::Schema(schema.into())),
            );
            true
        } else if data.relations.is_none() {
            self.note(
                format!("Tables of {schema} are not loaded."),
                Some(Scope::Schema(schema.into())),
            );
            true
        } else {
            false
        }
    }

    fn lookup(&self, path: &[Name]) -> Option<&'a IndexedRelation> {
        let relation = path.last()?;
        let schema = path.get(path.len().wrapping_sub(2));
        if path.len() > 2
            && (self.config.dialect != DatabaseType::Trino
                || !path[0].text.eq_ignore_ascii_case(&self.config.catalog))
        {
            return None;
        }
        if self.config.dialect != DatabaseType::Postgres {
            let schema = schema.map_or(self.config.schema.as_str(), |name| name.text.as_str());
            return self
                .index
                .folded_relations
                .get(&(schema.to_lowercase(), relation.text.to_lowercase()))
                .map(Arc::as_ref);
        }
        let name = if relation.quoted {
            relation.text.clone()
        } else {
            relation.text.to_lowercase()
        };
        let schema = match schema {
            Some(schema) if schema.quoted => schema.text.clone(),
            Some(schema) => schema.text.to_lowercase(),
            None => self
                .index
                .catalog
                .as_ref()?
                .schemas
                .keys()
                .find(|schema| {
                    schema.eq_ignore_ascii_case(&self.config.schema)
                        && self
                            .index
                            .relations
                            .contains_key(&(schema.to_string(), name.clone()))
                })?
                .clone(),
        };
        self.index.relations.get(&(schema, name)).map(Arc::as_ref)
    }

    fn columns(
        &mut self,
        reference: &context::Relation,
        qualified: bool,
        counts: &HashMap<String, usize>,
    ) {
        if let Some(columns) = &reference.columns {
            for name in columns {
                let ambiguous = !qualified
                    && counts
                        .get(&name.to_lowercase())
                        .is_some_and(|count| *count > 1);
                let label = if ambiguous {
                    format!("{}.{}", reference.alias, name)
                } else {
                    name.clone()
                };
                let insert = if ambiguous {
                    format!(
                        "{}.{}",
                        self.config.identifier(&reference.alias, false),
                        self.config.identifier(name, self.context.quoted)
                    )
                } else {
                    self.config.identifier(name, self.context.quoted)
                };
                self.add(
                    &label,
                    &name.to_lowercase(),
                    None,
                    format!(
                        "{}.{}\n\nCommon table expression or subquery column.",
                        reference.alias, name
                    ),
                    insert,
                    CompletionKind::Column,
                );
            }
            return;
        }
        if reference.path.is_empty() {
            return;
        }
        if reference.path.len() > 2
            && !reference.path[0]
                .text
                .eq_ignore_ascii_case(&self.config.catalog)
        {
            self.note(
                format!(
                    "{} is not in the catalog.",
                    reference
                        .path
                        .iter()
                        .map(|name| name.text.as_str())
                        .collect::<Vec<_>>()
                        .join(".")
                ),
                None,
            );
            return;
        }
        let schema_name = reference
            .path
            .get(reference.path.len().wrapping_sub(2))
            .cloned()
            .unwrap_or(Name {
                text: self.config.schema.clone(),
                quoted: false,
            });
        if schema_name.text.is_empty() {
            self.note(
                "Choose a schema or use a qualified table name.".into(),
                None,
            );
            return;
        }
        let Some(schema) = self.schema(&schema_name) else {
            return;
        };
        let part = reference.path.last().unwrap();
        let name = if self.config.dialect == DatabaseType::Postgres && !part.quoted {
            part.text.to_lowercase()
        } else {
            part.text.clone()
        };
        let qualified_name = format!("{schema}.{name}");
        let entry = self.lookup(&reference.path);
        let Some(entry) = entry else {
            if !self.schema_note(&schema) {
                let note = CatalogNote {
                    text: format!("{qualified_name} is not in the catalog."),
                    refresh: Some(Scope::Relation(schema, name)),
                    missing_relation: true,
                };
                if !self.batch.notes.contains(&note) {
                    self.batch.notes.push(note);
                }
            }
            return;
        };
        if let Some(error) = &entry.relation.error {
            self.note(
                format!("Columns of {qualified_name} could not be loaded. {error}"),
                Some(Scope::Relation(schema.clone(), name.clone())),
            );
            if entry.relation.columns.is_none() {
                return;
            }
        }
        if entry.relation.columns.is_none() {
            self.note(
                format!("Columns of {qualified_name} are not loaded."),
                Some(Scope::Relation(schema, name)),
            );
            return;
        }
        for column in &entry.columns {
            if match_rank(&self.prefix, &column.folded).is_none() {
                continue;
            }
            let data = &entry.relation.columns.as_ref().unwrap()[column.position];
            let name = &data.name;
            let ambiguous =
                !qualified && counts.get(&column.folded).is_some_and(|count| *count > 1);
            let label = if ambiguous {
                format!("{}.{}", reference.alias, name)
            } else {
                name.clone()
            };
            let insert = if ambiguous {
                format!(
                    "{}.{}",
                    self.config.identifier(&reference.alias, false),
                    self.config.identifier(name, self.context.quoted)
                )
            } else {
                self.config.identifier(name, self.context.quoted)
            };
            self.add(
                &label,
                &column.folded,
                Some(&data.data_type),
                documentation(&format!("{qualified_name}.{name}"), data.comment.as_deref()),
                insert,
                CompletionKind::Column,
            );
        }
    }
}

fn documentation(name: &str, comment: Option<&str>) -> String {
    format!(
        "{name}\n\n{}",
        comment
            .filter(|comment| !comment.trim().is_empty())
            .unwrap_or("No description")
    )
}
