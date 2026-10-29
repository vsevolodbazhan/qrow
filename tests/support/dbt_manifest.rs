//! Synthetic dbt manifests for tests and benchmarks. They have the shape of
//! the `manifest.json` (schema v12) that dbt-core 1.8 to 1.12 and dbt
//! Fusion 2.0 write for a Spark project, with made-up names and SQL. No
//! manifest of a real project is a fixture.
//!
//! The mix of bytes copies a real Spark project of 88 MB: models with their
//! columns and SQL are half of the file, test nodes a quarter, and docs
//! blocks most of the rest. Macros are a small part, because a manifest
//! keeps only the macros of the project and its packages.
#![allow(dead_code)]

use serde_json::{Map, Value, json};
use std::fmt::Write as _;

/// What a synthetic manifest contains.
#[derive(Clone, Debug)]
pub struct Shape {
    pub models: usize,
    pub sources: usize,
    pub macros: usize,
    pub columns: usize,
    /// A manifest from `dbt compile` or `dbt build` has `compiled_code`. A
    /// manifest from `dbt parse` has not.
    pub compiled: bool,
    /// The shape of dbt Fusion: no `parent_map` or `child_map`, no legacy
    /// semantic models, and fields that dbt-core does not write.
    pub fusion: bool,
}

impl Shape {
    /// A compiled dbt-core manifest of about `megabytes` MB.
    pub fn sized(megabytes: usize) -> Self {
        // About MODEL_BYTES for each model with its tests and docs, and
        // 9 KB for each macro, measured on the output.
        let bytes = megabytes * 1_000_000;
        Self {
            models: (bytes * 98 / 100 / MODEL_BYTES).max(4),
            sources: (bytes * 98 / 100 / MODEL_BYTES / 7).max(1),
            macros: (bytes * 2 / 100 / 9_000).max(4),
            columns: 18,
            compiled: true,
            fusion: false,
        }
    }

    pub fn parsed(mut self) -> Self {
        self.compiled = false;
        self
    }

    pub fn fusion(mut self) -> Self {
        self.fusion = true;
        self.compiled = false;
        self
    }
}

pub const PROJECT: &str = "synthetic_lake";
const MODEL_BYTES: usize = 50_000;
const SCHEMAS: [&str; 6] = ["staging", "core", "finance", "marketing", "ops", "product"];
const TYPES: [&str; 6] = [
    "bigint",
    "string",
    "decimal(18,2)",
    "timestamp",
    "date",
    "boolean",
];
const TAGS: [&str; 5] = ["daily", "hourly", "pii", "finance", "deprecated"];

/// A deterministic random number generator, so each run makes the same file.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, limit: usize) -> usize {
        (self.next() % limit.max(1) as u64) as usize
    }
}

/// The unique ID of model `index`.
pub fn model_id(index: usize) -> String {
    format!("model.{PROJECT}.{}", model_name(index))
}

pub fn model_name(index: usize) -> String {
    let layer = if index.is_multiple_of(3) {
        "stg"
    } else {
        "fct"
    };
    format!("{layer}_entity_{index:05}")
}

/// The schema that model `index` builds in, as dbt writes it.
pub fn model_schema(index: usize) -> String {
    format!("analytics_{}", SCHEMAS[index % SCHEMAS.len()])
}

/// The alias of model `index`: some models build a table with another name.
pub fn model_alias(index: usize) -> String {
    if index % 7 == 3 {
        format!("{}_v2", model_name(index))
    } else {
        model_name(index)
    }
}

pub fn source_id(index: usize) -> String {
    format!("source.{PROJECT}.raw_{}.table_{index:05}", index % 4)
}

fn materialization(index: usize) -> &'static str {
    match index % 20 {
        0 => "ephemeral",
        1..=6 => "view",
        7..=10 => "incremental",
        _ => "table",
    }
}

