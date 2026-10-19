//! What the settings provider's DeviceConfig calls need beside the
//! settings themselves (`SettingsProvider` and `SettingsState` at
//! `android-16.0.0_r1`): the aconfig flags' defaults `SettingsState` loads
//! for the config table (`loadAconfigDefaultValues` of each partition's and
//! APEX's `etc/aconfig_flags.pb`), which stage a write of an aconfig flag;
//! the namespaces whose accesses no monitor hears
//! (`DeviceConfig.getPublicNamespaces`); and the Java collections the calls
//! exchange: a `HashMap`'s order and `hashCode`, and a `HashSet<String>` in
//! its Object Serialization form (`Parcel.writeSerializable`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::path::Path;

use super::exit_info::fields;
use crate::package::info::java_hash;

/// `SettingsState.sAconfigTextProtoFilesOnDevice`.
const PROTO_FILES: [&str; 4] =
    ["system/etc/aconfig_flags.pb", "system_ext/etc/aconfig_flags.pb", "product/etc/aconfig_flags.pb", "vendor/etc/aconfig_flags.pb"];
/// `SettingsState.APEX_DIR`, `APEX_ACONFIG_PATH_SUFFIX`.
const APEX_DIR: &str = "apex";
const APEX_ACONFIG_PATH_SUFFIX: &str = "etc/aconfig_flags.pb";
/// `DeviceConfig.PUBLIC_NAMESPACES`.
pub const PUBLIC_NAMESPACES: [&str; 8] = [
    "textclassifier",
    "runtime",
    "statsd_java",
    "statsd_java_boot",
    "selection_toolbar",
    "autofill",
    "device_policy_manager",
    "content_capture",
];
/// aconfig's `flag_state.ENABLED`, `flag_permission.READ_WRITE`.
const ENABLED: i32 = 1;
const READ_WRITE: i32 = 2;
/// `HashMap.DEFAULT_INITIAL_CAPACITY`.
const DEFAULT_INITIAL_CAPACITY: usize = 16;
/// libcore's `HashSet.serialVersionUID`.
const HASH_SET_UID: i64 = -5024744406713321676;

/// An aconfig flag's `AconfigdFlagInfo`, as far as the provider reads it.
#[derive(Clone, Debug, PartialEq)]
pub struct FlagInfo {
    pub namespace: String,
    pub read_write: bool,
}

/// `SettingsState.mNamespaceDefaults` and `mAconfigDefaultFlags`.
#[derive(Debug, Default)]
pub struct AconfigDefaults {
    /// Each namespace's flags (`namespace/package.flag`) and defaults.
    pub namespaces: HashMap<String, HashMap<String, String>>,
    /// Each flag (`package.flag`), as its first file declares it.
    pub flags: HashMap<String, FlagInfo>,
}

impl AconfigDefaults {
    /// `loadAconfigDefaultValuesLocked` of the partitions' files, then of
    /// the APEXes' (`listApexProtoPaths`, by name here) of the image at
    /// `root`; a file that does not read or parse is logged and skipped,
    /// as the original logs it.
    pub fn load(root: &Path) -> AconfigDefaults {
        let mut defaults = AconfigDefaults::default();
        let mut paths: Vec<_> = PROTO_FILES.iter().map(|p| root.join(p)).collect();
        if let Ok(entries) = std::fs::read_dir(root.join(APEX_DIR)) {
            let mut apexes: Vec<_> = entries
                .flatten()
                .filter(|e| !e.file_name().to_string_lossy().contains('@'))
                .map(|e| e.path().join(APEX_ACONFIG_PATH_SUFFIX))
                .filter(|p| p.exists())
                .collect();
            apexes.sort();
            paths.extend(apexes);
        }
        for path in paths {
            let Ok(bytes) = std::fs::read(&path) else { continue };
            if let Err(e) = defaults.add(&bytes) {
                eprintln!("app host: settings: failed to parse protobuf {}: {e}", path.display());
            }
        }
        defaults
    }

