//! Property storage and stable opaque handles, separate from environment/auxv.
//! This is process-local storage; property-service permissions and transport
//! belong to the system service and are not granted by this module.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
#[path = "property_wait.rs"]
mod wait;

pub(crate) const VALUE_MAX: usize = 92;

struct Entry {
    name: Arc<[u8]>,
    value: Arc<[u8]>,
    generation: u32,
}

#[derive(Clone)]
pub(crate) struct ReadValue {
    pub name: Arc<[u8]>,
    pub value: Arc<[u8]>,
    pub serial: u32,
}

pub(crate) struct PropertyArea {
    entries: RwLock<BTreeMap<Vec<u8>, Box<Entry>>>,
    serial: AtomicU32,
    changes: Mutex<(u64, bool)>, // activation epoch, closed
    changed: Condvar,
}

fn terminated(value: &[u8]) -> Arc<[u8]> {
    let mut bytes = value.to_vec();
    bytes.push(0);
    bytes.into()
}

fn validate(name: &[u8], value: &[u8]) -> Result<(), &'static str> {
    if name.is_empty() || name.contains(&0) {
        return Err("invalid property name");
    }
    if value.contains(&0) || value.len() >= VALUE_MAX {
        return Err("invalid property value");
    }
    Ok(())
}

impl PropertyArea {
    pub fn new(properties: Vec<(Vec<u8>, Vec<u8>)>) -> Result<Self, &'static str> {
        let mut entries = BTreeMap::new();
        for (name, value) in properties {
            validate(&name, &value)?;
            let entry = Box::new(Entry {
                name: terminated(&name),
                value: terminated(&value),
                generation: 0,
            });
            if entries.insert(name, entry).is_some() {
                return Err("duplicate property name");
            }
        }
        Ok(Self {
            changes: Mutex::new((0, false)),
            changed: Condvar::new(),
            serial: AtomicU32::new(entries.len() as u32),
            entries: RwLock::new(entries),
        })
    }

    pub fn find(&self, name: &[u8]) -> Result<*const c_void, &'static str> {
        let entries = self.entries.read().map_err(|_| "property lock poisoned")?;
        Ok(entries
            .get(name)
            .map_or(std::ptr::null(), |entry| (&**entry as *const Entry).cast()))
    }

    fn copy(entry: &Entry) -> ReadValue {
        ReadValue {
            name: entry.name.clone(),
            value: entry.value.clone(),
            serial: (((entry.value.len() - 1) as u32) << 24) | entry.generation,
        }
    }

    pub fn area_serial(&self) -> u32 {
        self.serial.load(Ordering::Acquire)
    }

    pub fn serial(&self, token: *const c_void) -> Result<Option<u32>, &'static str> {
        let entries = self.entries.read().map_err(|_| "property lock poisoned")?;
        Ok(entries
            .values()
            .find(|entry| std::ptr::eq((&***entry as *const Entry).cast::<c_void>(), token))
            .map(|entry| (((entry.value.len() - 1) as u32) << 24) | entry.generation))
    }

    pub fn get(&self, name: &[u8]) -> Result<Option<ReadValue>, &'static str> {
        let entries = self.entries.read().map_err(|_| "property lock poisoned")?;
        Ok(entries.get(name).map(|entry| Self::copy(entry)))
    }

    pub fn read(&self, token: *const c_void) -> Result<Option<ReadValue>, &'static str> {
        let entries = self.entries.read().map_err(|_| "property lock poisoned")?;
        Ok(entries
            .values()
            .find(|entry| std::ptr::eq((&***entry as *const Entry).cast::<c_void>(), token))
            .map(|entry| Self::copy(entry)))
    }

    /// Return stable opaque handles for the entries currently owned by this
    /// area.  The read lock is intentionally released before callers invoke
    /// guest callbacks: callbacks may re-enter find/read/foreach, and each
    /// `Entry` is boxed so its address remains stable across value updates.
    pub fn tokens(&self) -> Result<Vec<*const c_void>, &'static str> {
        let entries = self.entries.read().map_err(|_| "property lock poisoned")?;
        Ok(entries
            .values()
            .map(|entry| (&**entry as *const Entry).cast::<c_void>())
            .collect())
    }

    // Only an authorized property-service update may call this. This API is
    // deliberately not exposed as an unprivileged native setter.
    pub fn update(&self, name: &[u8], value: &[u8]) -> Result<(), &'static str> {
        validate(name, value)?;
        // Same order as wait: notification lock, then entry lock. Holding
        // this through publication prevents a check-to-sleep lost wakeup.
        let changes = self
            .changes
            .lock()
            .map_err(|_| "property wait lock poisoned")?;
        if changes.1 {
            return Err("property area closed");
        }
        let mut entries = self.entries.write().map_err(|_| "property lock poisoned")?;
        if let Some(entry) = entries.get_mut(name) {
            if name.starts_with(b"ro.") {
                return Err("read-only property already initialized");
            }
            entry.value = terminated(value);
            entry.generation = entry.generation.wrapping_add(2) & 0x00ff_fffe;
        } else {
            entries.insert(
                name.to_vec(),
                Box::new(Entry {
                    name: terminated(name),
                    value: terminated(value),
                    generation: 0,
                }),
            );
        }
        self.serial.fetch_add(1, Ordering::Release);
        self.changed.notify_all();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_handle_and_versioned_snapshot() {
        let area = PropertyArea::new(vec![(b"cache.key".to_vec(), b"first".to_vec())]).unwrap();
        let token = area.find(b"cache.key").unwrap();
        let before = area.read(token).unwrap().unwrap();
        for i in 0..100 {
            area.update(format!("other.{i}").as_bytes(), b"x").unwrap();
        }
        area.update(b"cache.key", b"second").unwrap();
        assert_eq!(token, area.find(b"cache.key").unwrap());
        let after = area.read(token).unwrap().unwrap();
        assert_eq!(&*before.value, b"first\0");
        assert_eq!(&*after.value, b"second\0");
        assert_ne!(before.serial, after.serial);
        assert_eq!(after.serial & 1, 0);
        assert!(area.read(std::ptr::null()).unwrap().is_none());
        let other = PropertyArea::new(vec![]).unwrap();
        assert!(other.read(token).unwrap().is_none());
    }

    #[test]
    fn concurrent_reads_are_coherent() {
        let area = PropertyArea::new(vec![(b"key".to_vec(), b"short".to_vec())]).unwrap();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for i in 0..1000 {
                    area.update(
                        b"key",
                        if i % 2 == 0 {
                            b"short"
                        } else {
                            b"longer-value"
                        },
                    )
                    .unwrap();
                }
            });
            for _ in 0..4 {
                scope.spawn(|| {
                    let token = area.find(b"key").unwrap();
                    for _ in 0..1000 {
                        let read = area.read(token).unwrap().unwrap();
                        assert_eq!(&*read.name, b"key\0");
                        assert!(
                            read.value.as_ref() == b"short\0"
                                || read.value.as_ref() == b"longer-value\0"
                        );
                        assert_eq!((read.serial >> 24) as usize, read.value.len() - 1);
                        assert_eq!(read.serial & 1, 0);
                    }
                });
            }
        });
    }

    #[test]
    fn initialization_and_update_validation() {
        let area = PropertyArea::new(vec![]).unwrap();
        assert_eq!(area.area_serial(), 0);
        assert!(area.find(b"missing").unwrap().is_null());
        area.update(b"ro.version", b"36").unwrap();
        assert_eq!(area.area_serial(), 1);
        assert!(area.update(b"ro.version", b"35").is_err());
        assert!(area.update(b"key", &[b'x'; VALUE_MAX]).is_err());
        assert!(area.update(b"key", b"a\0b").is_err());
        assert_eq!(area.area_serial(), 1);
        let token = area.find(b"ro.version").unwrap();
        assert_eq!(area.serial(token).unwrap(), Some(2 << 24));
        let read = area.read(token).unwrap().unwrap();
        let read_pointer = read.value.as_ptr();
        drop(read);
        // Read-only callback values are retained by CachedProperty after the
        // callback returns: their storage must remain owned by the area.
        assert_eq!(
            read_pointer,
            area.read(token).unwrap().unwrap().value.as_ptr()
        );
        assert_eq!(area.serial(std::ptr::null()).unwrap(), None);
        assert_eq!(&*area.get(b"ro.version").unwrap().unwrap().value, b"36\0");
        assert!(PropertyArea::new(vec![(b"a".to_vec(), vec![]), (b"a".to_vec(), vec![])]).is_err());
    }

    #[test]
    fn foreach_tokens_are_sorted_and_retain_entry_identity() {
        let area = PropertyArea::new(vec![
            (b"z.key".to_vec(), b"z".to_vec()),
            (b"a.key".to_vec(), b"a".to_vec()),
        ])
        .unwrap();
        let tokens = area.tokens().unwrap();
        let names = tokens
            .iter()
            .map(|token| area.read(*token).unwrap().unwrap().name.to_vec())
            .collect::<Vec<_>>();
        assert_eq!(names, vec![b"a.key\0".to_vec(), b"z.key\0".to_vec()]);
        area.update(b"a.key", b"updated").unwrap();
        assert_eq!(tokens[0], area.find(b"a.key").unwrap());
        assert_eq!(
            area.read(tokens[0]).unwrap().unwrap().value.as_ref(),
            b"updated\0"
        );
    }
}
