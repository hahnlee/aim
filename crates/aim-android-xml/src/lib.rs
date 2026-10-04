//! Android's XML persistence: the documents system_server and its modules
//! keep their state in (`packages.xml`, `access.abx`, ...), read as
//! `Xml.resolvePullParser` reads them, binary XML ("ABX",
//! `BinaryXmlPullParser`) when a file starts with its magic and text XML
//! otherwise, with attribute values converted as `TypedXmlPullParser`
//! converts them. [`abx::write`] writes binary XML as
//! `BinaryXmlSerializer` does. Sources are read at `android-16.0.0_r1`
//! (frameworks/libs/modules-utils).

pub mod abx;
mod base64;
mod double;
pub mod text;
pub mod pull;

use std::borrow::Cow;

/// An attribute's value, typed as the writer typed it: binary XML keeps the
/// type of each `TypedXmlSerializer.attribute*` call, text XML has strings
/// only.
#[derive(Clone, Debug)]
pub enum Value {
    Null,
    String(String),
    /// A string written with `attributeInterned`.
    Interned(String),
    BytesHex(Vec<u8>),
    BytesBase64(Vec<u8>),
    Int(i32),
    IntHex(i32),
    Long(i64),
    LongHex(i64),
    Float(f32),
    Double(f64),
    Bool(bool),
}

// Document identity retains the writer's type and floating-point bits. This
// also lets persistence owners compare unchanged NaNs and detect signed-zero
// changes made by another writer.
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::String(a), Self::String(b)) | (Self::Interned(a), Self::Interned(b)) => a == b,
            (Self::BytesHex(a), Self::BytesHex(b))
            | (Self::BytesBase64(a), Self::BytesBase64(b)) => a == b,
            (Self::Int(a), Self::Int(b)) | (Self::IntHex(a), Self::IntHex(b)) => a == b,
            (Self::Long(a), Self::Long(b)) | (Self::LongHex(a), Self::LongHex(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => a.to_bits() == b.to_bits(),
            (Self::Double(a), Self::Double(b)) => a.to_bits() == b.to_bits(),
            (Self::Bool(a), Self::Bool(b)) => a == b,
            _ => false,
        }
    }
}

/// A node of an element's content: an element, or another token (text,
/// a comment, ...) by its `XmlPullParser` event type, with its text.
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Element(Element),
    Token(u8, Option<String>),
}

/// `XmlPullParser.TEXT`.
pub const TEXT: u8 = 4;
/// `XmlPullParser.CDSECT`.
pub const CDSECT: u8 = 5;
/// `XmlPullParser.COMMENT`.
pub const COMMENT: u8 = 9;
/// `XmlPullParser.ENTITY_REF`.
pub const ENTITY_REF: u8 = 6;
/// `XmlPullParser.IGNORABLE_WHITESPACE`.
pub const IGNORABLE_WHITESPACE: u8 = 7;
/// `XmlPullParser.PROCESSING_INSTRUCTION`.
pub const PROCESSING_INSTRUCTION: u8 = 8;
/// `XmlPullParser.DOCDECL`.
pub const DOCDECL: u8 = 10;

#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, Value)>,
    pub content: Vec<Node>,
}

/// The document `bytes`, binary or text XML: its root element.
pub fn read(bytes: &[u8]) -> Result<Element, String> {
    if bytes.starts_with(abx::MAGIC) {
        abx::read(bytes)
    } else {
        text::read(bytes)
    }
}

/// Read a document for owners using `XmlPullParser.next()`. Preserve comments
/// and other unrelated nodes, but normalize text/CDATA/entity events so a
/// subsequent ABX write retains their meaning. BinaryXmlPullParser skips a
/// standalone CDATA/entity token; text XML reports CDATA as text.
pub fn read_next(bytes: &[u8]) -> Result<Element, String> {
    read_next_optional(bytes)?.ok_or_else(|| {
        if bytes.starts_with(abx::MAGIC) {
            "ABX: no root element".into()
        } else {
            format!(
                "XML: no root element at line {}",
                bytes.iter().filter(|b| **b == b'\n').count() + 1
            )
        }
    })
}

