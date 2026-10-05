use super::saved::{self, FORMAT_VERSION, LoadError, Saved, Stamp};
use super::*;
use serde_json::{Value, json};

fn metadata(version: &str) -> Value {
    json!({
        "dbt_schema_version": format!("https://schemas.getdbt.com/dbt/manifest/{version}.json"),
        "dbt_version": "1.12.5",
        "generated_at": "2026-10-01T08:00:00.000000Z",
        "project_name": "tiny_lake",
        "adapter_type": "spark",
        "invocation_id": "00000000-0000-0000-0000-000000000000",
    })
}

/// A small manifest with one resource of each kind that the index reads.
fn manifest() -> Value {
    json!({
        "metadata": metadata("v12"),
        "nodes": {
            "model.tiny_lake.orders": {
                "unique_id": "model.tiny_lake.orders", "resource_type": "model",
                "name": "orders", "package_name": "tiny_lake", "database": null,
                "schema": "core", "alias": "fct_orders",
                "relation_name": "`core`.`fct_orders`",
                "description": "One row for each order.\nAmounts are in \"cents\".",
                "tags": ["daily", "finance"],
                "config": {"materialized": "incremental", "enabled": true},
                "columns": {
                    "order_id": {"name": "order_id", "description": "The key.", "data_type": "bigint"},
                    "status": {"name": "status", "description": "", "data_type": null},
                },
                "depends_on": {"macros": ["macro.dbt.is_incremental"], "nodes": [
                    "source.tiny_lake.raw.orders", "model.tiny_lake.customers",
                    "source.tiny_lake.raw.orders",
                ]},
                "original_file_path": "models/core/orders.sql",
                "compiled_path": "target/compiled/tiny_lake/models/core/orders.sql",
                "raw_code": "select *\nfrom {{ source('raw', 'orders') }} -- \"日本\"",
                "compiled_code": "select *\nfrom `raw`.`orders_v2`",
                "unknown_field": {"nested": [1, 2, 3]},
            },
            "model.tiny_lake.customers": {
                "unique_id": "model.tiny_lake.customers", "resource_type": "model",
                "name": "customers", "package_name": "tiny_lake", "database": null,
                "schema": "core", "alias": "customers", "relation_name": null,
                "description": "Customers and their countries.", "tags": "pii",
                "config": {"materialized": "ephemeral"},
                "columns": {}, "depends_on": {"nodes": []},
                "original_file_path": "models/core/customers.sql",
                "raw_code": "", "compiled_code": null,
            },
            "seed.tiny_lake.countries": {
                "unique_id": "seed.tiny_lake.countries", "resource_type": "seed",
                "name": "countries", "package_name": "tiny_lake", "database": null,
                "schema": "core", "alias": "countries",
                "relation_name": "`core`.`countries`", "description": "ISO countries.",
                "config": {"materialized": "seed"}, "original_file_path": "seeds/countries.csv",
            },
            "model.tiny_lake.disabled": {
                "unique_id": "model.tiny_lake.disabled", "resource_type": "model",
                "name": "disabled", "schema": "core", "config": {"enabled": false},
            },
            "analysis.tiny_lake.notes": {
                "unique_id": "analysis.tiny_lake.notes", "resource_type": "analysis",
                "name": "notes",
            },
            "test.tiny_lake.unique_orders_order_id.1": {
                "unique_id": "test.tiny_lake.unique_orders_order_id.1", "resource_type": "test",
                "attached_node": "model.tiny_lake.orders", "column_name": "order_id",
                "depends_on": {"nodes": ["model.tiny_lake.orders"]},
                "test_metadata": {"name": "unique", "namespace": null,
                    "kwargs": {"column_name": "order_id", "model": "{{ get_where_subquery(ref('orders')) }}"}},
            },
            "test.tiny_lake.accepted_values_orders_status.2": {
                "unique_id": "test.tiny_lake.accepted_values_orders_status.2", "resource_type": "test",
                "attached_node": "model.tiny_lake.orders", "column_name": "status",
                "depends_on": {"nodes": ["model.tiny_lake.orders"]},
                "test_metadata": {"name": "accepted_values",
                    "kwargs": {"column_name": "status", "values": ["placed", "shipped", 3],
                        "quote": false, "model": "{{ get_where_subquery(ref('orders')) }}"}},
            },
            "test.tiny_lake.relationships_orders_customer_id.3": {
                "unique_id": "test.tiny_lake.relationships_orders_customer_id.3", "resource_type": "test",
                "attached_node": "model.tiny_lake.orders", "column_name": "customer_id",
                "depends_on": {"nodes": ["model.tiny_lake.customers", "model.tiny_lake.orders"]},
                "test_metadata": {"name": "relationships",
                    "kwargs": {"column_name": "customer_id", "to": "ref('customers')", "field": "id",
                        "model": "{{ get_where_subquery(ref('orders')) }}"}},
            },
            "test.tiny_lake.source_not_null_raw_orders_id.4": {
                "unique_id": "test.tiny_lake.source_not_null_raw_orders_id.4", "resource_type": "test",
                "attached_node": null, "column_name": "id",
                "depends_on": {"nodes": ["source.tiny_lake.raw.orders"]},
                "test_metadata": {"name": "not_null",
                    "kwargs": {"column_name": "id", "model": "{{ get_where_subquery(source('raw', 'orders')) }}"}},
            },
            "test.tiny_lake.disabled_test.5": {
                "unique_id": "test.tiny_lake.disabled_test.5", "resource_type": "test",
                "attached_node": "model.tiny_lake.orders", "config": {"enabled": false},
                "test_metadata": {"name": "unique", "kwargs": {}},
            },
            "test.tiny_lake.assert_positive": {
                "unique_id": "test.tiny_lake.assert_positive", "resource_type": "test",
                "depends_on": {"nodes": ["model.tiny_lake.orders"]},
                "raw_code": "select 1 where false",
            },
        },
        "sources": {
            "source.tiny_lake.raw.orders": {
                "unique_id": "source.tiny_lake.raw.orders", "resource_type": "source",
                "name": "orders", "package_name": "tiny_lake", "database": null,
                "schema": "raw", "identifier": "orders_v2", "source_name": "raw",
                "loader": "fivetran", "relation_name": "`raw`.`orders_v2`",
                "description": "Orders from the shop.", "columns": {
                    "id": {"name": "id", "description": "Order key."},
                },
                "original_file_path": "models/sources.yml",
            },
        },
        "macros": {"macro.dbt.is_incremental": {"macro_sql": "{% macro is_incremental() %}{% endmacro %}"}},
        "docs": {"doc.tiny_lake.overview": {"block_contents": "Docs."}},
        "exposures": {
            "exposure.tiny_lake.revenue": {
                "name": "revenue", "type": "dashboard", "label": "Revenue",
                "description": "Revenue by day.", "owner": {"email": "analyst@example.com"},
                "url": "https://bi.example.com/revenue",
                "depends_on": {"nodes": ["model.tiny_lake.orders"]},
            },
        },
        "metrics": {
            "metric.tiny_lake.revenue": {
                "name": "revenue", "label": "Revenue", "description": "Sum of amounts.",
                "type": "simple", "type_params": {"measure": {"name": "amount"}}, "filter": null,
            },
        },
        "semantic_models": {
            "semantic_model.tiny_lake.orders": {
                "name": "orders", "description": "Orders.",
                "defaults": {"agg_time_dimension": "ordered_at"},
                "entities": [{"name": "order", "type": "primary", "expr": "order_id"}],
                "dimensions": [{"name": "ordered_at", "type": "time", "expr": null}],
                "measures": [{"name": "amount", "agg": "sum", "expr": "amount_cents", "description": "Amount."}],
                "depends_on": {"nodes": ["model.tiny_lake.orders"]},
            },
        },
        "parent_map": {}, "child_map": {}, "disabled": {}, "unit_tests": {},
    })
}

