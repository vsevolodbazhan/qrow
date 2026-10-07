//! The dbt tools of the assistant, and the dbt part of the workspace context.
//!
//! The tools read only the index of the manifest. They do not need the schema
//! catalog, a session, or a refresh. Like the other tools, they give bounded
//! results in pages and let the model ask for more: a model description is
//! light by default, the columns come on request with patterns, and lineage
//! beyond the direct parents and children comes from `dbt-read-lineage`.
//!
//! A resource names its table by the schema mapping of the connection, for
//! example `core.orders`. The catalog tools describe that table.

use super::broker::MAX_TOOL_OUTPUT_BYTES;
use crate::{
    dbt::{Entry, Index, Kind, Test, contains_folded, matching, worker::ManifestState},
    model::{DbtProject, glob_match},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, VecDeque},
    time::{SystemTime, UNIX_EPOCH},
};

/// The space for the envelope of a tool result.
const ENVELOPE_BYTES: usize = 2 * 1024;
/// The longest description that a tool gives. Longer ones are cut, and the
/// result tells so.
const MAX_DESCRIPTION_BYTES: usize = 12 * 1024;
/// The largest list of documented column names in a model description. The
/// columns argument gives all columns in pages.
const MAX_DOCUMENTED_BYTES: usize = 12 * 1024;
/// The largest list of the tests of a model, without its column tests.
const MAX_MODEL_TESTS_BYTES: usize = 8 * 1024;
/// The longest description in a list of resources.
const MAX_LIST_DESCRIPTION_BYTES: usize = 200;
/// The most parents or children that a model description lists.
/// `dbt-read-lineage` gives the others.
const MAX_DIRECT: usize = 100;
/// The resources of one page of `dbt-search-models` and `dbt-read-lineage`.
pub const DEFAULT_PAGE: usize = 50;
pub const MAX_PAGE: usize = 500;
/// The deepest lineage that one call follows.
pub const MAX_DEPTH: usize = 100;
/// The largest list of models that the tab SQL names, in the workspace
/// context.
pub const MAX_REFERENCED_BYTES: usize = 8 * 1024;

/// Why a dbt tool cannot answer.
#[derive(Debug, Eq, PartialEq)]
pub struct Failure {
    pub code: &'static str,
    pub message: String,
}

fn failure(code: &'static str, message: impl Into<String>) -> Failure {
    Failure {
        code,
        message: message.into(),
    }
}

/// The dbt project of a connection, with the index of its manifest.
pub struct Project<'a> {
    pub index: &'a Index,
    pub project: &'a DbtProject,
    /// When Qrow made the index.
    pub refreshed: Option<SystemTime>,
    /// Whether the manifest changed after Qrow made the index.
    pub changed: bool,
}

/// Which way `dbt-read-lineage` follows the dependencies.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Upstream,
    Downstream,
    #[default]
    Both,
}

/// The filters of `dbt-search-models`. Empty filters match all resources.
#[derive(Debug, Default)]
pub struct Search<'s> {
    /// Glob patterns. A resource matches when one pattern matches its name,
    /// its table, or its unique ID.
    pub patterns: &'s [String],
    /// Text in the description, without regard to letter case.
    pub text: Option<&'s str>,
    pub tag: Option<&'s str>,
    pub resource_type: Option<Kind>,
}

impl<'a> Project<'a> {
    pub fn new(
        index: &'a Index,
        project: &'a DbtProject,
        refreshed: Option<SystemTime>,
        changed: bool,
    ) -> Self {
        Self {
            index,
            project,
            refreshed,
            changed,
        }
    }

    /// The table that dbt builds for `entry`, like `core.orders`, after the
    /// schema mapping. `None` for an ephemeral model.
    pub fn relation(&self, entry: &Entry) -> Option<String> {
        if entry.kind == Kind::Model && entry.relation_name.is_none() {
            return None;
        }
        let schema = matching::mapped_schema(self.index, entry, self.project);
        Some(format!("{schema}.{}", entry.identifier))
    }

    /// The resource of a catalog table.
    pub fn entry_for(&self, schema: &str, relation: &str) -> Option<u32> {
        matching::entry_for(self.index, self.project, schema, relation)
    }

