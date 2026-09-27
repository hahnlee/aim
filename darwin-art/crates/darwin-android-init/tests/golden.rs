//! Byte-for-byte comparisons against the upstream C++ implementation.
//!
//! The fixtures under `tests/golden` were produced by compiling the
//! unmodified android-16.0.0_r1 sources
//! (system/core/property_service/libpropertyinfoserializer and
//! libpropertyinfoparser, bionic libc/system_properties/prop_area.cpp and
//! prop_info.cpp, and bionic's `SystemProperties::Update` body) with
//! libc++ and driving them with the same inputs:
//!
//! - `property_info/<Test>.tsv`: the `PropertyInfoEntry` lists of each
//!   `TEST(propertyinfoserializer, <Test>)` in property_info_serializer_test.cpp
//!   (first line: default context and type); `<Test>.bin` is upstream
//!   `BuildTrie`'s output; `<Test>.expect` the test's `GetPropertyInfo`
//!   expectations (`*` = type not checked).
//! - `sepolicy/plat_property_contexts`: system/sepolicy private/property_contexts;
//!   `plat_property_info.bin` is `ParsePropertyInfoFile` + `BuildTrie` with
//!   init's defaults.
//! - `areas/script.tsv`: property sets applied with init's add/update/ro rules
//!   to areas created from `plat_property_info.bin`; `reference_areas.bin`
//!   holds every area whose `bytes_used` grew (plus `properties_serial`),
//!   cut after `bytes_used` (the rest of each 128 KiB file is zero).

use std::path::PathBuf;

use darwin_android_init::props::area::PROP_AREA_HEADER_SIZE;
use darwin_android_init::props::{
    NoWake, PA_SIZE, PROP_VALUE_MAX, PropAreaReader, PropertyAreas, PropertyInfoArea,
    PropertyInfoEntry, build_trie, parse_property_info_file,
};

fn golden(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(path)
}

