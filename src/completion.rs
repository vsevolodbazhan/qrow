//! Suggestions of schemas, tables, views, and columns for the SQL editor.
//!
//! The suggestions come from the cached metadata of a connection. Nothing
//! here sends SQL. [`request`] reads the SQL around the cursor, and
//! [`complete`] matches the request against a [`Catalog`].

use crate::catalog::{Catalog, CatalogColumn, Relation, RelationKind, Schema};
use crate::model::{CatalogSettings, DatabaseType};
use crate::sql::{Kind, name_part, tokens_for};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;

mod statements;
pub use statements::Statements;

/// The most suggestions that one request gives.
pub const MAX_ITEMS: usize = 200;

/// A relation that a statement names, with its alias.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    pub schema: Option<String>,
    pub name: String,
    pub alias: Option<String>,
}

impl Source {
    /// The name that qualifies the columns of the source in SQL.
    pub fn qualifier(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }
}

/// Where the cursor is in the statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// After FROM, JOIN, and other words that a table name follows.
    Relation,
    /// After `name.`. `relation` tells whether a table name can follow the
    /// qualifier, as in `FROM avia.`.
    Qualified { parts: Vec<String>, relation: bool },
    /// Elsewhere in a statement.
    Column,
}

/// What the user asks for at the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The text that a suggestion replaces: the name at the cursor.
    pub replace: Range<usize>,
    /// The part of the name before the cursor, without quotes.
    pub prefix: String,
    /// The start of the dotted name at the cursor, with its qualifier.
    pub start: usize,
    pub place: Place,
    /// Whether a column name can start at the cursor: after SELECT, WHERE,
    /// AND, a comma, a comparison, and other words that an expression
    /// follows.
    pub column_start: bool,
    /// Whether Tab indents at the cursor instead of showing names: only
    /// blanks precede the cursor on its line, or a name starts at the
    /// cursor with nothing typed before it.
    pub indents: bool,
    /// The relations of the statement at the cursor.
    pub sources: Vec<Source>,
}

/// Words after which a table name follows.
const RELATION_WORDS: [&str; 5] = ["FROM", "JOIN", "INTO", "UPDATE", "TABLE"];

/// Words after which an expression, often a column name, follows.
const COLUMN_WORDS: [&str; 14] = [
    "SELECT", "WHERE", "AND", "OR", "ON", "BY", "HAVING", "SET", "WHEN", "THEN", "ELSE", "NOT",
    "DISTINCT", "QUALIFY",
];

/// Signs after which an expression follows.
const COLUMN_SIGNS: [&str; 9] = [",", "(", "=", "<>", "!=", "<", ">", "<=", ">="];

/// Words that end the table list of a FROM clause.
const CLAUSE_WORDS: [&str; 14] = [
    "WHERE",
    "GROUP",
    "ORDER",
    "HAVING",
    "LIMIT",
    "UNION",
    "EXCEPT",
    "INTERSECT",
    "WINDOW",
    "QUALIFY",
    "SELECT",
    "SET",
    "VALUES",
    "OFFSET",
];

/// Words that cannot be an alias after a table name.
const NOT_ALIAS: [&str; 16] = [
    "ON",
    "USING",
    "JOIN",
    "LEFT",
    "RIGHT",
    "INNER",
    "OUTER",
    "FULL",
    "CROSS",
    "NATURAL",
    "LATERAL",
    "TABLESAMPLE",
    "AS",
    "WITH",
    "FOR",
    "SEMI",
];

/// Reserved words that a name without quotes cannot be.
const RESERVED: [&str; 78] = [
    "ALL",
    "AND",
    "ANY",
    "ARRAY",
    "AS",
    "ASC",
    "BETWEEN",
    "BOTH",
    "BY",
    "CASE",
    "CAST",
    "CHECK",
    "COLLATE",
    "COLUMN",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "CURRENT_DATE",
    "CURRENT_TIME",
    "CURRENT_TIMESTAMP",
    "CURRENT_USER",
    "DEFAULT",
    "DELETE",
    "DESC",
    "DISTINCT",
    "DROP",
    "ELSE",
    "END",
    "EXCEPT",
    "EXISTS",
    "FALSE",
    "FETCH",
    "FOR",
    "FOREIGN",
    "FROM",
    "FULL",
    "GRANT",
    "GROUP",
    "HAVING",
    "IN",
    "INNER",
    "INSERT",
    "INTERSECT",
    "INTERVAL",
    "INTO",
    "IS",
    "JOIN",
    "LATERAL",
    "LEADING",
    "LEFT",
    "LIKE",
    "LIMIT",
    "NATURAL",
    "NOT",
    "NULL",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OUTER",
    "PRIMARY",
    "REFERENCES",
    "RIGHT",
    "SELECT",
    "SESSION_USER",
    "SOME",
    "TABLE",
    "THEN",
    "TO",
    "TRAILING",
    "TRUE",
    "UNION",
    "UNIQUE",
    "USER",
    "USING",
    "WHEN",
    "WHERE",
    "WITH",
];

