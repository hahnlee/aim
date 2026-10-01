//! Reading dex files (`dex_file.h`, the Dalvik executable format) and the
//! one kind of edit the derived image makes to them in place: turning
//! instructions into `nop`s, which moves nothing else ([`system_server`]).
//! [`reindex`] writes a dex again with method ids added.
//!
//! [`system_server`]: crate::system_server
//! [`reindex`]: crate::reindex

use sha1::{Digest, Sha1};

pub type Result<T> = std::result::Result<T, String>;

/// `dex::kDexNoIndex`.
const NO_INDEX: u32 = u32::MAX;

/// An `encoded_value` of a class's static values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Int(i64),
    String(String),
    Null,
    Bool(bool),
    /// Floating point, types, methods, arrays, annotations.
    Other,
}

pub struct ClassDef {
    pub descriptor: String,
    /// `access_flags`.
    pub access: u32,
    pub superclass: Option<String>,
    interfaces_off: u32,
    class_data_off: u32,
    static_values_off: u32,
}

/// One method's code: its instructions' offset and length in code units.
pub struct Code {
    pub insns_off: usize,
    pub insns_units: usize,
}

pub struct Dex<'a> {
    data: &'a [u8],
    string_ids: usize,
    strings: usize,
    type_ids: usize,
    types: usize,
    field_ids: usize,
    method_ids: usize,
    proto_ids: usize,
    pub classes: Vec<ClassDef>,
}

fn u16_at(d: &[u8], at: usize) -> Result<u16> {
    d.get(at..at + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| format!("dex: read past the end at {at:#x}"))
}

fn u32_at(d: &[u8], at: usize) -> Result<u32> {
    d.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| format!("dex: read past the end at {at:#x}"))
}

fn uleb128(d: &[u8], at: &mut usize) -> Result<u32> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = *d.get(*at).ok_or("dex: truncated uleb128")?;
        *at += 1;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("dex: overlong uleb128".into())
}

