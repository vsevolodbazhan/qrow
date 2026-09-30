use crate::support::fixture::{Kyuubi, PASSWORD, QUERY_TIMEOUT};
use crate::support::{TestApp, cell, header};
use gpui_kit::TestAppContext;
use gpui_kit::test::TestWindowExt;

#[gpui_kit::test]
#[ignore = "needs the server fixture: ./qtest run e2e"]
fn query_rows_reach_the_results_table(cx: &mut TestAppContext) {
    let kyuubi = Kyuubi::get();
    let (workspace, credentials) = kyuubi.workspace(
        "SELECT id, CONCAT('row-', CAST(id AS STRING)) AS label FROM range(3)",
        PASSWORD,
    );
    let app = TestApp::launch_with(cx, workspace, credentials);
    app.update(cx, |window, _| assert_eq!(cell(window, 0, 1), None));

    app.update(cx, |window, cx| window.click("run", cx));
    app.wait_until(cx, "the last result row", QUERY_TIMEOUT, |window, _| {
        cell(window, 2, 2).as_deref() == Some("row-2")
    });
    app.update(cx, |window, _| {
        assert_eq!(header(window, 1).as_deref(), Some("id"));
        assert_eq!(header(window, 2).as_deref(), Some("label"));
        let rows: Vec<_> = (0..3)
            .map(|row| (cell(window, row, 1), cell(window, row, 2)))
            .collect();
        assert_eq!(
            rows,
            [("0", "row-0"), ("1", "row-1"), ("2", "row-2")]
                .map(|(id, label)| (Some(id.to_owned()), Some(label.to_owned())))
        );
        assert_eq!(cell(window, 3, 1), None, "The query returned three rows");
    });
}
