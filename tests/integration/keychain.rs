#[cfg(target_os = "macos")]
#[test]
#[ignore = "Uses the real macOS Keychain for a temporary, randomly named test item"]
fn temporary_keychain_password_roundtrip() {
    use qrow::storage::{Credentials, Keychain};
    let id = uuid::Uuid::new_v4();
    Keychain
        .set_password(id, "qrow-synthetic-test-value")
        .unwrap();
    let result = Keychain.password(id);
    let cleanup = security_framework::passwords::delete_generic_password(
        "io.qrow.connection",
        &id.to_string(),
    );
    assert_eq!(&**result.as_ref().unwrap(), "qrow-synthetic-test-value");
    cleanup.unwrap();
}
