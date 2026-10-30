use super::super::{Kind, tokens_for};
use crate::model::DatabaseType;
use std::ops::Range;

#[derive(Clone, Debug)]
pub(super) struct Name {
    pub text: String,
    pub quoted: bool,
}

#[derive(Clone, Debug)]
pub(super) struct Relation {
    pub path: Vec<Name>,
    pub alias: String,
    pub columns: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub(super) struct Cte {
    pub name: String,
    pub columns: Vec<String>,
}

pub(super) struct Context {
    pub prefix: String,
    pub replacement: Range<usize>,
    pub quoted: bool,
    pub qualifiers: Vec<Name>,
    pub relation_position: bool,
    pub relations: Vec<Relation>,
    pub ctes: Vec<Cte>,
}

struct Token<'a> {
    text: &'a str,
    range: Range<usize>,
    kind: Kind,
    depth: usize,
}

impl Token<'_> {
    fn is(&self, word: &str) -> bool {
        self.kind == Kind::Plain && self.text.eq_ignore_ascii_case(word)
    }

    fn name(&self) -> Option<Name> {
        let quoted = self.kind == Kind::Identifier;
        (quoted
            || self.kind == Kind::Plain
                && self
                    .text
                    .starts_with(|c: char| c.is_alphabetic() || c == '_'))
        .then(|| Name {
            text: unquote(self.text),
            quoted,
        })
    }
}

pub(super) fn unquote(text: &str) -> String {
    let Some(quote @ ('`' | '"')) = text.chars().next() else {
        return text.to_string();
    };
    let content = &text[quote.len_utf8()..];
    content
        .strip_suffix(quote)
        .unwrap_or(content)
        .replace(&format!("{quote}{quote}"), &quote.to_string())
}