/// Reserved words of one dialect that [`RESERVED`] does not have.
fn reserved_in(database_type: DatabaseType) -> &'static [&'static str] {
    match database_type {
        DatabaseType::Postgres => &[
            "ANALYSE",
            "ANALYZE",
            "ASYMMETRIC",
            "AUTHORIZATION",
            "BINARY",
            "COLLATION",
            "CONCURRENTLY",
            "CURRENT_CATALOG",
            "CURRENT_ROLE",
            "CURRENT_SCHEMA",
            "DEFERRABLE",
            "DO",
            "FREEZE",
            "ILIKE",
            "INITIALLY",
            "ISNULL",
            "LOCALTIME",
            "LOCALTIMESTAMP",
            "NOTNULL",
            "ONLY",
            "OVERLAPS",
            "PLACING",
            "RETURNING",
            "SIMILAR",
            "SYMMETRIC",
            "SYSTEM_USER",
            "TABLESAMPLE",
            "VARIADIC",
            "VERBOSE",
            "WINDOW",
        ],
        DatabaseType::Trino => &[
            "ALTER",
            "CUBE",
            "CURRENT_CATALOG",
            "CURRENT_PATH",
            "CURRENT_ROLE",
            "CURRENT_SCHEMA",
            "DEALLOCATE",
            "DESCRIBE",
            "ESCAPE",
            "EXECUTE",
            "EXTRACT",
            "GROUPING",
            "JSON_ARRAY",
            "JSON_EXISTS",
            "JSON_OBJECT",
            "JSON_QUERY",
            "JSON_TABLE",
            "JSON_VALUE",
            "LISTAGG",
            "LOCALTIME",
            "LOCALTIMESTAMP",
            "NORMALIZE",
            "PREPARE",
            "RECURSIVE",
            "ROLLUP",
            "SKIP",
            "TRIM",
            "UESCAPE",
            "UNNEST",
            "VALUES",
        ],
        // Spark SQL with ANSI reserved keywords.
        DatabaseType::Kyuubi => &[
            "AUTHORIZATION",
            "ESCAPE",
            "FILTER",
            "ONLY",
            "OVERLAPS",
            "TIME",
            "UNKNOWN",
        ],
    }
}

fn is_word(text: &str, words: &[&str]) -> bool {
    words.iter().any(|word| text.eq_ignore_ascii_case(word))
}

/// A token of the statement, with its range in the whole SQL text.
struct Token<'a> {
    range: Range<usize>,
    kind: Kind,
    text: &'a str,
}

impl Token<'_> {
    fn is_blank(&self) -> bool {
        self.kind == Kind::Comment || (self.kind == Kind::Plain && self.text.trim().is_empty())
    }

    fn is(&self, words: &[&str]) -> bool {
        self.kind == Kind::Plain && is_word(self.text, words)
    }
}

/// The request at byte `offset` of `sql`, or `None` when the cursor is in a
/// comment or a string. To read many requests of an editor text that
/// changes, keep a [`Statements`] instead.
pub fn request(sql: &str, offset: usize, database_type: DatabaseType) -> Option<Request> {
    Statements::new(database_type).request(sql, offset)
}

