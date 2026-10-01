//! Android's compiled resources: string pools, typed values and binary XML
//! (`ResourceTypes.h` of libandroidfw), as aapt2 writes them into an APK.

use std::fmt;

#[derive(Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub fn bad(what: impl Into<String>) -> Error {
    Error(what.into())
}

// ResChunk_header types.
pub const STRING_POOL: u16 = 0x0001;
pub const TABLE: u16 = 0x0002;
pub const XML: u16 = 0x0003;
pub const XML_START_ELEMENT: u16 = 0x0102;
pub const XML_END_ELEMENT: u16 = 0x0103;
pub const XML_RESOURCE_MAP: u16 = 0x0180;
pub const TABLE_PACKAGE: u16 = 0x0200;
pub const TABLE_TYPE: u16 = 0x0201;
pub const TABLE_LIBRARY: u16 = 0x0203;

pub fn u16_at(b: &[u8], at: usize) -> Result<u16> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| bad("truncated resource data"))
}

pub fn u32_at(b: &[u8], at: usize) -> Result<u32> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| bad("truncated resource data"))
}

/// A chunk: its type, header size and the whole chunk's bytes.
pub struct Chunk<'a> {
    pub kind: u16,
    pub header: usize,
    pub data: &'a [u8],
}

/// The chunks laid end to end in `b`.
pub fn chunks(mut b: &[u8]) -> impl Iterator<Item = Result<Chunk<'_>>> {
    std::iter::from_fn(move || {
        if b.len() < 8 {
            return None;
        }
        let r = (|| {
            let kind = u16_at(b, 0)?;
            let header = u16_at(b, 2)? as usize;
            let size = u32_at(b, 4)? as usize;
            if size < 8 || size > b.len() || header > size {
                return Err(bad("bad resource chunk size"));
            }
            let c = Chunk {
                kind,
                header,
                data: &b[..size],
            };
            b = &b[size..];
            Ok(c)
        })();
        if r.is_err() {
            b = &[];
        }
        Some(r)
    })
}

/// A `ResStringPool`.
pub struct Strings(Vec<String>);

impl Strings {
    pub fn parse(c: &[u8]) -> Result<Strings> {
        let count = u32_at(c, 8)? as usize;
        let flags = u32_at(c, 16)?;
        let start = u32_at(c, 20)? as usize;
        let header = u16_at(c, 2)? as usize;
        let utf8 = flags & (1 << 8) != 0;
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let at = start + u32_at(c, header + 4 * i)? as usize;
            out.push(if utf8 {
                utf8_at(c, at)?
            } else {
                utf16_at(c, at)?
            });
        }
        Ok(Strings(out))
    }

    pub fn get(&self, i: u32) -> Option<&str> {
        self.0.get(i as usize).map(String::as_str)
    }
}

/// A length of one or two units with the high bit as the extension.
fn len8(c: &[u8], at: &mut usize) -> Result<usize> {
    let a = *c.get(*at).ok_or_else(|| bad("truncated string"))? as usize;
    *at += 1;
    if a & 0x80 == 0 {
        return Ok(a);
    }
    let b = *c.get(*at).ok_or_else(|| bad("truncated string"))? as usize;
    *at += 1;
    Ok((a & 0x7f) << 8 | b)
}

fn utf8_at(c: &[u8], mut at: usize) -> Result<String> {
    len8(c, &mut at)?; // UTF-16 length
    let n = len8(c, &mut at)?;
    let b = c.get(at..at + n).ok_or_else(|| bad("truncated string"))?;
    Ok(String::from_utf8_lossy(b).into_owned())
}

fn utf16_at(c: &[u8], mut at: usize) -> Result<String> {
    let mut n = u16_at(c, at)? as usize;
    at += 2;
    if n & 0x8000 != 0 {
        n = (n & 0x7fff) << 16 | u16_at(c, at)? as usize;
        at += 2;
    }
    let units = (0..n)
        .map(|i| u16_at(c, at + 2 * i))
        .collect::<Result<Vec<u16>>>()?;
    Ok(String::from_utf16_lossy(&units))
}

/// A `Res_value`.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    /// A resource (`@...`).
    Ref(u32),
    /// A theme attribute (`?...`).
    Attr(u32),
    String(String),
    Float(f32),
    /// Complex unit data, as `TypedValue.complexToFloat` reads it.
    Dimension(u32),
    Fraction(u32),
    Int(i32),
    Bool(bool),
    /// ARGB.
    Color(u32),
}

impl Value {
    /// Decode `Res_value` type `kind`, data `data`; strings index `pool`.
    pub fn decode(kind: u8, data: u32, pool: &Strings) -> Value {
        match kind {
            0x01 | 0x07 => Value::Ref(data),
            0x02 | 0x08 => Value::Attr(data),
            0x03 => Value::String(pool.get(data).unwrap_or_default().to_owned()),
            0x04 => Value::Float(f32::from_bits(data)),
            0x05 => Value::Dimension(data),
            0x06 => Value::Fraction(data),
            0x10 | 0x11 => Value::Int(data as i32),
            0x12 => Value::Bool(data != 0),
            0x1c | 0x1d => Value::Color(if kind == 0x1d {
                data | 0xff00_0000
            } else {
                data
            }),
            0x1e | 0x1f => {
                // #argb / #rgb nibbles, as aapt2 keeps them expanded.
                Value::Color(if kind == 0x1f {
                    data | 0xff00_0000
                } else {
                    data
                })
            }
            _ => Value::Null,
        }
    }

