//! The dbt project of a connection: the form fields, the manifest state, the
//! match summary, and the refresh from the sidebar.
use crate::dbt_manifest::{self, Shape};
use crate::support::{MemoryCredentials, TestApp, connection_row, label, offline_profile};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use qrow::{
    catalog::{Catalog, RelationEntry, RelationKind},
    model::{
        CatalogSettings, DbtProject, DbtRefresh, Profile, SavedTab, SchemaRule, SchemaRuleKind,
        Workspace,
    },
    storage,
};
use std::{path::PathBuf, time::Duration};
use tempfile::TempDir;

const TIMEOUT: Duration = Duration::from_secs(20);

/// 12 models (model 0 is ephemeral), 1 seed, 1 snapshot, and 3 sources.
fn shape() -> Shape {
    Shape {
        models: 12,
        sources: 3,
        macros: 2,
        columns: 3,
        compiled: false,
        fusion: false,
    }
}

/// Writes a synthetic manifest in a project folder in `directory`.
fn project(directory: &TempDir, bytes: &[u8]) -> PathBuf {
    let target = directory.path().join("lake/target");
    std::fs::create_dir_all(&target).unwrap();
    let manifest = target.join("manifest.json");
    std::fs::write(&manifest, bytes).unwrap();
    manifest
}

/// A schema cache like production: dbt writes to analytics_core, and the
/// catalog has core. Model 5 is missing, and raw_0 has no relation list.
fn cache(directory: &TempDir, profile: &Profile) {
    let mut catalog = Catalog::new(profile);
    let mut schemas: Vec<String> = (1..12)
        .map(|model| dbt_manifest::model_schema(model).replace("analytics_", ""))
        .collect();
    schemas.sort();
    schemas.dedup();
    schemas.push("raw_0".into());
    catalog.apply_schemas(schemas.clone(), &CatalogSettings::default(), 1);
    for schema in schemas.iter().filter(|schema| *schema != "raw_0") {
        let relations = (1..12)
            .filter(|model| *model != 5)
            .filter(|model| dbt_manifest::model_schema(*model) == format!("analytics_{schema}"))
            .map(|model| RelationEntry {
                name: dbt_manifest::model_alias(model),
                kind: RelationKind::Table,
                comment: None,
            })
            .collect();
        catalog.apply_relations(schema, None, relations, 1);
    }
    let path = storage::catalog_path(&directory.path().join("workspace.json"), profile.id);
    storage::save_catalog(&path, &catalog).unwrap();
}

fn launch(cx: &mut TestAppContext, directory: TempDir, profile: Profile) -> TestApp {
    let workspace = Workspace {
        tabs: vec![SavedTab::new(1, Some(profile.id))],
        profiles: vec![profile],
        ..Workspace::default()
    };
    TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default())
}

fn edit(app: &TestApp, cx: &mut TestAppContext, profile: uuid::Uuid) {
    app.context_menu(cx, connection_row(profile));
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-name");
    app.scroll_to(cx, "connection-dbt-manifest");
}

/// Replaces the schema mapping rules.
fn rules(app: &TestApp, cx: &mut TestAppContext, text: &str) {
    app.scroll_to(cx, "connection-dbt-rules");
    app.update(cx, |window, cx| {
        window.click("connection-dbt-rules", cx);
        window.press("cmd-a", cx);
        if text.is_empty() {
            window.press("backspace", cx);
        } else {
            window.input(text, cx);
        }
    });
}

fn wait_label(app: &TestApp, cx: &mut TestAppContext, id: &'static str, expected: &str) {
    app.wait_until(cx, expected, TIMEOUT, |window, _| {
        label(window, id).as_deref() == Some(expected)
    });
}

fn wait_label_starting(app: &TestApp, cx: &mut TestAppContext, id: &'static str, start: &str) {
    app.wait_until(cx, start, TIMEOUT, |window, _| {
        label(window, id).is_some_and(|text| text.starts_with(start))
    });
}

