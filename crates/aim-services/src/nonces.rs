//! The framework's cache nonces in ActivityManager's
//! `ApplicationSharedMemory`, mapped read-only as an app maps it
//! (`com_android_internal_os_ApplicationSharedMemory.cpp`,
//! `android_app_PropertyInvalidatedCache.h` and `PropertyInvalidatedCache`'s
//! `NonceStore` at the pinned tag). system_server bumps a nonce after every
//! change of what the caches keyed by it hold, so a value kept with a
//! nonce is current while the nonce is unchanged (#497).
//!
//! The memory is `SharedMemory`: an `int64_t` network time, the
//! `SystemFeaturesCache` (512 `int32_t` and an `int64_t` length), then the
//! system server's `NonceStore`: its header (the nonce count, the byte
//! block's size, their offsets from the header, the block's hash), the
//! `int64_t` nonces and the byte block, the nonces' names, each a length
//! byte and its bytes, in handle order.

use std::os::fd::AsRawFd;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicI64, Ordering};

/// Where the `NonceStore` starts.
const STORE: usize = 8 + 512 * 4 + 8;
/// `NonceStore`'s header.
const HEADER: usize = 24;
/// `PropertyInvalidatedCache`'s reserved nonces: `NONCE_UNSET`,
/// `NONCE_DISABLED`, `NONCE_CORKED`, `NONCE_BYPASS`.
const RESERVED: std::ops::RangeInclusive<i64> = 0..=3;

pub struct Nonces {
    base: *const u8,
    len: usize,
    count: usize,
    nonces: usize,
    block: usize,
    block_len: usize,
    /// Handles found so far: a name's handle never changes.
    handles: Mutex<Vec<(String, usize)>>,
}

// SAFETY: a read-only shared mapping, read with atomic loads or copied.
unsafe impl Send for Nonces {}
unsafe impl Sync for Nonces {}

impl Nonces {
    /// Maps `file` read-only and checks the store's layout.
    pub fn map(file: &aim_binder_driver::File) -> Option<Self> {
        let fd = aim_binder_host::server::file_fd(file)?;
        // SAFETY: fstat and a read-only shared mapping of a file we hold
        // open; the mapping outlives the fd.
        let (base, len) = unsafe {
            let mut st: libc::stat = std::mem::zeroed();
            if libc::fstat(fd.as_raw_fd(), &mut st) != 0 {
                return None;
            }
            let len = st.st_size as usize;
            let p = libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            );
            if p == libc::MAP_FAILED {
                return None;
            }
            (p as *const u8, len)
        };
        let mut nonces = Nonces {
            base,
            len,
            count: 0,
            nonces: 0,
            block: 0,
            block_len: 0,
            handles: Mutex::new(Vec::new()),
        };
        let header = |i: usize| nonces.i32_at(STORE + 4 * i).map(|v| v as usize);
        let (count, block_len) = (header(0)?, header(1)?);
        let (nonces_at, block_at) = (STORE + header(2)?, STORE + header(3)?);
        let fits = nonces_at >= STORE + HEADER
            && nonces_at % 8 == 0
            && nonces_at + 8 * count <= block_at
            && block_at + block_len <= len;
        if !fits {
            return None;
        }
        (nonces.count, nonces.nonces) = (count, nonces_at);
        (nonces.block, nonces.block_len) = (block_at, block_len);
        Some(nonces)
    }

    fn i32_at(&self, at: usize) -> Option<i32> {
        // SAFETY: inside the mapping and aligned.
        (at + 4 <= self.len && at % 4 == 0)
            .then(|| unsafe { (*(self.base.add(at) as *const AtomicI32)).load(Ordering::Acquire) })
    }

    /// The nonce named `name` (a key without `cache_key.system_server.`),
    /// or `None` while it is unset or reserved: the caches it keys are
    /// then bypassed.
    pub fn get(&self, name: &str) -> Option<i64> {
        let handle = self.handle(name)?;
        // SAFETY: inside the nonce array (`map` checked it), aligned.
        let nonce = unsafe {
            (*(self.base.add(self.nonces + 8 * handle) as *const AtomicI64)).load(Ordering::Acquire)
        };
        (!RESERVED.contains(&nonce)).then_some(nonce)
    }

    /// `NonceStore.getHandleForName`: the name's index in the byte block,
    /// read when its hash matches (else it is being written).
    fn handle(&self, name: &str) -> Option<usize> {
        let mut handles = self.handles.lock().unwrap();
        if let Some((_, handle)) = handles.iter().find(|(n, _)| n == name) {
            return Some(*handle);
        }
        let hash = self.i32_at(STORE + 16)?;
        let mut block = vec![0u8; self.block_len];
        // SAFETY: inside the mapping (`map` checked it).
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.base.add(self.block),
                block.as_mut_ptr(),
                self.block_len,
            )
        };
        std::sync::atomic::fence(Ordering::Acquire);
        if java_hash(&block) != hash {
            return None;
        }
        let handle = names(&block)
            .take(self.count)
            .position(|n| n == name.as_bytes())?;
        handles.push((name.to_string(), handle));
        Some(handle)
    }
}

impl Drop for Nonces {
    fn drop(&mut self) {
        // SAFETY: the mapping made in `map`.
        unsafe { libc::munmap(self.base as *mut _, self.len) };
    }
}

/// `Arrays.hashCode(byte[])`.
fn java_hash(bytes: &[u8]) -> i32 {
    bytes.iter().fold(1i32, |h, &b| {
        h.wrapping_mul(31).wrapping_add(b as i8 as i32)
    })
}

/// The names of a byte block, in handle order.
fn names(block: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut at = 0;
    std::iter::from_fn(move || {
        let len = *block.get(at).filter(|&&l| l != 0)? as usize;
        let name = block.get(at + 1..at + 1 + len)?;
        at += 1 + len;
        Some(name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_and_names() {
        assert_eq!(java_hash(&[]), 1);
        // Arrays.hashCode(new byte[] {1, -1}) == (31 + 1) * 31 - 1
        assert_eq!(java_hash(&[1, 0xff]), 991);
        let block = b"\x03abc\x12package_info_cache\x00\x00";
        let found: Vec<&[u8]> = names(block).collect();
        assert_eq!(found, [&b"abc"[..], b"package_info_cache"]);
    }
}
