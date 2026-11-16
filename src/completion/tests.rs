use super::*;
use crate::catalog::{CatalogColumn, RelationEntry};
use std::collections::BTreeMap;

mod reference;

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
fn a_column_starts_after_words_and_signs_that_an_expression_follows() {
    for sql in [
        "SELECT |",
        "SELECT * FROM a WHERE |",
        "select * from a\nwhere |",
        "SELECT * FROM a WHERE x = 1 AND |",
        "SELECT a, | FROM t",
        "SELECT * FROM a JOIN b ON |",
        "SELECT * FROM a WHERE x = |",
        "SELECT * FROM a ORDER BY |",
        "SELECT count(| FROM t",
    ] {
        assert!(at(sql, KYUUBI).1.column_start, "{sql}");
    }
    for sql in [
        "SELECT 1 |",
        "SELECT * FROM a |",
        "SELECT * FROM |",
        "SELECT * FROM a, |",
        "SELECT a.|",
    ] {
        assert!(!at(sql, KYUUBI).1.column_start, "{sql}");
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
    // A list that shows no name from the stale schema does not report it.
    let completion = suggest(Some(&catalog), "SELECT * FROM fina|");
    assert_eq!(names(&completion), ["finance"]);
    assert!(completion.stale.is_empty());
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

/// A small generator of repeatable random numbers for generated catalogs.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, count: usize) -> usize {
        (self.next() % count as u64) as usize
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

/// Parts of generated names: letter case variants, non-ASCII letters whose
/// lowercase form is ASCII or longer than one character, and digits.
const NAME_PARTS: [&str; 24] = [
    "id",
    "Id",
    "ID",
    "status",
    "STATUS",
    "Status",
    "fl",
    "fare",
    "flight",
    "Straße",
    "STRASSE",
    "\u{212a}elvin",
    "kelvin",
    "\u{130}d",
    "日本",
    "éte",
    "ÉTÉ",
    "a",
    "b",
    "x_1",
    "0042",
    "_",
    "ß",
    "Ωmega",
];

fn generated_name(random: &mut Random) -> String {
    let mut name = String::new();
    for _ in 0..1 + random.below(3) {
        if !name.is_empty() && random.chance(60) {
            name.push('_');
        }
        name.push_str(random.pick(&NAME_PARTS));
    }
    if random.chance(3) {
        // Longer than the buffer of `tier`.
        name.push_str(&"日本_".repeat(30));
    }
    if random.chance(50) {
        name.push_str(&random.below(500).to_string());
    }
    name
}

/// A catalog with schemas of random sizes, up to more relations and
/// columns than the list shows.
fn generated_catalog(random: &mut Random) -> Catalog {
    let mut catalog = Catalog::empty(uuid::Uuid::nil(), None);
    let mut schemas: Vec<String> = (0..1 + random.below(6))
        .map(|_| generated_name(random))
        .collect();
    if random.chance(30) {
        schemas.extend((0..MAX_ITEMS + random.below(100)).map(|n| format!("s{n}")));
    }
    schemas.push("main".into());
    catalog.apply_schemas(schemas.clone(), &CatalogSettings::default(), NOW - 60);
    for schema in &schemas {
        if random.chance(20) && schema != "main" {
            continue;
        }
        let count = if random.chance(15) {
            MAX_ITEMS * 2 + random.below(200)
        } else {
            random.below(20)
        };
        let names: Vec<String> = (0..count).map(|_| generated_name(random)).collect();
        let fetched = if random.chance(30) {
            NOW - 4_000
        } else {
            NOW - 60
        };
        catalog.apply_relations(
            schema,
            None,
            names
                .iter()
                .map(|name| RelationEntry {
                    name: name.clone(),
                    kind: if random.chance(20) {
                        RelationKind::View
                    } else {
                        RelationKind::Table
                    },
                    comment: random.chance(30).then(|| format!("About {name}")),
                })
                .collect(),
            fetched,
        );
        let Some(relations) = catalog.schemas[schema].relations.clone() else {
            continue;
        };
        for relation in relations.keys() {
            if random.chance(30) {
                continue;
            }
            let count = if random.chance(2) {
                MAX_ITEMS * 2 + random.below(100)
            } else {
                random.below(12)
            };
            let columns = (0..count)
                .map(|_| CatalogColumn {
                    name: generated_name(random),
                    data_type: random
                        .pick(&["BIGINT", "String", "DECIMAL(12,2)"])
                        .to_string(),
                    comment: random.chance(30).then(|| "A comment.".into()),
                })
                .collect();
            let fetched = if random.chance(20) {
                NOW - 4_000
            } else {
                NOW - 60
            };
            catalog.apply_columns(
                schema,
                Some(relation),
                BTreeMap::from([(relation.clone(), columns)]),
                fetched,
            );
        }
    }
    catalog
}

/// A name of `catalog`, or a name that it does not have, in a random
/// letter case.
fn some_name(random: &mut Random, names: &[String]) -> String {
    let name = if names.is_empty() || random.chance(10) {
        generated_name(random)
    } else {
        random.pick(names).clone()
    };
    match random.below(4) {
        0 => name.to_uppercase(),
        1 => name.to_lowercase(),
        _ => name,
    }
}

fn generated_prefix(random: &mut Random, names: &[String]) -> String {
    match random.below(5) {
        0 => String::new(),
        1 => random.pick(&NAME_PARTS).to_string(),
        2 => ["k", "K", "\u{212a}", "i", "ss", "zz", "日", "s1", "_"][random.below(9)].into(),
        _ => {
            let name: Vec<char> = some_name(random, names).chars().collect();
            let start = random.below(name.len().max(1));
            let end = (start + 1 + random.below(4)).min(name.len());
            name[start.min(end)..end].iter().collect()
        }
    }
}

#[test]
fn the_ranked_list_is_the_list_of_the_first_implementation() {
    let mut random = Random(0x5eed_1234_abcd_0001);
    let mut cases = 0;
    for _ in 0..24 {
        let catalog = generated_catalog(&mut random);
        let schemas: Vec<String> = catalog.schemas.keys().cloned().collect();
        let relations: Vec<(String, String)> = catalog
            .schemas
            .iter()
            .flat_map(|(schema, node)| {
                node.relations
                    .iter()
                    .flat_map(|relations| relations.keys())
                    .map(|relation| (schema.clone(), relation.clone()))
            })
            .collect();
        let relation_names: Vec<String> = relations.iter().map(|(_, name)| name.clone()).collect();
        for _ in 0..80 {
            let sources: Vec<Source> = (0..random.below(5))
                .map(|_| {
                    let (schema, name) = if relations.is_empty() || random.chance(10) {
                        ("main".to_string(), generated_name(&mut random))
                    } else {
                        random.pick(&relations).clone()
                    };
                    Source {
                        schema: random.chance(60).then(|| some_name(&mut random, &[schema])),
                        name: some_name(&mut random, &[name]),
                        alias: random
                            .chance(50)
                            .then(|| some_name(&mut random, &relation_names)),
                    }
                })
                .collect();
            let place = match random.below(3) {
                0 => Place::Relation,
                1 => Place::Column,
                _ => {
                    let qualifier = |random: &mut Random| match random.below(3) {
                        0 if !sources.is_empty() => random.pick(&sources).qualifier().to_owned(),
                        1 => some_name(random, &schemas),
                        _ => some_name(random, &relation_names),
                    };
                    let mut parts = vec![qualifier(&mut random)];
                    if random.chance(40) {
                        parts.insert(0, some_name(&mut random, &schemas));
                    }
                    Place::Qualified {
                        parts,
                        relation: random.chance(50),
                    }
                }
            };
            let request = Request {
                replace: 0..0,
                prefix: generated_prefix(&mut random, &relation_names),
                start: 0,
                place,
                column_start: true,
                sources,
            };
            let default_schema = if random.chance(80) {
                "main".to_string()
            } else {
                some_name(&mut random, &schemas)
            };
            let database_type =
                *random.pick(&[KYUUBI, DatabaseType::Postgres, DatabaseType::Trino]);
            let settings = CatalogSettings::default();
            let fast = complete(
                Some(&catalog),
                &request,
                &default_schema,
                &settings,
                NOW,
                database_type,
            );
            let first = reference::complete(
                Some(&catalog),
                &request,
                &default_schema,
                &settings,
                NOW,
                database_type,
            );
            assert_eq!(fast, first, "{request:?} in {default_schema}");
            if !fast.items.is_empty() {
                cases += 1;
            }
        }
    }
    // Most generated requests have suggestions.
    assert!(cases > 800, "{cases} requests with suggestions");
}

#[test]
fn the_tier_without_offsets_is_the_tier_of_fuzzy() {
    let mut random = Random(0x0dd_ba11);
    let mut names: Vec<String> = (0..800).map(|_| generated_name(&mut random)).collect();
    names.extend(["", "a", "\u{212a}", "\u{130}", "ǅ", "ﬁle", "ΣΑΣ"].map(String::from));
    for name in &names {
        for prefix in names.iter().take(150).map(String::as_str).chain([
            "", "k", "K", "i", "i\u{307}", "s", "ss", "σ", "日本", "fi", "_",
        ]) {
            let wanted: Vec<char> = prefix.chars().map(lower).collect();
            assert_eq!(
                tier(name, &wanted),
                fuzzy(name, prefix).map(|(tier, _)| tier),
                "{name:?} for {prefix:?}"
            );
        }
    }
}

#[test]
fn ranks_order_tiers_then_groups_then_sources() {
    let catalog = catalog(NOW - 60);
    // After FROM: schemas before relations in each tier, and each tier
    // before the next.
    let completion = suggest(Some(&catalog), "SELECT * FROM a|");
    assert_eq!(
        names(&completion),
        [
            "avia",
            "finance",
            "Mixed Case",
            "fare_rules",
            "fleet_summary"
        ]
    );
    // Columns of the same tier follow the order of the columns in their
    // relations. Columns at the same place keep the order of the relations
    // of the statement.
    let completion = suggest(
        Some(&catalog),
        "SELECT * FROM avia.flight_events f JOIN avia.bookings b ON 1=1 WHERE t|",
    );
    assert_eq!(
        names(&completion),
        [
            "f.status",
            "b.status",
            "station_code",
            "amount",
            "scheduled_at"
        ]
    );
}

#[test]
fn shared_column_names_ignore_ascii_letter_case_only() {
    let mut catalog = Catalog::empty(uuid::Uuid::nil(), None);
    catalog.apply_schemas(vec!["s".into()], &CatalogSettings::default(), NOW);
    let entry = |name: &str| RelationEntry {
        name: name.into(),
        kind: RelationKind::Table,
        comment: None,
    };
    catalog.apply_relations("s", None, vec![entry("one"), entry("two")], NOW);
    let column = |name: &str| CatalogColumn {
        name: name.into(),
        data_type: "INT".into(),
        comment: None,
    };
    catalog.apply_columns(
        "s",
        None,
        BTreeMap::from([
            (
                "one".into(),
                vec![column("Status"), column("straße"), column("é")],
            ),
            (
                "two".into(),
                vec![column("STATUS"), column("STRASSE"), column("É")],
            ),
        ]),
        NOW,
    );
    let (_, request) = at("SELECT | FROM s.one a JOIN s.two b", KYUUBI);
    let completion = complete(
        Some(&catalog),
        &request,
        "s",
        &CatalogSettings::default(),
        NOW,
        KYUUBI,
    );
    // `É` and `é` differ outside ASCII, so they stay apart.
    assert_eq!(
        names(&completion),
        ["a.Status", "b.STATUS", "straße", "STRASSE", "é", "É"]
    );
    assert_eq!(completion.items[2].source.as_deref(), Some("a"));
    // A self join shares all its columns.
    let (_, request) = at("SELECT | FROM s.one a JOIN s.one b", KYUUBI);
    let completion = complete(
        Some(&catalog),
        &request,
        "s",
        &CatalogSettings::default(),
        NOW,
        KYUUBI,
    );
    assert!(completion.items.iter().all(|item| item.qualifier.is_some()));
}

#[test]
fn the_cap_keeps_the_best_ranks_and_the_stale_schemas_that_it_shows() {
    let mut catalog = Catalog::empty(uuid::Uuid::nil(), None);
    let mut schemas: Vec<String> = (0..MAX_ITEMS + 20).map(|n| format!("s{n:04}")).collect();
    schemas.push("big".into());
    catalog.apply_schemas(schemas, &CatalogSettings::default(), NOW);
    catalog.apply_relations(
        "big",
        None,
        (0..MAX_ITEMS * 3)
            .map(|index| RelationEntry {
                name: format!("t{index:04}_s"),
                kind: RelationKind::Table,
                comment: None,
            })
            .collect(),
        NOW - 4_000,
    );
    let suggest_in = |sql: &str| {
        let (_, request) = at(sql, KYUUBI);
        complete(
            Some(&catalog),
            &request,
            "big",
            &CatalogSettings::default(),
            NOW,
            KYUUBI,
        )
    };
    // All schemas rank before the tables, and the list has no room for
    // a table: the stale schema `big` does not show.
    let completion = suggest_in("SELECT * FROM |");
    assert_eq!(completion.items.len(), MAX_ITEMS);
    assert!(
        completion
            .items
            .iter()
            .all(|item| item.kind == ItemKind::Schema)
    );
    assert!(completion.stale.is_empty());
    // A prefix of the tables ranks them first.
    let completion = suggest_in("SELECT * FROM t|");
    assert_eq!(completion.items.len(), MAX_ITEMS);
    assert_eq!(completion.items[0].name, "t0000_s");
    assert_eq!(
        completion.items[MAX_ITEMS - 1].name,
        format!("t{:04}_s", MAX_ITEMS - 1)
    );
    assert_eq!(completion.stale, ["big"]);
    // Schemas that start with the prefix rank before tables that only
    // have it.
    let completion = suggest_in("SELECT * FROM s|");
    assert_eq!(completion.items[0].name, "s0000");
    assert_eq!(completion.items.len(), MAX_ITEMS);
    assert!(
        completion
            .items
            .iter()
            .all(|item| item.kind == ItemKind::Schema)
    );
}

/// The statement at `offset` as the first implementation of [`request`]
/// found it: from an index of the whole text, else between the `;` before
/// and after the cursor.
fn first_statement_at(sql: &str, offset: usize, database_type: DatabaseType) -> Range<usize> {
    crate::sql::statement_range_at_for(sql, offset, database_type)
        .filter(|range| range.start <= offset && offset <= range.end)
        .unwrap_or_else(|| {
            let mut start = 0;
            let mut end = sql.len();
            for (range, kind) in tokens_for(sql, database_type) {
                if kind != Kind::Separator {
                    continue;
                }
                if range.end <= offset {
                    start = range.end;
                } else {
                    end = range.start;
                    break;
                }
            }
            start..end
        })
}

/// Pieces of generated SQL: quotes, comments, dollar quotes, escapes of
/// each dialect, separators, and non-ASCII text.
const SQL_PARTS: [&str; 40] = [
    "SELECT ", "a", "b.c", " FROM ", "t x", " WHERE ", ";", ";", ";", "\n", "\n", " ", "\t", "'",
    "\"", "`", "''", "$$", "$a$", "$a", "$", "--", "/*", "*/", "\\", "E'", "e'", "U&'", "u&\"",
    "日本", "é", "(", ")", ", ", "1.5", "_x", "\r\n", "\u{3000}", "-", "*",
];

/// SQL of up to `parts` pieces.
fn generated_sql(random: &mut Random, parts: usize) -> String {
    let count = random.below(parts + 1);
    (0..count).map(|_| *random.pick(&SQL_PARTS)).collect()
}

fn char_boundary(random: &mut Random, sql: &str) -> usize {
    let boundaries: Vec<usize> = sql
        .char_indices()
        .map(|(index, _)| index)
        .chain([sql.len()])
        .collect();
    *random.pick(&boundaries)
}

#[test]
fn kept_statements_find_the_statement_of_a_new_read_after_edits() {
    let mut random = Random(0xfeed_5eed_0003);
    let mut checks = 0;
    for document in 0..240 {
        let database_type = [KYUUBI, DatabaseType::Postgres, DatabaseType::Trino][document % 3];
        let mut statements = Statements::new(database_type);
        let mut sql = generated_sql(&mut random, 60);
        for _ in 0..25 {
            for _ in 0..1 + random.below(4) {
                let offset = char_boundary(&mut random, &sql);
                let expected = first_statement_at(&sql, offset, database_type);
                // A request also changes the text of the index, so the
                // first request after an edit checks the read after it.
                let request = if random.chance(50) {
                    statements.request(&sql, offset)
                } else {
                    statements.set_text(&sql);
                    assert_eq!(
                        statements.statement_at(offset),
                        expected,
                        "{sql:?} at {offset} in {database_type:?}"
                    );
                    statements.request(&sql, offset)
                };
                assert_eq!(
                    request,
                    request_in(&sql, offset, database_type, expected),
                    "{sql:?} at {offset} in {database_type:?}"
                );
                checks += 1;
            }
            // An edit: an insert, a delete, or both, anywhere.
            let start = char_boundary(&mut random, &sql);
            let end = if random.chance(40) {
                let end = char_boundary(&mut random, &sql);
                end.max(start)
            } else {
                start
            };
            let insert = if random.chance(70) {
                generated_sql(&mut random, 3)
            } else {
                String::new()
            };
            sql.replace_range(start..end, &insert);
            if random.chance(3) {
                sql = generated_sql(&mut random, 60);
            }
        }
    }
    assert!(checks > 10_000, "{checks} checks");
}

#[test]
fn kept_statements_read_only_up_to_the_statement_at_the_cursor() {
    let line = "SELECT a FROM t WHERE b = 'x; y';\n";
    let mut sql = line.repeat(1_000);
    let mut statements = Statements::new(KYUUBI);
    // A cursor in the first statement reads only that statement.
    statements.request(&sql, 10);
    assert_eq!(statements.statements.len(), 1);
    assert_eq!(statements.read, line.len() - 1);
    // A cursor at the end reads all statements.
    statements.request(&sql, sql.len());
    assert_eq!(statements.statements.len(), 1_000);
    // An edit at the end keeps the statements before it.
    sql.push_str("SELECT * FROM fl");
    statements.request(&sql, sql.len());
    assert_eq!(statements.statements.len(), 1_001);
    assert_eq!(
        *statements.statements.last().unwrap(),
        sql.len() - 16..sql.len()
    );
    // An edit in the first statement keeps none.
    sql.insert(7, 'x');
    statements.request(&sql, 8);
    assert_eq!(statements.statements.len(), 1);
}
