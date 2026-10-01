//! Adding method ids to a dex file ([`add_methods`]), for the redirect
//! edit of [`system_server`].
//!
//! A dex refers to its strings, types, protos and methods by their index in
//! sorted tables, so a method the dex does not name yet, with the strings,
//! type and proto it needs, is inserted where it sorts and every later index
//! moves. The dex is written again with each index and offset remapped: the
//! same sections and items in the same order, the new string data and type
//! lists at the end of their sections. Instructions keep their length, so
//! code addresses, branches and debug positions stay; a `const-string`
//! whose string would move past 65535 (it would need `const-string/jumbo`)
//! is refused, as are dex files with call sites, method handles, hidden API
//! flags or a link section, which the image's services.jar does not have.
//!
//! [`system_server`]: crate::system_server

use std::collections::{BTreeSet, HashMap};

use crate::dex::{self, Result, instruction_units};

const NO_INDEX: u32 = u32::MAX;
const HEADER_SIZE: usize = 0x70;

// Map item types (`DexFile::MapItemType`).
const STRING_ID: u16 = 0x0001;
const TYPE_ID: u16 = 0x0002;
const PROTO_ID: u16 = 0x0003;
const FIELD_ID: u16 = 0x0004;
const METHOD_ID: u16 = 0x0005;
const CLASS_DEF: u16 = 0x0006;
const MAP_LIST: u16 = 0x1000;
const TYPE_LIST: u16 = 0x1001;
const ANNOTATION_SET_REF_LIST: u16 = 0x1002;
const ANNOTATION_SET: u16 = 0x1003;
const CLASS_DATA: u16 = 0x2000;
const CODE: u16 = 0x2001;
const STRING_DATA: u16 = 0x2002;
const DEBUG_INFO: u16 = 0x2003;
const ANNOTATION: u16 = 0x2004;
const ENCODED_ARRAY: u16 = 0x2005;
const ANNOTATIONS_DIRECTORY: u16 = 0x2006;

/// A method id: class descriptor, name, signature `(params)return`.
pub type MethodRef = (String, String, String);

/// Writes `data` again with `methods` among its method ids. Returns the new
/// dex (checksums included) and each method's index in it.
pub fn add_methods(data: &[u8], methods: &[MethodRef]) -> Result<(Vec<u8>, Vec<u32>)> {
    let old = Old::parse(data)?;
    let plan = Plan::new(&old, methods)?;
    let mut offsets = Offsets::default();
    for _ in 0..8 {
        let (out, next) = Writer::write(&old, &plan, &offsets)?;
        if next == offsets {
            let indices = methods
                .iter()
                .map(|m| plan.method_index(&old, m))
                .collect::<Result<_>>()?;
            return Ok((out, indices));
        }
        offsets = next;
    }
    Err("dex: the layout does not settle".into())
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

fn uleb(d: &[u8], at: &mut usize) -> Result<u32> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = *d.get(*at).ok_or("dex: truncated leb128")?;
        *at += 1;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
    Err("dex: overlong leb128".into())
}

fn sleb(d: &[u8], at: &mut usize) -> Result<i32> {
    let start = *at;
    let value = uleb(d, at)?;
    let bits = 7 * (*at - start) as u32;
    Ok(if bits < 32 {
        ((value << (32 - bits)) as i32) >> (32 - bits)
    } else {
        value as i32
    })
}

fn align4(at: usize) -> usize {
    (at + 3) & !3
}

/// A string as UTF-16 code units, the order of the string ids.
fn utf16(mutf8: &[u8]) -> Vec<u16> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < mutf8.len() {
        let b = u16::from(mutf8[i]);
        let (unit, len) = if b < 0x80 {
            (b, 1)
        } else if b & 0xe0 == 0xc0 {
            let b1 = u16::from(*mutf8.get(i + 1).unwrap_or(&0));
            ((b & 0x1f) << 6 | (b1 & 0x3f), 2)
        } else {
            let b1 = u16::from(*mutf8.get(i + 1).unwrap_or(&0));
            let b2 = u16::from(*mutf8.get(i + 2).unwrap_or(&0));
            ((b & 0x0f) << 12 | (b1 & 0x3f) << 6 | (b2 & 0x3f), 3)
        };
        out.push(unit);
        i += len;
    }
    out
}

/// A `string_data_item` for `s`: its length in UTF-16 units, its MUTF-8
/// and a terminating zero.
fn string_data(s: &str) -> Vec<u8> {
    let units: Vec<u16> = s.encode_utf16().collect();
    let mut out = Vec::new();
    put_uleb(&mut out, units.len() as u32);
    for u in units {
        match u {
            0x01..=0x7f => out.push(u as u8),
            0x00 | 0x80..=0x7ff => out.extend([0xc0 | (u >> 6) as u8, 0x80 | (u & 0x3f) as u8]),
            _ => out.extend([
                0xe0 | (u >> 12) as u8,
                0x80 | ((u >> 6) & 0x3f) as u8,
                0x80 | (u & 0x3f) as u8,
            ]),
        }
    }
    out.push(0);
    out
}

