//! The autocomplete popup of the SQL editor, with cached metadata and a
//! connection that no test reaches.
use crate::support::{MemoryCredentials, TestApp, bounds_of, label, labelled, offline_profile};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{ElementId, InputEvent as _, TestAppContext};
use qrow::{
    catalog::{Catalog, CatalogColumn, RelationEntry, RelationKind},
    model::{CatalogSettings, Profile, SavedTab, Workspace},
    storage::{self, Credentials as _},
    ui::OpenSettings,
};
use std::{collections::BTreeMap, net::TcpListener, time::Duration};
use tempfile::TempDir;

const WAIT: Duration = Duration::from_secs(10);

fn column(name: &str, data_type: &str, comment: Option<&str>) -> CatalogColumn {
    CatalogColumn {
        name: name.into(),
        data_type: data_type.into(),
        comment: comment.map(Into::into),
    }
}

/// A long Markdown comment, like the description that dbt writes to the
/// database.
const ROUTE: &str = "The **route** of a booking: the `origin` and the `destination` airports, \
with the stops between them in the order of the flights. Join to `avia.airports` on the codes. \
A route of a cancelled booking has no stops.\n\n## Filter\n\nFilter on `pdate` first.";

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
                column("route", "STRING", Some(ROUTE)),
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

/// Like [`launch`], with a connection to a listening socket that never
/// answers, so each read stays in its loading state. The socket lives as
/// long as the returned listener.
fn launch_held(cx: &mut TestAppContext, read_at: Option<u64>) -> (TestApp, TcpListener) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let profile = Profile {
        host: "127.0.0.1".into(),
        port: listener.local_addr().unwrap().port(),
        ..profile()
    };
    let directory = tempfile::tempdir().unwrap();
    if let Some(read_at) = read_at {
        cache(&directory, &profile, read_at, &[]);
    }
    let credentials = MemoryCredentials::default();
    credentials
        .set_password(profile.id, "synthetic-password")
        .unwrap();
    let app = TestApp::launch_in(
        cx,
        directory,
        Workspace {
            tabs: vec![SavedTab::new(1, Some(profile.id))],
            profiles: vec![profile],
            ..Workspace::default()
        },
        credentials,
    );
    (app, listener)
}

fn fresh() -> Option<u64> {
    Some(qrow::catalog::now())
}

