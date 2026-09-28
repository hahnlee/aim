//! The resource table (`resources.arsc`, `ResTable`): packages, types,
//! and each entry's values per configuration, with the choice of the
//! configuration a Mac shows (the default language or English, no night
//! mode, left to right, the highest density).

use std::collections::HashMap;

use crate::res::{
    Result, STRING_POOL, Strings, TABLE, TABLE_PACKAGE, TABLE_TYPE, Value, bad, chunks, u16_at,
    u32_at,
};

/// An entry's value in one configuration.
#[derive(Clone, Debug)]
pub enum Entry {
    Simple(Value),
    /// A bag (style, array, ...): its parent and its (attribute, value)s.
    Bag(u32, Vec<(u32, Value)>),
}

/// The qualifiers of a `ResTable_config` this table looks at.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Config {
    pub mcc: u16,
    pub mnc: u16,
    pub language: [u8; 2],
    pub density: u16,
    pub sdk: u16,
    pub layout_dir: u8,
    pub night: bool,
}

pub const DENSITY_ANY: u16 = 0xfffe;
pub const DENSITY_NONE: u16 = 0xffff;
/// The newest API level the pinned image serves.
const SDK: u16 = 36;

impl Config {
    fn parse(c: &[u8]) -> Config {
        let size = u32_at(c, 0).unwrap_or(0) as usize;
        let byte = |at: usize| {
            if at < size {
                c.get(at).copied().unwrap_or(0)
            } else {
                0
            }
        };
        let short = |at: usize| u16::from_le_bytes([byte(at), byte(at + 1)]);
        Config {
            mcc: short(4),
            mnc: short(6),
            language: [byte(8), byte(9)],
            density: short(14),
            sdk: short(24),
            layout_dir: byte(28) & 0xc0,
            night: byte(29) & 0x30 == 0x20,
        }
    }

    /// Whether a Mac in English, day, left to right can use it.
    fn matches(&self) -> bool {
        self.mcc == 0
            && self.mnc == 0
            && (self.language == [0, 0] || self.language == *b"en")
            && self.sdk <= SDK
            && self.layout_dir != 0x80
            && !self.night
    }

    /// Higher is better: the default language, then the density a Mac
    /// wants (any, then the highest), then the newest API level.
    fn score(&self) -> (bool, u32, u16) {
        let density = match self.density {
            DENSITY_ANY => u32::MAX,
            DENSITY_NONE => u32::MAX - 1,
            0 => 160,
            d => d as u32,
        };
        (self.language == [0, 0], density, self.sdk)
    }
}

struct Type {
    config: Config,
    entries: HashMap<u16, Entry>,
}

struct Package {
    id: u8,
    /// Type chunks by type id.
    types: HashMap<u8, Vec<Type>>,
}

pub struct Table {
    strings: Strings,
    packages: Vec<Package>,
}

impl Table {
    pub fn parse(data: &[u8]) -> Result<Table> {
        let top = chunks(data)
            .next()
            .ok_or_else(|| bad("empty resource table"))??;
        if top.kind != TABLE {
            return Err(bad("not a resource table"));
        }
        let mut strings = None;
        let mut packages = Vec::new();
        for c in chunks(&top.data[top.header..]) {
            let c = c?;
            match c.kind {
                STRING_POOL => strings = Some(Strings::parse(c.data)?),
                TABLE_PACKAGE => {
                    let pool = strings
                        .as_ref()
                        .ok_or_else(|| bad("table without strings"))?;
                    packages.push(package(c.data, c.header, pool)?);
                }
                _ => {}
            }
        }
        Ok(Table {
            strings: strings.ok_or_else(|| bad("table without strings"))?,
            packages,
        })
    }

    /// Resource `id` in the best configuration that has it.
    pub fn get(&self, id: u32) -> Option<(&Entry, Config)> {
        let p = self.packages.iter().find(|p| p.id == (id >> 24) as u8)?;
        let entry = (id & 0xffff) as u16;
        p.types
            .get(&((id >> 16) as u8))?
            .iter()
            .filter(|t| t.config.matches())
            .filter_map(|t| t.entries.get(&entry).map(|e| (e, t.config)))
            .max_by_key(|(_, c)| c.score())
    }

    /// Whether the table has package `id` (0x7f an app, 0x01 the
    /// framework).
    pub fn has_package(&self, id: u8) -> bool {
        self.packages.iter().any(|p| p.id == id)
    }

    pub fn strings(&self) -> &Strings {
        &self.strings
    }
}

fn package(c: &[u8], header: usize, pool: &Strings) -> Result<Package> {
    let id = u32_at(c, 8)? as u8;
    let mut types: HashMap<u8, Vec<Type>> = HashMap::new();
    for sub in chunks(&c[header..]) {
        let sub = sub?;
        if sub.kind == TABLE_TYPE {
            let t = type_chunk(sub.data, sub.header, pool)?;
            types.entry(sub.data[8]).or_default().push(t);
        }
    }
    Ok(Package { id, types })
}

const FLAG_SPARSE: u8 = 0x01;
const FLAG_OFFSET16: u8 = 0x02;
const ENTRY_COMPLEX: u16 = 0x0001;
const ENTRY_COMPACT: u16 = 0x0008;

/// A `ResTable_type`: its configuration and entries.
fn type_chunk(c: &[u8], header: usize, pool: &Strings) -> Result<Type> {
    let flags = *c.get(9).ok_or_else(|| bad("truncated type"))?;
    let count = u32_at(c, 12)? as usize;
    let start = u32_at(c, 16)? as usize;
    let config = Config::parse(&c[20..header.max(20)]);
    let mut entries = HashMap::new();
    for i in 0..count {
        let (index, offset) = if flags & FLAG_SPARSE != 0 {
            (
                u16_at(c, header + 4 * i)?,
                u16_at(c, header + 4 * i + 2)? as usize * 4,
            )
        } else if flags & FLAG_OFFSET16 != 0 {
            match u16_at(c, header + 2 * i)? {
                0xffff => continue,
                o => (i as u16, o as usize * 4),
            }
        } else {
            match u32_at(c, header + 4 * i)? {
                u32::MAX => continue,
                o => (i as u16, o as usize),
            }
        };
        let at = start + offset;
        let size = u16_at(c, at)? as usize;
        let eflags = u16_at(c, at + 2)?;
        let entry = if eflags & ENTRY_COMPACT != 0 {
            Entry::Simple(Value::decode((eflags >> 8) as u8, u32_at(c, at + 4)?, pool))
        } else if eflags & ENTRY_COMPLEX != 0 {
            let parent = u32_at(c, at + 8)?;
            let n = u32_at(c, at + 12)? as usize;
            let mut map = Vec::with_capacity(n);
            for k in 0..n {
                let m = at + size + 12 * k;
                let name = u32_at(c, m)?;
                let kind = *c.get(m + 7).ok_or_else(|| bad("truncated map"))?;
                map.push((name, Value::decode(kind, u32_at(c, m + 8)?, pool)));
            }
            Entry::Bag(parent, map)
        } else {
            let v = at + size;
            let kind = *c.get(v + 3).ok_or_else(|| bad("truncated value"))?;
            Entry::Simple(Value::decode(kind, u32_at(c, v + 4)?, pool))
        };
        entries.insert(index, entry);
    }
    Ok(Type { config, entries })
}
