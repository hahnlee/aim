//! The overlay manifest: a strict subset of TOML.
//!
//! ```toml
//! schema = 1
//!
//! [[add]]                                   # a file the original lacks
//! path = "/vendor/etc/vintf/manifest.xml"   # absolute guest path
//! source = "_build/vendor/manifest.xml"     # relative to the source root
//! reason = "..."                            # optional
//!
//! [[replace]]                               # a file the original has
//! path = "/system/etc/foo.conf"
//! source = "image/files/foo.conf"
//! reason = "..."                            # required
//!
//! [[remove]]                                # a file or directory
//! path = "/system/app/Foo"
//! reason = "..."                            # required
//!
//! [[include]]                               # entries a build writes
//! source = "target/aim/oat/overlay.toml"    # a manifest without includes
//! reason = "..."                            # required
//! ```
//!
//! Accepted syntax: blank lines, `#` comments, the top-level `schema`
//! integer, `[[add]]`/`[[replace]]`/`[[remove]]`/`[[include]]` tables, and
//! `key = "string"`
//! (basic or literal, single-line) inside them. Anything else is an error, so
//! the file never means something the tool silently ignores.

use crate::problem::{Problem, ProblemKind};
use std::collections::BTreeMap;
use std::fmt;

/// The only schema this tool understands.
pub const SCHEMA: i64 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Add,
    Replace,
    Remove,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Add => "add",
            Kind::Replace => "replace",
            Kind::Remove => "remove",
        }
    }

    fn from_table(name: &str) -> Option<Kind> {
        match name {
            "add" => Some(Kind::Add),
            "replace" => Some(Kind::Replace),
            "remove" => Some(Kind::Remove),
            _ => None,
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// One declared deviation from the original image, as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    /// Absolute guest path, e.g. `/vendor/lib64/libfoo.so`.
    pub path: String,
    /// Source file, relative to the source root (`add` and `replace`).
    pub source: Option<String>,
    pub reason: Option<String>,
    /// Line of the table header, for messages.
    pub line: usize,
}

/// Another manifest whose entries count as this one's (`include`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Include {
    /// The manifest, relative to the source root.
    pub source: String,
    pub reason: String,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub entries: Vec<Entry>,
    pub includes: Vec<Include>,
}

#[derive(Debug)]
enum Value {
    Str(String),
    Int(i64),
}

struct Table {
    /// `None` for `[[include]]`.
    kind: Option<Kind>,
    line: usize,
    keys: BTreeMap<String, (Value, usize)>,
}

/// Parses the manifest text. Every problem found is reported, not just the
/// first.
pub fn parse(text: &str) -> Result<Manifest, Vec<Problem>> {
    let mut problems = Vec::new();
    let mut schema: Option<(Value, usize)> = None;
    let mut tables: Vec<Table> = Vec::new();

    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("[[") {
            let Some(end) = rest.find("]]") else {
                problems.push(syntax(line, "unterminated table header"));
                continue;
            };
            if !is_blank_or_comment(&rest[end + 2..]) {
                problems.push(syntax(line, "unexpected text after table header"));
                continue;
            }
            let name = rest[..end].trim();
            match Kind::from_table(name) {
                Some(kind) => tables.push(Table {
                    kind: Some(kind),
                    line,
                    keys: BTreeMap::new(),
                }),
                None if name == "include" => tables.push(Table {
                    kind: None,
                    line,
                    keys: BTreeMap::new(),
                }),
                None => problems.push(syntax(
                    line,
                    format!(
                        "unknown table [[{name}]]; expected [[add]], [[replace]], [[remove]] or [[include]]"
                    ),
                )),
            }
            continue;
        }
        if trimmed.starts_with('[') {
            problems.push(syntax(
                line,
                "only [[add]], [[replace]], [[remove]] and [[include]] tables are allowed",
            ));
            continue;
        }
        let Some(eq) = trimmed.find('=') else {
            problems.push(syntax(line, "expected `key = value`"));
            continue;
        };
        let key = trimmed[..eq].trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            problems.push(syntax(line, format!("invalid key `{key}`")));
            continue;
        }
        let value = match parse_value(trimmed[eq + 1..].trim_start()) {
            Ok(value) => value,
            Err(message) => {
                problems.push(syntax(line, message));
                continue;
            }
        };
        match tables.last_mut() {
            None => {
                if key != "schema" {
                    problems.push(Problem::at(
                        ProblemKind::UnexpectedField,
                        line,
                        format!("unknown top-level key `{key}`; only `schema` is allowed"),
                    ));
                } else if schema.is_some() {
                    problems.push(Problem::at(
                        ProblemKind::Syntax,
                        line,
                        "duplicate key `schema`",
                    ));
                } else {
                    schema = Some((value, line));
                }
            }
            Some(table) => {
                if table.keys.contains_key(key) {
                    problems.push(Problem::at(
                        ProblemKind::Syntax,
                        line,
                        format!("duplicate key `{key}`"),
                    ));
                } else {
                    table.keys.insert(key.to_string(), (value, line));
                }
            }
        }
    }

    match schema {
        None => problems.push(Problem::new(
            ProblemKind::Schema,
            format!("missing `schema = {SCHEMA}` before the first table"),
        )),
        Some((Value::Int(SCHEMA), _)) => {}
        Some((_, line)) => problems.push(Problem::at(
            ProblemKind::Schema,
            line,
            format!("unsupported schema; this tool reads `schema = {SCHEMA}`"),
        )),
    }

    let (mut entries, mut includes) = (Vec::new(), Vec::new());
    for table in tables {
        if table.kind.is_none() {
            if let Some(include) = include_from_table(table, &mut problems) {
                includes.push(include);
            }
        } else if let Some(entry) = entry_from_table(table, &mut problems) {
            entries.push(entry);
        }
    }

    if problems.is_empty() {
        Ok(Manifest { entries, includes })
    } else {
        Err(problems)
    }
}

