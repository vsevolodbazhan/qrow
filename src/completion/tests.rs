use super::*;
use crate::catalog::{CatalogColumn, RelationEntry};
use std::collections::BTreeMap;

const NOW: u64 = 100_000;
const KYUUBI: DatabaseType = DatabaseType::Kyuubi;

/// The request at the `|` in `sql`.
fn at(sql: &str, database_type: DatabaseType) -> (String, Request) {
    let offset = sql.find('|').expect("a cursor mark");
    let text = sql.replacen('|', "", 1);
    let request = request(&text, offset, database_type).expect("a request");
    (text, request)
}

fn catalog(read_at: u64) -> Catalog {
    let mut catalog = Catalog::empty(uuid::Uuid::nil(), None);
    catalog.apply_schemas(
        vec!["avia".into(), "finance".into(), "cold".into()],
        &CatalogSettings::default(),
        read_at,
    );
    let entry = |name: &str, kind| RelationEntry {
        name: name.into(),
        kind,
        comment: None,
    };
    catalog.apply_relations(
        "avia",
        None,
        vec![
            entry("bookings", RelationKind::Table),
            entry("fare_rules", RelationKind::Table),
            entry("fleet_summary", RelationKind::View),
            entry("flight_events", RelationKind::Table),
            entry("flights", RelationKind::Table),
            entry("Mixed Case", RelationKind::Table),
        ],
        read_at,
    );
    catalog.apply_relations(
        "finance",
        None,
        vec![entry("payments", RelationKind::Table)],
        read_at,
    );
    let column = |name: &str, data_type: &str| CatalogColumn {
        name: name.into(),
        data_type: data_type.into(),
        comment: (name == "amount").then(|| "Total fare.".into()),
    };
    // Reads of single relations, so that the other relations keep no columns.
    catalog.apply_columns(
        "avia",
        Some("bookings"),
        BTreeMap::from([(
            "bookings".into(),
            vec![
                column("booking_id", "BIGINT"),
                column("status", "STRING"),
                column("amount", "DECIMAL(12,2)"),
            ],
        )]),
        read_at,
    );
    catalog.apply_columns(
        "avia",
        Some("flight_events"),
        BTreeMap::from([(
            "flight_events".into(),
            vec![
                column("booking_id", "BIGINT"),
                column("status", "STRING"),
                column("station_code", "STRING"),
                column("scheduled_at", "TIMESTAMP"),
            ],
        )]),
        read_at,
    );
    catalog
}

fn suggest(catalog: Option<&Catalog>, sql: &str) -> Completion {
    let (_, request) = at(sql, KYUUBI);
    complete(
        catalog,
        &request,
        "avia",
        &CatalogSettings::default(),
        NOW,
        KYUUBI,
    )
}

fn names(completion: &Completion) -> Vec<String> {
    completion
        .items
        .iter()
        .map(|item| match &item.qualifier {
            Some(qualifier) => format!("{qualifier}.{}", item.name),
            None => item.name.clone(),
        })
        .collect()
}

#[test]
fn a_name_after_from_or_join_is_a_relation() {
    for sql in [
        "SELECT * FROM |",
        "SELECT * FROM fl|",
        "select * from a join |",
        "SELECT * FROM a, |",
        "INSERT INTO |",
        "UPDATE |",
    ] {
        assert_eq!(at(sql, KYUUBI).1.place, Place::Relation, "{sql}");
    }
    for sql in ["SELECT |", "SELECT * FROM a WHERE |", "SELECT a, | FROM t"] {
        assert_eq!(at(sql, KYUUBI).1.place, Place::Column, "{sql}");
    }
}

#[test]
fn the_request_replaces_the_whole_name_at_the_cursor() {
    let (text, request) = at("SELECT * FROM avia.fl|ights", KYUUBI);
    assert_eq!(&text[request.replace.clone()], "flights");
    assert_eq!(request.prefix, "fl");
    assert_eq!(
        request.place,
        Place::Qualified {
            parts: vec!["avia".into()],
            relation: true
        }
    );
    let (text, request) = at("SELECT b.| FROM t b", KYUUBI);
    assert_eq!(request.replace, 9..9);
    assert_eq!(&text[..request.replace.start], "SELECT b.");
}

#[test]
fn there_is_no_request_in_a_comment_or_a_string() {
    for sql in [
        "SELECT 1 -- FROM |",
        "SELECT '|'",
        "SELECT 'abc|",
        "SELECT /* FROM | */ 1",
    ] {
        let offset = sql.find('|').unwrap();
        let text = sql.replacen('|', "", 1);
        assert_eq!(request(&text, offset, KYUUBI), None, "{sql}");
    }
    // After a closed string or comment, the cursor is in SQL again.
    assert!(request("SELECT 'a' ", 11, KYUUBI).is_some());
    assert!(request("SELECT /* x */ ", 15, KYUUBI).is_some());
}

