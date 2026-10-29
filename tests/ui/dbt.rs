//! The dbt project of a connection: the form fields, the manifest state, the
//! match summary, and the refresh from the sidebar.
use crate::dbt_manifest::{self, Shape};
use crate::support::{MemoryCredentials, TestApp, connection_row, label, offline_profile};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, InputEvent as _, TestAppContext};
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
    app.connection_page(cx, "dbt");
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
    app.wait_gone(cx, "save-profile");
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
        "10 of 16 dbt resources matched the catalog",
    );
    // The bar and the numbers share the leading edge of the field, and the
    // percentage ends with the bar.
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "connection-dbt-match-percent").as_deref(),
            Some("63%")
        );
        let bounds = |id: &str| window.find(id.to_owned()).bounds();
        let field = bounds("connection-dbt-rules");
        let bar = bounds("connection-dbt-match-ratio");
        assert_eq!(bar.left(), field.left());
        assert_eq!(bar.right(), field.right());
        assert_eq!(bounds("connection-dbt-matches").left(), field.left());
        assert_eq!(bounds("connection-dbt-match-percent").right(), bar.right());
        assert!(!crate::support::present(
            window,
            &"connection-dbt-unmatched".into()
        ));
    });
    // An edit of the rules changes the summary before the save.
    rules(&app, cx, "");
    app.scroll_to(cx, "connection-dbt-matches");
    wait_label(
        &app,
        cx,
        "connection-dbt-matches",
        "0 of 16 dbt resources matched the catalog",
    );
    app.click(cx, "cancel-profile");
    app.wait_gone(cx, "save-profile");

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
    app.wait_gone(cx, "save-profile");
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
    app.wait_gone(cx, "save-profile");
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
    app.open_assistant(cx);
    app.send(cx, "Read dbt");
    // The context needs no schema catalog. The catalog tool loads it and
    // points to the model.
    app.wait_reply(
        cx,
        &format!(
            "dbt 1.12.5, models 12; model {id}, table points to {id}; {}, {} tests, columns id 2 tests; parents {}; raw sql, compiled missing True",
            index.symbol(model.materialized.unwrap()),
            dbt_manifest::test_names(1).len(),
            model.parents.len(),
            id = dbt_manifest::model_id(1),
        ),
    );
}

/// Launches a connection with the dbt manifest `bytes`, and waits until it
/// has read the manifest.
fn launch_project(
    cx: &mut TestAppContext,
    bytes: &[u8],
    refresh: DbtRefresh,
) -> (TestApp, PathBuf, uuid::Uuid) {
    let directory = tempfile::tempdir().unwrap();
    let manifest = project(&directory, bytes);
    let mut profile = offline_profile("Lake");
    profile.dbt = Some(DbtProject {
        manifest: manifest.to_string_lossy().into_owned(),
        refresh,
        schema_mapping: vec![SchemaRule {
            kind: SchemaRuleKind::Prefix,
            from: "analytics_".into(),
            to: String::new(),
        }],
    });
    cache(&directory, &profile);
    let saved = directory.path().join("dbt");
    let id = profile.id;
    let app = launch(cx, directory, profile);
    app.wait_until(cx, "the dbt index", TIMEOUT, |_, _| {
        std::fs::read_dir(&saved).is_ok_and(|mut files| files.next().is_some())
    });
    (app, manifest, id)
}

/// Shows the table of model 1 in the schema tree, and returns its name.
fn show_model_table(app: &TestApp, cx: &mut TestAppContext, id: uuid::Uuid) -> String {
    let schema = dbt_manifest::model_schema(1).replace("analytics_", "");
    let alias = dbt_manifest::model_alias(1);
    app.toggle_connection(cx, id);
    app.wait_until(cx, "the schema", TIMEOUT, |window, _| {
        crate::support::labelled(window, &schema).is_some()
    });
    app.click_labelled(cx, &schema);
    app.wait_until(cx, "the table", TIMEOUT, |window, _| {
        crate::support::labelled(window, &alias).is_some()
    });
    alias
}

