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
    report(
        "ui.editor.keystroke_1mb",
        keystroke_frames(&app, cx),
        "ms",
        50.,
    );
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
    report(
        "ui.assistant.keystroke",
        keystroke_frames(&app, cx),
        "ms",
        50.,
    );
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
    let path = qrow::storage::catalog_cache_path(directory.path(), profile.id);
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
        |window, _| support::labelled(window, "column_00 string").is_some(),
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
        let position = support::labelled(window, "column_00 string")
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
    app.click_labelled(cx, "Search connections and relations");
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
                "List columns of all tables and views in schema_{n:05} failed: Could not initialize the session{trace}"
            ),
        )
    } else {
        ActivityEntry::new(
            Severity::Info,
            format!("List tables and views in schema_{n:05}: 297 tables and views (1.22 s)"),
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

/// A cached catalog of 100 schemas with 1,000 tables each, without columns,
/// like a large warehouse.
fn wide_catalog(cx: &mut TestAppContext) -> TestApp {
    use qrow::catalog::{Catalog, RelationEntry, RelationKind};
    let profile = offline_profile("Warehouse");
    let directory = tempfile::tempdir().unwrap();
    let mut catalog = Catalog::new(&profile);
    let schemas: Vec<String> = (0..100).map(|n| format!("schema_{n:03}")).collect();
    catalog.apply_schemas(schemas.clone(), &Default::default(), 1);
    for schema in &schemas {
        let relations = (0..1000)
            .map(|n| RelationEntry {
                name: format!("{schema}_table_{n:04}"),
                kind: RelationKind::Table,
                comment: None,
            })
            .collect();
        catalog.apply_relations(schema, None, relations, 1);
    }
    let path = qrow::storage::catalog_cache_path(directory.path(), profile.id);
    qrow::storage::save_catalog(&path, &catalog).unwrap();
    let workspace = Workspace {
        tabs: vec![SavedTab::new(1, Some(profile.id))],
        profiles: vec![profile.clone()],
        ..Workspace::default()
    };
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.toggle_connection(cx, profile.id);
    app.wait_until(
        cx,
        "the cached schemas",
        std::time::Duration::from_secs(10),
        |window, _| support::labelled(window, "schema_000").is_some(),
    );
    app
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn catalog_search_100k(cx: &mut TestAppContext) {
    search_keystrokes(cx, &["t", "a", "b", "l", "e"], "search");
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn catalog_qualified_search_miss_100k(cx: &mut TestAppContext) {
    // A qualified name that matches no relation reads every relation name.
    search_keystrokes(
        cx,
        &["m", "i", "s", "s", ".", "`", "t", "a", "b", "l", "e"],
        "qualified_miss",
    );
}

/// Types `search` into the search of the wide catalog, then measures more
/// keystrokes. Each sample types one character and draws the frame, then
/// lets the pending search work finish and draws the result. The first part
/// is the input latency; the sum is all the work of one keystroke.
fn search_keystrokes(cx: &mut TestAppContext, search: &[&str], metric: &str) {
    let app = wide_catalog(cx);
    app.click_labelled(cx, "Search connections and relations");
    app.update(cx, |window, cx| {
        for key in search {
            window.press(key, cx);
        }
        window.render_frame(cx);
    });
    let mut keystroke = Vec::new();
    let mut total = Vec::new();
    for round in 0..23 {
        let key = if round % 2 == 0 { "7" } else { "backspace" };
        let key = Keystroke::parse(key).expect("a keystroke");
        let started = Instant::now();
        app.update(cx, |window, cx| window.dispatch_keystroke(key, cx));
        app.update(cx, |window, cx| window.render_frame(cx));
        let typed = started.elapsed();
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(500));
        cx.run_until_parked();
        app.update(cx, |window, cx| window.render_frame(cx));
        if round >= 3 {
            keystroke.push(typed);
            total.push(started.elapsed());
        }
    }
    report(
        &format!("ui.catalog.{metric}_keystroke_100k"),
        median_ms(&keystroke),
        "ms",
        50.,
    );
    report(
        &format!("ui.catalog.{metric}_apply_100k"),
        median_ms(&total),
        "ms",
        50.,
    );
}

/// Types one character into the focused field and draws the next frame like
/// the window does: only the views that changed render again. Each
/// keystroke is its own update, so the timing includes the work of the
/// events that the change sends.
fn keystroke_frames(app: &TestApp, cx: &mut TestAppContext) -> f64 {
    app.update(cx, |window, cx| window.draw(cx).clear(cx));
    let samples: Vec<_> = (0..45)
        .map(|_| {
            let mut key = Keystroke::parse("x").expect("x is a keystroke");
            key.key_char = Some("x".into());
            let started = Instant::now();
            app.update(cx, |window, cx| window.dispatch_keystroke(key, cx));
            app.update(cx, |window, cx| window.draw(cx).clear(cx));
            started.elapsed()
        })
        .skip(5)
        .collect();
    median_ms(&samples)
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn editor_keystroke_frame(cx: &mut TestAppContext) {
    // The demo shows the catalog and 1,000 rows of 141 columns.
    let app = demo(cx);
    app.click(cx, "sql-editor");
    app.press(cx, "cmd-down");
    report(
        "ui.editor.keystroke_frame_demo",
        keystroke_frames(&app, cx),
        "ms",
        50.,
    );
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn editor_keystroke_frame_busy_workspace(cx: &mut TestAppContext) {
    // 20 connections with three tabs of 10 KB of SQL each.
    let line = "SELECT route, COUNT(*) FROM flights GROUP BY route;\n";
    let sql = line.repeat(10_000 / line.len() + 1);
    let profiles: Vec<_> = (0..20)
        .map(|n| offline_profile(&format!("Warehouse {n:02}")))
        .collect();
    let tabs = profiles
        .iter()
        .enumerate()
        .flat_map(|(n, profile)| {
            let sql = sql.clone();
            (0..3).map(move |t| SavedTab {
                sql: sql.clone(),
                ..SavedTab::new(n * 3 + t + 1, Some(profile.id))
            })
        })
        .collect();
    let workspace = Workspace {
        profiles,
        tabs,
        ..Workspace::default()
    };
    let app = TestApp::launch_with(cx, workspace, MemoryCredentials::default());
    app.click(cx, "sql-editor");
    app.press(cx, "cmd-down");
    report(
        "ui.editor.keystroke_frame_busy",
        keystroke_frames(&app, cx),
        "ms",
        50.,
    );
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn logs_record(cx: &mut TestAppContext) {
    use qrow::logs::{ExecutionId, LogEvent, LogKind, Severity};
    // Logs with 20,000 entries in 100 runs, the most runs that a tab keeps.
    // Each sample adds a new run of 500 entries in one batch, like a busy
    // worker between two frames. The new run also removes the oldest one.
    let profile = offline_profile("Warehouse");
    let workspace = Workspace {
        tabs: vec![SavedTab::new(1, Some(profile.id))],
        profiles: vec![profile],
        ..Workspace::default()
    };
    let app = TestApp::launch_with(cx, workspace, MemoryCredentials::default());
    let qrow = app.qrow.upgrade().expect("The window is open");
    let run = |execution: u64, count: usize| -> Vec<LogEvent> {
        (0..count)
            .map(|n| {
                LogEvent::new(
                    Some(ExecutionId(execution)),
                    Severity::Info,
                    LogKind::FetchCompleted,
                    format!("Run {execution}: fetched batch {n} of 1,000 rows (12 ms)"),
                )
            })
            .collect()
    };
    for execution in 0..100 {
        qrow.update(cx, |qrow, cx| {
            qrow.record_tab_logs(0, run(execution, 200), cx)
        });
    }
    app.settle(cx);
    let mut next = 100;
    let samples = app.update(cx, |window, cx| {
        sample(3, 20, || {
            qrow.update(cx, |qrow, cx| qrow.record_tab_logs(0, run(next, 500), cx));
            next += 1;
            window.render_frame(cx);
        })
    });
    report("ui.logs.record_500", median_ms(&samples), "ms", 25.);
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn assistant_large_reply(cx: &mut TestAppContext) {
    let (directory, codex) = FakeCodex::new();
    let workspace = codex.workspace(Workspace::default());
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    app.open_assistant(cx);
    // Each reply streams about 400 KB in 400 deltas.
    let samples: Vec<_> = (1..=3)
        .map(|reply| {
            let message = format!("Stream a large reply {reply}");
            app.type_message(cx, &message);
            let started = Instant::now();
            app.click(cx, "assistant-send");
            app.wait_reply(cx, &format!("End of {message}"));
            started.elapsed()
        })
        .collect();
    report(
        "ui.assistant.stream_large_reply",
        median_ms(&samples),
        "ms",
        20000.,
    );
    app.wait_idle(cx);
    // The save timer runs each second while a reply streams. Without a
    // change it must not do the work of a save.
    let qrow = app.qrow.upgrade().expect("The window is open");
    qrow.update(cx, |qrow, _| qrow.save_assistant_transcripts());
    let samples = app.update(cx, |_, cx| {
        sample(3, 20, || {
            qrow.update(cx, |qrow, _| qrow.save_assistant_transcripts())
        })
    });
    report(
        "ui.assistant.transcript_save_unchanged",
        median_ms(&samples),
        "ms",
        50.,
    );
}

/// A cached catalog like a large warehouse: 500 schemas, and the schema
/// `default` with `tables` tables. The first two tables have 1,000 columns.
fn completion_catalog(cx: &mut TestAppContext, tables: usize, sql: String) -> TestApp {
    use qrow::catalog::{Catalog, CatalogColumn, RelationEntry, RelationKind};
    let profile = offline_profile("Warehouse");
    let directory = tempfile::tempdir().unwrap();
    let mut catalog = Catalog::new(&profile);
    let mut schemas: Vec<String> = (0..500).map(|n| format!("schema_{n:03}")).collect();
    schemas.push("default".into());
    let now = qrow::catalog::now();
    catalog.apply_schemas(schemas, &Default::default(), now);
    let names: Vec<String> = (0..tables).map(|n| format!("fact_table_{n:06}")).collect();
    catalog.apply_relations(
        "default",
        None,
        names
            .iter()
            .map(|name| RelationEntry {
                name: name.clone(),
                kind: RelationKind::Table,
                comment: Some("One row for each event of the day.".into()),
            })
            .collect(),
        now,
    );
    let columns = names
        .iter()
        .take(2)
        .map(|name| {
            let columns = (0..1000)
                .map(|n| CatalogColumn {
                    name: format!("measure_column_{n:04}"),
                    data_type: "DECIMAL(12,2)".into(),
                    comment: Some("The amount in the currency of the booking.".into()),
                })
                .collect();
            (name.clone(), columns)
        })
        .collect();
    catalog.apply_columns("default", None, columns, now);
    let path = qrow::storage::catalog_cache_path(directory.path(), profile.id);
    qrow::storage::save_catalog(&path, &catalog).unwrap();
    let workspace = Workspace {
        tabs: vec![SavedTab {
            sql,
            ..SavedTab::new(1, Some(profile.id))
        }],
        profiles: vec![profile.clone()],
        ..Workspace::default()
    };
    let app = TestApp::launch_in(cx, directory, workspace, MemoryCredentials::default());
    // The window has the cache when the sidebar shows its schemas.
    app.toggle_connection(cx, profile.id);
    app.wait_until(
        cx,
        "the cached schemas",
        std::time::Duration::from_secs(20),
        |window, _| support::labelled(window, "schema_000").is_some(),
    );
    app.click(cx, "sql-editor");
    app.press(cx, "cmd-down");
    app.press(cx, "cmd-right");
    app
}

/// Presses `keys` in turn. Returns the median time of the key work and of
/// the frame after it, drawn like the window does.
fn key_and_frame(app: &TestApp, cx: &mut TestAppContext, keys: &[&str]) -> (f64, f64) {
    app.update(cx, |window, cx| window.draw(cx).clear(cx));
    let mut work = Vec::new();
    let mut frame = Vec::new();
    for round in 0..46 {
        let name = keys[round % keys.len()];
        let mut key = Keystroke::parse(name).expect("a keystroke");
        if name.chars().count() == 1 {
            key.key_char = Some(name.into());
        }
        let started = Instant::now();
        app.update(cx, |window, cx| window.dispatch_keystroke(key, cx));
        let dispatched = started.elapsed();
        app.update(cx, |window, cx| window.draw(cx).clear(cx));
        if round >= 6 {
            work.push(dispatched);
            frame.push(started.elapsed() - dispatched);
        }
    }
    (median_ms(&work), median_ms(&frame))
}

/// Presses `key` once and draws the frame after it.
fn key_and_frame_once(app: &TestApp, cx: &mut TestAppContext, name: &str) {
    let mut key = Keystroke::parse(name).expect("a keystroke");
    if name.chars().count() == 1 {
        key.key_char = Some(name.into());
    }
    app.update(cx, |window, cx| window.dispatch_keystroke(key, cx));
    app.update(cx, |window, cx| window.draw(cx).clear(cx));
}

fn report_keys(probe: &str, (work, frame): (f64, f64)) {
    report(&format!("{probe}.work"), work, "ms", 50.);
    report(&format!("{probe}.frame"), frame, "ms", 50.);
}

/// The rows of the list of suggestions that the window shows, or none
/// while the list is closed.
fn completion_rows(app: &TestApp, cx: &mut TestAppContext) -> Vec<String> {
    app.update(cx, |window, cx| {
        window.render_frame(cx);
        let mut rows: Vec<String> = support::elements(window)
            .into_iter()
            .filter_map(|element| element.path().last().map(|id| format!("{id:?}")))
            .filter(|id| id.contains("sql-completion-item-"))
            .collect();
        rows.sort();
        rows
    })
}

/// Opens the list of suggestions with Tab. Returns its rows.
fn open_completion(app: &TestApp, cx: &mut TestAppContext) -> Vec<String> {
    app.press(cx, "tab");
    app.wait_until(
        cx,
        "the suggestions",
        std::time::Duration::from_secs(20),
        |window, _| {
            support::elements(window).into_iter().any(|element| {
                element
                    .path()
                    .last()
                    .is_some_and(|id| format!("{id:?}").contains("sql-completion-item-"))
            })
        },
    );
    completion_rows(app, cx)
}

/// A probe of the open list must end with the list open, else it measured
/// the editor without the list.
fn assert_completion_open(app: &TestApp, cx: &mut TestAppContext, probe: &str) {
    assert!(
        !completion_rows(app, cx).is_empty(),
        "the list of suggestions closed during {probe}"
    );
}

fn assert_completion_closed(app: &TestApp, cx: &mut TestAppContext, probe: &str) {
    let rows = completion_rows(app, cx);
    assert!(rows.is_empty(), "{probe} opened the list: {rows:?}");
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn completion_tables(cx: &mut TestAppContext) {
    for tables in [1_000, 20_000, 100_000] {
        let app = completion_catalog(cx, tables, "SELECT * FROM ".into());
        // The same keys in the editor without the list.
        report_keys(
            &format!("ui.completion.closed_keystroke_{tables}"),
            key_and_frame(&app, cx, &["f", "backspace"]),
        );
        assert_completion_closed(&app, cx, "closed_keystroke");
        let schemas = open_completion(&app, cx);
        // Each key changes the names of the list: schemas, then tables.
        key_and_frame_once(&app, cx, "f");
        let tables_shown = completion_rows(&app, cx);
        assert!(!tables_shown.is_empty() && tables_shown != schemas);
        key_and_frame_once(&app, cx, "backspace");
        assert_eq!(completion_rows(&app, cx), schemas);
        report_keys(
            &format!("ui.completion.keystroke_{tables}"),
            key_and_frame(&app, cx, &["f", "backspace"]),
        );
        assert_eq!(completion_rows(&app, cx), schemas);
        report_keys(
            &format!("ui.completion.move_{tables}"),
            key_and_frame(&app, cx, &["down"]),
        );
        assert_completion_open(&app, cx, "move");
        // Escape and Tab: the list opens again.
        report_keys(
            &format!("ui.completion.open_{tables}"),
            key_and_frame(&app, cx, &["escape", "tab"]),
        );
        assert_completion_open(&app, cx, "open");
    }
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn completion_columns(cx: &mut TestAppContext) {
    let tail = " FROM fact_table_000000 a JOIN fact_table_000001 b ON a.id = b.id";
    let app = completion_catalog(cx, 1_000, format!("SELECT me{tail}"));
    app.update(cx, |window, cx| {
        for _ in 0..tail.len() {
            window.press("left", cx);
        }
    });
    open_completion(&app, cx);
    report_keys(
        "ui.completion.columns_keystroke_2x1000",
        key_and_frame(&app, cx, &["a", "backspace"]),
    );
    assert_completion_open(&app, cx, "columns_keystroke");
    report_keys(
        "ui.completion.columns_move_2x1000",
        key_and_frame(&app, cx, &["down"]),
    );
    assert_completion_open(&app, cx, "columns_move");
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn completion_in_one_megabyte_of_sql(cx: &mut TestAppContext) {
    let line = "SELECT route, COUNT(*) FROM flights WHERE note = '日本語; safe' GROUP BY route;\n";
    let mut sql = line.repeat(1_000_000 / line.len() + 1);
    sql.push_str("SELECT * FROM ");
    let app = completion_catalog(cx, 1_000, sql);
    report_keys(
        "ui.completion.closed_keystroke_1mb",
        key_and_frame(&app, cx, &["f", "backspace"]),
    );
    assert_completion_closed(&app, cx, "closed_keystroke_1mb");
    open_completion(&app, cx);
    report_keys(
        "ui.completion.keystroke_1mb",
        key_and_frame(&app, cx, &["f", "backspace"]),
    );
    assert_completion_open(&app, cx, "keystroke_1mb");
    report_keys("ui.completion.move_1mb", key_and_frame(&app, cx, &["down"]));
    assert_completion_open(&app, cx, "move_1mb");
    // A blink of the caret while the list is open: the editor reports a
    // change, but the text and the cursor stay the same.
    let blink = |cx: &mut TestAppContext| {
        let samples: Vec<_> = (0..12)
            .map(|_| {
                let started = Instant::now();
                cx.executor()
                    .advance_clock(std::time::Duration::from_millis(500));
                cx.run_until_parked();
                started.elapsed()
            })
            .skip(2)
            .collect();
        median_ms(&samples)
    };
    report("ui.completion.blink_open_1mb", blink(cx), "ms", 50.);
    assert_completion_open(&app, cx, "blink_open_1mb");
    app.press(cx, "escape");
    assert_completion_closed(&app, cx, "escape");
    report("ui.completion.blink_closed_1mb", blink(cx), "ms", 50.);
}

#[gpui_kit::test]
#[ignore = "a performance probe: ./qtest run perf-ui"]
fn completion_tab_indent_in_one_megabyte_of_sql(cx: &mut TestAppContext) {
    // Tab at the start of a line indents. No list opens.
    let line = "SELECT route, COUNT(*) FROM flights WHERE note = '日本語; safe' GROUP BY route;\n";
    let sql = line.repeat(1_000_000 / line.len() + 1);
    let app = completion_catalog(cx, 1_000, sql);
    report_keys(
        "ui.completion.indent_char_1mb",
        key_and_frame(&app, cx, &[" "]),
    );
    report_keys(
        "ui.completion.indent_tab_1mb",
        key_and_frame(&app, cx, &["tab"]),
    );
    assert_completion_closed(&app, cx, "indent_tab_1mb");
}