/// The request at `offset` in `statement`, the statement at the cursor.
fn request_in(
    sql: &str,
    offset: usize,
    database_type: DatabaseType,
    statement: Range<usize>,
) -> Option<Request> {
    let tokens: Vec<Token> = tokens_for(&sql[statement.clone()], database_type)
        .into_iter()
        .map(|(range, kind)| {
            let range = range.start + statement.start..range.end + statement.start;
            Token {
                text: &sql[range.clone()],
                range,
                kind,
            }
        })
        .collect();
    // The token that ends at or contains the cursor.
    let at = tokens
        .iter()
        .position(|token| token.range.start < offset && offset <= token.range.end);
    let mut replace = offset..offset;
    let mut prefix = String::new();
    let mut before = tokens.partition_point(|token| token.range.end <= offset);
    if let Some(index) = at {
        let token = &tokens[index];
        match token.kind {
            Kind::Comment => {
                // A block comment ends before the cursor when it is closed.
                if !(token.text.starts_with("/*")
                    && token.text.ends_with("*/")
                    && offset == token.range.end)
                {
                    return None;
                }
            }
            Kind::String => {
                if offset < token.range.end || !closed(token.text) {
                    return None;
                }
            }
            Kind::Identifier => {
                // Inside the quotes or right after the closing quote, the
                // quoted name is the name at the cursor.
                let quote = token.text.chars().next()?;
                let end = if offset == token.range.end && closed(token.text) {
                    offset - quote.len_utf8()
                } else {
                    offset
                };
                let typed = &sql[token.range.start + quote.len_utf8()..end];
                prefix = typed.replace(&format!("{quote}{quote}"), &quote.to_string());
                replace = token.range.clone();
                before = index;
            }
            Kind::Plain if name_part(token.text, token.kind, database_type).is_some() => {
                prefix = sql[token.range.start..offset].to_owned();
                replace = token.range.clone();
                before = index;
            }
            _ => {}
        }
    }
    // The qualifier: `a.` or `a.b.` before the name.
    let mut parts = Vec::new();
    let mut index = before;
    while index >= 2
        && tokens[index - 1].kind == Kind::Plain
        && tokens[index - 1].text == "."
        && let Some(part) = name_part(
            tokens[index - 2].text,
            tokens[index - 2].kind,
            database_type,
        )
    {
        parts.insert(0, part);
        index -= 2;
    }
    let start = if parts.is_empty() {
        replace.start
    } else {
        tokens[index].range.start
    };
    let previous = tokens[..index].iter().rev().find(|token| !token.is_blank());
    let in_from = clause_is_from(&tokens[..index]);
    let relation = previous.is_some_and(|token| {
        token.is(&RELATION_WORDS) || (token.kind == Kind::Plain && token.text == "," && in_from)
    });
    let column_start = parts.is_empty()
        && !relation
        && previous.is_some_and(|token| {
            token.is(&COLUMN_WORDS)
                || (token.kind == Kind::Plain && COLUMN_SIGNS.contains(&token.text))
        });
    let line = &sql[sql[..offset].rfind('\n').map_or(0, |at| at + 1)..offset];
    let name_follows = replace.is_empty()
        && tokens.iter().any(|token| {
            token.range.start == offset
                && name_part(token.text, token.kind, database_type).is_some()
        });
    let indents = line.trim().is_empty() || name_follows;
    let place = if !parts.is_empty() {
        Place::Qualified { parts, relation }
    } else if relation {
        Place::Relation
    } else {
        Place::Column
    };
    Some(Request {
        replace,
        prefix,
        start,
        place,
        column_start,
        indents,
        sources: sources(&tokens, database_type),
    })
}

/// Whether a quoted token has its closing quote.
fn closed(text: &str) -> bool {
    let Some(quote) = text.chars().next() else {
        return false;
    };
    let inner = &text[quote.len_utf8()..];
    // Pairs of quotes inside the token are escaped quotes.
    let trailing = inner.chars().rev().take_while(|c| *c == quote).count();
    text.len() > quote.len_utf8() && trailing % 2 == 1
}

/// Whether the last clause before the end of `tokens` is a FROM clause, at
/// the depth of parentheses of the end.
fn clause_is_from(tokens: &[Token]) -> bool {
    let mut depth = 0usize;
    for token in tokens.iter().rev() {
        if token.kind != Kind::Plain {
            continue;
        }
        match token.text {
            ")" => depth += 1,
            "(" if depth == 0 => return false,
            "(" => depth -= 1,
            _ if depth > 0 => {}
            text if is_word(text, &["FROM", "JOIN"]) => return true,
            text if is_word(text, &CLAUSE_WORDS) || is_word(text, &["ON"]) => return false,
            _ => {}
        }
    }
    false
}