#[gpui_kit::test]
fn a_connection_reads_its_manifest_and_matches_its_tables(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let manifest = project(&directory, &dbt_manifest::generate(&shape()));
    let profile = offline_profile("Lake");
    cache(&directory, &profile);
    let id = profile.id;
    let app = launch(cx, directory, profile);

    edit(&app, cx, id);
    wait_label_starting(&app, cx, "connection-dbt-status", "The manifest.json file");
    app.fill(cx, "connection-dbt-manifest", manifest.to_str().unwrap());
    wait_label(&app, cx, "connection-dbt-status", "Read after you save.");
    rules(&app, cx, "analytics_* = *");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(cx, "the saved project", TIMEOUT, |_, _| {
        app.saved().profiles[0].dbt
            == Some(DbtProject {
                manifest: manifest.to_string_lossy().into_owned(),
                refresh: DbtRefresh::Automatic,
                schema_mapping: vec![SchemaRule {
                    kind: SchemaRuleKind::Prefix,
                    from: "analytics_".into(),
                    to: String::new(),
                }],
            })
    });

    edit(&app, cx, id);
    wait_label_starting(&app, cx, "connection-dbt-status", "Manifest from ");
    let status = app.update(cx, |window, _| {
        label(window, "connection-dbt-status").unwrap()
    });
    assert!(status.ends_with(", dbt 1.12.5, 12 models"), "{status}");
    app.scroll_to(cx, "connection-dbt-matches");
    wait_label(
        &app,
        cx,
        "connection-dbt-matches",
        "10 of 16 models and sources match tables in the catalog. 1 is in a schema without loaded tables.",
    );
    app.scroll_to(cx, "connection-dbt-unmatched");
    app.click(cx, "connection-dbt-unmatched");
    app.wait_for(cx, "connection-dbt-unmatched-list");
    let rows: Vec<String> = app.update(cx, |window, _| {
        (0..5usize)
            .filter_map(|row| label(window, ("connection-dbt-unmatched-row", row)))
            .collect()
    });
    assert_eq!(
        rows,
        [
            format!(
                "model {}: {}.{} (no table)",
                dbt_manifest::model_name(5),
                dbt_manifest::model_schema(5).replace("analytics_", ""),
                dbt_manifest::model_alias(5)
            ),
            "seed seed_0000: analytics.seed_0000 (no schema)".into(),
            "snapshot snapshot_0000: snapshots.snapshot_0000 (no schema)".into(),
            "source table_00001: raw_1.table_00001_src (no schema)".into(),
            "source table_00002: raw_2.table_00002_src (no schema)".into(),
        ]
    );
    // An edit of the rules changes the summary before the save.
    rules(&app, cx, "");
    app.scroll_to(cx, "connection-dbt-matches");
    wait_label(
        &app,
        cx,
        "connection-dbt-matches",
        "0 of 16 models and sources match tables in the catalog. 1 is in a schema without loaded tables.",
    );
    app.click(cx, "cancel-profile");
    app.wait_gone(cx, "connection-name");

    // Each refresh is in Activity, also one from the sidebar.
    app.context_menu(cx, connection_row(id));
    app.choose(cx, "popup-menu", "Refresh manifest");
    app.wait_until(cx, "the second refresh", TIMEOUT, |_, _| true);
    let activity = (0..200)
        .map(|_| {
            let text = app.activity(cx, id);
            std::thread::sleep(Duration::from_millis(50));
            text
        })
        .find(|text| text.matches("dbt manifest refreshed in").count() == 2)
        .expect("two refreshes in Activity");
    assert!(
        activity.contains("12 models, 1 seed, 1 snapshot, 3 sources"),
        "{activity}"
    );
}

#[gpui_kit::test]
fn the_form_rejects_invalid_projects_and_shows_manifest_errors(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let text = String::from_utf8(dbt_manifest::generate(&shape())).unwrap();
    let manifest = project(
        &directory,
        text.replace("manifest/v12", "manifest/v11").as_bytes(),
    );
    let profile = offline_profile("Lake");
    let id = profile.id;
    let app = launch(cx, directory, profile);
    let error = |app: &TestApp, cx: &mut TestAppContext, expected: &str| {
        app.click(cx, "save-profile");
        wait_label(app, cx, "connection-form-error-accessibility", expected);
    };

    edit(&app, cx, id);
    rules(&app, cx, "analytics = prod");
    error(
        &app,
        cx,
        "Choose the manifest.json file of the dbt project, or remove the schema mapping.",
    );
    app.fill(cx, "connection-dbt-manifest", "target/manifest.json");
    error(
        &app,
        cx,
        "Choose the manifest.json file of the dbt project.",
    );
    app.fill(cx, "connection-dbt-manifest", manifest.to_str().unwrap());
    rules(&app, cx, "analytics_*");
    error(
        &app,
        cx,
        "Schema mapping line 1 must be like dbt_dev_* = * or analytics = prod.",
    );
    rules(&app, cx, "");
    app.select(cx, "connection-dbt-refresh", "Manual");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(cx, "the saved project", TIMEOUT, |_, _| {
        app.saved().profiles[0]
            .dbt
            .as_ref()
            .is_some_and(|dbt| dbt.refresh == DbtRefresh::Manual)
    });
    edit(&app, cx, id);
    wait_label(
        &app,
        cx,
        "connection-dbt-status",
        "Unsupported manifest version v11: Qrow reads manifest v12 (dbt 1.8 and later)",
    );
    // An empty field removes the project.
    app.fill(cx, "connection-dbt-manifest", "");
    app.click(cx, "save-profile");
    app.wait_gone(cx, "connection-name");
    app.wait_until(cx, "the removed project", TIMEOUT, |_, _| {
        app.saved().profiles[0].dbt.is_none()
    });
}

