//! Binary XML (`BinaryXmlSerializer`, `BinaryXmlPullParser`): after the
//! magic, one byte per token (its event in the low nibble, its value's
//! type in the high nibble) and the token's data, big-endian. Element and
//! attribute names, and values written with `attributeInterned`, are
//! interned: a string's first use carries it and gets the next index,
//! later uses carry the index. Strings are Java's modified UTF-8 with a
//! 16-bit length.

use std::collections::HashMap;

use crate::{Element, Node, Value};

pub const MAGIC: &[u8] = b"ABX\0";

const START_DOCUMENT: u8 = 0;
const END_DOCUMENT: u8 = 1;
const START_TAG: u8 = 2;
const END_TAG: u8 = 3;
const ATTRIBUTE: u8 = 15;

const TYPE_NULL: u8 = 1 << 4;
const TYPE_STRING: u8 = 2 << 4;
const TYPE_STRING_INTERNED: u8 = 3 << 4;
const TYPE_BYTES_HEX: u8 = 4 << 4;
const TYPE_BYTES_BASE64: u8 = 5 << 4;
const TYPE_INT: u8 = 6 << 4;
const TYPE_INT_HEX: u8 = 7 << 4;
const TYPE_LONG: u8 = 8 << 4;
const TYPE_LONG_HEX: u8 = 9 << 4;
const TYPE_FLOAT: u8 = 10 << 4;
const TYPE_DOUBLE: u8 = 11 << 4;
const TYPE_BOOLEAN_TRUE: u8 = 12 << 4;
const TYPE_BOOLEAN_FALSE: u8 = 13 << 4;

/// The interned strings' index that marks a new string; also how many a
/// document can intern.
const NEW: u16 = u16::MAX;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    interned: Vec<String>,
}

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], String> {
        Ok(self.slice(N)?.try_into().unwrap())
    }

    fn slice(&mut self, n: usize) -> Result<&[u8], String> {
        let out = self
            .bytes
            .get(self.at..self.at + n)
            .ok_or_else(|| format!("ABX: truncated at {}", self.at))?;
        self.at += n;
        Ok(out)
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_be_bytes(self.take()?))
    }

    fn utf(&mut self) -> Result<String, String> {
        let len = self.u16()? as usize;
        let at = self.at;
        decode_utf(self.slice(len)?).ok_or_else(|| format!("ABX: bad string at {at}"))
    }

    fn interned(&mut self) -> Result<String, String> {
        match self.u16()? {
            NEW => {
                let s = self.utf()?;
                if self.interned.len() < NEW as usize {
                    self.interned.push(s.clone());
                }
                Ok(s)
            }
            i => self
                .interned
                .get(i as usize)
                .cloned()
                .ok_or_else(|| format!("ABX: no interned string {i}")),
        }
    }

    fn bytes(&mut self) -> Result<Vec<u8>, String> {
        let len = self.u16()? as usize;
        Ok(self.slice(len)?.to_vec())
    }

    fn value(&mut self, kind: u8) -> Result<Value, String> {
        Ok(match kind {
            TYPE_NULL => Value::Null,
            TYPE_STRING => Value::String(self.utf()?),
            TYPE_STRING_INTERNED => Value::Interned(self.interned()?),
            TYPE_BYTES_HEX => Value::BytesHex(self.bytes()?),
            TYPE_BYTES_BASE64 => Value::BytesBase64(self.bytes()?),
            TYPE_INT => Value::Int(i32::from_be_bytes(self.take()?)),
            TYPE_INT_HEX => Value::IntHex(i32::from_be_bytes(self.take()?)),
            TYPE_LONG => Value::Long(i64::from_be_bytes(self.take()?)),
            TYPE_LONG_HEX => Value::LongHex(i64::from_be_bytes(self.take()?)),
            TYPE_FLOAT => Value::Float(f32::from_be_bytes(self.take()?)),
            TYPE_DOUBLE => Value::Double(f64::from_be_bytes(self.take()?)),
            TYPE_BOOLEAN_TRUE => Value::Bool(true),
            TYPE_BOOLEAN_FALSE => Value::Bool(false),
            other => return Err(format!("ABX: value type {}", other >> 4)),
        })
    }
}

