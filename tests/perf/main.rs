//! Performance probes of the real window. Run with `./qtest run perf-ui`,
//! which builds them with the `perf` profile, like a release build.
#[path = "../support/mod.rs"]
mod support;

use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    InputEvent as _, Keystroke, ScrollDelta, ScrollWheelEvent, TestAppContext, point, px,
};
use qrow::model::{SavedTab, Workspace};
use std::time::Instant;
use support::assistant::FakeCodex;
use support::perf::{median_ms, report, sample};
use support::{MemoryCredentials, TestApp, cell, elements, offline_profile};

fn demo(cx: &mut TestAppContext) -> TestApp {
    let app = TestApp::launch_demo(cx);
    app.wait_until(
        cx,
        "the demo rows",
        std::time::Duration::from_secs(10),
        |window, _| cell(window, 0, 1).is_some(),
    );
    app
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn results_frame(cx: &mut TestAppContext) {
    // The demo tab shows 1,000 rows of 141 columns on its first page.
    let app = demo(cx);
    let samples = app.update(cx, |window, cx| sample(5, 40, || window.render_frame(cx)));
    report("ui.results.frame", median_ms(&samples), "ms", 50.);
}

fn scroll_frames(app: &TestApp, cx: &mut TestAppContext, delta: ScrollDelta) -> f64 {
    app.update(cx, |window, cx| {
        let column: gpui_kit::ElementId = ("cell", 1usize).into();
        let position = elements(window)
            .into_iter()
            .find(|element| element.visible() && element.path().last() == Some(&column))
            .expect("A visible result cell")
            .bounds()
            .center();
        let samples = sample(5, 40, || {
            window.dispatch_event(
                ScrollWheelEvent {
                    position,
                    delta,
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        });
        median_ms(&samples)
    })
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn results_scroll(cx: &mut TestAppContext) {
    let app = demo(cx);
    let down = scroll_frames(&app, cx, ScrollDelta::Pixels(point(px(0.), px(-120.))));
    report("ui.results.scroll_vertical", down, "ms", 50.);
    let right = scroll_frames(&app, cx, ScrollDelta::Pixels(point(px(-240.), px(0.))));
    report("ui.results.scroll_horizontal", right, "ms", 50.);
}

/// A workspace with one tab of 1 MB of SQL.
fn one_megabyte_of_sql() -> Workspace {
    let line = "SELECT route, COUNT(*) FROM flights WHERE note = '日本語; safe' GROUP BY route;\n";
    let sql = line.repeat(1_000_000 / line.len() + 1);
    let profile = offline_profile("Synthetic");
    let tab = SavedTab {
        sql,
        ..SavedTab::new(1, Some(profile.id))
    };
    Workspace {
        profiles: vec![profile],
        tabs: vec![tab],
        ..Workspace::default()
    }
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn editor_with_one_megabyte_of_sql(cx: &mut TestAppContext) {
    let workspace = one_megabyte_of_sql();
    let started = Instant::now();
    let app = TestApp::launch_with(cx, workspace, MemoryCredentials::default());
    report(
        "ui.editor.open_1mb",
        started.elapsed().as_secs_f64() * 1000.,
        "ms",
        1000.,
    );

    app.click(cx, "sql-editor");
    app.press(cx, "cmd-down");
    let samples = app.update(cx, |window, cx| sample(3, 20, || window.input("x", cx)));
    report("ui.editor.keystroke_1mb", median_ms(&samples), "ms", 50.);
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn assistant_transcript(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let workspace = codex.workspace(Workspace::default());
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_assistant(cx);
    // Each reply streams about 16 KB in 800 deltas.
    let samples: Vec<_> = (1..=3)
        .map(|reply| {
            let message = format!("Stream a long reply {reply}");
            app.type_message(cx, &message);
            let started = Instant::now();
            app.click(cx, "assistant-send");
            app.wait_reply(cx, &format!("End of {message}"));
            started.elapsed()
        })
        .collect();
    report(
        "ui.assistant.stream_reply",
        median_ms(&samples),
        "ms",
        5000.,
    );
    app.wait_idle(cx);
    let samples = app.update(cx, |window, cx| sample(5, 40, || window.render_frame(cx)));
    report("ui.assistant.frame", median_ms(&samples), "ms", 50.);
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn assistant_composer_keystroke(cx: &mut TestAppContext) {
    // A tab with 1 MB of SQL and two long replies are on screen. Each sample
    // types one character into the message field and draws the next frame
    // like the window does: only the views that changed render again.
    let (directory, codex) = FakeCodex::new();
    let workspace = codex.workspace(one_megabyte_of_sql());
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_assistant(cx);
    for reply in 1..=2 {
        let message = format!("Stream a long reply {reply}");
        app.send(cx, &message);
        app.wait_reply(cx, &format!("End of {message}"));
    }
    app.wait_idle(cx);
    app.click(cx, "assistant-composer");
    let samples = app.update(cx, |window, cx| {
        window.draw(cx).clear(cx);
        sample(5, 40, || {
            let mut key = Keystroke::parse("x").expect("x is a keystroke");
            key.key_char = Some("x".into());
            window.dispatch_keystroke(key, cx);
            window.draw(cx).clear(cx);
        })
    });
    report("ui.assistant.keystroke", median_ms(&samples), "ms", 50.);
}

/// A cached catalog of two schemas with 1,000 tables of 40 columns each, with
/// one schema and one table expanded.
fn large_catalog(cx: &mut TestAppContext) -> TestApp {
    use qrow::catalog::{Catalog, CatalogColumn, RelationEntry, RelationKind};
    let profile = offline_profile("Warehouse");
    let directory = tempfile::tempdir().unwrap();
    let mut catalog = Catalog::new(&profile);
    let schemas = ["events", "sales"];
    catalog.apply_schemas(
        schemas.iter().map(|s| (*s).into()).collect(),
        &Default::default(),
        1,
    );
    for schema in schemas {
        let names: Vec<String> = (0..1000)
            .map(|n| format!("{schema}_table_{n:04}"))
            .collect();
        catalog.apply_relations(
            schema,
            None,
            names
                .iter()
                .map(|name| RelationEntry {
                    name: name.clone(),
                    kind: RelationKind::Table,
                    comment: None,
                })
                .collect(),
            1,
        );
        let columns = names
            .into_iter()
            .map(|name| {
                let columns = (0..40)
                    .map(|n| CatalogColumn {
                        name: format!("column_{n:02}"),
                        data_type: "STRING".into(),
                        comment: None,
                    })
                    .collect();
                (name, columns)
            })
            .collect();
        catalog.apply_columns(schema, None, columns, 1);
    }
    let path = qrow::storage::catalog_path(&directory.path().join("workspace.json"), profile.id);
    qrow::storage::save_catalog(&path, &catalog).unwrap();
    let workspace = Workspace {
        tabs: vec![SavedTab::new(1, Some(profile.id))],
        profiles: vec![profile.clone()],
        ..Workspace::default()
    };
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.toggle_connection(cx, profile.id);
    for row in ["sales", "sales_table_0000"] {
        app.click_labelled(cx, row);
    }
    app.wait_until(
        cx,
        "the expanded table",
        std::time::Duration::from_secs(10),
        |window, _| support::labelled(window, "column_00 STRING").is_some(),
    );
    app
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn catalog_tree(cx: &mut TestAppContext) {
    let app = large_catalog(cx);
    let frame = app.update(cx, |window, cx| sample(5, 40, || window.render_frame(cx)));
    report("ui.catalog.frame", median_ms(&frame), "ms", 50.);

    let scroll = app.update(cx, |window, cx| {
        let position = support::labelled(window, "column_00 STRING")
            .expect("A visible column row")
            .bounds()
            .center();
        let samples = sample(5, 40, || {
            window.dispatch_event(
                ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
        });
        median_ms(&samples)
    });
    report("ui.catalog.scroll", scroll, "ms", 50.);

    // Each search keystroke rebuilds the tree from 2,000 cached tables.
    app.click_labelled(cx, "Search tables");
    let mut toggle = false;
    let search = app.update(cx, |window, cx| {
        let samples = sample(3, 20, || {
            toggle = !toggle;
            window.press(if toggle { "7" } else { "backspace" }, cx);
            window.render_frame(cx);
        });
        median_ms(&samples)
    });
    report("ui.catalog.search_keystroke", search, "ms", 50.);
}

/// The `n`th entry of a busy connection: schema refresh requests, with a
/// failed request and its stack trace in each 50 entries.
fn activity_entry(n: usize) -> qrow::activity::ActivityEntry {
    use qrow::{activity::ActivityEntry, logs::Severity};
    if n % 50 == 49 {
        let trace: String = (0..12)
            .map(|line| {
                format!(
                    "\n\tat org.apache.hive.service.cli.Operation.run{line}(Operation.java:{line})"
                )
            })
            .collect();
        ActivityEntry::new(
            Severity::Error,
            format!(
                "List columns of all relations in schema_{n:05} failed: Could not initialize the session{trace}"
            ),
        )
    } else {
        ActivityEntry::new(
            Severity::Info,
            format!("List relations in schema_{n:05}: 297 relations (client measurement: 1.22 s)"),
        )
    }
}

/// Qrow with one connection whose Activity is full: it holds the most
/// entries that a log keeps.
fn full_activity(cx: &mut TestAppContext) -> (TestApp, uuid::Uuid) {
    let profile = offline_profile("Warehouse");
    let id = profile.id;
    let workspace = Workspace {
        tabs: vec![SavedTab::new(1, Some(id))],
        profiles: vec![profile],
        ..Workspace::default()
    };
    let app = TestApp::launch_with(cx, workspace, MemoryCredentials::default());
    app.qrow
        .update(cx, |qrow, cx| {
            for n in 0..qrow::activity::MAX_ENTRIES {
                qrow.record_activity(id, activity_entry(n), cx);
            }
        })
        .expect("The window is open");
    app.settle(cx);
    (app, id)
}

/// Opens Activity, or closes it, and draws the next frame.
fn toggle_activity(app: &TestApp, cx: &mut TestAppContext) {
    app.update(cx, |window, cx| {
        window.press("cmd-shift-u", cx);
        window.render_frame(cx);
    });
}

// In test builds, `test_support` on a row keeps the selection state of each
// row that a frame drew until Activity closes, and each frame then does more
// work. The app does not do this. Probes that draw new rows in each sample
// open Activity again before each sample or each few samples.

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn activity(cx: &mut TestAppContext) {
    let (app, id) = full_activity(cx);
    let qrow = app.qrow.upgrade().expect("The window is open");
    let mut next = qrow::activity::MAX_ENTRIES;

    // Open: the rows of the full log and the first frame.
    let open: Vec<_> = (0..13)
        .map(|_| {
            let started = Instant::now();
            toggle_activity(&app, cx);
            let elapsed = started.elapsed();
            toggle_activity(&app, cx);
            elapsed
        })
        .skip(3)
        .collect();
    report("ui.activity.open_50k", median_ms(&open), "ms", 100.);

    toggle_activity(&app, cx);
    app.wait_for(cx, "activity");
    let frame = app.update(cx, |window, cx| sample(5, 40, || window.render_frame(cx)));
    report("ui.activity.frame", median_ms(&frame), "ms", 50.);

    // Each new entry while the log follows its end: a schema refresh sends
    // one for each request.
    let append = app.update(cx, |window, cx| {
        sample(5, 40, || {
            qrow.update(cx, |qrow, cx| {
                qrow.record_activity(id, activity_entry(next), cx)
            });
            next += 1;
            window.render_frame(cx);
        })
    });
    report("ui.activity.append", median_ms(&append), "ms", 50.);

    // A full log removes its oldest entries in one step. The slowest of
    // enough appends for one more step includes it.
    let slowest = app.update(cx, |_, cx| {
        let records = sample(0, qrow::activity::MAX_ENTRIES / 10 + 100, || {
            qrow.update(cx, |qrow, cx| {
                qrow.record_activity(id, activity_entry(next), cx)
            });
            next += 1;
        });
        records.into_iter().max().unwrap_or_default()
    });
    report("ui.activity.trim", slowest.as_secs_f64() * 1000., "ms", 50.);

    // Scroll up from the end, five wheel steps after each opening.
    let mut scroll = Vec::new();
    for round in 0..9 {
        toggle_activity(&app, cx);
        toggle_activity(&app, cx);
        let samples = app.update(cx, |window, cx| {
            let position = elements(window)
                .into_iter()
                .find(|element| {
                    element.visible()
                        && element
                            .path()
                            .last()
                            .is_some_and(|id| format!("{id:?}").contains("activity-entry"))
                })
                .expect("A visible Activity row")
                .bounds()
                .center();
            sample(0, 5, || {
                window.dispatch_event(
                    ScrollWheelEvent {
                        position,
                        delta: ScrollDelta::Pixels(point(px(0.), px(120.))),
                        ..Default::default()
                    }
                    .to_platform_input(),
                    cx,
                );
                window.render_frame(cx);
            })
        });
        // The first round warms up.
        if round > 0 {
            scroll.extend(samples);
        }
    }
    report("ui.activity.scroll", median_ms(&scroll), "ms", 50.);

    // Errors only and All activity filter the full log again.
    let filter: Vec<_> = (0..22)
        .map(|round| {
            toggle_activity(&app, cx);
            toggle_activity(&app, cx);
            let button = if round % 2 == 0 {
                "activity-errors"
            } else {
                "activity-all"
            };
            if round % 2 == 1 {
                app.update(cx, |window, cx| {
                    window.click("activity-errors", cx);
                    window.render_frame(cx);
                });
                toggle_activity(&app, cx);
                toggle_activity(&app, cx);
            }
            app.update(cx, |window, cx| {
                let started = Instant::now();
                window.click(button, cx);
                window.render_frame(cx);
                started.elapsed()
            })
        })
        .skip(2)
        .collect();
    report("ui.activity.filter", median_ms(&filter), "ms", 100.);

    // While Activity is closed, a busy refresh must not slow the window.
    toggle_activity(&app, cx);
    app.wait_gone(cx, "activity");
    let background = app.update(cx, |window, cx| {
        sample(3, 20, || {
            qrow.update(cx, |qrow, cx| {
                for _ in 0..100 {
                    qrow.record_activity(id, activity_entry(next), cx);
                    next += 1;
                }
            });
            window.render_frame(cx);
        })
    });
    report(
        "ui.activity.closed_100_entries",
        median_ms(&background),
        "ms",
        50.,
    );
}
