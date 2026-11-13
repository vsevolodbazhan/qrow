use super::*;

#[test]
fn schema_patterns_match_globs_without_case_and_exclude_wins() {
    let settings = CatalogSettings {
        include: vec!["sales_*".into(), "o?s".into()],
        exclude: vec!["*_tmp".into()],
        ..CatalogSettings::default()
    };
    assert!(settings.shows("Sales_EU"));
    assert!(settings.shows("ops"));
    assert!(!settings.shows("sales_eu_tmp"));
    assert!(!settings.shows("oops"));
    assert!(!settings.shows("hr"));
    assert!(CatalogSettings::default().shows("anything"));
    assert!(glob_match("a*b*c", "axxbyyc"));
    assert!(glob_match("*", ""));
    assert!(!glob_match("a*b", "a"));
    assert!(!glob_match("?", ""));
}

#[test]
fn schema_patterns_must_be_bounded_and_not_blank() {
    let mut settings = CatalogSettings {
        include: vec![" ".into()],
        exclude: vec![],
        ..CatalogSettings::default()
    };
    assert!(settings.validate().is_err());
    settings.include = vec!["x".repeat(MAX_SCHEMA_PATTERN + 1)];
    assert!(settings.validate().is_err());
    settings.include = vec!["a".into(); MAX_SCHEMA_PATTERNS + 1];
    assert!(settings.validate().is_err());
    settings.include = vec!["a*".into()];
    assert!(settings.validate().is_ok());
    let restored: Profile =
            serde_json::from_str(r#"{"id":"00000000-0000-0000-0000-000000000001","name":"a","host":"h","port":1,"username":"u","database":"d","parameters":{}}"#)
                .unwrap();
    // A connection saved before the schema tree existed did not browse
    // schemas, and it still does not.
    assert_eq!(restored.catalog, CatalogSettings::default());
}

#[test]
fn search_scope_toggles_round_trips_and_drops_missing_connections() {
    let first = Profile::default();
    let second = Profile::default();
    let mut scope = SearchScope::Current;
    scope.toggle(first.id);
    assert_eq!(scope, SearchScope::Connections(BTreeSet::from([first.id])));
    scope.toggle(second.id);
    assert!(scope.includes(second.id, None));
    assert!(!scope.includes(Uuid::new_v4(), Some(first.id)));
    scope.toggle(first.id);
    scope.toggle(second.id);
    assert_eq!(scope, SearchScope::All);
    assert!(SearchScope::Current.includes(first.id, Some(first.id)));
    assert!(!SearchScope::Current.includes(first.id, None));

    let mut workspace = Workspace {
        profiles: vec![first.clone()],
        tabs: vec![SavedTab::new(1, Some(first.id))],
        search_scope: SearchScope::Connections(BTreeSet::from([first.id, second.id])),
        ..Workspace::default()
    };
    let text = serde_json::to_string(&workspace).unwrap();
    workspace = serde_json::from_str(&text).unwrap();
    workspace.normalize();
    assert_eq!(
        workspace.search_scope,
        SearchScope::Connections(BTreeSet::from([first.id]))
    );
    workspace.profiles.clear();
    workspace.normalize();
    assert_eq!(workspace.search_scope, SearchScope::All);
    // The default scope is not saved, and files without it read as All.
    let saved = serde_json::to_value(&workspace).unwrap();
    assert!(saved.get("search_scope").is_none());
    let restored: Workspace = serde_json::from_value(saved).unwrap();
    assert_eq!(restored.search_scope, SearchScope::All);
}

#[test]
fn column_reads_default_for_saved_connections_and_round_trip_both_modes() {
    let profile = Profile::default();
    assert_eq!(profile.catalog_column_reads, CatalogColumnReads::Table);
    let mut saved = serde_json::to_value(&profile).unwrap();
    saved
        .as_object_mut()
        .unwrap()
        .remove("catalog_column_reads");
    let restored: Profile = serde_json::from_value(saved).unwrap();
    assert_eq!(restored.catalog_column_reads, CatalogColumnReads::Table);
    for mode in [CatalogColumnReads::Table, CatalogColumnReads::Schema] {
        let mut profile = profile.clone();
        profile.catalog_column_reads = mode;
        let text = serde_json::to_string(&profile).unwrap();
        assert_eq!(serde_json::from_str::<Profile>(&text).unwrap(), profile);
    }
}

#[test]
fn schema_refresh_policy_defaults_and_bounds() {
    // New connections do not browse schemas.
    let defaults = CatalogSettings::default();
    assert_eq!(defaults.refresh, CatalogRefresh::Disabled);
    assert!(!defaults.browses());
    assert_eq!(defaults.timeout_minutes, 30);
    // Settings saved with the schema tree but without a refresh choice
    // keep the tree with refreshes on request. Qrow ignores the removed
    // Logs option of older files, and the next save drops it.
    let restored: CatalogSettings =
        serde_json::from_str(r#"{"include":["a"],"exclude":[],"log_refreshes":true}"#).unwrap();
    let saved = serde_json::to_string(&restored).unwrap();
    assert!(!saved.contains("log_refreshes"), "{saved}");
    assert_eq!(restored.refresh, CatalogRefresh::Manual);
    assert!(restored.browses());
    assert_eq!(restored.timeout_minutes, 30);
    let text = serde_json::to_string(&defaults).unwrap();
    assert!(text.contains(r#""refresh":{"mode":"disabled"}"#), "{text}");
    assert_eq!(
        serde_json::from_str::<CatalogSettings>(&text).unwrap(),
        defaults
    );

    // Settings of an earlier build kept the period in the choice. They
    // still load, with the default period.
    let earlier: CatalogSettings =
        serde_json::from_str(r#"{"refresh":{"mode":"while_connected","minutes":15}}"#).unwrap();
    assert_eq!(earlier.refresh, CatalogRefresh::WhileConnected);
    assert_eq!(earlier.refresh_minutes, 60);

    for (minutes, valid) in [(4, false), (5, true), (10_080, true), (10_081, false)] {
        let settings = CatalogSettings {
            refresh: CatalogRefresh::WhileConnected,
            refresh_minutes: minutes,
            ..defaults.clone()
        };
        assert_eq!(settings.validate().is_ok(), valid, "{minutes}");
    }
    for (minutes, valid) in [(0, false), (1, true), (1_440, true), (1_441, false)] {
        let settings = CatalogSettings {
            timeout_minutes: minutes,
            ..defaults.clone()
        };
        assert_eq!(settings.validate().is_ok(), valid, "{minutes}");
    }
}

#[test]
fn assistant_notes_are_optional_bounded_and_repaired() {
    let mut profile = Profile {
        host: "localhost".into(),
        username: "user".into(),
        ..Profile::default()
    };
    // Profiles without notes do not save the field, and earlier files load.
    let json = serde_json::to_value(&profile).unwrap();
    assert!(json.get("assistant_notes").is_none(), "{json}");
    let restored: Profile = serde_json::from_value(json).unwrap();
    assert_eq!(restored.assistant_notes, "");

    profile.assistant_notes = "é".repeat(MAX_ASSISTANT_NOTES_BYTES / 2);
    assert!(profile.validate().is_ok());
    profile.assistant_notes.push('x');
    assert!(profile.validate().is_err());

    // An edited file with notes above the limit loses their end, on a
    // character boundary.
    profile.assistant_notes = format!("x{}", "é".repeat(MAX_ASSISTANT_NOTES_BYTES / 2));
    let mut workspace = Workspace {
        profiles: vec![profile],
        ..Workspace::default()
    };
    workspace.normalize();
    let notes = &workspace.profiles[0].assistant_notes;
    assert_eq!(notes.len(), MAX_ASSISTANT_NOTES_BYTES - 1);
    assert!(workspace.profiles[0].validate().is_ok());
}

fn rule(kind: SchemaRuleKind, from: &str, to: &str) -> SchemaRule {
    SchemaRule {
        kind,
        from: from.into(),
        to: to.into(),
    }
}

#[test]
fn a_dbt_project_is_optional_and_valid_with_an_absolute_manifest() {
    // An absolute path on Windows starts with a drive.
    let manifest = if cfg!(windows) {
        r"C:\projects\lake\target\manifest.json"
    } else {
        "/projects/lake/target/manifest.json"
    };
    let mut profile = Profile {
        host: "localhost".into(),
        username: "user".into(),
        ..Profile::default()
    };
    let json = serde_json::to_value(&profile).unwrap();
    assert!(json.get("dbt").is_none(), "{json}");

    profile.dbt = Some(DbtProject {
        manifest: manifest.into(),
        refresh: DbtRefresh::default(),
        schema_mapping: vec![],
    });
    assert!(profile.validate().is_ok());
    // A saved project without the optional fields loads with defaults.
    let json = serde_json::json!({"manifest": manifest});
    let project: DbtProject = serde_json::from_value(json).unwrap();
    assert_eq!(project.refresh, DbtRefresh::Automatic);
    assert!(project.schema_mapping.is_empty());

    profile.dbt.as_mut().unwrap().manifest = "target/manifest.json".into();
    assert!(profile.validate().is_err());
    let dbt = profile.dbt.as_mut().unwrap();
    dbt.manifest = manifest.into();
    dbt.schema_mapping = vec![rule(SchemaRuleKind::Exact, "analytics", "")];
    assert!(profile.validate().is_err());

    // An edited file keeps only the valid rules.
    let dbt = profile.dbt.as_mut().unwrap();
    dbt.schema_mapping = vec![
        rule(SchemaRuleKind::Prefix, "", "x"),
        rule(SchemaRuleKind::Prefix, "dbt_dev_", ""),
    ];
    let mut workspace = Workspace {
        profiles: vec![profile],
        ..Workspace::default()
    };
    workspace.normalize();
    let dbt = workspace.profiles[0].dbt.as_ref().unwrap();
    assert_eq!(
        dbt.schema_mapping,
        [rule(SchemaRuleKind::Prefix, "dbt_dev_", "")]
    );
}

#[test]
fn the_first_matching_rule_maps_a_dbt_schema() {
    let project = DbtProject {
        manifest: "/manifest.json".into(),
        refresh: DbtRefresh::Manual,
        schema_mapping: vec![
            rule(SchemaRuleKind::Exact, "Analytics", "prod_analytics"),
            rule(SchemaRuleKind::Prefix, "dbt_dev_", ""),
            rule(SchemaRuleKind::Prefix, "dbt_", "warehouse_"),
        ],
    };
    assert_eq!(project.map_schema("analytics"), "prod_analytics");
    assert_eq!(project.map_schema("DBT_DEV_core"), "core");
    assert_eq!(project.map_schema("dbt_finance"), "warehouse_finance");
    assert_eq!(project.map_schema("staging"), "staging");
    // A prefix that ends inside a character does not match.
    assert_eq!(project.map_schema("db"), "db");
    assert_eq!(project.map_schema("dbé"), "dbé");
}

#[test]
fn schema_rules_are_lines_of_text() {
    let rules =
        parse_schema_rules("dbt_dev_* = *\n\n  analytics = prod_analytics  \nstg_*=staging_*\n")
            .unwrap();
    assert_eq!(
        rules,
        [
            rule(SchemaRuleKind::Prefix, "dbt_dev_", ""),
            rule(SchemaRuleKind::Exact, "analytics", "prod_analytics"),
            rule(SchemaRuleKind::Prefix, "stg_", "staging_"),
        ]
    );
    assert_eq!(
        format_schema_rules(&rules),
        "dbt_dev_* = *\nanalytics = prod_analytics\nstg_* = staging_*"
    );
    assert_eq!(
        parse_schema_rules(&format_schema_rules(&rules)).unwrap(),
        rules
    );
    assert!(parse_schema_rules("").unwrap().is_empty());
    for invalid in [
        "analytics",
        "dbt_* = core",
        "* = *",
        "analytics = ",
        "a b = c",
    ] {
        let error = parse_schema_rules(&format!("x = y\n{invalid}")).unwrap_err();
        assert!(error.to_string().contains("line 2"), "{invalid}: {error}");
    }
}

#[test]
fn conversations_keep_the_record_of_sent_notes() {
    let mut conversation =
        AssistantConversation::new("thread", AssistantExecutionMode::AskBeforeRunning);
    let json = serde_json::to_value(&conversation).unwrap();
    assert!(json.get("sent_notes").is_none(), "{json}");
    conversation.sent_notes = Some(SentNotes {
        connection: Some(Uuid::new_v4()),
        digest: "00".into(),
    });
    let restored: AssistantConversation =
        serde_json::from_value(serde_json::to_value(&conversation).unwrap()).unwrap();
    assert_eq!(restored, conversation);
}

#[test]
fn default_profile_uses_a_neutral_spark_name() {
    assert_eq!(Profile::default().name, "Spark");
}

#[test]
fn profile_name_is_limited_to_sixty_characters() {
    let mut profile = Profile {
        name: "n".repeat(MAX_PROFILE_NAME),
        host: "localhost".into(),
        username: "user".into(),
        ..Profile::default()
    };
    assert!(profile.validate().is_ok());
    profile.name.push('n');
    assert!(profile.validate().is_err());
}

#[test]
fn old_profiles_restore_with_idle_disconnect_and_no_heartbeat() {
    let original = Profile::default();
    let mut json = serde_json::to_value(&original).unwrap();
    json.as_object_mut().unwrap().remove("lifecycle");
    let restored: Profile = serde_json::from_value(json).unwrap();
    assert_eq!(restored.id, original.id);
    assert_eq!(restored.lifecycle, ConnectionLifecycle::default());
    assert_eq!(restored.lifecycle.keep_alive_seconds, 0);
}

#[test]
fn a_lifecycle_without_a_response_timeout_waits_five_minutes() {
    // Lifecycles saved before the setting existed get the default.
    let restored: ConnectionLifecycle = serde_json::from_str(
        r#"{"idle_seconds":900,"keep_alive_seconds":0,"keep_alive_sql":"SELECT 1"}"#,
    )
    .unwrap();
    assert_eq!(restored.response_timeout_seconds, 300);
    let mut policy = ConnectionLifecycle::default();
    for (seconds, valid) in [(9, false), (10, true), (3_600, true), (3_601, false)] {
        policy.response_timeout_seconds = seconds;
        assert_eq!(policy.validate().is_ok(), valid, "{seconds}");
    }
}

#[test]
fn lifecycle_rejects_invalid_timers_and_multiple_heartbeat_statements() {
    let mut policy = ConnectionLifecycle {
        idle_seconds: 0,
        ..Default::default()
    };
    assert!(policy.validate().is_err());
    policy.idle_seconds = 900;
    policy.keep_alive_seconds = 604_801;
    assert!(policy.validate().is_err());
    policy.keep_alive_seconds = 300;
    policy.keep_alive_sql = "SELECT 1; SELECT 2".into();
    assert!(policy.validate().is_err());
    policy.keep_alive_sql = "".into();
    assert!(policy.validate().is_err());
    policy.keep_alive_sql = "SELECT 42".into();
    assert!(policy.validate().is_ok());
    let restored: ConnectionLifecycle =
        serde_json::from_str(&serde_json::to_string(&policy).unwrap()).unwrap();
    assert_eq!(restored, policy);
}

#[test]
fn old_profiles_restore_with_password_authentication_and_plain_tcp() {
    let original = Profile::default();
    let mut json = serde_json::to_value(&original).unwrap();
    let object = json.as_object_mut().unwrap();
    object.remove("tls");
    object.remove("authentication");
    let restored: Profile = serde_json::from_value(json).unwrap();
    assert_eq!(restored.id, original.id);
    assert_eq!(restored.authentication, Authentication::Password);
    assert!(!restored.tls);
    let workspace: Workspace = serde_json::from_value(serde_json::json!({
        "version": 5, "profiles": [], "tabs": [], "active_tab": 0,
    }))
    .unwrap();
    assert!(workspace.sign_ins.is_empty());
}

#[test]
fn a_saved_callback_port_loads_as_a_list() {
    let mut json = serde_json::to_value(SignIn::default()).unwrap();
    let object = json.as_object_mut().unwrap();
    object.remove("callback_ports");
    object.insert("callback_port".into(), serde_json::json!(8765));
    let restored: SignIn = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(restored.callback_ports, vec![8765]);
    json["callback_port"] = serde_json::json!(0);
    let restored: SignIn = serde_json::from_value(json).unwrap();
    assert!(restored.callback_ports.is_empty());
    let saved = serde_json::to_value(&restored).unwrap();
    assert_eq!(saved["callback_ports"], serde_json::json!([]));
}

#[test]
fn authentication_is_saved_with_the_sign_in_reference_only() {
    let sign_in = Uuid::new_v4();
    let profile = Profile {
        tls: true,
        authentication: Authentication::Oidc { sign_in },
        ..Profile::default()
    };
    let json = serde_json::to_value(&profile).unwrap();
    assert_eq!(
        json["authentication"],
        serde_json::json!({"method": "oidc", "sign_in": sign_in})
    );
    let restored: Profile = serde_json::from_value(json).unwrap();
    assert_eq!(restored, profile);
}

#[test]
fn sign_in_authentication_works_with_and_without_tls() {
    let mut profile = Profile {
        host: "kyuubi.example.test".into(),
        username: "analytics".into(),
        authentication: Authentication::Oidc {
            sign_in: Uuid::new_v4(),
        },
        ..Profile::default()
    };
    assert!(profile.validate().is_ok());
    profile.tls = true;
    assert!(profile.validate().is_ok());
}

#[test]
fn authentication_and_tls_are_part_of_the_session_identity() {
    let original = Profile::default();
    let mut updated = original.clone();
    updated.tls = true;
    assert!(!original.connection_identity_eq(&updated));
    let mut updated = original.clone();
    updated.authentication = Authentication::Oidc {
        sign_in: Uuid::new_v4(),
    };
    assert!(!original.connection_identity_eq(&updated));
}

fn sign_in() -> SignIn {
    SignIn {
        name: "Company".into(),
        issuer: "https://id.example.test/realms/data".into(),
        client_id: "qrow-desktop".into(),
        scopes: vec!["kyuubi".into(), "offline_access".into()],
        ..SignIn::default()
    }
}

#[test]
fn sign_in_validation_requires_https_and_a_client() {
    assert!(sign_in().validate().is_ok());
    let invalid: [fn(&mut SignIn); 7] = [
        |s| s.name = " ".into(),
        |s| s.issuer = "http://id.example.test".into(),
        |s| s.issuer = "https://id.example.test/?realm=x".into(),
        |s| s.issuer = "not a url".into(),
        |s| s.client_id = "".into(),
        |s| s.scopes = vec!["two words".into()],
        |s| s.resource = Some("relative".into()),
    ];
    for change in invalid {
        let mut value = sign_in();
        change(&mut value);
        assert!(value.validate().is_err(), "{value:?}");
    }
}

#[test]
fn legacy_oidc_database_hosts_are_ignored_and_removed_from_saved_settings() {
    let original = sign_in();
    let mut json = serde_json::to_value(&original).unwrap();
    json["allowed_hosts"] = serde_json::json!(["legacy.example.test"]);
    let restored: SignIn = serde_json::from_value(json).unwrap();
    assert_eq!(restored, original);
    assert!(
        serde_json::to_value(&restored)
            .unwrap()
            .get("allowed_hosts")
            .is_none()
    );
    let mut shared: serde_json::Value = serde_json::from_str(&original.to_shared_text()).unwrap();
    shared["database_hosts"] = serde_json::json!(["legacy.example.test"]);
    let pasted = SignIn::from_shared_text(&shared.to_string()).unwrap();
    pasted.validate().unwrap();
    assert!(
        serde_json::from_str::<serde_json::Value>(&pasted.to_shared_text())
            .unwrap()
            .get("database_hosts")
            .is_none()
    );
}

#[test]
fn shared_sign_in_text_has_the_settings_only() {
    let mut value = sign_in();
    value.scopes.push("profile".into());
    value.resource = Some("https://kyuubi.example.test".into());
    value.callback_ports = vec![8765, 8766];
    value.identity = Some(Identity {
        issuer: value.issuer.clone(),
        subject: "subject-1".into(),
        name: Some("Alice".into()),
        email: Some("alice@example.test".into()),
    });
    let text = value.to_shared_text();
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "qrow_sign_in": 1,
            "name": "Company",
            "issuer": "https://id.example.test/realms/data",
            "client_id": "qrow-desktop",
            "scopes": ["kyuubi", "offline_access"],
            "resource": "https://kyuubi.example.test",
            "callback_ports": [8765, 8766],
        })
    );
    let pasted = SignIn::from_shared_text(&text).unwrap();
    assert_ne!(pasted.id, value.id);
    assert_eq!(pasted.identity, None);
    assert_eq!(pasted.scopes, vec!["kyuubi", "offline_access"]);
    assert_eq!(pasted.resource, value.resource);
    assert_eq!(pasted.callback_ports, value.callback_ports);
    // Empty optional settings stay out of the text.
    let json: serde_json::Value = serde_json::from_str(
        &SignIn {
            scopes: vec![],
            ..sign_in()
        }
        .to_shared_text(),
    )
    .unwrap();
    for key in ["scopes", "resource", "callback_ports"] {
        assert!(json.get(key).is_none(), "{key}");
    }
}

#[test]
fn shared_sign_in_text_survives_a_chat_message() {
    let text = sign_in().to_shared_text();
    let message = format!("Our sign-in:\n```\n{text}\n```\nThanks!");
    assert_eq!(SignIn::from_shared_text(&message).unwrap().name, "Company");
    let braces = format!("Use {{your_username}} in the connection:\n{text}\nSee {{docs}}.");
    assert_eq!(SignIn::from_shared_text(&braces).unwrap().name, "Company");
    let compact =
        serde_json::to_string(&serde_json::from_str::<serde_json::Value>(&text).unwrap()).unwrap();
    assert_eq!(SignIn::from_shared_text(&compact).unwrap().name, "Company");
    let typographic = text.replace('"', "\u{201C}");
    assert_eq!(
        SignIn::from_shared_text(&typographic).unwrap().client_id,
        "qrow-desktop"
    );
    // Unknown fields from a later version of the same format are ignored.
    let mut json: serde_json::Value = serde_json::from_str(&text).unwrap();
    json["future"] = serde_json::json!(true);
    assert!(SignIn::from_shared_text(&json.to_string()).is_ok());
}

#[test]
fn shared_sign_in_text_rejects_other_or_invalid_content() {
    let not_a_sign_in = |text: &str| {
        let error = SignIn::from_shared_text(text).unwrap_err().to_string();
        assert!(
            error.contains("does not contain sign-in settings"),
            "{text}: {error}"
        );
    };
    not_a_sign_in("");
    not_a_sign_in("SELECT 1");
    not_a_sign_in("} {");
    not_a_sign_in(r#"{"name": "Company"}"#);
    not_a_sign_in(&format!(
        "{{\"qrow_sign_in\": 1, \"x\": \"{}\"}}",
        "x".repeat(70_000)
    ));
    let mut json: serde_json::Value = serde_json::from_str(&sign_in().to_shared_text()).unwrap();
    json["qrow_sign_in"] = serde_json::json!(SHARED_SIGN_IN_VERSION + 1);
    let error = SignIn::from_shared_text(&json.to_string()).unwrap_err();
    assert!(error.to_string().contains("newer version"), "{error}");
    json["qrow_sign_in"] = serde_json::json!(SHARED_SIGN_IN_VERSION);
    json.as_object_mut().unwrap().remove("client_id");
    let error = SignIn::from_shared_text(&json.to_string()).unwrap_err();
    assert!(error.to_string().contains("client ID"), "{error}");
    // The settings pass the same checks as Sign-in Settings.
    json["client_id"] = serde_json::json!("qrow-desktop");
    json["issuer"] = serde_json::json!("http://id.example.test");
    let error = SignIn::from_shared_text(&json.to_string()).unwrap_err();
    assert!(error.to_string().contains("HTTPS"), "{error}");
}

#[test]
fn pasted_sign_in_names_stay_unique_and_within_limit() {
    let used = ["Company", "Company copy"];
    assert_eq!(
        unique_sign_in_name("Other", |name| used.contains(&name)),
        "Other"
    );
    assert_eq!(
        unique_sign_in_name("Company", |name| used.contains(&name)),
        "Company copy 2"
    );
    let long = "x".repeat(MAX_SIGN_IN_NAME);
    let copy = unique_sign_in_name(&long, |name| name == long);
    assert!(copy.ends_with(" copy"));
    assert_eq!(copy.chars().count(), MAX_SIGN_IN_NAME);
}

#[test]
fn sign_in_scopes_and_requirements() {
    let value = sign_in();
    assert_eq!(
        value.requested_scopes(),
        vec!["openid", "profile", "email", "kyuubi", "offline_access"]
    );
    let mut renamed = value.clone();
    renamed.name = "Renamed".into();
    assert!(value.token_requirements_eq(&renamed));
    // The base scopes are always requested, so naming them changes
    // nothing.
    let mut same = value.clone();
    same.scopes.push("profile".into());
    assert!(value.token_requirements_eq(&same));
    let mut changed = value.clone();
    changed.scopes.push("groups".into());
    assert!(!value.token_requirements_eq(&changed));
}

#[test]
fn identity_display_prefers_email_then_name_then_subject() {
    let mut identity = Identity {
        issuer: "https://id.example.test".into(),
        subject: "s-1".into(),
        name: Some("Ada".into()),
        email: Some("ada@example.test".into()),
    };
    assert_eq!(identity.display(), "ada@example.test");
    identity.email = None;
    assert_eq!(identity.display(), "Ada");
    identity.name = None;
    assert_eq!(identity.display(), "s-1");
}

#[test]
fn postgres_tls_modes_preserve_legacy_profiles_and_session_identity() {
    let mut profile = Profile {
        database_type: DatabaseType::Postgres,
        ..Profile::default()
    };
    for (tls, expected) in [
        (false, PostgresSslMode::Disable),
        (true, PostgresSslMode::VerifyFull),
    ] {
        profile.tls = tls;
        let json = serde_json::to_string(&profile).unwrap();
        assert!(!json.contains("postgres_ssl_mode"));
        let restored: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.postgres_ssl_mode(), expected);
    }
    let legacy = profile.clone();
    profile.postgres_ssl_mode = Some(PostgresSslMode::VerifyFull);
    assert!(legacy.connection_identity_eq(&profile));
    for mode in [
        PostgresSslMode::Disable,
        PostgresSslMode::Require,
        PostgresSslMode::VerifyFull,
    ] {
        profile.postgres_ssl_mode = Some(mode);
        let json = serde_json::to_string(&profile).unwrap();
        let restored: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.postgres_ssl_mode(), mode);
        assert_eq!(
            legacy.connection_identity_eq(&restored),
            mode == PostgresSslMode::VerifyFull
        );
    }
    profile.database_type = DatabaseType::Kyuubi;
    let mut changed = profile.clone();
    changed.postgres_ssl_mode = Some(PostgresSslMode::Require);
    assert!(profile.connection_identity_eq(&changed));
    changed.tls = false;
    assert!(!profile.connection_identity_eq(&changed));
}

#[test]
fn connection_identity_excludes_name_and_lifecycle() {
    let original = Profile::default();
    let mut updated = original.clone();
    updated.name = "Renamed".into();
    updated.lifecycle.idle_seconds = 60;
    assert!(original.connection_identity_eq(&updated));

    updated.host = "other-host".into();
    assert!(!original.connection_identity_eq(&updated));
    updated = original.clone();
    updated.id = uuid::Uuid::new_v4();
    assert!(!original.connection_identity_eq(&updated));
}

#[test]
fn normalize_assigns_legacy_tabs_and_adds_one_tab_per_connection() {
    let first = Profile::default();
    let second = Profile::default();
    let mut unassigned = SavedTab::new(1, None);
    unassigned.sql = "select λ".into();
    let mut workspace = Workspace {
        version: 1,
        profiles: vec![first.clone(), second.clone()],
        tabs: vec![unassigned],
        active_tab: 0,
        active_tabs: BTreeMap::new(),
        ..Workspace::default()
    };
    workspace.normalize();
    assert_eq!(workspace.version, WORKSPACE_VERSION);
    assert_eq!(workspace.tabs[0].profile, Some(first.id));
    assert!(
        workspace
            .tabs
            .iter()
            .any(|tab| tab.profile == Some(second.id))
    );
    assert_eq!(workspace.active_tabs[&first.id], workspace.tabs[0].id);
    assert!(workspace.active_tabs.contains_key(&second.id));
    assert_eq!(workspace.tabs[0].sql, "select λ");
}

#[test]
fn copied_tab_titles_use_copy_suffixes_and_stay_within_limit() {
    let mut used = BTreeSet::new();
    let first = copied_tab_title("Query 1", |title| used.contains(title));
    used.insert(first.clone());
    let second = copied_tab_title("Query 1", |title| used.contains(title));
    assert_eq!(first, "Query 1 (Copy)");
    assert_eq!(second, "Query 1 (Copy 2)");
    assert!(second.chars().count() <= MAX_TAB_TITLE);

    let long = copied_tab_title(&"x".repeat(MAX_TAB_TITLE), |_| false);
    assert_eq!(long, format!("{} (Copy)", "x".repeat(MAX_TAB_TITLE - 7)));
}

#[test]
fn conversation_titles_fit_tabs_and_keep_connection_names_unique() {
    let long = "航".repeat(MAX_ASSISTANT_CONVERSATION_TITLE);
    let first = conversation_tab_title(&long, |_| false).unwrap();
    assert_eq!(first.chars().count(), MAX_TAB_TITLE);
    let second = conversation_tab_title(&long, |title| title == first).unwrap();
    assert_eq!(second, format!("{} (Copy)", "航".repeat(MAX_TAB_TITLE - 7)));
    assert_eq!(conversation_tab_title("  ", |_| false), None);
}

#[test]
fn copied_profile_names_are_unique_and_stay_within_limit() {
    let mut used = BTreeSet::new();
    let first = copied_profile_name("Spark", |name| used.contains(name));
    used.insert(first.clone());
    let second = copied_profile_name("Spark", |name| used.contains(name));
    assert_eq!(first, "Spark copy");
    assert_eq!(second, "Spark copy 2");
    assert!(second.chars().count() <= MAX_PROFILE_NAME);

    let long = copied_profile_name(&"x".repeat(MAX_PROFILE_NAME), |_| false);
    assert_eq!(long, format!("{} copy", "x".repeat(MAX_PROFILE_NAME - 5)));
}

#[test]
fn normalize_makes_tab_titles_unique_per_connection() {
    let first = Profile::default();
    let second = Profile::default();
    let first_id = first.id;
    let second_id = second.id;
    let mut first_tab = SavedTab::new(1, Some(first_id));
    first_tab.title = "Shared".into();
    let mut duplicate = SavedTab::new(2, Some(first_id));
    duplicate.title = "Shared".into();
    let mut other_connection = SavedTab::new(1, Some(second_id));
    other_connection.title = "Shared".into();
    let mut workspace = Workspace {
        version: WORKSPACE_VERSION,
        profiles: vec![first, second],
        tabs: vec![first_tab, duplicate, other_connection],
        active_tab: 0,
        active_tabs: BTreeMap::new(),
        ..Workspace::default()
    };

    workspace.normalize();

    assert_eq!(workspace.tabs[0].title, "Shared");
    assert_eq!(workspace.tabs[1].title, "Shared (Copy)");
    assert_eq!(workspace.tabs[2].title, "Shared");
}

#[test]
fn settings_restore_defaults_and_stay_within_supported_bounds() {
    let restored: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(restored, Settings::default());

    let mut settings = Settings {
        theme: String::new(),
        ui_scale: f32::INFINITY,
        ui_font_family: "   ".into(),
        editor_font_family: "   ".into(),
        editor_font_size: f32::NAN,
        editor_line_height: f32::NAN,
        editor_tab_size: 0,
        editor_smooth_caret: false,
        editor_smooth_scrolling: false,
        smooth_transitions: false,
        logs_font_family: "   ".into(),
        logs_font_size: f32::NAN,
        logs_line_height: f32::NAN,
        assistant_font_family: "   ".into(),
        assistant_font_size: f32::NAN,
        assistant_line_height: f32::NAN,
        assistant_smooth_caret: false,
        assistant_smooth_resize: false,
        assistant_buffer_replies: false,
        assistant: AssistantSettings::default(),
        export: crate::export::Settings::default(),
        export_replay_limit_mib: 2048,
        copy_format: CopyFormat::Markdown,
        results_minimized: false,
        check_for_updates: true,
        update_channel: None,
    };
    settings.sanitize();
    assert_eq!(settings, Settings::default());

    settings.ui_scale = 1.46;
    settings.editor_font_size = 33.;
    settings.editor_line_height = 2.1;
    settings.logs_font_size = 33.;
    settings.logs_line_height = 2.1;
    settings.assistant_font_size = 33.;
    settings.assistant_line_height = 2.1;
    settings.editor_tab_size = 9;
    settings.sanitize();
    assert_eq!(settings.ui_scale, 1.46);
    assert_eq!(settings.editor_font_size, MAX_EDITOR_FONT_SIZE);
    assert_eq!(settings.editor_line_height, MAX_LINE_HEIGHT);
    assert_eq!(settings.logs_font_size, MAX_EDITOR_FONT_SIZE);
    assert_eq!(settings.logs_line_height, MAX_LINE_HEIGHT);
    assert_eq!(settings.assistant_font_size, MAX_EDITOR_FONT_SIZE);
    assert_eq!(settings.assistant_line_height, MAX_LINE_HEIGHT);
    assert_eq!(settings.editor_tab_size, MAX_TAB_SIZE);
    let encoded = serde_json::to_string(&settings).unwrap();
    let mut restored: Settings = serde_json::from_str(&encoded).unwrap();
    restored.sanitize();
    assert_eq!(restored.ui_scale, 1.46);
    settings.ui_scale = 9.;
    settings.sanitize();
    assert_eq!(settings.ui_scale, MAX_UI_SCALE);
}

#[test]
fn copy_format_defaults_to_markdown_and_round_trips() {
    let restored: Settings = serde_json::from_str("{}").unwrap();
    assert_eq!(restored.copy_format, CopyFormat::Markdown);
    for format in CopyFormat::ALL {
        let settings = Settings {
            copy_format: format,
            ..Settings::default()
        };
        let encoded = serde_json::to_string(&settings).unwrap();
        let restored: Settings = serde_json::from_str(&encoded).unwrap();
        assert_eq!(restored.copy_format, format);
    }
    let settings: Settings = serde_json::from_str(r#"{"copy_format":"plain_text"}"#).unwrap();
    assert_eq!(settings.copy_format, CopyFormat::PlainText);
}

#[test]
fn legacy_settings_keep_editor_preferences_and_default_ui_font() {
    let mut settings: Settings = serde_json::from_str(
        r#"{"ui_scale":1.2,"editor_font_family":"Monaco","editor_font_size":16}"#,
    )
    .unwrap();
    settings.sanitize();
    assert_eq!(settings.ui_font_family, Settings::default().ui_font_family);
    assert_eq!(settings.editor_font_family, "Monaco");
    assert_eq!(settings.editor_font_size, 16.);
    assert_eq!(
        settings.editor_line_height,
        Settings::default().editor_line_height
    );
    assert_eq!(
        settings.logs_font_family,
        Settings::default().logs_font_family
    );
    assert_eq!(settings.logs_font_size, Settings::default().logs_font_size);
    assert_eq!(
        settings.logs_line_height,
        Settings::default().logs_line_height
    );
    assert_eq!(settings.ui_scale, 1.2);
    assert_eq!(
        settings.assistant_font_family,
        Settings::default().assistant_font_family
    );
    assert_eq!(
        settings.assistant_font_size,
        Settings::default().assistant_font_size
    );
    assert_eq!(
        settings.assistant_line_height,
        Settings::default().assistant_line_height
    );

    settings.ui_font_family = " Helvetica ".into();
    settings.sanitize();
    assert_eq!(settings.ui_font_family, "Helvetica");
    assert_eq!(settings.editor_font_size, 16.);
}

#[test]
fn removed_ui_font_size_does_not_prevent_restoring_settings() {
    let settings: Settings = serde_json::from_str(
            r#"{"ui_scale":1.2,"ui_font_family":"Helvetica","ui_font_size":20,"editor_font_family":"Monaco","editor_font_size":16}"#,
        )
        .unwrap();
    assert_eq!(settings.ui_scale, 1.2);
    assert_eq!(settings.ui_font_family, "Helvetica");
    assert_eq!(settings.editor_font_family, "Monaco");
    assert_eq!(settings.editor_font_size, 16.);
    let saved = serde_json::to_value(&settings).unwrap();
    assert!(saved.get("ui_font_size").is_none());
}

#[test]
fn assistant_settings_restore_safe_defaults_and_sanitize_values() {
    let restored: AssistantSettings = serde_json::from_str("{}").unwrap();
    assert_eq!(restored, AssistantSettings::default());
    assert!(!restored.enabled);
    assert_eq!(
        restored.default_execution_mode,
        AssistantExecutionMode::AskBeforeRunning
    );
    assert_eq!(restored.sql_keyword_case, KeywordCase::Uppercase);
    let lowercase: AssistantSettings =
        serde_json::from_str(r#"{"sql_keyword_case":"lowercase"}"#).unwrap();
    assert_eq!(lowercase.sql_keyword_case, KeywordCase::Lowercase);

    let mut settings = AssistantSettings {
        enabled: true,
        panel_width: f32::NAN,
        codex_executable: Some("   ".into()),
        claude_executable: Some(" ".into()),
        codex: AssistantModelChoice {
            model: Some(" gpt-test ".into()),
            reasoning_effort: Some(" high ".into()),
            service_tier: Some(" fast ".into()),
        },
        claude: AssistantModelChoice {
            model: Some(" opus ".into()),
            ..AssistantModelChoice::default()
        },
        ..AssistantSettings::default()
    };
    settings.sanitize();
    assert_eq!(settings.panel_width, DEFAULT_ASSISTANT_PANEL_WIDTH);
    assert!(!settings.enabled);
    assert_eq!(settings.data_sharing_notice_version, 0);
    assert_eq!(settings.codex_executable, None);
    assert_eq!(settings.claude_executable, None);
    assert_eq!(settings.codex.model.as_deref(), Some("gpt-test"));
    assert_eq!(settings.codex.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(settings.codex.service_tier.as_deref(), Some("fast"));
    assert_eq!(settings.claude.model.as_deref(), Some("opus"));

    settings.panel_width = MAX_ASSISTANT_PANEL_WIDTH + 1.;
    settings.enabled = true;
    settings.data_sharing_notice_version = ASSISTANT_DATA_SHARING_NOTICE_VERSION;
    settings.sanitize();
    assert_eq!(settings.panel_width, MAX_ASSISTANT_PANEL_WIDTH);
    assert!(settings.enabled);
}

#[test]
fn earlier_assistant_settings_and_conversations_load_as_codex() {
    let settings: AssistantSettings = serde_json::from_str(
        r#"{"model":"gpt-test","reasoning_effort":"high","service_tier":"fast"}"#,
    )
    .unwrap();
    assert_eq!(settings.harness, AssistantHarness::Codex);
    assert_eq!(settings.choice().model.as_deref(), Some("gpt-test"));
    assert_eq!(settings.choice().service_tier.as_deref(), Some("fast"));
    assert_eq!(settings.claude, AssistantModelChoice::default());
    // The Codex choices keep their field names, so an earlier Qrow reads them.
    let saved = serde_json::to_value(&settings).unwrap();
    assert_eq!(saved["model"], "gpt-test");
    assert_eq!(saved["claude"]["model"], serde_json::Value::Null);

    let mut claude = settings;
    claude.harness = AssistantHarness::Claude;
    claude.choice_mut().model = Some("opus".into());
    assert_eq!(claude.codex.model.as_deref(), Some("gpt-test"));
    assert_eq!(claude.choice().model.as_deref(), Some("opus"));

    let conversation: AssistantConversation = serde_json::from_str(
            r#"{"thread_id":"thread-1","title":"Old","title_source":"codex","last_activity":1,"execution_mode":"ask_before_running"}"#,
        )
        .unwrap();
    assert_eq!(conversation.harness, AssistantHarness::Codex);
    assert_eq!(conversation.title_source, AssistantTitleSource::Generated);
    assert_eq!(
        serde_json::to_value(&conversation).unwrap()["title_source"],
        "codex"
    );
}

#[test]
fn assistant_workspace_removes_invalid_threads() {
    let mut first =
        AssistantConversation::new(" thread-1 ", AssistantExecutionMode::RunAutomatically);
    first.title = format!("  {}  ", "x".repeat(MAX_ASSISTANT_CONVERSATION_TITLE + 5));
    let mut duplicate =
        AssistantConversation::new("thread-1", AssistantExecutionMode::AskBeforeRunning);
    duplicate.title = "Duplicate".into();
    let blank = AssistantConversation::new("   ", AssistantExecutionMode::AskBeforeRunning);
    let mut assistant = AssistantWorkspace {
        conversations: vec![first, duplicate, blank],
        ..AssistantWorkspace::default()
    };

    assistant.sanitize();

    assert_eq!(assistant.conversations.len(), 1);
    assert_eq!(assistant.conversations[0].thread_id, "thread-1");
    assert_eq!(
        assistant.conversations[0].title.chars().count(),
        MAX_ASSISTANT_CONVERSATION_TITLE
    );
    assert_eq!(
        assistant.conversations[0].execution_mode,
        AssistantExecutionMode::RunAutomatically
    );
}

#[test]
fn normalize_gives_each_tab_one_conversation_and_detaches_missing_tabs() {
    let first = Profile::default();
    let second = Profile::default();
    let first_tab = SavedTab::new(1, Some(first.id));
    let second_tab = SavedTab::new(1, Some(second.id));
    let conversation = |id: &str, tab: Option<Uuid>, activity: u64| {
        let mut conversation =
            AssistantConversation::new(id, AssistantExecutionMode::AskBeforeRunning);
        conversation.tab_id = tab;
        conversation.last_activity = activity;
        conversation
    };
    let mut linked = conversation("linked", Some(second_tab.id), 1);
    linked.detached_profile = Some(first.id);
    let mut missing = conversation("missing", Some(Uuid::new_v4()), 1);
    missing.detached_profile = Some(first.id);
    let mut gone_profile = conversation("gone-profile", None, 1);
    gone_profile.detached_profile = Some(Uuid::new_v4());
    let mut workspace = Workspace {
        version: WORKSPACE_VERSION,
        profiles: vec![first.clone(), second],
        tabs: vec![first_tab.clone(), second_tab.clone()],
        assistant: AssistantWorkspace {
            conversations: vec![
                conversation("older", Some(first_tab.id), 1),
                conversation("newer", Some(first_tab.id), 2),
                linked,
                missing,
                gone_profile,
            ],
            ..AssistantWorkspace::default()
        },
        ..Workspace::default()
    };

    workspace.normalize();

    let find = |id: &str| {
        workspace
            .assistant
            .conversations
            .iter()
            .find(|conversation| conversation.thread_id == id)
            .unwrap()
    };
    assert_eq!(find("newer").tab_id, Some(first_tab.id));
    assert_eq!(find("older").tab_id, None);
    assert_eq!(find("older").detached_profile, Some(first.id));
    assert_eq!(find("linked").tab_id, Some(second_tab.id));
    assert_eq!(find("linked").detached_profile, None);
    assert_eq!(find("missing").tab_id, None);
    assert_eq!(find("missing").detached_profile, Some(first.id));
    assert_eq!(find("gone-profile").detached_profile, None);
}

#[test]
fn version_three_selected_conversation_keeps_the_active_tab() {
    let first = Profile::default();
    let second = Profile::default();
    let first_tab = SavedTab::new(1, Some(first.id));
    let second_tab = SavedTab::new(1, Some(second.id));
    let mut open = AssistantConversation::new("open", AssistantExecutionMode::AskBeforeRunning);
    open.tab_id = Some(first_tab.id);
    let other = AssistantConversation::new("other", AssistantExecutionMode::AskBeforeRunning);
    let mut workspace = Workspace {
        version: 3,
        profiles: vec![first, second.clone()],
        tabs: vec![first_tab, second_tab.clone()],
        active_tab: 1,
        assistant: AssistantWorkspace {
            conversations: vec![open, other],
            legacy_selected_thread: Some(" open ".into()),
        },
        ..Workspace::default()
    };

    workspace.normalize();

    let open = workspace.assistant.conversation("open").unwrap();
    let other = workspace.assistant.conversation("other").unwrap();
    assert_eq!(workspace.version, WORKSPACE_VERSION);
    assert_eq!(open.tab_id, Some(second_tab.id));
    assert_eq!(open.detached_profile, None);
    assert_eq!(other.tab_id, None);
    assert_eq!(other.detached_profile, Some(second.id));
    assert_eq!(workspace.assistant.legacy_selected_thread, None);
    let saved = serde_json::to_value(&workspace.assistant).unwrap();
    assert!(saved.get("selected_thread").is_none());
}

#[test]
fn closing_a_tab_detaches_its_conversation() {
    let profile = Uuid::new_v4();
    let tab = Uuid::new_v4();
    let mut conversation =
        AssistantConversation::new("thread", AssistantExecutionMode::AskBeforeRunning);
    conversation.tab_id = Some(tab);
    let mut assistant = AssistantWorkspace {
        conversations: vec![conversation],
        ..AssistantWorkspace::default()
    };
    assert_eq!(
        assistant
            .conversation_for_tab(tab)
            .map(|c| c.thread_id.as_str()),
        Some("thread")
    );

    assistant.detach_tab(tab, Some(profile));

    assert!(assistant.conversation_for_tab(tab).is_none());
    let conversation = assistant.conversation("thread").unwrap();
    assert_eq!(conversation.tab_id, None);
    assert_eq!(conversation.detached_profile, Some(profile));
}

#[test]
fn assistant_workspace_removes_unstarted_threads() {
    let mut assistant = AssistantWorkspace {
        conversations: vec![
            AssistantConversation::new("used", AssistantExecutionMode::AskBeforeRunning),
            AssistantConversation::new("new", AssistantExecutionMode::AskBeforeRunning),
        ],
        ..AssistantWorkspace::default()
    };
    let unstarted = BTreeSet::from(["new".to_owned()]);

    assistant.remove_unstarted(&unstarted);

    assert_eq!(assistant.conversations.len(), 1);
    assert_eq!(assistant.conversations[0].thread_id, "used");
}

fn shared(settings: CatalogSettings) -> SharedCatalog {
    SharedCatalog {
        id: Uuid::new_v4(),
        name: "Lake".into(),
        settings,
        preferred: None,
    }
}

#[test]
fn a_member_uses_the_shared_settings() {
    let catalog = shared(CatalogSettings {
        refresh: CatalogRefresh::WhileConnected,
        include: vec!["sales_*".into()],
        ..CatalogSettings::default()
    });
    let mut profile = Profile {
        catalog: CatalogSettings {
            refresh: CatalogRefresh::Manual,
            include: vec!["hr".into()],
            ..CatalogSettings::default()
        },
        shared_catalog: Some(catalog.id),
        ..Profile::default()
    };
    let effective = effective_catalog(&profile, std::slice::from_ref(&catalog));
    assert_eq!(effective.include, ["sales_*"]);
    assert_eq!(effective.refresh, CatalogRefresh::WhileConnected);
    // A member with browsing off stays off.
    profile.catalog.refresh = CatalogRefresh::Disabled;
    let effective = effective_catalog(&profile, std::slice::from_ref(&catalog));
    assert!(!effective.browses());
    // A missing shared catalog leaves the settings of the connection.
    profile.catalog.refresh = CatalogRefresh::Manual;
    assert_eq!(effective_catalog(&profile, &[]), profile.catalog);
}

#[test]
fn normalize_repairs_shared_catalogs_and_drops_stale_references() {
    let mut used = shared(CatalogSettings {
        refresh: CatalogRefresh::Disabled,
        refresh_minutes: 0,
        ..CatalogSettings::default()
    });
    used.name = " ".into();
    let unused = shared(CatalogSettings::default());
    let member = Profile {
        name: "a".into(),
        shared_catalog: Some(used.id),
        ..Profile::default()
    };
    let other = Profile {
        name: "b".into(),
        shared_catalog: Some(Uuid::new_v4()),
        ..Profile::default()
    };
    used.preferred = Some(other.id);
    let mut workspace = Workspace {
        version: 4,
        profiles: vec![member, other],
        shared_catalogs: vec![used.clone(), unused],
        ..Workspace::default()
    };

    workspace.normalize();

    assert_eq!(workspace.version, WORKSPACE_VERSION);
    assert_eq!(workspace.shared_catalogs.len(), 1);
    let repaired = &workspace.shared_catalogs[0];
    assert_eq!(repaired.id, used.id);
    assert_eq!(repaired.name, "Shared metadata");
    // A shared catalog always browses, and invalid bounds get defaults.
    assert_eq!(repaired.settings.refresh, CatalogRefresh::Manual);
    assert_eq!(
        repaired.settings.refresh_minutes,
        CatalogSettings::default().refresh_minutes
    );
    // The preferred connection must be a member.
    assert_eq!(repaired.preferred, None);
    assert_eq!(workspace.profiles[0].shared_catalog, Some(used.id));
    assert_eq!(workspace.profiles[1].shared_catalog, None);
}

#[test]
fn shared_catalogs_round_trip_and_old_workspaces_have_none() {
    let catalog = shared(CatalogSettings::default());
    let profile = Profile {
        shared_catalog: Some(catalog.id),
        ..Profile::default()
    };
    let workspace = Workspace {
        profiles: vec![profile],
        shared_catalogs: vec![catalog],
        ..Workspace::default()
    };
    let json = serde_json::to_string(&workspace).unwrap();
    let restored: Workspace = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.shared_catalogs, workspace.shared_catalogs);
    assert_eq!(
        restored.profiles[0].shared_catalog,
        workspace.profiles[0].shared_catalog
    );
    // Version 4 files have no shared catalogs, and private catalogs do
    // not write the field.
    let private = serde_json::to_string(&Workspace::default()).unwrap();
    assert!(!private.contains("shared_catalog"));
    let restored: Workspace = serde_json::from_str(&private).unwrap();
    assert!(restored.shared_catalogs.is_empty());
}

#[test]
fn shared_catalog_names_are_required_and_bounded() {
    let mut catalog = shared(CatalogSettings::default());
    assert!(catalog.validate().is_ok());
    catalog.name = " ".into();
    assert!(catalog.validate().is_err());
    catalog.name = "x".repeat(MAX_SHARED_CATALOG_NAME + 1);
    assert!(catalog.validate().is_err());
}