/// Types `sql` in the editor and presses Tab.
fn complete(app: &TestApp, cx: &mut TestAppContext, sql: &str) {
    app.type_sql(cx, sql);
    app.press(cx, "tab");
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
fn tab_after_a_dot_suggests_tables_and_enter_inserts_the_selected_one(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    complete(&app, cx, "SELECT * FROM avia.");
    assert_eq!(items(&app, cx), ["bookings", "daily"]);
    // Rows fill the list, so selection and labels reach its right edge.
    let (list, row) = app.update(cx, |window, _| {
        (
            bounds_of(window, "sql-completion"),
            bounds_of(window, "sql-completion-item-bookings"),
        )
    });
    assert!(
        list.right() - row.right() < gpui_kit::px(8.),
        "{list:?} {row:?}"
    );
    // The rows show all there is to know, so there is no details box.
    assert!(!app.update(cx, |window, _| {
        window.try_find("sql-completion-details").is_some()
    }));
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
    // The statement names the table after the cursor.
    app.type_sql(cx, "SELECT b.am FROM avia.bookings b");
    app.update(cx, |window, cx| {
        for _ in 0.." FROM avia.bookings b".len() {
            window.press("left", cx);
        }
    });
    app.press(cx, "tab");
    app.wait_for(cx, "sql-completion");
    assert_eq!(items(&app, cx), ["amount"]);
    app.wait_for(cx, "sql-completion-details");
    // The box adds only what the row does not show: the comment.
    assert_eq!(
        app.update(cx, |window, _| label(window, "sql-completion-details")),
        Some("Total fare.".into())
    );
    app.press(cx, "tab");
    app.wait_gone(cx, "sql-completion");
    assert_sql(&app, cx, "SELECT b.amount FROM avia.bookings b");
}

#[gpui_kit::test]
fn the_details_box_shows_the_start_of_a_long_description_as_markdown(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    app.type_sql(cx, "SELECT b.rou FROM avia.bookings b");
    app.update(cx, |window, cx| {
        for _ in 0.." FROM avia.bookings b".len() {
            window.press("left", cx);
        }
    });
    app.press(cx, "tab");
    app.wait_for(cx, "sql-completion-description");
    assert_eq!(items(&app, cx), ["route"]);
    let text = app
        .update(cx, |window, _| label(window, "sql-completion-details"))
        .unwrap();
    // The box has the start of the first paragraph, cut at a word.
    assert!(
        text.starts_with("The **route** of a booking: the `origin` and the `destination`"),
        "{text}"
    );
    assert!(text.ends_with('…'), "{text}");
    assert!(text.chars().count() <= 141, "{text}");
    assert!(!text.contains("Filter"), "{text}");
    app.update(cx, |window, _| {
        let details = bounds_of(window, "sql-completion-details");
        let description = bounds_of(window, "sql-completion-description");
        // The Markdown wraps inside the box, in a few lines.
        assert!(
            description.right() <= details.right(),
            "{details:?} {description:?}"
        );
        assert!(
            description.size.height > window.rem_size() * 2.,
            "{description:?}"
        );
        assert!(
            description.size.height <= gpui_kit::px(120.),
            "{description:?}"
        );
    });
}

#[gpui_kit::test]
fn typing_past_the_name_or_leaving_the_editor_closes_the_popup(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    complete(&app, cx, "SELECT * FROM avia.b");
    app.update(cx, |window, cx| window.input(" ", cx));
    app.wait_gone(cx, "sql-completion");

    complete(&app, cx, "SELECT * FROM avia.b");
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
    // Tab inserts a name; Refresh reads the schema again.
    app.click(cx, "sql-completion-footer-action");
    app.wait_until(cx, "the refresh of avia", WAIT, |_, _| true);
    let activity = app.activity(cx, profile.id);
    assert!(
        activity.contains("Started a metadata refresh of schema avia"),
        "{activity}"
    );
}

#[gpui_kit::test]
fn a_stale_schema_shows_only_when_the_list_has_its_names(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, Some(1), &[]);
    complete(&app, cx, "SELECT * FROM fin");
    assert_eq!(items(&app, cx), ["finance"]);
    assert!(app.update(cx, |window, _| {
        window.try_find("sql-completion-footer").is_none()
    }));
}

#[gpui_kit::test]
fn the_refresh_of_a_stale_schema_says_loading(cx: &mut TestAppContext) {
    let (app, _listener) = launch_held(cx, Some(1));
    complete(&app, cx, "SELECT * FROM avia.");
    app.click(cx, "sql-completion-footer-action");
    app.wait_until(cx, "the refresh", WAIT, |window, _| {
        label(window, "sql-completion-footer").as_deref() == Some("Loading metadata for avia")
    });
}

/// The text of `part` of the suggestion toast, when one shows.
fn toast_part(window: &mut gpui_kit::Window, part: &'static str) -> Option<String> {
    window.try_find("sql-completion-toast")?;
    window
        .within("sql-completion-toast")
        .try_find(part)
        .and_then(|part| part.label().map(str::to_owned))
}

/// The title of the suggestion toast, when one shows.
fn toast_title(window: &mut gpui_kit::Window) -> Option<String> {
    toast_part(window, "title")
}

#[gpui_kit::test]
fn a_failed_load_closes_the_list_and_shows_a_toast_with_retry(cx: &mut TestAppContext) {
    let (app, profile) = launch(cx, fresh(), &[]);
    app.type_sql(cx, "SELECT d. FROM avia.daily d");
    app.press(cx, "tab");
    app.wait_until(cx, "the failed load", WAIT, |window, _| {
        toast_title(window).as_deref() == Some("Could not load metadata for")
    });
    assert!(app.update(cx, |window, _| {
        window.try_find("sql-completion").is_none()
            && window.try_find("sql-completion-retry").is_some()
            && toast_part(window, "name").as_deref() == Some("daily")
            // The error of the read is in Activity, not in the toast.
            && toast_part(window, "detail").is_none()
    }));
    // Tab reads again, and the read fails again.
    app.press(cx, "tab");
    app.settle(cx);
    app.wait_until(cx, "the second failed load", WAIT, |window, _| {
        window.try_find("sql-completion").is_none() && toast_title(window).is_some()
    });
    let activity = app.activity(cx, profile.id);
    assert!(activity.matches("avia.daily").count() >= 2, "{activity}");
}