/// The relations that the statement names after FROM and JOIN, with their
/// aliases. Subqueries are skipped.
fn sources(tokens: &[Token], database_type: DatabaseType) -> Vec<Source> {
    let significant: Vec<&Token> = tokens.iter().filter(|token| !token.is_blank()).collect();
    let mut found = Vec::new();
    let mut in_from = false;
    let mut index = 0;
    while index < significant.len() {
        let token = significant[index];
        let starts = token.is(&["FROM", "JOIN"])
            || (in_from && token.kind == Kind::Plain && token.text == ",");
        if token.is(&["FROM", "JOIN"]) {
            in_from = true;
        } else if token.is(&CLAUSE_WORDS) || token.text == ")" {
            in_from = false;
        }
        index += 1;
        if !starts {
            continue;
        }
        // A dotted name.
        let mut parts = Vec::new();
        while let Some(part) = significant
            .get(index)
            .and_then(|token| name_part(token.text, token.kind, database_type))
            .filter(|_| parts.is_empty() || significant[index - 1].text == ".")
        {
            if significant[index].kind == Kind::Plain && is_word(significant[index].text, &RESERVED)
            {
                break;
            }
            parts.push(part);
            index += 1;
            if significant
                .get(index)
                .is_some_and(|token| token.text == ".")
            {
                index += 1;
            } else {
                break;
            }
        }
        let Some(name) = parts.pop() else {
            continue;
        };
        let schema = parts.pop();
        if significant
            .get(index)
            .is_some_and(|token| token.is(&["AS"]))
        {
            index += 1;
        }
        let alias = significant
            .get(index)
            .filter(|token| {
                !(token.kind == Kind::Plain
                    && (is_word(token.text, &NOT_ALIAS)
                        || is_word(token.text, &CLAUSE_WORDS)
                        || is_word(token.text, &RESERVED)))
            })
            .and_then(|token| name_part(token.text, token.kind, database_type));
        if alias.is_some() {
            index += 1;
        }
        found.push(Source {
            schema,
            name,
            alias,
        });
    }
    found
}

/// The kind of a suggestion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Schema,
    Table,
    View,
    Column,
}

/// One suggestion.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: ItemKind,
    /// The name, as the list shows it.
    pub name: String,
    /// The alias or table before the name, for a column whose name more than
    /// one relation of the statement has. The list shows it before the name.
    pub qualifier: Option<String>,
    /// The text that replaces [`Request::replace`].
    pub insert: String,
    /// The byte offsets in `name` of the characters that match the prefix.
    pub matches: Vec<usize>,
    /// The relation of a column, for a statement with more than one relation.
    pub source: Option<String>,
    /// The kind of a schema or relation, or the type of a column.
    pub detail: String,
    /// The schema and the relation of a relation or a column.
    pub relation: Option<(String, String)>,
    pub comment: Option<String>,
    /// A schema that Qrow read and that has no tables or views.
    pub empty: bool,
}

/// Metadata that a request needs.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Need {
    Schemas,
    Relations(String),
    Columns(String, String),
}

/// The suggestions for a request, and what is missing or old.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Completion {
    pub items: Vec<Item>,
    /// Metadata that Qrow has not read.
    pub missing: Vec<Need>,
    /// The last error of each missing read that failed.
    pub errors: Vec<(Need, String)>,
    /// The schemas of the suggestions whose metadata is older than the
    /// refresh period.
    pub stale: Vec<String>,
    /// The schema after `schema.` when Qrow read it and it has no tables or
    /// views. The list shows the schema in place of names.
    pub empty_schema: Option<String>,
}

/// The suggestions for `request` from `catalog`. `default_schema` is the
/// schema of names without a schema.
pub fn complete(
    catalog: Option<&Catalog>,
    request: &Request,
    default_schema: &str,
    settings: &CatalogSettings,
    now: u64,
    database_type: DatabaseType,
) -> Completion {
    let mut completion = Completion::default();
    let Some(catalog) = catalog.filter(|catalog| catalog.fetched_at.is_some()) else {
        completion.need(Need::Schemas, catalog.and_then(|c| c.error.clone()));
        return completion;
    };
    let mut builder = Builder {
        catalog,
        request,
        settings,
        now,
        database_type,
        wanted: request.prefix.chars().map(lower).collect(),
        candidates: Vec::new(),
        bound: None,
        found: 0,
        completion,
    };
    match &request.place {
        Place::Relation => {
            for (name, node) in &catalog.schemas {
                let empty = node
                    .relations
                    .as_ref()
                    .is_some_and(|relations| relations.is_empty());
                builder.push(0, 0, Candidate::Schema(name, empty));
            }
            builder.relations(default_schema);
        }
        Place::Qualified { parts, relation } => builder.qualified(parts, *relation, default_schema),
        Place::Column => builder.statement_columns(default_schema),
    }
    builder.finish()
}

