use super::*;
use crate::model::DatabaseType;

#[test]
fn postgres_backslashes_do_not_escape_standard_strings_or_identifiers() {
    assert_eq!(
        statement_ranges_for(r"SELECT '\' AS slash; SELECT 2", DatabaseType::Postgres).len(),
        2
    );
    assert_eq!(
        statement_ranges_for(r#"SELECT "a\"; SELECT 2"#, DatabaseType::Postgres).len(),
        2
    );
    validate_single_for(r"SELECT E'a\';b'", DatabaseType::Postgres).unwrap();
}