/// Pull-parser END_DOCUMENT without START_TAG is distinct from malformed XML.
/// Required-root readers continue to reject this through read/read_next.
pub fn read_next_optional(bytes: &[u8]) -> Result<Option<Element>, String> {
    fn normalize(e: &mut Element, binary: bool) -> Result<(), String> {
        let mut pending_text = false;
        let mut content = Vec::new();
        for mut node in e.content.drain(..) {
            match &mut node {
                Node::Element(child) => {
                    normalize(child, binary)?;
                    pending_text = false;
                }
                Node::Token(TEXT, _) => pending_text = true,
                Node::Token(CDSECT, _) if !binary || pending_text => {
                    let Node::Token(kind, _) = &mut node else {
                        unreachable!()
                    };
                    *kind = TEXT;
                    pending_text = true;
                }
                Node::Token(CDSECT, _) => continue,
                Node::Token(6, Some(name)) if binary => {
                    let value = match name.as_str() {
                        "lt" => "<".into(),
                        "gt" => ">".into(),
                        "amp" => "&".into(),
                        "apos" => "'".into(),
                        "quot" => "\"".into(),
                        name if name.starts_with('#') => {
                            let value: i32 = name[1..]
                                .parse()
                                .map_err(|_| format!("invalid entity {name}"))?;
                            char::from_u32(value as u16 as u32)
                                .ok_or_else(|| format!("invalid UTF-16 entity {name}"))?
                                .to_string()
                        }
                        _ => return Err(format!("unknown entity {name}")),
                    };
                    if !pending_text {
                        continue;
                    }
                    node = Node::Token(TEXT, Some(value));
                }
                Node::Token(COMMENT | 8, _) => {}
                _ => pending_text = false,
            }
            content.push(node);
        }
        e.content = content;
        Ok(())
    }
    let mut root = if bytes.starts_with(abx::MAGIC) {
        abx::read_optional(bytes)?
    } else {
        text::read_optional(bytes)?
    };
    if let Some(root) = &mut root {
        normalize(root, bytes.starts_with(abx::MAGIC))?;
    }
    Ok(root)
}

impl Value {
    /// The value as `getAttributeValue` returns it.
    pub fn string(&self) -> Option<Cow<'_, str>> {
        Some(match self {
            Value::Null => return None,
            Value::String(s) | Value::Interned(s) => Cow::Borrowed(s),
            Value::BytesHex(b) => b.iter().map(|b| format!("{b:02x}")).collect(),
            Value::BytesBase64(b) => base64::encode(b).into(),
            Value::Int(v) => v.to_string().into(),
            Value::IntHex(v) => signed_hex(*v as i64).into(),
            Value::Long(v) => v.to_string().into(),
            Value::LongHex(v) => signed_hex(*v).into(),
            Value::Float(v) => v.to_string().into(),
            Value::Double(v) => v.to_string().into(),
            Value::Bool(v) => v.to_string().into(),
        })
    }

    fn text(&self) -> Option<&str> {
        match self {
            Value::String(s) | Value::Interned(s) => Some(s),
            _ => None,
        }
    }
}

/// `Integer.toString(v, 16)`: a sign and the magnitude, not two's
/// complement.
fn signed_hex(v: i64) -> String {
    if v < 0 {
        format!("-{:x}", v.unsigned_abs())
    } else {
        format!("{v:x}")
    }
}

/// `Integer.parseInt(s, radix)` and `Long.parseLong`: an optional sign and
/// digits, nothing else.
fn parse_radix(s: &str, radix: u32) -> Option<i64> {
    let (negative, digits) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    let magnitude = i128::from_str_radix(digits, radix).ok()?;
    i64::try_from(if negative { -magnitude } else { magnitude }).ok()
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&Value> {
        self.attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    /// The child elements.
    pub fn children(&self) -> impl Iterator<Item = &Element> {
        self.content.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            Node::Token(..) => None,
        })
    }

    /// `getAttributeValue`.
    pub fn string(&self, name: &str) -> Option<Cow<'_, str>> {
        self.attr(name).and_then(Value::string)
    }

    /// A typed attribute: `None` when absent, an error when its value does
    /// not convert, as `TypedXmlPullParser`'s getters throw.
    fn typed<T>(
        &self,
        name: &str,
        typed: impl Fn(&Value) -> Option<T>,
        parse: impl Fn(&str) -> Option<T>,
    ) -> Result<Option<T>, String> {
        let Some(v) = self.attr(name) else {
            return Ok(None);
        };
        typed(v)
            .or_else(|| v.text().and_then(&parse))
            .map(Some)
            .ok_or_else(|| format!("<{}>: attribute {name}: {v:?}", self.name))
    }

    /// `getAttributeInt`.
    pub fn int(&self, name: &str) -> Result<Option<i32>, String> {
        self.typed(name, int, |s| parse_radix(s, 10)?.try_into().ok())
    }

    /// `getAttributeIntHex`.
    pub fn int_hex(&self, name: &str) -> Result<Option<i32>, String> {
        self.typed(name, int, |s| parse_radix(s, 16)?.try_into().ok())
    }

    /// `getAttributeLong`.
    pub fn long(&self, name: &str) -> Result<Option<i64>, String> {
        self.typed(name, long, |s| parse_radix(s, 10))
    }

    /// `getAttributeLongHex`.
    pub fn long_hex(&self, name: &str) -> Result<Option<i64>, String> {
        self.typed(name, long, |s| parse_radix(s, 16))
    }

    /// `getAttributeFloat`.
    pub fn float(&self, name: &str) -> Result<Option<f32>, String> {
        let float = |v: &Value| match v {
            Value::Float(f) => Some(*f),
            _ => None,
        };
        self.typed(name, float, double::parse_float)
    }

    /// `getAttributeDouble`, including Java's hexadecimal float syntax.
    pub fn double(&self, name: &str) -> Result<Option<f64>, String> {
        self.typed(
            name,
            |v| match v {
                Value::Double(value) => Some(*value),
                _ => None,
            },
            double::parse,
        )
    }

    /// `getAttributeBoolean`.
    pub fn bool(&self, name: &str) -> Result<Option<bool>, String> {
        let bool = |v: &Value| match v {
            Value::Bool(b) => Some(*b),
            _ => None,
        };
        self.typed(name, bool, |s| {
            [("true", true), ("false", false)]
                .into_iter()
                .find(|(t, _)| s.eq_ignore_ascii_case(t))
                .map(|(_, b)| b)
        })
    }

    /// `getAttributeBytesHex`.
    pub fn bytes_hex(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        self.typed(name, bytes, |s| {
            if s.len() % 2 != 0 || !s.is_ascii() {
                return None;
            }
            (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
                .collect()
        })
    }

    /// `getAttributeBytesBase64`.
    pub fn bytes_base64(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        self.typed(name, bytes, base64::decode)
    }
}

