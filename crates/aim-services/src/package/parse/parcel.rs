//! The parser cache's parcel: `PackageImpl.writeToParcel` into a parcel
//! with `PackageParserCacheHelper.WriteHelper` installed, as
//! `PackageCacher.toCacheEntryStatic` writes it (`android-16.0.0_r1`).
//! Every string, `writeString8` included, is an index into a pool that the
//! helper appends at the end with `writeStringList`, and the parcel's first
//! int is the pool's position. Collections are written in Java's iteration
//! order: an `ArrayMap` or `ArraySet` iterates by the keys' `hashCode`.
//!
//! Ported from the Android Open Source Project (`android.os.Parcel`,
//! `android.os.BaseBundle`, `android.util.ArrayMap`), Copyright (C) The
//! Android Open Source Project, Licensed under the Apache License, Version
//! 2.0.

use std::collections::HashMap;

use aim_binder_host::parcel::Parcel;

/// `BaseBundle.BUNDLE_MAGIC`.
const BUNDLE_MAGIC: i32 = 0x4C44_4E42;

// `Parcel.VAL_*`.
const VAL_NULL: i32 = -1;
const VAL_STRING: i32 = 0;
const VAL_INTEGER: i32 = 1;
const VAL_PARCELABLE: i32 = 4;
const VAL_FLOAT: i32 = 7;
const VAL_BOOLEAN: i32 = 9;

/// `String.hashCode`.
pub fn java_hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, u| h.wrapping_mul(31).wrapping_add(u as i32))
}

/// An `ArrayMap<String, V>`: entries in the order of their keys' hashes,
/// a new key after the keys of its hash.
#[derive(Clone, Debug, PartialEq)]
pub struct ArrayMap<V>(Vec<(String, V)>);

impl<V> Default for ArrayMap<V> {
    fn default() -> Self {
        ArrayMap(Vec::new())
    }
}

impl<V> ArrayMap<V> {
    /// `indexOf`: the key's index, or where it goes.
    fn index_of(&self, key: &str) -> Result<usize, usize> {
        let hash = java_hash(key);
        let hashes: Vec<i32> = self.0.iter().map(|(k, _)| java_hash(k)).collect();
        let Ok(found) = hashes.binary_search(&hash) else {
            return Err(hashes.partition_point(|&h| h < hash));
        };
        let mut end = found;
        while end < hashes.len() && hashes[end] == hash {
            if self.0[end].0 == key {
                return Ok(end);
            }
            end += 1;
        }
        let mut i = found;
        while i > 0 && hashes[i - 1] == hash {
            i -= 1;
            if self.0[i].0 == key {
                return Ok(i);
            }
        }
        Err(end)
    }

    pub fn put(&mut self, key: &str, value: V) {
        match self.index_of(key) {
            Ok(i) => self.0[i].1 = value,
            Err(i) => self.0.insert(i, (key.to_owned(), value)),
        }
    }

    pub fn get(&self, key: &str) -> Option<&V> {
        self.index_of(key).ok().map(|i| &self.0[i].1)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        self.index_of(key).ok().map(|i| &mut self.0[i].1)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.index_of(key).is_ok()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.0.iter().map(|(k, v)| (k.as_str(), v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(k, _)| k.as_str())
    }
}

/// An `ArraySet<String>`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ArraySet(ArrayMap<()>);

impl ArraySet {
    pub fn add(&mut self, s: &str) {
        self.0.put(s, ());
    }

    pub fn contains(&self, s: &str) -> bool {
        self.0.contains(s)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.keys()
    }
}

/// A value of a `Bundle` the parser makes (`Property.toBundle`).
#[derive(Clone, Debug, PartialEq)]
pub enum BundleValue {
    Bool(bool),
    Float(f32),
    Int(i32),
    String(Option<String>),
}

pub type Bundle = ArrayMap<BundleValue>;

/// A parcel being written with the cache's string pool, and where each
/// field starts, for the comparison with the original's.
pub struct Writer {
    p: Parcel,
    pool: Vec<Option<String>>,
    index: HashMap<Option<String>, i32>,
    path: Vec<String>,
    marks: Vec<(usize, String)>,
    strings: Vec<(usize, i32)>,
}

/// A written cache entry.
pub struct Entry {
    pub bytes: Vec<u8>,
    /// Where the string pool starts.
    pub pool_at: usize,
    /// Each field's start and its path, in order.
    pub marks: Vec<(usize, String)>,
    /// The pool's strings.
    pub pool: Vec<Option<String>>,
    /// Where each string was written, and its index in the pool.
    pub strings: Vec<(usize, i32)>,
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

impl Writer {
    /// `WriteHelper`'s constructor: the pool's position, filled in later.
    pub fn new() -> Writer {
        let mut p = Parcel::new();
        p.write_i32(0);
        Writer {
            p,
            pool: Vec::new(),
            index: HashMap::new(),
            path: Vec::new(),
            marks: Vec::new(),
            strings: Vec::new(),
        }
    }