/// Takes the string key `name` out of `table`.
fn take(table: &mut Table, name: &str, problems: &mut Vec<Problem>) -> Option<String> {
    match table.keys.remove(name) {
        None => None,
        Some((Value::Str(text), _)) => Some(text),
        Some((Value::Int(_), key_line)) => {
            problems.push(Problem::at(
                ProblemKind::Syntax,
                key_line,
                format!("`{name}` must be a string"),
            ));
            None
        }
    }
}

/// Reports the keys left in `table` after its known ones were taken.
fn unknown_keys(table: &Table, name: &str, problems: &mut Vec<Problem>) {
    for (key, (_, key_line)) in &table.keys {
        problems.push(Problem::at(
            ProblemKind::UnexpectedField,
            *key_line,
            format!("unknown key `{key}` in [[{name}]]"),
        ));
    }
}

fn include_from_table(mut table: Table, problems: &mut Vec<Problem>) -> Option<Include> {
    let line = table.line;
    let source = take(&mut table, "source", problems);
    let reason = take(&mut table, "reason", problems);
    unknown_keys(&table, "include", problems);
    if source.is_none() {
        problems.push(Problem::at(
            ProblemKind::MissingField,
            line,
            "[[include]] needs `source`",
        ));
    }
    match &reason {
        Some(text) if !text.trim().is_empty() => {}
        _ => problems.push(Problem::at(
            ProblemKind::MissingReason,
            line,
            "[[include]] needs a `reason`",
        )),
    }
    Some(Include {
        source: source?,
        reason: reason.filter(|r| !r.trim().is_empty())?.trim().to_string(),
        line,
    })
}

fn entry_from_table(mut table: Table, problems: &mut Vec<Problem>) -> Option<Entry> {
    let kind = table.kind.expect("an entry table");
    let line = table.line;
    let before = problems.len();
    let path = take(&mut table, "path", problems);
    let source = take(&mut table, "source", problems);
    let reason = take(&mut table, "reason", problems);
    unknown_keys(&table, kind.as_str(), problems);
    if path.is_none() {
        problems.push(Problem::at(
            ProblemKind::MissingField,
            line,
            format!("[[{kind}]] needs `path`"),
        ));
    }
    match (kind, &source) {
        (Kind::Add | Kind::Replace, None) => problems.push(Problem::at(
            ProblemKind::MissingField,
            line,
            format!("[[{kind}]] needs `source`"),
        )),
        (Kind::Remove, Some(_)) => problems.push(Problem::at(
            ProblemKind::UnexpectedField,
            line,
            "[[remove]] takes no `source`",
        )),
        _ => {}
    }
    match (&reason, kind) {
        (None, Kind::Replace | Kind::Remove) => problems.push(Problem::at(
            ProblemKind::MissingReason,
            line,
            format!("[[{kind}]] needs a `reason`: deviations from the original must be justified"),
        )),
        (Some(text), _) if text.trim().is_empty() => problems.push(Problem::at(
            ProblemKind::MissingReason,
            line,
            "`reason` is empty",
        )),
        _ => {}
    }

    if problems.len() != before {
        return None;
    }
    Some(Entry {
        kind,
        path: path?,
        source,
        reason: reason.map(|text| text.trim().to_string()),
        line,
    })
}