pub(super) fn scan(sql: &str, cursor: usize, dialect: DatabaseType) -> Option<Context> {
    if cursor > sql.len() || !sql.is_char_boundary(cursor) {
        return None;
    }
    let lexical = tokens_for(sql, dialect);
    if lexical.iter().any(|(range, kind)| {
        matches!(kind, Kind::String | Kind::Comment) && range.start < cursor && cursor <= range.end
    }) {
        return None;
    }
    let start = lexical
        .iter()
        .filter(|(range, kind)| *kind == Kind::Separator && range.end <= cursor)
        .map(|(range, _)| range.end)
        .next_back()
        .unwrap_or(0);
    let end = lexical
        .iter()
        .find(|(range, kind)| *kind == Kind::Separator && range.start >= cursor)
        .map_or(sql.len(), |(range, _)| range.start);
    let mut depth = 0usize;
    let mut tokens = Vec::new();
    let mut pairs = Vec::new();
    let mut stack = Vec::new();
    for (range, kind) in lexical {
        if range.start < start || range.end > end || kind == Kind::Comment {
            continue;
        }
        let text = &sql[range.clone()];
        if text.trim().is_empty() {
            continue;
        }
        if text == ")" {
            depth = depth.saturating_sub(1);
        }
        let index = tokens.len();
        tokens.push(Token {
            text,
            range,
            kind,
            depth,
        });
        if text == "(" {
            stack.push(index);
            depth += 1;
        }
        if text == ")"
            && let Some(open) = stack.pop()
        {
            pairs.push((open, index));
        }
    }
    for open in stack {
        pairs.push((open, tokens.len()));
    }
    let mut scope = (0, tokens.len(), 0);
    for &(open, close) in &pairs {
        let first = tokens.get(open + 1);
        let scope_end = tokens.get(close).map_or(end, |token| token.range.start);
        if tokens[open].range.end <= cursor
            && cursor <= scope_end
            && first.is_some_and(|token| token.is("SELECT") || token.is("WITH"))
            && tokens[open].depth + 1 > scope.2
        {
            scope = (open + 1, close, tokens[open].depth + 1);
        }
    }
    let query_open = (scope.2 > 0).then(|| scope.0.saturating_sub(1));
    // Each arm of a set operation declares its own relation aliases.
    for (index, token) in tokens.iter().enumerate() {
        if index < scope.0 || index >= scope.1 {
            continue;
        }
        if token.depth == scope.2
            && (token.is("UNION") || token.is("INTERSECT") || token.is("EXCEPT"))
        {
            if token.range.end <= cursor {
                scope.0 = index + 1;
            } else if token.range.start >= cursor {
                scope.1 = scope.1.min(index);
                break;
            }
        }
    }
    let prefix_index = tokens.iter().position(|token| {
        token.range.start <= cursor && cursor <= token.range.end && token.name().is_some()
    });
    let (prefix, replacement, quoted, before) = if let Some(index) = prefix_index {
        let token = &tokens[index];
        (
            unquote(&sql[token.range.start..cursor]),
            token.range.clone(),
            token.kind == Kind::Identifier,
            index,
        )
    } else {
        (
            String::new(),
            cursor..cursor,
            false,
            tokens.partition_point(|token| token.range.end <= cursor),
        )
    };
    let mut qualifiers = Vec::new();
    let mut at = before;
    while at >= 2 && tokens[at - 1].text == "." {
        let Some(name) = tokens[at - 2].name() else {
            break;
        };
        qualifiers.push(name);
        at -= 2;
    }
    qualifiers.reverse();
    let mut ctes = Vec::new();
    for (i, token) in tokens.iter().enumerate() {
        if !token.is("WITH") || token.depth > scope.2 {
            continue;
        }
        let owner = pairs
            .iter()
            .filter(|&&(open, close)| open < i && i < close)
            .max_by_key(|&&(open, _)| tokens[open].depth);
        if owner.is_some_and(|&(open, close)| {
            cursor < tokens[open].range.end
                || cursor > tokens.get(close).map_or(end, |t| t.range.start)
        }) {
            continue;
        }
        let recursive = tokens.get(i + 1).is_some_and(|t| t.is("RECURSIVE"));
        let mut next = i + 1 + usize::from(recursive);
        while let Some(name) = tokens.get(next).and_then(Token::name) {
            next += 1;
            let mut columns = Vec::new();
            if tokens.get(next).is_some_and(|t| t.text == "(") {
                let Some(&(_, close)) = pairs.iter().find(|&&(open, _)| open == next) else {
                    break;
                };
                columns = tokens[next + 1..close.min(tokens.len())]
                    .iter()
                    .filter_map(Token::name)
                    .map(|name| canonical_name(name, dialect))
                    .collect();
                next = close + 1;
            }
            if !tokens.get(next).is_some_and(|t| t.is("AS"))
                || !tokens.get(next + 1).is_some_and(|t| t.text == "(")
            {
                break;
            }
            let open = next + 1;
            let Some(&(_, close)) = pairs.iter().find(|&&(o, _)| o == open) else {
                break;
            };
            let body_end = tokens.get(close).map_or(end, |t| t.range.end);
            let in_body = tokens[open].range.start < cursor && cursor < body_end;
            if columns.is_empty() {
                columns = projected_columns(&tokens, open + 1, close, token.depth + 1, dialect);
            }
            if !in_body || recursive {
                ctes.push(Cte {
                    name: canonical_name(name, dialect),
                    columns,
                });
            }
            if in_body {
                break;
            }
            next = close + 1;
            if tokens.get(next).is_none_or(|t| t.text != ",") {
                break;
            }
            next += 1;
        }
    }
    let mut relations = Vec::new();
    let mut in_from = false;
    let mut relation_position = false;
    let mut index = scope.0;
    while index < scope.1 {
        let token = &tokens[index];
        if token.depth != scope.2 {
            index += 1;
            continue;
        }
        let begins = token.is("FROM") || token.is("JOIN") || token.is("UPDATE") || token.is("INTO");
        let comma = in_from && token.text == ",";
        if token.is("WHERE")
            || token.is("ON")
            || token.is("USING")
            || token.is("GROUP")
            || token.is("ORDER")
            || token.is("HAVING")
            || token.is("LIMIT")
            || token.is("UNION")
        {
            in_from = false;
        }
        if token.range.end <= cursor {
            if begins || comma {
                relation_position = true;
            }
            if !in_from
                && !begins
                && !comma
                && (token.is("WHERE")
                    || token.is("ON")
                    || token.is("USING")
                    || token.is("GROUP")
                    || token.is("ORDER")
                    || token.is("HAVING")
                    || token.is("SELECT"))
            {
                relation_position = false;
            }
        }
        if !begins && !comma {
            index += 1;
            continue;
        }
        in_from = true;
        let mut next = index + 1;
        let mut columns = None;
        let mut path = Vec::new();
        if tokens.get(next).is_some_and(|t| t.text == "(") {
            if let Some(&(_, close)) = pairs.iter().find(|&&(open, _)| open == next) {
                columns = Some(projected_columns(
                    &tokens,
                    next + 1,
                    close,
                    scope.2 + 1,
                    dialect,
                ));
                next = close + 1;
            }
        } else if let Some(name) = tokens.get(next).and_then(Token::name) {
            path.push(name);
            next += 1;
            while tokens.get(next).is_some_and(|t| t.text == ".") {
                let Some(name) = tokens.get(next + 1).and_then(Token::name) else {
                    break;
                };
                path.push(name);
                next += 2;
            }
        }
        if path.is_empty() && columns.is_none() {
            index += 1;
            continue;
        }
        if tokens.get(next).is_some_and(|t| t.is("AS")) {
            next += 1;
        }
        let alias = tokens
            .get(next)
            .filter(|t| !reserved(t.text))
            .and_then(Token::name)
            .map(|name| canonical_name(name, dialect))
            .unwrap_or_else(|| {
                path.last()
                    .map_or(String::new(), |n| canonical_name(n.clone(), dialect))
            });
        if columns.is_none()
            && path.len() == 1
            && let Some(cte) = ctes.iter().rev().find(|cte| {
                if dialect == DatabaseType::Postgres {
                    if path[0].quoted {
                        cte.name == path[0].text
                    } else {
                        cte.name == path[0].text.to_lowercase()
                    }
                } else {
                    cte.name.eq_ignore_ascii_case(&path[0].text)
                }
            })
        {
            columns = Some(cte.columns.clone());
        }
        relations.push(Relation {
            path,
            alias,
            columns,
        });
        index = next.max(index + 1);
    }
    if let Some(open) = query_open {
        // CTE bodies cannot refer to the aliases of the following main query.
        let cte_body = open > 0 && tokens[open - 1].is("AS");
        if !cte_body
            && tokens.get(open).is_some_and(|token| token.text == "(")
            && let Some(parent) = scan(sql, tokens[open].range.start, dialect)
        {
            let mut inherited = parent.relations;
            inherited.retain(|outer| {
                !relations.iter().any(|inner| {
                    if dialect == DatabaseType::Postgres {
                        inner.alias == outer.alias
                    } else {
                        inner.alias.eq_ignore_ascii_case(&outer.alias)
                    }
                })
            });
            inherited.extend(relations);
            relations = inherited;
        }
    }
    Some(Context {
        prefix,
        replacement,
        quoted,
        qualifiers,
        relation_position,
        relations,
        ctes,
    })
}

