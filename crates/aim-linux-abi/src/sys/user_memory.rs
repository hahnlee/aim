//! Fault-contained kernel copies of guest user buffers. Verified lazy mappings
//! are resolved by their pager owner before any Mach VM copy touches them.
use super::verity_pager::Access;
use crate::errno::{self, Errno};

pub const PATH_MAX: usize = 4096;
const ENAMETOOLONG: Errno = 36;
const PAGE: u64 = super::mem::PAGE;

unsafe extern "C" {
    fn mach_vm_read_overwrite(
        task: u32,
        address: u64,
        length: u64,
        output: u64,
        actual: *mut u64,
    ) -> i32;
    fn mach_vm_write(task: u32, address: u64, data: usize, length: u32) -> i32;
}

fn range(address: u64, length: usize) -> Result<u64, Errno> {
    if length == 0 {
        return Ok(address);
    }
    if address == 0 {
        return Err(errno::EFAULT);
    }
    address.checked_add(length as u64).ok_or(errno::EFAULT)
}

fn prepare(address: u64, length: usize, access: Access) -> Result<(), Errno> {
    let end = range(address, length)?;
    if length == 0 {
        return Ok(());
    }
    super::verity_pager::prepare_user_range(address, length as u64, access)?;
    let required = match access {
        Access::Read => libc::PROT_READ,
        Access::Write => libc::PROT_WRITE,
        Access::Execute => libc::PROT_EXEC,
    };
    let mut at = address;
    while at < end {
        let (start, stop, protection, _) = crate::patch::vm::region(at).ok_or(errno::EFAULT)?;
        if start > at || stop <= at || protection & required == 0 {
            return Err(errno::EFAULT);
        }
        at = stop.min(end);
    }
    Ok(())
}

/// Prepare a readable user range before acquiring descriptor/I/O locks.
pub(super) fn prepare_read(address: u64, length: usize) -> Result<(), Errno> {
    prepare(address, length, Access::Read)
}

/// Prepare a writable user range before acquiring descriptor/I/O locks.
pub(super) fn prepare_write(address: u64, length: usize) -> Result<(), Errno> {
    prepare(address, length, Access::Write)
}

pub fn read_exact(address: u64, length: usize) -> Result<Vec<u8>, Errno> {
    prepare(address, length, Access::Read)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|_| errno::ENOMEM)?;
    bytes.resize(length, 0);
    if length == 0 {
        return Ok(bytes);
    }
    let mut actual = 0;
    // SAFETY: Mach copies into our owned buffer and contains invalid user
    // addresses. Mapping/protection races are reported as EFAULT, not a signal.
    let result = unsafe {
        mach_vm_read_overwrite(
            crate::patch::vm::task(),
            address,
            length as u64,
            bytes.as_mut_ptr() as u64,
            &mut actual,
        )
    };
    if result != 0 || actual != length as u64 {
        return Err(errno::EFAULT);
    }
    Ok(bytes)
}

pub fn write_exact(address: u64, bytes: &[u8]) -> Result<(), Errno> {
    prepare(address, bytes.len(), Access::Write)?;
    let mut done = 0;
    while done < bytes.len() {
        let count = (bytes.len() - done).min(u32::MAX as usize);
        // SAFETY: source is our live slice; Mach validates the user destination.
        let result = unsafe {
            mach_vm_write(
                crate::patch::vm::task(),
                address + done as u64,
                bytes[done..].as_ptr() as usize,
                count as u32,
            )
        };
        if result != 0 {
            return Err(errno::EFAULT);
        }
        done += count;
    }
    Ok(())
}

