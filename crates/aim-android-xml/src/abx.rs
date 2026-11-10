//! Binary XML (`BinaryXmlSerializer`, `BinaryXmlPullParser`): after the
//! magic, one byte per token (its event in the low nibble, its value's
//! type in the high nibble) and the token's data, big-endian. Element and
//! attribute names, and values written with `attributeInterned`, are
//! interned: a string's first use carries it and gets the next index,
//! later uses carry the index. Strings have a 16-bit byte length and are
//! Java strings in ART's JNI UTF-8 ([`decode_utf`], [`encode_utf`]).

use std::collections::HashMap;

use crate::{Element, JavaString, Node, Value};

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
    /// `Value::Interned` or `Value::InternedUtf16`.
    interned: Vec<Value>,
    /// Keep strings that are not well-formed UTF-16 as `Value::Utf16`.
    lossless: bool,
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

    fn utf16(&mut self) -> Result<JavaString, String> {
        let len = self.u16()? as usize;
        Ok(JavaString::from_units(decode_utf(self.slice(len)?)))
    }

    /// A string the model holds as a Rust `String`: names, and other strings
    /// outside a lossless read.
    fn utf(&mut self) -> Result<String, String> {
        let at = self.at;
        self.utf16()?
            .to_rust()
            .map_err(|e| format!("ABX: string at {at}: {e}"))
    }

    /// A `Value::String` or, in a lossless read, `Value::Utf16`.
    fn string(&mut self) -> Result<Value, String> {
        let at = self.at;
        let value = self.utf16()?.value(false);
        self.admit(at, value)
    }

    /// `value`, unless it is `Utf16` outside a lossless read.
    fn admit(&self, at: usize, value: Value) -> Result<Value, String> {
        match &value {
            Value::Utf16(s) | Value::InternedUtf16(s) if !self.lossless => {
                Err(format!("ABX: string at {at}: {}", s.to_rust().unwrap_err()))
            }
            _ => Ok(value),
        }
    }

    fn interned_value(&mut self) -> Result<Value, String> {
        let at = self.at;
        let value = match self.u16()? {
            NEW => {
                let value = self.utf16()?.value(true);
                if self.interned.len() < NEW as usize {
                    self.interned.push(value.clone());
                }
                value
            }
            i => self
                .interned
                .get(i as usize)
                .cloned()
                .ok_or_else(|| format!("ABX: no interned string {i}"))?,
        };
        self.admit(at, value)
    }

    /// An interned name: always a Rust `String`.
    fn interned(&mut self) -> Result<String, String> {
        let at = self.at;
        match self.interned_value()? {
            Value::Interned(s) => Ok(s),
            Value::InternedUtf16(s) => {
                Err(format!("ABX: name at {at}: {}", s.to_rust().unwrap_err()))
            }
            _ => unreachable!(),
        }
    }

    fn bytes(&mut self) -> Result<Vec<u8>, String> {
        let len = self.u16()? as usize;
        Ok(self.slice(len)?.to_vec())
    }

    fn value(&mut self, kind: u8) -> Result<Value, String> {
        Ok(match kind {
            TYPE_NULL => Value::Null,
            TYPE_STRING => self.string()?,
            TYPE_STRING_INTERNED => self.interned_value()?,
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
    let mut roots = read_sequence(bytes)?;
    if roots.len() > 1 {
        return Err("ABX: a second root element".into());
    }
    Ok(roots.pop())
}

/// The root elements of `bytes` in order: `TypedXmlSerializer` writes
/// independent sibling sections ([`write_sequence`]), as SettingsState
/// does after its settings.
pub fn read_sequence(bytes: &[u8]) -> Result<Vec<Element>, String> {
    read_sequence_with(bytes, false)
}

pub(crate) fn read_sequence_with(bytes: &[u8], lossless: bool) -> Result<Vec<Element>, String> {
    if !bytes.starts_with(MAGIC) {
        return Err("not ABX".into());
    }
    let mut r = Reader {
        bytes,
        at: MAGIC.len(),
        interned: Vec::new(),
        lossless,
    };
    let mut open: Vec<Element> = Vec::new();
    let mut roots = Vec::new();
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
                    None => roots.push(element),
                }
            }
            START_DOCUMENT => {}
            END_DOCUMENT => break,
            _ => {
                let node = match r.value(kind)? {
                    Value::Null => Node::Token(event, None),
                    Value::String(s) => Node::Token(event, Some(s)),
                    Value::Utf16(s) => Node::Utf16(event, s),
                    other => return Err(format!("ABX: token {event} with {other:?}")),
                };
                if let Some(parent) = open.last_mut() {
                    parent.content.push(node);
                }
            }
        }
    }
    if !open.is_empty() {
        return Err(format!("ABX: <{}> does not end", open[0].name));
    }
    Ok(roots)
}

