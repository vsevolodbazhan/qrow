//! A read-only index of a dbt project, made from its `manifest.json`.
//!
//! Qrow reads the descriptions, tests, lineage, semantic models, and metrics
//! of a project from the manifest that dbt writes. It does not run dbt, and
//! it does not read the YAML, SQL, or Jinja of the project. The index keeps
//! no SQL: it keeps the byte span of each SQL string in the manifest, and
//! [`read_sql`] reads a span on demand.

mod manifest;
pub mod matching;
pub mod saved;
pub mod worker;

pub use manifest::{ParseError, parse};

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    io::{Read, Seek, SeekFrom},
    path::Path,
};

/// The manifest schema that Qrow reads. dbt-core 1.8 and later and dbt
/// Fusion 2.0 write it.
pub const MANIFEST_SCHEMA_VERSION: u32 = 12;

/// An interned string of an [`Index`], for values that repeat, like schema
/// names, tags, and types.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Symbol(u32);

/// The byte range of a JSON string literal in the manifest file, with its
/// quotes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Span {
    pub start: u64,
    pub len: u32,
}

/// The kind of an [`Entry`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum Kind {
    Model,
    Seed,
    Snapshot,
    Source,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Seed => "seed",
            Self::Snapshot => "snapshot",
            Self::Source => "source",
        }
    }
}

/// A column that the project documents.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Column {
    pub name: Box<str>,
    pub description: Box<str>,
    /// The type that the project declares, if any.
    pub data_type: Option<Symbol>,
}

/// A model, seed, snapshot, or source: the dbt resources that are relations
/// in the warehouse.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Entry {
    pub unique_id: Box<str>,
    pub kind: Kind,
    pub name: Box<str>,
    pub package: Symbol,
    /// `None` when the adapter has no database level, like Spark.
    pub database: Option<Symbol>,
    pub schema: Symbol,
    /// The alias of a model, seed, or snapshot, or the identifier of a
    /// source: the name of the relation in its schema.
    pub identifier: Box<str>,
    /// The quoted relation name, or `None` for an ephemeral model.
    pub relation_name: Option<Box<str>>,
    pub materialized: Option<Symbol>,
    pub description: Box<str>,
    pub tags: Vec<Symbol>,
    pub columns: Vec<Column>,
    /// The entries that this entry selects from.
    pub parents: Vec<u32>,
    /// The path of the file that defines the entry, relative to the project.
    pub path: Box<str>,
    pub compiled_path: Option<Box<str>>,
    /// The source name of a source, for `source('name', 'table')`.
    pub source_name: Option<Symbol>,
    pub loader: Option<Symbol>,
    pub raw_code: Option<Span>,
    pub compiled_code: Option<Span>,
}

/// A generic data test of an entry, for example `unique` on a column.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Test {
    /// The test name, for example `unique`, `not_null`, `accepted_values`,
    /// `relationships`, or the name of a custom generic test.
    pub name: Symbol,
    /// The tested entry.
    pub entry: u32,
    pub column: Option<Box<str>>,
    /// The values of `accepted_values`.
    pub values: Vec<Box<str>>,
    /// The target of `relationships`, and its text, like `ref('customers')`.
    pub to: Option<u32>,
    pub to_text: Option<Box<str>>,
    /// The target column of `relationships`.
    pub field: Option<Box<str>>,
    /// The other arguments of a test that Qrow does not know, as JSON, if
    /// they are small.
    pub arguments: Option<Box<str>>,
}

/// An entity, dimension, or measure of a semantic model.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SemanticPart {
    pub name: Box<str>,
    /// The type of an entity or dimension, or the aggregation of a measure.
    pub kind: Option<Symbol>,
    pub expr: Option<Box<str>>,
    pub description: Option<Box<str>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SemanticModel {
    pub name: Box<str>,
    pub description: Box<str>,
    /// The entry that the semantic model reads.
    pub entry: Option<u32>,
    pub agg_time_dimension: Option<Box<str>>,
    pub entities: Vec<SemanticPart>,
    pub dimensions: Vec<SemanticPart>,
    pub measures: Vec<SemanticPart>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Metric {
    pub name: Box<str>,
    pub label: Option<Box<str>>,
    pub description: Box<str>,
    /// For example `simple`, `ratio`, `derived`, or `cumulative`.
    pub kind: Option<Symbol>,
    /// The type parameters as JSON, for example the measure or the
    /// expression, if they are small.
    pub type_params: Option<Box<str>>,
    pub filter: Option<Box<str>>,
}

/// A downstream use of entries, like a dashboard.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Exposure {
    pub name: Box<str>,
    pub kind: Option<Symbol>,
    pub label: Option<Box<str>>,
    pub description: Box<str>,
    pub owner: Option<Box<str>>,
    pub url: Option<Box<str>>,
    pub parents: Vec<u32>,
}

/// The parsed content of one manifest.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Index {
    pub project: Box<str>,
    pub dbt_version: Box<str>,
    pub generated_at: Box<str>,
    pub adapter: Box<str>,
    /// Whether the manifest has compiled SQL. A manifest from `dbt parse`
    /// has none.
    pub has_compiled_code: bool,
    symbols: Vec<Box<str>>,
    /// Sorted by unique ID.
    entries: Vec<Entry>,
    /// Sorted by entry.
    tests: Vec<Test>,
    /// The children of each entry, computed from the parents.
    children: Vec<Vec<u32>>,
    pub semantic_models: Vec<SemanticModel>,
    pub metrics: Vec<Metric>,
    pub exposures: Vec<Exposure>,
}

