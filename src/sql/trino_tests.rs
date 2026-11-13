use super::*;
use crate::model::DatabaseType;
#[test]
fn trino_uses_standard_strings_and_double_quoted_names() {
    let sql = r#"SELECT 'a\' AS "odd;name"; SELECT 2"#;
    assert_eq!(statement_ranges_for(sql, DatabaseType::Trino).len(), 2);
    assert!(validate_single_for(sql, DatabaseType::Trino).is_err());
    assert!(
            tokens_for(sql, DatabaseType::Trino)
                .iter()
                .any(|(range, kind)| *kind == Kind::Identifier
                    && &sql[range.clone()] == "\"odd;name\"")
        );
    assert!(validate_single_for("SELECT '$x$'; SELECT 2", DatabaseType::Trino).is_err());
}
