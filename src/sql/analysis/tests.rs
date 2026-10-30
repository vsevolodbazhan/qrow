use super::*;
use crate::catalog::{CatalogColumn, RelationKind, Schema};
use std::time::Instant;
use uuid::Uuid;

fn relation(columns: Option<Vec<CatalogColumn>>) -> Arc<Relation> {
    Arc::new(Relation {
        kind: RelationKind::Table,
        comment: Some("Order totals.".into()),
        fetched_at: None,
        error: None,
        error_member: None,
        error_id: None,
        columns,
    })
}

fn column(name: &str) -> CatalogColumn {
    CatalogColumn {
        name: name.into(),
        data_type: "decimal(18,2)".into(),
        comment: Some("Order total in the order currency.".into()),
    }
}

fn catalog() -> Catalog {
    let mut catalog = Catalog::empty(Uuid::new_v4(), None);
    catalog.schemas.insert(
        "sales".into(),
        Arc::new(Schema {
            relations: Some(BTreeMap::from([
                (
                    "orders".into(),
                    relation(Some(vec![
                        column("amount"),
                        column("amount_eur"),
                        column("sample_rate"),
                        column("order_id"),
                        column("order total"),
                        column("日本語"),
                    ])),
                ),
                ("refunds".into(), relation(None)),
                ("empty".into(), relation(Some(vec![]))),
            ])),
            ..Schema::default()
        }),
    );
    catalog
}

fn config(dialect: DatabaseType) -> CompletionConfig {
    CompletionConfig::new(
        dialect,
        "sales",
        "warehouse",
        CatalogSettings {
            refresh: crate::model::CatalogRefresh::Manual,
            ..CatalogSettings::default()
        },
        KeywordCase::Uppercase,
    )
}

fn at(sql: &str, index: &CatalogIndex, config: &CompletionConfig) -> CompletionBatch {
    let cursor = sql.find('|').unwrap();
    complete(&sql.replace('|', ""), cursor, index, config)
}

#[test]
fn scope_columns_rank_prefixes_then_fuzzy_matches_and_replace_the_whole_word() {
    let index = CatalogIndex::new(Some(Arc::new(catalog())), None);
    let result = at(
        "select am|unt from sales.orders o join sales.refunds r on o.order_id = r.order_id",
        &index,
        &config(DatabaseType::Kyuubi),
    );
    assert_eq!(
        &result
            .items
            .iter()
            .take(3)
            .map(Candidate::label)
            .collect::<Vec<_>>(),
        &["amount", "amount_eur", "sample_rate"]
    );
    assert_eq!(result.items[0].replacement(), 7..12);
    assert_eq!(result.items[0].detail(), Some("decimal(18,2)"));
    assert!(
        result.items[0]
            .documentation()
            .starts_with("sales.orders.amount\n\nOrder total")
    );
    assert_eq!(result.notes.len(), 1);
    assert_eq!(
        result.notes[0].refresh(),
        Some(&Scope::Relation("sales".into(), "refunds".into()))
    );
}

#[test]
fn qualifiers_quoted_names_and_unicode_have_byte_safe_replacements() {
    let index = CatalogIndex::new(Some(Arc::new(catalog())), None);
    for dialect in [
        DatabaseType::Kyuubi,
        DatabaseType::Postgres,
        DatabaseType::Trino,
    ] {
        let result = at(
            "select '😀', o.日|本語 from sales.orders o",
            &index,
            &config(dialect),
        );
        let item = result
            .items
            .iter()
            .find(|item| item.label() == "日本語")
            .unwrap();
        let mut source = "select '😀', o.日本語 from sales.orders o".to_string();
        source.replace_range(item.replacement(), item.insert());
        assert_eq!(
            source,
            format!(
                "select '😀', o.{} from sales.orders o",
                dialect.quote_identifier("日本語")
            )
        );
        let quote = if dialect == DatabaseType::Kyuubi {
            '`'
        } else {
            '"'
        };
        let text = format!("select o.{quote}order t|otal{quote} from sales.orders o");
        let result = at(&text, &index, &config(dialect));
        assert_eq!(
            result.items[0].insert(),
            format!("{quote}order total{quote}")
        );
        assert_eq!(result.items[0].replacement().len(), "order total".len() + 2);
    }
}

#[test]
fn schemas_relations_default_schema_and_keyword_case() {
    let index = CatalogIndex::new(Some(Arc::new(catalog())), None);
    for text in [
        "select * from sales.or|",
        "select * from or|",
        "select * from warehouse.sales.or|",
    ] {
        let result = at(text, &index, &config(DatabaseType::Trino));
        assert_eq!(result.items[0].label(), "orders");
        assert_eq!(result.items[0].insert(), "\"orders\"");
    }
    let result = at("select * from sa|", &index, &config(DatabaseType::Trino));
    assert!(
        result
            .items
            .iter()
            .any(|item| item.label() == "sales" && item.kind() == CompletionKind::Schema)
    );
    let mut lower = config(DatabaseType::Trino);
    lower.keyword_case = KeywordCase::Lowercase;
    let result = at("select co|", &index, &lower);
    assert!(result.items.iter().any(|item| item.label() == "count"));
    assert!(!result.items.iter().any(|item| item.label() == "COUNT"));
}

