//! A repeatable release benchmark of the SQL suggestions: the read of the
//! SQL around the cursor, and the match against cached metadata.
use qrow::catalog::{Catalog, CatalogColumn, RelationEntry, RelationKind};
use qrow::completion::{self, Place, Request};
use qrow::model::{CatalogSettings, DatabaseType, Profile};
use std::{collections::BTreeMap, hint::black_box, time::Instant};

fn median_ms(mut work: impl FnMut()) -> f64 {
    for _ in 0..3 {
        work();
    }
    let mut samples = Vec::new();
    for _ in 0..21 {
        let started = Instant::now();
        work();
        samples.push(started.elapsed().as_secs_f64() * 1000.);
    }
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn report(probe: &str, median: f64, budget_ms: f64) {
    println!("{probe} median_ms={median:.3}");
    // qtest collects this line; see docs/testing.md.
    println!(
        "QROW_PERF {{\"probe\":\"{probe}\",\"value\":{median},\"unit\":\"ms\",\"budget\":{budget_ms}}}"
    );
    assert!(
        median < budget_ms,
        "{probe} exceeded {budget_ms} ms: {median:.3} ms"
    );
}

/// A catalog of `schemas` schemas. The schema `default` has `relations`
/// tables, and its first `wide` tables have `columns` columns each.
fn catalog(schemas: usize, relations: usize, wide: usize, columns: usize) -> Catalog {
    let profile = Profile::default();
    let mut catalog = Catalog::new(&profile);
    let mut names: Vec<String> = (0..schemas).map(|n| format!("schema_{n:04}")).collect();
    names.push("default".into());
    catalog.apply_schemas(names, &CatalogSettings::default(), 1);
    let tables: Vec<String> = (0..relations)
        .map(|n| format!("fact_table_{n:06}"))
        .collect();
    catalog.apply_relations(
        "default",
        None,
        tables
            .iter()
            .map(|name| RelationEntry {
                name: name.clone(),
                kind: RelationKind::Table,
                comment: Some("One row for each event of the day.".into()),
            })
            .collect(),
        1,
    );
    let described: BTreeMap<String, Vec<CatalogColumn>> = tables
        .iter()
        .take(wide)
        .map(|name| {
            let columns = (0..columns)
                .map(|n| CatalogColumn {
                    name: format!("measure_column_{n:04}"),
                    data_type: "DECIMAL(12,2)".into(),
                    comment: Some("The amount in the currency of the booking.".into()),
                })
                .collect();
            (name.clone(), columns)
        })
        .collect();
    catalog.apply_columns("default", None, described, 1);
    catalog
}

fn complete(catalog: &Catalog, request: &Request) -> usize {
    completion::complete(
        Some(catalog),
        request,
        "default",
        &CatalogSettings::default(),
        2,
        DatabaseType::Kyuubi,
    )
    .items
    .len()
}

fn main() {
    // The read of the SQL around the cursor without kept statements: one
    // read of the text up to the statement at the cursor.
    let line = "SELECT route, COUNT(*) FROM flights WHERE note = '日本語; safe' GROUP BY route;\n";
    for (bytes, budget) in [(10_000, 2.), (100_000, 10.), (1_000_000, 50.)] {
        let mut source = line.repeat(bytes / line.len() + 1);
        source.push_str("SELECT * FROM default.fa");
        let offset = source.len();
        let median = median_ms(|| {
            black_box(completion::request(
                black_box(&source),
                offset,
                DatabaseType::Kyuubi,
            ));
        });
        report(&format!("completion.request.{bytes}"), median, budget);
    }
    // The same read with the statements that the window keeps for the
    // editor: the list follows a key at the end of 1 MB of SQL, the cursor
    // moves in the last statement, and an edit at the start makes a new
    // read of all the text.
    let mut source = line.repeat(1_000_000 / line.len() + 1);
    source.push_str("SELECT * FROM default.fa");
    let typed = format!("{source}c");
    let mut statements = completion::Statements::new(DatabaseType::Kyuubi);
    let mut flip = false;
    let median = median_ms(|| {
        flip = !flip;
        let sql = if flip { &typed } else { &source };
        black_box(statements.request(black_box(sql), sql.len()));
    });
    report("completion.statements.edit_end.1000000", median, 2.);
    let median = median_ms(|| {
        flip = !flip;
        let offset = source.len() - if flip { 1 } else { 3 };
        black_box(statements.request(black_box(&source), offset));
    });
    report("completion.statements.move.1000000", median, 1.);
    let edited = format!(" {source}");
    let median = median_ms(|| {
        flip = !flip;
        let sql = if flip { &edited } else { &source };
        black_box(statements.request(black_box(sql), sql.len()));
    });
    report("completion.statements.edit_start.1000000", median, 50.);

    // One long statement: a SELECT with many columns and joins.
    for (bytes, budget) in [(10_000, 5.), (100_000, 25.)] {
        let mut source = String::from("SELECT ");
        while source.len() < bytes {
            source.push_str("a.measure_column_0001 + b.measure_column_0002 AS total,\n  ");
        }
        source.push_str(
            "1 FROM fact_table_000000 a JOIN fact_table_000001 b ON a.id = b.id WHERE a.me",
        );
        let offset = source.len();
        let median = median_ms(|| {
            black_box(completion::request(
                black_box(&source),
                offset,
                DatabaseType::Kyuubi,
            ));
        });
        report(
            &format!("completion.request.statement.{bytes}"),
            median,
            budget,
        );
    }

    // The match of table names after FROM.
    for (relations, budget) in [(1_000, 2.), (10_000, 5.), (100_000, 25.)] {
        let catalog = catalog(1_000, relations, 0, 0);
        for (label, prefix) in [
            ("empty", ""),
            ("prefix", "fact"),
            ("inner", "0042"),
            ("none", "zz"),
        ] {
            let request = Request {
                replace: 0..0,
                prefix: prefix.into(),
                start: 0,
                place: Place::Relation,
                column_start: false,
                indents: false,
                sources: Vec::new(),
            };
            let median = median_ms(|| {
                black_box(complete(&catalog, black_box(&request)));
            });
            report(
                &format!("completion.relations.{label}.{relations}"),
                median,
                budget,
            );
        }
    }

    // The match of column names in a statement with joins of wide tables.
    for (tables, columns, budget) in [
        (2, 100, 2.),
        (2, 1_000, 5.),
        (5, 1_000, 10.),
        (10, 2_000, 25.),
    ] {
        let catalog = catalog(10, 100, tables, columns);
        let sql = format!(
            "SELECT me FROM {}",
            (0..tables)
                .map(|n| format!("fact_table_{n:06} t{n}"))
                .collect::<Vec<_>>()
                .join(" JOIN ")
        );
        let request = completion::request(&sql, "SELECT me".len(), DatabaseType::Kyuubi).unwrap();
        assert_eq!(request.sources.len(), tables);
        let median = median_ms(|| {
            black_box(complete(&catalog, black_box(&request)));
        });
        report(
            &format!("completion.columns.{tables}x{columns}"),
            median,
            budget,
        );
    }
}
