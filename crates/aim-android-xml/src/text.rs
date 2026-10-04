//! Text XML, as the platform's pull parser reads what `FastXmlSerializer`
//! and `Xml.newSerializer` write: elements, attributes (all strings),
//! character data, CDATA sections and comments, with the predefined
//! entities and character references. The declaration, processing
//! instructions and a document type declaration are skipped.

use crate::{CDSECT, COMMENT, Element, Node, TEXT, Value};

struct Reader<'a> {
    s: &'a str,
    at: usize,
}

impl<'a> Reader<'a> {
    fn rest(&self) -> &'a str {
        &self.s[self.at..]
    }

    fn error(&self, what: &str) -> String {
        let line = self.s[..self.at].matches('\n').count() + 1;
        format!("XML: {what} at line {line}")
    }

    fn skip_space(&mut self) {
        let rest = self.rest();
        self.at += rest.len() - rest.trim_start().len();
    }

    /// Everything up to `end`, and past it.
    fn until(&mut self, end: &str) -> Result<&'a str, String> {
        let rest = self.rest();
        let n = rest
            .find(end)
            .ok_or_else(|| self.error(&format!("no {end}")))?;
        self.at += n + end.len();
        Ok(&rest[..n])
    }

    fn eat(&mut self, prefix: &str) -> bool {
        let found = self.rest().starts_with(prefix);
        if found {
            self.at += prefix.len();
        }
        found
    }

    fn name(&mut self) -> Result<&'a str, String> {
        let rest = self.rest();
        let n = rest
            .find(|c: char| c.is_whitespace() || "/>=".contains(c))
            .unwrap_or(rest.len());
        if n == 0 {
            return Err(self.error("no name"));
        }
        self.at += n;
        Ok(&rest[..n])
    }

    /// Skips the prolog's or epilog's comments, processing instructions,
    /// document type and space; whether anything is left.
    fn misc(&mut self) -> Result<bool, String> {
        loop {
            self.skip_space();
            if self.eat("<?") {
                self.until("?>")?;
            } else if self.eat("<!--") {
                self.until("-->")?;
            } else if self.eat("<!DOCTYPE") {
                let declaration = self.until(">")?;
                if declaration.contains('[') && !declaration.contains(']') {
                    self.until("]>")?;
                }
            } else {
                return Ok(!self.rest().is_empty());
            }
        }
    }

    /// The element whose `<` was just read.
    fn element(&mut self) -> Result<Element, String> {
        let mut e = Element {
            name: self.name()?.to_owned(),
            attrs: Vec::new(),
            content: Vec::new(),
        };
        loop {
            self.skip_space();
            if self.eat("/>") {
                return Ok(e);
            }
            if self.eat(">") {
                break;
            }
            let name = self.name()?.to_owned();
            self.skip_space();
            if !self.eat("=") {
                return Err(self.error(&format!("attribute {name} without a value")));
            }
            self.skip_space();
            let quote = match self.rest().chars().next() {
                Some(q @ ('"' | '\'')) => q,
                _ => return Err(self.error(&format!("attribute {name} without quotes"))),
            };
            self.at += 1;
            let value = unescape(self.until(&quote.to_string())?).map_err(|w| self.error(&w))?;
            e.attrs.push((name, Value::String(value)));
        }
        loop {
            if self.eat("</") {
                let name = self.name()?;
                self.skip_space();
                if name != e.name || !self.eat(">") {
                    return Err(self.error(&format!("<{}> ends with </{name}", e.name)));
                }
                return Ok(e);
            }
            let node = if self.eat("<!--") {
                Node::Token(COMMENT, Some(self.until("-->")?.to_owned()))
            } else if self.eat("<![CDATA[") {
                Node::Token(CDSECT, Some(self.until("]]>")?.to_owned()))
            } else if self.eat("<?") {
                self.until("?>")?;
                continue;
            } else if self.eat("<") {
                Node::Element(self.element()?)
            } else {
                let rest = self.rest();
                let n = rest.find('<').ok_or_else(|| self.error("no end"))?;
                self.at += n;
                Node::Token(
                    TEXT,
                    Some(unescape(&rest[..n]).map_err(|w| self.error(&w))?),
                )
            };
            e.content.push(node);
        }
    }
}

fn unescape(s: &str) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let semi = rest[amp..]
            .find(';')
            .ok_or_else(|| format!("an unterminated entity in {s:?}"))?;
        let entity = &rest[amp + 1..amp + semi];
        let c = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => match entity.strip_prefix('#') {
                Some(n) => match n.strip_prefix('x') {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => n.parse().ok(),
                }
                .and_then(char::from_u32),
                None => None,
            },
        };
        out.push(c.ok_or_else(|| format!("entity &{entity};"))?);
        rest = &rest[amp + semi + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// The root element of the text XML document `bytes` (UTF-8).
pub fn read(bytes: &[u8]) -> Result<Element, String> {
    read_optional(bytes)?.ok_or_else(|| {
        format!(
            "XML: no root element at line {}",
            bytes.iter().filter(|b| **b == b'\n').count() + 1
        )
    })
}

pub fn read_optional(bytes: &[u8]) -> Result<Option<Element>, String> {
    let s = std::str::from_utf8(bytes).map_err(|e| format!("XML: {e}"))?;
    let mut r = Reader {
        s: s.strip_prefix('\u{feff}').unwrap_or(s),
        at: 0,
    };
    if !r.misc()? {
        return Ok(None);
    }
    if !r.eat("<") {
        return Err(r.error("no root element"));
    }
    let root = r.element()?;
    if r.misc()? {
        return Err(r.error("content after the root element"));
    }
    Ok(Some(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_the_serializer_writes() {
        let doc = "<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n\
                   <!-- state -->\n\
                   <runtime-permissions version=\"10\" fingerprint=\"a/b:16/x&amp;y\">\n  \
                   <package name='org.app'>\n    \
                   <permission name=\"android.permission.CAMERA\" granted=\"true\" flags=\"300\"/>\n  \
                   </package>\n  <shared-user name=\"s\" />\n  \
                   <text>&lt;&#65;&#x42;&gt;<![CDATA[<raw>]]></text>\n\
                   </runtime-permissions>\n";
        let root = read(doc.as_bytes()).unwrap();
        assert_eq!(root.name, "runtime-permissions");
        assert_eq!(root.int("version"), Ok(Some(10)));
        assert_eq!(root.string("fingerprint").as_deref(), Some("a/b:16/x&y"));
        let children: Vec<_> = root.children().collect();
        assert_eq!(children.len(), 3);
        let permission = children[0].children().next().unwrap();
        assert_eq!(permission.bool("granted"), Ok(Some(true)));
        assert_eq!(permission.int_hex("flags"), Ok(Some(0x300)));
        assert_eq!(children[1].attr("name"), Some(&Value::String("s".into())));
        assert_eq!(
            children[2].content,
            [
                Node::Token(TEXT, Some("<AB>".into())),
                Node::Token(CDSECT, Some("<raw>".into()))
            ]
        );
    }

    #[test]
    fn rejects_broken_documents() {
        for doc in [
            "",
            "<?xml version='1.0'?>",
            "<a>",
            "<a></b>",
            "<a x=1/>",
            "<a x/>",
            "<a>&nbsp;</a>",
            "<a/><b/>",
        ] {
            assert!(read(doc.as_bytes()).is_err(), "{doc}");
        }
    }
}