/// The root element of the binary XML document `bytes`. Tokens outside
/// the root element are left out.
pub fn read(bytes: &[u8]) -> Result<Element, String> {
    read_optional(bytes)?.ok_or_else(|| "ABX: no root element".into())
}

pub fn read_optional(bytes: &[u8]) -> Result<Option<Element>, String> {
    if !bytes.starts_with(MAGIC) {
        return Err("not ABX".into());
    }
    let mut r = Reader {
        bytes,
        at: MAGIC.len(),
        interned: Vec::new(),
    };
    let mut open: Vec<Element> = Vec::new();
    let mut root = None;
    loop {
        let [token] = r.take()?;
        let (event, kind) = (token & 0x0f, token & 0xf0);
        match event {
            START_TAG => open.push(Element {
                name: r.interned()?,
                attrs: Vec::new(),
                content: Vec::new(),
            }),
            ATTRIBUTE => {
                let name = r.interned()?;
                let value = r.value(kind)?;
                let element = open
                    .last_mut()
                    .ok_or("ABX: an attribute outside an element")?;
                element.attrs.push((name, value));
            }
            END_TAG => {
                let name = r.interned()?;
                let element = open.pop().ok_or("ABX: an end tag outside an element")?;
                if element.name != name {
                    return Err(format!("ABX: <{}> ends with </{name}>", element.name));
                }
                match open.last_mut() {
                    Some(parent) => parent.content.push(Node::Element(element)),
                    None if root.is_none() => root = Some(element),
                    None => return Err("ABX: a second root element".into()),
                }
            }
            START_DOCUMENT => {}
            END_DOCUMENT => break,
            _ => {
                let text = match r.value(kind)? {
                    Value::Null => None,
                    Value::String(s) => Some(s),
                    other => return Err(format!("ABX: token {event} with {other:?}")),
                };
                if let Some(parent) = open.last_mut() {
                    parent.content.push(Node::Token(event, text));
                }
            }
        }
    }
    if !open.is_empty() {
        return Err(format!("ABX: <{}> does not end", open[0].name));
    }
    Ok(root)
}

/// `root` as one binary XML document, written as `BinaryXmlSerializer`
/// writes it: a document whose values keep their types is written back
/// byte for byte.
pub fn write(root: &Element) -> Result<Vec<u8>, String> {
    let mut w = Writer {
        out: MAGIC.to_vec(),
        interned: HashMap::new(),
    };
    w.out.push(START_DOCUMENT | TYPE_NULL);
    w.element(root)?;
    w.out.push(END_DOCUMENT | TYPE_NULL);
    Ok(w.out)
}

struct Writer {
    out: Vec<u8>,
    interned: HashMap<String, u16>,
}

impl Writer {
    fn len(&mut self, n: usize) -> Result<(), String> {
        let n = u16::try_from(n).map_err(|_| format!("ABX: {n} bytes in one value"))?;
        self.out.extend(n.to_be_bytes());
        Ok(())
    }

    fn utf(&mut self, s: &str) -> Result<(), String> {
        let bytes = encode_utf(s);
        self.len(bytes.len())?;
        self.out.extend(bytes);
        Ok(())
    }

    fn interned(&mut self, s: &str) -> Result<(), String> {
        if let Some(i) = self.interned.get(s) {
            self.out.extend(i.to_be_bytes());
            return Ok(());
        }
        self.out.extend(NEW.to_be_bytes());
        self.utf(s)?;
        let next = self.interned.len();
        if next < NEW as usize {
            self.interned.insert(s.to_owned(), next as u16);
        }
        Ok(())
    }