#[gpui_kit::test]
fn a_connection_without_cached_metadata_shows_a_toast_with_refresh(cx: &mut TestAppContext) {
    let (app, _listener) = launch_held(cx, None);
    app.type_sql(cx, "SELECT * FROM ");
    app.press(cx, "tab");
    app.wait_until(cx, "the toast", WAIT, |window, _| {
        toast_title(window).as_deref() == Some("No cached metadata")
    });
    assert!(app.update(cx, |window, _| window.try_find("sql-completion").is_none()));
    // Refresh reads the connection and shows the list at the cursor again.
    app.click(cx, "sql-completion-refresh");
    app.wait_until(cx, "the read", WAIT, |window, _| {
        label(window, "sql-completion-loading").as_deref() == Some("Loading metadata…")
            && toast_title(window).is_none()
    });
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

/// Saves the table `avia.wide` with `columns`, read now, and opens the
/// list of its columns after `SELECT`.
fn complete_wide(cx: &mut TestAppContext, columns: Vec<CatalogColumn>) -> TestApp {
    let profile = profile();
    let directory = tempfile::tempdir().unwrap();
    let read_at = qrow::catalog::now();
    let mut catalog = Catalog::new(&profile);
    catalog.apply_schemas(vec!["avia".into()], &CatalogSettings::default(), read_at);
    let wide = RelationEntry {
        name: "wide".into(),
        kind: RelationKind::Table,
        comment: None,
    };
    catalog.apply_relations("avia", None, vec![wide], read_at);
    catalog.apply_columns(
        "avia",
        Some("wide"),
        BTreeMap::from([("wide".into(), columns)]),
        read_at,
    );
    let path = storage::catalog_cache_path(directory.path(), profile.id);
    storage::save_catalog(&path, &catalog).unwrap();
    let app = TestApp::launch_in(
        cx,
        directory,
        Workspace {
            tabs: vec![SavedTab::new(1, Some(profile.id))],
            profiles: vec![profile],
            ..Workspace::default()
        },
        MemoryCredentials::default(),
    );
    let tail = " FROM avia.wide";
    app.type_sql(cx, &format!("SELECT {tail}"));
    app.update(cx, |window, cx| {
        for _ in 0..tail.len() {
            window.press("left", cx);
        }
    });
    app.press(cx, "tab");
    app.wait_for(cx, "sql-completion");
    app
}

#[gpui_kit::test]
fn a_long_type_is_cut_in_its_row_and_other_rows_keep_their_types(cx: &mut TestAppContext) {
    let long_name = "health_score_weighted_by_product_usage_and_support_volume";
    let long_type =
        "MAP<STRING,STRUCT<FIRST_SEEN_AT:TIMESTAMP,LAST_SEEN_AT:TIMESTAMP,SOURCE_SYSTEM:STRING>>";
    let app = complete_wide(
        cx,
        vec![
            column("snapshot_date", "DATE", None),
            column("attributes", long_type, None),
            column(long_name, "DECIMAL(5,2)", None),
        ],
    );
    let row = |name: &str| format!("sql-completion-item-{name}");
    app.update(cx, |window, _| {
        // A type that fits shows in full.
        let short = window.within(row("snapshot_date")).find("label").bounds();
        // The long type takes the room that its name leaves, cut.
        let attributes = bounds_of(window, &row("attributes"));
        let name = window.within(row("attributes")).find("name").bounds();
        let cut = window.within(row("attributes")).find("label").bounds();
        assert!(cut.left() > name.left(), "{name:?} {cut:?}");
        assert!(cut.right() <= attributes.right(), "{attributes:?} {cut:?}");
        assert!(cut.size.width > short.size.width, "{short:?} {cut:?}");
        // The long name leaves no room for its type.
        assert!(window.within(row(long_name)).try_find("label").is_none());
    });
    // A cut type shows in full on hover; a type that fits does not.
    assert!(hover_shows_tooltip(&app, cx, row("attributes")));
    assert!(!hover_shows_tooltip(&app, cx, row("snapshot_date")));
    // The box shows the full type of the selected row.
    let index = items(&app, cx)
        .iter()
        .position(|item| item == "attributes")
        .unwrap();
    for _ in 0..index {
        app.press(cx, "down");
    }
    app.wait_until(cx, "the full type in the box", WAIT, |window, _| {
        label(window, "sql-completion-details")
            .is_some_and(|text| text.eq_ignore_ascii_case(long_type))
    });
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
    app.press(cx, "tab");
    app.wait_for(cx, "sql-completion");
    assert_eq!(items(&app, cx), ["amount"]);
    // The columns of `daily` are not cached, and the connection is out of
    // reach, so their read fails.
    app.wait_until(cx, "the failed read", WAIT, |window, _| {
        label(window, "sql-completion-footer").as_deref()
            == Some("Could not load metadata for daily")
    });
    assert_eq!(
        app.update(cx, |window, _| label(
            window,
            "sql-completion-footer-action"
        )),
        Some("Retry".into())
    );
    // The footer does not show the error. Show Activity opens it.
    assert!(!hover_shows_tooltip(
        &app,
        cx,
        "sql-completion-footer-action"
    ));
    // The two buttons sit closer to each other than to the text.
    app.update(cx, |window, _| {
        let retry = bounds_of(window, "sql-completion-footer-action");
        let activity = bounds_of(window, "sql-completion-footer-activity");
        let text = bounds_of(window, "sql-completion-footer-text");
        assert_eq!(activity.left() - retry.right(), gpui_kit::px(4.));
        assert!(
            retry.left() - text.right() >= gpui_kit::px(8.),
            "{text:?} {retry:?}"
        );
    });
    app.click(cx, "sql-completion-footer-activity");
    app.wait_for(cx, "activity");
    assert!(app.update(cx, |window, _| window.try_find("sql-completion").is_none()));
}

#[gpui_kit::test]
fn show_activity_in_the_failed_read_toast_opens_activity(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    app.type_sql(cx, "SELECT d. FROM avia.daily d");
    app.press(cx, "tab");
    app.wait_for(cx, "sql-completion-activity");
    assert!(app.update(cx, |window, _| window.try_find("activity").is_none()));
    app.click(cx, "sql-completion-activity");
    app.wait_for(cx, "activity");
    app.wait_until(cx, "the toast closes", WAIT, |window, _| {
        toast_title(window).is_none()
    });
}

/// Moves the pointer over the name in the suggestion toast, and returns
/// whether GPUI opens a tooltip.
fn toast_name_shows_tooltip(app: &TestApp, cx: &mut TestAppContext) -> bool {
    app.update(cx, |window, cx| {
        let position = window
            .within("sql-completion-toast")
            .find("name")
            .bounds()
            .center();
        window.dispatch_event(
            gpui_kit::MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: Default::default(),
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    });
    cx.executor().advance_clock(Duration::from_millis(800));
    app.settle(cx);
    app.update(cx, |window, _| window.try_find("tooltip").is_some())
}

#[gpui_kit::test]
fn a_cut_name_in_the_failed_read_toast_shows_in_full_on_hover(cx: &mut TestAppContext) {
    let long = "customer_lifetime_value_by_acquisition_channel_and_region_v2";
    let (app, _) = launch(cx, fresh(), &[long]);
    app.type_sql(cx, &format!("SELECT d. FROM avia.{long} d"));
    app.press(cx, "tab");
    app.wait_until(cx, "the toast", WAIT, |window, _| {
        toast_part(window, "name").as_deref() == Some(long)
    });
    app.update(cx, |window, _| {
        let toast = bounds_of(window, "sql-completion-toast");
        let name = window.within("sql-completion-toast").find("name").bounds();
        assert!(name.right() <= toast.right(), "{toast:?} {name:?}");
    });
    assert!(toast_name_shows_tooltip(&app, cx));
}

#[gpui_kit::test]
fn a_name_that_fits_in_the_toast_has_no_tooltip(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    app.type_sql(cx, "SELECT d. FROM avia.daily d");
    app.press(cx, "tab");
    app.wait_until(cx, "the toast", WAIT, |window, _| {
        toast_part(window, "name").as_deref() == Some("daily")
    });
    assert!(!toast_name_shows_tooltip(&app, cx));
}

/// Hovers `id` until GPUI opens its tooltip, and returns whether it did.
fn hover_shows_tooltip(app: &TestApp, cx: &mut TestAppContext, id: impl Into<ElementId>) -> bool {
    app.hover(cx, id);
    cx.executor().advance_clock(Duration::from_millis(800));
    app.settle(cx);
    app.update(cx, |window, _| window.try_find("tooltip").is_some())
}

#[gpui_kit::test]
fn a_cut_table_name_in_the_footer_shows_in_full_on_hover(cx: &mut TestAppContext) {
    let long = "customer_lifetime_value_by_acquisition_channel_and_region_v2";
    let (app, _) = launch(cx, fresh(), &[long]);
    let tail = format!(" FROM avia.bookings b JOIN avia.{long} d");
    app.type_sql(cx, &format!("SELECT am{tail}"));
    app.update(cx, |window, cx| {
        for _ in 0..tail.len() {
            window.press("left", cx);
        }
    });
    app.press(cx, "tab");
    let message = format!("Could not load metadata for {long}");
    app.wait_until(cx, "the failed read", WAIT, |window, _| {
        label(window, "sql-completion-footer").as_deref() == Some(message.as_str())
    });
    app.update(cx, |window, _| {
        let footer = bounds_of(window, "sql-completion-footer");
        let text = bounds_of(window, "sql-completion-footer-text");
        assert!(text.right() <= footer.right(), "{footer:?} {text:?}");
    });
    assert!(hover_shows_tooltip(&app, cx, "sql-completion-footer-text"));
}

#[gpui_kit::test]
fn the_loading_row_draws_its_text_at_the_measured_size(cx: &mut TestAppContext) {
    let (app, _listener) = launch_held(cx, fresh());
    complete(&app, cx, "SELECT d. FROM avia.daily d");
    app.wait_for(cx, "sql-completion-loading-text");
    assert_eq!(
        app.update(cx, |window, _| label(window, "sql-completion-loading")),
        Some("Loading metadata for avia.daily".into())
    );
    let loading = app.update(cx, |window, _| {
        bounds_of(window, "sql-completion-loading-text").size.height
    });
    // The footer draws the same kind of text at the size that both measure.
    let (app, _) = launch(cx, Some(1), &[]);
    complete(&app, cx, "SELECT * FROM avia.");
    app.wait_for(cx, "sql-completion-footer-text");
    let footer = app.update(cx, |window, _| {
        bounds_of(window, "sql-completion-footer-text").size.height
    });
    assert_eq!(loading, footer);
}

#[gpui_kit::test]
fn a_footer_that_fits_has_no_tooltip(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, Some(1), &[]);
    complete(&app, cx, "SELECT * FROM avia.");
    app.wait_for(cx, "sql-completion-footer");
    assert!(!hover_shows_tooltip(&app, cx, "sql-completion-footer-text"));
    // The footer names the one stale schema, so Refresh has no tooltip.
    assert!(!hover_shows_tooltip(
        &app,
        cx,
        "sql-completion-footer-action"
    ));
}

#[gpui_kit::test]
fn the_footer_lines_up_with_the_rows_and_its_button_has_even_space(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, Some(1), &[]);
    complete(&app, cx, "SELECT * FROM avia.");
    app.wait_for(cx, "sql-completion-footer-action");
    app.update(cx, |window, _| {
        let row = "sql-completion-item-bookings";
        let icon = window.within(row).find("icon").bounds();
        let name = window.within(row).find("name").bounds();
        let slot = window.within("sql-completion-footer").find("slot").bounds();
        let footer = bounds_of(window, "sql-completion-footer");
        // The message starts with the name of the schema, in the code font.
        let text = bounds_of(window, "sql-completion-footer-text");
        let button = bounds_of(window, "sql-completion-footer-action");
        assert_eq!(slot.left(), icon.left(), "{slot:?} {icon:?}");
        assert_eq!(text.left(), name.left(), "{text:?} {name:?}");
        // Below the border at the top of the footer.
        let above = button.top() - footer.top() - gpui_kit::px(1.);
        let below = footer.bottom() - button.bottom();
        let right = footer.right() - button.right();
        assert_eq!(above, below, "{footer:?} {button:?}");
        assert_eq!(above, right, "{footer:?} {button:?}");
        assert!(above >= gpui_kit::px(6.), "{footer:?} {button:?}");
    });
}