fn projected_columns(
    tokens: &[Token<'_>],
    start: usize,
    end: usize,
    depth: usize,
    dialect: DatabaseType,
) -> Vec<String> {
    let mut output = Vec::new();
    let mut expression = Vec::new();
    let mut selected = false;
    for token in tokens.get(start..end.min(tokens.len())).unwrap_or_default() {
        if token.depth != depth {
            continue;
        }
        if token.is("SELECT") {
            selected = true;
            continue;
        }
        if !selected {
            continue;
        }
        if token.text == "," || token.is("FROM") {
            if let Some(name) = projection_name(&expression, dialect) {
                output.push(name);
            }
            expression.clear();
            if token.is("FROM") {
                break;
            }
        } else {
            expression.push(token);
        }
    }
    if let Some(name) = projection_name(&expression, dialect) {
        output.push(name);
    }
    output
}

fn projection_name(tokens: &[&Token<'_>], dialect: DatabaseType) -> Option<String> {
    let last = tokens.last()?;
    // A simple field or a named expression has a stable output name. Do not
    // invent names for function calls, literals, or wildcard projections.
    if tokens.len() == 1
        || tokens.iter().any(|t| t.is("AS"))
        || tokens
            .get(tokens.len().saturating_sub(2))
            .is_some_and(|t| t.text == ".")
    {
        last.name().map(|name| canonical_name(name, dialect))
    } else {
        None
    }
}

pub(super) fn reserved(word: &str) -> bool {
    super::KEYWORDS
        .iter()
        .any(|keyword| keyword.eq_ignore_ascii_case(word))
        || [
            "AS",
            "ASC",
            "DESC",
            "OUTER",
            "CROSS",
            "LATERAL",
            "NATURAL",
            "TABLESAMPLE",
            "RECURSIVE",
            "QUALIFY",
            "WINDOW",
            "FETCH",
            "OFFSET",
        ]
        .iter()
        .any(|keyword| keyword.eq_ignore_ascii_case(word))
}

fn canonical_name(name: Name, dialect: DatabaseType) -> String {
    if dialect == DatabaseType::Postgres && !name.quoted {
        name.text.to_lowercase()
    } else {
        name.text
    }
}