fn sentence(random: &mut Random, words: usize) -> String {
    const WORDS: [&str; 16] = [
        "the",
        "order",
        "customer",
        "amount",
        "in",
        "EUR",
        "per",
        "day",
        "\"net\"",
        "booking",
        "status",
        "line",
        "of",
        "joins",
        "from",
        "warehouse",
    ];
    let mut text = String::new();
    for index in 0..words {
        if index > 0 {
            text.push(' ');
        }
        text.push_str(WORDS[random.below(WORDS.len())]);
    }
    text.push('.');
    text
}

fn sql(random: &mut Random, bytes: usize, jinja: bool) -> String {
    let mut text = String::from("with source as (\n");
    while text.len() < bytes {
        let column = random.below(400);
        if jinja && random.below(4) == 0 {
            let _ = writeln!(
                text,
                "    {{{{ cents('amount_{column}') }}}} as cents_{column},"
            );
        } else {
            let _ = writeln!(
                text,
                "    cast(c.col_{column} as decimal(18, 2)) as amount_{column}, -- \"note\"\t{column}"
            );
        }
    }
    text.push_str("    1 as last\nfrom source)\nselect * from source\n");
    text
}

fn checksum(random: &mut Random) -> Value {
    json!({"name": "sha256", "checksum": format!("{:016x}{:016x}{:016x}{:016x}", random.next(), random.next(), random.next(), random.next())})
}

fn column(random: &mut Random, name: &str, shape: &Shape) -> Value {
    let words = 8 + random.below(40);
    let mut value = json!({
        "name": name,
        "description": sentence(random, words),
        "meta": {},
        "constraints": [],
        "tags": [],
        "config": {"meta": {}, "tags": []},
    });
    let declared = random.below(3) == 0;
    if shape.fusion {
        if declared {
            value["data_type"] = json!(TYPES[random.below(TYPES.len())]);
        }
    } else {
        value["data_type"] = if declared {
            json!(TYPES[random.below(TYPES.len())])
        } else {
            Value::Null
        };
        value["quote"] = Value::Null;
        value["granularity"] = Value::Null;
        value["dimension"] = Value::Null;
        value["entity"] = Value::Null;
        value["doc_blocks"] = json!([]);
    }
    value
}

fn node_config(materialized: &str, tags: &[&str], schema: Option<&str>) -> Value {
    json!({
        "enabled": true, "alias": null, "schema": schema, "database": null,
        "tags": tags, "meta": {}, "group": null, "materialized": materialized,
        "incremental_strategy": null, "persist_docs": {}, "post-hook": [], "pre-hook": [],
        "quoting": {}, "column_types": {}, "full_refresh": null, "unique_key": null,
        "on_schema_change": "ignore", "on_configuration_change": "apply", "grants": {},
        "packages": [], "docs": {"show": true, "node_color": null},
        "contract": {"enforced": false, "alias_types": true}, "access": "protected",
    })
}

struct Writer {
    out: Vec<u8>,
    first: bool,
}

impl Writer {
    fn open(&mut self, key: &str) {
        self.out.extend_from_slice(b",\n");
        serde_json::to_writer(&mut self.out, key).unwrap();
        self.out.extend_from_slice(b": {");
        self.first = true;
    }

    fn entry(&mut self, key: &str, value: &Value) {
        if !self.first {
            self.out.push(b',');
        }
        self.first = false;
        self.out.push(b'\n');
        serde_json::to_writer(&mut self.out, key).unwrap();
        self.out.push(b':');
        serde_json::to_writer(&mut self.out, value).unwrap();
    }

    fn close(&mut self) {
        self.out.extend_from_slice(b"\n}");
    }
}

