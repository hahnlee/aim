#![forbid(unsafe_code)]
//! The native methods an Android jar declares, read from its DEX files.
//!
//! This is the inventory every JNI owner is accounted against: each entry is
//! a method with `ACC_NATIVE`, named by its class descriptor, method name and
//! descriptor signature, in the form `RegisterNatives` and the ART
//! "No implementation found" log use.

use std::fmt;

/// `ACC_NATIVE` (dex-format access_flags).
const ACC_NATIVE: u64 = 0x100;

/// One native method.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NativeMethod {
    /// Class descriptor, e.g. `Landroid/os/Debug;`.
    pub class: String,
    pub name: String,
    /// Method descriptor, e.g. `([J)V`.
    pub signature: String,
}

impl fmt::Display for NativeMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}\t{}\t{}",
            self.class, self.name, self.signature
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct DexError(pub String);

impl fmt::Display for DexError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "malformed dex: {}", self.0)
    }
}

impl std::error::Error for DexError {}

fn error(message: impl Into<String>) -> DexError {
    DexError(message.into())
}

struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    fn u16(&self, offset: usize) -> Result<u16, DexError> {
        self.bytes
            .get(offset..offset + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or_else(|| error(format!("u16 at {offset:#x} is out of range")))
    }

    fn u32(&self, offset: usize) -> Result<u32, DexError> {
        self.bytes
            .get(offset..offset + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| error(format!("u32 at {offset:#x} is out of range")))
    }

    /// uleb128 at `*offset`, advancing it.
    fn uleb128(&self, offset: &mut usize) -> Result<u64, DexError> {
        let mut value = 0u64;
        for shift in (0..35).step_by(7) {
            let byte = *self
                .bytes
                .get(*offset)
                .ok_or_else(|| error("uleb128 is out of range"))?;
            *offset += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(error("uleb128 is longer than five bytes"))
    }

    /// A table entry's offset, checked against the table's size.
    fn entry(&self, table: (u32, u32), index: u32, width: usize) -> Result<usize, DexError> {
        let (size, offset) = table;
        if index >= size {
            return Err(error(format!("index {index} is outside a table of {size}")));
        }
        Ok(offset as usize + index as usize * width)
    }
}

/// A parsed DEX file's id tables.
struct Dex<'a> {
    reader: Reader<'a>,
    strings: (u32, u32),
    types: (u32, u32),
    protos: (u32, u32),
    methods: (u32, u32),
    classes: (u32, u32),
}

impl<'a> Dex<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, DexError> {
        if bytes.len() < 0x70 || &bytes[..4] != b"dex\n" {
            return Err(error("missing dex magic"));
        }
        let reader = Reader { bytes };
        let table = |offset| Ok::<_, DexError>((reader.u32(offset)?, reader.u32(offset + 4)?));
        Ok(Self {
            strings: table(0x38)?,
            types: table(0x40)?,
            protos: table(0x48)?,
            methods: table(0x58)?,
            classes: table(0x60)?,
            reader,
        })
    }

    /// string_ids[index] as UTF-8 (the MUTF-8 of descriptors and names).
    fn string(&self, index: u32) -> Result<String, DexError> {
        let mut offset = self
            .reader
            .u32(self.reader.entry(self.strings, index, 4)?)? as usize;
        self.reader.uleb128(&mut offset)?;
        let tail = self
            .reader
            .bytes
            .get(offset..)
            .ok_or_else(|| error("string data is out of range"))?;
        let end = tail
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| error("unterminated string"))?;
        Ok(String::from_utf8_lossy(&tail[..end]).into_owned())
    }

    fn type_name(&self, index: u32) -> Result<String, DexError> {
        self.string(self.reader.u32(self.reader.entry(self.types, index, 4)?)?)
    }

    /// proto_ids[index] as a method descriptor.
    fn prototype(&self, index: u32) -> Result<String, DexError> {
        let entry = self.reader.entry(self.protos, index, 12)?;
        let return_type = self.type_name(self.reader.u32(entry + 4)?)?;
        let parameters = self.reader.u32(entry + 8)? as usize;
        let mut descriptor = String::from("(");
        if parameters != 0 {
            let count = self.reader.u32(parameters)?;
            for position in 0..count as usize {
                let type_index = self.reader.u16(parameters + 4 + position * 2)?;
                descriptor.push_str(&self.type_name(type_index.into())?);
            }
        }
        descriptor.push(')');
        descriptor.push_str(&return_type);
        Ok(descriptor)
    }

    fn method(&self, index: u32) -> Result<NativeMethod, DexError> {
        let entry = self.reader.entry(self.methods, index, 8)?;
        Ok(NativeMethod {
            class: self.type_name(self.reader.u16(entry)?.into())?,
            signature: self.prototype(self.reader.u16(entry + 2)?.into())?,
            name: self.string(self.reader.u32(entry + 4)?)?,
        })
    }

    fn natives(&self) -> Result<Vec<NativeMethod>, DexError> {
        let mut natives = Vec::new();
        for class in 0..self.classes.0 {
            let entry = self.reader.entry(self.classes, class, 32)?;
            let data = self.reader.u32(entry + 24)? as usize;
            if data == 0 {
                continue;
            }
            let mut offset = data;
            let static_fields = self.reader.uleb128(&mut offset)?;
            let instance_fields = self.reader.uleb128(&mut offset)?;
            let direct_methods = self.reader.uleb128(&mut offset)?;
            let virtual_methods = self.reader.uleb128(&mut offset)?;
            for _ in 0..(static_fields + instance_fields) * 2 {
                self.reader.uleb128(&mut offset)?;
            }
            for count in [direct_methods, virtual_methods] {
                // Method indices are delta-encoded within each list.
                let mut method = 0u64;
                for _ in 0..count {
                    method += self.reader.uleb128(&mut offset)?;
                    let access = self.reader.uleb128(&mut offset)?;
                    self.reader.uleb128(&mut offset)?; // code_off
                    if access & ACC_NATIVE != 0 {
                        let index = u32::try_from(method).map_err(|_| error("method index"))?;
                        natives.push(self.method(index)?);
                    }
                }
            }
        }
        Ok(natives)
    }
}