#[test]
fn cte_and_subquery_names_stay_in_their_query_scope() {
    let index = CatalogIndex::new(Some(Arc::new(catalog())), None);
    let result = at(
        "with recent(total) as (select amount from sales.orders) select r.to| from recent r",
        &index,
        &config(DatabaseType::Kyuubi),
    );
    assert_eq!(result.items[0].label(), "total");
    assert!(result.notes.is_empty());
    let result = at(
        "with recent as (select amount as total from sales.orders) select * from re|",
        &index,
        &config(DatabaseType::Kyuubi),
    );
    assert_eq!(result.items[0].label(), "recent");
    let result = at(
        "select inner_table.am| from (select amount from sales.orders) inner_table",
        &index,
        &config(DatabaseType::Kyuubi),
    );
    assert_eq!(result.items[0].label(), "amount");
    let result = at(
        "select (select o.am|) from sales.orders o",
        &index,
        &config(DatabaseType::Kyuubi),
    );
    assert!(result.items.iter().any(|item| item.label() == "amount"));
    let result = at(
        "select * from sales.orders o; select o.am|",
        &index,
        &config(DatabaseType::Kyuubi),
    );
    assert!(
        !result
            .items
            .iter()
            .any(|item| item.kind() == CompletionKind::Column)
    );
}

#[test]
fn word_start_set_operations_quoted_aliases_and_cte_shadowing() {
    let mut cache = catalog();
    Arc::make_mut(cache.schemas.get_mut("sales").unwrap())
        .relations
        .as_mut()
        .unwrap()
        .insert(
            "refunds".into(),
            relation(Some(vec![column("refund_amount")])),
        );
    let index = CatalogIndex::new(Some(Arc::new(cache)), None);
    let result = at(
        "select o.|amount from sales.orders o",
        &index,
        &config(DatabaseType::Postgres),
    );
    assert_eq!(result.items[0].replacement(), 9..15);
    for operation in ["UNION", "INTERSECT", "EXCEPT"] {
        let result = at(
            &format!("select | from sales.orders o {operation} select * from sales.refunds r"),
            &index,
            &config(DatabaseType::Postgres),
        );
        assert!(
            !result
                .items
                .iter()
                .any(|item| item.label() == "refund_amount")
        );
        assert!(result.items.iter().any(|item| item.label() == "amount"));
    }
    let result = at(
        "select \"A\".| from sales.orders \"a\" join sales.refunds \"A\" on true",
        &index,
        &config(DatabaseType::Postgres),
    );
    assert_eq!(result.items[0].label(), "refund_amount");
    let result = at(
        "with c as (select amount from sales.orders) select (with c as (select refund_amount from sales.refunds) select c.| from c)",
        &index,
        &config(DatabaseType::Postgres),
    );
    assert_eq!(result.items[0].label(), "refund_amount");
    let result = at(
        "select * from sales.|",
        &index,
        &config(DatabaseType::Postgres),
    );
    assert!(result.items.iter().any(|item| item.label() == "orders"));
    assert!(result.notes.is_empty());
}

#[test]
fn postgres_unquoted_aliases_and_projected_names_fold_to_lowercase() {
    let index = CatalogIndex::new(Some(Arc::new(catalog())), None);
    for sql in [
        "select O.am| from sales.orders O",
        "with C as (select amount AS Total from sales.orders) select C.to| from C",
    ] {
        let result = at(sql, &index, &config(DatabaseType::Postgres));
        assert!(!result.items.is_empty());
        assert!(result.notes.is_empty());
        assert!(result.items[0].insert().starts_with('"'));
    }
}

#[test]
fn reserved_catalog_names_are_quoted_in_every_dialect() {
    let mut cache = catalog();
    Arc::make_mut(cache.schemas.get_mut("sales").unwrap())
        .relations
        .as_mut()
        .unwrap()
        .insert(
            "users".into(),
            relation(Some(vec![
                column("user"),
                column("current_user"),
                column("check"),
            ])),
        );
    let index = CatalogIndex::new(Some(Arc::new(cache)), None);
    for dialect in [
        DatabaseType::Kyuubi,
        DatabaseType::Postgres,
        DatabaseType::Trino,
    ] {
        let result = at("select u.| from sales.users u", &index, &config(dialect));
        for item in result.items() {
            assert_eq!(item.insert(), dialect.quote_identifier(item.label()));
        }
    }
}

