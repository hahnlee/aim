//! SettingsProvider's `GenerationRegistry` at `android-16.0.0_r1`, and
//! the `MemoryIntArray`s it hands out (`android.util.MemoryIntArray`,
//! `android_util_MemoryIntArray.cpp`): per table and user (one for the
//! global settings of all users) an array of 1024 ints in shared memory,
//! with a generation for each setting a client asked to track, bumped on
//! each of its changes, so that clients keep a value while its generation
//! holds. An array is an ashmem region (`aim_ashmem::read_only_region`)
//! the app host maps writable and its clients map read-only.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, Ordering};

use super::settings::Table;
use crate::bundle::{SharedFile, Value};

/// `MemoryIntArray.MAX_SIZE`; `GenerationRegistry.MIN_NUM_BACKING_STORE`,
/// `MAX_NUM_BACKING_STORE`.
const MAX_SIZE: usize = 1024;
const MIN_NUM_BACKING_STORE: usize = 8;
const MAX_NUM_BACKING_STORE: usize = 38;
/// `DEFAULT_MAP_KEY_FOR_UNSET_SETTINGS`: the one generation of the unset
/// settings `Settings` does not declare.
pub const UNSET: &str = "";
/// `Settings.CALL_METHOD_TRACK_GENERATION_KEY`,
/// `CALL_METHOD_GENERATION_INDEX_KEY`, `CALL_METHOD_GENERATION_KEY`.
pub const TRACK_GENERATION: &str = "_track_generation";
const GENERATION_INDEX: &str = "_generation_index";
const GENERATION: &str = "_generation";

/// A `MemoryIntArray` this process owns, mapped writable.
struct MemoryIntArray {
    file: std::fs::File,
    base: *mut AtomicI32,
    len: usize,
}

// SAFETY: a shared mapping, read and written with atomic operations.
unsafe impl Send for MemoryIntArray {}
unsafe impl Sync for MemoryIntArray {}

impl MemoryIntArray {
    /// `new MemoryIntArray(size)`: `nativeCreate`'s region of `size` ints,
    /// named by a random UUID, and the owner's mapping of `nativeOpen`.
    fn new(size: usize) -> std::io::Result<MemoryIntArray> {
        let file = aim_ashmem::read_only_region(random_uuid().as_bytes())?;
        let bytes = size * size_of::<AtomicI32>();
        file.set_len(bytes as u64)?;
        // SAFETY: a shared mapping of the region, unmapped on drop.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        Ok(MemoryIntArray { file, base: base.cast(), len: size })
    }

    fn at(&self, index: usize) -> &AtomicI32 {
        assert!(index < self.len);
        // SAFETY: inside the mapping.
        unsafe { &*self.base.add(index) }
    }

    /// `nativeGet`.
    fn get(&self, index: usize) -> i32 {
        self.at(index).load(Ordering::Relaxed)
    }

    /// `nativeSet`.
    fn set(&self, index: usize, value: i32) {
        self.at(index).store(value, Ordering::Relaxed)
    }
}

impl Drop for MemoryIntArray {
    fn drop(&mut self) {
        // SAFETY: the mapping made in `new`.
        unsafe { libc::munmap(self.base.cast(), self.len * size_of::<AtomicI32>()) };
    }
}

