//! AppOps' access history for the app host (`AttributedOp`'s last access
//! and rejection by uid state and flags, at `android-16.0.0_r1`) and the
//! parcels its queries answer with (`AppOpsManager.PackageOps`, `OpEntry`,
//! `AttributedOpEntry`, `NoteOpEvent`, `OpEventProxyInfo`).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;

use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;

use crate::bundle::{VAL_NULL, VAL_PARCELABLE, VAL_STRING, length_prefixed};

/// `AppOpsManager.UID_STATE_TOP`, `UID_STATE_CACHED`.
pub(super) const UID_STATE_TOP: i32 = 200;
pub(super) const UID_STATE_CACHED: i32 = 700;

/// `AppOpsManager.makeKey`: an event's uid state and `OP_FLAG_*`.
pub(super) fn make_key(uid_state: i32, flags: i32) -> i64 {
    ((uid_state as i64) << 31) | flags as i64
}

/// The wall clock, as `System.currentTimeMillis`.
pub(super) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// `OpEventProxyInfo`: who noted an op for the app.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ProxyInfo {
    pub uid: i32,
    pub package: Option<String>,
    pub tag: Option<String>,
    pub device_id: String,
}

impl WriteParcelable for ProxyInfo {
    fn write_to(&self, p: &mut Parcel) {
        let mut flags = 0x8;
        if self.package.is_some() {
            flags |= 0x2;
        }
        if self.tag.is_some() {
            flags |= 0x4;
        }
        p.write_i32(flags);
        p.write_i32(self.uid);
        if let Some(package) = &self.package {
            p.write_string16(Some(package));
        }
        if let Some(tag) = &self.tag {
            p.write_string16(Some(tag));
        }
        p.write_string16(Some(&self.device_id));
    }
}

/// `NoteOpEvent`: when, for how long (-1 for a note) and by whom.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct NoteEvent {
    pub time: i64,
    pub duration: i64,
    pub proxy: Option<ProxyInfo>,
}

impl WriteParcelable for NoteEvent {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(if self.proxy.is_some() { 0x4 } else { 0 });
        p.write_i64(self.time);
        p.write_i64(self.duration);
        if let Some(proxy) = &self.proxy {
            p.write_i32(1);
            proxy.write_to(p);
        }
    }
}

/// `AttributedOpEntry` of an attributed op.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct AttributedEntry {
    pub op: i32,
    pub running: bool,
    pub access: Option<BTreeMap<i64, NoteEvent>>,
    pub reject: Option<BTreeMap<i64, NoteEvent>>,
}

/// `LongSparseArrayParceling.parcel`: -1 for none, else each key and its
/// event (`writeParcelable`).
fn write_events(p: &mut Parcel, events: &Option<BTreeMap<i64, NoteEvent>>) {
    let Some(events) = events else {
        p.write_i32(-1);
        return;
    };
    p.write_i32(events.len() as i32);
    for (key, event) in events {
        p.write_i64(*key);
        p.write_string16(Some("android.app.AppOpsManager$NoteOpEvent"));
        event.write_to(p);
    }
}

impl WriteParcelable for AttributedEntry {
    fn write_to(&self, p: &mut Parcel) {
        let mut flags = 0;
        if self.running {
            flags |= 0x2;
        }
        if self.access.is_some() {
            flags |= 0x4;
        }
        if self.reject.is_some() {
            flags |= 0x8;
        }
        p.write_i32(flags);
        p.write_i32(self.op);
        write_events(p, &self.access);
        write_events(p, &self.reject);
    }
}

/// `OpEntry`: an op, its package mode and its attributions' entries, in
/// `ArrayMap` order.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct OpEntry {
    pub op: i32,
    pub mode: i32,
    pub attributed: Vec<(Option<String>, AttributedEntry)>,
}

impl WriteParcelable for OpEntry {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.op);
        p.write_i32(self.mode);
        // `writeMap`: each key and value as `writeValue` writes them.
        p.write_i32(self.attributed.len() as i32);
        for (tag, entry) in &self.attributed {
            match tag {
                Some(tag) => {
                    p.write_i32(VAL_STRING);
                    p.write_string16(Some(tag));
                }
                None => p.write_i32(VAL_NULL),
            }
            length_prefixed(p, VAL_PARCELABLE, |p| {
                p.write_string16(Some("android.app.AppOpsManager$AttributedOpEntry"));
                entry.write_to(p);
            });
        }
    }
}

