//! Find the catalog relation of each dbt resource of a connection.
//!
//! The schema mapping of the connection changes the dbt schema into a
//! catalog schema. Then the schema and the alias or identifier must name a
//! relation in the catalog. Letter case does not matter, like in Spark. The
//! database of a resource does not count: Spark has no database level above
//! schemas.

use super::{Entry, Index, Kind};
use crate::{catalog::Catalog, model::DbtProject};
use std::collections::HashMap;

/// Case-insensitive identities with case-distinct names excluded.
fn unique_names(names: impl IntoIterator<Item = String>) -> HashMap<String, String> {
    let mut unique: HashMap<String, Option<String>> = HashMap::new();
    for name in names {
        unique
            .entry(name.to_lowercase())
            .and_modify(|known| {
                if known.as_ref().is_some_and(|old| old != &name) {
                    *known = None;
                }
            })
            .or_insert(Some(name));
    }
    unique
        .into_iter()
        .filter_map(|(key, name)| name.map(|name| (key, name)))
        .collect()
}

/// The schema and relation names of a catalog, in lowercase, for lookups
/// without regard to letter case.
pub struct CatalogNames {
    /// The lowercase schema name, its name, and the lowercase names of its
    /// relations if the catalog has them.
    schemas: HashMap<String, (String, Option<HashMap<String, String>>)>,
}

impl CatalogNames {
    pub fn of(catalog: &Catalog) -> Self {
        let schemas = unique_names(catalog.schemas.keys().cloned())
            .into_iter()
            .map(|(key, name)| {
                let schema = &catalog.schemas[&name];
                let relations = schema
                    .relations
                    .as_ref()
                    .map(|relations| unique_names(relations.keys().cloned()));
                (key, (name, relations))
            })
            .collect();
        Self { schemas }
    }
}

/// Where the catalog has a dbt resource.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Match {
    /// The names of the relation in the catalog.
    Relation { schema: String, relation: String },
    /// The catalog has no schema with the mapped name.
    NoSchema,
    /// The schema has no relation with the name.
    NoRelation,
    /// The catalog has the schema, but not its relations yet.
    NotLoaded,
    /// The resource is not a relation, like an ephemeral model.
    NotARelation,
}

/// The catalog schema of `entry`, after the schema mapping.
pub fn mapped_schema(index: &Index, entry: &Entry, project: &DbtProject) -> String {
    project.map_schema(index.symbol(entry.schema)).into_owned()
}

/// Find the relation of `entry` in the catalog.
pub fn find(index: &Index, entry: &Entry, project: &DbtProject, names: &CatalogNames) -> Match {
    if !is_ephemeral(entry)
        && !relations(index, project).contains_key(&(
            mapped_schema(index, entry, project).to_lowercase(),
            entry.identifier.to_lowercase(),
        ))
    {
        return Match::NoRelation;
    }
    find_catalog(index, entry, project, names)
}

fn find_catalog(index: &Index, entry: &Entry, project: &DbtProject, names: &CatalogNames) -> Match {
    if is_ephemeral(entry) {
        return Match::NotARelation;
    }
    let schema = mapped_schema(index, entry, project).to_lowercase();
    let Some((schema_name, relations)) = names.schemas.get(&schema) else {
        return Match::NoSchema;
    };
    let Some(relations) = relations else {
        return Match::NotLoaded;
    };
    match relations.get(&entry.identifier.to_lowercase()) {
        Some(relation) => Match::Relation {
            schema: schema_name.clone(),
            relation: relation.clone(),
        },
        None => Match::NoRelation,
    }
}

fn is_ephemeral(entry: &Entry) -> bool {
    entry.kind == Kind::Model && entry.relation_name.is_none()
}

/// How many dbt resources of a connection match catalog relations.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Summary {
    /// The models, seeds, snapshots, and sources that are relations.
    pub total: usize,
    pub matched: usize,
    /// The resources whose schema has no loaded relations.
    pub not_loaded: usize,
    /// The resources without a match, with their mapped schema and the
    /// reason, in the order of the index.
    pub unmatched: Vec<(u32, String, Match)>,
}

pub fn summary(index: &Index, project: &DbtProject, names: &CatalogNames) -> Summary {
    let mut summary = Summary::default();
    let identities = relations(index, project);
    for (position, entry) in index.entries().iter().enumerate() {
        let found = if !is_ephemeral(entry)
            && !identities.contains_key(&(
                mapped_schema(index, entry, project).to_lowercase(),
                entry.identifier.to_lowercase(),
            )) {
            Match::NoRelation
        } else {
            find_catalog(index, entry, project, names)
        };
        match found {
            Match::NotARelation => continue,
            Match::Relation { .. } => summary.matched += 1,
            Match::NotLoaded => summary.not_loaded += 1,
            Match::NoSchema | Match::NoRelation => {
                let schema = mapped_schema(index, entry, project);
                summary.unmatched.push((position as u32, schema, found));
            }
        }
        summary.total += 1;
    }
    summary
}

/// The dbt resource of the catalog relation `relation` in `schema`. A model
/// comes before a snapshot, a seed, and a source of the same relation.
pub fn entry_for(index: &Index, project: &DbtProject, schema: &str, relation: &str) -> Option<u32> {
    relations(index, project)
        .get(&(schema.to_lowercase(), relation.to_lowercase()))
        .copied()
}

fn rank(kind: Kind) -> u8 {
    match kind {
        Kind::Model => 0,
        Kind::Snapshot => 1,
        Kind::Seed => 2,
        Kind::Source => 3,
    }
}

/// The dbt resource of each catalog relation, by lowercase schema and
/// relation, for many lookups. It chooses like [`entry_for`].
pub fn relations(index: &Index, project: &DbtProject) -> HashMap<(String, String), u32> {
    let schemas = unique_names(
        index
            .entries()
            .iter()
            .filter(|entry| !is_ephemeral(entry))
            .map(|entry| mapped_schema(index, entry, project)),
    );
    type Identity = (String, String);
    let mut relations: HashMap<Identity, Option<(Identity, u32)>> = HashMap::new();
    for (position, entry) in index.entries().iter().enumerate() {
        if is_ephemeral(entry) {
            continue;
        }
        let identity = (
            mapped_schema(index, entry, project),
            entry.identifier.to_string(),
        );
        if !schemas.contains_key(&identity.0.to_lowercase()) {
            continue;
        }
        let key = (identity.0.to_lowercase(), identity.1.to_lowercase());
        let position = position as u32;
        relations
            .entry(key)
            .and_modify(|known| {
                if let Some((previous, known_position)) = known {
                    if previous != &identity {
                        *known = None;
                    } else if rank(entry.kind) < rank(index.entry(*known_position).kind) {
                        *known_position = position;
                    }
                }
            })
            .or_insert(Some((identity, position)));
    }
    relations
        .into_iter()
        .filter_map(|(key, entry)| entry.map(|(_, position)| (key, position)))
        .collect()
}