    /// `loadAconfigDefaultValues` of a `parsed_flags` message.
    fn add(&mut self, bytes: &[u8]) -> Result<(), String> {
        let mut parsed = Vec::new();
        fields(bytes, |number, value| {
            if number == 1 {
                // `parsed_flag`: an absent enum is its first value.
                let (mut package, mut name, mut namespace, mut state, mut permission) =
                    (String::new(), String::new(), String::new(), ENABLED, 1);
                fields(value.bytes()?, |number, value| {
                    match number {
                        1 => package = value.string()?,
                        2 => name = value.string()?,
                        3 => namespace = value.string()?,
                        6 => state = value.int()?,
                        7 => permission = value.int()?,
                        _ => {}
                    }
                    Ok(())
                })?;
                parsed.push((package, name, namespace, state, permission));
            }
            Ok(())
        })?;
        for (package, name, namespace, state, permission) in parsed {
            let full = format!("{package}.{name}");
            let value = if state == ENABLED { "true" } else { "false" };
            self.namespaces.entry(namespace.clone()).or_default().insert(format!("{namespace}/{full}"), value.into());
            self.flags.entry(full).or_insert(FlagInfo { namespace, read_write: permission == READ_WRITE });
        }
        Ok(())
    }
}

/// `HashMap.hash`: the key's hash spread to the table's low bits.
fn spread(key: &str) -> u32 {
    let h = java_hash(key) as u32;
    h ^ (h >> 16)
}

/// `HashMap.tableSizeFor`.
fn table_size_for(capacity: usize) -> usize {
    capacity.max(1).next_power_of_two()
}

/// The table a `HashMap` has once `len` keys are put, starting from
/// `capacity` (`resize` doubling past three quarters full).
fn table_size(mut capacity: usize, len: usize) -> usize {
    while len > capacity * 3 / 4 {
        capacity *= 2;
    }
    capacity
}

/// The order a `HashMap` whose table started at `capacity` iterates the
/// keys of `entries`, put in their order: by bucket, then as put (a bin
/// long enough to be a tree is not modelled).
pub fn hash_map_order<T>(entries: Vec<(String, T)>, capacity: usize) -> Vec<(String, T)> {
    let mask = table_size(capacity, entries.len()) - 1;
    let mut entries = entries;
    entries.sort_by_key(|(key, _)| spread(key) as usize & mask);
    entries
}

/// `new HashMap<>(n)`'s first table, for `n` entries
/// (`Parcel.readHashMapInternal`).
pub fn sized_capacity(n: usize) -> usize {
    table_size_for(n)
}

/// `AbstractMap.hashCode`: the sum of its entries' `key ^ value` hashes,
/// a null value's 0.
pub fn map_hash<'a>(entries: impl IntoIterator<Item = (&'a str, Option<&'a str>)>) -> i32 {
    entries.into_iter().fold(0i32, |sum, (key, value)| sum.wrapping_add(java_hash(key) ^ value.map_or(0, java_hash)))
}

/// `DataOutputStream.writeUTF`'s modified UTF-8 of `s`.
fn modified_utf8(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for c in s.encode_utf16() {
        match c {
            1..=0x7f => out.push(c as u8),
            0 | 0x80..=0x7ff => out.extend([0xc0 | (c >> 6) as u8, 0x80 | (c & 0x3f) as u8]),
            _ => out.extend([0xe0 | (c >> 12) as u8, 0x80 | ((c >> 6) & 0x3f) as u8, 0x80 | (c & 0x3f) as u8]),
        }
    }
    out
}

