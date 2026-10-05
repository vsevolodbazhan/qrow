//! The parser of `manifest.json`. It reads the file in one pass with typed
//! structs that borrow from the input, and it skips the large parts that Qrow
//! does not need, like macros, without allocation.

use super::{
    Column, Entry, Exposure, Index, Kind, MANIFEST_SCHEMA_VERSION, Metric, SemanticModel,
    SemanticPart, Span, Symbols, Test,
};
use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor},
};
use serde_json::value::RawValue;
use std::{borrow::Cow, collections::HashMap, fmt};

/// The largest JSON text of test arguments or metric parameters that the
/// index keeps.
const MAX_JSON_FIELD_BYTES: usize = 4 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    /// The file is a manifest of another schema version.
    UnsupportedVersion(String),
    /// The file is not a valid manifest.
    Invalid(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => write!(
                formatter,
                "Unsupported manifest version {version}: Qrow reads manifest v{MANIFEST_SCHEMA_VERSION} (dbt 1.8 and later)"
            ),
            Self::Invalid(message) => write!(formatter, "The manifest is not valid: {message}"),
        }
    }
}

impl std::error::Error for ParseError {}

/// The marker of an unsupported version in a serde error, so that the error
/// survives the JSON parser.
const VERSION_MARKER: &str = "qrow-unsupported-manifest-version:";

/// Parse a manifest. `bytes` is the complete file.
pub fn parse(bytes: &[u8]) -> Result<Index, ParseError> {
    let mut builder = Builder::default();
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let result = ManifestSeed {
        builder: &mut builder,
        base: bytes.as_ptr() as usize,
    }
    .deserialize(&mut deserializer)
    .and_then(|()| deserializer.end());
    if let Err(error) = result {
        let message = error.to_string();
        return Err(match message.strip_prefix(VERSION_MARKER) {
            Some(rest) => ParseError::UnsupportedVersion(
                rest.split(" at line").next().unwrap_or(rest).to_owned(),
            ),
            None => ParseError::Invalid(message),
        });
    }
    builder.finish()
}

/// A string that borrows from the input when it has no escapes.
#[derive(Deserialize)]
#[serde(transparent)]
struct Str<'a>(#[serde(borrow)] Cow<'a, str>);

impl Str<'_> {
    fn boxed(&self) -> Box<str> {
        self.0.as_ref().into()
    }
}

fn boxed(text: &Option<Str<'_>>) -> Option<Box<str>> {
    text.as_ref()
        .map(Str::boxed)
        .filter(|text| !text.is_empty())
}

#[derive(Deserialize)]
struct Metadata<'a> {
    #[serde(borrow, default)]
    dbt_schema_version: Option<Str<'a>>,
    #[serde(borrow, default)]
    dbt_version: Option<Str<'a>>,
    #[serde(borrow, default)]
    generated_at: Option<Str<'a>>,
    #[serde(borrow, default)]
    project_name: Option<Str<'a>>,
    #[serde(borrow, default)]
    adapter_type: Option<Str<'a>>,
}

#[derive(Default, Deserialize)]
struct Config<'a> {
    #[serde(borrow, default)]
    materialized: Option<Str<'a>>,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Default, Deserialize)]
struct DependsOn<'a> {
    #[serde(borrow, default)]
    nodes: Vec<Str<'a>>,
}

#[derive(Deserialize)]
struct RawColumn<'a> {
    #[serde(borrow)]
    name: Str<'a>,
    #[serde(borrow, default)]
    description: Option<Str<'a>>,
    #[serde(borrow, default)]
    data_type: Option<Str<'a>>,
}

#[derive(Deserialize)]
struct TestMetadata<'a> {
    #[serde(borrow)]
    name: Str<'a>,
    #[serde(borrow, default, deserialize_with = "map_entries")]
    kwargs: Vec<(Str<'a>, &'a RawValue)>,
}

