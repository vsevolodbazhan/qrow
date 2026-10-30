use crate::support::{MemoryCredentials, TestApp, elements, label, offline_profile};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Role, TestAppContext};
use qrow::{
    catalog::{Catalog, CatalogColumn, RelationEntry, RelationKind},
    model::{DatabaseType, SavedTab, Workspace},
    storage,
    ui::OpenSettings,
};
use std::{collections::BTreeMap, time::Duration};

fn launch(cx: &mut TestAppContext, dialect: DatabaseType, autocomplete: bool) -> TestApp {
    let directory = tempfile::tempdir().unwrap();
    let mut profile = offline_profile("Completion fixture");
    profile.database_type = dialect;
    profile.database = if dialect == DatabaseType::Kyuubi {
        "sales".into()
    } else {
        "warehouse".into()
    };
    profile.trino_schema = "sales".into();
    let mut cache = Catalog::new(&profile);
    cache.apply_schemas(vec!["sales".into(), "unread".into()], &profile.catalog, 1);
    cache.apply_relations(
        "sales",
        None,
        vec!["orders", "refunds", "empty"]
            .into_iter()
            .map(|name| RelationEntry {
                name: name.into(),
                kind: RelationKind::Table,
                comment: None,
            })
            .collect(),
        1,
    );
    cache.apply_columns("sales", Some("orders"), BTreeMap::from([("orders".into(), vec![
        CatalogColumn { name: "amount".into(), data_type: "decimal(18,2)".into(), comment: Some("Order total.\n\nKeep this complete description in narrow windows. [link](https://example.invalid) <script>".into()) },
        CatalogColumn { name: "amount_eur".into(), data_type: "decimal(18,2)".into(), comment: None },
        CatalogColumn { name: "sample_rate".into(), data_type: "double".into(), comment: None },
        CatalogColumn { name: "日本語".into(), data_type: "string".into(), comment: None },
    ])]), 1);
    cache.apply_columns(
        "sales",
        Some("empty"),
        BTreeMap::from([("empty".into(), vec![])]),
        1,
    );
    storage::save_catalog(
        &storage::catalog_path(&directory.path().join("workspace.json"), profile.id),
        &cache,
    )
    .unwrap();
    let mut workspace = Workspace {
        tabs: vec![SavedTab::new(1, Some(profile.id))],
        profiles: vec![profile],
        ..Workspace::default()
    };
    workspace.settings.autocomplete = autocomplete;
    TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default())
}

fn complete(app: &TestApp, cx: &mut TestAppContext, sql: &str) {
    app.complete_sql(cx, sql);
}

fn wait_item(app: &TestApp, cx: &mut TestAppContext, text: &str) {
    app.wait_until(cx, text, Duration::from_secs(10), |window, _| {
        elements(window)
            .iter()
            .any(|item| item.role() == Some(Role::MenuItem) && item.label() == Some(text))
    });
}

#[gpui_kit::test]
fn cached_columns_replace_prefix_and_suffix_with_unicode_and_one_undo(cx: &mut TestAppContext) {
    for dialect in [
        DatabaseType::Kyuubi,
        DatabaseType::Postgres,
        DatabaseType::Trino,
    ] {
        let app = launch(cx, dialect, true);
        let sql = "select '😀', o.am|unt from sales.orders o";
        complete(&app, cx, sql);
        wait_item(&app, cx, "amount");
        app.wait_for(cx, "completion-documentation");
        app.press(cx, "enter");
        let expected = format!(
            "select '😀', o.{} from sales.orders o",
            dialect.quote_identifier("amount")
        );
        app.wait_until(
            cx,
            "accepted cached column",
            Duration::from_secs(10),
            |_, _| app.saved().tabs[0].sql == expected,
        );
        app.press(cx, "cmd-z");
        app.wait_until(
            cx,
            "one completion undo",
            Duration::from_secs(10),
            |_, _| app.saved().tabs[0].sql == sql.replace('|', ""),
        );
        assert_eq!(app.credentials.reads(), 0);
    }
}

#[gpui_kit::test]
fn incomplete_catalog_notes_are_not_selectable_and_can_open_without_items(cx: &mut TestAppContext) {
    let app = launch(cx, DatabaseType::Kyuubi, true);
    for (sql, expected, action) in [
        (
            "select r.| from sales.refunds r",
            "Columns of sales.refunds are not loaded.",
            true,
        ),
        (
            "select r.| from sales.unknown r",
            "sales.unknown is not in the catalog.",
            true,
        ),
        (
            "select r.| from unread.orders r",
            "Tables of unread are not loaded.",
            true,
        ),
        (
            "select r.| from missing.orders r",
            "missing is not in the catalog.",
            false,
        ),
    ] {
        complete(&app, cx, sql);
        app.wait_label(cx, expected);
        app.update(cx, |window, _| {
            assert_eq!(label(window, "completion-note").as_deref(), Some(expected));
            assert_eq!(window.try_find("completion-note-action").is_some(), action);
            assert!(
                !elements(window)
                    .iter()
                    .any(|item| item.role() == Some(Role::MenuItem))
            );
        });
        for key in ["up", "down", "escape"] {
            app.press(cx, key);
        }
        app.wait_gone(cx, "completion-menu");
        assert_eq!(app.saved().tabs[0].sql, sql.replace('|', ""));
    }
    assert_eq!(app.credentials.reads(), 0);
}