/// Copy a NUL-terminated string, excluding its terminator. `max` counts the
/// terminator, as Linux pathname limits do. Never probe a page past the NUL.
pub fn read_cstr(address: u64, max: usize) -> Result<Vec<u8>, Errno> {
    if address == 0 {
        return Err(errno::EFAULT);
    }
    let mut result = Vec::new();
    while result.len() < max {
        let at = address
            .checked_add(result.len() as u64)
            .ok_or(errno::EFAULT)?;
        let count = ((PAGE - at % PAGE) as usize).min(max - result.len());
        let chunk = read_exact(at, count)?;
        if let Some(end) = chunk.iter().position(|byte| *byte == 0) {
            result.try_reserve(end).map_err(|_| errno::ENOMEM)?;
            result.extend_from_slice(&chunk[..end]);
            return Ok(result);
        }
        result.try_reserve(chunk.len()).map_err(|_| errno::ENOMEM)?;
        result.extend_from_slice(&chunk);
    }
    Err(ENAMETOOLONG)
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Mapping(*mut libc::c_void);
    impl Mapping {
        fn new() -> Self {
            let address = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    (2 * PAGE) as usize,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANON,
                    -1,
                    0,
                )
            };
            assert_ne!(address, libc::MAP_FAILED);
            Self(address)
        }
        fn address(&self) -> u64 {
            self.0 as u64
        }
    }
    impl Drop for Mapping {
        fn drop(&mut self) {
            assert_eq!(unsafe { libc::munmap(self.0, (2 * PAGE) as usize) }, 0);
        }
    }

    #[test]
    fn copies_owned_bytes_and_contains_bad_addresses_and_permissions() {
        let mapping = Mapping::new();
        let address = mapping.address();
        write_exact(address, b"guest\0").unwrap();
        assert_eq!(read_exact(address, 6).unwrap(), b"guest\0");
        assert_eq!(read_cstr(address, 6).unwrap(), b"guest");
        assert_eq!(read_cstr(address, 5), Err(ENAMETOOLONG));
        assert_eq!(read_exact(0, 1), Err(errno::EFAULT));
        assert_eq!(read_cstr(0, PATH_MAX), Err(errno::EFAULT));
        assert_eq!(read_exact(u64::MAX, 2), Err(errno::EFAULT));
        assert_eq!(read_exact(0, 0).unwrap(), b"");
        assert_eq!(
            unsafe { libc::mprotect(mapping.0, PAGE as usize, libc::PROT_READ) },
            0
        );
        assert_eq!(write_exact(address, b"changed"), Err(errno::EFAULT));
        assert_eq!(read_cstr(address, 6).unwrap(), b"guest");
    }

    #[test]
    fn nul_before_unreadable_next_page_stops_without_touching_it() {
        let mapping = Mapping::new();
        let address = mapping.address();
        let end = address + PAGE - 4;
        write_exact(end, b"end\0").unwrap();
        assert_eq!(
            unsafe { libc::mprotect((address + PAGE) as *mut _, PAGE as usize, libc::PROT_NONE) },
            0
        );
        assert_eq!(read_cstr(end, PATH_MAX).unwrap(), b"end");
        assert_eq!(read_exact(end, 5), Err(errno::EFAULT));
        write_exact(end + 3, b"!").unwrap();
        assert_eq!(read_cstr(end, PATH_MAX), Err(errno::EFAULT));
    }
    #[test]
    fn cold_verified_rodata_is_paged_before_copy_and_corruption_exposes_no_bytes() {
        use super::super::verity_pager::{self, MappingCache, Source};
        use aim_storage::{
            fsverity::{BuildOptions, Store},
            private_fd::PrivateFd,
        };
        use std::{
            fs::{self, File},
            os::unix::fs::FileExt,
            sync::Arc,
        };
        let root = std::env::temp_dir().join(format!(
            "aim-user-copy-proof-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let mut bytes = vec![0; (2 * PAGE) as usize];
        bytes[128..141].copy_from_slice(b"cold-rodata!\0");
        bytes[PAGE as usize - 4..PAGE as usize].copy_from_slice(b"tail");
        fs::write(root.join("original"), &bytes).unwrap();
        let data = File::open(root.join("original")).unwrap();
        let store = Store::new(&root.join("proof"), &root.join("runtime")).unwrap();
        let admission = store.lock_inode(&data).unwrap();
        let guard = admission.begin_enable().unwrap();
        drop(admission);
        let prepared = guard
            .build(
                BuildOptions::new(1, 4096, vec![], PAGE, 4096).unwrap(),
                &[],
                || false,
            )
            .unwrap();
        let proof = Arc::new(guard.commit(prepared).unwrap());
        let source = Arc::new(Source::original(
            PrivateFd::adopt(data.into()).unwrap(),
            Some(proof),
            None,
            Arc::new(MappingCache::new(&root.join("cache")).unwrap()),
            None,
        ));
        let memory = Mapping::new();
        let address = memory.address();
        assert_eq!(
            unsafe { libc::mprotect(memory.0, (2 * PAGE) as usize, libc::PROT_NONE) },
            0
        );
        let registration =
            verity_pager::register(address, 2 * PAGE, 0, libc::PROT_READ, false, source.clone())
                .unwrap();
        assert_eq!(
            crate::patch::vm::region(address).unwrap().2 & libc::PROT_READ,
            0
        );
        assert_eq!(read_cstr(address + 128, PATH_MAX).unwrap(), b"cold-rodata!");
        assert_ne!(
            crate::patch::vm::region(address).unwrap().2 & libc::PROT_READ,
            0
        );
        assert_eq!(write_exact(address + 128, b"overwrite"), Err(errno::EFAULT));
        File::options()
            .write(true)
            .open(root.join("original"))
            .unwrap()
            .write_all_at(&[99], PAGE + 20)
            .unwrap();
        // Both APIs return only the error, even though the first page is valid.
        assert_eq!(read_exact(address + PAGE - 4, 8), Err(errno::EIO));
        assert_eq!(read_cstr(address + PAGE - 4, PATH_MAX), Err(errno::EIO));
        assert_eq!(
            crate::patch::vm::region(address + PAGE).unwrap().2 & libc::PROT_READ,
            0
        );
        assert_eq!(read_cstr(address + 128, PATH_MAX).unwrap(), b"cold-rodata!");
        drop(registration);
        drop(memory);
        drop(source);
        fs::remove_dir_all(root).unwrap();
    }
}