impl<'a> Dex<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Self> {
        let version = data.get(4..7).ok_or("dex: no header")?;
        if &data[..4] != b"dex\n" || !(b"035"..=b"040").contains(&version.try_into().unwrap()) {
            return Err("dex: not a dex file of version 035 to 040".into());
        }
        let count = |at| u32_at(data, at).map(|v| v as usize);
        let mut dex = Dex {
            data,
            string_ids: count(0x3c)?,
            strings: count(0x38)?,
            type_ids: count(0x44)?,
            types: count(0x40)?,
            proto_ids: count(0x4c)?,
            field_ids: count(0x54)?,
            method_ids: count(0x5c)?,
            classes: Vec::new(),
        };
        let (class_defs, class_count) = (count(0x64)?, count(0x60)?);
        for i in 0..class_count {
            let at = class_defs + i * 32;
            let superclass = u32_at(data, at + 8)?;
            dex.classes.push(ClassDef {
                descriptor: dex.type_name(u32_at(data, at)?)?,
                access: u32_at(data, at + 4)?,
                superclass: match superclass {
                    NO_INDEX => None,
                    index => Some(dex.type_name(index)?),
                },
                interfaces_off: u32_at(data, at + 12)?,
                class_data_off: u32_at(data, at + 24)?,
                static_values_off: u32_at(data, at + 28)?,
            });
        }
        Ok(dex)
    }

    /// A string by index (MUTF-8; the descriptors and names read here are
    /// plain UTF-8).
    pub fn string(&self, index: u32) -> Result<String> {
        if index as usize >= self.strings {
            return Err(format!("dex: string {index} out of range"));
        }
        let mut at = u32_at(self.data, self.string_ids + 4 * index as usize)? as usize;
        uleb128(self.data, &mut at)?;
        let end = self.data[at..]
            .iter()
            .position(|b| *b == 0)
            .ok_or("dex: unterminated string")?;
        Ok(String::from_utf8_lossy(&self.data[at..at + end]).into_owned())
    }

    pub fn type_name(&self, index: u32) -> Result<String> {
        if index as usize >= self.types {
            return Err(format!("dex: type {index} out of range"));
        }
        self.string(u32_at(self.data, self.type_ids + 4 * index as usize)?)
    }

    /// The index of the type named `descriptor`.
    pub fn type_index(&self, descriptor: &str) -> Result<Option<u32>> {
        for i in 0..self.types as u32 {
            if self.type_name(i)? == descriptor {
                return Ok(Some(i));
            }
        }
        Ok(None)
    }

    /// A method id: (class descriptor, name, shorty-free signature
    /// `(params)return`).
    pub fn method(&self, index: u32) -> Result<(String, String, String)> {
        let at = self.method_ids + 8 * index as usize;
        let class = self.type_name(u32::from(u16_at(self.data, at)?))?;
        let proto = self.proto_ids + 12 * u16_at(self.data, at + 2)? as usize;
        let name = self.string(u32_at(self.data, at + 4)?)?;
        let ret = self.type_name(u32_at(self.data, proto + 4)?)?;
        let params_off = u32_at(self.data, proto + 8)? as usize;
        let mut sig = String::from("(");
        if params_off != 0 {
            for i in 0..u32_at(self.data, params_off)? as usize {
                sig += &self.type_name(u32::from(u16_at(self.data, params_off + 4 + 2 * i)?))?;
            }
        }
        sig += ")";
        sig += &ret;
        Ok((class, name, sig))
    }

    fn field_name(&self, index: u32) -> Result<String> {
        self.string(u32_at(self.data, self.field_ids + 8 * index as usize + 4)?)
    }

    /// A field id: (class descriptor, name, type descriptor).
    pub fn field(&self, index: u32) -> Result<(String, String, String)> {
        let at = self.field_ids + 8 * index as usize;
        Ok((
            self.type_name(u32::from(u16_at(self.data, at)?))?,
            self.field_name(index)?,
            self.type_name(u32::from(u16_at(self.data, at + 2)?))?,
        ))
    }

    /// The number of field and method ids: every field and method the
    /// dex defines or refers to.
    pub fn ids(&self) -> Result<(u32, u32)> {
        Ok((u32_at(self.data, 0x50)?, u32_at(self.data, 0x58)?))
    }

    /// The interfaces `class` implements directly.
    pub fn interfaces(&self, class: &ClassDef) -> Result<Vec<String>> {
        if class.interfaces_off == 0 {
            return Ok(Vec::new());
        }
        let at = class.interfaces_off as usize;
        (0..u32_at(self.data, at)? as usize)
            .map(|i| self.type_name(u32::from(u16_at(self.data, at + 4 + 2 * i)?)))
            .collect()
    }

    /// The field and method ids `class` declares.
    pub fn members(&self, class: &ClassDef) -> Result<(Vec<u32>, Vec<u32>)> {
        let (mut fields, mut methods) = (Vec::new(), Vec::new());
        if class.class_data_off == 0 {
            return Ok((fields, methods));
        }
        let mut at = class.class_data_off as usize;
        let counts: Vec<u32> = (0..4)
            .map(|_| uleb128(self.data, &mut at))
            .collect::<Result<_>>()?;
        for (i, count) in counts.into_iter().enumerate() {
            let mut index = 0u32;
            for _ in 0..count {
                index += uleb128(self.data, &mut at)?;
                uleb128(self.data, &mut at)?;
                if i < 2 {
                    fields.push(index);
                } else {
                    uleb128(self.data, &mut at)?;
                    methods.push(index);
                }
            }
        }
        Ok((fields, methods))
    }

    /// The direct methods `class` declares (static, private and
    /// constructors): their method ids and access flags.
    pub fn direct_methods(&self, class: &ClassDef) -> Result<Vec<(u32, u32)>> {
        if class.class_data_off == 0 {
            return Ok(Vec::new());
        }
        let mut at = class.class_data_off as usize;
        let counts: Vec<u32> = (0..3)
            .map(|_| uleb128(self.data, &mut at))
            .collect::<Result<_>>()?;
        uleb128(self.data, &mut at)?;
        for _ in 0..counts[0] + counts[1] {
            uleb128(self.data, &mut at)?;
            uleb128(self.data, &mut at)?;
        }
        let mut out = Vec::new();
        let mut index = 0u32;
        for _ in 0..counts[2] {
            index += uleb128(self.data, &mut at)?;
            out.push((index, uleb128(self.data, &mut at)?));
            uleb128(self.data, &mut at)?;
        }
        Ok(out)
    }

    pub fn class(&self, descriptor: &str) -> Option<&ClassDef> {
        self.classes.iter().find(|c| c.descriptor == descriptor)
    }

    /// The static fields of `class` that have an initial value, in field
    /// order, with that value.
    pub fn static_values(&self, class: &ClassDef) -> Result<Vec<(String, Value)>> {
        if class.class_data_off == 0 || class.static_values_off == 0 {
            return Ok(Vec::new());
        }
        let mut at = class.class_data_off as usize;
        let statics = uleb128(self.data, &mut at)?;
        for _ in 0..3 {
            uleb128(self.data, &mut at)?;
        }
        let mut names = Vec::new();
        let mut field = 0u32;
        for _ in 0..statics {
            field += uleb128(self.data, &mut at)?;
            uleb128(self.data, &mut at)?;
            names.push(self.field_name(field)?);
        }
        let mut at = class.static_values_off as usize;
        let count = uleb128(self.data, &mut at)? as usize;
        let mut out = Vec::new();
        for name in names.into_iter().take(count) {
            out.push((name, self.encoded_value(&mut at)?));
        }
        Ok(out)
    }

    fn encoded_value(&self, at: &mut usize) -> Result<Value> {
        let head = *self.data.get(*at).ok_or("dex: truncated value")?;
        *at += 1;
        let (kind, arg) = (head & 0x1f, usize::from(head >> 5));
        let mut bytes = |signed: bool| -> Result<i64> {
            let raw = self
                .data
                .get(*at..*at + arg + 1)
                .ok_or("dex: truncated value")?;
            *at += arg + 1;
            let mut v = 0u64;
            for (i, b) in raw.iter().enumerate() {
                v |= u64::from(*b) << (8 * i);
            }
            let shift = 64 - 8 * (arg + 1);
            Ok(if signed {
                ((v << shift) as i64) >> shift
            } else {
                v as i64
            })
        };
        Ok(match kind {
            0x00 | 0x02 | 0x04 | 0x06 => Value::Int(bytes(true)?),
            0x03 => Value::Int(bytes(false)?),
            0x17 => Value::String(self.string(bytes(false)? as u32)?),
            0x10 | 0x11 | 0x15 | 0x16 | 0x18 | 0x19 | 0x1a | 0x1b => {
                bytes(false)?;
                Value::Other
            }
            0x1c => {
                for _ in 0..uleb128(self.data, at)? {
                    self.encoded_value(at)?;
                }
                Value::Other
            }
            0x1d => {
                uleb128(self.data, at)?;
                for _ in 0..uleb128(self.data, at)? {
                    uleb128(self.data, at)?;
                    self.encoded_value(at)?;
                }
                Value::Other
            }
            0x1e => Value::Null,
            0x1f => Value::Bool(arg != 0),
            _ => return Err(format!("dex: unknown value type {kind:#x}")),
        })
    }

    /// The code of every method of `class` named `name`.
    pub fn methods_named(&self, class: &ClassDef, name: &str) -> Result<Vec<Code>> {
        if class.class_data_off == 0 {
            return Ok(Vec::new());
        }
        let mut at = class.class_data_off as usize;
        let (statics, instance) = (uleb128(self.data, &mut at)?, uleb128(self.data, &mut at)?);
        let (direct, virtuals) = (uleb128(self.data, &mut at)?, uleb128(self.data, &mut at)?);
        for _ in 0..statics + instance {
            uleb128(self.data, &mut at)?;
            uleb128(self.data, &mut at)?;
        }
        let mut out = Vec::new();
        for count in [direct, virtuals] {
            let mut method = 0u32;
            for _ in 0..count {
                method += uleb128(self.data, &mut at)?;
                uleb128(self.data, &mut at)?;
                let code_off = uleb128(self.data, &mut at)? as usize;
                if code_off != 0 && self.method(method)?.1 == name {
                    out.push(Code {
                        insns_off: code_off + 16,
                        insns_units: u32_at(self.data, code_off + 12)? as usize,
                    });
                }
            }
        }
        Ok(out)
    }
}