fn syntax(line: usize, message: impl Into<String>) -> Problem {
    Problem::at(ProblemKind::Syntax, line, message)
}

fn is_blank_or_comment(rest: &str) -> bool {
    let rest = rest.trim();
    rest.is_empty() || rest.starts_with('#')
}

fn parse_value(text: &str) -> Result<Value, String> {
    let (value, rest) = if text.starts_with("\"\"\"") || text.starts_with("'''") {
        return Err("multi-line strings are not supported".into());
    } else if let Some(body) = text.strip_prefix('"') {
        parse_basic_string(body)?
    } else if let Some(body) = text.strip_prefix('\'') {
        let end = body
            .find('\'')
            .ok_or_else(|| "unterminated string".to_string())?;
        let literal = &body[..end];
        if literal.chars().any(|c| c.is_control() && c != '\t') {
            return Err("control character in string".into());
        }
        (literal.to_string(), &body[end + 1..])
    } else {
        let end = text
            .find(|c: char| c.is_whitespace() || c == '#')
            .unwrap_or(text.len());
        let digits = &text[..end];
        let number = digits
            .parse::<i64>()
            .map_err(|_| format!("expected a string or an integer, found `{digits}`"))?;
        return if is_blank_or_comment(&text[end..]) {
            Ok(Value::Int(number))
        } else {
            Err("unexpected text after value".into())
        };
    };
    if !is_blank_or_comment(rest) {
        return Err("unexpected text after value".into());
    }
    Ok(Value::Str(value))
}

/// Parses the body of a basic string after its opening quote; returns the
/// value and the text after the closing quote.
fn parse_basic_string(body: &str) -> Result<(String, &str), String> {
    let mut out = String::new();
    let mut chars = body.char_indices();
    while let Some((index, c)) = chars.next() {
        match c {
            '"' => return Ok((out, &body[index + 1..])),
            '\\' => {
                let (_, escape) = chars.next().ok_or("unterminated string")?;
                match escape {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'u' | 'U' => {
                        let width = if escape == 'u' { 4 } else { 8 };
                        let mut code = String::new();
                        for _ in 0..width {
                            code.push(chars.next().ok_or("truncated unicode escape")?.1);
                        }
                        let scalar = u32::from_str_radix(&code, 16)
                            .ok()
                            .and_then(char::from_u32)
                            .ok_or_else(|| format!("invalid unicode escape `\\{escape}{code}`"))?;
                        out.push(scalar);
                    }
                    other => return Err(format!("invalid escape `\\{other}`")),
                }
            }
            c if c.is_control() && c != '\t' => {
                return Err("control character in string".into());
            }
            c => out.push(c),
        }
    }
    Err("unterminated string".into())
}

/// Appends the entries of every included manifest (read from under
/// `source_root`) to `manifest`, each at its `[[include]]`'s line. An
/// included manifest has no includes of its own.
pub fn expand(manifest: &mut Manifest, source_root: &std::path::Path) -> Result<(), Vec<Problem>> {
    let mut problems = Vec::new();
    for include in &manifest.includes {
        let at = |message: String| Problem::at(ProblemKind::Syntax, include.line, message);
        let text = match std::fs::read_to_string(source_root.join(&include.source)) {
            Ok(text) => text,
            Err(error) => {
                problems.push(Problem::at(
                    ProblemKind::SourceMissing,
                    include.line,
                    format!("include `{}`: {error}", include.source),
                ));
                continue;
            }
        };
        match parse(&text) {
            Ok(included) if !included.includes.is_empty() => problems.push(at(format!(
                "include `{}` has [[include]] tables of its own",
                include.source
            ))),
            Ok(included) => manifest
                .entries
                .extend(included.entries.into_iter().map(|entry| Entry {
                    line: include.line,
                    ..entry
                })),
            Err(inner) => problems.extend(
                inner
                    .into_iter()
                    .map(|p| at(format!("include `{}`: {p}", include.source))),
            ),
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}
