//! Direct kernel copies of guest user buffers, with an arm64 exception table.
//! Only listed user-access instructions recover; pager work runs after sigreturn.
use super::verity_pager::Access;
use crate::errno::{self, Errno};

pub const PATH_MAX: usize = 4096;
const ENAMETOOLONG: Errno = 36;
const PAGE: u64 = super::mem::PAGE;

#[cfg(test)]
thread_local! {
    static BEFORE_COPY: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
fn before_copy() {
    let hook = BEFORE_COPY.with(|slot| slot.borrow_mut().take());
    if let Some(hook) = hook {
        hook();
    }
}

/// The syscall owners use negative Linux errno, optionally inside their dispatch
/// result; keep fault propagation identical for each convention.
pub(super) trait CopyError {
    fn copy_error(error: Errno) -> Self;
}
impl CopyError for i64 {
    fn copy_error(error: Errno) -> Self {
        -(error as i64)
    }
}
impl CopyError for Option<i64> {
    fn copy_error(error: Errno) -> Self {
        Some(-(error as i64))
    }
}
impl<T> CopyError for Result<T, i64> {
    fn copy_error(error: Errno) -> Self {
        Err(-(error as i64))
    }
}
impl<T> CopyError for Result<T, Errno> {
    fn copy_error(error: Errno) -> Self {
        Err(error)
    }
}

/// Copy an ABI value into owned storage.
///
/// # Safety
/// Every bit pattern of `T` must be valid (no references, bools or enums).
pub unsafe fn read_value<T: Copy>(address: u64) -> Result<T, Errno> {
    let bytes = read_exact(address, std::mem::size_of::<T>())?;
    // SAFETY: caller guarantees that arbitrary user bytes are a valid T.
    Ok(unsafe { bytes.as_ptr().cast::<T>().read_unaligned() })
}

/// Copy an initialized ABI value to user memory.
///
/// # Safety
/// All bytes of `value`, including padding, must be initialized.
pub unsafe fn write_value<T: Copy>(address: u64, value: &T) -> Result<(), Errno> {
    // SAFETY: caller guarantees initialized representation.
    let bytes = unsafe {
        std::slice::from_raw_parts((value as *const T).cast::<u8>(), std::mem::size_of::<T>())
    };
    write_exact(address, bytes)
}

/// Fault-contained transfer without forming a Rust reference to either user
/// range. Used at ABI boundaries that already carry raw byte pointers.
///
/// # Safety
/// Kernel-owned endpoints must remain live for the transfer.
pub unsafe fn copy_bytes(
    source: *const u8,
    destination: *mut u8,
    length: usize,
) -> Result<(), Errno> {
    let bytes = read_exact(source as u64, length)?;
    write_exact(destination as u64, &bytes)
}

/// Snapshot vectors before any owner consumes their data.
pub(super) fn gather(vectors: &[libc::iovec]) -> Result<Vec<u8>, Errno> {
    let mut bytes = Vec::new();
    for vector in vectors {
        let part = read_exact(vector.iov_base as u64, vector.iov_len)?;
        bytes.try_reserve(part.len()).map_err(|_| errno::ENOMEM)?;
        bytes.extend_from_slice(&part);
    }
    Ok(bytes)
}

pub(super) fn scatter(vectors: &[libc::iovec], bytes: &[u8]) -> Result<usize, Errno> {
    let mut done = 0;
    for vector in vectors {
        let count = vector.iov_len.min(bytes.len() - done);
        write_exact(vector.iov_base as u64, &bytes[done..done + count])?;
        done += count;
        if done == bytes.len() {
            break;
        }
    }
    Ok(done)
}

std::arch::global_asm!(include_str!("user_copy.S"));
std::arch::global_asm!(include_str!("user_atomic.S"));

#[repr(C)]
struct CopyResult {
    remaining: usize,
    error: usize,
}
#[repr(C)]
struct ExceptionEntry {
    pc: u64,
    retry: u64,
    fixup: u64,
}
unsafe extern "C" {
    fn aim_user_copy(source: u64, destination: u64, length: usize) -> CopyResult;
    fn aim_user_atomic(
        address: u64,
        operation: u32,
        expected: u32,
        value: u32,
        output: *mut u32,
    ) -> i32;
    fn aim_user_pager();
    static aim_user_copy_ex_table: [ExceptionEntry; 4];
    static aim_user_atomic_ex_table: [ExceptionEntry; 4];
}

struct UserFaultSlot {
    busy: std::sync::atomic::AtomicBool,
    intent: std::cell::UnsafeCell<Option<super::verity_pager::FaultIntent>>,
}
thread_local! {
    static USER_FAULT: UserFaultSlot = const { UserFaultSlot {
        busy: std::sync::atomic::AtomicBool::new(false), intent: std::cell::UnsafeCell::new(None),
    } };
}

pub(super) fn initialize_fault_slot() {
    // Register the TLS destructor on the ordinary host stack, before a fault.
    USER_FAULT.with(|_| {});
}

/// After resident JIT handling, defer a copy's pager work to its saved host
/// stack before returning through its fixup. Never recover an arbitrary Rust access,
/// including the atomic routine's stores to its kernel-owned output.
pub(super) fn recover_fault(context: &mut super::sigframe::DarwinMcontext, si_addr: u64) -> bool {
    // SAFETY: immutable tables emitted beside the assembly access instructions.
    let entry = unsafe {
        aim_user_copy_ex_table
            .iter()
            .chain(aim_user_atomic_ex_table.iter())
    }
    .find(|entry| entry.pc == context.pc);
    let Some(entry) = entry else {
        return false;
    };
    let ec = context.esr >> 26;
    let address = if matches!(ec, 0x24 | 0x25) {
        context.far
    } else {
        si_addr
    };
    let access = if context.esr & (1 << 6) != 0 {
        Access::Write
    } else {
        Access::Read
    };
    let deferred = USER_FAULT.with(|slot| {
        if slot.busy.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        let Some(intent) = super::verity_pager::fault_intent(address, access) else {
            return false;
        };
        slot.busy.store(true, std::sync::atomic::Ordering::SeqCst);
        // SAFETY: only this thread uses the slot; busy excludes nested faults.
        unsafe { *slot.intent.get() = Some(intent) };
        true
    });
    context.x[7] = entry.retry;
    context.x[8] = entry.fixup;
    context.x[9] = errno::EFAULT as u64;
    context.pc = if deferred {
        aim_user_pager as *const () as u64
    } else {
        entry.fixup
    };
    true
}

/// Called by the assembly continuation after sigreturn, with the copy's live
/// registers saved on its interrupted host stack. Retry restarts an exclusive
/// sequence at LDXR, since signal delivery clears the exclusive monitor.
#[unsafe(no_mangle)]
extern "C" fn aim_resolve_user_fault() -> i32 {
    let intent = USER_FAULT.with(|slot| unsafe { (*slot.intent.get()).take().unwrap() });
    let address = intent.address();
    let result = match intent.jit_request() {
        Ok(Some(request)) => {
            drop(intent);
            super::verity_pager::resolve_jit(request)
        }
        Ok(None) => {
            let result = super::verity_pager::resolve(&intent);
            drop(intent);
            result
        }
        Err(_) => {
            drop(intent);
            super::verity_pager::FaultResult::Bus { address }
        }
    };
    USER_FAULT.with(|slot| slot.busy.store(false, std::sync::atomic::Ordering::SeqCst));
    match result {
        super::verity_pager::FaultResult::Verified | super::verity_pager::FaultResult::Retry => 0,
        super::verity_pager::FaultResult::Bus { .. } => errno::EIO,
        super::verity_pager::FaultResult::Permission
        | super::verity_pager::FaultResult::Unowned => errno::EFAULT,
    }
}

fn transfer(source: u64, destination: u64, length: usize) -> Result<(), Errno> {
    range(source, length)?;
    range(destination, length)?;
    initialize_fault_slot();
    #[cfg(test)]
    before_copy();
    // SAFETY: no user references are formed; each user load/store is listed in
    // the exception table and returns its remaining count on a terminal fault.
    let copied = unsafe { aim_user_copy(source, destination, length) };
    if copied.remaining == 0 {
        Ok(())
    } else {
        Err(copied.error as Errno)
    }
}

#[derive(Clone, Copy)]
pub(super) struct UserWord(pub u64);
impl UserWord {
    fn operation(self, operation: u32, expected: u32, value: u32) -> Result<(i32, u32), Errno> {
        if self.0 & 3 != 0 {
            return Err(errno::EINVAL);
        }
        range(self.0, 4)?;
        initialize_fault_slot();
        #[cfg(test)]
        before_copy();
        let mut output = 0;
        // SAFETY: output is owned; all user instructions have a fault return.
        let status = unsafe { aim_user_atomic(self.0, operation, expected, value, &mut output) };
        if status != 0 && status != 1 {
            Err(status)
        } else {
            Ok((status, output))
        }
    }
    pub fn load(self, _: std::sync::atomic::Ordering) -> Result<u32, Errno> {
        self.operation(0, 0, 0).map(|(_, value)| value)
    }
    pub fn store(self, value: u32, _: std::sync::atomic::Ordering) -> Result<(), Errno> {
        self.operation(1, 0, value).map(|_| ())
    }
    pub fn compare_exchange(
        self,
        expected: u32,
        value: u32,
        _: std::sync::atomic::Ordering,
        _: std::sync::atomic::Ordering,
    ) -> Result<Result<u32, u32>, Errno> {
        self.operation(2, expected, value)
            .map(|(status, old)| if status == 0 { Ok(old) } else { Err(old) })
    }
    pub fn fetch_update(
        self,
        _: std::sync::atomic::Ordering,
        _: std::sync::atomic::Ordering,
        mut update: impl FnMut(u32) -> Option<u32>,
    ) -> Result<Result<u32, u32>, Errno> {
        use std::sync::atomic::Ordering::SeqCst;
        let mut old = self.load(SeqCst)?;
        loop {
            let Some(new) = update(old) else {
                return Ok(Err(old));
            };
            match self.compare_exchange(old, new, SeqCst, SeqCst)? {
                Ok(value) => return Ok(Ok(value)),
                Err(value) => old = value,
            }
        }
    }
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
    range(address, length)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length).map_err(|_| errno::ENOMEM)?;
    bytes.resize(length, 0);
    transfer(address, bytes.as_mut_ptr() as u64, length)?;
    Ok(bytes)
}