    fn element(&mut self, e: &Element) -> Result<(), String> {
        self.out.push(START_TAG | TYPE_STRING_INTERNED);
        self.interned(&e.name)?;
        for (name, value) in &e.attrs {
            self.attribute(name, value)?;
        }
        for node in &e.content {
            match node {
                Node::Element(child) => self.element(child)?,
                Node::Token(event, None) => self.out.push(event | TYPE_NULL),
                Node::Token(event, Some(text)) => {
                    self.out.push(event | TYPE_STRING);
                    self.utf(text)?;
                }
            }
        }
        self.out.push(END_TAG | TYPE_STRING_INTERNED);
        self.interned(&e.name)
    }

    fn attribute(&mut self, name: &str, value: &Value) -> Result<(), String> {
        let kind = match value {
            Value::Null => TYPE_NULL,
            Value::String(_) => TYPE_STRING,
            Value::Interned(_) => TYPE_STRING_INTERNED,
            Value::BytesHex(_) => TYPE_BYTES_HEX,
            Value::BytesBase64(_) => TYPE_BYTES_BASE64,
            Value::Int(_) => TYPE_INT,
            Value::IntHex(_) => TYPE_INT_HEX,
            Value::Long(_) => TYPE_LONG,
            Value::LongHex(_) => TYPE_LONG_HEX,
            Value::Float(_) => TYPE_FLOAT,
            Value::Double(_) => TYPE_DOUBLE,
            Value::Bool(true) => TYPE_BOOLEAN_TRUE,
            Value::Bool(false) => TYPE_BOOLEAN_FALSE,
        };
        self.out.push(ATTRIBUTE | kind);
        self.interned(name)?;
        match value {
            Value::Null | Value::Bool(_) => {}
            Value::String(s) => self.utf(s)?,
            Value::Interned(s) => self.interned(s)?,
            Value::BytesHex(b) | Value::BytesBase64(b) => {
                self.len(b.len())?;
                self.out.extend(b);
            }
            Value::Int(v) | Value::IntHex(v) => self.out.extend(v.to_be_bytes()),
            Value::Long(v) | Value::LongHex(v) => self.out.extend(v.to_be_bytes()),
            Value::Float(v) => self.out.extend(v.to_be_bytes()),
            Value::Double(v) => self.out.extend(v.to_be_bytes()),
        }
        Ok(())
    }
}

/// Java's modified UTF-8 (`ModifiedUtf8`): UTF-16 units, NUL in two
/// bytes, a supplementary character as its two surrogates in three bytes
/// each.
fn decode_utf(b: &[u8]) -> Option<String> {
    let mut units = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let cont = |j: usize| {
            b.get(j)
                .filter(|c| *c & 0xc0 == 0x80)
                .map(|c| (c & 0x3f) as u16)
        };
        let (unit, n) = match b[i] {
            c @ 0x01..=0x7f => (c as u16, 1),
            c @ 0xc0..=0xdf => (((c & 0x1f) as u16) << 6 | cont(i + 1)?, 2),
            c @ 0xe0..=0xef => (
                ((c & 0x0f) as u16) << 12 | cont(i + 1)? << 6 | cont(i + 2)?,
                3,
            ),
            _ => return None,
        };
        units.push(unit);
        i += n;
    }
    String::from_utf16(&units).ok()
}