/// `PackageOps`: a package (none for a uid's ops), its uid and its ops.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PackageOps {
    pub package: Option<String>,
    pub uid: i32,
    pub entries: Vec<OpEntry>,
}

impl WriteParcelable for PackageOps {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(self.package.as_deref());
        p.write_i32(self.uid);
        p.write_i32(self.entries.len() as i32);
        for entry in &self.entries {
            entry.write_to(p);
        }
    }
}

/// An attributed op's last accesses and rejections (`mAccessEvents`,
/// `mRejectEvents`), by `make_key`.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Events {
    pub access: Option<BTreeMap<i64, NoteEvent>>,
    pub reject: Option<BTreeMap<i64, NoteEvent>>,
}

impl Events {
    /// `AttributedOp.accessed`: the access replaces its key's last.
    pub fn accessed(&mut self, key: i64, event: NoteEvent) {
        self.access
            .get_or_insert_with(BTreeMap::new)
            .insert(key, event);
    }

    /// `AttributedOp.rejected`: no proxy is kept for rejections.
    pub fn rejected(&mut self, key: i64, time: i64) {
        self.reject.get_or_insert_with(BTreeMap::new).insert(
            key,
            NoteEvent {
                time,
                duration: -1,
                proxy: None,
            },
        );
    }
}

/// `ArrayMap` order: by the key's `hashCode` (a null key's is 0), then as
/// added.
pub(super) fn array_map_order<T>(entries: &mut [(Option<String>, T)]) {
    entries.sort_by_key(|(k, _)| k.as_deref().map_or(0, crate::package::info::java_hash));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A noted then rejected op's entry, parcelled as `OpEntry` writes it
    /// with its attribution's `AttributedOpEntry` in a length-prefixed
    /// value.
    #[test]
    fn an_op_entry_is_written_as_the_original() {
        let mut events = Events::default();
        let key = make_key(UID_STATE_TOP, 1);
        events.accessed(
            key,
            NoteEvent {
                time: 5,
                duration: -1,
                proxy: Some(ProxyInfo {
                    uid: 1000,
                    package: Some("android".into()),
                    tag: None,
                    device_id: "default:0".into(),
                }),
            },
        );
        events.rejected(key, 7);
        let entry = OpEntry {
            op: 26,
            mode: 3,
            attributed: vec![(
                None,
                AttributedEntry {
                    op: 26,
                    running: false,
                    access: events.access,
                    reject: events.reject,
                },
            )],
        };
        let mut p = Parcel::new();
        entry.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (26, 3));
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_i32().unwrap(), VAL_NULL);
        assert_eq!(r.read_i32().unwrap(), VAL_PARCELABLE);
        let len = r.read_i32().unwrap() as usize;
        assert_eq!(len, r.remaining());
        assert_eq!(
            r.read_string16().unwrap().as_deref(),
            Some("android.app.AppOpsManager$AttributedOpEntry")
        );
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (0xc, 26));
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.read_i64().unwrap(), (200i64 << 31) | 1);
        r.read_string16().unwrap();
        assert_eq!(r.read_i32().unwrap(), 0x4);
        assert_eq!((r.read_i64().unwrap(), r.read_i64().unwrap()), (5, -1));
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!((r.read_i32().unwrap(), r.read_i32().unwrap()), (0xa, 1000));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("android"));
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("default:0"));
        assert_eq!(r.read_i32().unwrap(), 1);
        r.read_i64().unwrap();
        r.read_string16().unwrap();
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!((r.read_i64().unwrap(), r.read_i64().unwrap()), (7, -1));
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn attributions_follow_array_map_order() {
        let mut tags = vec![
            (Some("b".to_string()), ()),
            (None, ()),
            (Some("a".into()), ()),
        ];
        array_map_order(&mut tags);
        let order: Vec<Option<&str>> = tags.iter().map(|(t, _)| t.as_deref()).collect();
        assert_eq!(order, [None, Some("a"), Some("b")]);
    }
}