fn put_uleb(out: &mut Vec<u8>, mut value: u32) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// The shorty of a signature `(params)return`.
fn shorty(sig: &str) -> Result<String> {
    let (params, ret) = split_sig(sig)?;
    Ok(std::iter::once(ret.as_str())
        .chain(params.iter().map(String::as_str))
        .map(|t| {
            if t.starts_with(['L', '[']) {
                'L'
            } else {
                t.as_bytes()[0] as char
            }
        })
        .collect())
}

/// The parameter and return type descriptors of `(params)return`.
pub fn split_sig(sig: &str) -> Result<(Vec<String>, String)> {
    let bad = || format!("dex: bad signature {sig}");
    let rest = sig.strip_prefix('(').ok_or_else(bad)?;
    let (params, ret) = rest.split_once(')').ok_or_else(bad)?;
    let mut out = Vec::new();
    let mut chars = params.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        let mut c = c;
        while c == '[' {
            c = chars.next().ok_or_else(bad)?.1;
        }
        if c == 'L' {
            chars.find(|&(_, c)| c == ';').ok_or_else(bad)?;
        } else if !"ZBSCIJFD".contains(c) {
            return Err(bad());
        }
        let end = chars.peek().map_or(params.len(), |&(i, _)| i);
        out.push(params[start..end].to_string());
    }
    if ret.is_empty() {
        return Err(bad());
    }
    Ok((out, ret.to_string()))
}

/// A table's new entries: each old index moves past the entries inserted
/// at or before it.
#[derive(Default)]
struct Shift {
    /// Where each new entry goes, as the old index it precedes, sorted.
    at: Vec<u32>,
}

impl Shift {
    fn map(&self, old: u32) -> u32 {
        if old == NO_INDEX {
            return old;
        }
        old + self.at.partition_point(|&p| p <= old) as u32
    }

    /// The new index of the `k`th new entry.
    fn new_index(&self, k: usize) -> u32 {
        self.at[k] + k as u32
    }
}

/// The original dex's tables.
struct Old<'a> {
    data: &'a [u8],
    sections: Vec<(u16, u32, u32)>,
    strings: (usize, usize),
    types: (usize, usize),
    protos: (usize, usize),
    fields: (usize, usize),
    methods: (usize, usize),
    classes: (usize, usize),
}

impl<'a> Old<'a> {
    fn parse(data: &'a [u8]) -> Result<Self> {
        dex::Dex::parse(data)?;
        if u32_at(data, 0x24)? as usize != HEADER_SIZE || u32_at(data, 0x2c)? != 0 {
            return Err("dex: not a plain header without a link section".into());
        }
        let map = u32_at(data, 0x34)? as usize;
        let mut sections = Vec::new();
        for i in 0..u32_at(data, map)? as usize {
            let at = map + 4 + 12 * i;
            sections.push((
                u16_at(data, at)?,
                u32_at(data, at + 4)?,
                u32_at(data, at + 8)?,
            ));
        }
        sections.sort_by_key(|s| s.2);
        let kinds: Vec<u16> = sections.iter().map(|s| s.0).collect();
        let index = [
            0, STRING_ID, TYPE_ID, PROTO_ID, FIELD_ID, METHOD_ID, CLASS_DEF,
        ];
        let data_kinds = [
            TYPE_LIST,
            ANNOTATION_SET_REF_LIST,
            ANNOTATION_SET,
            CLASS_DATA,
            CODE,
            STRING_DATA,
            DEBUG_INFO,
            ANNOTATION,
            ENCODED_ARRAY,
            ANNOTATIONS_DIRECTORY,
        ];
        let ok = kinds.len() > index.len()
            && kinds[..index.len()] == index
            && kinds.last() == Some(&MAP_LIST)
            && kinds[index.len()..kinds.len() - 1]
                .iter()
                .all(|k| data_kinds.contains(k))
            && kinds.contains(&STRING_DATA)
            && kinds.contains(&TYPE_LIST);
        if !ok {
            return Err(format!(
                "dex: sections {kinds:x?} are not the plain layout (index sections, data, map)"
            ));
        }
        let table = |off: usize| -> Result<(usize, usize)> {
            Ok((u32_at(data, off + 4)? as usize, u32_at(data, off)? as usize))
        };
        Ok(Old {
            data,
            sections,
            strings: table(0x38)?,
            types: table(0x40)?,
            protos: table(0x48)?,
            fields: table(0x50)?,
            methods: table(0x58)?,
            classes: table(0x60)?,
        })
    }

    /// The MUTF-8 of string `i`, and where its `string_data_item` starts.
    fn string(&self, i: usize) -> Result<(&'a [u8], u32)> {
        let off = u32_at(self.data, self.strings.0 + 4 * i)?;
        let mut at = off as usize;
        uleb(self.data, &mut at)?;
        let len = self.data[at..]
            .iter()
            .position(|b| *b == 0)
            .ok_or("dex: unterminated string")?;
        Ok((&self.data[at..at + len], off))
    }