#[gpui_kit::test]
fn the_tree_marks_dbt_tables_and_shows_their_details(cx: &mut TestAppContext) {
    let bytes = dbt_manifest::generate(&shape());
    let index = qrow::dbt::parse(&bytes).unwrap();
    let model = index
        .entry(index.find(&dbt_manifest::model_id(1)).unwrap())
        .clone();
    let (app, manifest, id) = launch_project(cx, &bytes, DbtRefresh::Manual);
    let schema = dbt_manifest::model_schema(1).replace("analytics_", "");
    let alias = show_model_table(&app, cx, id);
    // The row has a detail with the materialization; a unit test checks
    // its text.
    let detail: gpui_kit::ElementId =
        format!("r\u{1f}{id}\u{1f}{schema}\u{1f}{alias}\u{1f}detail").into();
    app.wait_until(cx, "the dbt detail", TIMEOUT, |window, _| {
        crate::support::present(window, &detail)
    });
    // The tooltip has the start of the dbt description on one line.
    app.hover_labelled(cx, &alias);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(800));
    app.settle(cx);
    let tooltip = app.update(cx, |window, _| label(window, "catalog-tooltip"));
    let words: Vec<&str> = model.description.split_whitespace().take(6).collect();
    assert!(
        tooltip
            .as_deref()
            .is_some_and(|text| text.starts_with(&format!("{alias}\n{}", words.join(" ")))),
        "{tooltip:?}"
    );

    // The details sheet has the full description and the columns.
    app.context_menu_labelled(cx, &alias);
    app.update(cx, |window, _| {
        assert!(crate::support::labelled(window, "Open Model SQL").is_none());
    });
    app.choose(cx, "popup-menu", "Show dbt details");
    app.wait_until(cx, "the dbt details", TIMEOUT, |window, _| {
        label(window, "dbt-details-description").as_deref() == Some(model.description.trim())
    });
    let column_id =
        |name: &str| -> gpui_kit::ElementId { format!("dbt-details-column-{name}").into() };
    // The parts below the SQL are closed, with the counts of their items.
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "dbt-details-Unique ID").as_deref(),
            Some(dbt_manifest::model_id(1).as_str())
        );
        let columns = format!("Columns ({})", model.columns.len());
        assert!(
            crate::support::labelled(window, &columns).is_some(),
            "{columns}"
        );
        for title in ["Tests (", "Parents (", "Children ("] {
            assert_eq!(
                crate::support::labelled_starting(window, title).len(),
                1,
                "{title}"
            );
        }
        assert!(crate::support::labelled(window, "Table Tests").is_none());
        assert!(!crate::support::present(window, &column_id("id")));
        assert!(!crate::support::present(
            window,
            &"dbt-details-filter".into()
        ));
        assert!(!crate::support::present(
            window,
            &("dbt-details-parent", 0usize).into()
        ));
    });
    app.click(cx, "dbt-details-columns-toggle");
    app.wait_until(cx, "the columns", TIMEOUT, |window, _| {
        crate::support::present(window, &column_id("id"))
    });
    app.update(cx, |window, _| {
        for name in ["id", "col_001", "col_002"] {
            assert!(crate::support::present(window, &column_id(name)), "{name}");
        }
    });
    // The manifest has no compiled SQL, so the Compiled SQL part tells why.
    app.click(cx, "dbt-details-compiled-sql-toggle");
    app.wait_until(cx, "the compiled SQL note", TIMEOUT, |window, _| {
        label(window, "dbt-details-compiled-sql-note")
            .is_some_and(|text| text.starts_with("The manifest has no compiled SQL."))
    });
    app.click(cx, "dbt-details-compiled-sql-toggle");
    app.wait_gone(cx, "dbt-details-compiled-sql-note");
    // The closed parts stand close together.
    app.update(cx, |window, _| {
        let compiled = window.find("dbt-details-compiled-sql-toggle").bounds();
        let raw = window.find("dbt-details-raw-sql-toggle").bounds();
        assert_eq!(raw.top() - compiled.bottom(), window.rem_size() * 0.25);
    });
    // The raw SQL opens in the sheet and can be copied, without the empty
    // line at its end.
    let raw = qrow::dbt::read_sql(&manifest, model.raw_code.unwrap()).unwrap();
    let raw = raw.trim_end_matches('\n').to_owned();
    // The pointer on the header of the closed part reads the SQL, so the
    // part opens with the SQL in the next frame.
    app.hover_labelled(cx, "Raw SQL");
    app.settle(cx);
    app.click(cx, "dbt-details-raw-sql-toggle");
    app.update(cx, |window, cx| {
        window.render_frame(cx);
        assert_eq!(
            label(window, "dbt-details-raw-sql").as_deref(),
            Some(raw.as_str())
        );
    });
    app.click(cx, "dbt-details-copy-raw-sql");
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some(raw.as_str()));
    app.click(cx, "dbt-details-raw-sql-toggle");
    app.wait_until(cx, "the closed SQL", TIMEOUT, |window, _| {
        !crate::support::present(window, &"dbt-details-raw-sql".into())
    });

    // dbt writes new SQL while the part is closed. The connection refreshes
    // manually, so the reopened part reads the manifest again first.
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let changed = format!("{raw}\n-- changed");
    value["nodes"][dbt_manifest::model_id(1)]["raw_code"] = changed.clone().into();
    // dbt also compiles the model this time.
    let compiled = "select 1 as id";
    value["nodes"][dbt_manifest::model_id(1)]["compiled_code"] = compiled.into();
    std::fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
    // The Compiled SQL part reads the manifest again, so it has the compiled
    // SQL that the old manifest did not have.
    app.click(cx, "dbt-details-compiled-sql-toggle");
    app.wait_until(cx, "the compiled SQL", TIMEOUT, |window, _| {
        label(window, "dbt-details-compiled-sql").as_deref() == Some(compiled)
    });
    app.click(cx, "dbt-details-compiled-sql-toggle");
    app.wait_gone(cx, "dbt-details-compiled-sql");
    app.click(cx, "dbt-details-raw-sql-toggle");
    app.wait_until(cx, "the new model SQL", TIMEOUT, |window, _| {
        label(window, "dbt-details-raw-sql").as_deref() == Some(changed.as_str())
    });
    app.click(cx, "dbt-details-raw-sql-toggle");
    app.wait_until(cx, "the closed SQL", TIMEOUT, |window, _| {
        !crate::support::present(window, &"dbt-details-raw-sql".into())
    });

    // The space above a column description and the space below it look the
    // same: the name and the description share a line height, and the tests
    // move down by the leading that a line of text has and a tag does not.
    app.update(cx, |window, _| {
        let bounds = |suffix: &str| {
            window
                .find(gpui_kit::ElementId::from(format!(
                    "dbt-details-column-id{suffix}"
                )))
                .bounds()
        };
        let (header, text, tests) = (bounds("-header"), bounds("-text"), bounds("-tests"));
        let lines = text.size.height / header.size.height;
        assert!((lines - lines.round()).abs() < 0.01, "{header:?} {text:?}");
        let above = text.top() - header.bottom();
        let below = tests.top() - text.bottom();
        assert!(
            (below - above * 2.).abs() < gpui_kit::px(0.5),
            "{above:?} {below:?}"
        );
    });
    app.fill(cx, "dbt-details-filter", "col_001");
    app.wait_until(cx, "the filtered columns", TIMEOUT, |window, _| {
        !crate::support::present(window, &column_id("id"))
    });
    app.update(cx, |window, _| {
        assert!(crate::support::present(window, &column_id("col_001")));
        assert!(!crate::support::present(window, &column_id("col_002")));
    });

    // The closed columns keep no rows, and open again with the filter.
    app.click(cx, "dbt-details-columns-toggle");
    app.wait_gone(cx, column_id("col_001"));
    app.click(cx, "dbt-details-columns-toggle");
    app.wait_for(cx, column_id("col_001"));
    app.update(cx, |window, _| {
        assert!(!crate::support::present(window, &column_id("col_002")));
    });

    // A parent opens its details in the sheet without the filter, and the
    // back button returns to the model as it was, with the filter.
    app.click(cx, "dbt-details-parents-toggle");
    app.wait_for(cx, ("dbt-details-parent", 0usize));
    let parent = index
        .entry(*model.parents.first().expect("the model has a parent"))
        .unique_id
        .to_string();
    app.click(cx, ("dbt-details-parent", 0usize));
    app.wait_until(cx, "the parent details", TIMEOUT, |window, _| {
        label(window, "dbt-details-Unique ID").as_deref() == Some(parent.as_str())
    });
    app.update(cx, |window, _| {
        assert_eq!(
            crate::support::value(window, "dbt-details-filter").as_deref(),
            Some("")
        );
        // The back button stands next to the close button of the sheet.
        let back = window.find("dbt-details-back").bounds();
        let close = window.find("close").bounds();
        assert!(back.right() < close.left(), "{back:?} {close:?}");
        assert!(
            close.left() - back.right() < window.rem_size(),
            "{back:?} {close:?}"
        );
        assert_eq!(back.center().y, close.center().y);
    });
    // The button and its tooltip name the model.
    app.hover_labelled(cx, &format!("Back to {}", dbt_manifest::model_name(1)));
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(800));
    app.wait_for(cx, "tooltip");
    app.click(cx, "dbt-details-back");
    app.wait_until(cx, "the model details", TIMEOUT, |window, _| {
        label(window, "dbt-details-Unique ID").as_deref()
            == Some(dbt_manifest::model_id(1).as_str())
    });
    app.update(cx, |window, _| {
        assert_eq!(
            crate::support::value(window, "dbt-details-filter").as_deref(),
            Some("col_001")
        );
        assert!(crate::support::present(window, &column_id("col_001")));
        assert!(!crate::support::present(window, &column_id("col_002")));
        assert!(!crate::support::present(window, &"dbt-details-back".into()));
    });
    // The close button closes the details of a parent, also with a way
    // back. The model details open again without the way back.
    app.click(cx, ("dbt-details-parent", 0usize));
    app.wait_until(cx, "the parent details", TIMEOUT, |window, _| {
        label(window, "dbt-details-Unique ID").as_deref() == Some(parent.as_str())
    });
    app.click(cx, "close");
    app.wait_until(cx, "the closed sheet", TIMEOUT, |window, _| {
        !crate::support::present(window, &"dbt-details".into())
    });
    app.context_menu_labelled(cx, &alias);
    app.choose(cx, "popup-menu", "Show dbt details");
    app.wait_until(cx, "the model details again", TIMEOUT, |window, _| {
        label(window, "dbt-details-Unique ID").as_deref()
            == Some(dbt_manifest::model_id(1).as_str())
    });
    app.update(cx, |window, _| {
        assert!(!crate::support::present(window, &"dbt-details-back".into()));
    });
    // Escape closes the sheet too.
    app.click(cx, "dbt-details-parents-toggle");
    app.wait_for(cx, ("dbt-details-parent", 0usize));
    app.click(cx, ("dbt-details-parent", 0usize));
    app.wait_until(cx, "the parent details", TIMEOUT, |window, _| {
        label(window, "dbt-details-Unique ID").as_deref() == Some(parent.as_str())
    });
    app.press(cx, "escape");
    app.wait_until(cx, "the closed sheet", TIMEOUT, |window, _| {
        !crate::support::present(window, &"dbt-details".into())
    });
}