    /// Find a resource by its unique ID, its table like `core.orders`, or its
    /// name when only one resource has it.
    pub fn resolve(&self, model: &str) -> Result<u32, Failure> {
        let model = model.trim();
        if let Some(position) = self.index.find(model) {
            return Ok(position);
        }
        if let Some((schema, relation)) = model.rsplit_once('.')
            && let Some(position) = self.entry_for(schema, relation)
        {
            return Ok(position);
        }
        let named: Vec<u32> = (0..self.index.entries().len() as u32)
            .filter(|position| self.index.entry(*position).name.eq_ignore_ascii_case(model))
            .collect();
        match named.as_slice() {
            [position] => Ok(*position),
            [] => Err(failure(
                "model_not_found",
                format!("The dbt project has no resource {model}. Find it with dbt-search-models."),
            )),
            several => Err(failure(
                "ambiguous_model",
                format!(
                    "Several resources have the name {model}: {}. Use a unique ID.",
                    several
                        .iter()
                        .take(10)
                        .map(|position| &*self.index.entry(*position).unique_id)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
        }
    }

    /// The name of a resource in a list: its table, or the unique ID of an
    /// ephemeral model.
    fn short_name(&self, position: u32) -> String {
        let entry = self.index.entry(position);
        self.relation(entry)
            .unwrap_or_else(|| entry.unique_id.to_string())
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
        let mut value = json!({"test": self.index.symbol(test.name)});
        if !test.values.is_empty() {
            value["values"] = json!(test.values);
        }
        if test.to_text.is_some() {
            value["to"] = json!(
                test.to
                    .map(|to| self.short_name(to))
                    .or_else(|| test.to_text.as_deref().map(str::to_owned))
            );
            value["field"] = json!(test.field);
        }
        if let Some(arguments) = &test.arguments {
            value["arguments"] = serde_json::from_str(arguments).unwrap_or(Value::Null);
        }
        value
    }

    /// A resource in a list, with the start of its description.
    fn listed(&self, position: u32, description: bool) -> Value {
        let entry = self.index.entry(position);
        let mut value = json!({
            "unique_id": entry.unique_id,
            "resource_type": entry.kind.name(),
            "relation": self.relation(entry),
        });
        if description {
            let first = entry.description.lines().next().unwrap_or_default();
            let (text, cut) = cut_flag(first, MAX_LIST_DESCRIPTION_BYTES);
            value["description"] = json!(text);
            value["description_truncated"] =
                json!(cut || first.len() < entry.description.trim_end().len());
        }
        value
    }

    /// The details of a resource for `dbt-describe-model`. Without
    /// `columns`, it lists only the names of the columns that the project
    /// documents. With glob patterns in `columns`, it gives the matching
    /// columns with their descriptions and tests, from `column_offset`, as
    /// many as fit in one result.
    pub fn describe(&self, position: u32, columns: &[String], column_offset: usize) -> Value {
        let index = self.index;
        let entry = index.entry(position);
        let tests = index.tests(position);
        // The columns of the project, then the columns that only tests name.
        let mut names: Vec<&str> = entry.columns.iter().map(|column| &*column.name).collect();
        for test in tests {
            if let Some(column) = test.column.as_deref()
                && !names.iter().any(|name| name.eq_ignore_ascii_case(column))
            {
                names.push(column);
            }
        }
        let column_tests = |name: &str| -> Vec<&Test> {
            tests
                .iter()
                .filter(|test| {
                    test.column
                        .as_deref()
                        .is_some_and(|column| column.eq_ignore_ascii_case(name))
                })
                .collect()
        };
        let declared = |name: &str| entry.columns.iter().find(|column| &*column.name == name);
        let documented: Vec<Value> = names
            .iter()
            .filter(|name| {
                !column_tests(name).is_empty()
                    || declared(name).is_some_and(|column| {
                        !column.description.is_empty() || column.data_type.is_some()
                    })
            })
            .map(|name| json!(name))
            .collect();
        let (documented, documented_truncated) = fit(documented, MAX_DOCUMENTED_BYTES);
        let model_tests: Vec<Value> = tests
            .iter()
            .filter(|test| test.column.is_none())
            .map(|test| self.test(test))
            .collect();
        let model_test_count = model_tests.len();
        let (model_tests, model_tests_truncated) = fit(model_tests, MAX_MODEL_TESTS_BYTES);
        let parents = &entry.parents;
        let children = index.children(position);
        let (description, description_truncated) =
            cut_flag(&entry.description, MAX_DESCRIPTION_BYTES);
        let mut value = json!({
            "unique_id": entry.unique_id,
            "resource_type": entry.kind.name(),
            "name": entry.name,
            "relation": self.relation(entry),
            "materialized": entry.materialized.map(|m| index.symbol(m)),
            "description": description,
            "description_truncated": description_truncated,
            "tags": entry.tags.iter().map(|tag| index.symbol(*tag)).collect::<Vec<_>>(),
            "path": entry.path,
            "source_name": entry.source_name.map(|name| index.symbol(name)),
            "column_count": names.len(),
            "documented_columns": documented,
            "documented_columns_truncated": documented_truncated,
            "test_count": tests.len(),
            "model_test_count": model_test_count,
            "model_tests": model_tests,
            "model_tests_truncated": model_tests_truncated,
            "parent_count": parents.len(),
            "parents": parents.iter().take(MAX_DIRECT).map(|p| self.short_name(*p)).collect::<Vec<_>>(),
            "parents_truncated": parents.len() > MAX_DIRECT,
            "child_count": children.len(),
            "children": children.iter().take(MAX_DIRECT).map(|c| self.short_name(*c)).collect::<Vec<_>>(),
            "children_truncated": children.len() > MAX_DIRECT,
            "manifest": self.manifest(),
        });
        if columns.is_empty() {
            return value;
        }
        let matched: Vec<&str> = names
            .iter()
            .copied()
            .filter(|name| columns.iter().any(|pattern| glob_match(pattern, name)))
            .collect();
        // A column within `limit` bytes: a column that does not fit gets a
        // shorter description, and then fewer tests.
        let column = |name: &str, limit: usize| {
            let declared = declared(name);
            let tests: Vec<Value> = column_tests(name)
                .into_iter()
                .map(|test| self.test(test))
                .collect();
            let test_count = tests.len();
            let make = |description: usize, tests: &[Value]| {
                let (text, truncated) = cut_flag(
                    declared.map_or("", |column| &column.description),
                    description,
                );
                json!({
                    "name": name,
                    "description": text,
                    "description_truncated": truncated,
                    "data_type": declared.and_then(|column| column.data_type).map(|t| index.symbol(t)),
                    "test_count": test_count,
                    "tests": tests,
                    "tests_truncated": tests.len() < test_count,
                })
            };
            let full = make(MAX_DESCRIPTION_BYTES, &tests);
            if size(&full) <= limit {
                return full;
            }
            let (kept, _) = fit(tests, limit / 2);
            let without = size(&make(0, &kept));
            let shortened = make(limit.saturating_sub(without + 16), &kept);
            if size(&shortened) <= limit {
                shortened
            } else {
                make(0, &[])
            }
        };
        value["matched_column_count"] = json!(matched.len());
        value["column_offset"] = json!(column_offset);
        let budget = MAX_TOOL_OUTPUT_BYTES - ENVELOPE_BYTES;
        let mut used = size(&value) + 64;
        let mut page = Vec::new();
        for name in matched.iter().skip(column_offset) {
            let item = column(name, usize::MAX);
            let item_size = size(&item) + 1;
            if used + item_size <= budget {
                used += item_size;
                page.push(item);
                continue;
            }
            // A column that does not fit alone is shortened to the space left.
            if page.is_empty() {
                page.push(column(name, budget.saturating_sub(used)));
            }
            break;
        }
        let end = column_offset.min(matched.len()) + page.len();
        value["columns"] = json!(page);
        value["next_column_offset"] = json!((end < matched.len()).then_some(end));
        value
    }

    /// The resources that pass `filters`, from `offset`, for
    /// `dbt-search-models`.
    pub fn search(&self, filters: &Search, offset: usize, limit: usize) -> Value {
        let text = filters.text.map(|text| text.trim().to_lowercase());
        let found: Vec<u32> = (0..self.index.entries().len() as u32)
            .filter(|position| {
                let entry = self.index.entry(*position);
                filters.resource_type.is_none_or(|kind| entry.kind == kind)
                    && filters.tag.is_none_or(|tag| {
                        entry.tags.iter().any(|candidate| {
                            self.index
                                .symbol(*candidate)
                                .eq_ignore_ascii_case(tag.trim())
                        })
                    })
                    && text
                        .as_deref()
                        .is_none_or(|text| contains_folded(&entry.description, text))
                    && (filters.patterns.is_empty()
                        || filters.patterns.iter().any(|pattern| {
                            glob_match(pattern, &entry.name)
                                || glob_match(pattern, &entry.unique_id)
                                || self
                                    .relation(entry)
                                    .is_some_and(|relation| glob_match(pattern, &relation))
                        }))
            })
            .collect();
        page(&found, offset, limit, |items, next| {
            json!({
                "model_count": found.len(),
                "offset": offset,
                "next_offset": next,
                "models": items.iter().map(|position| self.listed(*position, true)).collect::<Vec<_>>(),
                "manifest": self.manifest(),
            })
        })
    }

    /// The resources upstream or downstream of `position`, to `depth` steps,
    /// from `offset`, for `dbt-read-lineage`.
    pub fn lineage(
        &self,
        position: u32,
        direction: Direction,
        depth: usize,
        descriptions: bool,
        offset: usize,
        limit: usize,
    ) -> Value {
        let mut found: Vec<(u32, &'static str, usize)> = Vec::new();
        let mut walk = |name: &'static str, next: &dyn Fn(u32) -> Vec<u32>| {
            let mut seen = BTreeSet::from([position]);
            let mut queue = VecDeque::from([(position, 0)]);
            while let Some((current, steps)) = queue.pop_front() {
                if steps == depth {
                    continue;
                }
                for other in next(current) {
                    if seen.insert(other) {
                        found.push((other, name, steps + 1));
                        queue.push_back((other, steps + 1));
                    }
                }
            }
        };
        if direction != Direction::Downstream {
            walk("upstream", &|current| {
                self.index.entry(current).parents.clone()
            });
        }
        if direction != Direction::Upstream {
            walk("downstream", &|current| {
                self.index.children(current).to_vec()
            });
        }
        let entry = self.index.entry(position);
        page(&found, offset, limit, |items, next| {
            json!({
                "unique_id": entry.unique_id,
                "relation": self.relation(entry),
                "depth": depth,
                "resource_count": found.len(),
                "offset": offset,
                "next_offset": next,
                "resources": items.iter().map(|(other, direction, steps)| {
                    let mut value = self.listed(*other, descriptions);
                    value["direction"] = json!(direction);
                    value["depth"] = json!(steps);
                    value
                }).collect::<Vec<_>>(),
                "manifest": self.manifest(),
            })
        })
    }

    /// The models that `sql` names, in at most `MAX_REFERENCED_BYTES`, and
    /// whether some did not fit. A name without a schema uses
    /// `default_schema`. The match is best-effort, like the catalog
    /// context.
    fn referenced(&self, sql: &str, default_schema: &str) -> (Vec<ReferencedModel>, bool) {
        let mut found: Vec<ReferencedModel> = Vec::new();
        let mut bytes = 0;
        let mut truncated = false;
        // One pass over the manifest, and one lookup for each distinct name.
        let relations = matching::relations(self.index, self.project);
        let mut seen = BTreeSet::new();
        for name in super::catalog::names(sql) {
            let (schema, relation) = match name.as_slice() {
                [.., schema, relation] => (schema.as_str(), relation.as_str()),
                [relation] => (default_schema, relation.as_str()),
                [] => continue,
            };
            let key = (schema.to_lowercase(), relation.to_lowercase());
            if !seen.insert(key.clone()) {
                continue;
            }
            let Some(&position) = relations.get(&key) else {
                continue;
            };
            let entry = self.index.entry(position);
            if found
                .iter()
                .any(|model| *model.unique_id == *entry.unique_id)
            {
                continue;
            }
            let first = entry.description.lines().next().unwrap_or_default();
            let model = ReferencedModel {
                relation: format!("{schema}.{relation}"),
                unique_id: entry.unique_id.to_string(),
                description: cut(first, MAX_LIST_DESCRIPTION_BYTES),
            };
            let model_size = serde_json::to_vec(&model).map_or(usize::MAX, |bytes| bytes.len());
            if bytes + model_size > MAX_REFERENCED_BYTES {
                truncated = true;
                continue;
            }
            bytes += model_size;
            found.push(model);
        }
        (found, truncated)
    }
}

/// The dbt project of the connection of a conversation, in the workspace
/// context.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct DbtContext {
    pub connection_id: uuid::Uuid,
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
    /// Qrow reads the manifest for the first time.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub reading: bool,
    /// Why Qrow has no current data of the manifest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The models whose tables the tab SQL names.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub referenced_models: Vec<ReferencedModel>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub referenced_models_truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReferencedModel {
    /// The table as the SQL names it, with its schema.
    pub relation: String,
    pub unique_id: String,
    /// The first line of the description.
    pub description: String,
}

/// The dbt context of a connection. `project` is `None` while Qrow has no
/// index of the manifest. `sql` is the SQL of the tab, and `default_schema`
/// the initial database of the connection.
pub fn context(
    connection_id: uuid::Uuid,
    state: &ManifestState,
    project: Option<&Project>,
    sql: &str,
    default_schema: &str,
) -> DbtContext {
    let context = DbtContext {
        connection_id,
        reading: state.parsing && project.is_none(),
        error: state.error.as_ref().map(ToString::to_string),
        ..DbtContext::default()
    };
    let Some(project) = project else {
        return context;
    };
    let count = |kind| {
        project
            .index
            .entries()
            .iter()
            .filter(|entry| entry.kind == kind)
            .count()
    };
    let (referenced_models, referenced_models_truncated) = project.referenced(sql, default_schema);
    DbtContext {
        project: Some(project.index.project.to_string()),
        dbt_version: Some(project.index.dbt_version.to_string()),
        manifest_generated_at: Some(project.index.generated_at.to_string()),
        refreshed_at: project.refreshed.and_then(seconds),
        manifest_changed: project.changed,
        models: Some(count(Kind::Model)),
        sources: Some(count(Kind::Source)),
        tests: Some(project.index.test_count()),
        has_compiled_sql: Some(project.index.has_compiled_code),
        referenced_models,
        referenced_models_truncated,
        ..context
    }
}

/// The kind of a `resource_type` argument.
pub fn kind(name: &str) -> Option<Kind> {
    [Kind::Model, Kind::Seed, Kind::Snapshot, Kind::Source]
        .into_iter()
        .find(|kind| kind.name() == name)
}

/// A page of `items` from `offset`, halved until it fits in a tool result.
fn page<T>(
    items: &[T],
    offset: usize,
    limit: usize,
    make: impl Fn(&[T], Option<usize>) -> Value,
) -> Value {
    let start = offset.min(items.len());
    let mut count = limit.clamp(1, MAX_PAGE).min(items.len() - start);
    loop {
        let end = start + count;
        let next = (end < items.len()).then_some(end);
        let value = make(&items[start..end], next);
        if size(&value) + ENVELOPE_BYTES <= MAX_TOOL_OUTPUT_BYTES || count <= 1 {
            return value;
        }
        count /= 2;
    }
}

/// The first `items` that fit in `limit` bytes, and whether some did not.
fn fit(items: Vec<Value>, limit: usize) -> (Vec<Value>, bool) {
    let mut used = 2;
    let count = items
        .iter()
        .take_while(|item| {
            used += size(item) + 1;
            used <= limit
        })
        .count();
    let truncated = count < items.len();
    (items.into_iter().take(count).collect(), truncated)
}

fn seconds(time: SystemTime) -> Option<u64> {
    time.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

fn size(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |bytes| bytes.len())
}

/// `text` cut to at most `limit` bytes, with `…` at the end when it is cut.
fn cut(text: &str, limit: usize) -> String {
    cut_flag(text, limit).0
}

/// `text` cut so that it takes at most `limit` bytes in JSON, with `…` at
/// the end, and whether it was cut. Escapes count: a newline takes two bytes.
fn cut_flag(text: &str, limit: usize) -> (String, bool) {
    let cost = |c: char| match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if (c as u32) < 0x20 => 6,
        c => c.len_utf8(),
    };
    if text.chars().map(cost).sum::<usize>() <= limit {
        return (text.to_owned(), false);
    }
    let room = limit.saturating_sub('…'.len_utf8());
    let mut used = 0;
    let mut end = 0;
    for (at, c) in text.char_indices() {
        used += cost(c);
        if used > room {
            break;
        }
        end = at + c.len_utf8();
    }
    (format!("{}…", &text[..end]), true)
}

#[cfg(test)]
mod tests;
