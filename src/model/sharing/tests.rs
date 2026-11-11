use super::*;
use crate::model::{CatalogRefresh, DbtProject, SignInProvider};
use uuid::Uuid;

fn sign_in() -> SignIn {
    SignIn {
        name: "Company".into(),
        issuer: "https://id.example.test/realms/data".into(),
        client_id: "qrow-desktop".into(),
        scopes: vec!["offline_access".into()],
        ..SignIn::default()
    }
}

fn profile() -> Profile {
    Profile {
        name: "Warehouse".into(),
        host: "kyuubi.example.test".into(),
        port: 10009,
        username: "alice".into(),
        database: "default".into(),
        parameters: BTreeMap::from([("spark.app.name".into(), "qrow".into())]),
        assistant_notes: "Partitioned by day.".into(),
        shared_catalog: Some(Uuid::new_v4()),
        dbt: Some(DbtProject {
            manifest: "/Users/alice/dbt/target/manifest.json".into(),
            refresh: Default::default(),
            schema_mapping: vec![],
        }),
        ..Profile::default()
    }
}

#[test]
fn shared_connection_text_has_the_settings_without_private_data() {
    let mut value = profile();
    let mut account = sign_in();
    account.identity = Some(crate::model::Identity {
        issuer: account.issuer.clone(),
        subject: "subject-1".into(),
        name: None,
        email: Some("alice@example.test".into()),
    });
    value.authentication = Authentication::Oidc {
        sign_in: account.id,
    };
    let catalog = CatalogSettings {
        include: vec!["sales_*".into()],
        refresh: CatalogRefresh::Manual,
        ..CatalogSettings::default()
    };
    let text = value.to_shared_text(&catalog, Some(&account));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        json,
        serde_json::json!({
            "qrow_connection": 1,
            "name": "Warehouse",
            "database_type": "kyuubi",
            "host": "kyuubi.example.test",
            "port": 10009,
            "username": "alice",
            "database": "default",
            "parameters": {"spark.app.name": "qrow"},
            "authentication": {
                "method": "oidc",
                "sign_in": {
                    "qrow_sign_in": 1,
                    "name": "Company",
                    "issuer": "https://id.example.test/realms/data",
                    "client_id": "qrow-desktop",
                    "scopes": ["offline_access"],
                },
            },
            "catalog": {
                "include": ["sales_*"],
                "exclude": [],
                "refresh": {"mode": "manual"},
                "refresh_minutes": 60,
                "timeout_minutes": 30,
            },
            "assistant_notes": "Partitioned by day.",
        })
    );
    for private in [
        value.id.to_string(),
        account.id.to_string(),
        value.shared_catalog.unwrap().to_string(),
        "manifest".into(),
        "subject-1".into(),
        "alice@example.test".into(),
        "password".into(),
    ] {
        assert!(!text.contains(&private), "{private} in {text}");
    }
}

#[test]
fn a_pasted_connection_is_new_and_has_the_same_settings() {
    let mut original = profile();
    original.database_type = DatabaseType::Postgres;
    original.port = 5432;
    original.postgres_ssl_mode = Some(PostgresSslMode::VerifyFull);
    original.tls = true;
    original.lifecycle.idle_seconds = 120;
    original.transfer.preset = crate::model::transfer::TransferPreset::Fast;
    original.catalog_column_reads = CatalogColumnReads::Schema;
    let text = original.to_shared_text(&original.catalog, None);
    let message = format!("Our database:\n```\n{text}\n```\nUse {{your name}}.");
    let pasted = Profile::from_shared_text(&message).unwrap();
    assert_eq!(pasted.sign_in, None);
    let profile = pasted.profile;
    assert_ne!(profile.id, original.id);
    assert_eq!(profile.shared_catalog, None);
    assert_eq!(profile.dbt, None);
    assert_eq!(
        Profile {
            id: original.id,
            shared_catalog: original.shared_catalog,
            dbt: original.dbt.clone(),
            ..profile
        },
        original
    );
    // Typographic quotes from a chat app are accepted.
    let typographic = text.replace('"', "\u{201C}");
    assert!(Profile::from_shared_text(&typographic).is_ok());
}