#[test]
fn a_quoted_name_matches_without_its_quotes() {
    let (text, request) = at("SELECT * FROM `fl|", KYUUBI);
    assert_eq!(request.prefix, "fl");
    assert_eq!(&text[request.replace], "`fl");
    let (_, request) = at("SELECT * FROM \"avia\".\"fl|", DatabaseType::Postgres);
    assert_eq!(request.prefix, "fl");
    assert_eq!(
        request.place,
        Place::Qualified {
            parts: vec!["avia".into()],
            relation: true
        }
    );
}

#[test]
fn a_closed_quoted_name_before_the_cursor_is_the_name_to_replace() {
    let (text, request) = at("SELECT b.\"sta\"| FROM t b", DatabaseType::Postgres);
    assert_eq!(request.prefix, "sta");
    assert_eq!(&text[request.replace], "\"sta\"");
    let (_, request) = at("SELECT `a``b`|", KYUUBI);
    assert_eq!(request.prefix, "a`b");
}

#[test]
fn sources_have_their_schemas_and_aliases() {
    let (_, request) = at(
        "SELECT | FROM avia.bookings AS b JOIN flight_events f ON f.x = b.x, `cold`.t WHERE 1 = 1",
        KYUUBI,
    );
    assert_eq!(
        request.sources,
        [
            Source {
                schema: Some("avia".into()),
                name: "bookings".into(),
                alias: Some("b".into())
            },
            Source {
                schema: None,
                name: "flight_events".into(),
                alias: Some("f".into())
            },
            Source {
                schema: Some("cold".into()),
                name: "t".into(),
                alias: None
            },
        ]
    );
    // A subquery is not a source, and the words after a table are not aliases.
    let (_, request) = at(
        "SELECT | FROM (SELECT 1) x JOIN avia.bookings LEFT JOIN t2 ON 1=1",
        KYUUBI,
    );
    let sources: Vec<_> = request
        .sources
        .iter()
        .map(|source| (source.name.as_str(), source.alias.as_deref()))
        .collect();
    assert_eq!(sources, [("bookings", None), ("t2", None)]);
}

#[test]
fn only_the_statement_at_the_cursor_counts() {
    let (_, request) = at(
        "SELECT * FROM finance.payments p;\nSELECT p.| FROM avia.bookings b",
        KYUUBI,
    );
    assert_eq!(request.sources.len(), 1);
    assert_eq!(request.sources[0].name, "bookings");
}

#[test]
fn after_from_the_list_has_schemas_then_relations_of_the_default_schema() {
    let catalog = catalog(NOW - 60);
    let completion = suggest(Some(&catalog), "SELECT * FROM |");
    assert_eq!(
        names(&completion),
        [
            "avia",
            "cold",
            "finance",
            "Mixed Case",
            "bookings",
            "fare_rules",
            "fleet_summary",
            "flight_events",
            "flights"
        ]
    );
    assert!(completion.stale.is_empty());
    assert!(completion.missing.is_empty());
}

#[test]
fn a_prefix_ranks_prefix_matches_before_parts_and_characters_in_order() {
    let catalog = catalog(NOW - 60);
    let completion = suggest(Some(&catalog), "SELECT * FROM avia.fl|");
    assert_eq!(
        names(&completion),
        ["fleet_summary", "flight_events", "flights", "fare_rules"]
    );
    let fare = &completion.items[3];
    assert_eq!(fare.matches, [0, 7]);
    assert_eq!(fare.kind, ItemKind::Table);
    assert_eq!(completion.items[0].kind, ItemKind::View);
    assert_eq!(completion.items[0].detail, "view");
}

#[test]
fn fuzzy_matches_ignore_letter_case() {
    assert_eq!(fuzzy("Flights", "fl"), Some((0, vec![0, 1])));
    assert_eq!(fuzzy("my_flights", "FLI"), Some((1, vec![3, 4, 5])));
    assert_eq!(fuzzy("seat_number", "st"), Some((2, vec![0, 3])));
    assert_eq!(fuzzy("bookings", "st"), None);
    // Characters in order must start at the start of the name.
    assert_eq!(fuzzy("est", "st"), Some((1, vec![1, 2])));
    assert_eq!(fuzzy("xsxt", "st"), None);
    assert_eq!(fuzzy("Ünïcode", "ün"), Some((0, vec![0, 2])));
}

#[test]
fn an_alias_gives_the_columns_of_its_relation() {
    let catalog = catalog(NOW - 60);
    let completion = suggest(
        Some(&catalog),
        "SELECT b.| FROM avia.bookings AS b JOIN avia.flight_events f ON 1=1",
    );
    assert_eq!(names(&completion), ["booking_id", "status", "amount"]);
    let amount = &completion.items[2];
    assert_eq!(amount.detail, "decimal(12,2)");
    assert_eq!(amount.relation, Some(("avia".into(), "bookings".into())));
    assert_eq!(amount.comment.as_deref(), Some("Total fare."));
    assert_eq!(amount.insert, "amount");
    // A table name without an alias works as a qualifier too.
    let completion = suggest(Some(&catalog), "SELECT bookings.st| FROM bookings");
    assert_eq!(names(&completion), ["status"]);
}

