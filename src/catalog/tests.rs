use super::*;

fn rows(columns: &[&str], rows: &[&[Option<&str>]]) -> MetadataRows {
    MetadataRows {
        columns: columns
            .iter()
            .map(|name| Column {
                name: (*name).into(),
                data_type: "STRING".into(),
            })
            .collect(),
        rows: rows
            .iter()
            .map(|row| row.iter().map(|value| value.map(str::to_owned)).collect())
            .collect(),
    }
}

fn table(name: &str) -> RelationEntry {
    RelationEntry {
        name: name.into(),
        kind: RelationKind::Table,
        comment: None,
    }
}

fn column(name: &str) -> CatalogColumn {
    CatalogColumn {
        name: name.into(),
        data_type: "INT".into(),
        comment: None,
    }
}

fn loaded() -> Catalog {
    let mut catalog = Catalog::new(&Profile::default());
    let settings = CatalogSettings::default();
    catalog.apply_schemas(vec!["sales".into(), "ops".into()], &settings, 1);
    catalog.apply_relations("sales", None, vec![table("orders"), table("items")], 1);
    catalog.apply_columns(
        "sales",
        None,
        BTreeMap::from([("orders".into(), vec![column("id")])]),
        1,
    );
    catalog
}

#[test]
fn parsing_keeps_only_the_requested_names_in_table_order() {
    let schemas = rows(
        &["TABLE_SCHEM", "TABLE_CATALOG"],
        &[
            &[Some("b"), None],
            &[Some("a"), None],
            &[Some("a"), None],
            &[None, None],
        ],
    );
    assert_eq!(parse_schemas(&schemas).unwrap(), ["a", "b"]);

    // `_` is a pattern character on the server, so `my_db` also returns `myxdb`.
    let relations = rows(
        &[
            "TABLE_CAT",
            "TABLE_SCHEM",
            "TABLE_NAME",
            "TABLE_TYPE",
            "REMARKS",
        ],
        &[
            &[None, Some("my_db"), Some("v"), Some("VIEW"), Some("A view")],
            &[None, Some("my_db"), Some("t"), Some("TABLE"), Some("")],
            &[None, Some("myxdb"), Some("other"), Some("TABLE"), None],
        ],
    );
    assert_eq!(
        parse_relations(&relations, "my_db", None).unwrap(),
        [
            table("t"),
            RelationEntry {
                name: "v".into(),
                kind: RelationKind::View,
                comment: Some("A view".into()),
            },
        ]
    );
    assert_eq!(
        parse_relations(&relations, "my_db", Some("t")).unwrap(),
        [table("t")]
    );

    let columns = rows(
        &[
            "TABLE_SCHEM",
            "TABLE_NAME",
            "COLUMN_NAME",
            "TYPE_NAME",
            "REMARKS",
            "ORDINAL_POSITION",
        ],
        &[
            &[
                Some("my_db"),
                Some("t"),
                Some("b"),
                Some("INT"),
                None,
                Some("2"),
            ],
            &[
                Some("my_db"),
                Some("t"),
                Some("a"),
                Some("INT"),
                Some("First"),
                Some("1"),
            ],
            &[
                Some("myxdb"),
                Some("t"),
                Some("c"),
                Some("INT"),
                None,
                Some("1"),
            ],
        ],
    );
    let parsed = parse_columns(&columns, "my_db", None).unwrap();
    assert_eq!(
        parsed["t"]
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
    assert_eq!(parsed["t"][0].comment.as_deref(), Some("First"));
    assert!(parse_relations(&rows(&["TABLE_SCHEM"], &[]), "a", None).is_err());
}

#[test]
fn a_relation_list_refresh_keeps_the_columns_of_remaining_relations() {
    let mut catalog = loaded();
    catalog.apply_relations("sales", None, vec![table("orders"), table("refunds")], 2);
    let schema = catalog.schema("sales").unwrap();
    let names: Vec<_> = schema.relations.as_ref().unwrap().keys().collect();
    assert_eq!(names, ["orders", "refunds"]);
    assert_eq!(
        catalog
            .relation("sales", "orders")
            .unwrap()
            .columns
            .as_ref()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(catalog.relation("sales", "refunds").unwrap().columns, None);
    assert_eq!(schema.fetched_at, Some(2));

    // The schema list refresh keeps the relations of remaining schemas.
    catalog.apply_schemas(vec!["sales".into()], &CatalogSettings::default(), 3);
    assert_eq!(catalog.relation_count(), 2);
    assert!(catalog.schema("ops").is_none());
}

#[test]
fn a_relation_refresh_updates_or_removes_only_that_relation() {
    let mut catalog = loaded();
    catalog.apply_relations(
        "sales",
        Some("orders"),
        vec![RelationEntry {
            name: "orders".into(),
            kind: RelationKind::View,
            comment: Some("Now a view".into()),
        }],
        2,
    );
    let orders = catalog.relation("sales", "orders").unwrap();
    assert_eq!(orders.kind, RelationKind::View);
    assert!(orders.columns.is_some());
    catalog.apply_relations("sales", Some("items"), vec![], 2);
    assert!(catalog.relation("sales", "items").is_none());
    assert!(catalog.relation("sales", "orders").is_some());

    // Without a relation list, Qrow does not make a partial one.
    catalog.apply_relations("ops", Some("x"), vec![table("x")], 2);
    assert!(catalog.schema("ops").unwrap().relations.is_none());
}

#[test]
fn a_schema_column_refresh_sets_every_relation() {
    let mut catalog = loaded();
    assert_eq!(
        catalog.relation("sales", "items").unwrap().columns,
        Some(vec![])
    );
    catalog.apply_columns(
        "sales",
        Some("items"),
        BTreeMap::from([("items".into(), vec![column("sku")])]),
        2,
    );
    assert_eq!(
        catalog
            .relation("sales", "items")
            .unwrap()
            .columns
            .as_ref()
            .unwrap()[0]
            .name,
        "sku"
    );
    assert_eq!(
        catalog.relation("sales", "orders").unwrap().fetched_at,
        Some(1)
    );
}

#[test]
fn errors_stay_on_their_node_and_a_success_clears_them() {
    let mut catalog = loaded();
    catalog.set_error(&Scope::Schema("sales".into()), "denied".into(), Uuid::nil());
    catalog.set_error(
        &Scope::Relation("sales".into(), "orders".into()),
        "broken".into(),
        Uuid::nil(),
    );
    assert_eq!(
        catalog.schema("sales").unwrap().error.as_deref(),
        Some("denied")
    );
    assert_eq!(
        catalog
            .relation("sales", "orders")
            .unwrap()
            .error
            .as_deref(),
        Some("broken")
    );
    // Only the member that ran the refresh sees the error.
    let other = Uuid::from_u128(1);
    let sales = catalog.schema("sales").unwrap();
    assert_eq!(sales.error_for(Uuid::nil()), Some("denied"));
    assert_eq!(sales.error_for(other), None);
    let orders = catalog.relation("sales", "orders").unwrap();
    assert_eq!(orders.error_for(Uuid::nil()), Some("broken"));
    assert_eq!(orders.error_for(other), None);
    let first = sales.error_id.unwrap();
    assert!(orders.error_id.is_some());
    catalog.set_error(&Scope::Schema("sales".into()), "denied".into(), Uuid::nil());
    assert_ne!(catalog.schema("sales").unwrap().error_id, Some(first));
    catalog.apply_relations("sales", None, vec![table("orders")], 2);
    let sales = catalog.schema("sales").unwrap();
    assert!(sales.error_id.is_none());
    assert_eq!((sales.error.as_deref(), sales.error_member), (None, None));
    catalog.apply_columns("sales", None, BTreeMap::new(), 2);
    let orders = catalog.relation("sales", "orders").unwrap();
    assert_eq!((orders.error.as_deref(), orders.error_member), (None, None));
    assert!(orders.error_id.is_none());
}

#[test]
fn filters_apply_to_the_schema_list_and_to_the_cache() {
    let settings = CatalogSettings {
        include: vec!["sales*".into(), "OPS".into()],
        exclude: vec!["*_tmp".into()],
        ..CatalogSettings::default()
    };
    let mut catalog = Catalog::new(&Profile::default());
    catalog.apply_schemas(
        vec![
            "sales".into(),
            "sales_tmp".into(),
            "ops".into(),
            "hr".into(),
        ],
        &settings,
        1,
    );
    assert_eq!(catalog.schemas.keys().collect::<Vec<_>>(), ["ops", "sales"]);
    catalog.retain(&CatalogSettings {
        include: vec![],
        exclude: vec!["o?s".into()],
        ..CatalogSettings::default()
    });
    assert_eq!(catalog.schemas.keys().collect::<Vec<_>>(), ["sales"]);
}

#[test]
fn names_are_quoted_for_sql() {
    assert_eq!(
        qualified_name("sales_eu", "Orders2"),
        "`sales_eu`.`Orders2`"
    );
    assert_eq!(qualified_name("select", "from"), "`select`.`from`");
    assert_eq!(qualified_name("2024", "a-b"), "`2024`.`a-b`");
    assert_eq!(quote_identifier("odd`name"), "`odd``name`");
    assert_eq!(quote_identifier(""), "``");
}

#[test]
fn scopes_cover_their_own_relations_only() {
    let schema = Scope::Schema("a".into());
    assert!(schema.covers(&Scope::Relation("a".into(), "t".into())));
    assert!(!schema.covers(&Scope::Relation("b".into(), "t".into())));
    assert!(Scope::Connection.covers(&schema));
    assert!(Scope::Connection.covers(&Scope::Relation("b".into(), "t".into())));
    assert!(!schema.covers(&Scope::Connection));
}

#[test]
fn the_cache_round_trips_without_errors_and_detects_other_connections() {
    let mut catalog = loaded();
    catalog.set_error(&Scope::Connection, "offline".into(), catalog.owner);
    let restored: Catalog = serde_json::from_slice(&serde_json::to_vec(&catalog).unwrap()).unwrap();
    assert_eq!(restored.error, None);
    assert_eq!(restored.error_member, None);
    assert_eq!(restored.schemas, catalog.schemas);
    let mut profile = Profile {
        id: catalog.owner,
        ..Profile::default()
    };
    let describes =
        |profile: &Profile| catalog.describes(profile.id, Some(&CatalogIdentity::of(profile)));
    assert!(describes(&profile));
    profile.database = "other".into();
    assert!(describes(&profile));
    profile.host = "elsewhere".into();
    assert!(!describes(&profile));
    // A shared catalog has no identity, so no member clears it.
    let shared = Catalog::empty(Uuid::new_v4(), None);
    assert!(shared.describes(shared.owner, None));
    assert!(!shared.describes(shared.owner, Some(&CatalogIdentity::of(&profile))));
}

#[test]
fn a_shared_catalog_has_the_browsing_members_with_their_settings() {
    use crate::model::{CatalogRefresh, SharedCatalog};
    let shared = SharedCatalog {
        id: Uuid::new_v4(),
        name: "Lake".into(),
        settings: CatalogSettings {
            refresh: CatalogRefresh::Manual,
            include: vec!["sales".into()],
            ..CatalogSettings::default()
        },
        preferred: None,
    };
    let browsing = CatalogSettings {
        refresh: CatalogRefresh::Manual,
        ..CatalogSettings::default()
    };
    let member = |name: &str, catalog: CatalogSettings| Profile {
        name: name.into(),
        catalog,
        shared_catalog: Some(shared.id),
        ..Profile::default()
    };
    let small = member("small", browsing.clone());
    let off = member("off", CatalogSettings::default());
    let large = member("large", browsing.clone());
    let private = Profile {
        name: "private".into(),
        catalog: browsing,
        ..Profile::default()
    };
    let profiles = vec![small.clone(), off.clone(), large, private.clone()];
    let catalogs = std::slice::from_ref(&shared);

    assert_eq!(catalog_key(&small, catalogs), shared.id);
    assert_eq!(catalog_key(&private, catalogs), private.id);
    assert_eq!(catalog_key(&small, &[]), small.id);

    let config = CatalogConfig::of(shared.id, &profiles, catalogs).unwrap();
    assert!(config.shared);
    assert_eq!(config.identity(), None);
    let members: Vec<_> = config.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(members, ["small", "large"]);
    assert_eq!(config.members[0].catalog.include, ["sales"]);
    assert!(config.member(off.id).is_none());

    let config = CatalogConfig::of(private.id, &profiles, catalogs).unwrap();
    assert!(!config.shared);
    assert_eq!(config.identity(), Some(CatalogIdentity::of(&private)));
    // A member has no private catalog, and a connection that does not
    // browse has no catalog.
    assert!(CatalogConfig::of(small.id, &profiles, catalogs).is_none());
    let none = [off];
    assert!(CatalogConfig::of(shared.id, &none, catalogs).is_none());
}

#[test]
fn a_member_sees_only_its_own_refreshes() {
    let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
    let status = Status {
        active: Some(Scope::Connection),
        runner: Some(a),
        queued: vec![
            Request {
                member: b,
                scope: Scope::Schema("sales".into()),
            },
            Request {
                member: a,
                scope: Scope::Schema("hr".into()),
            },
        ],
        done: 2,
        total: 5,
    };
    let mine = status.of_member(a);
    assert_eq!(mine.active, Some(Scope::Connection));
    assert_eq!((mine.done, mine.total), (2, 5));
    assert_eq!(mine.queued.len(), 1);
    let theirs = status.of_member(b);
    assert_eq!(theirs.active, None);
    assert_eq!((theirs.done, theirs.total), (0, 0));
    assert!(theirs.includes(&Scope::Schema("sales".into())));
    assert!(!theirs.includes(&Scope::Schema("hr".into())));
}
