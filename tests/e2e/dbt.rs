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
    app.scroll_to(cx, "connection-dbt-manifest");
    app.wait_until(cx, "the manifest", QUERY_TIMEOUT, |window, _| {
        label(window, "connection-dbt-status").as_deref()
            == Some("Manifest from 2026-10-01 08:00 UTC, dbt 1.12.5, 2 models")
    });
    app.scroll_to(cx, "connection-dbt-matches");
    app.wait_until(cx, "the match summary", QUERY_TIMEOUT, |window, _| {
        label(window, "connection-dbt-matches").as_deref()
            == Some("1 of 2 models and sources match tables in the catalog.")
    });
    app.click(cx, "cancel-profile");
    app.wait_gone(cx, "connection-name");
    app.run_complete(cx, &format!("DROP DATABASE {schema} CASCADE"));
}