#[gpui_kit::test]
fn the_dbt_details_scroll_evenly_past_a_long_description(cx: &mut TestAppContext) {
    // A description larger than the text view parses at once, and compiled
    // SQL with empty lines at the start, as dbt writes for a model with a
    // configuration block.
    let mut value: serde_json::Value =
        serde_json::from_slice(&dbt_manifest::generate(&shape())).unwrap();
    let description = (0..60)
        .map(|paragraph| {
            format!(
                "Paragraph {paragraph}: one row for each search, with its `route`, \
                 its device, and the source of its traffic."
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    assert!(description.len() > 4 * 1024);
    let sql = (0..120)
        .map(|line| format!("    column_{line},"))
        .collect::<Vec<_>>()
        .join("\n");
    let sql = format!("select\n{sql}\n    1 as one\nfrom orders");
    let node = &mut value["nodes"][dbt_manifest::model_id(1)];
    node["description"] = description.into();
    node["compiled_code"] = format!("\n\n{sql}\n\n").into();
    // dbt writes the manifest again while the panel is open.
    let (app, manifest, id) = launch_project(
        cx,
        &serde_json::to_vec(&value).unwrap(),
        DbtRefresh::Automatic,
    );
    let alias = show_model_table(&app, cx, id);
    // The tooltip has the start of the long description, then a smaller
    // hint below it.
    app.hover_labelled(cx, &alias);
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(800));
    app.wait_for(cx, "catalog-tooltip-hint");
    app.update(cx, |window, _| {
        let hint = "Open dbt details to see the full description.";
        assert_eq!(label(window, "catalog-tooltip-hint").as_deref(), Some(hint));
        let tooltip = label(window, "catalog-tooltip").unwrap_or_default();
        assert!(
            tooltip.starts_with(&format!(
                "{alias}\nParagraph 0: one row for each search, with its `route`,"
            )),
            "{tooltip}"
        );
        assert!(tooltip.ends_with(&format!("…\n{hint}")), "{tooltip}");
        let tooltip = window.find("catalog-tooltip").bounds();
        let hint = window.find("catalog-tooltip-hint").bounds();
        assert!(hint.bottom() <= tooltip.bottom(), "{tooltip:?} {hint:?}");
        assert!(hint.size.height < window.rem_size() * 1.25, "{hint:?}");
        let description = window.find("catalog-tooltip-description").bounds();
        // The Markdown description wraps at the width of the tooltip.
        assert_eq!(description.size.width, tooltip.size.width);
        assert!(tooltip.size.width <= window.rem_size() * 26., "{tooltip:?}");
        assert!(
            description.size.height > window.rem_size() * 2.,
            "{description:?}"
        );
    });
    app.context_menu_labelled(cx, &alias);
    app.choose(cx, "popup-menu", "Show dbt details");
    app.wait_for(cx, "dbt-details-description");
    let wheel = |cx: &mut TestAppContext, delta: f32| {
        app.update(cx, |window, cx| {
            let position = window.find("dbt-details-list").bounds().center();
            window.dispatch_event(
                gpui_kit::ScrollWheelEvent {
                    position,
                    delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                        gpui_kit::px(0.),
                        gpui_kit::px(delta),
                    )),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            // The frame after the wheel, before a parse in the background
            // can end.
            window.render_frame(cx);
        });
    };
    let top = |cx: &mut TestAppContext, id: &str| {
        app.update(cx, |window, _| {
            window
                .try_find(id.to_owned())
                .filter(|element| element.visible())
                .map(|element| element.bounds().top())
        })
    };
    // The description is taller than the window.
    for _ in 0..40 {
        if top(cx, "dbt-details-compiled-sql-toggle").is_some() {
            break;
        }
        wheel(cx, -120.);
    }
    app.settle(cx);
    app.click(cx, "dbt-details-compiled-sql-toggle");
    // The SQL has no empty lines at the start and at the end.
    app.wait_until(cx, "the compiled SQL", TIMEOUT, |window, _| {
        label(window, "dbt-details-compiled-sql").as_deref() == Some(sql.as_str())
    });

    // Scroll into the SQL, past the description.
    for _ in 0..40 {
        if top(cx, "dbt-details-description").is_none() {
            break;
        }
        wheel(cx, -120.);
    }
    for _ in 0..4 {
        wheel(cx, -120.);
    }
    app.settle(cx);
    assert!(top(cx, "dbt-details-description").is_none());
    // Scroll back up. Each step moves the header of the SQL by the same
    // distance, also when the description comes back.
    let mut header = top(cx, "dbt-details-compiled-sql-toggle");
    let mut description_seen = false;
    for step in 0..80 {
        wheel(cx, 40.);
        let next = top(cx, "dbt-details-compiled-sql-toggle");
        description_seen |= top(cx, "dbt-details-description").is_some();
        if let (Some(before), Some(after)) = (header, next) {
            let moved = f32::from(after - before);
            assert!(
                (moved - 40.).abs() < 0.5,
                "step {step}: {before:?} to {after:?}"
            );
        }
        if header.is_some() && next.is_none() {
            break;
        }
        // No background task runs between two steps, as when a trackpad
        // sends many wheel events quickly.
        header = next;
    }
    assert!(description_seen);

    // dbt compiles the model again. The panel keeps its place and shows
    // the new SQL.
    for _ in 0..10 {
        if top(cx, "dbt-details-compiled-sql-toggle").is_some() {
            break;
        }
        wheel(cx, -120.);
    }
    app.settle(cx);
    let before = top(cx, "dbt-details-compiled-sql-toggle").expect("the SQL header");
    let changed = format!("-- compiled again\n{sql}");
    value["nodes"][dbt_manifest::model_id(1)]["compiled_code"] = changed.clone().into();
    std::fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
    app.wait_until(cx, "the new compiled SQL", TIMEOUT, |window, _| {
        label(window, "dbt-details-compiled-sql").as_deref() == Some(changed.as_str())
    });
    assert_eq!(top(cx, "dbt-details-compiled-sql-toggle"), Some(before));

    // A parent opens from below the SQL. The back button returns to the
    // model at the same place, with the SQL open again: the place is in
    // rows of SQL that come after the read.
    let parent_top = |cx: &mut TestAppContext| {
        app.update(cx, |window, _| {
            window
                .try_find(("dbt-details-parent", 0usize))
                .filter(|element| element.visible())
                .map(|element| element.bounds().top())
        })
    };
    for _ in 0..40 {
        if top(cx, "dbt-details-parents-toggle").is_some() {
            break;
        }
        wheel(cx, -120.);
    }
    app.settle(cx);
    // The header comes in at the bottom edge of the list: move it fully
    // into view.
    for _ in 0..2 {
        wheel(cx, -120.);
    }
    app.settle(cx);
    app.click(cx, "dbt-details-parents-toggle");
    for _ in 0..40 {
        if parent_top(cx).is_some() {
            break;
        }
        wheel(cx, -120.);
    }
    app.settle(cx);
    let before = parent_top(cx).expect("the parent row");
    // Only rows of compiled SQL come between the two SQL headers, so a row
    // of SQL is at the top of the list.
    assert!(top(cx, "dbt-details-compiled-sql-toggle").is_none());
    let list_top = app.update(cx, |window, _| {
        window.find("dbt-details-list").bounds().top()
    });
    let raw_top = top(cx, "dbt-details-raw-sql-toggle").expect("the raw SQL header");
    assert!(
        raw_top > list_top + gpui_kit::px(20.),
        "{list_top:?} {raw_top:?}"
    );
    app.click(cx, ("dbt-details-parent", 0usize));
    app.wait_until(cx, "the parent details", TIMEOUT, |window, _| {
        label(window, "dbt-details-Unique ID").is_some_and(|id| id != dbt_manifest::model_id(1))
    });
    app.click(cx, "dbt-details-back");
    // The model opens from the top, and only the read of its SQL brings
    // the row of SQL that was at the top.
    app.wait_until(cx, "the model at the parent row", TIMEOUT, |window, _| {
        window
            .try_find(("dbt-details-parent", 0usize))
            .is_some_and(|element| element.visible() && element.bounds().top() == before)
    });
    app.settle(cx);
    assert_eq!(parent_top(cx), Some(before));
    let raw_top_again = top(cx, "dbt-details-raw-sql-toggle");
    assert_eq!(raw_top_again, Some(raw_top));

    // The user scrolls before the SQL is read again. The read does not move
    // the list back to the old place.
    app.click(cx, ("dbt-details-parent", 0usize));
    app.wait_until(cx, "the parent details", TIMEOUT, |window, _| {
        label(window, "dbt-details-Unique ID").is_some_and(|id| id != dbt_manifest::model_id(1))
    });
    app.update(cx, |window, cx| {
        window.click("dbt-details-back", cx);
        window.render_frame(cx);
        let position = window.find("dbt-details-list").bounds().center();
        window.dispatch_event(
            gpui_kit::ScrollWheelEvent {
                position,
                delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                    gpui_kit::px(0.),
                    gpui_kit::px(-120.),
                )),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    });
    app.wait_until(cx, "the model SQL read", TIMEOUT, |window, _| {
        label(window, "dbt-details-Unique ID").is_none()
            && crate::support::present(window, &"dbt-details-description".into())
    });
    app.settle(cx);
    let description = top(cx, "dbt-details-description");
    assert!(description.is_some());
    assert_eq!(parent_top(cx), None);

    // A click on the scrollbar track before the read moves the list too.
    for _ in 0..40 {
        if parent_top(cx).is_some() {
            break;
        }
        wheel(cx, -120.);
    }
    app.settle(cx);
    let before = parent_top(cx).expect("the parent row");
    app.click(cx, ("dbt-details-parent", 0usize));
    app.wait_until(cx, "the parent details", TIMEOUT, |window, _| {
        label(window, "dbt-details-Unique ID").is_some_and(|id| id != dbt_manifest::model_id(1))
    });
    app.update(cx, |window, cx| {
        window.click("dbt-details-back", cx);
        window.render_frame(cx);
        let list = window.find("dbt-details-list").bounds();
        let position = gpui_kit::point(
            list.right() - gpui_kit::px(4.),
            list.bottom() - list.size.height / 4.,
        );
        window.dispatch_event(
            gpui_kit::MouseDownEvent {
                button: gpui_kit::MouseButton::Left,
                position,
                modifiers: Default::default(),
                click_count: 1,
                first_mouse: false,
            }
            .to_platform_input(),
            cx,
        );
        window.dispatch_event(
            gpui_kit::MouseUpEvent {
                button: gpui_kit::MouseButton::Left,
                position,
                modifiers: Default::default(),
                click_count: 1,
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    });
    app.wait_until(cx, "the model SQL read", TIMEOUT, |window, _| {
        label(window, "dbt-details-Unique ID").is_none()
    });
    app.settle(cx);
    assert_ne!(parent_top(cx), Some(before));
}

#[gpui_kit::test]
fn the_demo_opens_a_catalog_and_a_complete_dbt_project(cx: &mut TestAppContext) {
    let app = TestApp::launch_demo(cx);
    app.wait_until(cx, "the expanded demo catalog", TIMEOUT, |window, _| {
        crate::support::labelled(window, "booking_id bigint").is_some()
    });
    app.context_menu_labelled(cx, "rivendell-s");
    app.choose(cx, "popup-menu", "Edit");
    app.wait_for(cx, "connection-name");
    app.connection_page(cx, "dbt");
    app.scroll_to(cx, "connection-dbt-manifest");
    let path = app.update(cx, |window, _| {
        PathBuf::from(window.find("connection-dbt-manifest").value().unwrap())
    });
    assert!(path.is_file());
    app.scroll_to(cx, "connection-dbt-matches");
    wait_label(
        &app,
        cx,
        "connection-dbt-matches",
        "4 of 4 dbt resources matched the catalog",
    );
    app.scroll_to(cx, "connection-dbt-refresh-now");
    app.click(cx, "connection-dbt-refresh-now");
    wait_label_starting(&app, cx, "connection-dbt-status", "Manifest from ");
    app.click(cx, "cancel-profile");
    app.wait_gone(cx, "save-profile");

    app.context_menu_labelled(cx, "bookings");
    app.choose(cx, "popup-menu", "Show dbt details");
    wait_label(&app, cx, "dbt-details-Unique ID", "model.travel.bookings");
    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "dbt-details-description").as_deref(),
            Some("One row per flight booking. Use booking_id to join payments.")
        );
    });
    app.click(cx, "dbt-details-columns-toggle");
    app.wait_for(cx, "dbt-details-column-booking_id");
    app.scroll_to(cx, "dbt-details-compiled-sql-toggle");
    app.click(cx, "dbt-details-compiled-sql-toggle");
    wait_label(
        &app,
        cx,
        "dbt-details-compiled-sql",
        "select booking_id, gate, amount, booked_at\nfrom avia.flight_events",
    );
    app.click(cx, "dbt-details-compiled-sql-toggle");
    app.click(cx, "dbt-details-raw-sql-toggle");
    wait_label(
        &app,
        cx,
        "dbt-details-raw-sql",
        "select booking_id, gate, amount, booked_at\nfrom {{ source('flights', 'flight_events') }}",
    );
    app.click(cx, "dbt-details-raw-sql-toggle");
    app.scroll_to(cx, "dbt-details-parents-toggle");
    app.click(cx, "dbt-details-parents-toggle");
    app.update(cx, |window, cx| {
        window.click(("dbt-details-parent", 0usize), cx)
    });
    wait_label(
        &app,
        cx,
        "dbt-details-Unique ID",
        "source.travel.flights.flight_events",
    );
    app.click(cx, "dbt-details-back");
    wait_label(&app, cx, "dbt-details-Unique ID", "model.travel.bookings");
    app.press(cx, "escape");
    app.wait_gone(cx, "dbt-details-Unique ID");

    app.update(cx, |window, _| {
        assert_eq!(
            label(window, "workspace-status").as_deref(),
            Some("Demo changes are not saved")
        );
    });
    cx.update_window(app.window, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(
        !path.exists(),
        "closing the demo removes its temporary manifest"
    );
}

#[gpui_kit::test]
fn the_dbt_details_close_after_the_focused_filter_leaves(cx: &mut TestAppContext) {
    let bytes = dbt_manifest::generate(&shape());
    let (app, _manifest, id) = launch_project(cx, &bytes, DbtRefresh::Manual);
    let alias = show_model_table(&app, cx, id);
    // The focused column filter leaves the sheet with its closed part. The
    // close button and Escape still close the sheet.
    for close in ["button", "escape"] {
        app.context_menu_labelled(cx, &alias);
        app.choose(cx, "popup-menu", "Show dbt details");
        app.wait_for(cx, "dbt-details-columns-toggle");
        app.click(cx, "dbt-details-columns-toggle");
        app.fill(cx, "dbt-details-filter", "col_001");
        app.click(cx, "dbt-details-columns-toggle");
        app.wait_gone(cx, "dbt-details-filter");
        if close == "button" {
            app.click(cx, "close");
        } else {
            app.press(cx, "escape");
        }
        app.wait_until(cx, "the closed sheet", TIMEOUT, |window, _| {
            !crate::support::present(window, &"dbt-details".into())
        });
    }
}