fn parse_value(value: &Value) -> Index {
    parse(&serde_json::to_vec(value).unwrap()).unwrap()
}

fn entry<'a>(index: &'a Index, unique_id: &str) -> (u32, &'a Entry) {
    let position = index.find(unique_id).unwrap();
    (position, index.entry(position))
}

#[test]
fn the_index_keeps_the_resources_that_are_relations_and_their_meaning() {
    let index = parse_value(&manifest());
    assert_eq!(&*index.project, "tiny_lake");
    assert_eq!(&*index.dbt_version, "1.12.5");
    assert_eq!(&*index.adapter, "spark");
    assert!(index.has_compiled_code);
    // The disabled model, the analysis, and the tests are not entries.
    let ids: Vec<&str> = index.entries().iter().map(|e| &*e.unique_id).collect();
    assert_eq!(
        ids,
        [
            "model.tiny_lake.customers",
            "model.tiny_lake.orders",
            "seed.tiny_lake.countries",
            "source.tiny_lake.raw.orders",
        ]
    );

    let (orders_at, orders) = entry(&index, "model.tiny_lake.orders");
    assert_eq!(orders.kind, Kind::Model);
    assert_eq!(orders.database, None);
    assert_eq!(index.symbol(orders.schema), "core");
    assert_eq!(&*orders.identifier, "fct_orders");
    assert_eq!(orders.relation_name.as_deref(), Some("`core`.`fct_orders`"));
    assert_eq!(index.symbol(orders.materialized.unwrap()), "incremental");
    assert_eq!(
        &*orders.description,
        "One row for each order.\nAmounts are in \"cents\"."
    );
    let tags: Vec<&str> = orders.tags.iter().map(|t| index.symbol(*t)).collect();
    assert_eq!(tags, ["daily", "finance"]);
    assert_eq!(orders.columns.len(), 2);
    assert_eq!(index.symbol(orders.columns[0].data_type.unwrap()), "bigint");
    assert_eq!(orders.columns[1].data_type, None);
    assert_eq!(&*orders.path, "models/core/orders.sql");

    // Parents come from depends_on without repeats, and children from the
    // parents.
    let (customers_at, customers) = entry(&index, "model.tiny_lake.customers");
    let (source_at, source) = entry(&index, "source.tiny_lake.raw.orders");
    assert_eq!(orders.parents, [customers_at, source_at]);
    assert_eq!(index.children(source_at), [orders_at]);
    assert_eq!(index.children(customers_at), [orders_at]);
    assert!(index.children(orders_at).is_empty());

    // An ephemeral model has no relation, and a single tag is a list.
    assert_eq!(customers.relation_name, None);
    assert_eq!(customers.raw_code, None);
    let tags: Vec<&str> = customers.tags.iter().map(|t| index.symbol(*t)).collect();
    assert_eq!(tags, ["pii"]);

    assert_eq!(source.kind, Kind::Source);
    assert_eq!(&*source.identifier, "orders_v2");
    assert_eq!(index.symbol(source.source_name.unwrap()), "raw");
    assert_eq!(index.symbol(source.loader.unwrap()), "fivetran");
    assert_eq!(source.materialized, None);
}