/// The fields of a node that Qrow reads. serde skips the other fields.
#[derive(Deserialize)]
struct RawNode<'a> {
    #[serde(borrow)]
    unique_id: Str<'a>,
    #[serde(borrow)]
    resource_type: Str<'a>,
    #[serde(borrow, default)]
    name: Option<Str<'a>>,
    #[serde(borrow, default)]
    package_name: Option<Str<'a>>,
    #[serde(borrow, default)]
    database: Option<Str<'a>>,
    #[serde(borrow, default)]
    schema: Option<Str<'a>>,
    #[serde(borrow, default)]
    alias: Option<Str<'a>>,
    #[serde(borrow, default)]
    identifier: Option<Str<'a>>,
    #[serde(borrow, default)]
    relation_name: Option<Str<'a>>,
    #[serde(borrow, default)]
    description: Option<Str<'a>>,
    #[serde(borrow, default)]
    tags: Tags<'a>,
    #[serde(borrow, default)]
    config: Config<'a>,
    #[serde(borrow, default, deserialize_with = "map_values")]
    columns: Vec<RawColumn<'a>>,
    #[serde(borrow, default)]
    depends_on: DependsOn<'a>,
    #[serde(borrow, default)]
    original_file_path: Option<Str<'a>>,
    #[serde(borrow, default)]
    compiled_path: Option<Str<'a>>,
    #[serde(borrow, default)]
    raw_code: Option<&'a RawValue>,
    #[serde(borrow, default)]
    compiled_code: Option<&'a RawValue>,
    #[serde(borrow, default)]
    source_name: Option<Str<'a>>,
    #[serde(borrow, default)]
    loader: Option<Str<'a>>,
    #[serde(borrow, default)]
    attached_node: Option<Str<'a>>,
    #[serde(borrow, default)]
    column_name: Option<Str<'a>>,
    #[serde(borrow, default)]
    test_metadata: Option<TestMetadata<'a>>,
}

/// Tags: a list, or one string in some older files.
#[derive(Default)]
struct Tags<'a>(Vec<Str<'a>>);

impl<'de: 'a, 'a> Deserialize<'de> for Tags<'a> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum OneOrMany<'a> {
            One(#[serde(borrow)] Str<'a>),
            Many(#[serde(borrow)] Vec<Str<'a>>),
            Nothing(()),
        }
        Ok(Tags(match OneOrMany::deserialize(deserializer)? {
            OneOrMany::One(tag) => vec![tag],
            OneOrMany::Many(tags) => tags,
            OneOrMany::Nothing(()) => Vec::new(),
        }))
    }
}

#[derive(Deserialize)]
struct RawPart<'a> {
    #[serde(borrow)]
    name: Str<'a>,
    #[serde(borrow, default, rename = "type")]
    kind: Option<Str<'a>>,
    #[serde(borrow, default)]
    agg: Option<Str<'a>>,
    #[serde(borrow, default)]
    expr: Option<Str<'a>>,
    #[serde(borrow, default)]
    description: Option<Str<'a>>,
}

#[derive(Default, Deserialize)]
struct Defaults<'a> {
    #[serde(borrow, default)]
    agg_time_dimension: Option<Str<'a>>,
}

#[derive(Deserialize)]
struct RawSemanticModel<'a> {
    #[serde(borrow)]
    name: Str<'a>,
    #[serde(borrow, default)]
    description: Option<Str<'a>>,
    #[serde(borrow, default)]
    defaults: Option<Defaults<'a>>,
    #[serde(borrow, default)]
    entities: Vec<RawPart<'a>>,
    #[serde(borrow, default)]
    dimensions: Vec<RawPart<'a>>,
    #[serde(borrow, default)]
    measures: Vec<RawPart<'a>>,
    #[serde(borrow, default)]
    depends_on: DependsOn<'a>,
}

#[derive(Deserialize)]
struct RawMetric<'a> {
    #[serde(borrow)]
    name: Str<'a>,
    #[serde(borrow, default)]
    label: Option<Str<'a>>,
    #[serde(borrow, default)]
    description: Option<Str<'a>>,
    #[serde(borrow, default, rename = "type")]
    kind: Option<Str<'a>>,
    #[serde(borrow, default)]
    type_params: Option<&'a RawValue>,
    #[serde(borrow, default)]
    filter: Option<&'a RawValue>,
}

#[derive(Deserialize)]
struct Owner<'a> {
    #[serde(borrow, default)]
    name: Option<Str<'a>>,
    #[serde(borrow, default)]
    email: Option<Str<'a>>,
}

#[derive(Deserialize)]
struct RawExposure<'a> {
    #[serde(borrow)]
    name: Str<'a>,
    #[serde(borrow, default, rename = "type")]
    kind: Option<Str<'a>>,
    #[serde(borrow, default)]
    label: Option<Str<'a>>,
    #[serde(borrow, default)]
    description: Option<Str<'a>>,
    #[serde(borrow, default)]
    owner: Option<Owner<'a>>,
    #[serde(borrow, default)]
    url: Option<Str<'a>>,
    #[serde(borrow, default)]
    depends_on: DependsOn<'a>,
}