impl Index {
    pub fn symbol(&self, symbol: Symbol) -> &str {
        &self.symbols[symbol.0 as usize]
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn entry(&self, index: u32) -> &Entry {
        &self.entries[index as usize]
    }

    /// The position of the entry with `unique_id`.
    pub fn find(&self, unique_id: &str) -> Option<u32> {
        self.entries
            .binary_search_by(|entry| (*entry.unique_id).cmp(unique_id))
            .ok()
            .map(|index| index as u32)
    }

    pub fn children(&self, index: u32) -> &[u32] {
        &self.children[index as usize]
    }

    /// The tests of the entry at `index`.
    pub fn tests(&self, index: u32) -> &[Test] {
        let start = self.tests.partition_point(|test| test.entry < index);
        let end = self.tests.partition_point(|test| test.entry <= index);
        &self.tests[start..end]
    }

    pub fn test_count(&self) -> usize {
        self.tests.len()
    }

    /// Check that the positions and symbols of a loaded index are in range
    /// and that its lists have their order, so that lookups do not panic.
    fn check(&self) -> Result<(), String> {
        let entries = self.entries.len();
        let symbols = self.symbols.len();
        let entry = |position: &u32| (*position as usize) < entries;
        let symbol = |symbol: &Symbol| (symbol.0 as usize) < symbols;
        let optional = |value: &Option<Symbol>| value.as_ref().is_none_or(symbol);
        let entries_ok = self.entries.iter().all(|item| {
            symbol(&item.package)
                && optional(&item.database)
                && symbol(&item.schema)
                && optional(&item.materialized)
                && optional(&item.source_name)
                && optional(&item.loader)
                && item.tags.iter().all(symbol)
                && item
                    .columns
                    .iter()
                    .all(|column| optional(&column.data_type))
                && item.parents.iter().all(entry)
        });
        let parts_ok = |parts: &[SemanticPart]| parts.iter().all(|part| optional(&part.kind));
        let ok = entries_ok
            && self
                .entries
                .windows(2)
                .all(|pair| pair[0].unique_id < pair[1].unique_id)
            && self.tests.iter().all(|test| {
                symbol(&test.name) && entry(&test.entry) && test.to.as_ref().is_none_or(entry)
            })
            && self.tests.is_sorted_by_key(|test| test.entry)
            && self.children.len() == entries
            && self.children.iter().flatten().all(entry)
            && self.semantic_models.iter().all(|model| {
                model.entry.as_ref().is_none_or(entry)
                    && parts_ok(&model.entities)
                    && parts_ok(&model.dimensions)
                    && parts_ok(&model.measures)
            })
            && self.metrics.iter().all(|metric| optional(&metric.kind))
            && self
                .exposures
                .iter()
                .all(|exposure| optional(&exposure.kind) && exposure.parents.iter().all(entry));
        if ok {
            Ok(())
        } else {
            Err("a position or a symbol is out of range".into())
        }
    }

    /// The positions of the entries whose name, relation, tags, or
    /// description contain `query`, without regard to letter case, up to
    /// `limit`. Names and relations match before descriptions.
    pub fn search(&self, query: &str, kind: Option<Kind>, limit: usize) -> Vec<u32> {
        let query = query.trim().to_lowercase();
        let candidates = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| kind.is_none_or(|kind| entry.kind == kind));
        let mut by_name = Vec::new();
        let mut by_text = Vec::new();
        for (index, entry) in candidates {
            if contains_folded(&entry.name, &query)
                || contains_folded(&entry.identifier, &query)
                || entry
                    .tags
                    .iter()
                    .any(|tag| self.symbol(*tag).eq_ignore_ascii_case(&query))
            {
                by_name.push(index as u32);
            } else if by_name.len() + by_text.len() < limit
                && contains_folded(&entry.description, &query)
            {
                by_text.push(index as u32);
            }
            if by_name.len() >= limit {
                break;
            }
        }
        by_name.extend(by_text);
        by_name.truncate(limit);
        by_name
    }
}

/// Whether `text` contains `query`, which is in lowercase, without regard to
/// ASCII letter case. Other letters must match in their lowercase form.
fn contains_folded(text: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    if text.is_ascii() && query.is_ascii() {
        let (text, query) = (text.as_bytes(), query.as_bytes());
        return text
            .windows(query.len())
            .any(|window| window.eq_ignore_ascii_case(query));
    }
    text.to_lowercase().contains(query)
}

/// Builds the symbol table of an index.
#[derive(Default)]
struct Symbols {
    list: Vec<Box<str>>,
    positions: HashMap<Box<str>, u32>,
}

impl Symbols {
    fn intern(&mut self, text: &str) -> Symbol {
        if let Some(position) = self.positions.get(text) {
            return Symbol(*position);
        }
        let position = self.list.len() as u32;
        self.list.push(text.into());
        self.positions.insert(text.into(), position);
        Symbol(position)
    }
}

/// Read the SQL string at `span` of the manifest at `path`. The caller
/// checks first that the manifest did not change after the index was made.
pub fn read_sql(path: &Path, span: Span) -> anyhow::Result<String> {
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(span.start))?;
    let mut bytes = vec![0; span.len as usize];
    file.read_exact(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

#[cfg(test)]
mod tests;
