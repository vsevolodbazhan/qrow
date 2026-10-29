//! A dbt project matched with the tables that the real server reports.
use crate::support::fixture::{Kyuubi, QUERY_TIMEOUT};
use crate::support::{TestApp, connection_row, label, labelled};
use gpui_kit::TestAppContext;
use qrow::model::{CatalogRefresh, DbtProject, DbtRefresh, SchemaRule, SchemaRuleKind};

/// A manifest with two models that dbt builds in `dev_<schema>`.
fn manifest(schema: &str) -> String {
    let model = |name: &str| {
        format!(
            r#""model.lake.{name}": {{"unique_id": "model.lake.{name}", "resource_type": "model", "name": "{name}", "package_name": "lake", "database": null, "schema": "dev_{schema}", "alias": "{name}", "relation_name": "`dev_{schema}`.`{name}`", "config": {{"materialized": "table"}}, "original_file_path": "models/{name}.sql"}}"#
        )
    };
    format!(
        r#"{{"metadata": {{"dbt_schema_version": "https://schemas.getdbt.com/dbt/manifest/v12.json", "dbt_version": "1.12.5", "generated_at": "2026-10-01T08:00:00Z", "project_name": "lake", "adapter_type": "spark"}}, "nodes": {{{}, {}}}, "sources": {{}}}}"#,
        model("bookings"),
        model("missing")
    )
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn a_dbt_project_matches_the_tables_of_the_server(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let schema = format!("qrow_dbt_{}", uuid::Uuid::new_v4().simple());
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("manifest.json");
    std::fs::write(&path, manifest(&schema)).unwrap();
    let (mut workspace, credentials) =
        kyuubi.workspace("SELECT 1", crate::support::fixture::PASSWORD);
    let profile = &mut workspace.profiles[0];
    profile.catalog.include = vec![schema.clone()];
    profile.catalog.refresh = CatalogRefresh::Manual;
    profile.dbt = Some(DbtProject {
        manifest: path.to_string_lossy().into_owned(),
        refresh: DbtRefresh::Manual,
        schema_mapping: vec![SchemaRule {
            kind: SchemaRuleKind::Prefix,
            from: "dev_".into(),
            to: String::new(),
        }],
    });
    let profile = profile.clone();
    let app = TestApp::launch_with(cx, workspace, credentials);

    app.run_complete(cx, &format!("CREATE DATABASE {schema}"));
    app.run_complete(
        cx,
        &format!("CREATE TABLE {schema}.bookings (id BIGINT) USING parquet"),
    );
    app.toggle_connection(cx, profile.id);
    app.wait_until(cx, "the schema", QUERY_TIMEOUT, |window, _| {
        labelled(window, &schema).is_some()
    });
    app.click_labelled(cx, &schema);
    app.wait_until(cx, "the table", QUERY_TIMEOUT, |window, _| {
        labelled(window, "bookings").is_some()
    });

    app.context_menu(cx, connection_row(profile.id));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-name");
    app.connection_page(cx, "dbt");
    app.scroll_to(cx, "connection-dbt-manifest");
    app.wait_until(cx, "the manifest", QUERY_TIMEOUT, |window, _| {
        label(window, "connection-dbt-status").as_deref()
            == Some("Manifest from 2026-10-01 08:00 UTC, dbt 1.12.5, 2 models")
    });
    app.scroll_to(cx, "connection-dbt-matches");
    app.wait_until(cx, "the match summary", QUERY_TIMEOUT, |window, _| {
        label(window, "connection-dbt-matches").as_deref()
            == Some("1 of 2 dbt resources matched the catalog")
            && label(window, "connection-dbt-match-percent").as_deref() == Some("50%")
    });
    app.click(cx, "cancel-profile");
    app.wait_gone(cx, "save-profile");
    app.run_complete(cx, &format!("DROP DATABASE {schema} CASCADE"));
}

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn the_demo_manifest_matches_a_live_catalog_and_opens_model_sql(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let schema = format!("qrow_demo_dbt_{}", uuid::Uuid::new_v4().simple());
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("manifest.json");
    std::fs::write(&path, include_bytes!("../../assets/demo/manifest.json")).unwrap();
    let (mut workspace, credentials) =
        kyuubi.workspace("SELECT 1", crate::support::fixture::PASSWORD);
    let profile = &mut workspace.profiles[0];
    profile.catalog.include = vec![schema.clone()];
    profile.catalog.refresh = CatalogRefresh::Manual;
    profile.dbt = Some(DbtProject {
        manifest: path.to_string_lossy().into_owned(),
        refresh: DbtRefresh::Manual,
        schema_mapping: vec![SchemaRule {
            kind: SchemaRuleKind::Exact,
            from: "avia".into(),
            to: schema.clone(),
        }],
    });
    let id = profile.id;
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.run_complete(cx, &format!("CREATE DATABASE {schema}"));
    app.run_complete(cx, &format!(
        "CREATE TABLE {schema}.bookings (booking_id BIGINT, gate STRING, amount DECIMAL(12,2), booked_at TIMESTAMP) USING parquet"));
    app.toggle_connection(cx, id);
    app.wait_until(cx, "the demo schema", QUERY_TIMEOUT, |window, _| {
        labelled(window, &schema).is_some()
    });
    app.click_labelled(cx, &schema);
    app.wait_until(cx, "the demo table", QUERY_TIMEOUT, |window, _| {
        labelled(window, "bookings").is_some()
    });
    app.context_menu_labelled(cx, "bookings");
    app.choose(cx, "popup-menu", "Show dbt details");
    app.wait_until(cx, "the demo model", QUERY_TIMEOUT, |window, _| {
        label(window, "dbt-details-Unique ID").as_deref() == Some("model.travel.bookings")
    });
    app.scroll_to(cx, "dbt-details-raw-sql-toggle");
    app.click(cx, "dbt-details-raw-sql-toggle");
    app.wait_until(cx, "the demo model SQL", QUERY_TIMEOUT, |window, _| {
        label(window, "dbt-details-raw-sql").as_deref() == Some(
            "select booking_id, gate, amount, booked_at\nfrom {{ source('flights', 'flight_events') }}")
    });
    app.press(cx, "escape");
    app.wait_gone(cx, "dbt-details-Unique ID");
    app.run_complete(cx, &format!("DROP DATABASE {schema} CASCADE"));
}