/// The values of a JSON object, in order, without its keys.
fn map_values<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Values<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> Visitor<'de> for Values<T> {
        type Value = Vec<T>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object")
        }
        fn visit_unit<E: de::Error>(self) -> Result<Vec<T>, E> {
            Ok(Vec::new())
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Vec<T>, A::Error> {
            let mut values = Vec::with_capacity(map.size_hint().unwrap_or(0));
            while let Some((IgnoredAny, value)) = map.next_entry::<IgnoredAny, T>()? {
                values.push(value);
            }
            Ok(values)
        }
    }
    deserializer.deserialize_any(Values(std::marker::PhantomData))
}

/// The entries of a JSON object, in order.
fn map_entries<'de, D, K, V>(deserializer: D) -> Result<Vec<(K, V)>, D::Error>
where
    D: Deserializer<'de>,
    K: Deserialize<'de>,
    V: Deserialize<'de>,
{
    struct Entries<K, V>(std::marker::PhantomData<(K, V)>);
    impl<'de, K: Deserialize<'de>, V: Deserialize<'de>> Visitor<'de> for Entries<K, V> {
        type Value = Vec<(K, V)>;
        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an object")
        }
        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            Ok(Vec::new())
        }
        fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
            let mut entries = Vec::with_capacity(map.size_hint().unwrap_or(0));
            while let Some(entry) = map.next_entry()? {
                entries.push(entry);
            }
            Ok(entries)
        }
    }
    deserializer.deserialize_any(Entries(std::marker::PhantomData))
}

/// An entry before the index knows the positions of the other entries.
struct Pending {
    entry: Entry,
    parents: Vec<Box<str>>,
}

struct PendingTest {
    test: Test,
    attached: Option<Box<str>>,
    depends_on: Vec<Box<str>>,
    model_text: Option<Box<str>>,
}

#[derive(Default)]
struct Builder {
    symbols: Symbols,
    project: Option<Box<str>>,
    dbt_version: Box<str>,
    generated_at: Box<str>,
    adapter: Box<str>,
    saw_metadata: bool,
    has_compiled_code: bool,
    entries: Vec<Pending>,
    tests: Vec<PendingTest>,
    semantic_models: Vec<(SemanticModel, Vec<Box<str>>)>,
    metrics: Vec<Metric>,
    exposures: Vec<(Exposure, Vec<Box<str>>)>,
}

impl Builder {
    fn metadata(&mut self, metadata: Metadata<'_>) -> Result<(), String> {
        let version = metadata
            .dbt_schema_version
            .as_ref()
            .map(|version| version.0.as_ref())
            .unwrap_or_default();
        let number = version
            .rsplit('/')
            .next()
            .and_then(|file| file.strip_suffix(".json"))
            .and_then(|file| file.strip_prefix('v'));
        if !version.contains("/manifest/") || number.is_none() {
            return Err(format!(
                "{VERSION_MARKER}{}",
                if version.is_empty() {
                    "unknown"
                } else {
                    version
                }
            ));
        }
        if number != Some(&MANIFEST_SCHEMA_VERSION.to_string()) {
            return Err(format!("{VERSION_MARKER}v{}", number.unwrap_or_default()));
        }
        self.saw_metadata = true;
        self.project = metadata.project_name.as_ref().map(Str::boxed);
        self.dbt_version = metadata
            .dbt_version
            .as_ref()
            .map(Str::boxed)
            .unwrap_or_default();
        self.generated_at = metadata
            .generated_at
            .as_ref()
            .map(Str::boxed)
            .unwrap_or_default();
        self.adapter = metadata
            .adapter_type
            .as_ref()
            .map(Str::boxed)
            .unwrap_or_default();
        Ok(())
    }

