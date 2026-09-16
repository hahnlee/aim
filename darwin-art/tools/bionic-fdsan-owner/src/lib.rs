//! Process-local FDSAN descriptor tags. Sparse storage supports opaque guest FD
//! tokens without allocating an array indexed by their large numeric value.
use std::collections::BTreeMap;
mod error_level;
mod process_store;

#[derive(Default)]
struct Tags(BTreeMap<i32, u64>);
impl Tags {
    fn get(&self, fd: i32) -> u64 {
        self.0.get(&fd).copied().unwrap_or(0)
    }
    fn exchange(&mut self, fd: i32, expected: u64, new: u64) -> Result<(), u64> {
        if fd < 0 {
            return Ok(());
        } // AOSP GetFdEntry ignores negative descriptors.
        let actual = self.0.get(&fd).copied().unwrap_or(0);
        if actual != expected {
            return Err(actual);
        }
        if new == 0 {
            self.0.remove(&fd);
        } else {
            self.0.insert(fd, new);
        }
        Ok(())
    }
}
/// Register host fork coordination at process startup, before guest threads run.
#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_fdsan_initialize() {
    process_store::initialize();
}

/// Read the same process-owned tag used by exchange and close. Missing and
/// negative descriptors are unowned; querying does not create an entry.
#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_fdsan_get_owner_tag(fd: i32) -> u64 {
    process_store::with_tags(|tags| tags.get(fd))
}

/// # Safety
/// `actual` points to writable storage. 0 exchanged/negative-fd ignored,
/// 1 ownership mismatch (unchanged), -1 invalid output. Diagnostics and the
/// actual close run after releasing the tag mutex, as in AOSP's atomic CAS.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_fdsan_exchange(
    fd: i32,
    expected: u64,
    new: u64,
    actual: *mut u64,
) -> i32 {
    if actual.is_null() {
        return -1;
    }
    unsafe {
        *actual = 0;
    }
    match process_store::with_tags(|tags| tags.exchange(fd, expected, new)) {
        Ok(()) => 0,
        Err(value) => {
            unsafe {
                *actual = value;
            }
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    #[test]
    fn querying_preserves_tags_and_does_not_allocate_missing_entries() {
        let mut tags = Tags::default();
        assert_eq!(tags.get(-1), 0);
        assert_eq!(tags.get(i32::MAX), 0);
        assert!(tags.0.is_empty());
        let owner = 0xff12_3456_789a_bcde;
        tags.exchange(0x40000041, 0, owner).unwrap();
        assert_eq!(tags.get(0x40000041), owner);
        assert_eq!(tags.get(0x40000041), owner);
        assert_eq!(tags.0.len(), 1);
        tags.exchange(0x40000041, owner, 0).unwrap();
        assert_eq!(tags.get(0x40000041), 0);
    }
    #[test]
    fn mismatch_does_not_change_owner_and_close_clears_for_reuse() {
        let mut tags = Tags::default();
        assert_eq!(tags.exchange(0x40000001, 0, 17), Ok(()));
        assert_eq!(tags.exchange(0x40000001, 22, 0), Err(17));
        assert_eq!(tags.exchange(0x40000001, 17, 0), Ok(()));
        assert_eq!(tags.exchange(0x40000001, 0, 33), Ok(()));
        assert_eq!(tags.exchange(0x40000002, 0, 0), Ok(())); // dup starts unowned.
        assert_eq!(tags.exchange(-1, 19, 20), Ok(()));
        assert_eq!(tags.0.len(), 1);
    }
    #[test]
    fn concurrent_claim_has_exactly_one_winner() {
        let tags = std::sync::Arc::new(Mutex::new(Tags::default()));
        let workers: Vec<_> = (1..=12)
            .map(|tag| {
                let tags = tags.clone();
                std::thread::spawn(move || tags.lock().unwrap().exchange(42, 0, tag).is_ok())
            })
            .collect();
        assert_eq!(
            workers
                .into_iter()
                .map(|w| usize::from(w.join().unwrap()))
                .sum::<usize>(),
            1
        );
    }
}