/// `UUID.randomUUID().toString()`.
fn random_uuid() -> String {
    let mut b = [0u8; 16];
    // SAFETY: fills the local buffer.
    unsafe { libc::arc4random_buf(b.as_mut_ptr().cast(), b.len()) };
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let hex: String = b.iter().map(|b| format!("{b:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..])
}

/// A backing store and the index of each tracked name in it.
struct Store {
    array: MemoryIntArray,
    indexes: HashMap<String, usize>,
}

pub struct GenerationRegistry {
    max_stores: usize,
    stores: Mutex<HashMap<(Table, i32), Store>>,
}

impl GenerationRegistry {
    /// `new GenerationRegistry(maxNumUsers)`: stores for the secure and
    /// system settings of two users more, the global and the config ones.
    pub fn new(max_users: usize) -> GenerationRegistry {
        GenerationRegistry {
            max_stores: ((max_users + 2) * 2 + 2).clamp(MIN_NUM_BACKING_STORE, MAX_NUM_BACKING_STORE),
            stores: Mutex::new(HashMap::new()),
        }
    }

    /// The global settings are shared across users.
    fn key(table: Table, user: i32) -> (Table, i32) {
        if table == Table::Global { (Table::Global, 0) } else { (table, user) }
    }

    /// `incrementGeneration`: of a flag, its namespace's (`namespace/`).
    pub fn increment(&self, table: Table, user: i32, name: &str) {
        if table == Table::Config {
            let namespace = name.split('/').next().unwrap_or_default();
            self.increment_key(table, user, &format!("{namespace}/"));
        } else {
            self.increment_key(table, user, name);
        }
    }

    /// `incrementGenerationForUnsetSettings`: a setting `Settings` does not
    /// declare was added.
    pub fn increment_unset(&self, table: Table, user: i32) {
        if table != Table::Config {
            self.increment_key(table, user, UNSET);
        }
    }

    /// `incrementGenerationInternal`: only a tracked name has a generation.
    fn increment_key(&self, table: Table, user: i32, key: &str) {
        let stores = self.stores.lock().unwrap();
        if let Some(store) = stores.get(&Self::key(table, user))
            && let Some(&index) = store.indexes.get(key)
        {
            store.array.set(index, store.array.get(index).wrapping_add(1));
        }
    }

    /// `addGenerationData`: the array, the index of `key` in it (from 1
    /// when new) and its generation, as a reply carries them; none when no
    /// store or index is left.
    pub fn generation_data(&self, table: Table, user: i32, key: &str) -> Vec<(String, Value)> {
        let mut stores = self.stores.lock().unwrap();
        let count = stores.len();
        let store = match stores.entry(Self::key(table, user)) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(_) if count >= self.max_stores => return Vec::new(),
            std::collections::hash_map::Entry::Vacant(e) => match MemoryIntArray::new(MAX_SIZE) {
                Ok(array) => e.insert(Store { array, indexes: HashMap::new() }),
                Err(err) => {
                    eprintln!("app host: settings: error creating generation tracker: {err}");
                    return Vec::new();
                }
            },
        };
        let index = match store.indexes.get(key) {
            Some(&index) => index,
            // `findNextEmptyIndex`.
            None => match (0..store.array.len).find(|&i| store.array.get(i) == 0) {
                Some(index) => {
                    store.array.set(index, 1);
                    store.indexes.insert(key.to_string(), index);
                    index
                }
                None => return Vec::new(),
            },
        };
        let Some(file) = aim_binder_host::server::file_from_fd(store.array.file.as_fd()) else {
            eprintln!("app host: settings: no fileport for the generation tracker");
            return Vec::new();
        };
        vec![
            (TRACK_GENERATION.to_string(), Value::MemoryIntArray(SharedFile(file))),
            (GENERATION_INDEX.to_string(), Value::Int(index as i32)),
            (GENERATION.to_string(), Value::Int(store.array.get(index))),
        ]
    }

    #[cfg(test)]
    pub(super) fn generation(&self, table: Table, user: i32, key: &str) -> Option<i32> {
        let stores = self.stores.lock().unwrap();
        let store = stores.get(&Self::key(table, user))?;
        Some(store.array.get(*store.indexes.get(key)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `GenerationRegistryTest`: a tracked name starts at 1 and is bumped
    /// by its changes; the global settings share one store; a flag's
    /// namespace is tracked.
    #[test]
    fn generations_are_tracked_and_bumped() {
        let registry = GenerationRegistry::new(4);
        assert_eq!(registry.max_stores, 14);
        registry.increment(Table::Secure, 0, "a");
        assert_eq!(registry.generation(Table::Secure, 0, "a"), None);
        let data = registry.generation_data(Table::Secure, 0, "a");
        assert_eq!(data[1], (GENERATION_INDEX.to_string(), Value::Int(0)));
        assert_eq!(data[2], (GENERATION.to_string(), Value::Int(1)));
        registry.increment(Table::Secure, 0, "a");
        registry.increment(Table::Secure, 0, "b");
        assert_eq!(registry.generation(Table::Secure, 0, "a"), Some(2));
        assert_eq!(registry.generation_data(Table::Secure, 0, "b")[1].1, Value::Int(1));
        registry.generation_data(Table::Global, 10, "g");
        registry.increment(Table::Global, 0, "g");
        assert_eq!(registry.generation(Table::Global, 10, "g"), Some(2));
        registry.generation_data(Table::Config, 0, "ns/");
        registry.increment(Table::Config, 0, "ns/flag");
        assert_eq!(registry.generation(Table::Config, 0, "ns/"), Some(2));
        registry.generation_data(Table::System, 0, UNSET);
        registry.increment_unset(Table::System, 0);
        assert_eq!(registry.generation(Table::System, 0, UNSET), Some(2));
        assert_eq!(GenerationRegistry::new(40).max_stores, MAX_NUM_BACKING_STORE);
        assert_eq!(GenerationRegistry::new(1).max_stores, MIN_NUM_BACKING_STORE);
    }

    #[test]
    fn stores_are_limited() {
        let registry = GenerationRegistry::new(1);
        for user in 0..MIN_NUM_BACKING_STORE as i32 {
            assert_eq!(registry.generation_data(Table::System, user, "a").len(), 3);
        }
        assert!(registry.generation_data(Table::Secure, 0, "a").is_empty());
    }

    #[test]
    fn region_is_the_shared_array() {
        let array = MemoryIntArray::new(MAX_SIZE).unwrap();
        array.set(3, 42);
        assert_eq!(array.file.metadata().unwrap().len(), 4096);
        let mut bytes = [0u8; 4];
        std::os::unix::fs::FileExt::read_exact_at(&array.file, &mut bytes, 12).unwrap();
        assert_eq!(i32::from_le_bytes(bytes), 42);
        assert_eq!(random_uuid().len(), 36);
    }
}