fn int(v: &Value) -> Option<i32> {
    match v {
        Value::Int(i) | Value::IntHex(i) => Some(*i),
        _ => None,
    }
}

fn long(v: &Value) -> Option<i64> {
    match v {
        Value::Long(l) | Value::LongHex(l) => Some(*l),
        _ => None,
    }
}

fn bytes(v: &Value) -> Option<Vec<u8>> {
    match v {
        Value::BytesHex(b) | Value::BytesBase64(b) => Some(b.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn element(attrs: &[(&str, Value)]) -> Element {
        Element {
            name: "e".into(),
            attrs: attrs
                .iter()
                .map(|(n, v)| (n.to_string(), v.clone()))
                .collect(),
            content: Vec::new(),
        }
    }

    #[test]
    fn typed_values_convert_as_the_parsers_do() {
        let e = element(&[
            ("i", Value::Int(-7)),
            ("ih", Value::IntHex(-1)),
            ("lh", Value::LongHex(0x1234)),
            ("b", Value::Bool(true)),
            ("bh", Value::BytesHex(vec![0xab, 0x01])),
            ("b64", Value::BytesBase64(b"key".to_vec())),
            ("f", Value::Float(1.5)),
            ("n", Value::Null),
        ]);
        assert_eq!(e.int("i"), Ok(Some(-7)));
        assert_eq!(e.string("ih").as_deref(), Some("-1"));
        assert_eq!(e.int_hex("ih"), Ok(Some(-1)));
        assert_eq!(e.string("lh").as_deref(), Some("1234"));
        assert_eq!(e.long_hex("lh"), Ok(Some(0x1234)));
        assert_eq!(e.bool("b"), Ok(Some(true)));
        assert_eq!(e.string("bh").as_deref(), Some("ab01"));
        assert_eq!(e.string("b64").as_deref(), Some("a2V5"));
        assert_eq!(e.bytes_base64("b64"), Ok(Some(b"key".to_vec())));
        assert_eq!(e.float("f"), Ok(Some(1.5)));
        assert_eq!(e.string("n"), None);
        assert_eq!(e.int("missing"), Ok(None));
        // A typed value converts only to its own kind.
        assert!(e.long("i").is_err());
        assert!(e.int("b").is_err());
    }

    #[test]
    fn strings_parse_as_the_text_parser_does() {
        let s = |v: &str| Value::String(v.into());
        let e = element(&[
            ("i", s("-12")),
            ("h", s("ff")),
            ("nh", s("-1")),
            ("l", s("1700000000000")),
            ("t", s("TRUE")),
            ("bh", s("AB01")),
            ("b64", s("a2V5")),
            ("bad", s("12x")),
        ]);
        assert_eq!(e.int("i"), Ok(Some(-12)));
        assert_eq!(e.int_hex("h"), Ok(Some(255)));
        assert_eq!(e.int_hex("nh"), Ok(Some(-1)));
        assert_eq!(e.long("l"), Ok(Some(1_700_000_000_000)));
        assert_eq!(e.bool("t"), Ok(Some(true)));
        assert_eq!(e.bytes_hex("bh"), Ok(Some(vec![0xab, 0x01])));
        assert_eq!(e.bytes_base64("b64"), Ok(Some(b"key".to_vec())));
        assert!(e.int("bad").is_err());
        assert!(e.bool("i").is_err());
        // `Integer.parseInt(s, 16)` takes no two's complement.
        assert!(element(&[("h", s("ffffffff"))]).int_hex("h").is_err());
    }
}

#[cfg(test)]
mod optional_document_tests {
    #[test]
    fn no_start_tag_is_distinct_from_malformed_or_truncated_input() {
        for bytes in [b"".as_slice(), b" \n", b"ABX\0\x10\x11"] {
            assert_eq!(super::read_next_optional(bytes).unwrap(), None);
            assert!(super::read_next(bytes).is_err());
            assert!(super::read(bytes).is_err());
        }
        assert!(super::read_next_optional(b"ABX\0").is_err());
        assert!(super::read_next_optional(b"<packages><").is_err());
        assert_eq!(
            super::read_next_optional(b"<packages/>")
                .unwrap()
                .unwrap()
                .name,
            "packages"
        );
    }
}
