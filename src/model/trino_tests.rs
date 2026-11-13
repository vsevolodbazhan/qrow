use super::*;
#[test]
fn profiles_round_trip_and_schema_changes_replace_the_session() {
    let mut profile = Profile {
        database_type: DatabaseType::Trino,
        host: "localhost".into(),
        username: "qrow".into(),
        database: "tpch".into(),
        trino_schema: "tiny".into(),
        ..Profile::default()
    };
    assert_eq!(
        serde_json::from_str::<Profile>(&serde_json::to_string(&profile).unwrap()).unwrap(),
        profile
    );
    let old = profile.clone();
    profile.trino_schema = "sf1".into();
    assert!(!profile.connection_identity_eq(&old));
    profile.authentication = Authentication::Oidc {
        sign_in: Uuid::new_v4(),
    };
    assert!(profile.validate().is_err());
    profile.tls = true;
    profile.validate().unwrap();
    assert_eq!(
        DatabaseType::Trino.quote_identifier("odd\"name"),
        "\"odd\"\"name\""
    );
    assert_eq!(DatabaseType::Trino.default_port(), 8080);
}