/// Make a synthetic manifest. The output is valid JSON.
pub fn generate(shape: &Shape) -> Vec<u8> {
    let mut random = Random(0x5eed_dbf0);
    let mut writer = Writer {
        out: Vec::with_capacity(shape.models * 50_000 + shape.macros * 10_000),
        first: true,
    };
    let version = if shape.fusion { "2.0.5" } else { "1.12.5" };
    writer.out.extend_from_slice(b"{\"metadata\": ");
    serde_json::to_writer(
        &mut writer.out,
        &json!({
            "dbt_schema_version": "https://schemas.getdbt.com/dbt/manifest/v12.json",
            "dbt_version": version,
            "generated_at": "2026-10-05T05:39:31.858737Z",
            "invocation_id": "00000000-0000-4000-8000-000000000000",
            "env": {},
            "project_name": PROJECT,
            "project_id": "00000000000000000000000000000000",
            "user_id": null,
            "send_anonymous_usage_stats": false,
            "adapter_type": "spark",
        }),
    )
    .unwrap();

    let mut parents: Vec<Vec<String>> = Vec::new();
    writer.open("nodes");
    for index in 0..shape.models {
        let id = model_id(index);
        let name = model_name(index);
        let materialized = materialization(index);
        let schema = model_schema(index);
        let alias = model_alias(index);
        let mut depends: Vec<String> = Vec::new();
        if index % 3 == 0 {
            depends.push(source_id(index / 3 % shape.sources.max(1)));
        } else {
            for _ in 0..1 + random.below(4) {
                let parent = model_id(random.below(index));
                if !depends.contains(&parent) {
                    depends.push(parent);
                }
            }
        }
        let tags: Vec<&str> = (0..random.below(3))
            .map(|_| TAGS[random.below(TAGS.len())])
            .collect();
        let mut columns = Map::new();
        for number in 0..shape.columns {
            let column_name = if number == 0 {
                "id".to_owned()
            } else {
                format!("col_{number:03}")
            };
            columns.insert(
                column_name.clone(),
                column(&mut random, &column_name, shape),
            );
        }
        let raw_bytes = 3_000 + random.below(9_000);
        let raw = sql(&mut random, raw_bytes, true);
        let description_words = 10 + random.below(60);
        let relation = (materialized != "ephemeral").then(|| format!("`{schema}`.`{alias}`"));
        let mut node = json!({
            "database": null,
            "schema": schema,
            "name": name,
            "resource_type": "model",
            "package_name": PROJECT,
            "path": format!("{}/{name}.sql", SCHEMAS[index % SCHEMAS.len()]),
            "original_file_path": format!("models/{}/{name}.sql", SCHEMAS[index % SCHEMAS.len()]),
            "unique_id": id,
            "fqn": [PROJECT, SCHEMAS[index % SCHEMAS.len()], name],
            "alias": alias,
            "checksum": checksum(&mut random),
            "config": node_config(materialized, &tags, Some(SCHEMAS[index % SCHEMAS.len()])),
            "tags": tags,
            "description": format!("{} It has \"quotes\"\nand lines.", sentence(&mut random, description_words)),
            "columns": columns,
            "meta": {},
            "patch_path": format!("{PROJECT}://models/{}/schema.yml", SCHEMAS[index % SCHEMAS.len()]),
            "unrendered_config": {"materialized": materialized},
            "relation_name": relation,
            "raw_code": raw,
            "language": "sql",
            "refs": depends.iter().filter(|d| d.starts_with("model.")).map(|d| json!({"name": d.rsplit('.').next(), "package": null, "version": null})).collect::<Vec<_>>(),
            "sources": [],
            "metrics": [],
            "depends_on": {"macros": [format!("macro.{PROJECT}.cents")], "nodes": depends},
            "contract": {"enforced": false, "alias_types": true, "checksum": null},
            "access": "protected",
            "constraints": [],
            "version": null,
            "latest_version": null,
            "primary_key": ["id"],
        });
        if shape.fusion {
            node["classifiers"] = json!({"layer": "synthetic"});
            node["depends_on"]["nodes_with_ref_location"] = json!([]);
        } else {
            node["created_at"] = json!(1_791_178_772.731);
            node["build_path"] = Value::Null;
            node["doc_blocks"] = json!([]);
            node["deprecation_date"] = Value::Null;
            node["compiled_path"] = json!(format!(
                "target/compiled/{PROJECT}/models/{}/{name}.sql",
                SCHEMAS[index % SCHEMAS.len()]
            ));
        }
        if shape.compiled {
            node["compiled"] = json!(true);
            node["compiled_code"] = json!(sql(&mut random, raw.len() * 2, false));
            node["extra_ctes_injected"] = json!(true);
            node["extra_ctes"] = json!([]);
        }
        writer.entry(&id, &node);
        parents.push(
            node["depends_on"]["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect(),
        );
        write_tests(&mut writer, &mut random, shape, index, &id, &mut parents);
    }
    for index in 0..seeds(shape) {
        let id = format!("seed.{PROJECT}.seed_{index:04}");
        writer.entry(
            &id,
            &json!({
                "database": null, "schema": "analytics", "name": format!("seed_{index:04}"),
                "resource_type": "seed", "package_name": PROJECT, "path": format!("seed_{index:04}.csv"),
                "original_file_path": format!("seeds/seed_{index:04}.csv"), "unique_id": id,
                "fqn": [PROJECT, format!("seed_{index:04}")], "alias": format!("seed_{index:04}"),
                "checksum": checksum(&mut random), "config": node_config("seed", &[], None),
                "tags": [], "description": "", "columns": {}, "meta": {},
                "relation_name": format!("`analytics`.`seed_{index:04}`"), "raw_code": "",
                "depends_on": {"macros": []},
            }),
        );
        parents.push(Vec::new());
    }
    for index in 0..snapshots(shape) {
        let id = format!("snapshot.{PROJECT}.snapshot_{index:04}");
        let source = source_id(index % shape.sources.max(1));
        writer.entry(
            &id,
            &json!({
                "database": null, "schema": "snapshots", "name": format!("snapshot_{index:04}"),
                "resource_type": "snapshot", "package_name": PROJECT,
                "path": format!("snapshot_{index:04}.yml"),
                "original_file_path": format!("snapshots/snapshot_{index:04}.yml"), "unique_id": id,
                "fqn": [PROJECT, format!("snapshot_{index:04}")], "alias": format!("snapshot_{index:04}"),
                "checksum": checksum(&mut random), "config": node_config("snapshot", &[], Some("snapshots")),
                "tags": [], "description": sentence(&mut random, 8), "columns": {}, "meta": {},
                "relation_name": format!("`snapshots`.`snapshot_{index:04}`"),
                "raw_code": "", "depends_on": {"macros": [], "nodes": [source]},
            }),
        );
        parents.push(vec![source]);
    }
    // Tests of sources have no attached node. Their model argument names
    // the source.
    for index in source_tests(shape) {
        let source = source_id(index);
        let schema = format!("raw_{}", index % 4);
        let table = format!("table_{index:05}");
        let name = format!("source_not_null_{schema}_{table}_col_000");
        let id = format!("test.{PROJECT}.{name}.00000");
        let mut node = json!({
            "database": null, "schema": "analytics_dbt_test__audit", "name": name,
            "resource_type": "test", "package_name": PROJECT, "path": format!("{name}.sql"),
            "original_file_path": "models/staging/sources.yml", "unique_id": id,
            "fqn": [PROJECT, "staging", name], "alias": name,
            "checksum": {"name": "none", "checksum": ""},
            "config": {"enabled": true, "materialized": "test", "severity": "ERROR"},
            "tags": [], "description": "", "columns": {}, "meta": {}, "relation_name": null,
            "raw_code": "{{ test_not_null(**_dbt_generic_test_kwargs) }}",
            "depends_on": {"macros": ["macro.dbt.test_not_null"], "nodes": [source]},
            "column_name": "col_000", "attached_node": null,
            "test_metadata": {"name": "not_null", "kwargs": {
                "column_name": "col_000",
                "model": format!("{{{{ get_where_subquery(source('{schema}', '{table}')) }}}}"),
            }},
        });
        if !shape.fusion {
            node["test_metadata"]["namespace"] = Value::Null;
        }
        writer.entry(&id, &node);
        parents.push(vec![source]);
    }
    // A singular test has no metadata. The index skips it.
    let singular = format!("test.{PROJECT}.assert_totals_balance");
    writer.entry(
        &singular,
        &json!({
            "database": null, "schema": "analytics_dbt_test__audit", "name": "assert_totals_balance",
            "resource_type": "test", "package_name": PROJECT, "path": "assert_totals_balance.sql",
            "original_file_path": "tests/assert_totals_balance.sql", "unique_id": singular,
            "config": {"enabled": true, "materialized": "test"}, "tags": [], "description": "",
            "raw_code": "select 1 where false", "depends_on": {"macros": [], "nodes": [model_id(0)]},
        }),
    );
    parents.push(vec![model_id(0)]);
    writer.close();

    writer.open("sources");
    for index in 0..shape.sources {
        let id = source_id(index);
        let mut columns = Map::new();
        for number in 0..shape.columns / 2 {
            let column_name = format!("col_{number:03}");
            columns.insert(
                column_name.clone(),
                column(&mut random, &column_name, shape),
            );
        }
        writer.entry(
            &id,
            &json!({
                "database": null, "schema": format!("raw_{}", index % 4),
                "name": format!("table_{index:05}"), "resource_type": "source",
                "package_name": PROJECT, "path": "models/staging/sources.yml",
                "original_file_path": "models/staging/sources.yml", "unique_id": id,
                "fqn": [PROJECT, "staging", format!("raw_{}", index % 4), format!("table_{index:05}")],
                "source_name": format!("raw_{}", index % 4),
                "source_description": "Raw data loaded by the synthetic loader.",
                "loader": "synthetic_loader",
                "identifier": format!("table_{index:05}_src"),
                "quoting": {"database": null, "schema": null, "identifier": null, "column": null},
                "loaded_at_field": null,
                "freshness": {"warn_after": {"count": null, "period": null}, "error_after": {"count": null, "period": null}, "filter": null},
                "external": null, "description": sentence(&mut random, 12), "columns": columns,
                "meta": {}, "source_meta": {}, "tags": [], "config": {"enabled": true, "meta": {}, "tags": []},
                "patch_path": null, "unrendered_config": {},
                "relation_name": format!("`raw_{}`.`table_{index:05}_src`", index % 4),
                "created_at": 1_791_178_772.817,
            }),
        );
    }
    writer.close();

    writer.open("macros");
    for index in 0..shape.macros {
        let package = ["dbt", "dbt_utils", "spark_utils", PROJECT][index % 4];
        let id = format!("macro.{package}.macro_{index:05}");
        let macro_bytes = 6_000 + random.below(6_000);
        writer.entry(
            &id,
            &json!({
                "name": format!("macro_{index:05}"), "resource_type": "macro", "package_name": package,
                "path": format!("macros/macro_{index:05}.sql"),
                "original_file_path": format!("macros/macro_{index:05}.sql"), "unique_id": id,
                "macro_sql": format!("{{% macro macro_{index:05}(relation) %}}\n{}{{% endmacro %}}", sql(&mut random, macro_bytes, true)),
                "depends_on": {"macros": []}, "description": "", "meta": {},
                "docs": {"show": true, "node_color": null}, "patch_path": null,
                "arguments": [], "created_at": 1_791_178_771.93, "supported_languages": null,
            }),
        );
    }
    writer.close();

    writer.open("docs");
    for index in 0..shape.models * 6 + 1 {
        let id = format!("doc.{PROJECT}.doc_{index:05}");
        writer.entry(
            &id,
            &json!({
                "name": format!("doc_{index:05}"), "resource_type": "doc", "package_name": PROJECT,
                "path": "docs.md", "original_file_path": "models/docs.md", "unique_id": id,
                "block_contents": sentence(&mut random, 120),
            }),
        );
    }
    writer.close();

    writer.open("exposures");
    for index in 0..shape.models / 50 + 1 {
        let id = format!("exposure.{PROJECT}.dashboard_{index:04}");
        writer.entry(
            &id,
            &json!({
                "name": format!("dashboard_{index:04}"), "resource_type": "exposure",
                "package_name": PROJECT, "path": "exposures.yml",
                "original_file_path": "models/exposures.yml", "unique_id": id,
                "fqn": [PROJECT, format!("dashboard_{index:04}")], "type": "dashboard",
                "owner": {"email": "analyst@example.com", "name": "Synthetic Analyst"},
                "description": sentence(&mut random, 10), "label": format!("Dashboard {index}"),
                "maturity": "high", "meta": {}, "tags": [], "config": {"enabled": true},
                "unrendered_config": {}, "url": format!("https://bi.example.com/{index}"),
                "depends_on": {"macros": [], "nodes": [model_id(random.below(shape.models))]},
                "refs": [], "sources": [], "metrics": [], "created_at": 1_791_178_772.9,
            }),
        );
    }
    writer.close();

    writer.open("metrics");
    if !shape.fusion {
        for index in 0..shape.models / 15 + 1 {
            let id = format!("metric.{PROJECT}.metric_{index:04}");
            writer.entry(
                &id,
                &json!({
                    "name": format!("metric_{index:04}"), "resource_type": "metric",
                    "package_name": PROJECT, "path": "metrics.yml",
                    "original_file_path": "models/metrics.yml", "unique_id": id,
                    "fqn": [PROJECT, format!("metric_{index:04}")],
                    "description": sentence(&mut random, 10), "label": format!("Metric {index}"),
                    "type": "simple",
                    "type_params": {"measure": {"name": format!("measure_{index:04}"), "filter": null, "alias": null, "join_to_timespine": false, "fill_nulls_with": null}, "input_measures": [{"name": format!("measure_{index:04}"), "filter": null}], "metrics": []},
                    "filter": null, "metadata": null, "time_granularity": null, "meta": {}, "tags": [],
                    "config": {"enabled": true, "group": null, "meta": {}},
                    "depends_on": {"macros": [], "nodes": [format!("semantic_model.{PROJECT}.semantic_{:04}", index / 2)]},
                    "refs": [], "metrics": [], "created_at": 1_791_178_772.9, "group": null,
                }),
            );
        }
    }
    writer.close();

    for key in ["groups", "selectors"] {
        writer.open(key);
        writer.close();
    }

    writer.open("disabled");
    for index in 0..shape.models / 50 + 1 {
        let id = format!("model.{PROJECT}.disabled_{index:04}");
        if !writer.first {
            writer.out.push(b',');
        }
        writer.first = false;
        writer.out.push(b'\n');
        serde_json::to_writer(&mut writer.out, &id).unwrap();
        writer.out.push(b':');
        serde_json::to_writer(
            &mut writer.out,
            &json!([{
                "database": null, "schema": "analytics", "name": format!("disabled_{index:04}"),
                "resource_type": "model", "package_name": PROJECT, "unique_id": id,
                "original_file_path": format!("models/disabled_{index:04}.sql"),
                "alias": format!("disabled_{index:04}"), "config": {"enabled": false},
                "raw_code": sql(&mut random, 800, true), "depends_on": {"macros": [], "nodes": []},
            }]),
        )
        .unwrap();
    }
    writer.close();

    if !shape.fusion {
        let mut children: std::collections::BTreeMap<&str, Vec<&str>> = Default::default();
        writer.open("parent_map");
        let ids = node_ids(shape);
        for (id, node_parents) in ids.iter().zip(&parents) {
            writer.entry(id, &json!(node_parents));
            for parent in node_parents {
                children.entry(parent).or_default().push(id);
            }
        }
        writer.close();
        writer.open("child_map");
        for id in &ids {
            writer.entry(
                id,
                &json!(children.get(id.as_str()).cloned().unwrap_or_default()),
            );
        }
        writer.close();
    }
    writer.open("group_map");
    writer.close();
    writer.open("saved_queries");
    writer.close();

    writer.open("semantic_models");
    if !shape.fusion {
        for index in 0..shape.models / 30 + 1 {
            let model = 1 + 3 * index % shape.models.max(2);
            let id = format!("semantic_model.{PROJECT}.semantic_{index:04}");
            writer.entry(
                &id,
                &json!({
                    "name": format!("semantic_{index:04}"), "resource_type": "semantic_model",
                    "package_name": PROJECT, "path": "semantic.yml",
                    "original_file_path": "models/semantic.yml", "unique_id": id,
                    "fqn": [PROJECT, format!("semantic_{index:04}")],
                    "model": format!("ref('{}')", model_name(model)),
                    "node_relation": {"alias": model_alias(model), "schema_name": model_schema(model), "database": null, "relation_name": format!("`{}`.`{}`", model_schema(model), model_alias(model))},
                    "description": sentence(&mut random, 10), "label": null,
                    "defaults": {"agg_time_dimension": "ordered_at"},
                    "entities": [{"name": "entity", "type": "primary", "description": null, "label": null, "role": null, "expr": "id", "config": {"meta": {}}}],
                    "measures": [
                        {"name": format!("measure_{:04}", index * 2), "agg": "sum", "description": "Sum of amounts.", "label": null, "create_metric": false, "expr": "col_001", "agg_params": null, "non_additive_dimension": null, "agg_time_dimension": null, "config": {"meta": {}}},
                        {"name": format!("measure_{:04}", index * 2 + 1), "agg": "count_distinct", "description": null, "label": null, "create_metric": false, "expr": "id", "agg_params": null, "non_additive_dimension": null, "agg_time_dimension": null, "config": {"meta": {}}},
                    ],
                    "dimensions": [
                        {"name": "ordered_at", "type": "time", "description": null, "label": null, "is_partition": false, "type_params": {"time_granularity": "day", "validity_params": null}, "expr": "col_002", "metadata": null, "config": {"meta": {}}},
                        {"name": "status", "type": "categorical", "description": "Order status.", "label": null, "is_partition": false, "type_params": null, "expr": null, "metadata": null, "config": {"meta": {}}},
                    ],
                    "metadata": null, "depends_on": {"macros": [], "nodes": [model_id(model)]},
                    "refs": [{"name": model_name(model), "package": null, "version": null}],
                    "created_at": 1_791_178_772.9, "config": {"enabled": true, "group": null, "meta": {}},
                    "unrendered_config": {}, "primary_entity": null, "group": null,
                }),
            );
        }
    }
    writer.close();
    for key in ["unit_tests", "functions"] {
        writer.open(key);
        writer.close();
    }
    writer.out.extend_from_slice(b"\n}\n");
    writer.out
}

/// The IDs of the nodes in the order of `generate`, with their tests.
fn node_ids(shape: &Shape) -> Vec<String> {
    let mut ids = Vec::new();
    for index in 0..shape.models {
        ids.push(model_id(index));
        for test in test_names(index) {
            ids.push(format!(
                "test.{PROJECT}.{test}_{}.0000{index:05}",
                model_name(index)
            ));
        }
    }
    for index in 0..shape.models / 20 + 1 {
        ids.push(format!("seed.{PROJECT}.seed_{index:04}"));
    }
    for index in 0..shape.models / 40 + 1 {
        ids.push(format!("snapshot.{PROJECT}.snapshot_{index:04}"));
    }
    for index in source_tests(shape) {
        ids.push(format!(
            "test.{PROJECT}.source_not_null_raw_{}_table_{index:05}_col_000.00000",
            index % 4
        ));
    }
    ids.push(format!("test.{PROJECT}.assert_totals_balance"));
    ids
}

/// The sources with a `not_null` test.
pub fn source_tests(shape: &Shape) -> impl Iterator<Item = usize> {
    (0..shape.sources).step_by(3)
}

/// The number of seeds.
pub fn seeds(shape: &Shape) -> usize {
    shape.models / 20 + 1
}

/// The number of snapshots.
pub fn snapshots(shape: &Shape) -> usize {
    shape.models / 40 + 1
}

/// The generic tests that the index keeps.
pub fn generic_tests(shape: &Shape) -> usize {
    (0..shape.models)
        .map(|index| test_names(index).len())
        .sum::<usize>()
        + source_tests(shape).count()
}

/// The generic tests of model `index`.
pub fn test_names(index: usize) -> Vec<&'static str> {
    let mut names = vec!["unique", "not_null"];
    if index % 2 == 1 {
        names.push("accepted_values");
    }
    if index % 3 == 1 && index > 0 {
        names.push("relationships");
    }
    if index % 5 == 2 {
        names.push("positive_values");
    }
    if index % 4 != 3 {
        names.push("not_empty_string");
    }
    names
}