pub fn write_exact(address: u64, bytes: &[u8]) -> Result<(), Errno> {
    transfer(bytes.as_ptr() as u64, address, bytes.len())
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
    fn isolated(name: &str) -> bool {
        if super::super::fdtab::isolated_kernel_test(&format!("sys::user_memory::tests::{name}")) {
            return true;
        }
        crate::diag::install_signal_handlers();
        false
    }

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
        if isolated("copies_owned_bytes_and_contains_bad_addresses_and_permissions") {
            return;
        }
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
        if isolated("nul_before_unreadable_next_page_stops_without_touching_it") {
            return;
        }
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
    fn copies_fault_after_validation_without_dereferencing_user_memory() {
        if isolated("copies_fault_after_validation_without_dereferencing_user_memory") {
            return;
        }
        let memory = Mapping::new();
        let address = memory.address();
        BEFORE_COPY.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                assert_eq!(
                    unsafe { libc::mprotect(address as *mut _, PAGE as usize, libc::PROT_NONE) },
                    0
                );
            }))
        });
        assert_eq!(read_exact(address, 16), Err(errno::EFAULT));
        assert_eq!(
            unsafe { libc::mprotect(memory.0, PAGE as usize, libc::PROT_READ | libc::PROT_WRITE) },
            0
        );
        BEFORE_COPY.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                assert_eq!(unsafe { libc::munmap(address as *mut _, PAGE as usize) }, 0);
            }))
        });
        assert_eq!(write_exact(address, b"race"), Err(errno::EFAULT));
    }

    #[test]
    fn atomic_fault_returns_efault_and_releases_the_callers_lock() {
        if isolated("atomic_fault_returns_efault_and_releases_the_callers_lock") {
            return;
        }
        let memory = Mapping::new();
        let address = memory.address();
        use std::sync::{Mutex, atomic::Ordering::SeqCst};
        let lock = Mutex::new(());
        let compare = || -> i64 {
            let _guard = lock.lock().unwrap();
            assert_eq!(
                user_try!(UserWord(address).compare_exchange(0, 7, SeqCst, SeqCst)),
                Ok(0)
            );
            0
        };
        BEFORE_COPY.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                assert_eq!(
                    unsafe { libc::mprotect(address as *mut _, PAGE as usize, libc::PROT_NONE) },
                    0
                );
            }))
        });
        assert_eq!(compare(), -(errno::EFAULT as i64));
        assert!(lock.try_lock().is_ok());
        assert_eq!(
            unsafe { libc::mprotect(memory.0, PAGE as usize, libc::PROT_READ | libc::PROT_WRITE) },
            0
        );
        assert_eq!(
            UserWord(address).compare_exchange(0, 7, SeqCst, SeqCst),
            Ok(Ok(0))
        );
        assert_eq!(
            UserWord(address).compare_exchange(0, 9, SeqCst, SeqCst),
            Ok(Err(7))
        );
        assert_eq!(UserWord(address).load(SeqCst), Ok(7));
        UserWord(address).store(11, SeqCst).unwrap();
        assert_eq!(UserWord(address).load(SeqCst), Ok(11));
        assert_eq!(
            unsafe { libc::mprotect(memory.0, PAGE as usize, libc::PROT_READ) },
            0
        );
        assert_eq!(
            UserWord(address).compare_exchange(11, 12, SeqCst, SeqCst),
            Err(errno::EFAULT)
        );
        assert_eq!(UserWord(address).store(12, SeqCst), Err(errno::EFAULT));
        assert_eq!(UserWord(address).load(SeqCst), Ok(11));
    }

    #[test]
    fn concurrent_host_protection_changes_leave_copies_fault_contained() {
        if isolated("concurrent_host_protection_changes_leave_copies_fault_contained") {
            return;
        }
        let memory = Mapping::new();
        let address = memory.address();
        use std::sync::atomic::{AtomicBool, Ordering};
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                while !stop.load(Ordering::Relaxed) {
                    for prot in [libc::PROT_NONE, libc::PROT_READ | libc::PROT_WRITE] {
                        assert_eq!(
                            unsafe { libc::mprotect(address as *mut _, PAGE as usize, prot) },
                            0
                        );
                    }
                }
            });
            for _ in 0..2000 {
                assert!(matches!(
                    read_exact(address, 16),
                    Ok(_) | Err(errno::EFAULT)
                ));
                assert!(matches!(
                    write_exact(address, &[0; 16]),
                    Ok(()) | Err(errno::EFAULT)
                ));
            }
            stop.store(true, Ordering::Relaxed);
        });
    }

    #[test]
    fn small_direct_copies_benchmark() {
        // The timed API path is range arithmetic, TLS and ordinary loads/stores;
        // it has no VM query, syscall or Mach call. Warm TLS/allocator first.
        let source = 0x1234_5678_9abc_def0u64.to_ne_bytes();
        let mut destination = [0u8; 8];
        let address = destination.as_mut_ptr() as u64;
        write_exact(address, &source).unwrap();
        const COPIES: u32 = 1_000_000;
        let start = std::time::Instant::now();
        for _ in 0..COPIES {
            write_exact(std::hint::black_box(address), std::hint::black_box(&source)).unwrap();
        }
        let elapsed = start.elapsed();
        assert_eq!(destination, source);
        eprintln!(
            "{COPIES} direct 8-byte copies: {:.1} ns/copy",
            elapsed.as_nanos() as f64 / COPIES as f64
        );
        assert!(elapsed.as_nanos() / (COPIES as u128) < 1000);
    }

    #[test]
    fn fixups_return_remaining_bytes_for_both_copy_endpoints() {
        if isolated("fixups_return_remaining_bytes_for_both_copy_endpoints") {
            return;
        }
        let memory = Mapping::new();
        let address = memory.address();
        unsafe { std::ptr::write_bytes(memory.0.cast::<u8>(), 0x5a, PAGE as usize) };
        assert_eq!(
            unsafe { libc::mprotect((address + PAGE) as *mut _, PAGE as usize, libc::PROT_NONE) },
            0
        );
        for (prefix, count) in [(16, 32), (3, 7)] {
            let mut bytes = [0xa5u8; 32];
            initialize_fault_slot();
            let copied =
                unsafe { aim_user_copy(address + PAGE - prefix, bytes.as_mut_ptr() as u64, count) };
            assert_eq!(copied.remaining, count - prefix as usize);
            assert_eq!(copied.error, errno::EFAULT as usize);
            assert_eq!(&bytes[..prefix as usize], vec![0x5a; prefix as usize]);
            assert_eq!(
                &bytes[prefix as usize..count],
                vec![0xa5; count - prefix as usize]
            );
            let copied =
                unsafe { aim_user_copy(bytes.as_ptr() as u64, address + PAGE - prefix, count) };
            assert_eq!(copied.remaining, count - prefix as usize);
            assert_eq!(copied.error, errno::EFAULT as usize);
        }
    }

    #[test]
    fn only_exception_table_instructions_recover() {
        let mut context: super::super::sigframe::DarwinMcontext = unsafe { std::mem::zeroed() };
        context.pc = aim_user_copy as *const () as u64;
        assert!(!recover_fault(&mut context, 0));
        // The STR immediately after LDAR writes the owned output, and must
        // never be mistaken for a guest access.
        context.pc = unsafe { aim_user_atomic_ex_table[2].pc } + 4;
        assert!(!recover_fault(&mut context, 0));
        context.pc = read_exact as *const () as u64;
        assert!(!recover_fault(&mut context, 0));
    }

    #[test]
    fn cold_verified_rodata_is_paged_before_copy_and_corruption_exposes_no_bytes() {
        if isolated("cold_verified_rodata_is_paged_before_copy_and_corruption_exposes_no_bytes") {
            return;
        }
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
        let root = aim_paths::scratch_dir("aim-user-copy-proof");
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
        let atomic_memory = Mapping::new();
        assert_eq!(
            unsafe { libc::mprotect(atomic_memory.0, (2 * PAGE) as usize, libc::PROT_NONE) },
            0
        );
        let atomic_registration = verity_pager::register(
            atomic_memory.address(),
            2 * PAGE,
            0,
            libc::PROT_READ,
            false,
            source.clone(),
        )
        .unwrap();
        use std::sync::atomic::Ordering::SeqCst;
        assert_eq!(UserWord(atomic_memory.address()).load(SeqCst), Ok(0));
        assert_eq!(
            UserWord(atomic_memory.address()).store(7, SeqCst),
            Err(errno::EFAULT)
        );
        assert_eq!(
            UserWord(atomic_memory.address()).compare_exchange(0, 7, SeqCst, SeqCst),
            Err(errno::EFAULT)
        );
        drop(atomic_registration);
        drop(atomic_memory);

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