    fn type_string(&self, i: usize) -> Result<u32> {
        u32_at(self.data, self.types.0 + 4 * i)
    }

    /// Proto `i`: shorty, return type, parameter types, parameters offset.
    fn proto(&self, i: usize) -> Result<(u32, u32, Vec<u32>, u32)> {
        let at = self.protos.0 + 12 * i;
        let params_off = u32_at(self.data, at + 8)?;
        let mut params = Vec::new();
        if params_off != 0 {
            let p = params_off as usize;
            for k in 0..u32_at(self.data, p)? as usize {
                params.push(u32::from(u16_at(self.data, p + 4 + 2 * k)?));
            }
        }
        Ok((
            u32_at(self.data, at)?,
            u32_at(self.data, at + 4)?,
            params,
            params_off,
        ))
    }

    /// Method `i`: class, proto, name.
    fn method(&self, i: usize) -> Result<(u32, u32, u32)> {
        let at = self.methods.0 + 8 * i;
        Ok((
            u32::from(u16_at(self.data, at)?),
            u32::from(u16_at(self.data, at + 2)?),
            u32_at(self.data, at + 4)?,
        ))
    }

    /// The first index in `0..n` for which `below` is false.
    fn lower_bound(n: usize, mut below: impl FnMut(usize) -> Result<bool>) -> Result<usize> {
        let (mut lo, mut hi) = (0, n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if below(mid)? {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        Ok(lo)
    }
}

/// What is added to each table, and the remapping of old indices.
struct Plan {
    strings: Shift,
    new_strings: Vec<String>,
    types: Shift,
    /// The new types' descriptors as new string indices.
    new_types: Vec<u32>,
    protos: Shift,
    /// New protos: shorty, return type, parameter types (new indices), and
    /// the old parameter list to share, if one is equal.
    new_protos: Vec<(u32, u32, Vec<u32>, Option<u32>)>,
    /// New type lists, for the new protos without an equal old one.
    new_type_lists: Vec<Vec<u32>>,
    methods: Shift,
    new_methods: Vec<(u32, u32, u32)>,
}

impl Plan {
    fn new(old: &Old, methods: &[MethodRef]) -> Result<Self> {
        let mut plan = Plan {
            strings: Shift::default(),
            new_strings: Vec::new(),
            types: Shift::default(),
            new_types: Vec::new(),
            protos: Shift::default(),
            new_protos: Vec::new(),
            new_type_lists: Vec::new(),
            methods: Shift::default(),
            new_methods: Vec::new(),
        };

        let mut wanted = BTreeSet::new();
        for (class, name, sig) in methods {
            let (params, ret) = split_sig(sig)?;
            wanted.insert(shorty(sig)?);
            wanted.insert(class.clone());
            wanted.insert(name.clone());
            wanted.insert(ret);
            wanted.extend(params);
        }
        let mut added: Vec<(Vec<u16>, String, u32)> = Vec::new();
        for s in wanted {
            let units: Vec<u16> = s.encode_utf16().collect();
            let at = Old::lower_bound(old.strings.1, |i| Ok(utf16(old.string(i)?.0) < units))?;
            let present = at < old.strings.1 && utf16(old.string(at)?.0) == units;
            if !present {
                added.push((units, s, at as u32));
            }
        }
        added.sort();
        for (_, s, at) in added {
            plan.strings.at.push(at);
            plan.new_strings.push(s);
        }

        let mut descriptors = BTreeSet::new();
        for (class, _, sig) in methods {
            let (params, ret) = split_sig(sig)?;
            descriptors.insert(plan.string_index(old, class)?);
            descriptors.insert(plan.string_index(old, &ret)?);
            for p in params {
                descriptors.insert(plan.string_index(old, &p)?);
            }
        }
        for s in descriptors {
            let at = Old::lower_bound(old.types.1, |i| {
                Ok(plan.strings.map(old.type_string(i)?) < s)
            })?;
            if at == old.types.1 || plan.strings.map(old.type_string(at)?) != s {
                plan.types.at.push(at as u32);
                plan.new_types.push(s);
            }
        }

        let mut protos = BTreeSet::new();
        for (_, _, sig) in methods {
            protos.insert(plan.proto_key(old, sig)?);
        }
        for (ret, params, shorty) in protos {
            let at = Old::lower_bound(old.protos.1, |i| {
                Ok(plan.old_proto_key(old, i)? < (ret, params.clone()))
            })?;
            if at < old.protos.1 && plan.old_proto_key(old, at)? == (ret, params.clone()) {
                continue;
            }
            let mut shared = None;
            if !params.is_empty() {
                for i in 0..old.protos.1 {
                    let (_, _, p, off) = old.proto(i)?;
                    if p.len() == params.len()
                        && p.iter()
                            .map(|&t| plan.types.map(t))
                            .eq(params.iter().copied())
                    {
                        shared = Some(off);
                        break;
                    }
                }
                if shared.is_none() {
                    plan.new_type_lists.push(params.clone());
                }
            }
            plan.protos.at.push(at as u32);
            plan.new_protos.push((shorty, ret, params, shared));
        }

        let mut keys = BTreeSet::new();
        for m in methods {
            keys.insert(plan.method_key(old, m)?);
        }
        for key in keys {
            let at = Old::lower_bound(old.methods.1, |i| Ok(plan.old_method_key(old, i)? < key))?;
            if at == old.methods.1 || plan.old_method_key(old, at)? != key {
                plan.methods.at.push(at as u32);
                plan.new_methods.push((key.0, key.2, key.1));
            }
        }

        let counts = [
            ("types", old.types.1 + plan.new_types.len()),
            ("protos", old.protos.1 + plan.new_protos.len()),
            ("methods", old.methods.1 + plan.new_methods.len()),
        ];
        for (what, n) in counts {
            if n > 0x10000 {
                return Err(format!("dex: {n} {what}, more than 16-bit indices reach"));
            }
        }
        Ok(plan)
    }

    /// The new index of string `s`, which is in the old dex or added.
    fn string_index(&self, old: &Old, s: &str) -> Result<u32> {
        if let Some(k) = self.new_strings.iter().position(|n| n == s) {
            return Ok(self.strings.new_index(k));
        }
        let units: Vec<u16> = s.encode_utf16().collect();
        let at = Old::lower_bound(old.strings.1, |i| Ok(utf16(old.string(i)?.0) < units))?;
        Ok(self.strings.map(at as u32))
    }

    fn type_index(&self, old: &Old, descriptor: &str) -> Result<u32> {
        let s = self.string_index(old, descriptor)?;
        if let Some(k) = self.new_types.iter().position(|&n| n == s) {
            return Ok(self.types.new_index(k));
        }
        let at = Old::lower_bound(old.types.1, |i| {
            Ok(self.strings.map(old.type_string(i)?) < s)
        })?;
        Ok(self.types.map(at as u32))
    }

    /// A signature's proto: return type, parameter types, shorty (new
    /// indices).
    fn proto_key(&self, old: &Old, sig: &str) -> Result<(u32, Vec<u32>, u32)> {
        let (params, ret) = split_sig(sig)?;
        Ok((
            self.type_index(old, &ret)?,
            params
                .iter()
                .map(|p| self.type_index(old, p))
                .collect::<Result<_>>()?,
            self.string_index(old, &shorty(sig)?)?,
        ))
    }

    fn old_proto_key(&self, old: &Old, i: usize) -> Result<(u32, Vec<u32>)> {
        let (_, ret, params, _) = old.proto(i)?;
        Ok((
            self.types.map(ret),
            params.into_iter().map(|t| self.types.map(t)).collect(),
        ))
    }

    fn proto_index(&self, old: &Old, sig: &str) -> Result<u32> {
        let (ret, params, _) = self.proto_key(old, sig)?;
        if let Some(k) = self
            .new_protos
            .iter()
            .position(|p| p.1 == ret && p.2 == params)
        {
            return Ok(self.protos.new_index(k));
        }
        let key = (ret, params);
        let at = Old::lower_bound(old.protos.1, |i| Ok(self.old_proto_key(old, i)? < key))?;
        Ok(self.protos.map(at as u32))
    }

    /// A method's sort key: class, name, proto (new indices).
    fn method_key(&self, old: &Old, (class, name, sig): &MethodRef) -> Result<(u32, u32, u32)> {
        Ok((
            self.type_index(old, class)?,
            self.string_index(old, name)?,
            self.proto_index(old, sig)?,
        ))
    }

    fn old_method_key(&self, old: &Old, i: usize) -> Result<(u32, u32, u32)> {
        let (class, proto, name) = old.method(i)?;
        Ok((
            self.types.map(class),
            self.strings.map(name),
            self.protos.map(proto),
        ))
    }

    fn method_index(&self, old: &Old, m: &MethodRef) -> Result<u32> {
        let key = self.method_key(old, m)?;
        if let Some(k) = self.new_methods.iter().position(|n| (n.0, n.2, n.1) == key) {
            return Ok(self.methods.new_index(k));
        }
        let at = Old::lower_bound(old.methods.1, |i| Ok(self.old_method_key(old, i)? < key))?;
        Ok(self.methods.map(at as u32))
    }
}

/// Where the items are in a written dex: each old item's offset by its old
/// offset, and the new string data and type lists.
#[derive(Default, PartialEq)]
struct Offsets {
    old: HashMap<u32, u32>,
    strings: Vec<u32>,
    type_lists: Vec<u32>,
}

impl Offsets {
    fn of(&self, old: u32) -> u32 {
        if old == 0 {
            return 0;
        }
        self.old.get(&old).copied().unwrap_or(0)
    }
}

struct Writer<'a, 'p> {
    old: &'a Old<'a>,
    plan: &'p Plan,
    prev: &'p Offsets,
    out: Vec<u8>,
    next: Offsets,
}

impl<'a, 'p> Writer<'a, 'p> {
    /// Writes the dex with the offsets of the previous pass; returns it
    /// and the offsets it has.
    fn write(old: &'a Old<'a>, plan: &'p Plan, prev: &'p Offsets) -> Result<(Vec<u8>, Offsets)> {
        let mut w = Writer {
            old,
            plan,
            prev,
            out: vec![0; HEADER_SIZE],
            next: Offsets::default(),
        };
        let d = old.data;
        let mut map = vec![(0u16, 1u32, 0u32)];

        let strings_off = w.out.len();
        let mut new_string = 0;
        for i in 0..old.strings.1 + plan.new_strings.len() {
            let off = if plan
                .strings
                .at
                .get(new_string)
                .copied()
                .map(|a| a as usize + new_string)
                == Some(i)
            {
                new_string += 1;
                prev.strings.get(new_string - 1).copied().unwrap_or(0)
            } else {
                prev.of(old.string(i - new_string)?.1)
            };
            w.u32(off);
        }
        map.push((
            STRING_ID,
            (old.strings.1 + plan.new_strings.len()) as u32,
            strings_off as u32,
        ));

        let types_off = w.out.len();
        let types = merge(
            (0..old.types.1).map(|i| old.type_string(i).map(|s| plan.strings.map(s))),
            &plan.types,
            plan.new_types.iter().map(|&s| Ok(s)),
        )?;
        for s in &types {
            w.u32(*s);
        }
        map.push((TYPE_ID, types.len() as u32, types_off as u32));

        let protos_off = w.out.len();
        let mut new_list = 0;
        let protos = merge(
            (0..old.protos.1).map(|i| {
                let (shorty, ret, _, params) = old.proto(i)?;
                Ok((
                    plan.strings.map(shorty),
                    plan.types.map(ret),
                    prev.of(params),
                ))
            }),
            &plan.protos,
            plan.new_protos.iter().map(|(shorty, ret, params, shared)| {
                let params = match shared {
                    _ if params.is_empty() => 0,
                    Some(off) => prev.of(*off),
                    None => {
                        new_list += 1;
                        prev.type_lists.get(new_list - 1).copied().unwrap_or(0)
                    }
                };
                Ok((*shorty, *ret, params))
            }),
        )?;
        for (shorty, ret, params) in &protos {
            w.u32(*shorty);
            w.u32(*ret);
            w.u32(*params);
        }
        map.push((PROTO_ID, protos.len() as u32, protos_off as u32));

        let fields_off = w.out.len();
        for i in 0..old.fields.1 {
            let at = old.fields.0 + 8 * i;
            w.u16(plan.types.map(u32::from(u16_at(d, at)?)) as u16);
            w.u16(plan.types.map(u32::from(u16_at(d, at + 2)?)) as u16);
            w.u32(plan.strings.map(u32_at(d, at + 4)?));
        }
        map.push((FIELD_ID, old.fields.1 as u32, fields_off as u32));

        let methods_off = w.out.len();
        let methods = merge(
            (0..old.methods.1).map(|i| {
                let (class, proto, name) = old.method(i)?;
                Ok((
                    plan.types.map(class),
                    plan.protos.map(proto),
                    plan.strings.map(name),
                ))
            }),
            &plan.methods,
            plan.new_methods.iter().map(|&m| Ok(m)),
        )?;
        for (class, proto, name) in &methods {
            w.u16(*class as u16);
            w.u16(*proto as u16);
            w.u32(*name);
        }
        map.push((METHOD_ID, methods.len() as u32, methods_off as u32));

        let classes_off = w.out.len();
        for i in 0..old.classes.1 {
            let at = old.classes.0 + 32 * i;
            let f = |k: usize| u32_at(d, at + 4 * k);
            w.u32(plan.types.map(f(0)?));
            w.u32(f(1)?);
            w.u32(plan.types.map(f(2)?));
            w.u32(prev.of(f(3)?));
            w.u32(plan.strings.map(f(4)?));
            for k in 5..8 {
                w.u32(prev.of(f(k)?));
            }
        }
        map.push((CLASS_DEF, old.classes.1 as u32, classes_off as u32));

        let data_off = w.out.len();
        let data_sections: Vec<(u16, u32, u32)> = old.sections[7..old.sections.len() - 1].to_vec();
        for (kind, count, offset) in data_sections {
            let aligned = matches!(
                kind,
                TYPE_LIST | ANNOTATION_SET_REF_LIST | ANNOTATION_SET | CODE | ANNOTATIONS_DIRECTORY
            );
            if aligned {
                w.align();
            }
            let start = w.out.len() as u32;
            let mut at = offset as usize;
            for _ in 0..count {
                if aligned {
                    at = align4(at);
                    w.align();
                }
                w.next.old.insert(at as u32, w.out.len() as u32);
                at = w.item(kind, at)?;
            }
            let mut added = 0;
            if kind == STRING_DATA {
                for s in &plan.new_strings {
                    w.next.strings.push(w.out.len() as u32);
                    w.out.extend(string_data(s));
                    added += 1;
                }
            } else if kind == TYPE_LIST {
                for list in &plan.new_type_lists {
                    w.align();
                    w.next.type_lists.push(w.out.len() as u32);
                    w.u32(list.len() as u32);
                    for &t in list {
                        w.u16(t as u16);
                    }
                    added += 1;
                }
            }
            map.push((kind, count + added, start));
        }

        w.align();
        let map_off = w.out.len();
        map.push((MAP_LIST, 1, map_off as u32));
        w.u32(map.len() as u32);
        for &(kind, count, offset) in &map {
            w.u16(kind);
            w.u16(0);
            w.u32(count);
            w.u32(offset);
        }

        let mut out = w.out;
        let size = out.len();
        let mut header = Vec::with_capacity(HEADER_SIZE);
        header.extend_from_slice(&d[..8]);
        header.resize(32, 0);
        let counts =
            |count: usize, off: usize| [count as u32, if count == 0 { 0 } else { off as u32 }];
        for v in [
            size as u32,
            HEADER_SIZE as u32,
            0x1234_5678,
            0,
            0,
            map_off as u32,
        ]
        .into_iter()
        .chain(counts(old.strings.1 + plan.new_strings.len(), strings_off))
        .chain(counts(types.len(), types_off))
        .chain(counts(protos.len(), protos_off))
        .chain(counts(old.fields.1, fields_off))
        .chain(counts(methods.len(), methods_off))
        .chain(counts(old.classes.1, classes_off))
        .chain([(size - data_off) as u32, data_off as u32])
        {
            header.extend(v.to_le_bytes());
        }
        out[..HEADER_SIZE].copy_from_slice(&header);
        dex::fix_checksums(&mut out);
        Ok((out, w.next))
    }

    fn u16(&mut self, v: u16) {
        self.out.extend(v.to_le_bytes());
    }

    fn u32(&mut self, v: u32) {
        self.out.extend(v.to_le_bytes());
    }

    fn uleb(&mut self, v: u32) {
        put_uleb(&mut self.out, v);
    }

    fn align(&mut self) {
        self.out.resize(align4(self.out.len()), 0);
    }

    /// Copies the leb128 at `at` as it is.
    fn copy_leb(&mut self, at: &mut usize) -> Result<()> {
        let start = *at;
        uleb(self.old.data, at)?;
        self.out.extend_from_slice(&self.old.data[start..*at]);
        Ok(())
    }

    fn string(&self, i: u32) -> u32 {
        self.plan.strings.map(i)
    }

    fn type_(&self, i: u32) -> u32 {
        self.plan.types.map(i)
    }

    fn method(&self, i: u32) -> u32 {
        self.plan.methods.map(i)
    }

    /// A `uleb128p1` index remapped by `map`.
    fn p1(&mut self, at: &mut usize, map: fn(&Self, u32) -> u32) -> Result<()> {
        let v = uleb(self.old.data, at)?;
        let v = if v == 0 { 0 } else { map(self, v - 1) + 1 };
        self.uleb(v);
        Ok(())
    }

    /// Writes the item of `kind` at `at`; returns where it ends.
    fn item(&mut self, kind: u16, at: usize) -> Result<usize> {
        let d = self.old.data;
        let mut at = at;
        match kind {
            STRING_DATA => {
                let start = at;
                uleb(d, &mut at)?;
                at += d[at..]
                    .iter()
                    .position(|b| *b == 0)
                    .ok_or("dex: unterminated string")?
                    + 1;
                self.out.extend_from_slice(&d[start..at]);
            }
            TYPE_LIST => {
                let n = u32_at(d, at)?;
                self.u32(n);
                for k in 0..n as usize {
                    let t = self.type_(u32::from(u16_at(d, at + 4 + 2 * k)?));
                    self.u16(t as u16);
                }
                at += 4 + 2 * n as usize;
            }
            ANNOTATION_SET_REF_LIST | ANNOTATION_SET => {
                let n = u32_at(d, at)?;
                self.u32(n);
                for k in 0..n as usize {
                    let off = self.prev.of(u32_at(d, at + 4 + 4 * k)?);
                    self.u32(off);
                }
                at += 4 + 4 * n as usize;
            }
            CLASS_DATA => {
                let counts: Vec<u32> = (0..4).map(|_| uleb(d, &mut at)).collect::<Result<_>>()?;
                for &c in &counts {
                    self.uleb(c);
                }
                for _ in 0..counts[0] + counts[1] {
                    self.copy_leb(&mut at)?;
                    self.copy_leb(&mut at)?;
                }
                for &count in &counts[2..] {
                    let (mut old, mut new) = (0u32, 0u32);
                    for _ in 0..count {
                        old += uleb(d, &mut at)?;
                        let mapped = self.method(old);
                        self.uleb(mapped - new);
                        new = mapped;
                        self.copy_leb(&mut at)?;
                        let code = uleb(d, &mut at)?;
                        let code = self.prev.of(code);
                        self.uleb(code);
                    }
                }
            }
            CODE => at = self.code(at)?,
            DEBUG_INFO => {
                self.copy_leb(&mut at)?;
                let params = uleb(d, &mut at)?;
                self.uleb(params);
                for _ in 0..params {
                    self.p1(&mut at, Self::string)?;
                }
                loop {
                    let op = d[at];
                    at += 1;
                    self.out.push(op);
                    match op {
                        0x00 => break,
                        0x01 | 0x02 | 0x05 | 0x06 => self.copy_leb(&mut at)?,
                        0x03 | 0x04 => {
                            self.copy_leb(&mut at)?;
                            self.p1(&mut at, Self::string)?;
                            self.p1(&mut at, Self::type_)?;
                            if op == 0x04 {
                                self.p1(&mut at, Self::string)?;
                            }
                        }
                        0x09 => self.p1(&mut at, Self::string)?,
                        _ => {}
                    }
                }
            }
            ANNOTATION => {
                self.out.push(d[at]);
                at += 1;
                self.annotation(&mut at)?;
            }
            ENCODED_ARRAY => self.array(&mut at)?,
            ANNOTATIONS_DIRECTORY => {
                let class = self.prev.of(u32_at(d, at)?);
                self.u32(class);
                let (fields, methods, params) =
                    (u32_at(d, at + 4)?, u32_at(d, at + 8)?, u32_at(d, at + 12)?);
                self.u32(fields);
                self.u32(methods);
                self.u32(params);
                at += 16;
                for k in 0..fields + methods + params {
                    let index = u32_at(d, at)?;
                    let index = if k < fields {
                        index
                    } else {
                        self.method(index)
                    };
                    self.u32(index);
                    let off = self.prev.of(u32_at(d, at + 4)?);
                    self.u32(off);
                    at += 8;
                }
            }
            _ => return Err(format!("dex: no writer for section {kind:#x}")),
        }
        Ok(at)
    }

    fn code(&mut self, at: usize) -> Result<usize> {
        let d = self.old.data;
        self.out.extend_from_slice(&d[at..at + 6]);
        let tries = u16_at(d, at + 6)? as usize;
        self.u16(tries as u16);
        let debug = self.prev.of(u32_at(d, at + 8)?);
        self.u32(debug);
        let n = u32_at(d, at + 12)? as usize;
        self.u32(n as u32);
        let insns_at = at + 16;
        let insns: Vec<u16> = (0..n)
            .map(|k| u16_at(d, insns_at + 2 * k))
            .collect::<Result<_>>()?;
        let mut units = insns.clone();
        let mut pc = 0;
        while pc < n {
            let op = insns[pc] & 0xff;
            let wide = |u: &[u16]| u32::from(u[pc + 1]) | u32::from(u[pc + 2]) << 16;
            match op {
                0x1a => {
                    let s = self.string(u32::from(insns[pc + 1]));
                    if s > 0xffff {
                        return Err(format!(
                            "dex: a const-string at {:#x} would need const-string/jumbo",
                            insns_at + 2 * pc
                        ));
                    }
                    units[pc + 1] = s as u16;
                }
                0x1b => {
                    let s = self.string(wide(&insns));
                    units[pc + 1] = s as u16;
                    units[pc + 2] = (s >> 16) as u16;
                }
                0x1c | 0x1f | 0x20 | 0x22 | 0x23 | 0x24 | 0x25 => {
                    units[pc + 1] = self.type_(u32::from(insns[pc + 1])) as u16;
                }
                0x6e..=0x72 | 0x74..=0x78 => {
                    units[pc + 1] = self.method(u32::from(insns[pc + 1])) as u16;
                }
                0xfa | 0xfb => {
                    units[pc + 1] = self.method(u32::from(insns[pc + 1])) as u16;
                    units[pc + 3] = self.plan.protos.map(u32::from(insns[pc + 3])) as u16;
                }
                0xff => units[pc + 1] = self.plan.protos.map(u32::from(insns[pc + 1])) as u16,
                0xfc..=0xfe => return Err("dex: call sites and method handles".into()),
                _ => {}
            }
            pc += instruction_units(&insns, pc)?;
        }
        for u in units {
            self.u16(u);
        }
        let mut at = insns_at + 2 * n;
        if tries == 0 {
            return Ok(at);
        }
        if n % 2 == 1 {
            self.u16(0);
            at += 2;
        }
        let tries_at = at;
        let list_at = tries_at + 8 * tries;
        // The handlers first, to know where each moved within the list.
        let mut list = Vec::new();
        let mut moved = HashMap::new();
        let mut h = list_at;
        let count = uleb(d, &mut h)?;
        put_uleb(&mut list, count);
        for _ in 0..count {
            moved.insert(h - list_at, list.len());
            let start = h;
            let size = sleb(d, &mut h)?;
            list.extend_from_slice(&d[start..h]);
            for _ in 0..size.unsigned_abs() {
                put_uleb(&mut list, self.type_(uleb(d, &mut h)?));
                let start = h;
                uleb(d, &mut h)?;
                list.extend_from_slice(&d[start..h]);
            }
            if size <= 0 {
                let start = h;
                uleb(d, &mut h)?;
                list.extend_from_slice(&d[start..h]);
            }
        }
        for t in 0..tries {
            let at = tries_at + 8 * t;
            self.out.extend_from_slice(&d[at..at + 6]);
            let off = *moved
                .get(&(u16_at(d, at + 6)? as usize))
                .ok_or("dex: a try names no handler")?;
            self.u16(u16::try_from(off).map_err(|_| "dex: handlers past 64 KiB")?);
        }
        self.out.extend(list);
        Ok(h)
    }

    fn annotation(&mut self, at: &mut usize) -> Result<()> {
        let d = self.old.data;
        let t = self.type_(uleb(d, at)?);
        self.uleb(t);
        let n = uleb(d, at)?;
        self.uleb(n);
        for _ in 0..n {
            let name = self.string(uleb(d, at)?);
            self.uleb(name);
            self.value(at)?;
        }
        Ok(())
    }

    fn array(&mut self, at: &mut usize) -> Result<()> {
        let n = uleb(self.old.data, at)?;
        self.uleb(n);
        for _ in 0..n {
            self.value(at)?;
        }
        Ok(())
    }

    fn value(&mut self, at: &mut usize) -> Result<()> {
        let d = self.old.data;
        let head = *d.get(*at).ok_or("dex: truncated value")?;
        *at += 1;
        let (kind, len) = (head & 0x1f, usize::from(head >> 5) + 1);
        let map: Option<fn(&Self, u32) -> u32> = match kind {
            0x15 => Some(|w, i| w.plan.protos.map(i)),
            0x17 => Some(Self::string),
            0x18 => Some(Self::type_),
            0x1a => Some(Self::method),
            _ => None,
        };
        match kind {
            0x00 | 0x02..=0x04 | 0x06 | 0x10 | 0x11 | 0x19 | 0x1b => {
                self.out.push(head);
                self.out
                    .extend_from_slice(d.get(*at..*at + len).ok_or("dex: truncated value")?);
                *at += len;
            }
            0x15 | 0x17 | 0x18 | 0x1a => {
                let bytes = d.get(*at..*at + len).ok_or("dex: truncated value")?;
                *at += len;
                let index = bytes.iter().rev().fold(0u32, |v, b| v << 8 | u32::from(*b));
                let index = map.unwrap()(self, index);
                let width = (4 - index.leading_zeros() as usize / 8).max(1);
                self.out.push(kind | ((width - 1) as u8) << 5);
                self.out.extend_from_slice(&index.to_le_bytes()[..width]);
            }
            0x1c => {
                self.out.push(head);
                self.array(at)?;
            }
            0x1d => {
                self.out.push(head);
                self.annotation(at)?;
            }
            0x1e | 0x1f => self.out.push(head),
            _ => return Err(format!("dex: value type {kind:#x}")),
        }
        Ok(())
    }
}

/// A table of `old` entries with `new` ones inserted where `shift` puts
/// them.
fn merge<T>(
    old: impl Iterator<Item = Result<T>>,
    shift: &Shift,
    new: impl Iterator<Item = Result<T>>,
) -> Result<Vec<T>> {
    let mut out = Vec::new();
    let mut new = new.zip(&shift.at).peekable();
    for (i, item) in old.enumerate() {
        while let Some((n, _)) = new.next_if(|(_, at)| **at as usize <= i) {
            out.push(n?);
        }
        out.push(item?);
    }
    for (n, _) in new {
        out.push(n?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures() {
        assert_eq!(
            split_sig("(I[JLa/B;[[Lc;Z)V").unwrap(),
            (
                vec![
                    "I".into(),
                    "[J".into(),
                    "La/B;".into(),
                    "[[Lc;".into(),
                    "Z".into()
                ],
                "V".into()
            )
        );
        assert_eq!(shorty("(I[JLa/B;Z)[I").unwrap(), "LILLZ");
        assert!(split_sig("(Q)V").is_err());
        assert!(split_sig("()").is_err());
    }

    #[test]
    fn strings() {
        let s = "a\u{0}é€😀";
        let data = string_data(s);
        assert_eq!(data[0], 6);
        let units = utf16(&data[1..data.len() - 1]);
        assert_eq!(units, s.encode_utf16().collect::<Vec<_>>());
    }

    #[test]
    fn shifts() {
        let shift = Shift { at: vec![0, 2, 2] };
        assert_eq!([0, 1, 2, 3].map(|i| shift.map(i)), [1, 2, 5, 6]);
        assert_eq!([0, 1, 2].map(|k| shift.new_index(k)), [0, 3, 4]);
        let merged = merge(
            ["a", "b", "c"].into_iter().map(Ok),
            &shift,
            ["x", "y", "z"].into_iter().map(Ok),
        )
        .unwrap();
        assert_eq!(merged, ["x", "a", "b", "y", "z", "c"]);
    }
}