/// `root` as one binary XML document, written as `BinaryXmlSerializer`
/// writes it: a document whose values keep their types is written back
/// in the pinned ArtFastDataOutput string encoding.
pub fn write(root: &Element) -> Result<Vec<u8>, String> {
    write_sequence(std::slice::from_ref(root))
}

/// Android TypedXmlSerializer also emits independent sibling root sections.
pub fn write_sequence(roots: &[Element]) -> Result<Vec<u8>, String> {
    let mut w = Writer::default();
    for root in roots {
        w.element(root)?;
    }
    Ok(w.finish())
}

/// Incremental TypedXmlSerializer, retaining one document's interned strings.
pub struct Writer {
    out: Vec<u8>,
    interned: HashMap<String, u16>,
    /// Interned strings that are not well-formed UTF-16; they never equal
    /// one in `interned`, and share its index sequence.
    interned_utf16: HashMap<JavaString, u16>,
}

impl Default for Writer {
    fn default() -> Self {
        let mut out = MAGIC.to_vec();
        out.push(START_DOCUMENT | TYPE_NULL);
        Self {
            out,
            interned: HashMap::new(),
            interned_utf16: HashMap::new(),
        }
    }
}

impl Writer {
    pub fn finish(mut self) -> Vec<u8> {
        self.out.push(END_DOCUMENT | TYPE_NULL);
        self.out
    }

    fn len(&mut self, n: usize) -> Result<(), String> {
        let n = u16::try_from(n).map_err(|_| format!("ABX: {n} bytes in one value"))?;
        self.out.extend(n.to_be_bytes());
        Ok(())
    }

    fn utf(&mut self, units: impl Iterator<Item = u16>) -> Result<(), String> {
        let bytes = encode_utf(units);
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
        self.utf(s.encode_utf16())?;
        let next = self.interned.len() + self.interned_utf16.len();
        if next < NEW as usize {
            self.interned.insert(s.to_owned(), next as u16);
        }
        Ok(())
    }

    fn interned_utf16(&mut self, s: &JavaString) -> Result<(), String> {
        if let Some(i) = self.interned_utf16.get(s) {
            self.out.extend(i.to_be_bytes());
            return Ok(());
        }
        self.out.extend(NEW.to_be_bytes());
        self.utf(s.units().iter().copied())?;
        let next = self.interned.len() + self.interned_utf16.len();
        if next < NEW as usize {
            self.interned_utf16.insert(s.clone(), next as u16);
        }
        Ok(())
    }

    pub fn start_element(&mut self, e: &Element) -> Result<(), String> {
        self.out.push(START_TAG | TYPE_STRING_INTERNED);
        self.interned(&e.name)?;
        for (name, value) in &e.attrs {
            self.attribute(name, value)?;
        }
        Ok(())
    }

    pub fn end_element(&mut self, name: &str) -> Result<(), String> {
        self.out.push(END_TAG | TYPE_STRING_INTERNED);
        self.interned(name)
    }