#[test]
fn generic_tests_attach_to_their_entry_and_singular_tests_are_skipped() {
    let index = parse_value(&manifest());
    assert_eq!(index.test_count(), 4);
    let (orders_at, _) = entry(&index, "model.tiny_lake.orders");
    let (customers_at, _) = entry(&index, "model.tiny_lake.customers");
    let tests = index.tests(orders_at);
    let names: Vec<&str> = tests.iter().map(|t| index.symbol(t.name)).collect();
    assert_eq!(names, ["unique", "accepted_values", "relationships"]);
    assert_eq!(tests[0].column.as_deref(), Some("order_id"));
    assert_eq!(tests[0].arguments, None);
    let values: Vec<&str> = tests[1].values.iter().map(|v| &**v).collect();
    assert_eq!(values, ["placed", "shipped", "3"]);
    assert_eq!(tests[1].arguments.as_deref(), Some(r#"{"quote":false}"#));
    assert_eq!(tests[2].to, Some(customers_at));
    assert_eq!(tests[2].to_text.as_deref(), Some("ref('customers')"));
    assert_eq!(tests[2].field.as_deref(), Some("id"));

    // A test of a source has no attached node.
    let (source_at, _) = entry(&index, "source.tiny_lake.raw.orders");
    let tests = index.tests(source_at);
    assert_eq!(tests.len(), 1);
    assert_eq!(index.symbol(tests[0].name), "not_null");
    assert!(index.tests(customers_at).is_empty());
}

#[test]
fn a_relationship_targets_the_entry_that_its_to_argument_names() {
    let relationship = "test.tiny_lake.relationships_orders_customer_id.3";
    // A relationship to its own table.
    let mut value = manifest();
    let node = &mut value["nodes"][relationship];
    node["depends_on"]["nodes"] = json!(["model.tiny_lake.orders"]);
    node["test_metadata"]["kwargs"]["to"] = json!("ref('orders')");
    let index = parse_value(&value);
    let (orders_at, _) = entry(&index, "model.tiny_lake.orders");
    assert_eq!(index.tests(orders_at)[2].to, Some(orders_at));

    // A literal relation is not an entry of the project.
    let mut value = manifest();
    let node = &mut value["nodes"][relationship];
    node["depends_on"]["nodes"] = json!(["model.tiny_lake.orders"]);
    node["test_metadata"]["kwargs"]["to"] = json!("raw.customers");
    let index = parse_value(&value);
    let test = &index.tests(orders_at)[2];
    assert_eq!(test.to, None);
    assert_eq!(test.to_text.as_deref(), Some("raw.customers"));
}

#[test]
fn a_source_test_attaches_to_its_source_when_a_model_has_the_same_name() {
    // A relationship from source('raw', 'orders') to ref('orders').
    let mut value = manifest();
    let mut orders = value["nodes"]["model.tiny_lake.orders"].clone();
    orders["unique_id"] = json!("model.tiny_lake.a_orders");
    value["nodes"]["model.tiny_lake.a_orders"] = orders;
    value["nodes"]["model.tiny_lake.orders"]["name"] = json!("orders_v1");
    value["nodes"]["model.tiny_lake.a_orders"]["name"] = json!("orders");
    value["nodes"]["test.tiny_lake.source_relationships.6"] = json!({
        "unique_id": "test.tiny_lake.source_relationships.6", "resource_type": "test",
        "attached_node": null, "column_name": "id",
        "depends_on": {"nodes": ["model.tiny_lake.a_orders", "source.tiny_lake.raw.orders"]},
        "test_metadata": {"name": "relationships", "kwargs": {
            "column_name": "id", "to": "ref('orders')", "field": "order_id",
            "model": "{{ get_where_subquery(source('raw', 'orders')) }}"}},
    });
    let index = parse_value(&value);
    let (source_at, _) = entry(&index, "source.tiny_lake.raw.orders");
    let (model_at, _) = entry(&index, "model.tiny_lake.a_orders");
    let tests = index.tests(source_at);
    let names: Vec<&str> = tests.iter().map(|t| index.symbol(t.name)).collect();
    assert_eq!(names, ["not_null", "relationships"]);
    assert_eq!(tests[1].to, Some(model_at));
    assert!(index.tests(model_at).is_empty());
}

#[test]
fn a_source_test_with_several_dependencies_takes_the_source_its_model_names() {
    let mut value = manifest();
    value["sources"]["source.tiny_lake.raw.customers"] = json!({
        "unique_id": "source.tiny_lake.raw.customers", "resource_type": "source",
        "name": "customers", "schema": "raw", "source_name": "raw",
    });
    value["nodes"]["test.tiny_lake.source_not_null_raw_orders_id.4"]["depends_on"]["nodes"] =
        json!([
            "source.tiny_lake.raw.customers",
            "source.tiny_lake.raw.orders"
        ]);
    let index = parse_value(&value);
    let (orders_at, _) = entry(&index, "source.tiny_lake.raw.orders");
    let (customers_at, _) = entry(&index, "source.tiny_lake.raw.customers");
    assert_eq!(index.tests(orders_at).len(), 1);
    assert!(index.tests(customers_at).is_empty());
}

#[test]
fn semantic_models_metrics_and_exposures_refer_to_entries() {
    let index = parse_value(&manifest());
    let (orders_at, _) = entry(&index, "model.tiny_lake.orders");
    let [model] = index.semantic_models.as_slice() else {
        panic!("one semantic model");
    };
    assert_eq!(model.entry, Some(orders_at));
    assert_eq!(model.agg_time_dimension.as_deref(), Some("ordered_at"));
    assert_eq!(index.symbol(model.entities[0].kind.unwrap()), "primary");
    assert_eq!(index.symbol(model.measures[0].kind.unwrap()), "sum");
    assert_eq!(model.measures[0].expr.as_deref(), Some("amount_cents"));
    assert_eq!(model.dimensions[0].expr, None);

    let [metric] = index.metrics.as_slice() else {
        panic!("one metric");
    };
    assert_eq!(index.symbol(metric.kind.unwrap()), "simple");
    assert_eq!(
        metric.type_params.as_deref(),
        Some(r#"{"measure":{"name":"amount"}}"#)
    );
    assert_eq!(metric.filter, None);

    let [exposure] = index.exposures.as_slice() else {
        panic!("one exposure");
    };
    assert_eq!(exposure.parents, [orders_at]);
    assert_eq!(exposure.owner.as_deref(), Some("analyst@example.com"));
}

#[test]
fn large_test_arguments_and_metric_parameters_are_not_kept() {
    let mut value = manifest();
    let large = "x".repeat(5_000);
    value["nodes"]["test.tiny_lake.unique_orders_order_id.1"]["test_metadata"]["kwargs"]["where"] =
        json!(large);
    value["metrics"]["metric.tiny_lake.revenue"]["type_params"] = json!({"expr": large});
    let index = parse_value(&value);
    let (orders_at, _) = entry(&index, "model.tiny_lake.orders");
    assert_eq!(index.tests(orders_at)[0].arguments, None);
    assert_eq!(index.metrics[0].type_params, None);
}

#[test]
fn other_schema_versions_and_other_files_are_errors() {
    let mut value = manifest();
    value["metadata"] = metadata("v11");
    let error = parse(&serde_json::to_vec(&value).unwrap()).unwrap_err();
    assert_eq!(error, ParseError::UnsupportedVersion("v11".into()));
    assert_eq!(
        error.to_string(),
        "Unsupported manifest version v11: Qrow reads manifest v12 (dbt 1.8 and later)"
    );

    // The catalog of dbt docs is not a manifest.
    value["metadata"]["dbt_schema_version"] =
        json!("https://schemas.getdbt.com/dbt/catalog/v1.json");
    assert!(matches!(
        parse(&serde_json::to_vec(&value).unwrap()),
        Err(ParseError::UnsupportedVersion(_))
    ));

    assert!(matches!(
        parse(br#"{"nodes": {}}"#),
        Err(ParseError::Invalid(message)) if message.contains("no metadata")
    ));
    assert!(matches!(parse(b"[1, 2]"), Err(ParseError::Invalid(_))));
    // A file that dbt is still writing ends early.
    let bytes = serde_json::to_vec(&manifest()).unwrap();
    assert!(matches!(
        parse(&bytes[..bytes.len() / 2]),
        Err(ParseError::Invalid(_))
    ));
}

#[test]
fn a_span_reads_the_sql_from_the_manifest_file() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("manifest.json");
    // Indented output moves the strings in the file.
    std::fs::write(&path, serde_json::to_vec_pretty(&manifest()).unwrap()).unwrap();
    let index = parse(&std::fs::read(&path).unwrap()).unwrap();
    let (_, orders) = entry(&index, "model.tiny_lake.orders");
    assert_eq!(
        read_sql(&path, orders.raw_code.unwrap()).unwrap(),
        "select *\nfrom {{ source('raw', 'orders') }} -- \"日本\""
    );
    assert_eq!(
        read_sql(&path, orders.compiled_code.unwrap()).unwrap(),
        "select *\nfrom `raw`.`orders_v2`"
    );
}

#[test]
fn search_matches_names_before_descriptions() {
    let index = parse_value(&manifest());
    let names = |found: Vec<u32>| -> Vec<String> {
        found
            .into_iter()
            .map(|position| index.entry(position).unique_id.to_string())
            .collect()
    };
    assert_eq!(
        names(index.search("ORDERS", None, 10)),
        ["model.tiny_lake.orders", "source.tiny_lake.raw.orders"]
    );
    assert_eq!(
        names(index.search("orders", Some(Kind::Source), 10)),
        ["source.tiny_lake.raw.orders"]
    );
    // The alias matches, and so does a tag.
    assert_eq!(
        names(index.search("fct_", None, 10)),
        ["model.tiny_lake.orders"]
    );
    assert_eq!(
        names(index.search("pii", None, 10)),
        ["model.tiny_lake.customers"]
    );
    // The seed has the name, and the customers model has the word in its
    // description.
    assert_eq!(
        names(index.search("countries", None, 10)),
        ["seed.tiny_lake.countries", "model.tiny_lake.customers"]
    );
    assert_eq!(
        names(index.search("countries", None, 1)),
        ["seed.tiny_lake.countries"]
    );
    assert_eq!(index.search("orders", None, 1).len(), 1);
    assert!(index.search("no such text", None, 10).is_empty());
}

fn stamped(index: Index) -> Saved {
    Saved {
        stamp: Stamp {
            len: 42,
            modified: Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_791_000_000)),
        },
        index,
    }
}

#[test]
fn a_saved_index_loads_back() {
    let saved = stamped(parse_value(&manifest()));
    let bytes = saved::encode(&saved);
    assert_eq!(saved::decode(&bytes).unwrap(), saved);
}

#[test]
fn a_saved_index_of_another_version_or_with_damage_does_not_load() {
    let bytes = saved::encode(&stamped(parse_value(&manifest())));
    assert_eq!(saved::decode(b"{}"), Err(LoadError::NotSaved));
    assert_eq!(saved::decode(&bytes[..10]), Err(LoadError::NotSaved));

    let mut other = bytes.clone();
    other[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    assert_eq!(
        saved::decode(&other),
        Err(LoadError::OtherVersion(FORMAT_VERSION + 1))
    );

    let mut damaged = bytes.clone();
    let last = damaged.len() - 1;
    damaged[last] ^= 1;
    assert!(matches!(
        saved::decode(&damaged),
        Err(LoadError::Damaged(message)) if message.contains("digest")
    ));
    assert!(matches!(
        saved::decode(&bytes[..bytes.len() - 1]),
        Err(LoadError::Damaged(_))
    ));
}

#[test]
fn a_saved_index_with_positions_out_of_range_does_not_load() {
    let mut index = parse_value(&manifest());
    index.entries[0].parents.push(99);
    let bytes = saved::encode(&stamped(index));
    assert!(matches!(
        saved::decode(&bytes),
        Err(LoadError::Damaged(message)) if message.contains("out of range")
    ));

    let mut index = parse_value(&manifest());
    index.symbols.truncate(1);
    assert!(index.check().is_err());
    let mut index = parse_value(&manifest());
    index.entries.swap(0, 1);
    assert!(index.check().is_err());
}

/// A manifest as fixed bytes, so that the spans do not depend on the JSON
/// writer.
const FIXED: &str = r#"{"metadata": {"dbt_schema_version": "https://schemas.getdbt.com/dbt/manifest/v12.json", "dbt_version": "1.12.5", "generated_at": "2026-10-01T08:00:00Z", "project_name": "tiny_lake", "adapter_type": "spark"},
"nodes": {"model.tiny_lake.orders": {"unique_id": "model.tiny_lake.orders", "resource_type": "model", "name": "orders", "package_name": "tiny_lake", "database": null, "schema": "core", "alias": "fct_orders", "relation_name": "`core`.`fct_orders`", "description": "Orders.", "tags": ["daily"], "config": {"materialized": "table"}, "columns": {"id": {"name": "id", "description": "Key.", "data_type": "bigint"}}, "depends_on": {"nodes": ["source.tiny_lake.raw.orders"]}, "original_file_path": "models/orders.sql", "compiled_path": "target/compiled/orders.sql", "raw_code": "select * from {{ source('raw', 'orders') }}", "compiled_code": "select * from `raw`.`orders`"},
"test.tiny_lake.unique_orders_id.1": {"unique_id": "test.tiny_lake.unique_orders_id.1", "resource_type": "test", "attached_node": "model.tiny_lake.orders", "column_name": "id", "test_metadata": {"name": "unique", "kwargs": {"column_name": "id", "where": "id > 0"}}}},
"sources": {"source.tiny_lake.raw.orders": {"unique_id": "source.tiny_lake.raw.orders", "resource_type": "source", "name": "orders", "schema": "raw", "identifier": "orders", "source_name": "raw", "loader": "fivetran"}},
"semantic_models": {"semantic_model.tiny_lake.orders": {"name": "orders", "entities": [{"name": "order", "type": "primary"}], "depends_on": {"nodes": ["model.tiny_lake.orders"]}}},
"metrics": {"metric.tiny_lake.count": {"name": "count", "type": "simple", "type_params": {"measure": {"name": "count"}}}},
"exposures": {"exposure.tiny_lake.board": {"name": "board", "type": "dashboard", "owner": {"name": "Analyst"}, "depends_on": {"nodes": ["model.tiny_lake.orders"]}}}}"#;

/// The saved form of a fixed index. When this test fails, a change altered
/// the payload: increase `FORMAT_VERSION`, and then update the digest.
#[test]
fn the_saved_form_changes_only_with_its_format_version() {
    let bytes = saved::encode(&stamped(parse(FIXED.as_bytes()).unwrap()));
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    let hex: String = digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(FORMAT_VERSION, 1);
    assert_eq!(
        hex,
        "977c34b1668831e5a9f3ef1cf73f5fc5b6e97bc084f687ccde589da5b89c782b"
    );
}