    fn node(&mut self, node: RawNode<'_>, base: usize) {
        let kind = match node.resource_type.0.as_ref() {
            "model" => Kind::Model,
            "seed" => Kind::Seed,
            "snapshot" => Kind::Snapshot,
            "source" => Kind::Source,
            "test" => return self.test(node),
            _ => return,
        };
        if node.config.enabled == Some(false) {
            return;
        }
        let span = |raw: Option<&RawValue>| {
            raw.filter(|raw| raw.get().starts_with('"') && raw.get().len() > 2)
                .map(|raw| Span {
                    start: (raw.get().as_ptr() as usize - base) as u64,
                    len: raw.get().len() as u32,
                })
        };
        let compiled_code = span(node.compiled_code);
        self.has_compiled_code |= compiled_code.is_some();
        let symbols = &mut self.symbols;
        let identifier = match kind {
            Kind::Source => node.identifier.as_ref().or(node.name.as_ref()),
            _ => node.alias.as_ref().or(node.name.as_ref()),
        };
        let entry = Entry {
            unique_id: node.unique_id.boxed(),
            kind,
            name: node.name.as_ref().map(Str::boxed).unwrap_or_default(),
            package: symbols.intern(node.package_name.as_ref().map_or("", |p| &p.0)),
            database: node
                .database
                .as_ref()
                .filter(|database| !database.0.is_empty())
                .map(|database| symbols.intern(&database.0)),
            schema: symbols.intern(node.schema.as_ref().map_or("", |s| &s.0)),
            identifier: identifier.map(Str::boxed).unwrap_or_default(),
            relation_name: boxed(&node.relation_name),
            materialized: match kind {
                Kind::Source => None,
                _ => node
                    .config
                    .materialized
                    .as_ref()
                    .map(|m| symbols.intern(&m.0)),
            },
            description: node
                .description
                .as_ref()
                .map(Str::boxed)
                .unwrap_or_default(),
            tags: node
                .tags
                .0
                .iter()
                .map(|tag| symbols.intern(&tag.0))
                .collect(),
            columns: node
                .columns
                .iter()
                .map(|column| Column {
                    name: column.name.boxed(),
                    description: column
                        .description
                        .as_ref()
                        .map(Str::boxed)
                        .unwrap_or_default(),
                    data_type: column
                        .data_type
                        .as_ref()
                        .filter(|data_type| !data_type.0.is_empty())
                        .map(|data_type| symbols.intern(&data_type.0)),
                })
                .collect(),
            parents: Vec::new(),
            path: node
                .original_file_path
                .as_ref()
                .map(Str::boxed)
                .unwrap_or_default(),
            compiled_path: boxed(&node.compiled_path),
            source_name: node.source_name.as_ref().map(|s| symbols.intern(&s.0)),
            loader: node
                .loader
                .as_ref()
                .filter(|loader| !loader.0.is_empty())
                .map(|loader| symbols.intern(&loader.0)),
            raw_code: span(node.raw_code),
            compiled_code,
        };
        self.entries.push(Pending {
            entry,
            parents: node.depends_on.nodes.iter().map(Str::boxed).collect(),
        });
    }

    fn test(&mut self, node: RawNode<'_>) {
        let Some(metadata) = node.test_metadata else {
            // A singular test has no metadata about its meaning.
            return;
        };
        if node.config.enabled == Some(false) {
            return;
        }
        let mut test = Test {
            name: self.symbols.intern(&metadata.name.0),
            entry: 0,
            column: boxed(&node.column_name),
            values: Vec::new(),
            to: None,
            to_text: None,
            field: None,
            arguments: None,
        };
        let mut model_text = None;
        let mut others = serde_json::Map::new();
        for (key, value) in &metadata.kwargs {
            match key.0.as_ref() {
                "model" => model_text = serde_json::from_str::<Box<str>>(value.get()).ok(),
                "column_name" => {}
                "values" => {
                    test.values = serde_json::from_str::<Vec<serde_json::Value>>(value.get())
                        .unwrap_or_default()
                        .into_iter()
                        .map(|value| match value {
                            serde_json::Value::String(text) => text.into(),
                            other => other.to_string().into(),
                        })
                        .collect();
                }
                "to" => test.to_text = serde_json::from_str(value.get()).ok(),
                "field" => test.field = serde_json::from_str(value.get()).ok(),
                key => {
                    if let Ok(value) = serde_json::from_str(value.get()) {
                        others.insert(key.to_owned(), value);
                    }
                }
            }
        }
        if !others.is_empty() {
            let text = serde_json::Value::Object(others).to_string();
            test.arguments = (text.len() <= MAX_JSON_FIELD_BYTES).then(|| text.into());
        }
        self.tests.push(PendingTest {
            test,
            attached: boxed(&node.attached_node),
            depends_on: node.depends_on.nodes.iter().map(Str::boxed).collect(),
            model_text,
        });
    }

