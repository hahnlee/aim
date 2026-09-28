//! Writer/reader agreement and wake semantics of the property areas.

use std::cell::RefCell;
use std::path::PathBuf;

use aim_android_init::props::area::{LONG_FLAG, LONG_LEGACY_ERROR, PROP_AREA_HEADER_SIZE};
use aim_android_init::props::{
    FutexWaker, PropAreaReader, PropertyAreas, PropertyInfoEntry, WakeTarget, build_trie,
};

#[derive(Default)]
struct RecordingWaker {
    wakes: RefCell<Vec<(String, usize)>>,
}

impl FutexWaker for RecordingWaker {
    fn wake_all(&self, target: WakeTarget<'_>) {
        self.wakes
            .borrow_mut()
            .push((target.area.to_string(), target.offset));
    }
}

fn small_info() -> Vec<u8> {
    build_trie(
        &[
            PropertyInfoEntry::new("ro.", "u:object_r:ro_prop:s0", "string", false),
            PropertyInfoEntry::new("sys.", "u:object_r:system_prop:s0", "string", false),
            PropertyInfoEntry::new("sys.flag", "u:object_r:flag_prop:s0", "bool", true),
        ],
        "u:object_r:default_prop:s0",
        "string",
    )
    .unwrap()
}

#[test]
fn many_properties_round_trip_through_bionic_reader() {
    let areas = PropertyAreas::new(small_info()).unwrap();
    let waker = RecordingWaker::default();
    let mut expected = Vec::new();
    for i in 0..600usize {
        let (name, value) = match i % 4 {
            0 => (format!("ro.item{i}.long"), "L".repeat(92 + i)),
            1 => (format!("sys.a{}.b{}.c{i}", i % 7, i % 3), format!("v{i}")),
            2 => (format!("persist.x{i}"), "y".repeat(i % 92)),
            _ => (format!("ro.short.{i}"), String::new()),
        };
        areas.add(&name, value.as_bytes(), &waker).unwrap();
        expected.push((name, value));
    }
    for (name, value) in &expected {
        let (context, _) = areas.info_area().property_info(name);
        let bytes = areas.area(context.unwrap()).unwrap().bytes();
        let reader = PropAreaReader::new(&bytes).unwrap();
        assert_eq!(&reader.get(name).unwrap().value, value, "{name}");
    }
    // foreach visits every property exactly once.
    let mut all: Vec<_> = areas.foreach().into_iter().map(|(n, _)| n).collect();
    all.sort();
    let mut names: Vec<_> = expected.iter().map(|(n, _)| n.clone()).collect();
    names.sort();
    assert_eq!(all, names);
    // Every add bumps and wakes only the global serial.
    assert_eq!(areas.area_serial(), 600);
    assert!(
        waker
            .wakes
            .borrow()
            .iter()
            .all(|w| *w == ("properties_serial".to_string(), 4))
    );
}

#[test]
fn long_values_use_the_legacy_error_slot() {
    let areas = PropertyAreas::new(small_info()).unwrap();
    let waker = RecordingWaker::default();
    let long = "x".repeat(300);
    areas.add("ro.long", long.as_bytes(), &waker).unwrap();
    let serial = areas.property_serial("ro.long").unwrap();
    assert_eq!(serial, ((LONG_LEGACY_ERROR.len() as u32) << 24) | LONG_FLAG);
    // Legacy __system_property_get sees the error string.
    let area = areas.area("u:object_r:ro_prop:s0").unwrap();
    let bytes = area.bytes();
    let reader = PropAreaReader::new(&bytes).unwrap();
    let info = reader.find("ro.long").unwrap();
    let legacy = &bytes[PROP_AREA_HEADER_SIZE + info + 4..][..LONG_LEGACY_ERROR.len()];
    assert_eq!(legacy, LONG_LEGACY_ERROR.as_bytes());
    assert_eq!(reader.get("ro.long").unwrap().value, long);
    // Mutable names may not be long.
    assert!(areas.add("sys.long", long.as_bytes(), &waker).is_err());
}

#[test]
fn update_follows_the_dirty_backup_protocol() {
    let areas = PropertyAreas::new(small_info()).unwrap();
    let waker = RecordingWaker::default();
    areas.add("sys.flag", b"false", &waker).unwrap();
    let before = areas.property_serial("sys.flag").unwrap();
    waker.wakes.borrow_mut().clear();
    areas.update("sys.flag", b"true", &waker).unwrap();
    let after = areas.property_serial("sys.flag").unwrap();
    assert_eq!(after, (4 << 24) | ((before | 1) + 1) & 0xffffff);

    let area = areas.area("u:object_r:flag_prop:s0").unwrap();
    let mut bytes = area.bytes();
    let info = PropAreaReader::new(&bytes)
        .unwrap()
        .find("sys.flag")
        .unwrap();
    // The old value (with its NUL) is in the dirty backup area after the root.
    assert_eq!(&bytes[PROP_AREA_HEADER_SIZE + 20..][..6], b"false\0");
    // Wakes: the prop_info serial word, then the global serial.
    assert_eq!(
        *waker.wakes.borrow(),
        vec![
            (
                "u:object_r:flag_prop:s0".to_string(),
                PROP_AREA_HEADER_SIZE + info
            ),
            ("properties_serial".to_string(), 4),
        ]
    );
    // A reader that observes the dirty bit reads the backup copy.
    let dirty = ((5u32) << 24) | (after & 0xffffff) | 1;
    bytes[PROP_AREA_HEADER_SIZE + info..][..4].copy_from_slice(&dirty.to_ne_bytes());
    let reader = PropAreaReader::new(&bytes).unwrap();
    assert_eq!(reader.get("sys.flag").unwrap().value, "false");
}

#[test]
fn files_are_written_read_only() {
    let areas = PropertyAreas::new(small_info()).unwrap();
    areas
        .add("sys.a", b"1", &aim_android_init::props::NoWake)
        .unwrap();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dev__properties__");
    let _ = std::fs::remove_dir_all(&dir);
    areas.write_to_dir(&dir).unwrap();
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "properties_serial",
            "property_info",
            "u:object_r:default_prop:s0",
            "u:object_r:flag_prop:s0",
            "u:object_r:ro_prop:s0",
            "u:object_r:system_prop:s0",
        ]
    );
    let area = std::fs::read(dir.join("u:object_r:system_prop:s0")).unwrap();
    assert_eq!(area.len(), 128 * 1024);
    assert_eq!(
        PropAreaReader::new(&area)
            .unwrap()
            .get("sys.a")
            .unwrap()
            .value,
        "1"
    );
    assert!(
        std::fs::metadata(dir.join("property_info"))
            .unwrap()
            .permissions()
            .readonly()
    );
}
