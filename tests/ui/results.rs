//! The result table of the demo tab.
use crate::support::{TestApp, cell, elements};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    ElementId, InputEvent as _, ScrollDelta, ScrollWheelEvent, TestAppContext, point, px,
};
use std::collections::BTreeSet;
use std::time::Duration;

/// The columns of the visible elements with the ID `(name, column)`.
fn visible_columns(app: &TestApp, cx: &mut TestAppContext, name: &str) -> BTreeSet<usize> {
    app.update(cx, |window, _| {
        elements(window)
            .into_iter()
            .filter(|element| element.visible())
            .filter_map(|element| match element.path().last() {
                Some(ElementId::NamedInteger(id, column)) if id.as_ref() == name => {
                    Some(*column as usize)
                }
                _ => None,
            })
            .collect()
    })
}

#[gpui_kit::test]
fn fast_horizontal_scroll_keeps_the_column_headers(cx: &mut TestAppContext) {
    let app = TestApp::launch_demo(cx);
    app.wait_until(cx, "the demo rows", Duration::from_secs(10), |window, _| {
        cell(window, 0, 1).is_some()
    });
    let position = app.update(cx, |window, _| {
        let column: ElementId = ("cell", 1usize).into();
        elements(window)
            .into_iter()
            .find(|element| element.visible() && element.path().last() == Some(&column))
            .expect("A visible result cell")
            .bounds()
            .center()
    });
    // A fast trackpad swipe moves the offset past the right end of the
    // table before layout clamps it. The header must not skip the columns
    // that the rows show.
    for dx in [-20000., -300., -300., -600.] {
        app.update(cx, |window, cx| {
            window.dispatch_event(
                ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(dx), px(0.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        });
        let cells = visible_columns(&app, cx, "cell");
        let headers = visible_columns(&app, cx, "column-header");
        assert!(
            cells.iter().filter(|&&c| c > 0).count() > 1,
            "No visible result columns: {cells:?}"
        );
        assert!(
            cells.is_subset(&headers),
            "Columns without a header: {:?}",
            cells.difference(&headers).collect::<Vec<_>>()
        );
    }
}
