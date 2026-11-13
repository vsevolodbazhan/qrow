//! The autocomplete popup of the SQL editor, with cached metadata and a
//! connection that no test reaches.
use crate::support::{MemoryCredentials, TestApp, bounds_of, label, labelled, offline_profile};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt as _;
use qrow::{
    catalog::{Catalog, CatalogColumn, RelationEntry, RelationKind},
    model::{CatalogSettings, Profile, SavedTab, Workspace},
    storage,
};
use std::{collections::BTreeMap, time::Duration};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(10);

fn column(name: &str, data_type: &str, comment: Option<&str>) -> CatalogColumn {
    CatalogColumn {
        name: name.into(),
        data_type: data_type.into(),
        comment: comment.map(Into::into),
    }
}

/// Saves a metadata cache for `profile`, read at `read_at`: the schema
/// `avia` with `bookings` and its columns, and the view `daily` without
/// columns; and the schema `finance` without tables.
fn cache(directory: &TempDir, profile: &Profile, read_at: u64, relations: &[&str]) {
    let mut catalog = Catalog::new(profile);
    catalog.apply_schemas(
        vec!["avia".into(), "finance".into()],
        &CatalogSettings::default(),
        read_at,
    );
    let mut entries = vec![
        RelationEntry {
            name: "bookings".into(),
            kind: RelationKind::Table,
            comment: None,
        },
        RelationEntry {
            name: "daily".into(),
            kind: RelationKind::View,
            comment: None,
        },
    ];
    entries.extend(relations.iter().map(|name| RelationEntry {
        name: (*name).into(),
        kind: RelationKind::Table,
        comment: None,
    }));
    catalog.apply_relations("avia", None, entries, read_at);
    catalog.apply_columns(
        "avia",
        Some("bookings"),
        BTreeMap::from([(
            "bookings".into(),
            vec![
                column("booking_id", "BIGINT", None),
                column("amount", "DECIMAL(12,2)", Some("Total fare.")),
            ],
        )]),
        read_at,
    );
    let path = storage::catalog_cache_path(directory.path(), profile.id);
    storage::save_catalog(&path, &catalog).unwrap();
}

fn profile() -> Profile {
    let mut profile = offline_profile("Warehouse");
    profile.database = "avia".into();
    profile
}

fn launch(cx: &mut TestAppContext, read_at: Option<u64>, relations: &[&str]) -> (TestApp, Profile) {
    let profile = profile();
    let directory = tempfile::tempdir().unwrap();
    if let Some(read_at) = read_at {
        cache(&directory, &profile, read_at, relations);
    }
    let app = TestApp::launch_in(
        cx,
        directory,
        Workspace {
            tabs: vec![SavedTab::new(1, Some(profile.id))],
            profiles: vec![profile.clone()],
            ..Workspace::default()
        },
        MemoryCredentials::default(),
    );
    (app, profile)
}

fn fresh() -> Option<u64> {
    Some(qrow::catalog::now())
}

/// Types `sql` in the editor and presses ⌃Space.
fn complete(app: &TestApp, cx: &mut TestAppContext, sql: &str) {
    app.type_sql(cx, sql);
    app.press(cx, "ctrl-space");
    app.wait_for(cx, "sql-completion");
}

/// Waits until the tab saves `expected` as its SQL.
fn assert_sql(app: &TestApp, cx: &mut TestAppContext, expected: &str) {
    app.wait_until(cx, expected, WAIT, |_, _| {
        app.saved().tabs[0].sql == expected
    });
}