fn read(path: &str) -> Vec<u8> {
    std::fs::read(golden(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

const UPSTREAM_TESTS: &[&str] = &[
    "TrieNodeCheck",
    "GetPropertyInfo",
    "RealProperties",
    "GetPropertyInfo_prefix_without_dot",
    "GetPropertyInfo_prefix_with_dot_vs_without",
    "GetPropertyInfo_empty_context_and_type",
];

#[test]
fn build_trie_matches_upstream_bytes_and_lookups() {
    for test in UPSTREAM_TESTS {
        let tsv = String::from_utf8(read(&format!("property_info/{test}.tsv"))).unwrap();
        let mut lines = tsv.lines();
        let defaults: Vec<&str> = lines.next().unwrap().split('\t').collect();
        let entries: Vec<PropertyInfoEntry> = lines
            .map(|line| {
                let f: Vec<&str> = line.split('\t').collect();
                PropertyInfoEntry::new(f[0], f[1], f[2], f[3] == "1")
            })
            .collect();
        let ours = build_trie(&entries, defaults[0], defaults[1]).unwrap();
        let reference = read(&format!("property_info/{test}.bin"));
        assert_eq!(ours.len(), reference.len(), "{test}: size");
        assert!(ours == reference, "{test}: bytes differ");

        let area = PropertyInfoArea::new(&ours).unwrap();
        let expect = String::from_utf8(read(&format!("property_info/{test}.expect"))).unwrap();
        for line in expect.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            let (context, type_) = area.property_info(f[0]);
            assert_eq!(context, Some(f[1]), "{test}: context of {}", f[0]);
            if f[2] != "*" {
                assert_eq!(type_, Some(f[2]), "{test}: type of {}", f[0]);
            }
        }
    }
}

#[test]
fn plat_property_contexts_match_upstream_bytes() {
    let contexts = String::from_utf8(read("sepolicy/plat_property_contexts")).unwrap();
    let mut entries = Vec::new();
    let errors = parse_property_info_file(&contexts, true, &mut entries);
    assert!(errors.is_empty(), "{errors:?}");
    let ours = build_trie(&entries, "u:object_r:default_prop:s0", "string").unwrap();
    let reference = read("sepolicy/plat_property_info.bin");
    assert_eq!(ours.len(), reference.len());
    let first_difference = ours.iter().zip(&reference).position(|(a, b)| a != b);
    assert_eq!(first_difference, None);

    let area = PropertyInfoArea::new(&ours).unwrap();
    // Lines 157, 947 and 23 of the contexts file.
    assert_eq!(
        area.property_info("ro.build.fingerprint"),
        (Some("u:object_r:fingerprint_prop:s0"), Some("string"))
    );
    assert_eq!(
        area.property_info("sys.boot_completed"),
        (Some("u:object_r:boot_status_prop:s0"), Some("bool"))
    );
    assert_eq!(
        area.property_info("sys.anything").0,
        Some("u:object_r:system_prop:s0")
    );
    assert_eq!(
        area.property_info("no.such.prefix").0,
        Some("u:object_r:default_prop:s0")
    );
}

fn reference_areas() -> Vec<(String, Vec<u8>)> {
    let bytes = read("areas/reference_areas.bin");
    let mut out = Vec::new();
    let mut at = 0;
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
    while at < bytes.len() {
        let name_len = u32_at(at);
        let name = String::from_utf8(bytes[at + 4..at + 4 + name_len].to_vec()).unwrap();
        at += 4 + name_len;
        let body_len = u32_at(at);
        out.push((name, bytes[at + 4..at + 4 + body_len].to_vec()));
        at += 4 + body_len;
    }
    out
}

#[test]
fn property_areas_match_upstream_bytes() {
    let info = read("sepolicy/plat_property_info.bin");
    let areas = PropertyAreas::new(info).unwrap();
    let script = String::from_utf8(read("areas/script.tsv")).unwrap();
    let mut expected: std::collections::BTreeMap<String, String> = Default::default();
    for line in script.lines() {
        let (name, value) = line.split_once('\t').unwrap_or((line, ""));
        // The driver's (and init's PropertySet's) rules.
        if areas.contains(name) {
            if name.starts_with("ro.") {
                continue;
            }
            areas.update(name, value.as_bytes(), &NoWake).unwrap();
        } else {
            if value.len() >= PROP_VALUE_MAX && !name.starts_with("ro.") {
                continue;
            }
            areas.add(name, value.as_bytes(), &NoWake).unwrap();
        }
        expected.insert(name.to_string(), value.to_string());
    }

    let reference = reference_areas();
    let mut compared = 0;
    for context in areas
        .contexts()
        .iter()
        .map(String::as_str)
        .chain(["properties_serial"])
    {
        let ours = if context == "properties_serial" {
            areas.serial_area().bytes()
        } else {
            areas.area(context).unwrap().bytes()
        };
        assert_eq!(ours.len(), PA_SIZE);
        match reference.iter().find(|(name, _)| name == context) {
            Some((_, body)) => {
                assert!(ours[..body.len()] == body[..], "{context}: bytes differ");
                assert!(
                    ours[body.len()..].iter().all(|b| *b == 0),
                    "{context}: tail"
                );
                compared += 1;
            }
            None => {
                // Untouched: header plus zeroed root and backup area.
                let used = u32::from_le_bytes(ours[0..4].try_into().unwrap());
                assert_eq!(used, 112, "{context}");
                assert!(ours[PROP_AREA_HEADER_SIZE..].iter().all(|b| *b == 0));
            }
        }
    }
    assert_eq!(compared, reference.len());

    // A reader built from bionic's algorithm reads back every value.
    for (name, value) in &expected {
        let (context, _) = areas.info_area().property_info(name);
        let bytes = areas.area(context.unwrap()).unwrap().bytes();
        let reader = PropAreaReader::new(&bytes).unwrap();
        let read = reader.get(name).unwrap_or_else(|| panic!("{name} missing"));
        assert_eq!(&read.value, value, "{name}");
    }
}