impl Completion {
    fn need(&mut self, need: Need, error: Option<String>) {
        if self.missing.contains(&need) {
            return;
        }
        if let Some(error) = error {
            self.errors.push((need.clone(), error));
        }
        self.missing.push(need);
    }
}

/// The rank of a match: the match tier, the group, the order in the group,
/// and then the order in which the builder found it. The last part keeps
/// the order of the relations of the statement for columns of the same
/// rank. No two matches have the same rank.
type Rank = (u8, u8, usize, usize);

/// A name that can become a suggestion, with the metadata that the
/// suggestion needs. The builder makes [`Item`]s only for the best matches.
enum Candidate<'a> {
    /// A schema, and whether Qrow read it without tables or views.
    Schema(&'a str, bool),
    Relation {
        schema: &'a str,
        name: &'a str,
        relation: &'a Relation,
    },
    Column {
        schema: &'a str,
        relation: &'a str,
        column: &'a CatalogColumn,
        qualify: Option<Qualify<'a>>,
    },
}

impl Candidate<'_> {
    /// The name that the prefix matches.
    fn name(&self) -> &str {
        match self {
            Self::Schema(name, _) | Self::Relation { name, .. } => name,
            Self::Column { column, .. } => &column.name,
        }
    }

    fn item(&self, prefix: &str, database_type: DatabaseType) -> Item {
        let name = self.name();
        let matches = fuzzy(name, prefix)
            .map(|(_, matches)| matches)
            .unwrap_or_default();
        match *self {
            Self::Schema(name, empty) => Item {
                kind: ItemKind::Schema,
                name: name.to_owned(),
                qualifier: None,
                insert: sql_name(name, database_type),
                matches,
                source: None,
                detail: if empty { "empty schema" } else { "schema" }.into(),
                relation: None,
                comment: None,
                empty,
            },
            Self::Relation {
                schema,
                name,
                relation,
            } => {
                let (kind, detail) = match relation.kind {
                    RelationKind::Table => (ItemKind::Table, "table"),
                    RelationKind::View => (ItemKind::View, "view"),
                };
                Item {
                    kind,
                    name: name.to_owned(),
                    qualifier: None,
                    insert: sql_name(name, database_type),
                    matches,
                    source: None,
                    detail: detail.into(),
                    relation: Some((schema.to_owned(), name.to_owned())),
                    comment: relation.comment.clone(),
                    empty: false,
                }
            }
            Self::Column {
                schema,
                relation,
                column,
                qualify,
            } => {
                let (qualifier, source, insert) = match qualify {
                    Some(Qualify::Prefix(qualifier)) => {
                        let insert = format!(
                            "{}.{}",
                            sql_name(qualifier, database_type),
                            sql_name(&column.name, database_type)
                        );
                        (Some(qualifier.to_owned()), None, insert)
                    }
                    Some(Qualify::Tag(tag)) => (
                        None,
                        Some(tag.to_owned()),
                        sql_name(&column.name, database_type),
                    ),
                    None => (None, None, sql_name(&column.name, database_type)),
                };
                Item {
                    kind: ItemKind::Column,
                    name: column.name.clone(),
                    qualifier,
                    insert,
                    matches,
                    source,
                    detail: column.data_type.to_lowercase(),
                    relation: Some((schema.to_owned(), relation.to_owned())),
                    comment: column.comment.clone(),
                    empty: false,
                }
            }
        }
    }
}

struct Builder<'a> {
    catalog: &'a Catalog,
    request: &'a Request,
    settings: &'a CatalogSettings,
    now: u64,
    database_type: DatabaseType,
    /// The characters of the prefix, as [`fuzzy`] compares them.
    wanted: Vec<char>,
    /// The best matches so far, with their ranks. [`Builder::trim`] keeps
    /// the list short.
    candidates: Vec<(Rank, Candidate<'a>)>,
    /// The rank of the last kept match after the last trim. A match with a
    /// higher rank cannot be in the list.
    bound: Option<Rank>,
    /// The names that the builder looked at.
    found: usize,
    completion: Completion,
}

impl<'a> Builder<'a> {
    fn schema(&self, name: &str) -> Option<(&'a String, &'a Schema)> {
        self.catalog
            .schemas
            .get_key_value(name)
            .or_else(|| {
                self.catalog
                    .schemas
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
            })
            .map(|(key, schema)| (key, schema.as_ref()))
    }