#[test]
fn shared_connection_text_rejects_other_or_invalid_content() {
    let error = |text: &str| Profile::from_shared_text(text).unwrap_err().to_string();
    for text in ["", "SELECT 1", r#"{"name": "Warehouse"}"#] {
        assert!(
            error(text).contains("does not contain connection settings"),
            "{text}"
        );
    }
    // Sign-in settings are not connection settings.
    assert!(error(&sign_in().to_shared_text()).contains("does not contain connection settings"));
    let text = profile().to_shared_text(&CatalogSettings::default(), None);
    let mut json: serde_json::Value = serde_json::from_str(&text).unwrap();
    json["qrow_connection"] = serde_json::json!(SHARED_CONNECTION_VERSION + 1);
    assert!(error(&json.to_string()).contains("newer version"));
    json["qrow_connection"] = serde_json::json!(SHARED_CONNECTION_VERSION);
    json["port"] = serde_json::json!("ten");
    assert!(error(&json.to_string()).contains("not valid"));
    json["port"] = serde_json::json!(10009);
    json["authentication"] = serde_json::json!({"method": "oidc"});
    assert!(error(&json.to_string()).contains("sign_in"));
    let mut bad_sign_in: serde_json::Value =
        serde_json::from_str(&sign_in().to_shared_text()).unwrap();
    bad_sign_in["issuer"] = serde_json::json!("http://id.example.test");
    json["authentication"] = serde_json::json!({"method": "oidc", "sign_in": bad_sign_in});
    assert!(error(&json.to_string()).contains("HTTPS"));
}

#[test]
fn pasted_settings_can_be_partial() {
    // A colleague enters their own username, and Save checks the fields.
    let partial =
        r#"{"qrow_connection": 1, "database_type": "postgres", "host": "db.example.test"}"#;
    let pasted = Profile::from_shared_text(partial).unwrap().profile;
    assert_eq!(pasted.host, "db.example.test");
    assert_eq!(pasted.username, "");
    assert_eq!(pasted.name, "");
    // A missing port is the default port of the database type.
    assert_eq!(pasted.port, 5432);
}

#[test]
fn trino_browser_authentication_is_shared() {
    let original = Profile {
        database_type: DatabaseType::Trino,
        tls: true,
        authentication: Authentication::TrinoExternal,
        trino_schema: "tiny".into(),
        ..profile()
    };
    let text = original.to_shared_text(&original.catalog, None);
    let pasted = Profile::from_shared_text(&text).unwrap();
    assert_eq!(pasted.profile.authentication, Authentication::TrinoExternal);
    assert_eq!(pasted.profile.trino_schema, "tiny");
}

#[test]
fn a_legacy_postgres_connection_keeps_its_tls_mode() {
    for (tls, mode) in [
        (true, PostgresSslMode::VerifyFull),
        (false, PostgresSslMode::Disable),
    ] {
        let legacy = Profile {
            database_type: DatabaseType::Postgres,
            port: 5432,
            tls,
            postgres_ssl_mode: None,
            ..profile()
        };
        let text = legacy.to_shared_text(&legacy.catalog, None);
        let pasted = Profile::from_shared_text(&text).unwrap().profile;
        assert_eq!(pasted.postgres_ssl_mode, Some(mode));
        // Settings without the mode read it from the TLS switch.
        let mut json: serde_json::Value = serde_json::from_str(&text).unwrap();
        json.as_object_mut().unwrap().remove("postgres_ssl_mode");
        let pasted = Profile::from_shared_text(&json.to_string())
            .unwrap()
            .profile;
        assert_eq!(pasted.postgres_ssl_mode, Some(mode));
    }
}

#[test]
fn a_placed_connection_gets_a_free_name_and_reuses_a_matching_sign_in() {
    let existing_sign_in = SignIn {
        name: "Ours".into(),
        callback_ports: vec![8765],
        ..sign_in()
    };
    let existing = profile();
    let shared = |name: &str, sign_in: Option<SignIn>| SharedConnectionSettings {
        profile: Profile {
            name: name.into(),
            authentication: sign_in
                .as_ref()
                .map_or(Authentication::Password, |sign_in| Authentication::Oidc {
                    sign_in: sign_in.id,
                }),
            ..profile()
        },
        sign_in,
    };
    let profiles = std::slice::from_ref(&existing);
    let sign_ins = std::slice::from_ref(&existing_sign_in);
    // The same provider settings use the sign-in of the workspace.
    let (placed, new) = place_connection(shared("Warehouse", Some(sign_in())), profiles, sign_ins);
    assert_eq!(placed.name, "Warehouse copy");
    assert_eq!(
        placed.authentication,
        Authentication::Oidc {
            sign_in: existing_sign_in.id
        }
    );
    assert_eq!(new, None);
    let (placed, new) = place_connection(shared("Lake", None), profiles, sign_ins);
    assert_eq!(placed.name, "Lake");
    assert_eq!(placed.authentication, Authentication::Password);
    assert_eq!(new, None);
    // Other settings make a new sign-in with a free name.
    let other = SignIn {
        name: "Ours".into(),
        issuer: "https://other.example.test".into(),
        ..sign_in()
    };
    let (placed, new) = place_connection(shared("Lake", Some(other)), profiles, sign_ins);
    let new = new.unwrap();
    assert_eq!(new.name, "Ours copy");
    assert_eq!(new.provider, SignInProvider::Oidc);
    assert_eq!(
        placed.authentication,
        Authentication::Oidc { sign_in: new.id }
    );
}
