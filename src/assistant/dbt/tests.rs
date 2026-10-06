use super::*;
use crate::{
    dbt::worker::ManifestError,
    model::{DbtRefresh, SchemaRule, SchemaRuleKind},
};

const V12: &str = "https://schemas.getdbt.com/dbt/manifest/v12.json";

fn index(value: Value) -> Index {
    crate::dbt::parse(&serde_json::to_vec(&value).unwrap()).unwrap()
}

/// Orders in dev_core, built from a raw source, with tests that refer to
/// customers, a model that orders feeds.
fn lake(description: &str) -> Index {
    index(json!({
        "metadata": {"dbt_schema_version": V12, "dbt_version": "1.12.5",
            "generated_at": "2026-10-01T08:00:00Z", "project_name": "lake"},
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
                "description": "One row per customer.\nMore text.",
                "config": {"materialized": "table"},
                "depends_on": {"nodes": ["model.lake.orders"]},
            },
            "model.lake.staged": {
                "unique_id": "model.lake.staged", "resource_type": "model", "name": "staged",
                "schema": "dev_core", "alias": "staged", "relation_name": null,
                "config": {"materialized": "ephemeral"},
                "depends_on": {"nodes": ["model.lake.customers"]},
            },
            "test.lake.unique_orders_id": {
                "unique_id": "test.lake.unique_orders_id", "resource_type": "test",
                "attached_node": "model.lake.orders", "column_name": "id",
                "test_metadata": {"name": "unique", "kwargs": {"column_name": "id"}},
            },
            "test.lake.relationships_orders_customer_id": {
                "unique_id": "test.lake.relationships_orders_customer_id", "resource_type": "test",
                "attached_node": "model.lake.orders", "column_name": "customer_id",
                "depends_on": {"nodes": ["model.lake.customers", "model.lake.orders"]},
                "test_metadata": {"name": "relationships", "kwargs": {
                    "column_name": "customer_id", "to": "ref('customers')", "field": "id"}},
            },
            "test.lake.not_null_orders_amount": {
                "unique_id": "test.lake.not_null_orders_amount", "resource_type": "test",
                "attached_node": "model.lake.orders", "column_name": "amount",
                "test_metadata": {"name": "not_null", "kwargs": {"column_name": "amount"}},
            },
            "test.lake.recency_orders": {
                "unique_id": "test.lake.recency_orders", "resource_type": "test",
                "attached_node": "model.lake.orders",
                "test_metadata": {"name": "recency", "kwargs": {"datepart": "day"}},
            },
        },
        "sources": {
            "source.lake.raw.orders": {
                "unique_id": "source.lake.raw.orders", "resource_type": "source", "name": "orders",
                "schema": "raw", "identifier": "orders", "source_name": "raw",
                "description": "Orders from the shop.",
            },
        },
    }))
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

#[test]
fn a_model_is_found_by_its_id_its_table_or_its_name() {
    let (index, project) = (lake("Orders."), project());
    let dbt = Project::new(&index, &project, None, false);
    let orders = index.find("model.lake.orders").unwrap();
    assert_eq!(dbt.resolve("model.lake.orders"), Ok(orders));
    assert_eq!(dbt.resolve("CORE.Orders"), Ok(orders));
    assert_eq!(
        dbt.resolve("customers"),
        Ok(index.find("model.lake.customers").unwrap())
    );
    // The model and the source have the same name.
    let ambiguous = dbt.resolve("orders").unwrap_err();
    assert_eq!(ambiguous.code, "ambiguous_model");
    assert!(ambiguous.message.contains("source.lake.raw.orders"));
    assert_eq!(dbt.resolve("missing").unwrap_err().code, "model_not_found");
}