    fn relation(
        &mut self,
        schema: &str,
        name: &str,
    ) -> Option<(&'a String, &'a String, &'a Relation)> {
        let (schema_name, node) = self.schema(schema)?;
        let Some(relations) = &node.relations else {
            self.completion
                .need(Need::Relations(schema_name.clone()), node.error.clone());
            return None;
        };
        relations
            .get_key_value(name)
            .or_else(|| {
                relations
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
            })
            .map(|(key, relation)| (schema_name, key, relation.as_ref()))
    }

    fn mark_stale(&mut self, schema: &str, fetched_at: Option<u64>) {
        if crate::assistant::catalog::stale(fetched_at, self.settings, self.now)
            && !self.completion.stale.iter().any(|known| known == schema)
        {
            self.completion.stale.push(schema.to_owned());
        }
    }

    /// Keeps `candidate` when its name matches the prefix and its rank can
    /// be in the list.
    fn push(&mut self, group: u8, order: usize, candidate: Candidate<'a>) {
        let found = self.found;
        self.found += 1;
        // The best tier is 0, so a match cannot rank before this.
        if self
            .bound
            .is_some_and(|bound| (0, group, order, found) > bound)
        {
            return;
        }
        let Some(tier) = tier(candidate.name(), &self.wanted) else {
            return;
        };
        let rank = (tier, group, order, found);
        if self.bound.is_some_and(|bound| rank > bound) {
            return;
        }
        self.candidates.push((rank, candidate));
        if self.candidates.len() >= 2 * MAX_ITEMS {
            self.trim();
        }
    }

    /// Keeps only the `MAX_ITEMS` best matches.
    fn trim(&mut self) {
        self.candidates
            .select_nth_unstable_by_key(MAX_ITEMS - 1, |(rank, _)| *rank);
        self.candidates.truncate(MAX_ITEMS);
        self.bound = self.candidates.iter().map(|(rank, _)| *rank).max();
    }

    /// The tables and views of `schema`.
    fn relations(&mut self, schema: &str) {
        let Some((schema_name, node)) = self.schema(schema) else {
            return;
        };
        let Some(relations) = &node.relations else {
            self.completion
                .need(Need::Relations(schema_name.clone()), node.error.clone());
            return;
        };
        self.mark_stale(schema_name, node.fetched_at);
        for (order, (name, relation)) in relations.iter().enumerate() {
            self.push(
                1,
                order,
                Candidate::Relation {
                    schema: schema_name,
                    name,
                    relation,
                },
            );
        }
    }

    fn qualified(&mut self, parts: &[String], relation_place: bool, default_schema: &str) {
        let request = self.request;
        let last = parts.last().map(String::as_str).unwrap_or_default();
        if !relation_place && parts.len() == 1 {
            let source = request
                .sources
                .iter()
                .find(|source| source.qualifier().eq_ignore_ascii_case(last));
            if let Some(source) = source {
                let schema = source.schema.as_deref().unwrap_or(default_schema);
                self.columns(schema, &source.name, None);
                return;
            }
        }
        if parts.len() >= 2 {
            let schema = &parts[parts.len() - 2];
            if self.schema(schema).is_some()
                && let Some((schema, name, _)) = self.relation(schema, last)
            {
                self.columns(schema, name, None);
                return;
            }
        }
        if let Some((schema_name, node)) = self.schema(last) {
            if node
                .relations
                .as_ref()
                .is_some_and(|relations| relations.is_empty())
            {
                self.completion.empty_schema = Some(schema_name.clone());
            }
            self.relations(last);
        } else if !relation_place
            && parts.len() == 1
            && self.relation(default_schema, last).is_some()
        {
            self.columns(default_schema, last, None);
        }
    }