    fn semantic_model(&mut self, model: RawSemanticModel<'_>) {
        let symbols = &mut self.symbols;
        let mut part = |part: &RawPart<'_>| SemanticPart {
            name: part.name.boxed(),
            kind: part
                .agg
                .as_ref()
                .or(part.kind.as_ref())
                .map(|kind| symbols.intern(&kind.0)),
            expr: boxed(&part.expr),
            description: boxed(&part.description),
        };
        let semantic = SemanticModel {
            name: model.name.boxed(),
            description: model
                .description
                .as_ref()
                .map(Str::boxed)
                .unwrap_or_default(),
            entry: None,
            agg_time_dimension: model
                .defaults
                .as_ref()
                .and_then(|defaults| boxed(&defaults.agg_time_dimension)),
            entities: model.entities.iter().map(&mut part).collect(),
            dimensions: model.dimensions.iter().map(&mut part).collect(),
            measures: model.measures.iter().map(&mut part).collect(),
        };
        let parents = model.depends_on.nodes.iter().map(Str::boxed).collect();
        self.semantic_models.push((semantic, parents));
    }

    fn metric(&mut self, metric: RawMetric<'_>) {
        let small = |raw: Option<&RawValue>| {
            raw.map(RawValue::get)
                .filter(|text| *text != "null" && text.len() <= MAX_JSON_FIELD_BYTES)
                .map(Box::from)
        };
        self.metrics.push(Metric {
            name: metric.name.boxed(),
            label: boxed(&metric.label),
            description: metric
                .description
                .as_ref()
                .map(Str::boxed)
                .unwrap_or_default(),
            kind: metric
                .kind
                .as_ref()
                .map(|kind| self.symbols.intern(&kind.0)),
            type_params: small(metric.type_params),
            filter: small(metric.filter),
        });
    }

    fn exposure(&mut self, exposure: RawExposure<'_>) {
        let owner = exposure
            .owner
            .as_ref()
            .and_then(|owner| boxed(&owner.name).or_else(|| boxed(&owner.email)));
        let value = Exposure {
            name: exposure.name.boxed(),
            kind: exposure
                .kind
                .as_ref()
                .map(|kind| self.symbols.intern(&kind.0)),
            label: boxed(&exposure.label),
            description: exposure
                .description
                .as_ref()
                .map(Str::boxed)
                .unwrap_or_default(),
            owner,
            url: boxed(&exposure.url),
            parents: Vec::new(),
        };
        let parents = exposure.depends_on.nodes.iter().map(Str::boxed).collect();
        self.exposures.push((value, parents));
    }

    fn finish(mut self) -> Result<Index, ParseError> {
        if !self.saw_metadata {
            return Err(ParseError::Invalid(
                "it has no metadata. Select the manifest.json file that dbt writes.".into(),
            ));
        }
        self.entries
            .sort_unstable_by(|a, b| a.entry.unique_id.cmp(&b.entry.unique_id));
        self.entries
            .dedup_by(|a, b| a.entry.unique_id == b.entry.unique_id);
        let positions: HashMap<&str, u32> = self
            .entries
            .iter()
            .enumerate()
            .map(|(index, pending)| (&*pending.entry.unique_id, index as u32))
            .collect();
        let resolve = |ids: &[Box<str>]| -> Vec<u32> {
            let mut found: Vec<u32> = ids
                .iter()
                .filter_map(|id| positions.get(&**id).copied())
                .collect();
            found.sort_unstable();
            found.dedup();
            found
        };
        let parents: Vec<Vec<u32>> = self
            .entries
            .iter()
            .map(|pending| resolve(&pending.parents))
            .collect();
        let mut children = vec![Vec::new(); self.entries.len()];
        for (index, entry_parents) in parents.iter().enumerate() {
            for parent in entry_parents {
                children[*parent as usize].push(index as u32);
            }
        }
        let mut tests = Vec::with_capacity(self.tests.len());
        for pending in self.tests {
            let mut test = pending.test;
            let attached = pending
                .attached
                .as_deref()
                .and_then(|id| positions.get(id).copied())
                .or_else(|| {
                    // Tests of sources have no attached node: the tested
                    // entry is the dependency that the model argument names.
                    let candidates: Vec<u32> = resolve(&pending.depends_on);
                    match candidates.as_slice() {
                        [only] => Some(*only),
                        _ => candidates.into_iter().find(|&candidate| {
                            let entry = &self.entries[candidate as usize].entry;
                            pending
                                .model_text
                                .as_deref()
                                .is_some_and(|text| text.contains(&format!("'{}'", entry.name)))
                        }),
                    }
                });
            let Some(attached) = attached else {
                continue;
            };
            test.entry = attached;
            if test.to_text.is_some() {
                // The other dependency, or the tested entry itself when the
                // relationship refers to its own table.
                let targets = resolve(&pending.depends_on);
                test.to = targets
                    .iter()
                    .copied()
                    .find(|&candidate| candidate != attached)
                    .or_else(|| targets.contains(&attached).then_some(attached));
            }
            tests.push(test);
        }
        tests.sort_by_key(|test| test.entry);
        let semantic_models = self
            .semantic_models
            .into_iter()
            .map(|(mut model, parents)| {
                model.entry = resolve(&parents).first().copied();
                model
            })
            .collect();
        let exposures = self
            .exposures
            .into_iter()
            .map(|(mut exposure, parents)| {
                exposure.parents = resolve(&parents);
                exposure
            })
            .collect();
        drop(positions);
        let entries = self
            .entries
            .into_iter()
            .zip(parents)
            .map(|(pending, parents)| Entry {
                parents,
                ..pending.entry
            })
            .collect();
        Ok(Index {
            project: self.project.unwrap_or_default(),
            dbt_version: self.dbt_version,
            generated_at: self.generated_at,
            adapter: self.adapter,
            has_compiled_code: self.has_compiled_code,
            symbols: self.symbols.list,
            entries,
            tests,
            children,
            semantic_models,
            metrics: self.metrics,
            exposures,
        })
    }
}