#[gpui_kit::test]
fn a_launch_reads_the_manifests_of_the_connections(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let manifest = project(&directory, &dbt_manifest::generate(&shape()));
    let saved = directory.path().join("dbt");
    let mut profile = offline_profile("Lake");
    profile.dbt = Some(DbtProject {
        manifest: manifest.to_string_lossy().into_owned(),
        refresh: DbtRefresh::Automatic,
        schema_mapping: vec![],
    });
    let app = launch(cx, directory, profile);
    // No action wakes the window: the launch alone starts the worker.
    app.wait_until(cx, "the saved index", TIMEOUT, |_, _| {
        std::fs::read_dir(&saved).is_ok_and(|mut files| files.next().is_some())
    });
}

#[gpui_kit::test]
fn the_assistant_reads_the_dbt_meaning_of_the_tables_of_its_tab(cx: &mut TestAppContext) {
    use crate::support::assistant::FakeCodex;
    let (directory, codex) = FakeCodex::new();
    let bytes = dbt_manifest::generate(&shape());
    let manifest = project(&directory, &bytes);
    let index = qrow::dbt::parse(&bytes).unwrap();
    let model = index
        .entry(index.find(&dbt_manifest::model_id(1)).unwrap())
        .clone();
    let schema = dbt_manifest::model_schema(1).replace("analytics_", "");
    let alias = dbt_manifest::model_alias(1);
    let mut profile = offline_profile("Lake");
    profile.dbt = Some(DbtProject {
        manifest: manifest.to_string_lossy().into_owned(),
        refresh: DbtRefresh::Manual,
        schema_mapping: vec![SchemaRule {
            kind: SchemaRuleKind::Prefix,
            from: "analytics_".into(),
            to: String::new(),
        }],
    });
    let mut catalog = Catalog::new(&profile);
    catalog.apply_schemas(vec![schema.clone()], &CatalogSettings::default(), 1);
    let entry = RelationEntry {
        name: alias.clone(),
        kind: RelationKind::Table,
        comment: None,
    };
    catalog.apply_relations(&schema, None, vec![entry], 1);
    catalog.apply_columns(
        &schema,
        Some(&alias),
        std::collections::BTreeMap::from([(
            alias.clone(),
            vec![qrow::catalog::CatalogColumn {
                name: "id".into(),
                data_type: "BIGINT".into(),
                comment: None,
            }],
        )]),
        1,
    );
    let path = storage::catalog_path(&directory.path().join("workspace.json"), profile.id);
    storage::save_catalog(&path, &catalog).unwrap();
    let saved = directory.path().join("dbt");
    let id = profile.id;
    let mut tab = SavedTab::new(1, Some(profile.id));
    tab.sql = format!("SELECT id FROM {schema}.{alias}");
    let workspace = codex.workspace(Workspace {
        profiles: vec![profile],
        tabs: vec![tab],
        ..Workspace::default()
    });
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.wait_until(cx, "the dbt index", TIMEOUT, |_, _| {
        std::fs::read_dir(&saved).is_ok_and(|mut files| files.next().is_some())
    });
    // The sidebar loads the saved catalog.
    app.toggle_connection(cx, id);
    app.wait_until(cx, "the schema", TIMEOUT, |window, _| {
        crate::support::labelled(window, &schema).is_some()
    });
    app.open_assistant(cx);
    app.send(cx, "Read dbt");
    app.wait_reply(
        cx,
        &format!(
            "dbt 1.12.5, models 12, matched 1; summary {} unique id; describe {}, parents {}, tests {}",
            dbt_manifest::model_id(1),
            index.symbol(model.materialized.unwrap()),
            model.parents.len(),
            dbt_manifest::test_names(1).len(),
        ),
    );
}