fn write_tests(
    writer: &mut Writer,
    random: &mut Random,
    shape: &Shape,
    index: usize,
    model: &str,
    parents: &mut Vec<Vec<String>>,
) {
    let name = model_name(index);
    for test in test_names(index) {
        let id = format!("test.{PROJECT}.{test}_{name}.0000{index:05}");
        let column = match test {
            "accepted_values" => "col_001",
            "relationships" => "col_002",
            "positive_values" => "col_003",
            "not_empty_string" => "col_004",
            _ => "id",
        };
        let mut kwargs = json!({
            "column_name": column,
            "model": format!("{{{{ get_where_subquery(ref('{name}')) }}}}"),
        });
        let mut depends = vec![model.to_owned()];
        match test {
            "accepted_values" => kwargs["values"] = json!(["placed", "shipped", "returned"]),
            "not_empty_string" => kwargs["trim_whitespaces"] = json!(true),
            "relationships" => {
                let target = random.below(index);
                kwargs["to"] = json!(format!("ref('{}')", model_name(target)));
                kwargs["field"] = json!("id");
                depends.insert(0, model_id(target));
            }
            _ => {}
        }
        let mut node = json!({
            "database": null, "schema": "analytics_dbt_test__audit", "name": format!("{test}_{name}"),
            "resource_type": "test", "package_name": PROJECT,
            "path": format!("{test}_{name}.sql"), "original_file_path": "models/schema.yml",
            "unique_id": id, "fqn": [PROJECT, format!("{test}_{name}")], "alias": format!("{test}_{name}"),
            "checksum": {"name": "none", "checksum": ""},
            "config": {"enabled": true, "alias": null, "schema": "dbt_test__audit", "database": null, "tags": [], "meta": {}, "group": null, "materialized": "test", "severity": "ERROR", "store_failures": null, "where": null, "limit": null, "fail_calc": "count(*)", "warn_if": "!= 0", "error_if": "!= 0"},
            "tags": [], "description": "", "columns": {}, "meta": {}, "patch_path": null,
            "unrendered_config": {}, "relation_name": null,
            "raw_code": format!("{{{{ test_{test}(**_dbt_generic_test_kwargs) }}}}"),
            "language": "sql", "refs": [], "sources": [], "metrics": [],
            "depends_on": {"macros": [format!("macro.dbt.test_{test}")], "nodes": depends.clone()},
            "contract": {"enforced": false, "alias_types": true, "checksum": null},
            "column_name": column, "file_key_name": format!("models.{name}"),
            "attached_node": model,
            "test_metadata": {"name": test, "kwargs": kwargs},
        });
        if !shape.fusion {
            node["test_metadata"]["namespace"] = Value::Null;
            node["created_at"] = json!(1_791_178_772.68);
            node["compiled_path"] = json!(format!(
                "target/compiled/{PROJECT}/models/schema.yml/{test}_{name}.sql"
            ));
        }
        if shape.compiled {
            node["compiled"] = json!(true);
            node["compiled_code"] = json!(sql(random, 600, false));
            node["extra_ctes_injected"] = json!(true);
            node["extra_ctes"] = json!([]);
        }
        writer.entry(&id, &node);
        parents.push(depends);
    }
}
