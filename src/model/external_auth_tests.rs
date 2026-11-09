use super::*;
#[test]
fn legacy_trino_references_migrate_without_changing_oidc_or_connection_identity() {
    let external = SignIn {
        name: "Legacy Trino".into(),
        provider: SignInProvider::TrinoExternal,
        ..SignIn::default()
    };
    let oidc = SignIn {
        name: "OIDC".into(),
        ..SignIn::default()
    };
    let legacy_auth = serde_json::json!({"method": "trino_external", "sign_in": external.id});
    let authentication: Authentication = serde_json::from_value(legacy_auth).unwrap();
    assert_eq!(authentication, Authentication::TrinoExternal);
    let external_profile = Profile {
        database_type: DatabaseType::Trino,
        host: "trino.example".into(),
        username: "alice".into(),
        tls: true,
        authentication,
        ..Profile::default()
    };
    let oidc_profile = Profile {
        database_type: DatabaseType::Trino,
        authentication: Authentication::Oidc { sign_in: oidc.id },
        ..Profile::default()
    };
    let mut workspace = Workspace {
        profiles: vec![external_profile.clone(), oidc_profile.clone()],
        sign_ins: vec![external, oidc.clone()],
        ..Workspace::default()
    };
    workspace.normalize();
    assert_eq!(workspace.profiles, vec![external_profile, oidc_profile]);
    assert_eq!(workspace.sign_ins, vec![oidc]);
    let serialized = serde_json::to_value(workspace.profiles[0].authentication).unwrap();
    assert_eq!(serialized, serde_json::json!({"method":"trino_external"}));
    assert!(workspace.profiles[0].authentication.sign_in().is_none());
}
#[test]
fn legacy_oidc_and_external_sign_ins_round_trip_without_client_settings() {
    let old = SignIn {
        name: "OIDC".into(),
        issuer: "https://id.example".into(),
        client_id: "desktop".into(),
        ..SignIn::default()
    };
    let mut value = serde_json::to_value(&old).unwrap();
    value.as_object_mut().unwrap().remove("provider");
    let restored: SignIn = serde_json::from_value(value).unwrap();
    assert_eq!(restored.provider, SignInProvider::Oidc);
    restored.validate().unwrap();
    let external = SignIn {
        name: "Trino".into(),
        provider: SignInProvider::TrinoExternal,
        ..SignIn::default()
    };
    external.validate().unwrap();
    let shared = external.to_shared_text();
    assert!(!shared.contains("client_id") && !shared.contains("issuer"));
    let restored = SignIn::from_shared_text(&shared).unwrap();
    assert_eq!(restored.provider, SignInProvider::TrinoExternal);
    let profile = Profile {
        name: "Trino".into(),
        host: "trino.example".into(),
        username: "alice".into(),
        database_type: DatabaseType::Trino,
        tls: true,
        authentication: Authentication::TrinoExternal,
        ..Profile::default()
    };
    profile.validate().unwrap();
    let restored: Profile =
        serde_json::from_str(&serde_json::to_string(&profile).unwrap()).unwrap();
    assert_eq!(restored.authentication, profile.authentication);
    let unsupported = Profile {
        database_type: DatabaseType::Kyuubi,
        ..profile
    };
    assert!(unsupported.validate().is_err());
}