/// The names of the list, from top to bottom.
fn items(app: &TestApp, cx: &mut TestAppContext) -> Vec<String> {
    let mut rows: Vec<_> = app.update(cx, |window, _| {
        crate::support::elements(window)
            .into_iter()
            .filter(|element| {
                element
                    .path()
                    .last()
                    .is_some_and(|id| format!("{id:?}").contains("sql-completion-item-"))
            })
            .map(|element| {
                (
                    element.bounds().origin.y,
                    element.label().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    });
    rows.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    rows.into_iter().map(|(_, label)| label).collect()
}

#[gpui_kit::test]
fn ctrl_space_suggests_tables_and_enter_inserts_the_selected_one(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    complete(&app, cx, "SELECT * FROM avia.");
    assert_eq!(items(&app, cx), ["bookings", "daily"]);
    // Fresh metadata has no footer.
    assert!(!app.update(cx, |window, _| {
        window.try_find("sql-completion-footer").is_some()
    }));

    // Down and Enter insert the selected name.
    app.press(cx, "down");
    app.press(cx, "enter");
    app.wait_gone(cx, "sql-completion");
    assert_sql(&app, cx, "SELECT * FROM avia.daily");

    // Typing filters the list.
    complete(&app, cx, "SELECT * FROM avia.");
    app.update(cx, |window, cx| window.input("bo", cx));
    app.settle(cx);
    assert_eq!(items(&app, cx), ["bookings"]);
    app.press(cx, "enter");
    assert_sql(&app, cx, "SELECT * FROM avia.bookings");
}

#[gpui_kit::test]
fn after_from_the_popup_lists_schemas_then_tables_of_the_default_schema(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    complete(&app, cx, "SELECT * FROM ");
    assert_eq!(items(&app, cx), ["avia", "finance", "bookings", "daily"]);
    // Escape closes the popup and keeps the text.
    app.press(cx, "escape");
    app.wait_gone(cx, "sql-completion");
    assert_sql(&app, cx, "SELECT * FROM ");
}

#[gpui_kit::test]
fn an_alias_lists_the_columns_with_details_of_the_selected_one(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    complete(&app, cx, "SELECT b.");
    app.press(cx, "escape");
    app.wait_gone(cx, "sql-completion");
    // The statement names the table after the cursor.
    app.type_sql(cx, "SELECT b.am FROM avia.bookings b");
    app.update(cx, |window, cx| {
        for _ in 0.." FROM avia.bookings b".len() {
            window.press("left", cx);
        }
    });
    app.press(cx, "ctrl-space");
    app.wait_for(cx, "sql-completion");
    assert_eq!(items(&app, cx), ["amount"]);
    app.wait_for(cx, "sql-completion-details");
    assert_eq!(
        app.update(cx, |window, _| label(window, "sql-completion-details")),
        Some("amount, decimal(12,2), avia.bookings, Total fare.".into())
    );
    app.press(cx, "tab");
    app.wait_gone(cx, "sql-completion");
    assert_sql(&app, cx, "SELECT b.amount FROM avia.bookings b");
}

#[gpui_kit::test]
fn typing_past_the_name_or_leaving_the_editor_closes_the_popup(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    complete(&app, cx, "SELECT * FROM avia.b");
    app.update(cx, |window, cx| window.input(" ", cx));
    app.wait_gone(cx, "sql-completion");

    app.press(cx, "ctrl-space");
    app.wait_for(cx, "sql-completion");
    app.click_labelled(cx, "Search connections and relations");
    app.wait_gone(cx, "sql-completion");
}

#[gpui_kit::test]
fn stale_metadata_shows_a_footer_whose_refresh_reads_the_schema(cx: &mut TestAppContext) {
    let (app, profile) = launch(cx, Some(1), &[]);
    complete(&app, cx, "SELECT * FROM avia.");
    app.wait_for(cx, "sql-completion-footer");
    assert_eq!(
        app.update(cx, |window, _| label(window, "sql-completion-footer")),
        Some("avia is stale".into())
    );
    // The shortcut runs Refresh while the popup is open.
    app.press(cx, "ctrl-space");
    app.wait_until(cx, "the refresh of avia", WAIT, |_, _| true);
    let activity = app.activity(cx, profile.id);
    assert!(
        activity.contains("Started a metadata refresh of schema avia"),
        "{activity}"
    );
}

#[gpui_kit::test]
fn columns_that_are_not_cached_load_and_a_failed_load_offers_retry(cx: &mut TestAppContext) {
    let (app, profile) = launch(cx, fresh(), &[]);
    complete(&app, cx, "SELECT d. FROM avia.daily d");
    app.wait_until(cx, "the failed load", WAIT, |window, _| {
        label(window, "sql-completion-notice").as_deref() == Some("Could not load columns")
    });
    assert!(app.update(cx, |window, _| {
        window.try_find("sql-completion-retry").is_some()
    }));
    let activity = app.activity(cx, profile.id);
    assert!(activity.contains("avia.daily"), "{activity}");
}

#[gpui_kit::test]
fn a_connection_without_cached_metadata_offers_refresh(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, None, &[]);
    complete(&app, cx, "SELECT * FROM ");
    app.wait_until(cx, "the empty state", WAIT, |window, _| {
        label(window, "sql-completion-notice").as_deref() == Some("No cached tables")
    });
    assert!(app.update(cx, |window, _| {
        window.try_find("sql-completion-refresh").is_some()
    }));
}

#[gpui_kit::test]
fn long_names_widen_the_list_up_to_its_limit(cx: &mut TestAppContext) {
    let long = "customer_lifetime_value_by_acquisition_channel_and_region_v2";
    let (app, _) = launch(cx, fresh(), &[long, "customers"]);
    complete(&app, cx, "SELECT * FROM avia.cust");
    assert_eq!(items(&app, cx), [long, "customers"]);
    let width = app.update(cx, |window, _| {
        bounds_of(window, "sql-completion").size.width
    });
    assert!(width <= gpui_kit::px(440.), "{width:?}");
    assert!(width > gpui_kit::px(320.), "{width:?}");
    assert!(app.update(cx, |window, _| labelled(window, long).is_some()));
}

#[gpui_kit::test]
fn a_failed_read_of_one_table_shows_retry_below_the_other_columns(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    let tail = " FROM avia.bookings b JOIN avia.daily d";
    app.type_sql(cx, &format!("SELECT am{tail}"));
    app.update(cx, |window, cx| {
        for _ in 0..tail.len() {
            window.press("left", cx);
        }
    });
    app.press(cx, "ctrl-space");
    app.wait_for(cx, "sql-completion");
    assert_eq!(items(&app, cx), ["amount"]);
    // The columns of `daily` are not cached, and the connection is out of
    // reach, so their read fails.
    app.wait_until(cx, "the failed read", WAIT, |window, _| {
        label(window, "sql-completion-footer").as_deref()
            == Some("Could not load columns of avia.daily")
    });
    assert_eq!(
        app.update(cx, |window, _| label(
            window,
            "sql-completion-footer-action"
        )),
        Some("Retry".into())
    );
}

#[gpui_kit::test]
fn tab_after_a_name_opens_the_list_and_tab_inserts(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    app.type_sql(cx, "SELECT * FROM avia.bo");
    app.press(cx, "tab");
    app.wait_for(cx, "sql-completion");
    assert_eq!(items(&app, cx), ["bookings"]);
    app.press(cx, "tab");
    app.wait_gone(cx, "sql-completion");
    assert_sql(&app, cx, "SELECT * FROM avia.bookings");

    // After FROM and a space, Tab opens the list too.
    app.type_sql(cx, "SELECT * FROM ");
    app.press(cx, "tab");
    app.wait_for(cx, "sql-completion");
    app.press(cx, "escape");
    app.wait_gone(cx, "sql-completion");
}

#[gpui_kit::test]
fn tab_indents_where_no_name_fits(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    for sql in ["SELECT 1 ", "SELECT * FROM avia.zzz"] {
        app.type_sql(cx, sql);
        app.press(cx, "tab");
        app.wait_until(cx, "the indent", WAIT, |_, _| {
            let saved = app.saved().tabs[0].sql.clone();
            // Tab indents the line or inserts spaces at the cursor.
            saved.len() > sql.len() && saved.contains(sql.trim_end())
        });
        assert!(!app.update(cx, |window, _| window.try_find("sql-completion").is_some()));
    }
}