#[gpui_kit::test]
fn tab_after_a_name_opens_the_list_and_tab_inserts(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    app.type_sql(cx, "SELECT * FROM avia.bo");
    // ⌃Space is not a shortcut of the list.
    app.press(cx, "ctrl-space");
    app.settle(cx);
    assert!(app.update(cx, |window, _| window.try_find("sql-completion").is_none()));
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
fn autocomplete_turned_off_closes_the_list_and_tab_indents(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    assert!(app.saved().settings.editor_autocomplete);
    complete(&app, cx, "SELECT * FROM avia.bo");
    app.wait_for(cx, "sql-completion");
    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "autocomplete");
    app.wait_for(cx, "editor-autocomplete");
    app.click(cx, "editor-autocomplete");
    app.wait_saved(cx, "autocomplete off", |workspace| {
        !workspace.settings.editor_autocomplete
    });
    app.wait_gone(cx, "sql-completion");
    app.press(cx, "secondary-w");
    app.wait_gone(cx, "reset-settings");

    let sql = "SELECT * FROM avia.bo";
    app.type_sql(cx, sql);
    app.press(cx, "tab");
    app.wait_until(cx, "the indent", WAIT, |_, _| {
        let saved = app.saved().tabs[0].sql.clone();
        saved.len() > sql.len() && saved.contains(sql)
    });
    assert!(app.update(cx, |window, _| window.try_find("sql-completion").is_none()));

    // Reset turns it on again.
    app.dispatch(cx, OpenSettings);
    app.wait_for(cx, "reset-settings");
    app.click(cx, "reset-settings");
    app.wait_saved(cx, "autocomplete on", |workspace| {
        workspace.settings.editor_autocomplete
    });
}