    /// The columns of the relations of the statement.
    fn statement_columns(&mut self, default_schema: &str) {
        let request = self.request;
        let mut resolved: Vec<(&'a Source, &'a [CatalogColumn])> = Vec::new();
        let mut seen: Vec<(&'a String, &'a String, &'a str)> = Vec::new();
        for source in &request.sources {
            let schema = source.schema.as_deref().unwrap_or(default_schema);
            let Some((schema_name, name, relation)) = self.relation(schema, &source.name) else {
                continue;
            };
            // A subquery can name the same relation with the same
            // qualifier. Its columns show once.
            let key = (schema_name, name, source.qualifier());
            if seen.iter().any(|known| {
                known.0 == key.0 && known.1 == key.1 && known.2.eq_ignore_ascii_case(key.2)
            }) {
                continue;
            }
            seen.push(key);
            resolved.push((source, relation.columns.as_deref().unwrap_or_default()));
        }
        // For each column name, without regard to ASCII letter case: the
        // first source that has it, and whether another source has it too.
        // A source has each of its own columns, so a column is shared when
        // more than one source has it.
        let mut owners: HashMap<NoCase<'a>, (usize, bool)> = HashMap::new();
        if resolved.len() > 1 {
            for (index, (_, columns)) in resolved.iter().enumerate() {
                for column in *columns {
                    owners
                        .entry(NoCase(&column.name))
                        .and_modify(|(first, shared)| *shared |= *first != index)
                        .or_insert((index, false));
                }
            }
        }
        let shared = |column: &str| {
            owners
                .get(&NoCase(column))
                .is_some_and(|(_, shared)| *shared)
        };
        let several = resolved.len() > 1;
        for (source, _) in &resolved {
            let schema = source.schema.as_deref().unwrap_or(default_schema);
            let qualifier = source.qualifier();
            self.columns_with(schema, &source.name, |column| {
                if shared(column) {
                    Some(Qualify::Prefix(qualifier))
                } else if several {
                    Some(Qualify::Tag(qualifier))
                } else {
                    None
                }
            });
        }
    }

    fn columns(&mut self, schema: &str, relation: &str, qualifier: Option<Qualify<'a>>) {
        self.columns_with(schema, relation, |_| qualifier);
    }

    fn columns_with(
        &mut self,
        schema: &str,
        relation: &str,
        qualify: impl Fn(&str) -> Option<Qualify<'a>>,
    ) {
        let Some((schema_name, name, node)) = self.relation(schema, relation) else {
            return;
        };
        let Some(columns) = &node.columns else {
            self.completion.need(
                Need::Columns(schema_name.clone(), name.clone()),
                node.error.clone(),
            );
            return;
        };
        let schema_fetched = self
            .schema(schema_name)
            .and_then(|(_, node)| node.fetched_at);
        self.mark_stale(schema_name, schema_fetched.min(node.fetched_at));
        for (order, column) in columns.iter().enumerate() {
            self.push(
                2,
                order,
                Candidate::Column {
                    schema: schema_name,
                    relation: name,
                    column,
                    qualify: qualify(&column.name),
                },
            );
        }
    }

    fn finish(mut self) -> Completion {
        self.candidates.sort_unstable_by_key(|(rank, _)| *rank);
        self.candidates.truncate(MAX_ITEMS);
        // "Empty" can be old: a stale empty schema in the list gets the
        // stale footer. Only the kept schemas are checked. They come first,
        // in catalog order, as schemas are found before names.
        let mut empty: Vec<(usize, &'a str)> = self
            .candidates
            .iter()
            .filter_map(|(rank, candidate)| match *candidate {
                Candidate::Schema(name, true) => Some((rank.3, name)),
                _ => None,
            })
            .collect();
        if !empty.is_empty() {
            empty.sort_unstable();
            let later = std::mem::take(&mut self.completion.stale);
            let catalog = self.catalog;
            for (_, name) in empty {
                if let Some(node) = catalog.schemas.get(name) {
                    self.mark_stale(name, node.fetched_at);
                }
            }
            for schema in later {
                if !self.completion.stale.contains(&schema) {
                    self.completion.stale.push(schema);
                }
            }
        }
        let prefix = &self.request.prefix;
        let database_type = self.database_type;
        self.completion.items = self
            .candidates
            .iter()
            .map(|(_, candidate)| candidate.item(prefix, database_type))
            .collect();
        // A stale schema matters only when the list shows names from it,
        // or shows that it is empty.
        let items = &self.completion.items;
        let empty_schema = self.completion.empty_schema.as_deref();
        self.completion.stale.retain(|schema| {
            empty_schema == Some(schema.as_str())
                || items.iter().any(|item| {
                    item.relation.as_ref().is_some_and(|(of, _)| of == schema)
                        || (item.empty && item.name == *schema)
                })
        });
        self.completion
    }
}

#[derive(Clone, Copy)]
enum Qualify<'a> {
    /// Insert and show the column as `qualifier.column`.
    Prefix(&'a str),
    /// Show the source as a tag.
    Tag(&'a str),
}

/// A name that equals another name without regard to ASCII letter case, as
/// [`str::eq_ignore_ascii_case`] compares them.
struct NoCase<'a>(&'a str);

impl PartialEq for NoCase<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(other.0)
    }
}

