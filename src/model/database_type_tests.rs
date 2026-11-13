use super::*;

#[test]
fn old_profiles_use_kyuubi_and_postgres_profiles_round_trip() {
    let mut json = serde_json::to_value(Profile::default()).unwrap();
    json.as_object_mut().unwrap().remove("database_type");
    assert_eq!(
        serde_json::from_value::<Profile>(json)
            .unwrap()
            .database_type,
        DatabaseType::Kyuubi
    );
    let mut profile = Profile {
        database_type: DatabaseType::Postgres,
        host: "localhost".into(),
        username: "qrow".into(),
        ..Profile::default()
    };
    assert_eq!(
        serde_json::from_str::<Profile>(&serde_json::to_string(&profile).unwrap()).unwrap(),
        profile
    );
    let old = profile.clone();
    profile.database_type = DatabaseType::Kyuubi;
    assert!(!profile.connection_identity_eq(&old));
    profile.database_type = DatabaseType::Postgres;
    profile.authentication = Authentication::Oidc {
        sign_in: Uuid::new_v4(),
    };
    assert!(
        profile
            .validate()
            .unwrap_err()
            .to_string()
            .contains("password authentication")
    );
}

#[test]
fn each_database_quotes_identifiers_in_its_dialect() {
    assert_eq!(
        DatabaseType::Postgres.quote_identifier("odd\"name"),
        "\"odd\"\"name\""
    );
    assert_eq!(
        DatabaseType::Kyuubi.quote_identifier("odd`name"),
        "`odd``name`"
    );
}
