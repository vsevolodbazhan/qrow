use super::*;

#[test]
fn dollar_quoted_bodies_keep_semicolons_and_quotes_inside_one_statement() {
    for sql in [
        "SELECT $$a;b'c$$",
        "DO $body$ BEGIN RAISE NOTICE 'ok'; END $body$",
        "SELECT $тег$a;b$тег$",
    ] {
        validate_single(sql).unwrap();
        assert_eq!(statement_ranges(sql).len(), 1);
    }
    assert!(validate_single("SELECT $$a;b$$; SELECT 2").is_err());
    assert_eq!(statement_ranges("SELECT $1; SELECT 2").len(), 2);
}