impl Eq for NoCase<'_> {}

impl Hash for NoCase<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut buffer = [0u8; 32];
        for chunk in self.0.as_bytes().chunks(buffer.len()) {
            for (to, from) in buffer.iter_mut().zip(chunk) {
                *to = from.to_ascii_lowercase();
            }
            state.write(&buffer[..chunk.len()]);
        }
        state.write_u8(0xff);
    }
}

/// A character as [`fuzzy`] compares it: the first character of its
/// lowercase form.
fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// The tier that [`fuzzy`] gives `name` for the characters `wanted` of a
/// prefix, without the offsets of the matched characters. It does not
/// allocate for names of up to 64 characters.
fn tier(name: &str, wanted: &[char]) -> Option<u8> {
    if wanted.is_empty() {
        return Some(0);
    }
    if name.is_ascii() {
        let bytes = name.as_bytes();
        return tier_of(
            bytes.len(),
            |index| bytes[index].to_ascii_lowercase() as char,
            wanted,
        );
    }
    let mut chars = ['\0'; 64];
    let mut count = 0;
    for c in name.chars() {
        let Some(slot) = chars.get_mut(count) else {
            let long: Vec<char> = name.chars().map(lower).collect();
            return tier_of(long.len(), |index| long[index], wanted);
        };
        *slot = lower(c);
        count += 1;
    }
    tier_of(count, |index| chars[index], wanted)
}

/// [`tier`] of the `len` characters that `at` gives.
fn tier_of(len: usize, at: impl Fn(usize) -> char, wanted: &[char]) -> Option<u8> {
    let starts_at = |start: usize| {
        wanted.len() <= len - start
            && wanted
                .iter()
                .enumerate()
                .all(|(index, want)| at(start + index) == *want)
    };
    if starts_at(0) {
        return Some(0);
    }
    if (1..len).any(starts_at) {
        return Some(1);
    }
    if len == 0 || at(0) != wanted[0] {
        return None;
    }
    let mut next = 0;
    for index in 0..len {
        if next < wanted.len() && at(index) == wanted[next] {
            next += 1;
        }
    }
    (next == wanted.len()).then_some(2)
}

/// How `name` matches `prefix`, without regard to letter case: 0 for a
/// prefix, 1 for a part of the name, 2 for characters in order. Returns the
/// byte offsets of the matched characters in `name`.
pub fn fuzzy(name: &str, prefix: &str) -> Option<(u8, Vec<usize>)> {
    if prefix.is_empty() {
        return Some((0, Vec::new()));
    }
    let chars: Vec<(usize, char)> = name.char_indices().map(|(i, c)| (i, lower(c))).collect();
    let wanted: Vec<char> = prefix.chars().map(lower).collect();
    let contiguous = |start: usize| {
        wanted.len() <= chars.len() - start
            && wanted
                .iter()
                .zip(&chars[start..])
                .all(|(want, (_, c))| want == c)
    };
    if contiguous(0) {
        return Some((0, chars[..wanted.len()].iter().map(|(i, _)| *i).collect()));
    }
    if let Some(start) = (1..chars.len()).find(|start| contiguous(*start)) {
        return Some((
            1,
            chars[start..start + wanted.len()]
                .iter()
                .map(|(i, _)| *i)
                .collect(),
        ));
    }
    // Characters in order. The first character must start the name, so a
    // match stays close to what the user typed.
    if chars.first().map(|(_, c)| *c) != wanted.first().copied() {
        return None;
    }
    let mut matches = Vec::with_capacity(wanted.len());
    let mut want = wanted.iter().peekable();
    for (i, c) in &chars {
        if want.peek() == Some(&c) {
            matches.push(*i);
            want.next();
        }
    }
    want.peek().is_none().then_some((2, matches))
}

/// `name` for SQL: without quotes when it can be, else quoted in the dialect.
pub fn sql_name(name: &str, database_type: DatabaseType) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !is_word(name, &RESERVED)
        && !is_word(name, reserved_in(database_type))
        // Postgres and Trino fold names without quotes to lowercase.
        && (database_type == DatabaseType::Kyuubi || !name.chars().any(|c| c.is_ascii_uppercase()));
    if plain {
        name.to_owned()
    } else {
        database_type.quote_identifier(name)
    }
}

#[cfg(test)]
mod tests;