struct ManifestSeed<'b> {
    builder: &'b mut Builder,
    base: usize,
}

impl<'de> DeserializeSeed<'de> for ManifestSeed<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for ManifestSeed<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a dbt manifest object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let builder = self.builder;
        while let Some(key) = map.next_key::<Str<'de>>()? {
            match key.0.as_ref() {
                "metadata" => {
                    let metadata: Metadata<'de> = map.next_value()?;
                    builder.metadata(metadata).map_err(de::Error::custom)?;
                }
                "nodes" | "sources" => map.next_value_seed(Each {
                    builder: &mut *builder,
                    base: self.base,
                    resource: std::marker::PhantomData,
                    visit: |builder: &mut Builder, node: RawNode<'de>, base| {
                        builder.node(node, base)
                    },
                })?,
                "semantic_models" => map.next_value_seed(Each {
                    builder: &mut *builder,
                    base: self.base,
                    resource: std::marker::PhantomData,
                    visit: |builder: &mut Builder, model: RawSemanticModel<'de>, _| {
                        builder.semantic_model(model)
                    },
                })?,
                "metrics" => map.next_value_seed(Each {
                    builder: &mut *builder,
                    base: self.base,
                    resource: std::marker::PhantomData,
                    visit: |builder: &mut Builder, metric: RawMetric<'de>, _| {
                        builder.metric(metric)
                    },
                })?,
                "exposures" => map.next_value_seed(Each {
                    builder: &mut *builder,
                    base: self.base,
                    resource: std::marker::PhantomData,
                    visit: |builder: &mut Builder, exposure: RawExposure<'de>, _| {
                        builder.exposure(exposure)
                    },
                })?,
                // Macros, docs, the parent and child maps, disabled nodes,
                // and other parts that the index does not use.
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

/// Visits the values of an object of resources one at a time, so that only
/// one resource is in memory at a time.
struct Each<'b, F, T> {
    builder: &'b mut Builder,
    base: usize,
    visit: F,
    resource: std::marker::PhantomData<fn() -> T>,
}

impl<'de, T, F> DeserializeSeed<'de> for Each<'_, F, T>
where
    T: Deserialize<'de>,
    F: FnMut(&mut Builder, T, usize),
{
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de, T, F> Visitor<'de> for Each<'_, F, T>
where
    T: Deserialize<'de>,
    F: FnMut(&mut Builder, T, usize),
{
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an object of dbt resources")
    }

    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        let Self {
            builder,
            base,
            mut visit,
            ..
        } = self;
        while let Some(value) = seq.next_element::<T>()? {
            visit(builder, value, base);
        }
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let Self {
            builder,
            base,
            mut visit,
            ..
        } = self;
        while let Some((IgnoredAny, value)) = map.next_entry::<IgnoredAny, T>()? {
            visit(builder, value, base);
        }
        Ok(())
    }
}
