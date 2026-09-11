use proptest::prelude::*;
use qrow::sql::{self, Kind};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn token_ranges_partition_any_unicode_input(chars in prop::collection::vec(any::<char>(), 0..2048)) {
        let source: String = chars.into_iter().collect();
        let mut offset = 0;
        for (range, _) in sql::tokens(&source) {
            prop_assert_eq!(range.start, offset);
            prop_assert!(range.end > range.start);
            prop_assert!(source.get(range.clone()).is_some(), "invalid UTF-8 boundary: {:?}", range);
            offset = range.end;
        }
        prop_assert_eq!(offset, source.len());
        // Validation must also be total for malformed and incomplete editor text.
        let _ = sql::validate_single(&source);
    }

    #[test]
    fn quoted_unicode_and_semicolons_cannot_create_extra_statements(chars in prop::collection::vec(any::<char>(), 0..512)) {
        let value: String = chars.into_iter().collect();
        let escaped = value.replace('\\', "\\\\").replace('\'', "''");
        let query = format!("SELECT '{escaped};' AS value;");
        prop_assert!(sql::validate_single(&query).is_ok());
        let multiple = format!("{query} SELECT 2");
        prop_assert!(sql::validate_single(&multiple).is_err());
        prop_assert_eq!(sql::tokens(&query).iter().filter(|(_,kind)| *kind == Kind::Separator).count(), 1);
    }

    #[test]
    fn nested_comments_do_not_count_as_statements(depth in 1usize..128) {
        let comment = format!("{} ; SELECT 'ignored'; {}", "/*".repeat(depth), "*/".repeat(depth));
        prop_assert!(sql::validate_single(&comment).is_err());
        let query = format!("{comment} SELECT 1; {comment}");
        prop_assert!(sql::validate_single(&query).is_ok());
    }
}
