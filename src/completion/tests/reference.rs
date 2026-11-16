//! The first implementation of [`super::super::complete`]: it builds every
//! suggestion, then sorts all of them. The tests compare the faster
//! implementation with it.

use super::super::*;
use crate::catalog::Schema;

/// The suggestions for `request` from `catalog`. `default_schema` is the
/// schema of names without a schema.
pub(super) fn complete(
    catalog: Option<&Catalog>,
    request: &Request,
    default_schema: &str,
    settings: &CatalogSettings,
    now: u64,
    database_type: DatabaseType,
) -> Completion {
    let mut completion = Completion::default();
    let Some(catalog) = catalog.filter(|catalog| catalog.fetched_at.is_some()) else {
        completion.need(Need::Schemas, catalog.and_then(|c| c.error.clone()));
        return completion;
    };
    let mut builder = Builder {
        catalog,
        request,
        settings,
        now,
        database_type,
        candidates: Vec::new(),
        completion,
    };
    match &request.place {
        Place::Relation => {
            for name in catalog.schemas.keys() {
                builder.schema_item(name);
            }
            builder.relations(default_schema);
        }
        Place::Qualified { parts, relation } => builder.qualified(parts, *relation, default_schema),
        Place::Column => builder.statement_columns(default_schema),
    }
    builder.finish()
}
struct Builder<'a> {
    catalog: &'a Catalog,
    request: &'a Request,
    settings: &'a CatalogSettings,
    now: u64,
    database_type: DatabaseType,
    /// Each match with its rank: the match tier, then the group, then the
    /// order in the group.
    candidates: Vec<((u8, u8, usize), Item)>,
    completion: Completion,
}

impl<'a> Builder<'a> {
    fn schema(&self, name: &str) -> Option<(&'a String, &'a Schema)> {
        self.catalog
            .schemas
            .get_key_value(name)
            .or_else(|| {
                self.catalog
                    .schemas
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
            })
            .map(|(key, schema)| (key, schema.as_ref()))
    }

    fn relation(
        &mut self,
        schema: &str,
        name: &str,
    ) -> Option<(&'a String, &'a String, &'a Relation)> {
        let (schema_name, node) = self.schema(schema)?;
        let Some(relations) = &node.relations else {
            self.completion
                .need(Need::Relations(schema_name.clone()), node.error.clone());
            return None;
        };
        relations
            .get_key_value(name)
            .or_else(|| {
                relations
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
            })
            .map(|(key, relation)| (schema_name, key, relation.as_ref()))
    }

    fn mark_stale(&mut self, schema: &str, fetched_at: Option<u64>) {
        if crate::assistant::catalog::stale(fetched_at, self.settings, self.now)
            && !self.completion.stale.iter().any(|known| known == schema)
        {
            self.completion.stale.push(schema.to_owned());
        }
    }

    fn push(&mut self, group: u8, order: usize, item: Item) {
        let Some((tier, matches)) = fuzzy(&item.name, &self.request.prefix) else {
            return;
        };
        self.candidates
            .push(((tier, group, order), Item { matches, ..item }));
    }

    fn schema_item(&mut self, name: &str) {
        let order = self.candidates.len();
        let node = self.schema(name).map(|(_, node)| node);
        let empty = node
            .and_then(|node| node.relations.as_ref())
            .is_some_and(|relations| relations.is_empty());
        // "Empty" can be old: a stale empty schema gets the stale footer.
        if empty && let Some(node) = node {
            self.mark_stale(name, node.fetched_at);
        }
        self.push(
            0,
            order,
            Item {
                kind: ItemKind::Schema,
                name: name.to_owned(),
                qualifier: None,
                insert: sql_name(name, self.database_type),
                matches: Vec::new(),
                source: None,
                detail: if empty { "empty schema" } else { "schema" }.into(),
                relation: None,
                comment: None,
                empty,
            },
        );
    }

    /// The tables and views of `schema`.
    fn relations(&mut self, schema: &str) {
        let Some((schema_name, node)) = self.schema(schema) else {
            return;
        };
        let Some(relations) = &node.relations else {
            self.completion
                .need(Need::Relations(schema_name.clone()), node.error.clone());
            return;
        };
        self.mark_stale(schema_name, node.fetched_at);
        for (order, (name, relation)) in relations.iter().enumerate() {
            let (kind, detail) = match relation.kind {
                RelationKind::Table => (ItemKind::Table, "table"),
                RelationKind::View => (ItemKind::View, "view"),
            };
            self.push(
                1,
                order,
                Item {
                    kind,
                    name: name.clone(),
                    qualifier: None,
                    insert: sql_name(name, self.database_type),
                    matches: Vec::new(),
                    source: None,
                    detail: detail.into(),
                    relation: Some((schema_name.clone(), name.clone())),
                    comment: relation.comment.clone(),
                    empty: false,
                },
            );
        }
    }