#[test]
fn cache_states_have_scoped_notes_and_loaded_empty_columns_have_no_note() {
    let mut catalog = catalog();
    catalog
        .schemas
        .insert("unread".into(), Arc::new(Schema::default()));
    let index = CatalogIndex::new(Some(Arc::new(catalog)), None);
    for (sql, text, refresh) in [
        (
            "select r.| from sales.refunds r",
            "Columns of sales.refunds are not loaded.",
            Some(Scope::Relation("sales".into(), "refunds".into())),
        ),
        (
            "select r.| from sales.missing r",
            "sales.missing is not in the catalog.",
            Some(Scope::Relation("sales".into(), "missing".into())),
        ),
        (
            "select r.| from unread.orders r",
            "Tables of unread are not loaded.",
            Some(Scope::Schema("unread".into())),
        ),
        (
            "select r.| from unknown.orders r",
            "unknown is not in the catalog.",
            None,
        ),
    ] {
        let result = at(sql, &index, &config(DatabaseType::Trino));
        assert!(result.items.is_empty());
        assert_eq!(result.notes[0].text(), text);
        assert_eq!(result.notes[0].refresh(), refresh.as_ref());
    }
    let result = at(
        "select e.| from sales.empty e",
        &index,
        &config(DatabaseType::Trino),
    );
    assert!(result.items.is_empty() && result.notes.is_empty());
    let mut hidden = config(DatabaseType::Trino);
    hidden.filters.exclude.push("sales".into());
    let result = at("select o.| from sales.orders o", &index, &hidden);
    assert_eq!(
        result.notes[0].text(),
        "sales is hidden by the catalog filters."
    );
    assert!(result.notes[0].refresh().is_none());
}

#[test]
fn disabled_catalogs_and_failed_refreshes_keep_their_actual_cache_state() {
    let mut cache = catalog();
    Arc::make_mut(cache.schemas.get_mut("sales").unwrap()).error = Some("Permission denied".into());
    let relation = Arc::make_mut(cache.schemas.get_mut("sales").unwrap())
        .relations
        .as_mut()
        .unwrap()
        .get_mut("orders")
        .unwrap();
    Arc::make_mut(relation).error = Some("Read failed".into());
    let index = CatalogIndex::new(Some(Arc::new(cache)), None);
    let mut settings = config(DatabaseType::Postgres);
    let result = at("select o.am| from sales.orders o", &index, &settings);
    assert_eq!(result.items[0].label(), "amount");
    assert!(result.notes[0].text().contains("Read failed"));
    let result = at("select m.| from sales.MISSING m", &index, &settings);
    assert!(result.notes[0].text().contains("Permission denied"));
    assert_eq!(
        result.notes[0].refresh(),
        Some(&Scope::Schema("sales".into()))
    );
    settings.filters.refresh = crate::model::CatalogRefresh::Disabled;
    let result = at("select o.am| from sales.orders o", &index, &settings);
    assert!(result.items.is_empty());
    assert_eq!(
        result.notes[0].text(),
        "The catalog is disabled for this connection."
    );
    assert!(result.notes[0].refresh().is_none());
    let result = at(
        "with c(x) as (select 1) select c.x| from c",
        &index,
        &settings,
    );
    assert_eq!(result.items[0].label(), "x");
    assert!(result.notes.is_empty());
}

#[test]
fn strings_comments_and_invalid_cursor_offsets_do_not_complete() {
    let index = CatalogIndex::new(Some(Arc::new(catalog())), None);
    for sql in [
        "select 'o.am|'",
        "select 1 -- o.am|",
        "select /* o.am| */ 1",
        "select $$o.am|$$",
    ] {
        let result = at(sql, &index, &config(DatabaseType::Postgres));
        assert!(result.items.is_empty() && result.notes.is_empty());
    }
    assert!(
        complete("日本語", 1, &index, &config(DatabaseType::Trino))
            .items
            .is_empty()
    );
}

#[test]
fn incremental_indexes_reuse_unchanged_relations_and_handle_large_catalogs() {
    let mut catalog = catalog();
    let schema = Arc::make_mut(catalog.schemas.get_mut("sales").unwrap());
    for i in 0..5000 {
        schema.relations.as_mut().unwrap().insert(
            format!("table_{i:04}"),
            relation(Some(vec![column("id"), column("value")])),
        );
    }
    let started = Instant::now();
    let index = CatalogIndex::new(Some(Arc::new(catalog.clone())), None);
    let build = started.elapsed();
    Arc::make_mut(catalog.schemas.get_mut("sales").unwrap())
        .relations
        .as_mut()
        .unwrap()
        .insert("added".into(), relation(None));
    let started = Instant::now();
    let next = CatalogIndex::new(Some(Arc::new(catalog)), Some(&index));
    let rebuild = started.elapsed();
    let key = ("sales".into(), "orders".into());
    assert!(Arc::ptr_eq(&index.relations[&key], &next.relations[&key]));
    let started = Instant::now();
    for _ in 0..100 {
        let result = at(
            "select * from table_49|",
            &next,
            &config(DatabaseType::Kyuubi),
        );
        assert_eq!(result.items[0].label(), "table_4900");
    }
    eprintln!(
        "completion index: build {build:?}, incremental {rebuild:?}, 100 requests {:?}",
        started.elapsed()
    );
    assert!(
        started.elapsed().as_secs() < 5,
        "completion must stay interactive with 5,000 relations"
    );
}