    /// A dimension or fraction's number (`complexToFloat`).
    pub fn complex(data: u32) -> f32 {
        const RADIX_MULTS: [f32; 4] = [
            1.0 / (1 << 8) as f32,
            1.0 / (1 << 15) as f32,
            1.0 / (1 << 23) as f32,
            1.0 / (1u64 << 31) as f32,
        ];
        let mantissa = (data & 0xffff_ff00) as i32 as f32;
        mantissa * RADIX_MULTS[((data >> 4) & 3) as usize]
    }

    /// A dimension in dp (dp, dip and px as one: icons are drawn in
    /// density-independent units) or a fraction of `base`.
    pub fn length(&self, base: f32) -> Option<f32> {
        match *self {
            Value::Dimension(d) => Some(Value::complex(d)),
            Value::Fraction(d) => Some(Value::complex(d) * base),
            Value::Float(f) => Some(f),
            Value::Int(i) => Some(i as f32),
            _ => None,
        }
    }
}

/// An element of a binary XML document.
#[derive(Clone, Debug, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<Attr>,
    pub children: Vec<Element>,
}

#[derive(Clone, Debug)]
pub struct Attr {
    /// The namespace's URI, empty for none.
    pub ns: String,
    pub name: String,
    /// The attribute's resource id (`android:*` attributes), or 0.
    pub id: u32,
    pub value: Value,
    /// The `Res_value` as stored: its type and data.
    pub kind: u8,
    pub data: u32,
}

impl Element {
    /// Attribute resource id `id`'s value.
    pub fn attr(&self, id: u32) -> Option<&Value> {
        self.attrs.iter().find(|a| a.id == id).map(|a| &a.value)
    }

    /// An attribute by name, for the few without a resource id.
    pub fn named(&self, name: &str) -> Option<&Value> {
        self.attrs.iter().find(|a| a.name == name).map(|a| &a.value)
    }
}

/// Parse a binary XML document (`ResXMLTree`): its root element.
pub fn xml(data: &[u8]) -> Result<Element> {
    let top = chunks(data).next().ok_or_else(|| bad("empty XML"))??;
    if top.kind != XML {
        return Err(bad("not binary XML"));
    }
    let mut pool = None;
    let mut ids: Vec<u32> = Vec::new();
    let mut stack: Vec<Element> = vec![Element::default()];
    for c in chunks(&top.data[top.header..]) {
        let c = c?;
        match c.kind {
            STRING_POOL => pool = Some(Strings::parse(c.data)?),
            XML_RESOURCE_MAP => {
                ids = (8..c.data.len())
                    .step_by(4)
                    .map(|at| u32_at(c.data, at))
                    .collect::<Result<_>>()?;
            }
            XML_START_ELEMENT => {
                let pool = pool.as_ref().ok_or_else(|| bad("XML without strings"))?;
                let ext = c.header;
                let name = pool
                    .get(u32_at(c.data, ext + 4)?)
                    .unwrap_or_default()
                    .to_owned();
                let start = u16_at(c.data, ext + 8)? as usize;
                let size = u16_at(c.data, ext + 10)? as usize;
                let count = u16_at(c.data, ext + 12)? as usize;
                let mut attrs = Vec::with_capacity(count);
                for i in 0..count {
                    let at = ext + start + i * size;
                    let ns = u32_at(c.data, at)?;
                    let n = u32_at(c.data, at + 4)?;
                    let raw = u32_at(c.data, at + 8)?;
                    let kind = *c
                        .data
                        .get(at + 15)
                        .ok_or_else(|| bad("truncated attribute"))?;
                    let data = u32_at(c.data, at + 16)?;
                    let value = if kind == 0x03 && raw != u32::MAX {
                        Value::String(pool.get(raw).unwrap_or_default().to_owned())
                    } else {
                        Value::decode(kind, data, pool)
                    };
                    attrs.push(Attr {
                        ns: pool.get(ns).unwrap_or_default().to_owned(),
                        name: pool.get(n).unwrap_or_default().to_owned(),
                        id: ids.get(n as usize).copied().unwrap_or(0),
                        value,
                        kind,
                        data,
                    });
                }
                stack.push(Element {
                    name,
                    attrs,
                    children: Vec::new(),
                });
            }
            XML_END_ELEMENT => {
                let e = stack.pop().ok_or_else(|| bad("unbalanced XML"))?;
                stack
                    .last_mut()
                    .ok_or_else(|| bad("unbalanced XML"))?
                    .children
                    .push(e);
            }
            _ => {}
        }
    }
    stack
        .pop()
        .and_then(|mut doc| doc.children.pop())
        .ok_or_else(|| bad("XML without an element"))
}