/// An instruction's length in code units (the formats of the Dalvik
/// bytecode reference), payloads included.
pub fn instruction_units(insns: &[u16], at: usize) -> Result<usize> {
    let unit = insns[at];
    let units = match (unit & 0xff) as u8 {
        0x00 => match unit {
            0x0100 => 4 + 2 * usize::from(*insns.get(at + 1).ok_or("dex: truncated payload")?),
            0x0200 => 2 + 4 * usize::from(*insns.get(at + 1).ok_or("dex: truncated payload")?),
            0x0300 => {
                let width = usize::from(*insns.get(at + 1).ok_or("dex: truncated payload")?);
                let lo = usize::from(*insns.get(at + 2).ok_or("dex: truncated payload")?);
                let hi = usize::from(*insns.get(at + 3).ok_or("dex: truncated payload")?);
                4 + ((lo | hi << 16) * width).div_ceil(2)
            }
            _ => 1,
        },
        0x01 | 0x04 | 0x07 | 0x0a..=0x12 | 0x1d | 0x1e | 0x21 | 0x27 | 0x28 => 1,
        0x3e..=0x43 | 0x73 | 0x79 | 0x7a | 0x7b..=0x8f | 0xb0..=0xcf | 0xe3..=0xf9 => 1,
        0x02 | 0x05 | 0x08 | 0x13 | 0x15 | 0x16 | 0x19 | 0x1a | 0x1c | 0x1f | 0x20 => 2,
        0x22 | 0x23 | 0x29 | 0x2d..=0x3d | 0x44..=0x6d | 0x90..=0xaf | 0xd0..=0xe2 => 2,
        0xfe | 0xff => 2,
        0x03 | 0x06 | 0x09 | 0x14 | 0x17 | 0x1b | 0x24..=0x26 | 0x2a..=0x2c => 3,
        0x6e..=0x72 | 0x74..=0x78 | 0xfc | 0xfd => 3,
        0xfa | 0xfb => 4,
        0x18 => 5,
    };
    Ok(units)
}

/// Recomputes the header's SHA-1 signature and Adler-32 checksum after an
/// edit.
pub fn fix_checksums(data: &mut [u8]) {
    let signature = Sha1::digest(&data[32..]);
    data[12..32].copy_from_slice(&signature);
    let mut adler = adler2::Adler32::new();
    adler.write_slice(&data[12..]);
    data[8..12].copy_from_slice(&adler.checksum().to_le_bytes());
}

/// Reads a dex's code units.
pub fn units(data: &[u8], code: &Code) -> Result<Vec<u16>> {
    let bytes = data
        .get(code.insns_off..code.insns_off + 2 * code.insns_units)
        .ok_or("dex: code past the end")?;
    Ok(bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect())
}
