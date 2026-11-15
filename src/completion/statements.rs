//! The statement at the cursor of an editor, found again after each edit
//! or cursor move without a new read of the whole SQL.

use super::{Request, request_in};
use crate::model::DatabaseType;
use crate::sql::{Kind, Tokens};
use std::ops::Range;

/// The statements of one editor text, read from its start up to the
/// statement at the cursor of the last request. Keep it while the editor
/// changes:
///
/// - A cursor move or a caret blink reads no SQL again.
/// - An edit keeps the tokens before the last `;` before the edit, and the
///   next request reads from there to the statement at its cursor.
///
/// The statement at the cursor is the one that [`super::request`] gives for
/// the whole text: [`crate::sql::StatementIndex::range_at`] when it has the
/// cursor, else the text between the `;` before and after the cursor.
pub struct Statements {
    database_type: DatabaseType,
    /// The text of the index.
    sql: String,
    /// Where the read of `sql` stopped: 0, the end of a `;`, or the end of
    /// `sql`. The tokens from there do not depend on the tokens before.
    pub(super) read: usize,
    /// The statements before `read`, as [`crate::sql::statement_ranges`]
    /// gives them.
    pub(super) statements: Vec<Range<usize>>,
    /// The comments before `read`.
    comments: Vec<Range<usize>>,
    /// The `;` tokens before `read`.
    separators: Vec<Range<usize>>,
    /// The last request and its cursor, while `sql` stays the same.
    last: Option<(usize, Option<Request>)>,
}

impl Statements {
    pub fn new(database_type: DatabaseType) -> Self {
        Self {
            database_type,
            sql: String::new(),
            read: 0,
            statements: Vec::new(),
            comments: Vec::new(),
            separators: Vec::new(),
            last: None,
        }
    }

    pub fn database_type(&self) -> DatabaseType {
        self.database_type
    }

    /// The request at byte `offset` of `sql`, as [`super::request`] gives it.
    pub fn request(&mut self, sql: &str, offset: usize) -> Option<Request> {
        self.set_text(sql);
        let offset = offset.min(sql.len());
        if let Some((at, request)) = &self.last
            && *at == offset
        {
            return request.clone();
        }
        let statement = self.statement_at(offset);
        let request = request_in(&self.sql, offset, self.database_type, statement);
        self.last = Some((offset, request.clone()));
        request
    }

    /// Changes the text of the index to `sql`. Only the tokens before the
    /// last `;` before the first changed byte stay.
    ///
    /// A token of the tokenizer depends only on the bytes from the byte
    /// before it up to the second byte after it, except a `$` that can
    /// start a dollar quote, which looks for the next `$`. For the tokens up
    /// to a `;` before the change, these bytes are before the `;` or are the
    /// `;`, and do not change. A `$` before the `;` either finds the next
    /// `$` before the `;`, or its tag has the `;` and the `$` starts no
    /// dollar quote. The tokens from the end of the `;` do not depend on
    /// the tokens before it.
    pub(super) fn set_text(&mut self, sql: &str) {
        let mut same = common_prefix(self.sql.as_bytes(), sql.as_bytes());
        if same == self.sql.len() && same == sql.len() {
            return;
        }
        while !self.sql.is_char_boundary(same) {
            same -= 1;
        }
        let kept = self.separators.partition_point(|range| range.end <= same);
        let restart = kept
            .checked_sub(1)
            .map_or(0, |last| self.separators[last].end);
        self.separators.truncate(kept);
        let statements = self
            .statements
            .partition_point(|range| range.end <= restart);
        self.statements.truncate(statements);
        let comments = self.comments.partition_point(|range| range.end <= restart);
        self.comments.truncate(comments);
        self.read = restart;
        self.sql.truncate(same);
        self.sql.push_str(&sql[same..]);
        self.last = None;
    }

    /// Reads the text from where the last read stopped, up to the end of the
    /// first `;` that ends after `offset`, or up to the end of the text.
    fn read_past(&mut self, offset: usize) {
        if self.read >= self.sql.len()
            || self
                .separators
                .last()
                .is_some_and(|range| range.end > offset)
        {
            return;
        }
        // The same steps as `statement_ranges_in`, from a `;`.
        let mut start = None;
        let mut end = 0;
        for (range, kind) in Tokens::new(&self.sql, self.read, self.database_type) {
            if kind == Kind::Comment {
                self.comments.push(range);
                continue;
            }
            if self.sql[range.clone()].trim().is_empty() {
                continue;
            }
            if kind == Kind::Separator {
                if let Some(start) = start.take() {
                    self.statements.push(start..range.end);
                }
                self.read = range.end;
                self.separators.push(range);
                if self.read > offset {
                    return;
                }
            } else {
                start.get_or_insert(range.start);
                end = range.end;
            }
        }
        if let Some(start) = start {
            self.statements.push(start..end);
        }
        self.read = self.sql.len();
    }

    /// The statement at `offset`. See [`Statements`].
    pub(super) fn statement_at(&mut self, offset: usize) -> Range<usize> {
        self.read_past(offset);
        if self.sql.is_char_boundary(offset) {
            // The statement that has the cursor.
            let after = self
                .statements
                .partition_point(|range| range.start <= offset);
            if let Some(range) = after
                .checked_sub(1)
                .map(|index| &self.statements[index])
                .filter(|range| offset < range.end)
            {
                return range.clone();
            }
            let after = self.comments.partition_point(|range| range.start <= offset);
            let in_comment = after
                .checked_sub(1)
                .is_some_and(|index| offset < self.comments[index].end);
            // The statement that ends at the cursor. `range_at` also accepts
            // a statement on the same line, but the request keeps only a
            // statement that has the cursor or ends at it.
            if !in_comment {
                let index = self.statements.partition_point(|range| range.end < offset);
                if let Some(range) = self
                    .statements
                    .get(index)
                    .filter(|range| range.end == offset)
                {
                    return range.clone();
                }
            }
        }
        // The text between the `;` before the cursor and the `;` after it.
        let after = self.separators.partition_point(|range| range.end <= offset);
        let start = after
            .checked_sub(1)
            .map_or(0, |index| self.separators[index].end);
        let end = self
            .separators
            .get(after)
            .map_or(self.sql.len(), |range| range.start);
        start..end
    }
}

/// The length of the longest common start of `a` and `b`, in bytes.
fn common_prefix(a: &[u8], b: &[u8]) -> usize {
    const CHUNK: usize = 4096;
    let length = a.len().min(b.len());
    let mut same = 0;
    while same + CHUNK <= length && a[same..same + CHUNK] == b[same..same + CHUNK] {
        same += CHUNK;
    }
    while same < length && a[same] == b[same] {
        same += 1;
    }
    same
}