#[test]
fn a_model_is_light_by_default_and_gives_columns_on_request() {
    let (index, project) = (lake("One row for each order."), project());
    let dbt = Project::new(&index, &project, None, false);
    let orders = dbt.resolve("core.orders").unwrap();
    let value = dbt.describe(orders, &[], 0);
    assert_eq!(value["relation"], "core.orders");
    assert_eq!(value["materialized"], "table");
    assert_eq!(value["description"], "One row for each order.");
    assert_eq!(value["tags"], json!(["finance"]));
    // amount has only a test, so it counts as a column.
    assert_eq!(value["column_count"], 4);
    assert_eq!(
        value["documented_columns"],
        json!(["id", "customer_id", "amount"])
    );
    assert_eq!(value["test_count"], 4);
    assert_eq!(
        value["model_tests"],
        json!([{"test": "recency", "arguments": {"datepart": "day"}}])
    );
    assert_eq!(value["parents"], json!(["raw.orders"]));
    assert_eq!(value["children"], json!(["core.customers"]));
    assert_eq!(
        (value["parent_count"].clone(), value["child_count"].clone()),
        (json!(1), json!(1))
    );
    assert!(value.get("columns").is_none());

    let value = dbt.describe(orders, &["*ID".into(), "amount".into()], 0);
    assert_eq!(value["matched_column_count"], 3);
    assert_eq!(value["next_column_offset"], Value::Null);
    assert_eq!(
        value["columns"],
        json!([
            {"name": "id", "description": "The key.", "description_truncated": false,
             "data_type": null, "test_count": 1, "tests": [{"test": "unique"}], "tests_truncated": false},
            {"name": "customer_id", "description": "", "description_truncated": false,
             "data_type": "bigint", "test_count": 1,
             "tests": [{"test": "relationships", "to": "core.customers", "field": "id"}], "tests_truncated": false},
            {"name": "amount", "description": "", "description_truncated": false,
             "data_type": null, "test_count": 1, "tests": [{"test": "not_null"}], "tests_truncated": false},
        ])
    );

    // An ephemeral model has no table, so lists name it by its unique ID.
    let customers = dbt.resolve("core.customers").unwrap();
    let value = dbt.describe(customers, &[], 0);
    assert_eq!(value["children"], json!(["model.lake.staged"]));
    let staged = dbt.resolve("model.lake.staged").unwrap();
    assert_eq!(dbt.describe(staged, &[], 0)["relation"], Value::Null);
}

/// A model like a central fact table: many columns with long descriptions,
/// and many children.
fn wide() -> Index {
    let mut nodes = serde_json::Map::new();
    let columns: serde_json::Map<String, Value> = (0..141)
        .map(|number| {
            let name = format!("column_{number:03}");
            let value =
                json!({"name": name, "description": format!("{number} {}", "Text. ".repeat(200))});
            (name, value)
        })
        .collect();
    nodes.insert(
        "model.lake.clicks".into(),
        json!({
            "unique_id": "model.lake.clicks", "resource_type": "model", "name": "clicks",
            "schema": "core", "alias": "clicks", "relation_name": "`core`.`clicks`",
            "description": "Clicks.", "columns": columns,
        }),
    );
    for number in 0..152 {
        let id = format!("model.lake.child_{number:03}");
        nodes.insert(
            id.clone(),
            json!({
                "unique_id": id, "resource_type": "model", "name": format!("child_{number:03}"),
                "schema": "core", "alias": format!("child_{number:03}"),
                "relation_name": format!("`core`.`child_{number:03}`"),
                "description": format!("Child {number}."),
                "depends_on": {"nodes": [if number < 2 { "model.lake.clicks".to_owned() } else { format!("model.lake.child_{:03}", number % 2) }]},
            }),
        );
    }
    index(json!({"metadata": {"dbt_schema_version": V12}, "nodes": nodes}))
}

#[test]
fn the_columns_of_a_wide_model_come_in_pages_without_loss() {
    let index = wide();
    let project = DbtProject {
        schema_mapping: vec![],
        ..project()
    };
    let dbt = Project::new(&index, &project, None, false);
    let clicks = dbt.resolve("core.clicks").unwrap();
    let mut names = Vec::new();
    let mut offset = 0;
    let mut pages = 0;
    loop {
        let value = dbt.describe(clicks, &["*".into()], offset);
        assert!(size(&value) + ENVELOPE_BYTES <= MAX_TOOL_OUTPUT_BYTES);
        assert_eq!(value["matched_column_count"], 141);
        for column in value["columns"].as_array().unwrap() {
            assert_eq!(column["description_truncated"], false);
            names.push(column["name"].as_str().unwrap().to_owned());
        }
        pages += 1;
        match value["next_column_offset"].as_u64() {
            Some(next) => offset = next as usize,
            None => break,
        }
    }
    assert!(pages > 1);
    let expected: Vec<String> = (0..141).map(|n| format!("column_{n:03}")).collect();
    assert_eq!(names, expected);
}

