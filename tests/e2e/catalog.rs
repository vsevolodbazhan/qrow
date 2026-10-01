use crate::support::fixture::{Kyuubi, QUERY_TIMEOUT};
use crate::support::{TestApp, labelled};
use gpui_kit::TestAppContext;

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn a_live_session_loads_the_tree_and_a_refresh_shows_a_new_column(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let schema = format!("qrow_tree_{}", uuid::Uuid::new_v4().simple());
    let (mut workspace, credentials) =
        kyuubi.workspace("SELECT 1", crate::support::fixture::PASSWORD);
    // Other tests make schemas too. The filter keeps the tree to this one.
    workspace.profiles[0].catalog.include = vec![schema.clone()];
    let profile = workspace.profiles[0].clone();
    let app = TestApp::launch_with(cx, workspace, credentials);

    app.run_complete(cx, &format!("CREATE DATABASE {schema}"));
    app.run_complete(
        cx,
        &format!("CREATE TABLE {schema}.bookings (id BIGINT, gate STRING) USING parquet"),
    );

    // The tab has a live session, so expansion reads the catalog by itself.
    app.toggle_connection(cx, profile.id);
    app.wait_until(cx, "the schema", QUERY_TIMEOUT, |window, _| {
        labelled(window, &schema).is_some()
    });
    app.click_labelled(cx, &schema);
    app.wait_until(cx, "the table", QUERY_TIMEOUT, |window, _| {
        labelled(window, "bookings").is_some()
    });
    app.click_labelled(cx, "bookings");
    app.wait_until(cx, "the columns", QUERY_TIMEOUT, |window, _| {
        labelled(window, "gate STRING").is_some()
    });

    app.run_complete(
        cx,
        &format!("ALTER TABLE {schema}.bookings ADD COLUMNS (fare DOUBLE)"),
    );
    app.context_menu_labelled(cx, "bookings");
    app.choose(cx, "popup-menu", "Refresh");
    app.wait_until(cx, "the new column", QUERY_TIMEOUT, |window, _| {
        labelled(window, "fare DOUBLE").is_some()
    });

    app.run_complete(cx, &format!("DROP DATABASE {schema} CASCADE"));
}