#[gpui_kit::test]
fn notes_do_not_join_keyboard_navigation_and_full_documentation_survives_narrow_layout(
    cx: &mut TestAppContext,
) {
    let app = launch(cx, DatabaseType::Kyuubi, true);
    cx.simulate_window_resize(
        app.window,
        gpui_kit::size(gpui_kit::px(760.), gpui_kit::px(800.)),
    );
    complete(
        &app,
        cx,
        "select am| from sales.orders o join sales.refunds r on true",
    );
    wait_item(&app, cx, "amount");
    app.wait_label(cx, "Columns of sales.refunds are not loaded.");
    app.update(cx, |window, _| {
        let doc = label(window, "completion-documentation").unwrap();
        assert!(doc.contains("Keep this complete description"), "{doc}");
        assert!(
            doc.contains("\\[link\\]"),
            "Catalog Markdown must remain plain text: {doc}"
        );
        assert!(
            window.find("completion-documentation").bounds().bottom()
                <= window.viewport_size().height
        );
    });
    app.press(cx, "down");
    app.wait_until(
        cx,
        "selected column documentation",
        Duration::from_secs(10),
        |window, _| {
            label(window, "completion-documentation")
                .is_some_and(|text| text.contains("No description"))
        },
    );
    app.press(cx, "enter");
    app.wait_until(
        cx,
        "keyboard skips note",
        Duration::from_secs(10),
        |_, _| app.saved().tabs[0].sql.contains("`amount_eur`"),
    );
    assert_eq!(app.credentials.reads(), 0);
}

#[gpui_kit::test]
fn autocomplete_can_be_disabled_saved_and_reenabled(cx: &mut TestAppContext) {
    let app = launch(cx, DatabaseType::Kyuubi, false);
    complete(&app, cx, "select o.am| from sales.orders o");
    app.update(cx, |window, _| {
        assert!(window.try_find("completion-menu").is_none())
    });
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "Autocomplete");
    app.wait_for(cx, "setting-autocomplete");
    app.click(cx, "setting-autocomplete");
    app.click(cx, "save-settings");
    app.wait_gone(cx, "save-settings");
    complete(&app, cx, "select o.am| from sales.orders o");
    wait_item(&app, cx, "amount");
    app.press(cx, "escape");
    app.wait_until(
        cx,
        "saved autocomplete setting",
        Duration::from_secs(10),
        |_, _| app.saved().settings.autocomplete,
    );
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "Autocomplete");
    app.click(cx, "setting-autocomplete");
    app.click(cx, "save-settings");
    app.wait_gone(cx, "save-settings");
    complete(&app, cx, "select o.am| from sales.orders o");
    app.update(cx, |window, _| {
        assert!(window.try_find("completion-menu").is_none())
    });
    app.wait_until(
        cx,
        "saved disabled autocomplete",
        Duration::from_secs(10),
        |_, _| !app.saved().settings.autocomplete,
    );
    assert_eq!(app.credentials.reads(), 0);
}

#[gpui_kit::test]
fn mouse_acceptance_and_automatic_typing_keep_atomic_edits(cx: &mut TestAppContext) {
    let app = launch(cx, DatabaseType::Trino, true);
    complete(&app, cx, "select o.am|unt from sales.orders o");
    wait_item(&app, cx, "amount");
    app.click_labelled(cx, "amount");
    app.wait_until(cx, "mouse completion", Duration::from_secs(10), |_, _| {
        app.saved().tabs[0].sql == "select o.\"amount\" from sales.orders o"
    });
    app.press(cx, "cmd-z");
    app.wait_until(
        cx,
        "mouse completion undo",
        Duration::from_secs(10),
        |_, _| app.saved().tabs[0].sql == "select o.amunt from sales.orders o",
    );
    complete(&app, cx, "select o.a| from sales.orders o");
    app.press(cx, "escape");
    app.update(cx, |window, cx| window.input("m", cx));
    wait_item(&app, cx, "amount");
    app.press(cx, "enter");
    app.wait_until(
        cx,
        "automatic completion",
        Duration::from_secs(10),
        |_, _| app.saved().tabs[0].sql == "select o.\"amount\" from sales.orders o",
    );
    app.press(cx, "cmd-z");
    app.wait_until(
        cx,
        "automatic completion undo",
        Duration::from_secs(10),
        |_, _| app.saved().tabs[0].sql == "select o.am from sales.orders o",
    );
}