#[test]
fn a_column_name_in_more_than_one_relation_is_qualified() {
    let catalog = catalog(NOW - 60);
    let completion = suggest(
        Some(&catalog),
        "SELECT route FROM avia.bookings AS b JOIN avia.flight_events AS f ON 1=1 WHERE st|",
    );
    assert_eq!(
        names(&completion),
        ["b.status", "f.status", "station_code", "scheduled_at"]
    );
    assert_eq!(completion.items[0].insert, "b.status");
    assert_eq!(completion.items[0].source, None);
    assert_eq!(completion.items[2].insert, "station_code");
    assert_eq!(completion.items[2].source.as_deref(), Some("f"));
}

#[test]
fn a_single_relation_needs_no_tags() {
    let catalog = catalog(NOW - 60);
    let completion = suggest(Some(&catalog), "SELECT | FROM avia.bookings");
    assert_eq!(names(&completion), ["booking_id", "status", "amount"]);
    assert!(completion.items.iter().all(|item| item.source.is_none()));
}

#[test]
fn missing_metadata_is_reported_with_its_error() {
    assert_eq!(suggest(None, "SELECT * FROM |").missing, [Need::Schemas]);
    let mut catalog = catalog(NOW - 60);
    // `cold` has no relation list yet.
    let completion = suggest(Some(&catalog), "SELECT * FROM cold.|");
    assert_eq!(completion.missing, [Need::Relations("cold".into())]);
    // `flights` has no columns yet.
    let completion = suggest(Some(&catalog), "SELECT f.| FROM avia.flights f");
    assert_eq!(
        completion.missing,
        [Need::Columns("avia".into(), "flights".into())]
    );
    assert!(completion.errors.is_empty());
    catalog.set_error(
        &crate::catalog::Scope::Relation("avia".into(), "flights".into()),
        "Table or view not found: avia.flights".into(),
        uuid::Uuid::nil(),
    );
    let completion = suggest(Some(&catalog), "SELECT f.| FROM avia.flights f");
    assert_eq!(
        completion.errors,
        [(
            Need::Columns("avia".into(), "flights".into()),
            "Table or view not found: avia.flights".into()
        )]
    );
}

#[test]
fn metadata_older_than_the_refresh_period_is_stale() {
    let catalog = catalog(NOW - 3_601);
    let completion = suggest(Some(&catalog), "SELECT * FROM avia.|");
    assert_eq!(completion.stale, ["avia"]);
    let completion = suggest(Some(&catalog), "SELECT b.| FROM avia.bookings b");
    assert_eq!(completion.stale, ["avia"]);
}

#[test]
fn the_list_has_at_most_max_items() {
    let mut catalog = Catalog::empty(uuid::Uuid::nil(), None);
    catalog.apply_schemas(vec!["big".into()], &CatalogSettings::default(), NOW);
    catalog.apply_relations(
        "big",
        None,
        (0..MAX_ITEMS + 50)
            .map(|index| RelationEntry {
                name: format!("t{index:04}"),
                kind: RelationKind::Table,
                comment: None,
            })
            .collect(),
        NOW,
    );
    let completion = suggest(Some(&catalog), "SELECT * FROM big.|");
    assert_eq!(completion.items.len(), MAX_ITEMS);
}

#[test]
fn names_are_quoted_only_when_they_need_it() {
    assert_eq!(sql_name("flights", KYUUBI), "flights");
    assert_eq!(sql_name("Flights", KYUUBI), "Flights");
    assert_eq!(sql_name("Flights", DatabaseType::Postgres), "\"Flights\"");
    assert_eq!(sql_name("Flights", DatabaseType::Trino), "\"Flights\"");
    assert_eq!(sql_name("order", KYUUBI), "`order`");
    assert_eq!(sql_name("Mixed Case", KYUUBI), "`Mixed Case`");
    assert_eq!(sql_name("1st", DatabaseType::Postgres), "\"1st\"");
    assert_eq!(sql_name("a`b", KYUUBI), "`a``b`");
    // Each dialect has reserved words of its own.
    assert_eq!(
        sql_name("returning", DatabaseType::Postgres),
        "\"returning\""
    );
    assert_eq!(sql_name("returning", KYUUBI), "returning");
    assert_eq!(sql_name("unnest", DatabaseType::Trino), "\"unnest\"");
    assert_eq!(sql_name("filter", KYUUBI), "`filter`");
    let catalog = catalog(NOW - 60);
    let completion = suggest(Some(&catalog), "SELECT * FROM avia.Mi|");
    assert_eq!(completion.items[0].insert, "`Mixed Case`");
}