/// `ObjectOutputStream.writeObject` of a `HashSet<String>` made by
/// `new HashSet<>()` and an `add` of each of `names`: its class
/// descriptor, then `writeObject`'s capacity, load factor and size, and
/// each element in the set's order (`TC_STRING`, or `TC_LONGSTRING`).
pub fn serialize_hash_set(names: &[String]) -> Vec<u8> {
    let capacity = table_size(DEFAULT_INITIAL_CAPACITY, names.len());
    let ordered = hash_map_order(names.iter().map(|n| (n.clone(), ())).collect(), DEFAULT_INITIAL_CAPACITY);
    let mut out = vec![0xac, 0xed, 0x00, 0x05, 0x73, 0x72];
    let class = b"java.util.HashSet";
    out.extend((class.len() as u16).to_be_bytes());
    out.extend(class);
    out.extend(HASH_SET_UID.to_be_bytes());
    // `SC_WRITE_METHOD | SC_SERIALIZABLE`, no fields, no annotation, no
    // serializable superclass.
    out.extend([0x03, 0x00, 0x00, 0x78, 0x70]);
    out.extend([0x77, 12]);
    out.extend((capacity as i32).to_be_bytes());
    out.extend(0.75f32.to_be_bytes());
    out.extend((names.len() as i32).to_be_bytes());
    for (name, ()) in ordered {
        let utf = modified_utf8(&name);
        match u16::try_from(utf.len()) {
            Ok(len) => {
                out.push(0x74);
                out.extend(len.to_be_bytes());
            }
            Err(_) => {
                out.push(0x7c);
                out.extend((utf.len() as u64).to_be_bytes());
            }
        }
        out.extend(utf);
    }
    out.push(0x78);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `loadAconfigDefaultValues`: each flag under its namespace, its
    /// value from its state; the first file's permission kept.
    #[test]
    fn defaults_are_loaded_as_the_state_loads_them() {
        fn field(number: u8, bytes: &[u8]) -> Vec<u8> {
            let mut out = vec![number << 3 | 2, bytes.len() as u8];
            out.extend(bytes);
            out
        }
        let flag = |name: &str, state: u8, permission: u8| {
            let mut f = field(1, b"com.example");
            f.extend(field(2, name.as_bytes()));
            f.extend(field(3, b"ns"));
            f.extend([6 << 3, state, 7 << 3, permission]);
            field(1, &f)
        };
        let mut defaults = AconfigDefaults::default();
        defaults.add(&[flag("on", 1, 2), flag("off", 2, 1)].concat()).unwrap();
        defaults.add(&flag("on", 2, 1)).unwrap();
        let ns = &defaults.namespaces["ns"];
        assert_eq!(ns["ns/com.example.on"], "false");
        assert_eq!(ns["ns/com.example.off"], "false");
        assert_eq!(defaults.flags["com.example.on"], FlagInfo { namespace: "ns".into(), read_write: true });
        assert!(!defaults.flags["com.example.off"].read_write);
    }

    /// `HashMap`'s buckets: "Aa" and "BB" share a hash, so a bucket, and
    /// keep the order they were put in; the table doubles past 3/4 full.
    #[test]
    fn hash_maps_iterate_by_bucket() {
        let keys = |names: &[&str]| names.iter().map(|n| (n.to_string(), ())).collect::<Vec<_>>();
        let order: Vec<_> = hash_map_order(keys(&["c", "BB", "a", "Aa"]), 16).into_iter().map(|(k, _)| k).collect();
        assert_eq!(order, ["BB", "Aa", "a", "c"]);
        assert_eq!(table_size(1, 1), 2);
        assert_eq!(table_size(16, 13), 32);
        assert_eq!(sized_capacity(0), 1);
        assert_eq!(sized_capacity(5), 8);
    }

    /// `AbstractMap.hashCode` of {"a" -> "b", "c" -> null}: ("a" ^ "b") + "c".
    #[test]
    fn map_hashes_are_java_s() {
        assert_eq!(map_hash([("a", Some("b")), ("c", None)]), (97 ^ 98) + 99);
    }

    /// The stream `ObjectOutputStream` writes for a `HashSet` of "ns".
    #[test]
    fn hash_sets_serialize_as_object_output_stream_writes_them() {
        let mut expected = vec![0xac, 0xed, 0, 5, 0x73, 0x72, 0, 17];
        expected.extend(b"java.util.HashSet");
        expected.extend([0xba, 0x44, 0x85, 0x95, 0x96, 0xb8, 0xb7, 0x34, 3, 0, 0, 0x78, 0x70, 0x77, 12]);
        expected.extend([0, 0, 0, 16, 0x3f, 0x40, 0, 0, 0, 0, 0, 1, 0x74, 0, 2, b'n', b's', 0x78]);
        assert_eq!(serialize_hash_set(&["ns".into()]), expected);
    }
}
