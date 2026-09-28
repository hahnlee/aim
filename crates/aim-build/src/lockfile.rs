//! The source locks (`hal/sources.lock`, `daemons/sources.lock`,
//! `patches/art-android/sources.lock`, `upstream/*.lock`, ...): shell-style
//! assignments, which the Python helpers also receive entry by entry.
//!
//! Accepted: `# comments`, `NAME=value`, `NAME=(` one or more `"entries"`
//! per line `)`, `unset NAME...`, and `$NAME`/`${NAME}` of an earlier scalar
//! inside values.

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Default)]
pub struct Lock {
    scalars: HashMap<String, String>,
    arrays: HashMap<String, Vec<String>>,
}

impl Lock {
    pub fn read(path: &Path) -> Result<Lock, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn get(&self, name: &str) -> Result<&str, String> {
        self.scalars
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("no {name}"))
    }

    pub fn array(&self, name: &str) -> &[String] {
        self.arrays.get(name).map_or(&[], Vec::as_slice)
    }
}

fn expand(value: &str, scalars: &HashMap<String, String>) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        rest = &rest[at + 1..];
        let (name, after) = if let Some(braced) = rest.strip_prefix('{') {
            let end = braced.find('}').ok_or("unterminated ${")?;
            (&braced[..end], &braced[end + 1..])
        } else {
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            (&rest[..end], &rest[end..])
        };
        out.push_str(
            scalars
                .get(name)
                .ok_or_else(|| format!("${name} is not set"))?,
        );
        rest = after;
    }
    out.push_str(rest);
    Ok(out)
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

/// The quoted entries of one array line.
fn entries(line: &str) -> Result<Vec<&str>, String> {
    let mut out = Vec::new();
    let mut rest = line.trim();
    while !rest.is_empty() {
        let body = rest
            .strip_prefix('"')
            .ok_or_else(|| format!("expected a quoted entry: {line}"))?;
        let end = body
            .find('"')
            .ok_or_else(|| format!("unterminated entry: {line}"))?;
        out.push(&body[..end]);
        rest = body[end + 1..].trim_start();
    }
    Ok(out)
}

pub fn parse(text: &str) -> Result<Lock, String> {
    let mut lock = Lock::default();
    let mut array: Option<(String, Vec<String>)> = None;
    for (number, raw) in text.lines().enumerate() {
        let line = raw.trim();
        let context = |e: String| format!("line {}: {e}", number + 1);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, items)) = array.as_mut() {
            if line == ")" {
                let (name, items) = (std::mem::take(name), std::mem::take(items));
                lock.arrays.insert(name, items);
                array = None;
            } else {
                for entry in entries(line).map_err(context)? {
                    items.push(expand(entry, &lock.scalars).map_err(context)?);
                }
            }
            continue;
        }
        if let Some(names) = line.strip_prefix("unset ") {
            for name in names.split_whitespace() {
                lock.scalars.remove(name);
            }
            continue;
        }
        let (name, value) = line
            .split_once('=')
            .ok_or_else(|| context(format!("not an assignment: {line}")))?;
        if value == "(" {
            array = Some((name.to_string(), Vec::new()));
        } else {
            let value = expand(unquote(value), &lock.scalars).map_err(context)?;
            lock.scalars.insert(name.to_string(), value);
        }
    }
    if let Some((name, _)) = array {
        return Err(format!("{name}=( is not closed"));
    }
    Ok(lock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalars_arrays_and_expansion() {
        let lock = parse(
            "# comment\nTAG=android-16\nhw=platform/hw\nLIST=(\n  \"$hw|a|${TAG}\"\n  \"b\" \"c\"\n)\nunset hw\n",
        )
        .unwrap();
        assert_eq!(lock.get("TAG").unwrap(), "android-16");
        assert!(lock.get("hw").is_err());
        assert_eq!(lock.array("LIST"), ["platform/hw|a|android-16", "b", "c"]);
    }

    #[test]
    fn errors() {
        assert!(parse("X=(\n\"a\"\n").is_err());
        assert!(parse("X=$Y\n").is_err());
        assert!(parse("junk\n").is_err());
    }
}