/// The checked-in inventory of the pinned Android 16 jars
/// (`art-bootstrap native-inventory`): `JAR\tCLASS\tMETHOD\tSIGNATURE` lines.
pub const ANDROID16_NATIVES: &str = include_str!("../generated/android16-natives.tsv");

/// One inventory entry and the jar that declares it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryEntry<'a> {
    pub jar: &'a str,
    pub class: &'a str,
    pub name: &'a str,
    pub signature: &'a str,
}

/// The entries of an inventory text, skipping comments.
pub fn inventory(text: &str) -> impl Iterator<Item = InventoryEntry<'_>> {
    text.lines()
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .filter_map(|line| {
            let mut fields = line.split('\t');
            Some(InventoryEntry {
                jar: fields.next()?,
                class: fields.next()?,
                name: fields.next()?,
                signature: fields.next()?,
            })
        })
}

/// Every native method declared in one DEX file.
pub fn dex_natives(bytes: &[u8]) -> Result<Vec<NativeMethod>, DexError> {
    Dex::parse(bytes)?.natives()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal dex: class `LA;` with native `static void f(int)` and a
    /// non-native method `g()V`, laid out at fixed offsets.
    fn fixture() -> Vec<u8> {
        let mut dex = vec![0u8; 0x200];
        dex[..8].copy_from_slice(b"dex\n035\0");
        let put = |dex: &mut Vec<u8>, at: usize, value: u32| {
            dex[at..at + 4].copy_from_slice(&value.to_le_bytes())
        };
        // Strings 0 "I", 1 "LA;", 2 "V", 3 "f", 4 "g": ids at 0x70, data at 0x100.
        let mut data = 0x100;
        for (index, text) in ["I", "LA;", "V", "f", "g"].iter().enumerate() {
            put(&mut dex, 0x70 + index * 4, data as u32);
            dex[data] = text.len() as u8;
            dex[data + 1..data + 1 + text.len()].copy_from_slice(text.as_bytes());
            data += text.len() + 2;
        }
        put(&mut dex, 0x38, 5);
        put(&mut dex, 0x3c, 0x70);
        // Types 0 I, 1 LA;, 2 V at 0x90.
        for (index, string) in [0u32, 1, 2].iter().enumerate() {
            put(&mut dex, 0x90 + index * 4, *string);
        }
        put(&mut dex, 0x40, 3);
        put(&mut dex, 0x44, 0x90);
        // Protos at 0xa0: 0 (I)V with parameters at 0x140, 1 ()V.
        put(&mut dex, 0xa0 + 4, 2);
        put(&mut dex, 0xa0 + 8, 0x140);
        put(&mut dex, 0xa0 + 12 + 4, 2);
        put(&mut dex, 0x48, 2);
        put(&mut dex, 0x4c, 0xa0);
        put(&mut dex, 0x140, 1); // one parameter: type 0 (I)
        // Methods at 0xc0: 0 LA;.f (I)V, 1 LA;.g ()V.
        for (index, (proto, name)) in [(0u16, 3u32), (1, 4)].iter().enumerate() {
            let at = 0xc0 + index * 8;
            dex[at..at + 2].copy_from_slice(&1u16.to_le_bytes());
            dex[at + 2..at + 4].copy_from_slice(&proto.to_le_bytes());
            put(&mut dex, at + 4, *name);
        }
        put(&mut dex, 0x58, 2);
        put(&mut dex, 0x5c, 0xc0);
        // One class def at 0xd0 with class data at 0x160: no fields, two
        // direct methods, f (native static) then g (index delta 1).
        put(&mut dex, 0xd0, 1);
        put(&mut dex, 0xd0 + 24, 0x160);
        put(&mut dex, 0x60, 1);
        put(&mut dex, 0x64, 0xd0);
        dex[0x160..0x16b].copy_from_slice(&[0, 0, 2, 0, 0, 0x88, 0x02, 0, 1, 0x08, 0]);
        dex
    }

    #[test]
    fn lists_only_native_methods() {
        assert_eq!(
            dex_natives(&fixture()).unwrap(),
            vec![NativeMethod {
                class: "LA;".into(),
                name: "f".into(),
                signature: "(I)V".into(),
            }]
        );
    }

    #[test]
    fn the_checked_in_inventory_parses() {
        let entries: Vec<_> = inventory(ANDROID16_NATIVES).collect();
        assert!(entries.len() > 6000);
        assert!(
            entries
                .iter()
                .any(|entry| entry.class == "Landroid/os/Debug;"
                    && entry.name == "getMemInfo"
                    && entry.signature == "([J)V"
                    && entry.jar == "/system/framework/framework.jar")
        );
    }

    #[test]
    fn rejects_non_dex_bytes() {
        assert!(dex_natives(b"PK\x03\x04").is_err());
        let mut truncated = fixture();
        truncated.truncate(0x90);
        assert!(dex_natives(&truncated).is_err());
    }
}