    fn qualified(&mut self, parts: &[String], relation_place: bool, default_schema: &str) {
        let last = parts.last().map(String::as_str).unwrap_or_default();
        if !relation_place && parts.len() == 1 {
            let source = self
                .request
                .sources
                .iter()
                .find(|source| source.qualifier().eq_ignore_ascii_case(last))
                .cloned();
            if let Some(source) = source {
                let schema = source.schema.as_deref().unwrap_or(default_schema);
                self.columns(schema, &source.name, None);
                return;
            }
        }
        if parts.len() >= 2 {
            let schema = &parts[parts.len() - 2];
            if self.schema(schema).is_some()
                && let Some((schema, name, _)) = self.relation(schema, last)
            {
                let (schema, name) = (schema.clone(), name.clone());
                self.columns(&schema, &name, None);
                return;
            }
        }
        if let Some((schema_name, node)) = self.schema(last) {
            if node
                .relations
                .as_ref()
                .is_some_and(|relations| relations.is_empty())
            {
                self.completion.empty_schema = Some(schema_name.clone());
            }
            self.relations(last);
        } else if !relation_place
            && parts.len() == 1
            && self.relation(default_schema, last).is_some()
        {
            self.columns(default_schema, last, None);
        }
    }

    /// The columns of the relations of the statement.
    fn statement_columns(&mut self, default_schema: &str) {
        let sources = self.request.sources.clone();
        let mut names: Vec<(Source, Vec<String>)> = Vec::new();
        for source in &sources {
            let schema = source.schema.as_deref().unwrap_or(default_schema);
            let Some((_, _, relation)) = self.relation(schema, &source.name) else {
                continue;
            };
            let columns = relation
                .columns
                .as_ref()
                .map(|columns| columns.iter().map(|column| column.name.clone()).collect())
                .unwrap_or_default();
            names.push((source.clone(), columns));
        }
        let several = names.len() > 1;
        for (index, (source, _)) in names.iter().enumerate() {
            let schema = source.schema.as_deref().unwrap_or(default_schema);
            let shared = |column: &str| {
                names.iter().enumerate().any(|(other, (_, columns))| {
                    other != index && columns.iter().any(|name| name.eq_ignore_ascii_case(column))
                })
            };
            let qualifier = source.qualifier().to_owned();
            self.columns_with(schema, &source.name, |column| {
                if shared(column) {
                    Some(RefQualify::Prefix(qualifier.clone()))
                } else if several {
                    Some(RefQualify::Tag(qualifier.clone()))
                } else {
                    None
                }
            });
        }
    }

    fn columns(&mut self, schema: &str, relation: &str, qualifier: Option<RefQualify>) {
        self.columns_with(schema, relation, |_| qualifier.clone());
    }

    fn columns_with(
        &mut self,
        schema: &str,
        relation: &str,
        qualify: impl Fn(&str) -> Option<RefQualify>,
    ) {
        let Some((schema_name, name, node)) = self.relation(schema, relation) else {
            return;
        };
        let Some(columns) = &node.columns else {
            self.completion.need(
                Need::Columns(schema_name.clone(), name.clone()),
                node.error.clone(),
            );
            return;
        };
        let schema_fetched = self
            .schema(schema_name)
            .and_then(|(_, node)| node.fetched_at);
        self.mark_stale(schema_name, schema_fetched.min(node.fetched_at));
        for (order, column) in columns.iter().enumerate() {
            let (qualifier, source, insert) = match qualify(&column.name) {
                Some(RefQualify::Prefix(qualifier)) => {
                    let insert = format!(
                        "{}.{}",
                        sql_name(&qualifier, self.database_type),
                        sql_name(&column.name, self.database_type)
                    );
                    (Some(qualifier), None, insert)
                }
                Some(RefQualify::Tag(tag)) => {
                    (None, Some(tag), sql_name(&column.name, self.database_type))
                }
                None => (None, None, sql_name(&column.name, self.database_type)),
            };
            self.push(
                2,
                order,
                Item {
                    kind: ItemKind::Column,
                    name: column.name.clone(),
                    qualifier,
                    insert,
                    matches: Vec::new(),
                    source,
                    detail: column.data_type.to_lowercase(),
                    relation: Some((schema_name.clone(), name.clone())),
                    comment: column.comment.clone(),
                    empty: false,
                },
            );
        }
    }

    fn finish(mut self) -> Completion {
        // A stable sort keeps the order of the relations of the statement
        // for columns of the same rank.
        self.candidates.sort_by_key(|(rank, _)| *rank);
        self.completion.items = self
            .candidates
            .into_iter()
            .take(MAX_ITEMS)
            .map(|(_, item)| item)
            .collect();
        // A stale schema matters only when the list shows names from it,
        // or shows that it is empty.
        let items = &self.completion.items;
        let empty_schema = self.completion.empty_schema.as_deref();
        self.completion.stale.retain(|schema| {
            empty_schema == Some(schema.as_str())
                || items.iter().any(|item| {
                    item.relation.as_ref().is_some_and(|(of, _)| of == schema)
                        || (item.empty && item.name == *schema)
                })
        });
        self.completion
    }
}

#[derive(Clone)]
enum RefQualify {
    /// Insert and show the column as `qualifier.column`.
    Prefix(String),
    /// Show the source as a tag.
    Tag(String),
}