    pub fn element(&mut self, e: &Element) -> Result<(), String> {
        self.start_element(e)?;
        for node in &e.content {
            match node {
                Node::Element(child) => self.element(child)?,
                Node::Token(event, None) => self.out.push(event | TYPE_NULL),
                Node::Token(event, Some(text)) => {
                    self.out.push(event | TYPE_STRING);
                    self.utf(text.encode_utf16())?;
                }
                Node::Utf16(event, text) => {
                    self.out.push(event | TYPE_STRING);
                    self.utf(text.units().iter().copied())?;
                }
            }
        }
        self.end_element(&e.name)
    }

    fn attribute(&mut self, name: &str, value: &Value) -> Result<(), String> {
        let kind = match value {
            Value::Null => TYPE_NULL,
            Value::String(_) | Value::Utf16(_) => TYPE_STRING,
            Value::Interned(_) | Value::InternedUtf16(_) => TYPE_STRING_INTERNED,
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
            Value::String(s) => self.utf(s.encode_utf16())?,
            Value::Interned(s) => self.interned(s)?,
            Value::Utf16(s) => self.utf(s.units().iter().copied())?,
            Value::InternedUtf16(s) => self.interned_utf16(s)?,
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

/// `ArtFastDataInput.readUTF`: CharsetUtils.fromModifiedUtf8Bytes hands
/// the bytes, NUL-terminated, to JNI `NewStringUTF` (art
/// runtime/jni/jni_internal.cc, android-16.0.0_r1), which reads up to the
/// first zero byte, sizes each sequence by its lead byte alone without
/// checking continuation bytes, turns a four-byte sequence into a surrogate
/// pair, and ends the string with '?' when a sequence is cut short. Any
/// unit, an unpaired surrogate included, is kept.
pub fn decode_utf(b: &[u8]) -> Vec<u16> {
    let b = &b[..b.iter().position(|c| *c == 0).unwrap_or(b.len())];
    let mut units = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let lead = b[i];
        let n = match lead {
            _ if lead & 0x80 == 0 => 1,
            _ if lead & 0x20 == 0 => 2,
            _ if lead & 0x10 == 0 => 3,
            _ => 4,
        };
        let Some(s) = b.get(i..i + n) else {
            units.push(u16::from(b'?'));
            break;
        };
        let low = |j: usize| u16::from(s[j] & 0x3f);
        match n {
            1 => units.push(lead.into()),
            2 => units.push(u16::from(lead & 0x1f) << 6 | low(1)),
            3 => units.push(u16::from(lead & 0x0f) << 12 | low(1) << 6 | low(2)),
            _ => {
                let code_point = u32::from(lead & 0x0f) << 18
                    | u32::from(low(1)) << 12
                    | u32::from(low(2)) << 6
                    | u32::from(low(3));
                let bits = code_point.wrapping_sub(0x10000);
                units.push(0xd800 | (bits >> 10 & 0x3ff) as u16);
                units.push(0xdc00 | (bits & 0x3ff) as u16);
            }
        }
        i += n;
    }
    units
}

/// `ArtFastDataOutput.writeUTF`: JNI `GetStringUTFRegion`, ART's
/// `ConvertUtf16ToUtf8<kUtfUseShortZero = false, kUtfUse4ByteSequence =
/// true, kUtfReplaceBadSurrogates = false>`: NUL in two bytes, a surrogate
/// pair in four, any other unit (an unpaired surrogate too) in up to three.
pub fn encode_utf(units: impl Iterator<Item = u16>) -> Vec<u8> {
    let mut out = Vec::new();
    let mut units = units.peekable();
    while let Some(unit) = units.next() {
        let trail = units.next_if(|t| unit & 0xfc00 == 0xd800 && t & 0xfc00 == 0xdc00);
        match (unit, trail) {
            (1..=0x7f, _) => out.push(unit as u8),
            (0..=0x7ff, _) => out.extend([(unit >> 6) as u8 | 0xc0, (unit & 0x3f) as u8 | 0x80]),
            (_, Some(trail)) => {
                let c = 0x10000 + ((u32::from(unit) & 0x3ff) << 10 | u32::from(trail) & 0x3ff);
                out.extend([
                    (c >> 18) as u8 | 0xf0,
                    (c >> 12 & 0x3f) as u8 | 0x80,
                    (c >> 6 & 0x3f) as u8 | 0x80,
                    (c & 0x3f) as u8 | 0x80,
                ]);
            }
            _ => out.extend([
                (unit >> 12) as u8 | 0xe0,
                (unit >> 6 & 0x3f) as u8 | 0x80,
                (unit & 0x3f) as u8 | 0x80,
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
    fn sibling_roots_read_in_sequence() {
        let root = |name: &str| Element {
            name: name.into(),
            attrs: Vec::new(),
            content: Vec::new(),
        };
        let bytes = write_sequence(&[root("settings"), root("namespaceHashes")]).unwrap();
        let roots = read_sequence(&bytes).unwrap();
        assert_eq!(
            roots.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["settings", "namespaceHashes"]
        );
        assert_eq!(
            read_optional(&bytes).unwrap_err(),
            "ABX: a second root element"
        );
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

    fn units(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn strings_are_art_jni_utf8() {
        for s in ["", "a\u{0}b", "é", "한", "😀"] {
            let b = encode_utf(s.encode_utf16());
            assert_eq!(decode_utf(&b), units(s));
        }
        assert_eq!(encode_utf(units("\u{0}").into_iter()), [0xc0, 0x80]);
        assert_eq!(
            encode_utf(units("😀").into_iter()),
            [0xf0, 0x9f, 0x98, 0x80]
        );
        // Legacy six-byte surrogate pairs decode to the same units.
        assert_eq!(
            decode_utf(&[0xed, 0xa0, 0xbd, 0xed, 0xb8, 0x80]),
            units("😀")
        );
        // Unpaired surrogates, either order, are three bytes each way.
        for (lone, bytes) in [
            (vec![0xd83d], vec![0xed, 0xa0, 0xbd]),
            (vec![0xde00, 0x61], vec![0xed, 0xb8, 0x80, 0x61]),
            (
                vec![0xde00, 0xd83d],
                vec![0xed, 0xb8, 0x80, 0xed, 0xa0, 0xbd],
            ),
        ] {
            assert_eq!(encode_utf(lone.iter().copied()), bytes);
            assert_eq!(decode_utf(&bytes), lone);
        }
        // NewStringUTF reads to the first zero byte.
        assert_eq!(decode_utf(&[0x61, 0x00, 0x62]), [0x61]);
        // A sequence cut short ends the string with '?'.
        assert_eq!(decode_utf(&[0x61, 0xf0, 0x9f, 0x98]), [0x61, 0x3f]);
        assert_eq!(decode_utf(&[0xc3]), [0x3f]);
        // Lead bytes alone size a sequence; continuation bits are unchecked
        // and four-byte code points below U+10000 still become two units.
        assert_eq!(decode_utf(&[0xc1, 0x41]), [0x41]);
        assert_eq!(decode_utf(&[0x80, 0x80]), [0]);
        assert_eq!(decode_utf(&[0xf0, 0x80, 0x80, 0x80]), [0xdbc0, 0xdc00]);
        assert_eq!(decode_utf(&[0xf4, 0x90, 0x80, 0x80]), [0xd800, 0xdc00]);
    }

    #[test]
    fn unpaired_surrogates_are_explicit_or_lossless() {
        let mut bytes = MAGIC.to_vec();
        bytes.push(START_DOCUMENT | TYPE_NULL);
        bytes.push(START_TAG | TYPE_STRING_INTERNED);
        new(&mut bytes, "string");
        bytes.push(ATTRIBUTE | TYPE_STRING);
        new(&mut bytes, "name");
        bytes.extend(3u16.to_be_bytes());
        bytes.extend([0xed, 0xa0, 0xbd]);
        bytes.push(ATTRIBUTE | TYPE_STRING_INTERNED);
        new(&mut bytes, "kind");
        bytes.extend(NEW.to_be_bytes());
        bytes.extend(4u16.to_be_bytes());
        bytes.extend([0xed, 0xb8, 0x80, 0x61]);
        bytes.push(crate::TEXT | TYPE_STRING);
        bytes.extend(6u16.to_be_bytes());
        bytes.extend([0x61, 0xc0, 0x80, 0xed, 0xa0, 0xbd]);
        bytes.push(END_TAG | TYPE_STRING_INTERNED);
        bytes.extend(0u16.to_be_bytes());
        bytes.push(END_DOCUMENT | TYPE_NULL);

        let error = read(&bytes).unwrap_err();
        assert!(
            error.contains("0xd83d") && error.contains("#843"),
            "{error}"
        );
        let root = crate::read_next_lossless(&bytes).unwrap();
        let lone = |u: &[u16]| JavaString::from_units(u.to_vec());
        assert_eq!(root.attr("name"), Some(&Value::Utf16(lone(&[0xd83d]))));
        assert_eq!(root.string("name"), None);
        assert_eq!(root.java_string("name"), Some(lone(&[0xd83d])));
        assert_eq!(
            root.attr("kind"),
            Some(&Value::InternedUtf16(lone(&[0xde00, 0x61])))
        );
        assert_eq!(
            root.content,
            [Node::Utf16(crate::TEXT, lone(&[0x61, 0, 0xd83d]))]
        );
        // The interned value shares the name table's indices.
        assert_eq!(write(&root).unwrap(), bytes);
    }
}

pub(crate) struct Pull<'a> {
    reader: Reader<'a>,
}

impl<'a> Pull<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Result<Self, String> {
        let mut reader = Reader {
            bytes,
            at: MAGIC.len(),
            interned: Vec::new(),
            lossless: false,
        };
        // setInput peeks the first token, so magic alone is an input error.
        let token = *bytes.get(reader.at).ok_or("ABX: truncated header")?;
        if token & 0x0f == START_DOCUMENT {
            reader.at += 1;
        }
        Ok(Self { reader })
    }

    pub(crate) fn peek(&self) -> Result<u8, String> {
        self.reader
            .bytes
            .get(self.reader.at)
            .map(|b| b & 0x0f)
            .ok_or_else(|| format!("ABX: truncated at {}", self.reader.at))
    }

    pub(crate) fn token(&mut self) -> Result<crate::pull::Token, String> {
        use crate::pull::Token;
        let r = &mut self.reader;
        // nextToken treats EOF during external token decoding as END_DOCUMENT.
        let result = (|| {
            let [token] = r.take()?;
            let event = token & 0x0f;
            Ok::<_, String>(match event {
                START_TAG => Token::Start(Element {
                    name: r.interned()?,
                    attrs: Vec::new(),
                    content: Vec::new(),
                }),
                END_TAG => Token::End(r.interned()?),
                END_DOCUMENT => Token::EndDocument,
                START_DOCUMENT => Token::Content(START_DOCUMENT, None),
                crate::ENTITY_REF => {
                    let name = r.utf()?;
                    let text = crate::pull::entity(&name)?
                        .to_rust()
                        .map_err(|e| format!("ABX: entity {name}: {e}"))?;
                    Token::Content(crate::CDSECT, Some(text))
                }
                crate::TEXT
                | crate::CDSECT
                | crate::COMMENT
                | crate::PROCESSING_INSTRUCTION
                | crate::DOCDECL
                | crate::IGNORABLE_WHITESPACE => Token::Content(event, Some(r.utf()?)),
                _ => return Err(format!("ABX: unexpected token {event}")),
            })
        })();
        let mut token = match result {
            Err(message) if message.starts_with("ABX: truncated at ") => {
                return Ok(Token::EndDocument);
            }
            other => other?,
        };
        if let Token::Start(element) = &mut token {
            loop {
                let byte = *r
                    .bytes
                    .get(r.at)
                    .ok_or_else(|| format!("ABX: truncated at {}", r.at))?;
                if byte & 0x0f != ATTRIBUTE {
                    break;
                }
                r.at += 1;
                let name = r.interned()?;
                let value = r.value(byte & 0xf0)?;
                element.attrs.push((name, value));
            }
        }
        Ok(token)
    }
}
