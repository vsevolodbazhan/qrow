use crate::model::PREVIEW_ROWS;
use std::ops::Range;

/// A view over downloaded rows. Changing pages never executes SQL.
#[derive(Default)]
pub struct Pagination {
    page: usize,
}

impl Pagination {
    pub fn page(&self) -> usize {
        self.page
    }

    pub fn pages(&self, rows: usize) -> usize {
        rows.div_ceil(PREVIEW_ROWS).max(1)
    }

    pub fn range(&self, rows: usize) -> Range<usize> {
        let start = (self.page * PREVIEW_ROWS).min(rows);
        start..start.saturating_add(PREVIEW_ROWS).min(rows)
    }

    pub fn select(&mut self, page: usize, rows: usize) -> bool {
        if page >= self.pages(rows) || page == self.page {
            return false;
        }
        self.page = page;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_follow_incoming_rows_without_moving_the_current_page() {
        let mut pages = Pagination::default();
        assert_eq!(pages.range(0), 0..0);
        assert_eq!(pages.pages(0), 1);
        assert!(!pages.select(1, 1000));
        assert_eq!(pages.range(250), 0..250);
        assert_eq!(pages.range(1250), 0..1000);
        assert!(pages.select(1, 1250));
        assert_eq!(pages.range(1250), 1000..1250);
        assert_eq!(pages.range(2250), 1000..2000);
        assert_eq!(pages.page(), 1);
        assert!(!pages.select(3, 2250));
        assert!(pages.select(2, 2250));
        assert_eq!(pages.range(2250), 2000..2250);
        assert!(pages.select(0, 2250));
        assert_eq!(pages.range(2250), 0..1000);
    }
}