    /// `finishAndUninstall`: the pool, written without the helper.
    pub fn finish(mut self) -> Entry {
        let pool_at = self.p.position();
        self.p.set_i32_at(0, pool_at as i32);
        self.p.write_i32(self.pool.len() as i32);
        for s in &self.pool {
            self.p.write_string16(s.as_deref());
        }
        Entry {
            bytes: self.p.data().to_vec(),
            pool_at,
            marks: self.marks,
            pool: self.pool,
            strings: self.strings,
        }
    }

    /// Names the field written next.
    pub fn field(&mut self, name: &str) {
        let mut path = self.path.join(".");
        if !path.is_empty() {
            path.push('.');
        }
        path.push_str(name);
        self.marks.push((self.p.position(), path));
    }

    /// Writes the fields of `name` (an element of a list, an object).
    pub fn scope(&mut self, name: impl Into<String>, write: impl FnOnce(&mut Writer)) {
        self.path.push(name.into());
        write(self);
        self.path.pop();
    }

    pub fn int(&mut self, v: i32) {
        self.p.write_i32(v);
    }

    pub fn long(&mut self, v: i64) {
        self.p.write_i64(v);
    }

    pub fn float(&mut self, v: f32) {
        self.p.write_f32(v);
    }

    /// `writeBoolean`.
    pub fn bool(&mut self, v: bool) {
        self.p.write_i32(v.into());
    }

    /// `writeString`, `writeString8`: the string's index in the pool.
    pub fn string(&mut self, s: Option<&str>) {
        let key = s.map(str::to_owned);
        let i = match self.index.get(&key) {
            Some(&i) => i,
            None => {
                let i = self.pool.len() as i32;
                self.index.insert(key.clone(), i);
                self.pool.push(key);
                i
            }
        };
        self.strings.push((self.p.position(), i));
        self.p.write_i32(i);
    }

    /// `writeStringList`, `writeStringArray`, `writeString8Array`; `None`
    /// is null.
    pub fn strings<S: AsRef<str>>(&mut self, v: Option<&[S]>) {
        let Some(v) = v else { return self.int(-1) };
        self.int(v.len() as i32);
        for s in v {
            self.string(Some(s.as_ref()));
        }
    }

    /// `ForStringSet` and `ForInternedStringSet`.
    pub fn set(&mut self, v: &ArraySet) {
        self.int(v.len() as i32);
        for s in v.iter() {
            self.string(Some(s));
        }
    }

    /// `writeIntArray`.
    pub fn ints(&mut self, v: Option<&[i32]>) {
        let Some(v) = v else { return self.int(-1) };
        self.int(v.len() as i32);
        v.iter().for_each(|&i| self.int(i));
    }

    /// `writeLongArray`.
    pub fn longs(&mut self, v: Option<&[i64]>) {
        let Some(v) = v else { return self.int(-1) };
        self.int(v.len() as i32);
        v.iter().for_each(|&i| self.long(i));
    }

    /// `writeBooleanArray`.
    pub fn bools(&mut self, v: Option<&[bool]>) {
        let Some(v) = v else { return self.int(-1) };
        self.int(v.len() as i32);
        v.iter().for_each(|&b| self.bool(b));
    }

    /// `writeByteArray`.
    pub fn bytes(&mut self, v: Option<&[u8]>) {
        let Some(v) = v else { return self.int(-1) };
        self.int(v.len() as i32);
        self.p.write_raw(v, &[]);
    }

    /// `writeCharSequence` of a plain string (`TextUtils.writeToParcel`).
    pub fn char_sequence(&mut self, s: Option<&str>) {
        self.int(1);
        self.string(s);
    }

    /// `writeTypedList`, `ParsingUtils.writeParcelableList`: each element
    /// as a present `writeTypedObject`.
    pub fn list<T>(&mut self, name: &str, v: &[T], mut write: impl FnMut(&mut Writer, &T)) {
        self.int(v.len() as i32);
        for (i, e) in v.iter().enumerate() {
            self.scope(format!("{name}[{i}]"), |w| {
                w.int(1);
                write(w, e);
            });
        }
    }

    /// A length-prefixed `writeValue` of type `kind`.
    pub fn prefixed(&mut self, kind: i32, write: impl FnOnce(&mut Writer)) {
        self.int(kind);
        let at = self.p.position();
        self.int(-1);
        let start = self.p.position();
        write(self);
        let end = self.p.position();
        self.p.set_i32_at(at, (end - start) as i32);
    }