#[gpui_kit::test]
fn tab_after_where_shows_the_columns_of_the_statement(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    complete(&app, cx, "SELECT * FROM avia.bookings\nWHERE ");
    assert_eq!(items(&app, cx), ["booking_id", "amount", "route"]);
    app.press(cx, "enter");
    assert_sql(&app, cx, "SELECT * FROM avia.bookings\nWHERE booking_id");
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

#[gpui_kit::test]
fn a_failed_refresh_from_the_toast_shows_its_error(cx: &mut TestAppContext) {
    // The connection has no password, so the read fails at once.
    let (app, profile) = launch(cx, None, &[]);
    app.type_sql(cx, "SELECT * FROM ");
    app.press(cx, "tab");
    app.wait_until(cx, "the toast", WAIT, |window, _| {
        toast_title(window).as_deref() == Some("No cached metadata")
    });
    app.click(cx, "sql-completion-refresh");
    app.wait_until(cx, "the error", WAIT, |window, _| {
        window.try_find("sql-completion").is_none()
            && toast_title(window).as_deref() == Some("Could not load metadata")
            && window.try_find("sql-completion-retry").is_some()
    });
    // Tab reads the connection again, and the read fails again.
    app.press(cx, "tab");
    app.settle(cx);
    app.wait_until(cx, "the second error", WAIT, |window, _| {
        window.try_find("sql-completion").is_none() && toast_title(window).is_some()
    });
    // Retry in the toast reads it once more.
    app.click(cx, "sql-completion-retry");
    app.settle(cx);
    app.wait_until(cx, "the third error", WAIT, |window, _| {
        window.try_find("sql-completion").is_none() && toast_title(window).is_some()
    });
    let activity = app.activity(cx, profile.id);
    let reads = activity
        .matches("Started a metadata refresh of the connection")
        .count();
    assert_eq!(reads, 3, "{activity}");
}

#[gpui_kit::test]
fn autocomplete_off_loads_no_metadata_and_the_first_tab_after_it_turns_on_suggests(
    cx: &mut TestAppContext,
) {
    let profile = profile();
    let directory = tempfile::tempdir().unwrap();
    cache(&directory, &profile, qrow::catalog::now(), &[]);
    let mut workspace = Workspace {
        tabs: vec![SavedTab::new(1, Some(profile.id))],
        profiles: vec![profile.clone()],
        ..Workspace::default()
    };
    workspace.settings.editor_autocomplete = false;
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    let holds = |app: &TestApp, cx: &mut TestAppContext| {
        app.qrow
            .update(cx, |qrow, _| qrow.holds_catalog(profile.id))
            .unwrap()
    };
    // Focus and edits of the editor load nothing while Autocomplete is off.
    app.type_sql(cx, "SELECT * FROM avia.bo");
    app.settle(cx);
    assert!(!holds(&app, cx));

    app.dispatch(cx, OpenSettings);
    app.fill_labelled(cx, "Search...", "autocomplete");
    app.wait_for(cx, "editor-autocomplete");
    app.click(cx, "editor-autocomplete");
    app.wait_saved(cx, "autocomplete on", |workspace| {
        workspace.settings.editor_autocomplete
    });
    app.press(cx, "secondary-w");
    app.wait_gone(cx, "reset-settings");

    // The first Tab loads the cache. The list can show its loading row
    // first, then the names.
    app.type_sql(cx, "SELECT * FROM avia.bo");
    app.press(cx, "tab");
    app.wait_for(cx, "sql-completion");
    app.wait_until(cx, "the cached names", WAIT, |window, _| {
        labelled(window, "bookings").is_some()
    });
    assert_eq!(items(&app, cx), ["bookings"]);
    assert!(holds(&app, cx));

    // With Autocomplete on, a focus of the editor loads the cache before
    // the first Tab.
    let (app, profile) = launch(cx, fresh(), &[]);
    app.click(cx, "sql-editor");
    app.settle(cx);
    assert!(
        app.qrow
            .update(cx, |qrow, _| qrow.holds_catalog(profile.id))
            .unwrap()
    );
}

/// Moves the cursor to the start of the editor text.
fn move_to_start(app: &TestApp, cx: &mut TestAppContext) {
    app.press(
        cx,
        if cfg!(target_os = "macos") {
            "cmd-up"
        } else {
            "ctrl-home"
        },
    );
}

/// Moves the cursor to the end of the editor text.
fn move_to_end(app: &TestApp, cx: &mut TestAppContext) {
    app.press(
        cx,
        if cfg!(target_os = "macos") {
            "cmd-down"
        } else {
            "ctrl-end"
        },
    );
}

#[gpui_kit::test]
fn the_list_follows_edits_after_other_statements_and_closes_away_from_the_name(
    cx: &mut TestAppContext,
) {
    let (app, _) = launch(cx, fresh(), &[]);
    // Statements before the cursor, with `;` in a string and in a comment.
    let head = "SELECT 'a;b' AS x; -- c;\nSELECT 1 /* ; */;\n";
    complete(&app, cx, &format!("{head}SELECT * FROM avia."));
    assert_eq!(items(&app, cx), ["bookings", "daily"]);
    // Typing filters the list, and Backspace widens it again.
    app.update(cx, |window, cx| window.input("b", cx));
    app.wait_until(cx, "the filtered list", WAIT, |window, _| {
        labelled(window, "daily").is_none()
    });
    assert_eq!(items(&app, cx), ["bookings"]);
    app.press(cx, "backspace");
    app.wait_until(cx, "the full list", WAIT, |window, _| {
        labelled(window, "daily").is_some()
    });
    // Down moves the selection, and the list stays.
    app.press(cx, "down");
    app.settle(cx);
    assert_eq!(items(&app, cx), ["bookings", "daily"]);
    app.press(cx, "enter");
    app.wait_gone(cx, "sql-completion");
    assert_sql(&app, cx, &format!("{head}SELECT * FROM avia.daily"));

    // A move of the cursor away from the name closes the list.
    app.press(cx, "backspace");
    app.press(cx, "tab");
    app.wait_for(cx, "sql-completion");
    app.press(cx, "home");
    app.wait_gone(cx, "sql-completion");
}

#[gpui_kit::test]
fn an_edit_above_the_cursor_changes_what_tab_reads(cx: &mut TestAppContext) {
    let (app, _) = launch(cx, fresh(), &[]);
    let sql = "SELECT 1;\nSELECT * FROM avia.b";
    complete(&app, cx, sql);
    assert_eq!(items(&app, cx), ["bookings"]);
    app.press(cx, "escape");
    app.wait_gone(cx, "sql-completion");
    // A quote at the start makes the rest of the SQL a string, so Tab at
    // the end indents.
    move_to_start(&app, cx);
    app.update(cx, |window, cx| window.input("'", cx));
    move_to_end(&app, cx);
    app.press(cx, "tab");
    app.wait_until(cx, "the indent", WAIT, |_, _| {
        let saved = app.saved().tabs[0].sql.clone();
        saved.starts_with('\'') && saved.len() > sql.len() + 1
    });
    assert!(app.update(cx, |window, _| window.try_find("sql-completion").is_none()));
    // Without the quote and the indent, Tab after the name opens the
    // list again.
    move_to_start(&app, cx);
    app.press(cx, "delete");
    move_to_end(&app, cx);
    for _ in 0..8 {
        if editor_text(&app, cx) == sql {
            break;
        }
        app.press(cx, "backspace");
    }
    assert_eq!(editor_text(&app, cx), sql);
    app.press(cx, "tab");
    app.wait_for(cx, "sql-completion");
    assert_eq!(items(&app, cx), ["bookings"]);
}

/// The text of the SQL editor.
fn editor_text(app: &TestApp, cx: &mut TestAppContext) -> String {
    app.update(cx, |window, _| {
        crate::support::assistant::editor_text(window)
    })
    .unwrap_or_default()
}

/// Saves the schema `avia` with `bookings` and the schema `actions` without
/// tables, both read at `read_at`, and launches.
fn launch_with_empty_schema(cx: &mut TestAppContext, read_at: u64) -> TestApp {
    let profile = profile();
    let directory = tempfile::tempdir().unwrap();
    let mut catalog = Catalog::new(&profile);
    catalog.apply_schemas(
        vec!["actions".into(), "avia".into()],
        &CatalogSettings::default(),
        read_at,
    );
    let bookings = RelationEntry {
        name: "bookings".into(),
        kind: RelationKind::Table,
        comment: None,
    };
    catalog.apply_relations("avia", None, vec![bookings], read_at);
    catalog.apply_relations("actions", None, Vec::new(), read_at);
    let path = storage::catalog_cache_path(directory.path(), profile.id);
    storage::save_catalog(&path, &catalog).unwrap();
    TestApp::launch_in(
        cx,
        directory,
        Workspace {
            tabs: vec![SavedTab::new(1, Some(profile.id))],
            profiles: vec![profile],
            ..Workspace::default()
        },
        MemoryCredentials::default(),
    )
}

#[gpui_kit::test]
fn tab_after_an_empty_schema_says_it_has_no_tables_instead_of_an_indent(cx: &mut TestAppContext) {
    let app = launch_with_empty_schema(cx, qrow::catalog::now());
    app.type_sql(cx, "SELECT * FROM actions.");
    app.press(cx, "tab");
    app.wait_for(cx, "sql-completion-empty-schema");
    assert_eq!(
        app.update(cx, |window, _| label(window, "sql-completion-empty-schema")),
        Some("No tables or views in actions".into())
    );
    // Tab did not indent, and fresh metadata has no footer.
    assert_sql(&app, cx, "SELECT * FROM actions.");
    assert!(app.update(cx, |window, _| {
        window.try_find("sql-completion-footer").is_none()
    }));
    // The schema lines up with the rows of other lists.
    app.update(cx, |window, _| {
        let list = bounds_of(window, "sql-completion");
        let icon = window
            .within("sql-completion-empty-schema")
            .find("icon")
            .bounds();
        assert_eq!(
            icon.left() - list.left(),
            gpui_kit::px(13.),
            "{list:?} {icon:?}"
        );
    });
    // Typing a name keeps the list open: the schema is still empty.
    app.update(cx, |window, cx| window.input("ev", cx));
    app.wait_for(cx, "sql-completion-empty-schema");
    app.press(cx, "escape");
    app.wait_gone(cx, "sql-completion");
}

#[gpui_kit::test]
fn a_stale_empty_schema_has_the_stale_footer(cx: &mut TestAppContext) {
    let app = launch_with_empty_schema(cx, 1);
    app.type_sql(cx, "SELECT * FROM actions.");
    app.press(cx, "tab");
    app.wait_for(cx, "sql-completion-empty-schema");
    app.wait_until(cx, "the stale footer", WAIT, |window, _| {
        label(window, "sql-completion-footer").as_deref() == Some("actions is stale")
    });
    assert_eq!(
        app.update(cx, |window, _| label(
            window,
            "sql-completion-footer-action"
        )),
        Some("Refresh".into())
    );
}

#[gpui_kit::test]
fn an_empty_schema_after_from_can_be_inserted(cx: &mut TestAppContext) {
    let app = launch_with_empty_schema(cx, qrow::catalog::now());
    complete(&app, cx, "SELECT * FROM ");
    assert_eq!(items(&app, cx), ["actions", "avia", "bookings"]);
    app.press(cx, "enter");
    assert_sql(&app, cx, "SELECT * FROM actions");
}