fn encode_utf(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for u in s.encode_utf16() {
        match u {
            0x01..=0x7f => out.push(u as u8),
            0x00..=0x7ff => out.extend([0xc0 | (u >> 6) as u8, 0x80 | (u & 0x3f) as u8]),
            _ => out.extend([
                0xe0 | (u >> 12) as u8,
                0x80 | (u >> 6 & 0x3f) as u8,
                0x80 | (u & 0x3f) as u8,
            ]),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf(b: &mut Vec<u8>, s: &str) {
        b.extend((s.len() as u16).to_be_bytes());
        b.extend(s.as_bytes());
    }

    fn new(b: &mut Vec<u8>, s: &str) {
        b.extend(NEW.to_be_bytes());
        utf(b, s);
    }

    /// A document as `BinaryXmlSerializer` writes it, byte by byte.
    fn serialized() -> Vec<u8> {
        let mut b = MAGIC.to_vec();
        b.push(START_DOCUMENT | TYPE_NULL);
        b.push(START_TAG | TYPE_STRING_INTERNED);
        new(&mut b, "packages");
        b.push(START_TAG | TYPE_STRING_INTERNED);
        new(&mut b, "package");
        b.push(ATTRIBUTE | TYPE_STRING);
        new(&mut b, "name");
        utf(&mut b, "android");
        b.push(ATTRIBUTE | TYPE_INT);
        new(&mut b, "userId");
        b.extend(1000i32.to_be_bytes());
        b.push(ATTRIBUTE | TYPE_LONG_HEX);
        new(&mut b, "ft");
        b.extend(0x1234i64.to_be_bytes());
        b.push(ATTRIBUTE | TYPE_BOOLEAN_TRUE);
        new(&mut b, "isOrphaned");
        b.push(ATTRIBUTE | TYPE_BYTES_HEX);
        new(&mut b, "key");
        b.extend(2u16.to_be_bytes());
        b.extend([0x30, 0x82]);
        b.push(END_TAG | TYPE_STRING_INTERNED);
        b.extend(1u16.to_be_bytes());
        b.push(START_TAG | TYPE_STRING_INTERNED);
        b.extend(1u16.to_be_bytes());
        b.push(ATTRIBUTE | TYPE_STRING_INTERNED);
        b.extend(2u16.to_be_bytes());
        b.extend(0u16.to_be_bytes());
        b.push(crate::TEXT | TYPE_STRING);
        utf(&mut b, "text");
        b.push(END_TAG | TYPE_STRING_INTERNED);
        b.extend(1u16.to_be_bytes());
        b.push(END_TAG | TYPE_STRING_INTERNED);
        b.extend(0u16.to_be_bytes());
        b.push(END_DOCUMENT | TYPE_NULL);
        b
    }

    #[test]
    fn reads_elements_and_typed_attributes() {
        let root = read(&serialized()).unwrap();
        assert_eq!(root.name, "packages");
        let p: Vec<_> = root.children().collect();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].string("name").as_deref(), Some("android"));
        assert_eq!(p[0].attr("userId"), Some(&Value::Int(1000)));
        assert_eq!(p[0].attr("ft"), Some(&Value::LongHex(0x1234)));
        assert_eq!(p[0].attr("isOrphaned"), Some(&Value::Bool(true)));
        assert_eq!(p[0].bytes_hex("key"), Ok(Some(vec![0x30, 0x82])));
        assert_eq!(p[1].attr("name"), Some(&Value::Interned("packages".into())));
        assert_eq!(
            p[1].content,
            [Node::Token(crate::TEXT, Some("text".into()))]
        );
        assert!(read(b"<?xml").is_err());
    }

    #[test]
    fn writes_what_it_reads() {
        let bytes = serialized();
        assert_eq!(write(&read(&bytes).unwrap()).unwrap(), bytes);
    }

    #[test]
    fn rejects_broken_documents() {
        let bytes = serialized();
        assert!(read(&bytes[..bytes.len() - 4]).is_err());
        let mut unknown = bytes.clone();
        // The first attribute's type.
        let at = bytes
            .iter()
            .position(|b| *b == ATTRIBUTE | TYPE_STRING)
            .unwrap();
        unknown[at] = ATTRIBUTE | 14 << 4;
        assert!(read(&unknown).is_err());
    }

    #[test]
    fn strings_are_modified_utf8() {
        for s in ["", "a\u{0}b", "é", "한", "😀"] {
            let b = encode_utf(s);
            assert_eq!(decode_utf(&b).as_deref(), Some(s));
        }
        assert_eq!(encode_utf("\u{0}"), [0xc0, 0x80]);
        assert_eq!(encode_utf("😀"), [0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80]);
        assert_eq!(decode_utf(&[0x00]), None);
        assert_eq!(decode_utf(&[0xf0, 0x9f, 0x98, 0x80]), None);
    }
}