    /// Nullable pooled strings, including null entries of a decoded array.
    pub fn optional_strings(&mut self, values: Option<&[Option<String>]>) {
        let Some(values) = values else {
            return self.int(-1);
        };
        self.int(values.len() as i32);
        for value in values {
            self.string(value.as_deref());
        }
    }

    pub fn double(&mut self, value: f64) {
        self.p.write_i64(value.to_bits() as i64);
    }

    /// Bundle framing shared by parsed and decoded package values.
    pub fn bundle_entries<T>(
        &mut self,
        values: Option<&[T]>,
        mut write: impl FnMut(&mut Self, &T),
    ) {
        let Some(values) = values else {
            return self.int(-1);
        };
        if values.is_empty() {
            return self.int(0);
        }
        let at = self.p.position();
        self.int(-1);
        self.int(BUNDLE_MAGIC);
        let start = self.p.position();
        self.int(values.len() as i32);
        for value in values {
            write(self, value);
        }
        self.p.set_i32_at(at, (self.p.position() - start) as i32);
        self.bool(false);
    }

    /// `writeBundle`.
    pub fn bundle(&mut self, b: Option<&Bundle>) {
        let Some(b) = b else { return self.int(-1) };
        if b.is_empty() {
            return self.int(0);
        }
        let at = self.p.position();
        self.int(-1);
        self.int(BUNDLE_MAGIC);
        let start = self.p.position();
        self.int(b.len() as i32);
        for (k, v) in b.iter() {
            self.string(Some(k));
            match v {
                BundleValue::Bool(v) => {
                    self.int(VAL_BOOLEAN);
                    self.bool(*v);
                }
                BundleValue::Float(v) => {
                    self.int(VAL_FLOAT);
                    self.float(*v);
                }
                BundleValue::Int(v) => {
                    self.int(VAL_INTEGER);
                    self.int(*v);
                }
                BundleValue::String(v) => {
                    self.int(VAL_STRING);
                    self.string(v.as_deref());
                }
            }
        }
        let end = self.p.position();
        self.p.set_i32_at(at, (end - start) as i32);
        // mHasIntent
        self.bool(false);
    }

    /// `writeMap` of strings (`ForInternedStringValueMap`).
    pub fn string_map(&mut self, m: &ArrayMap<String>) {
        self.int(m.len() as i32);
        for (k, v) in m.iter() {
            self.int(VAL_STRING);
            self.string(Some(k));
            self.int(VAL_STRING);
            self.string(Some(v));
        }
    }

    /// `writeMap` of parcelables: each value as `writeValue`'s
    /// `VAL_PARCELABLE`, its class name then its fields; `None` is null.
    pub fn parcelable_map<V>(
        &mut self,
        class: &str,
        m: &ArrayMap<V>,
        mut write: impl FnMut(&mut Writer, &V),
    ) {
        self.int(m.len() as i32);
        for (k, v) in m.iter() {
            self.int(VAL_STRING);
            self.string(Some(k));
            self.scope(k.to_owned(), |w| {
                w.prefixed(VAL_PARCELABLE, |w| {
                    w.string(Some(class));
                    write(w, v);
                })
            });
        }
    }

    /// `writeValue(null)`.
    pub fn null_value(&mut self) {
        self.int(VAL_NULL);
    }

    /// `writeValue` of a `Float`.
    pub fn float_value(&mut self, v: f32) {
        self.int(VAL_FLOAT);
        self.float(v);
    }

    /// `writeValue` of a `String`.
    pub fn string_value(&mut self, v: &str) {
        self.int(VAL_STRING);
        self.string(Some(v));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn array_map_orders_by_hash() {
        let mut m = ArrayMap::default();
        for k in ["b", "a", "c", "a"] {
            m.put(k, k.len());
        }
        assert_eq!(m.keys().collect::<Vec<_>>(), ["a", "b", "c"]);
        // "Aa" and "BB" share a hash: the later one goes after.
        let mut s = ArraySet::default();
        for k in ["BB", "Aa", "zz", "BB"] {
            s.add(k);
        }
        assert_eq!(java_hash("Aa"), java_hash("BB"));
        assert_eq!(s.iter().collect::<Vec<_>>(), ["BB", "Aa", "zz"]);
    }

    #[test]
    fn pools_strings_at_the_end() {
        let mut w = Writer::new();
        w.string(Some("x"));
        w.string(None);
        w.string(Some("x"));
        let e = w.finish();
        let int = |at: usize| i32::from_le_bytes(e.bytes[at..at + 4].try_into().unwrap());
        assert_eq!((int(0), int(4), int(8), int(12)), (16, 0, 1, 0));
        // Two strings: "x" (length 1, the unit and its NUL), then null.
        assert_eq!((int(16), int(20), int(28)), (2, 1, -1));
        assert_eq!(e.bytes.len(), 32);
    }
}