#[test]
fn lineage_follows_the_dependencies_in_pages() {
    let index = wide();
    let project = DbtProject {
        schema_mapping: vec![],
        ..project()
    };
    let dbt = Project::new(&index, &project, None, false);
    let clicks = dbt.resolve("model.lake.clicks").unwrap();
    // Two direct children, and the others below them.
    let value = dbt.describe(clicks, &[], 0);
    assert_eq!(value["child_count"], 2);
    let direct = dbt.lineage(clicks, Direction::Downstream, 1, false, 0, 50);
    assert_eq!(direct["resource_count"], 2);
    let all = dbt.lineage(clicks, Direction::Downstream, MAX_DEPTH, true, 0, 100);
    assert_eq!(all["resource_count"], 152);
    assert_eq!(all["next_offset"], 100);
    assert_eq!(all["resources"][0]["depth"], 1);
    assert_eq!(all["resources"][0]["description"], "Child 0.");
    let rest = dbt.lineage(clicks, Direction::Downstream, MAX_DEPTH, false, 100, 100);
    assert_eq!(rest["resources"].as_array().unwrap().len(), 52);
    assert_eq!(rest["next_offset"], Value::Null);
    assert_eq!(rest["resources"][0]["depth"], 2);
    assert!(rest["resources"][0].get("description").is_none());

    let child = dbt.resolve("model.lake.child_004").unwrap();
    let up = dbt.lineage(child, Direction::Upstream, MAX_DEPTH, false, 0, 10);
    let ids: Vec<&str> = up["resources"]
        .as_array()
        .unwrap()
        .iter()
        .map(|resource| resource["unique_id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["model.lake.child_000", "model.lake.clicks"]);
    assert_eq!(up["resources"][0]["direction"], "upstream");

    // A model with more direct children than a description lists.
    let many = DbtProject {
        schema_mapping: vec![],
        ..project
    };
    let mut nodes = serde_json::Map::new();
    nodes.insert("model.lake.hub".into(), json!({"unique_id": "model.lake.hub", "resource_type": "model", "name": "hub", "schema": "core", "relation_name": "`core`.`hub`"}));
    for number in 0..152 {
        let id = format!("model.lake.leaf_{number:03}");
        nodes.insert(id.clone(), json!({"unique_id": id, "resource_type": "model", "name": format!("leaf_{number:03}"), "schema": "core", "relation_name": "x", "depends_on": {"nodes": ["model.lake.hub"]}}));
    }
    let index =
        super::tests::index(json!({"metadata": {"dbt_schema_version": V12}, "nodes": nodes}));
    let dbt = Project::new(&index, &many, None, false);
    let value = dbt.describe(dbt.resolve("model.lake.hub").unwrap(), &[], 0);
    assert_eq!(value["child_count"], 152);
    assert_eq!(value["children"].as_array().unwrap().len(), MAX_DIRECT);
    assert_eq!(value["children_truncated"], true);
}

#[test]
fn search_filters_by_pattern_text_tag_and_type() {
    let (index, project) = (lake("Orders of the shop."), project());
    let dbt = Project::new(&index, &project, None, false);
    let ids = |value: Value| -> Vec<String> {
        value["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["unique_id"].as_str().unwrap().to_owned())
            .collect()
    };
    let search = |filters: Search| dbt.search(&filters, 0, 50);
    let patterns = ["*ORDERS".to_owned()];
    assert_eq!(
        ids(search(Search {
            patterns: &patterns,
            ..Search::default()
        })),
        ["model.lake.orders", "source.lake.raw.orders"]
    );
    let tables = ["core.cust*".to_owned()];
    assert_eq!(
        ids(search(Search {
            patterns: &tables,
            ..Search::default()
        })),
        ["model.lake.customers"]
    );
    assert_eq!(
        ids(search(Search {
            text: Some("SHOP"),
            ..Search::default()
        })),
        ["model.lake.orders", "source.lake.raw.orders"]
    );
    assert_eq!(
        ids(search(Search {
            tag: Some("finance"),
            ..Search::default()
        })),
        ["model.lake.orders"]
    );
    assert_eq!(
        ids(search(Search {
            resource_type: Some(Kind::Source),
            ..Search::default()
        })),
        ["source.lake.raw.orders"]
    );
    let all = dbt.search(&Search::default(), 0, 2);
    assert_eq!(all["model_count"], 4);
    assert_eq!(all["next_offset"], 2);
    // A list gives the first line of a description.
    let customers = dbt.search(
        &Search {
            patterns: &tables,
            ..Search::default()
        },
        0,
        50,
    );
    assert_eq!(
        customers["models"][0]["description"],
        "One row per customer."
    );
    assert_eq!(customers["models"][0]["description_truncated"], true);
}

#[test]
fn the_context_names_the_models_of_the_tab_sql() {
    let (index, project) = (lake("Orders.\nMore."), project());
    let dbt = Project::new(&index, &project, None, true);
    let state = ManifestState::default();
    let found = context(
        uuid::Uuid::nil(),
        &state,
        Some(&dbt),
        "SELECT * FROM core.orders o JOIN customers c ON 1 = 1 JOIN raw.unknown u",
        "core",
    );
    assert_eq!(found.models, Some(3));
    assert_eq!(found.sources, Some(1));
    assert_eq!(found.tests, Some(4));
    assert!(found.manifest_changed);
    assert_eq!(
        found.referenced_models,
        [
            ReferencedModel {
                relation: "core.orders".into(),
                unique_id: "model.lake.orders".into(),
                description: "Orders.".into(),
            },
            ReferencedModel {
                relation: "core.customers".into(),
                unique_id: "model.lake.customers".into(),
                description: "One row per customer.".into(),
            },
        ]
    );

    // A manifest that Qrow reads for the first time, or that failed.
    let reading = ManifestState {
        parsing: true,
        ..ManifestState::default()
    };
    let value = context(uuid::Uuid::nil(), &reading, None, "", "core");
    assert!(value.reading && value.models.is_none());
    let failed = ManifestState {
        error: Some(ManifestError::NotFound),
        ..ManifestState::default()
    };
    let value =
        serde_json::to_value(context(uuid::Uuid::nil(), &failed, Some(&dbt), "", "core")).unwrap();
    assert_eq!(
        value["error"],
        "Manifest not found: run dbt parse in the project"
    );
    assert_eq!(value["models"], 3);
}

#[test]
fn every_part_of_a_model_fits_in_a_tool_result() {
    // Many model tests and one column with many large tests.
    let mut nodes = serde_json::Map::new();
    nodes.insert(
        "model.lake.big".into(),
        json!({
            "unique_id": "model.lake.big", "resource_type": "model", "name": "big",
            "schema": "core", "relation_name": "`core`.`big`",
            "description": "x\"".repeat(50_000),
            "columns": {"status": {"name": "status", "description": "y\n".repeat(50_000)}},
        }),
    );
    for number in 0..400 {
        let id = format!("test.lake.model_test_{number}");
        nodes.insert(
            id.clone(),
            json!({
                "unique_id": id, "resource_type": "test", "attached_node": "model.lake.big",
                "test_metadata": {"name": format!("custom_model_check_{number}"), "kwargs": {}},
            }),
        );
        let id = format!("test.lake.status_{number}");
        let values: Vec<String> = (0..50)
            .map(|v| format!("status_value_{number}_{v}"))
            .collect();
        nodes.insert(
            id.clone(),
            json!({
                "unique_id": id, "resource_type": "test", "attached_node": "model.lake.big",
                "column_name": "status",
                "test_metadata": {"name": "accepted_values", "kwargs": {"values": values}},
            }),
        );
    }
    let index = index(json!({"metadata": {"dbt_schema_version": V12}, "nodes": nodes}));
    let project = DbtProject {
        schema_mapping: vec![],
        ..project()
    };
    let dbt = Project::new(&index, &project, None, false);
    let big = dbt.resolve("core.big").unwrap();
    let light = dbt.describe(big, &[], 0);
    assert!(size(&light) + ENVELOPE_BYTES <= MAX_TOOL_OUTPUT_BYTES);
    assert_eq!(light["description_truncated"], true);
    assert_eq!(light["model_test_count"], 400);
    assert_eq!(light["model_tests_truncated"], true);
    let columns = dbt.describe(big, &["status".into()], 0);
    assert!(
        size(&columns) + ENVELOPE_BYTES <= MAX_TOOL_OUTPUT_BYTES,
        "{}",
        size(&columns)
    );
    let status = &columns["columns"][0];
    assert_eq!(status["test_count"], 400);
    assert_eq!(status["tests_truncated"], true);
}

#[test]
fn the_relation_lookup_agrees_with_one_lookup() {
    let (index, project) = (lake("Orders."), project());
    let relations = matching::relations(&index, &project);
    for entry in index.entries() {
        let Some(relation) = Project::new(&index, &project, None, false).relation(entry) else {
            continue;
        };
        let (schema, name) = relation.split_once('.').unwrap();
        let key = (schema.to_lowercase(), name.to_lowercase());
        assert_eq!(
            relations.get(&key).copied(),
            matching::entry_for(&index, &project, schema, name),
            "{relation}"
        );
    }
}

#[test]
fn a_cut_counts_the_escapes_of_json() {
    let text = "a\n".repeat(1_000);
    let (cut, truncated) = cut_flag(&text, 100);
    assert!(truncated);
    assert!(serde_json::to_string(&cut).unwrap().len() - 2 <= 100);
    assert_eq!(
        cut_flag("short \"text\"", 100),
        ("short \"text\"".into(), false)
    );
    let (cut, _) = cut_flag(&"é".repeat(100), 11);
    assert_eq!(cut, "éééé…");

    // A list tells when escapes made a description too long.
    let (index, project) = (lake(&"\"".repeat(150)), project());
    let dbt = Project::new(&index, &project, None, false);
    let listed = dbt.listed(index.find("model.lake.orders").unwrap(), true);
    assert_eq!(listed["description_truncated"], true);
}

#[test]
fn model_sql_is_read_from_the_manifest_in_pages() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("manifest.json");
    let raw = "select * from {{ ref('orders') }} -- заказы";
    let value = json!({
        "metadata": {"dbt_schema_version": V12},
        "nodes": {
            "model.lake.daily": {
                "unique_id": "model.lake.daily", "resource_type": "model", "name": "daily",
                "schema": "core", "relation_name": "`core`.`daily`",
                "raw_code": raw, "compiled_code": "select * from `core`.`orders` -- заказы",
            },
            "model.lake.parsed": {
                "unique_id": "model.lake.parsed", "resource_type": "model", "name": "parsed",
                "schema": "core", "relation_name": "`core`.`parsed`", "raw_code": "select 1",
            },
        },
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    let index = crate::dbt::parse(&std::fs::read(&path).unwrap()).unwrap();
    let project = DbtProject {
        schema_mapping: vec![],
        ..project()
    };
    let dbt = Project::new(&index, &project, None, false);
    let daily = dbt.resolve("core.daily").unwrap();
    let compiled = dbt.read_sql(daily, &path, true, 0, MAX_SQL_BYTES).unwrap();
    assert_eq!(compiled["code"], "compiled");
    assert_eq!(compiled["sql"], "select * from `core`.`orders` -- заказы");
    assert_eq!(compiled["next_offset"], Value::Null);
    let first = dbt.read_sql(daily, &path, false, 0, 40).unwrap();
    assert_eq!(first["code"], "raw");
    // The page ends before a character that does not fit.
    let end = first["next_offset"].as_u64().unwrap() as usize;
    assert!(end <= 40 && raw.is_char_boundary(end));
    let rest = dbt.read_sql(daily, &path, false, end, 100).unwrap();
    assert_eq!(
        format!(
            "{}{}",
            first["sql"].as_str().unwrap(),
            rest["sql"].as_str().unwrap()
        ),
        raw
    );
    assert_eq!(rest["sql_bytes"], raw.len());

    // A manifest from dbt parse has only the raw SQL.
    let parsed = dbt.resolve("core.parsed").unwrap();
    let value = dbt.read_sql(parsed, &path, true, 0, 100).unwrap();
    assert_eq!(
        (value["code"].clone(), value["compiled_missing"].clone()),
        (json!("raw"), json!(true))
    );
    assert_eq!(value["sql"], "select 1");
}
