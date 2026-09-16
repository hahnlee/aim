#![forbid(unsafe_op_in_unsafe_fn)]

use std::collections::BTreeMap;
use std::ffi::{c_int, c_void};
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, RwLockReadGuard};

const ANDROID_EIO: i32 = 5;
const ANDROID_EBADF: i32 = 9;
const ANDROID_ENOMEM: i32 = 12;
const ANDROID_EACCES: i32 = 13;
const ANDROID_EEXIST: i32 = 17;
const ANDROID_EINVAL: i32 = 22;
const ANDROID_EOVERFLOW: i32 = 75;
const ANDROID_EOPNOTSUPP: i32 = 95;

const ANDROID_PROT_READ: i32 = 0x1;
const ANDROID_PROT_WRITE: i32 = 0x2;
const ANDROID_PROT_EXEC: i32 = 0x4;
const ANDROID_PROT_MASK: i32 = 0x7;
const ANDROID_MAP_PRIVATE: i32 = 0x02;
const ANDROID_MAP_SHARED: i32 = 0x01;
const ANDROID_MAP_FIXED: i32 = 0x10;
const ANDROID_MAP_ANONYMOUS: i32 = 0x20;
const ANDROID_MAP_GROWSDOWN: i32 = 0x100;
const ANDROID_MAP_NORESERVE: i32 = 0x4000;
const ANDROID_MAP_STACK: i32 = 0x20000;
const ANDROID_MREMAP_MAYMOVE: i32 = 0x1;

// Keep the ordinary Linux-style VMA registry independent from the fixed-size,
// async-signal-safe JIT view. Chromium legitimately creates more than 1,024
// VMAs; the lower historic limit made mprotect fail with a synthetic ENOMEM.
const MAX_MAPPING_SEGMENTS: usize = 65_536;
const MAX_JIT_RANGES: usize = 4_096;
const MAP_FAILED: *mut c_void = usize::MAX as *mut c_void;
const VM_MADVISE_TRACE_LIMIT: usize = 256;

static VM_MADVISE_TRACE_EVENTS: AtomicUsize = AtomicUsize::new(0);

struct JitRange {
    start: AtomicUsize,
    end: AtomicUsize,
    executable: AtomicBool,
    writable: AtomicBool,
    emulated_rwx: AtomicBool,
}

struct JitSnapshot {
    ranges: [JitRange; MAX_JIT_RANGES],
    count: AtomicUsize,
    readers: AtomicUsize,
}

// SIGBUS can arrive on a V8 execution thread while another thread is changing
// CodeRange protections. The ordinary mapping table is mutex-owned and cannot
// be consulted safely from a signal handler. Every inert anonymous reservation
// can become a V8 CodeRange because Linux provides no flag identifying it at
// reservation time, so publish all candidates and their guest permissions.
//
// Writers fill the inactive snapshot and publish it with one release store.
// A reader pins the active slot before scanning it; a later writer waits before
// reusing that slot. Unlike a seqlock this cannot deadlock when SIGBUS interrupts
// the publishing thread, and the handler never allocates or takes a mutex.
static JIT_SNAPSHOTS: [JitSnapshot; 2] = [const {
    JitSnapshot {
        ranges: [const {
            JitRange {
                start: AtomicUsize::new(0),
                end: AtomicUsize::new(0),
                executable: AtomicBool::new(false),
                writable: AtomicBool::new(false),
                emulated_rwx: AtomicBool::new(false),
            }
        }; MAX_JIT_RANGES],
        count: AtomicUsize::new(0),
        readers: AtomicUsize::new(0),
    }
}; 2];
static ACTIVE_JIT_SNAPSHOT: AtomicUsize = AtomicUsize::new(0);
static JIT_PAGE_SIZE: AtomicUsize = AtomicUsize::new(0);

unsafe extern "C" {
    fn darwin_art_host_vm_page_size() -> usize;
    fn darwin_art_host_vm_map(
        address: *mut c_void,
        length: usize,
        protection: c_int,
        flags: c_int,
        fd: c_int,
        offset: i64,
        error: *mut c_int,
    ) -> *mut c_void;
    fn darwin_art_host_vm_unmap(address: *mut c_void, length: usize, error: *mut c_int) -> c_int;
    fn darwin_art_host_vm_protect(
        address: *mut c_void,
        length: usize,
        protection: c_int,
        error: *mut c_int,
    ) -> c_int;
    fn darwin_art_host_vm_advise(
        address: *mut c_void,
        length: usize,
        advice: c_int,
        error: *mut c_int,
    ) -> c_int;
    fn darwin_art_host_vm_remap_zero(
        address: *mut c_void,
        length: usize,
        protection: c_int,
        error: *mut c_int,
    ) -> *mut c_void;
    fn darwin_art_host_vm_invalidate_icache(address: *mut c_void, length: usize);
    fn darwin_art_host_vm_close_fd(fd: c_int, error: *mut c_int) -> c_int;
    fn darwin_art_bionic_errno_set_from_darwin(error: c_int) -> c_int;
    fn darwin_art_bionic_errno_store(error: i32);
}

#[derive(Clone, Copy)]
struct Mapping {
    allocation_id: usize,
    requested_length: usize,
    mapped_length: usize,
    protection: i32,
    anonymous: bool,
    jit_capable: bool,
    jit_activated: bool,
}

pub struct Provider {
    page_size: usize,
    mappings: Mutex<BTreeMap<usize, Mapping>>,
    borrowed_ranges: Mutex<BTreeMap<usize, usize>>,
    capability_failure: AtomicBool,
    next_allocation_id: AtomicUsize,
}

static ACTIVE: OnceLock<RwLock<Option<Arc<Provider>>>> = OnceLock::new();
type FileDescriptorResolver = unsafe extern "C" fn(c_int, *mut c_int) -> c_int;
static FILE_DESCRIPTOR_RESOLVER: OnceLock<RwLock<Option<FileDescriptorResolver>>> = OnceLock::new();

fn active_slot() -> &'static RwLock<Option<Arc<Provider>>> {
    ACTIVE.get_or_init(|| RwLock::new(None))
}

fn file_descriptor_resolver() -> &'static RwLock<Option<FileDescriptorResolver>> {
    FILE_DESCRIPTOR_RESOLVER.get_or_init(|| RwLock::new(None))
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_vm_bind_file_descriptor_resolver(
    resolver: Option<FileDescriptorResolver>,
) -> c_int {
    let Ok(mut active) = file_descriptor_resolver().write() else {
        set_errno(ANDROID_EIO);
        return -1;
    };
    *active = resolver;
    0
}

impl Provider {
    pub fn new() -> Result<Self, &'static str> {
        // SAFETY: the host helper has no pointer preconditions.
        let page_size = unsafe { darwin_art_host_vm_page_size() };
        if page_size == 0 || !page_size.is_power_of_two() {
            return Err("invalid Darwin page size");
        }
        Ok(Self {
            page_size,
            mappings: Mutex::new(BTreeMap::new()),
            borrowed_ranges: Mutex::new(BTreeMap::new()),
            capability_failure: AtomicBool::new(false),
            next_allocation_id: AtomicUsize::new(1),
        })
    }

    pub fn activate(self: &Arc<Self>) -> Result<Activation, &'static str> {
        let mut active = active_slot()
            .write()
            .map_err(|_| "VM activation lock poisoned")?;
        if active.is_some() {
            return Err("VM provider already active");
        }
        JIT_PAGE_SIZE.store(self.page_size, Ordering::Release);
        *active = Some(Arc::clone(self));
        Ok(Activation)
    }

    pub fn page_size(&self) -> usize {
        self.page_size
    }

    pub fn mapping_count(&self) -> usize {
        self.mappings.lock().map(|m| m.len()).unwrap_or(usize::MAX)
    }

    pub fn protection(&self, address: usize) -> Option<i32> {
        self.mappings
            .lock()
            .ok()?
            .get(&address)
            .map(|mapping| mapping.protection)
    }

    pub fn capability_failed(&self) -> bool {
        self.capability_failure.load(Ordering::Acquire)
    }

    fn next_allocation_id(&self) -> Option<usize> {
        self.next_allocation_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_add(1)
            })
            .ok()
    }
}

impl Drop for Provider {
    fn drop(&mut self) {
        let borrowed = match self.borrowed_ranges.get_mut() {
            Ok(value) => value,
            Err(poisoned) => poisoned.into_inner(),
        };
        if !borrowed.is_empty() {
            self.capability_failure.store(true, Ordering::Release);
        }
        let mappings = match self.mappings.get_mut() {
            Ok(value) => value,
            Err(poisoned) => poisoned.into_inner(),
        };
        for (address, mapping) in std::mem::take(mappings) {
            let mut host_error = 0;
            // SAFETY: every entry is a live complete host mapping owned by this provider.
            unsafe {
                darwin_art_host_vm_unmap(
                    address as *mut c_void,
                    mapping.mapped_length,
                    &mut host_error,
                )
            };
        }
    }
}

pub struct Activation;

impl Drop for Activation {
    fn drop(&mut self) {
        if let Ok(mut active) = active_slot().write() {
            active.take();
            clear_jit_ranges();
            JIT_PAGE_SIZE.store(0, Ordering::Release);
        }
    }
}

struct ProcessOwner {
    users: usize,
    _provider: Arc<Provider>,
    _activation: Activation,
}

static PROCESS_OWNER: OnceLock<Mutex<Option<ProcessOwner>>> = OnceLock::new();

fn process_owner() -> &'static Mutex<Option<ProcessOwner>> {
    PROCESS_OWNER.get_or_init(|| Mutex::new(None))
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_vm_process_install() -> c_int {
    let Ok(mut owner) = process_owner().lock() else {
        set_errno(ANDROID_EIO);
        return -1;
    };
    if let Some(active) = owner.as_mut() {
        let Some(users) = active.users.checked_add(1) else {
            set_errno(ANDROID_EOVERFLOW);
            return -1;
        };
        active.users = users;
        return 0;
    }
    let provider = match Provider::new() {
        Ok(provider) => Arc::new(provider),
        Err(_) => {
            set_errno(ANDROID_EIO);
            return -1;
        }
    };
    let activation = match provider.activate() {
        Ok(activation) => activation,
        Err(_) => {
            set_errno(ANDROID_EIO);
            return -1;
        }
    };
    *owner = Some(ProcessOwner {
        users: 1,
        _provider: provider,
        _activation: activation,
    });
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_vm_process_uninstall() -> c_int {
    let removed = {
        let Ok(mut owner) = process_owner().lock() else {
            set_errno(ANDROID_EIO);
            return -1;
        };
        let Some(active) = owner.as_mut() else {
            set_errno(ANDROID_EINVAL);
            return -1;
        };
        active.users -= 1;
        if active.users == 0 {
            owner.take()
        } else {
            None
        }
    };
    let final_owner = removed.is_some();
    drop(removed);
    if final_owner {
        let Ok(mut resolver) = file_descriptor_resolver().write() else {
            set_errno(ANDROID_EIO);
            return -1;
        };
        *resolver = None;
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_vm_register_borrowed_range(
    address: *mut c_void,
    length: usize,
) -> c_int {
    let Some(provider) = provider() else {
        return -1;
    };
    // Android ELF PT_LOAD envelopes use the guest 4 KiB page contract even
    // on Apple Silicon hosts whose Mach VM page size is 16 KiB. Borrowed
    // ranges are metadata only; requiring the host page size here rejects
    // valid guest images before JNI_OnLoad can run.
    const GUEST_PAGE_SIZE: usize = 4096;
    if address.is_null()
        || length == 0
        || address as usize % GUEST_PAGE_SIZE != 0
        || length % GUEST_PAGE_SIZE != 0
    {
        set_errno(ANDROID_EINVAL);
        return -1;
    }
    let start = address as usize;
    let Some(_end) = start.checked_add(length) else {
        set_errno(ANDROID_EOVERFLOW);
        return -1;
    };
    // A borrowed range describes an externally-owned Android ELF image. The
    // ELF loader maps its PT_LOAD segments through this same VM provider, so
    // the image envelope legitimately overlaps several tracked mappings.
    // Keep duplicate borrowed envelopes rejected below, but do not confuse
    // ownership overlap with a second anonymous mmap allocation.
    let Ok(mut borrowed) = provider.borrowed_ranges.lock() else {
        set_errno(ANDROID_EIO);
        return -1;
    };
    // One ELF image may publish multiple overlapping envelopes while its
    // PT_LOAD segments are finalized. Track each exact envelope independently
    // so the matching unregister call remains transactional.
    if borrowed.insert(start, length).is_some() {
        set_errno(ANDROID_EEXIST);
        return -1;
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_vm_unregister_borrowed_range(
    address: *mut c_void,
    length: usize,
) -> c_int {
    let Some(provider) = provider() else {
        return -1;
    };
    let Ok(mut borrowed) = provider.borrowed_ranges.lock() else {
        set_errno(ANDROID_EIO);
        return -1;
    };
    if borrowed.get(&(address as usize)) != Some(&length) {
        set_errno(ANDROID_EINVAL);
        return -1;
    }
    borrowed.remove(&(address as usize));
    0
}

fn set_errno(error: i32) {
    // SAFETY: the errno provider accepts every 32-bit Android errno value.
    unsafe { darwin_art_bionic_errno_store(error) }
}

struct ProviderLease {
    _active: RwLockReadGuard<'static, Option<Arc<Provider>>>,
    provider: Arc<Provider>,
}

impl Deref for ProviderLease {
    type Target = Provider;

    fn deref(&self) -> &Self::Target {
        &self.provider
    }
}

fn provider() -> Option<ProviderLease> {
    match active_slot().read() {
        Ok(active) => match active.as_ref().cloned() {
            Some(provider) => Some(ProviderLease {
                _active: active,
                provider,
            }),
            None => {
                set_errno(ANDROID_EIO);
                None
            }
        },
        Err(_) => {
            set_errno(ANDROID_EIO);
            None
        }
    }
}

fn fail_host(provider: &Provider, host_error: i32) {
    // SAFETY: this does not expose host errno storage and preserves host errno.
    if host_error == 0 || unsafe { darwin_art_bionic_errno_set_from_darwin(host_error) } == 0 {
        provider.capability_failure.store(true, Ordering::Release);
        set_errno(ANDROID_EIO);
    }
}

fn host_protection(android: i32) -> Option<i32> {
    if android & !ANDROID_PROT_MASK != 0 {
        return None;
    }
    let mut host = 0;
    if android & ANDROID_PROT_READ != 0 {
        host |= 1;
    }
    if android & ANDROID_PROT_WRITE != 0 {
        host |= 2;
    }
    if android & ANDROID_PROT_EXEC != 0 {
        host |= 4;
    }
    Some(host)
}

fn trace_madvise_event(
    enabled: bool,
    phase: &str,
    address: *mut c_void,
    length: usize,
    advice: c_int,
    error: i32,
) {
    if !enabled {
        return;
    }
    let sequence = VM_MADVISE_TRACE_EVENTS.fetch_add(1, Ordering::Relaxed);
    if sequence < VM_MADVISE_TRACE_LIMIT {
        eprintln!(
            "DARWIN VM: madvise {phase} address={:#x} length={length:#x} advice={advice} errno={error}",
            address as usize
        );
    }
}

fn trace_vm_failure(
    operation: &str,
    address: *mut c_void,
    length: usize,
    error: i32,
    detail: &str,
) {
    if std::env::var_os("DARWIN_ART_VM_FAILURE_TRACE").is_none() {
        return;
    }
    eprintln!(
        "DARWIN VM failure: pid={} operation={operation} address={address:p} length={length:#x} errno={error} detail={detail}",
        std::process::id()
    );
}

fn whole_mapping(mapping: Mapping, length: usize) -> bool {
    length == mapping.requested_length || length == mapping.mapped_length
}

fn round_length(length: usize, page_size: usize) -> Option<usize> {
    length
        .checked_add(page_size - 1)
        .map(|value| value & !(page_size - 1))
}

fn owned_range(mappings: &BTreeMap<usize, Mapping>, start: usize, length: usize) -> bool {
    let Some(end) = start.checked_add(length) else {
        return false;
    };
    let mut cursor = start;
    while cursor < end {
        let Some((&base, mapping)) = mappings.range(..=cursor).next_back() else {
            return false;
        };
        let Some(mapping_end) = base.checked_add(mapping.mapped_length) else {
            return false;
        };
        if cursor < base || cursor >= mapping_end {
            return false;
        }
        cursor = mapping_end.min(end);
    }
    true
}

fn jit_capable_range(mappings: &BTreeMap<usize, Mapping>, start: usize, length: usize) -> bool {
    let Some(end) = start.checked_add(length) else {
        return false;
    };
    let mut cursor = start;
    while cursor < end {
        let Some((&base, mapping)) = mappings.range(..=cursor).next_back() else {
            return false;
        };
        let Some(mapping_end) = base.checked_add(mapping.mapped_length) else {
            return false;
        };
        if cursor < base || cursor >= mapping_end || !mapping.jit_capable {
            return false;
        }
        cursor = mapping_end.min(end);
    }
    true
}

fn borrowed_range(ranges: &BTreeMap<usize, usize>, start: usize, length: usize) -> bool {
    let Some(end) = start.checked_add(length) else {
        return false;
    };
    let Some((&base, &range_length)) = ranges.range(..=start).next_back() else {
        return false;
    };
    base <= start
        && base
            .checked_add(range_length)
            .is_some_and(|limit| end <= limit)
}

fn jit_range_count(mappings: &BTreeMap<usize, Mapping>) -> usize {
    mappings
        .values()
        .filter(|mapping| mapping.jit_capable)
        .count()
}

fn mapping_table_admissible(mappings: &BTreeMap<usize, Mapping>) -> bool {
    mappings.len() <= MAX_MAPPING_SEGMENTS && jit_range_count(mappings) <= MAX_JIT_RANGES
}

fn publish_jit_ranges(mappings: &BTreeMap<usize, Mapping>) {
    debug_assert!(mapping_table_admissible(mappings));
    let active = ACTIVE_JIT_SNAPSHOT.load(Ordering::Acquire) & 1;
    let target_index = active ^ 1;
    let snapshot = &JIT_SNAPSHOTS[target_index];
    while snapshot.readers.load(Ordering::Acquire) != 0 {
        std::thread::yield_now();
    }
    let previous_count = snapshot.count.load(Ordering::Relaxed).min(MAX_JIT_RANGES);
    let mut count = 0;
    for (&base, mapping) in mappings {
        if !mapping.jit_capable {
            continue;
        }
        if count == MAX_JIT_RANGES {
            break;
        }
        let Some(end) = base.checked_add(mapping.mapped_length) else {
            continue;
        };
        let range = &snapshot.ranges[count];
        range.start.store(base, Ordering::Relaxed);
        range.end.store(end, Ordering::Relaxed);
        range.executable.store(
            mapping.protection & ANDROID_PROT_EXEC != 0,
            Ordering::Relaxed,
        );
        range.writable.store(
            mapping.protection & ANDROID_PROT_WRITE != 0,
            Ordering::Relaxed,
        );
        range.emulated_rwx.store(
            mapping.jit_activated
                && mapping.protection & ANDROID_PROT_WRITE != 0
                && mapping.protection & ANDROID_PROT_EXEC != 0,
            Ordering::Relaxed,
        );
        count += 1;
    }
    if count < previous_count {
        for range in &snapshot.ranges[count..previous_count] {
            range.start.store(0, Ordering::Relaxed);
            range.end.store(0, Ordering::Relaxed);
            range.executable.store(false, Ordering::Relaxed);
            range.writable.store(false, Ordering::Relaxed);
            range.emulated_rwx.store(false, Ordering::Relaxed);
        }
    }
    snapshot.count.store(count, Ordering::Relaxed);
    ACTIVE_JIT_SNAPSHOT.store(target_index, Ordering::Release);
}

fn clear_jit_ranges() {
    let active = ACTIVE_JIT_SNAPSHOT.load(Ordering::Acquire) & 1;
    let target_index = active ^ 1;
    let snapshot = &JIT_SNAPSHOTS[target_index];
    while snapshot.readers.load(Ordering::Acquire) != 0 {
        std::thread::yield_now();
    }
    snapshot.count.store(0, Ordering::Relaxed);
    ACTIVE_JIT_SNAPSHOT.store(target_index, Ordering::Release);
}

fn replace_range(
    mappings: &mut BTreeMap<usize, Mapping>,
    start: usize,
    length: usize,
    replacement_protection: Option<i32>,
) {
    let end = start + length;
    let affected: Vec<(usize, Mapping)> = mappings
        .range(..end)
        .filter_map(|(&base, mapping)| {
            (base + mapping.mapped_length > start).then_some((base, *mapping))
        })
        .collect();
    for (base, mapping) in affected {
        mappings.remove(&base);
        let mapping_end = base + mapping.mapped_length;
        if base < start {
            let prefix_length = start - base;
            mappings.insert(
                base,
                Mapping {
                    allocation_id: mapping.allocation_id,
                    requested_length: prefix_length,
                    mapped_length: prefix_length,
                    protection: mapping.protection,
                    anonymous: mapping.anonymous,
                    jit_capable: mapping.jit_capable,
                    jit_activated: mapping.jit_activated,
                },
            );
        }
        let overlap_start = base.max(start);
        let overlap_end = mapping_end.min(end);
        if let Some(protection) = replacement_protection {
            let overlap_length = overlap_end - overlap_start;
            mappings.insert(
                overlap_start,
                Mapping {
                    allocation_id: mapping.allocation_id,
                    requested_length: overlap_length,
                    mapped_length: overlap_length,
                    protection,
                    anonymous: mapping.anonymous,
                    jit_capable: mapping.jit_capable,
                    jit_activated: mapping.jit_activated,
                },
            );
        }
        if mapping_end > end {
            let suffix_length = mapping_end - end;
            mappings.insert(
                end,
                Mapping {
                    allocation_id: mapping.allocation_id,
                    requested_length: suffix_length,
                    mapped_length: suffix_length,
                    protection: mapping.protection,
                    anonymous: mapping.anonymous,
                    jit_capable: mapping.jit_capable,
                    jit_activated: mapping.jit_activated,
                },
            );
        }
    }
    coalesce_mapping_segments(mappings);
}

fn same_mapping_state(left: Mapping, right: Mapping) -> bool {
    left.allocation_id == right.allocation_id
        && left.protection == right.protection
        && left.anonymous == right.anonymous
        && left.jit_capable == right.jit_capable
        && left.jit_activated == right.jit_activated
}

fn coalesce_mapping_segments(mappings: &mut BTreeMap<usize, Mapping>) {
    let entries: Vec<(usize, Mapping)> = mappings
        .iter()
        .map(|(&base, &mapping)| (base, mapping))
        .collect();
    let mut coalesced: BTreeMap<usize, Mapping> = BTreeMap::new();
    for (base, mapping) in entries {
        if let Some((&previous_base, previous)) = coalesced.last_key_value() {
            if previous_base.checked_add(previous.mapped_length) == Some(base)
                && same_mapping_state(*previous, mapping)
            {
                let previous = coalesced
                    .get_mut(&previous_base)
                    .expect("last mapping remains present");
                previous.mapped_length += mapping.mapped_length;
                previous.requested_length = previous.mapped_length;
                continue;
            }
        }
        coalesced.insert(base, mapping);
    }
    *mappings = coalesced;
}

fn replacement_table(
    mappings: &BTreeMap<usize, Mapping>,
    start: usize,
    length: usize,
    replacement_protection: Option<i32>,
) -> Option<BTreeMap<usize, Mapping>> {
    start.checked_add(length)?;
    let mut replacement = mappings.clone();
    replace_range(&mut replacement, start, length, replacement_protection);
    mapping_table_admissible(&replacement).then_some(replacement)
}

#[unsafe(no_mangle)]
/// # Safety
///
/// `address` is an opaque Android mapping hint. `MAP_FIXED` is honored only
/// when the complete replacement range is already owned by this provider;
/// otherwise the host may choose a different free range.
pub unsafe extern "C" fn darwin_art_bionic_vm_mmap_core(
    address: *mut c_void,
    length: usize,
    protection: c_int,
    flags: c_int,
    fd: c_int,
    offset: i64,
) -> *mut c_void {
    let trace = std::env::var_os("DARWIN_ART_VM_TRACE").is_some();
    if trace {
        eprintln!(
            "DARWIN VM: pid={} mmap address={address:p} length={length:#x} protection={protection:#x} flags={flags:#x} fd={fd} offset={offset:#x}",
            std::process::id()
        );
    }
    let Some(provider) = provider() else {
        return MAP_FAILED;
    };
    if length == 0 {
        set_errno(ANDROID_EINVAL);
        return MAP_FAILED;
    }
    if length > isize::MAX as usize {
        set_errno(ANDROID_EOVERFLOW);
        return MAP_FAILED;
    }
    if offset < 0 || offset as usize % provider.page_size != 0 {
        set_errno(ANDROID_EINVAL);
        return MAP_FAILED;
    }
    let fixed = flags & ANDROID_MAP_FIXED != 0;
    // MAP_NORESERVE changes the host's commit policy, not the mapping kind.
    // V8 uses it for the multi-gigabyte sandbox cage reservation. Linux's
    // MAP_STACK is likewise a compatibility hint. MAP_GROWSDOWN cannot be
    // delegated to Darwin, but for a null-address anonymous stack request we
    // can map the complete requested extent eagerly; accesses within the
    // Android-owned stack then have the same permissions without host growth.
    let stack_hints = ANDROID_MAP_STACK | ANDROID_MAP_GROWSDOWN;
    let mapping_flags = flags & !(ANDROID_MAP_FIXED | ANDROID_MAP_NORESERVE | stack_hints);
    let stack_mapping = flags & stack_hints != 0;
    let anonymous = mapping_flags == ANDROID_MAP_PRIVATE | ANDROID_MAP_ANONYMOUS
        || mapping_flags == ANDROID_MAP_SHARED | ANDROID_MAP_ANONYMOUS;
    let file_backed = mapping_flags == ANDROID_MAP_PRIVATE || mapping_flags == ANDROID_MAP_SHARED;
    let mapping_jit_capable = anonymous
        && mapping_flags == ANDROID_MAP_PRIVATE | ANDROID_MAP_ANONYMOUS
        && protection == 0;
    if stack_mapping
        && (!address.is_null() || mapping_flags != (ANDROID_MAP_PRIVATE | ANDROID_MAP_ANONYMOUS))
    {
        if trace {
            eprintln!("DARWIN VM: mmap rejected non-anonymous stack hints");
        }
        set_errno(ANDROID_EOPNOTSUPP);
        return MAP_FAILED;
    }
    if (!anonymous && !file_backed) || (anonymous && offset != 0) {
        if trace {
            eprintln!("DARWIN VM: mmap rejected unsupported address/flags/offset");
        }
        set_errno(ANDROID_EOPNOTSUPP);
        return MAP_FAILED;
    }
    if file_backed && fd < 0 {
        set_errno(ANDROID_EBADF);
        return MAP_FAILED;
    }
    let Some(host_protection) = host_protection(protection) else {
        set_errno(ANDROID_EINVAL);
        return MAP_FAILED;
    };
    if protection & ANDROID_PROT_WRITE != 0 && protection & ANDROID_PROT_EXEC != 0 {
        set_errno(ANDROID_EACCES);
        return MAP_FAILED;
    }
    let Some(mapped_length) = round_length(length, provider.page_size) else {
        set_errno(ANDROID_EOVERFLOW);
        return MAP_FAILED;
    };
    let mut mappings = match provider.mappings.lock() {
        Ok(value) => value,
        Err(_) => {
            provider.capability_failure.store(true, Ordering::Release);
            set_errno(ANDROID_EIO);
            return MAP_FAILED;
        }
    };
    if !fixed
        && (mappings.len() >= MAX_MAPPING_SEGMENTS
            || (mapping_jit_capable && jit_range_count(&mappings) >= MAX_JIT_RANGES))
    {
        if trace {
            eprintln!(
                "DARWIN VM: pid={} mmap rejected mapping capacity segments={}/{} jit-ranges={}/{}",
                std::process::id(),
                mappings.len(),
                MAX_MAPPING_SEGMENTS,
                jit_range_count(&mappings),
                MAX_JIT_RANGES
            );
        }
        set_errno(ANDROID_ENOMEM);
        return MAP_FAILED;
    }
    let Some(allocation_id) = provider.next_allocation_id() else {
        set_errno(ANDROID_EOVERFLOW);
        return MAP_FAILED;
    };
    if fixed
        && (address.is_null()
            || address as usize % provider.page_size != 0
            || !owned_range(&mappings, address as usize, mapped_length))
    {
        set_errno(ANDROID_ENOMEM);
        trace_vm_failure(
            "mmap-fixed",
            address,
            mapped_length,
            ANDROID_ENOMEM,
            &format!("invalid-or-unowned-range mappings={}", mappings.len()),
        );
        return MAP_FAILED;
    }
    let fixed_replacement = if fixed {
        let Some(mut replacement) =
            replacement_table(&mappings, address as usize, mapped_length, None)
        else {
            set_errno(ANDROID_ENOMEM);
            trace_vm_failure(
                "mmap-fixed",
                address,
                mapped_length,
                ANDROID_ENOMEM,
                &format!(
                    "mapping-capacity segments={}/{} jit-ranges={}/{}",
                    mappings.len(),
                    MAX_MAPPING_SEGMENTS,
                    jit_range_count(&mappings),
                    MAX_JIT_RANGES
                ),
            );
            return MAP_FAILED;
        };
        replacement.insert(
            address as usize,
            Mapping {
                allocation_id,
                requested_length: length,
                mapped_length,
                protection,
                anonymous,
                jit_capable: mapping_jit_capable,
                jit_activated: false,
            },
        );
        if !mapping_table_admissible(&replacement) {
            set_errno(ANDROID_ENOMEM);
            return MAP_FAILED;
        }
        Some(replacement)
    } else {
        None
    };
    let mut host_error = 0;
    let mut mapped_fd = fd;
    let mut close_mapped_fd = false;
    let mut host_mapping_flags = flags;
    if file_backed {
        let resolver = match file_descriptor_resolver().read() {
            Ok(resolver) => *resolver,
            Err(_) => {
                provider.capability_failure.store(true, Ordering::Release);
                set_errno(ANDROID_EIO);
                return MAP_FAILED;
            }
        };
        if let Some(resolver) = resolver {
            let mut resolved_fd = -1;
            // SAFETY: the callback consumes the writable out-parameter before
            // returning and retains no pointer.
            let resolution = unsafe { resolver(fd, &mut resolved_fd) };
            if trace {
                eprintln!(
                    "DARWIN VM: pid={} fd resolver guest_fd={fd} status={resolution} host_fd={resolved_fd}",
                    std::process::id()
                );
            }
            match resolution {
                1 if resolved_fd >= 0 => {
                    mapped_fd = resolved_fd;
                    close_mapped_fd = true;
                }
                2 if resolved_fd >= 0 && mapping_flags == ANDROID_MAP_PRIVATE => {
                    mapped_fd = resolved_fd;
                    close_mapped_fd = true;
                    host_mapping_flags = (flags & !ANDROID_MAP_PRIVATE) | ANDROID_MAP_SHARED;
                }
                0 => {}
                -1 => return MAP_FAILED,
                _ => {
                    provider.capability_failure.store(true, Ordering::Release);
                    set_errno(ANDROID_EIO);
                    return MAP_FAILED;
                }
            }
        }
    }
    // SAFETY: mapped_length is nonzero and rounded; the host helper fixes
    // anonymous/private flags and treats address as a non-fixed hint.
    let result = unsafe {
        darwin_art_host_vm_map(
            address,
            mapped_length,
            host_protection,
            host_mapping_flags,
            if anonymous { -1 } else { mapped_fd },
            offset,
            &mut host_error,
        )
    };
    if close_mapped_fd {
        let mut close_error = 0;
        // SAFETY: a successful resolver transferred this duplicate to us.
        if unsafe { darwin_art_host_vm_close_fd(mapped_fd, &mut close_error) } != 0 {
            if result != MAP_FAILED {
                let mut ignored = 0;
                // SAFETY: this is the complete mapping just returned above.
                unsafe { darwin_art_host_vm_unmap(result, mapped_length, &mut ignored) };
            }
            fail_host(&provider, close_error);
            return MAP_FAILED;
        }
    }
    if result == MAP_FAILED {
        if trace {
            eprintln!(
                "DARWIN VM: pid={} host mmap failed errno={host_error}",
                std::process::id()
            );
        }
        trace_vm_failure("mmap", address, mapped_length, host_error, "host-mmap");
        fail_host(&provider, host_error);
        return MAP_FAILED;
    }
    if fixed {
        if result != address {
            provider.capability_failure.store(true, Ordering::Release);
            set_errno(ANDROID_EIO);
            return MAP_FAILED;
        }
        *mappings = fixed_replacement.expect("fixed mapping has replacement metadata");
        publish_jit_ranges(&mappings);
        if trace {
            eprintln!(
                "DARWIN VM: pid={} mmap fixed result={result:p} mapped_length={mapped_length:#x}",
                std::process::id()
            );
        }
        return result;
    }
    if trace {
        eprintln!(
            "DARWIN VM: pid={} mmap result={result:p} mapped_length={mapped_length:#x}",
            std::process::id()
        );
    }
    let old = mappings.insert(
        result as usize,
        Mapping {
            allocation_id,
            requested_length: length,
            mapped_length,
            protection,
            anonymous,
            jit_capable: mapping_jit_capable,
            jit_activated: false,
        },
    );
    if old.is_some() {
        let mut ignored = 0;
        // SAFETY: result is the just-created complete mapping.
        unsafe { darwin_art_host_vm_unmap(result, mapped_length, &mut ignored) };
        provider.capability_failure.store(true, Ordering::Release);
        set_errno(ANDROID_EIO);
        return MAP_FAILED;
    }
    result
}

#[unsafe(no_mangle)]
/// # Safety
/// `address` is an opaque guest value and is passed to Darwin only after the
/// side table proves ownership of the complete mapping.
pub unsafe extern "C" fn darwin_art_bionic_vm_munmap_core(
    address: *mut c_void,
    length: usize,
) -> c_int {
    let Some(provider) = provider() else {
        trace_vm_failure(
            "munmap",
            address,
            length,
            ANDROID_EIO,
            "provider-unavailable",
        );
        return -1;
    };
    if address.is_null() || length == 0 || address as usize % provider.page_size != 0 {
        set_errno(ANDROID_EINVAL);
        trace_vm_failure("munmap", address, length, ANDROID_EINVAL, "invalid-range");
        return -1;
    }
    let mut mappings = match provider.mappings.lock() {
        Ok(value) => value,
        Err(_) => {
            provider.capability_failure.store(true, Ordering::Release);
            set_errno(ANDROID_EIO);
            trace_vm_failure("munmap", address, length, ANDROID_EIO, "mapping-lock");
            return -1;
        }
    };
    let Some(mapped_length) = round_length(length, provider.page_size) else {
        set_errno(ANDROID_EOVERFLOW);
        trace_vm_failure(
            "munmap",
            address,
            length,
            ANDROID_EOVERFLOW,
            "length-overflow",
        );
        return -1;
    };
    if !owned_range(&mappings, address as usize, mapped_length) {
        set_errno(ANDROID_EINVAL);
        trace_vm_failure(
            "munmap",
            address,
            mapped_length,
            ANDROID_EINVAL,
            &format!("unowned-range mappings={}", mappings.len()),
        );
        return -1;
    }
    let Some(replacement) = replacement_table(&mappings, address as usize, mapped_length, None)
    else {
        set_errno(ANDROID_ENOMEM);
        trace_vm_failure(
            "munmap",
            address,
            mapped_length,
            ANDROID_ENOMEM,
            &format!(
                "mapping-capacity segments={}/{} jit-ranges={}/{}",
                mappings.len(),
                MAX_MAPPING_SEGMENTS,
                jit_range_count(&mappings),
                MAX_JIT_RANGES
            ),
        );
        return -1;
    };
    let mut host_error = 0;
    // SAFETY: the side table proves every page in this rounded range is owned.
    if unsafe { darwin_art_host_vm_unmap(address, mapped_length, &mut host_error) } != 0 {
        trace_vm_failure("munmap", address, mapped_length, host_error, "host-munmap");
        fail_host(&provider, host_error);
        return -1;
    }
    *mappings = replacement;
    publish_jit_ranges(&mappings);
    0
}

#[unsafe(no_mangle)]
/// # Safety
/// `old_address` is dereferenced only after the side table proves ownership
/// of the complete mapping. Fixed-address remapping is not accepted.
pub unsafe extern "C" fn darwin_art_bionic_vm_mremap_core(
    old_address: *mut c_void,
    old_length: usize,
    new_length: usize,
    flags: c_int,
    _new_address: *mut c_void,
) -> *mut c_void {
    let Some(provider) = provider() else {
        return MAP_FAILED;
    };
    if old_address.is_null()
        || old_length == 0
        || new_length == 0
        || old_address as usize % provider.page_size != 0
    {
        set_errno(ANDROID_EINVAL);
        return MAP_FAILED;
    }
    // The fifth AAPCS64 register is unspecified unless MREMAP_FIXED is set.
    // Unsupported flag bits are rejected before that optional value matters.
    if flags & !ANDROID_MREMAP_MAYMOVE != 0 {
        set_errno(ANDROID_EOPNOTSUPP);
        return MAP_FAILED;
    }
    let Some(new_mapped_length) = round_length(new_length, provider.page_size) else {
        set_errno(ANDROID_EOVERFLOW);
        return MAP_FAILED;
    };
    let mut mappings = match provider.mappings.lock() {
        Ok(value) => value,
        Err(_) => {
            provider.capability_failure.store(true, Ordering::Release);
            set_errno(ANDROID_EIO);
            return MAP_FAILED;
        }
    };
    let Some(old_mapping) = mappings.get(&(old_address as usize)).copied() else {
        set_errno(ANDROID_EINVAL);
        return MAP_FAILED;
    };
    if !whole_mapping(old_mapping, old_length) {
        set_errno(ANDROID_EOPNOTSUPP);
        return MAP_FAILED;
    }
    if new_mapped_length <= old_mapping.mapped_length {
        if let Some(mapping) = mappings.get_mut(&(old_address as usize)) {
            mapping.requested_length = new_length;
        }
        return old_address;
    }
    if flags & ANDROID_MREMAP_MAYMOVE == 0 {
        set_errno(ANDROID_ENOMEM);
        return MAP_FAILED;
    }

    let mut host_error = 0;
    // Allocate writable storage for the copy, then restore the guest's exact
    // protection before publishing the moved mapping.
    let moved = unsafe {
        darwin_art_host_vm_map(
            std::ptr::null_mut(),
            new_mapped_length,
            3,
            ANDROID_MAP_PRIVATE | ANDROID_MAP_ANONYMOUS,
            -1,
            0,
            &mut host_error,
        )
    };
    if moved == MAP_FAILED {
        fail_host(&provider, host_error);
        return MAP_FAILED;
    }
    if old_mapping.protection & ANDROID_PROT_READ == 0 {
        let readable = host_protection(old_mapping.protection | ANDROID_PROT_READ)
            .expect("validated protection");
        if unsafe {
            darwin_art_host_vm_protect(
                old_address,
                old_mapping.mapped_length,
                readable,
                &mut host_error,
            )
        } != 0
        {
            let mut ignored = 0;
            unsafe { darwin_art_host_vm_unmap(moved, new_mapped_length, &mut ignored) };
            fail_host(&provider, host_error);
            return MAP_FAILED;
        }
    }
    // SAFETY: both complete mappings are live and non-overlapping under the
    // ownership lock; the copied range is bounded by both requested sizes.
    unsafe {
        std::ptr::copy_nonoverlapping(
            old_address.cast::<u8>(),
            moved.cast::<u8>(),
            old_mapping.requested_length.min(new_length),
        )
    };
    let target_protection = host_protection(old_mapping.protection).expect("validated protection");
    if unsafe {
        darwin_art_host_vm_protect(moved, new_mapped_length, target_protection, &mut host_error)
    } != 0
    {
        let mut ignored = 0;
        unsafe { darwin_art_host_vm_unmap(moved, new_mapped_length, &mut ignored) };
        fail_host(&provider, host_error);
        return MAP_FAILED;
    }
    if unsafe { darwin_art_host_vm_unmap(old_address, old_mapping.mapped_length, &mut host_error) }
        != 0
    {
        let mut ignored = 0;
        unsafe { darwin_art_host_vm_unmap(moved, new_mapped_length, &mut ignored) };
        fail_host(&provider, host_error);
        return MAP_FAILED;
    }
    mappings.remove(&(old_address as usize));
    mappings.insert(
        moved as usize,
        Mapping {
            allocation_id: old_mapping.allocation_id,
            requested_length: new_length,
            mapped_length: new_mapped_length,
            protection: old_mapping.protection,
            anonymous: old_mapping.anonymous,
            jit_capable: old_mapping.jit_capable,
            jit_activated: old_mapping.jit_activated,
        },
    );
    publish_jit_ranges(&mappings);
    moved
}

#[unsafe(no_mangle)]
/// # Safety
/// `address` is an opaque guest value and is passed to Darwin only after the
/// side table proves ownership of the complete mapping.
pub unsafe extern "C" fn darwin_art_bionic_vm_mprotect_core(
    address: *mut c_void,
    length: usize,
    protection: c_int,
) -> c_int {
    let trace = std::env::var_os("DARWIN_ART_VM_TRACE").is_some();
    if trace {
        eprintln!(
            "DARWIN VM: mprotect address={address:p} length={length:#x} protection={protection:#x}"
        );
    }
    let Some(provider) = provider() else {
        trace_vm_failure(
            "mprotect",
            address,
            length,
            ANDROID_EIO,
            "provider-unavailable",
        );
        return -1;
    };
    if address.is_null() || length == 0 || address as usize % provider.page_size != 0 {
        set_errno(ANDROID_EINVAL);
        trace_vm_failure("mprotect", address, length, ANDROID_EINVAL, "invalid-range");
        return -1;
    }
    let Some(mut host_protection) = host_protection(protection) else {
        set_errno(ANDROID_EINVAL);
        return -1;
    };
    let mut mappings = match provider.mappings.lock() {
        Ok(value) => value,
        Err(_) => {
            provider.capability_failure.store(true, Ordering::Release);
            set_errno(ANDROID_EIO);
            return -1;
        }
    };
    let Some(mapped_length) = round_length(length, provider.page_size) else {
        set_errno(ANDROID_EOVERFLOW);
        return -1;
    };
    let borrowed = match provider.borrowed_ranges.lock() {
        Ok(value) => value,
        Err(_) => {
            provider.capability_failure.store(true, Ordering::Release);
            set_errno(ANDROID_EIO);
            return -1;
        }
    };
    let provider_owned = owned_range(&mappings, address as usize, mapped_length);
    if !provider_owned && !borrowed_range(&borrowed, address as usize, mapped_length) {
        if trace {
            eprintln!("DARWIN VM: mprotect rejected range is not provider-owned");
        }
        set_errno(ANDROID_ENOMEM);
        trace_vm_failure(
            "mprotect",
            address,
            mapped_length,
            ANDROID_ENOMEM,
            &format!("unowned-range mappings={}", mappings.len()),
        );
        return -1;
    }
    if protection & ANDROID_PROT_WRITE != 0
        && protection & ANDROID_PROT_EXEC != 0
        && (!provider_owned || !jit_capable_range(&mappings, address as usize, mapped_length))
    {
        set_errno(ANDROID_EACCES);
        return -1;
    }
    let jit_capable =
        provider_owned && jit_capable_range(&mappings, address as usize, mapped_length);
    let replacement = if provider_owned {
        let Some(replacement) =
            replacement_table(&mappings, address as usize, mapped_length, Some(protection))
        else {
            set_errno(ANDROID_ENOMEM);
            trace_vm_failure(
                "mprotect",
                address,
                mapped_length,
                ANDROID_ENOMEM,
                &format!(
                    "mapping-capacity segments={}/{} jit-ranges={}/{}",
                    mappings.len(),
                    MAX_MAPPING_SEGMENTS,
                    jit_range_count(&mappings),
                    MAX_JIT_RANGES
                ),
            );
            return -1;
        };
        Some(replacement)
    } else {
        None
    };
    let emulated_rwx =
        jit_capable && protection & ANDROID_PROT_WRITE != 0 && protection & ANDROID_PROT_EXEC != 0;
    if emulated_rwx {
        // Darwin's hardened runtime rejects simultaneous writable+executable
        // protection for ordinary anonymous memory. Start the requested range
        // writable; signal recovery later toggles only the faulting host page
        // between RW and RX. Page-granular W^X avoids a process-wide phase
        // race when independent V8 compilation threads fault concurrently.
        host_protection = ANDROID_PROT_READ | ANDROID_PROT_WRITE;
    }
    let mut host_error = 0;
    // SAFETY: the side table proves every page in this rounded range is owned.
    if unsafe {
        darwin_art_host_vm_protect(address, mapped_length, host_protection, &mut host_error)
    } != 0
    {
        if trace {
            eprintln!("DARWIN VM: host mprotect failed errno={host_error}");
        }
        trace_vm_failure(
            "mprotect",
            address,
            mapped_length,
            host_error,
            "host-mprotect",
        );
        fail_host(&provider, host_error);
        return -1;
    }
    if provider_owned {
        let mut replacement = replacement.expect("provider-owned range has replacement metadata");
        let end = address as usize + mapped_length;
        for (_, mapping) in replacement.range_mut(address as usize..end) {
            mapping.jit_activated = emulated_rwx;
        }
        coalesce_mapping_segments(&mut replacement);
        *mappings = replacement;
        publish_jit_ranges(&mappings);
    }
    if protection & ANDROID_PROT_EXEC != 0 && !emulated_rwx {
        // SAFETY: the mapping remains live under the table lock for this range.
        unsafe { darwin_art_host_vm_invalidate_icache(address, mapped_length) };
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_bionic_vm_recover_jit_execution_fault(
    fault_address: usize,
    execution_fault: c_int,
) -> c_int {
    loop {
        let snapshot_index = ACTIVE_JIT_SNAPSHOT.load(Ordering::Acquire) & 1;
        let snapshot = &JIT_SNAPSHOTS[snapshot_index];
        snapshot.readers.fetch_add(1, Ordering::Acquire);
        if ACTIVE_JIT_SNAPSHOT.load(Ordering::Acquire) & 1 != snapshot_index {
            snapshot.readers.fetch_sub(1, Ordering::Release);
            continue;
        }
        let count = snapshot.count.load(Ordering::Relaxed).min(MAX_JIT_RANGES);
        let mut fault_matches = false;
        for range in &snapshot.ranges[..count] {
            let start = range.start.load(Ordering::Relaxed);
            let end = range.end.load(Ordering::Relaxed);
            let executable = range.executable.load(Ordering::Relaxed);
            let writable = range.writable.load(Ordering::Relaxed);
            let emulated_rwx = range.emulated_rwx.load(Ordering::Relaxed);
            if fault_address >= start
                && fault_address < end
                && emulated_rwx
                && if execution_fault != 0 {
                    executable
                } else {
                    writable
                }
            {
                fault_matches = true;
                break;
            }
        }
        if !fault_matches {
            snapshot.readers.fetch_sub(1, Ordering::Release);
            return 0;
        }
        let page_size = JIT_PAGE_SIZE.load(Ordering::Acquire);
        if page_size == 0 || !page_size.is_power_of_two() {
            snapshot.readers.fetch_sub(1, Ordering::Release);
            return 0;
        }
        let page_start = fault_address & !(page_size - 1);
        let host_protection = if execution_fault != 0 {
            ANDROID_PROT_READ | ANDROID_PROT_EXEC
        } else {
            ANDROID_PROT_READ | ANDROID_PROT_WRITE
        };
        let mut host_error = 0;
        if unsafe {
            darwin_art_host_vm_protect(
                page_start as *mut c_void,
                page_size,
                host_protection,
                &mut host_error,
            )
        } != 0
        {
            snapshot.readers.fetch_sub(1, Ordering::Release);
            return 0;
        }
        if execution_fault != 0 {
            unsafe { darwin_art_host_vm_invalidate_icache(page_start as *mut c_void, page_size) };
        }
        snapshot.readers.fetch_sub(1, Ordering::Release);
        return 1;
    }
}

#[unsafe(no_mangle)]
/// # Safety
/// `address` is an opaque guest value and is passed to Darwin only after the
/// side table proves ownership of the complete mapping.
pub unsafe extern "C" fn darwin_art_bionic_vm_madvise_core(
    address: *mut c_void,
    length: usize,
    advice: c_int,
) -> c_int {
    let trace = std::env::var_os("DARWIN_ART_VM_TRACE").is_some();
    trace_madvise_event(trace, "ENTRY", address, length, advice, 0);
    let Some(provider) = provider() else {
        trace_madvise_event(
            trace,
            "EARLY_NO_PROVIDER",
            address,
            length,
            advice,
            ANDROID_EIO,
        );
        trace_vm_failure(
            "madvise",
            address,
            length,
            ANDROID_EIO,
            "provider-unavailable",
        );
        return -1;
    };
    if address.is_null() || length == 0 || address as usize % provider.page_size != 0 {
        set_errno(ANDROID_EINVAL);
        trace_madvise_event(
            trace,
            "EARLY_INVALID_RANGE",
            address,
            length,
            advice,
            ANDROID_EINVAL,
        );
        return -1;
    }
    let host_advice = match advice {
        0..=3 => advice,
        // Linux MADV_DONTNEED on a private anonymous mapping guarantees
        // zero-fill on the next access. Darwin MADV_ZERO rejects some of
        // PartitionAlloc's ranges with EPERM, so those ranges are replaced
        // in place below instead of passing either platform's value through.
        4 => 4,
        8 => 5,
        9..=25 | 100 | 101 => {
            set_errno(ANDROID_EOPNOTSUPP);
            trace_madvise_event(
                trace,
                "EARLY_UNSUPPORTED_ADVICE",
                address,
                length,
                advice,
                ANDROID_EOPNOTSUPP,
            );
            return -1;
        }
        _ => {
            set_errno(ANDROID_EINVAL);
            trace_madvise_event(
                trace,
                "EARLY_INVALID_ADVICE",
                address,
                length,
                advice,
                ANDROID_EINVAL,
            );
            return -1;
        }
    };
    let mappings = match provider.mappings.lock() {
        Ok(value) => value,
        Err(_) => {
            provider.capability_failure.store(true, Ordering::Release);
            set_errno(ANDROID_EIO);
            trace_madvise_event(
                trace,
                "EARLY_MAPPING_LOCK",
                address,
                length,
                advice,
                ANDROID_EIO,
            );
            return -1;
        }
    };
    let Some(mapped_length) = round_length(length, provider.page_size) else {
        set_errno(ANDROID_EOVERFLOW);
        trace_madvise_event(
            trace,
            "EARLY_LENGTH_OVERFLOW",
            address,
            length,
            advice,
            ANDROID_EOVERFLOW,
        );
        return -1;
    };
    if !owned_range(&mappings, address as usize, mapped_length) {
        set_errno(ANDROID_ENOMEM);
        trace_madvise_event(
            trace,
            "EARLY_UNOWNED_RANGE",
            address,
            mapped_length,
            advice,
            ANDROID_ENOMEM,
        );
        trace_vm_failure(
            "madvise",
            address,
            mapped_length,
            ANDROID_ENOMEM,
            &format!("unowned-range mappings={}", mappings.len()),
        );
        return -1;
    }
    if advice == 4 {
        let start = address as usize;
        let end = start + mapped_length;
        let segments: Vec<(usize, usize, Mapping)> = mappings
            .range(..end)
            .filter_map(|(&base, mapping)| {
                let mapping_end = base + mapping.mapped_length;
                if mapping_end <= start {
                    return None;
                }
                let segment_start = base.max(start);
                let segment_end = mapping_end.min(end);
                Some((segment_start, segment_end - segment_start, *mapping))
            })
            .collect();
        for (segment_start, segment_length, mapping) in segments {
            let mut host_error = 0;
            let result = if mapping.anonymous && !mapping.jit_capable {
                let host_protection =
                    host_protection(mapping.protection).expect("registered protection is valid");
                // SAFETY: ownership was proven for this page-aligned segment;
                // MAP_FIXED atomically replaces it with zero-filled anonymous
                // pages while preserving the guest protection.
                let remapped = unsafe {
                    darwin_art_host_vm_remap_zero(
                        segment_start as *mut c_void,
                        segment_length,
                        host_protection,
                        &mut host_error,
                    )
                };
                if remapped == segment_start as *mut c_void {
                    0
                } else {
                    -1
                }
            } else {
                // SAFETY: ownership was proven. File mappings retain their
                // backing-object semantics. Potential/emulated RWX ranges use
                // native DONTNEED so a fixed remap cannot accidentally request
                // simultaneous host write+execute permission.
                unsafe {
                    darwin_art_host_vm_advise(
                        segment_start as *mut c_void,
                        segment_length,
                        host_advice,
                        &mut host_error,
                    )
                }
            };
            if result != 0 {
                trace_madvise_event(
                    trace,
                    "HOST_ERROR",
                    segment_start as *mut c_void,
                    segment_length,
                    advice,
                    host_error,
                );
                trace_vm_failure(
                    "madvise",
                    segment_start as *mut c_void,
                    segment_length,
                    host_error,
                    "host-advice",
                );
                fail_host(&provider, host_error);
                return -1;
            }
        }
        return 0;
    }
    let mut host_error = 0;
    // SAFETY: the side table proves this is the complete live owned mapping.
    if unsafe { darwin_art_host_vm_advise(address, mapped_length, host_advice, &mut host_error) }
        != 0
    {
        trace_madvise_event(
            trace,
            "HOST_ERROR",
            address,
            mapped_length,
            advice,
            host_error,
        );
        trace_vm_failure("madvise", address, mapped_length, host_error, "host-advice");
        fail_host(&provider, host_error);
        return -1;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    static JIT_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn executable_jit_mapping(length: usize) -> Mapping {
        Mapping {
            allocation_id: 1,
            requested_length: length,
            mapped_length: length,
            protection: ANDROID_PROT_READ | ANDROID_PROT_EXEC,
            anonymous: true,
            jit_capable: true,
            jit_activated: true,
        }
    }

    fn writable_jit_mapping(length: usize) -> Mapping {
        Mapping {
            allocation_id: 1,
            requested_length: length,
            mapped_length: length,
            protection: ANDROID_PROT_READ | ANDROID_PROT_WRITE,
            anonymous: true,
            jit_capable: true,
            jit_activated: false,
        }
    }

    #[test]
    fn jit_signal_snapshot_grows_and_shrinks() {
        let _serial = JIT_TEST_LOCK.lock().unwrap();
        clear_jit_ranges();
        let mut mappings = BTreeMap::new();
        mappings.insert(0x10000, executable_jit_mapping(0x4000));
        publish_jit_ranges(&mappings);
        let snapshot = &JIT_SNAPSHOTS[ACTIVE_JIT_SNAPSHOT.load(Ordering::Acquire) & 1];
        assert_eq!(snapshot.count.load(Ordering::Acquire), 1);
        assert_eq!(snapshot.ranges[0].start.load(Ordering::Acquire), 0x10000);
        assert_eq!(snapshot.ranges[0].end.load(Ordering::Acquire), 0x14000);
        assert!(snapshot.ranges[0].executable.load(Ordering::Acquire));
        assert!(!snapshot.ranges[0].writable.load(Ordering::Acquire));

        mappings.insert(0x20000, writable_jit_mapping(0x8000));
        publish_jit_ranges(&mappings);
        let snapshot = &JIT_SNAPSHOTS[ACTIVE_JIT_SNAPSHOT.load(Ordering::Acquire) & 1];
        assert_eq!(snapshot.count.load(Ordering::Acquire), 2);
        assert!(!snapshot.ranges[1].executable.load(Ordering::Acquire));
        assert!(snapshot.ranges[1].writable.load(Ordering::Acquire));

        mappings.remove(&0x10000);
        publish_jit_ranges(&mappings);
        let snapshot = &JIT_SNAPSHOTS[ACTIVE_JIT_SNAPSHOT.load(Ordering::Acquire) & 1];
        assert_eq!(snapshot.count.load(Ordering::Acquire), 1);
        assert_eq!(snapshot.ranges[0].start.load(Ordering::Acquire), 0x20000);
        clear_jit_ranges();
        let snapshot = &JIT_SNAPSHOTS[ACTIVE_JIT_SNAPSHOT.load(Ordering::Acquire) & 1];
        assert_eq!(snapshot.count.load(Ordering::Acquire), 0);
    }

    #[test]
    fn mapping_split_fails_closed_at_snapshot_capacity() {
        let mut mappings = BTreeMap::new();
        for index in 0..MAX_JIT_RANGES {
            let mut mapping = writable_jit_mapping(0x4000);
            mapping.allocation_id = index + 1;
            mappings.insert(0x10000 + index * 0x4000, mapping);
        }
        assert!(replacement_table(&mappings, 0x11000, 0x1000, None).is_none());
    }

    #[test]
    fn restored_protection_coalesces_only_the_original_allocation() {
        let mut mappings = BTreeMap::new();
        mappings.insert(0x10000, writable_jit_mapping(0x4000));

        replace_range(&mut mappings, 0x11000, 0x1000, Some(ANDROID_PROT_READ));
        assert_eq!(mappings.len(), 3);
        replace_range(
            &mut mappings,
            0x11000,
            0x1000,
            Some(ANDROID_PROT_READ | ANDROID_PROT_WRITE),
        );
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[&0x10000].mapped_length, 0x4000);

        let mut adjacent = writable_jit_mapping(0x1000);
        adjacent.allocation_id = 2;
        mappings.insert(0x14000, adjacent);
        coalesce_mapping_segments(&mut mappings);
        assert_eq!(mappings.len(), 2);
    }

    #[test]
    fn ordinary_mapping_table_is_not_limited_by_jit_snapshot_capacity() {
        let mut mappings = BTreeMap::new();
        for index in 0..(MAX_JIT_RANGES + 1) {
            mappings.insert(
                0x10000 + index * 0x4000,
                Mapping {
                    allocation_id: index + 1,
                    requested_length: 0x1000,
                    mapped_length: 0x1000,
                    protection: ANDROID_PROT_READ,
                    anonymous: false,
                    jit_capable: false,
                    jit_activated: false,
                },
            );
        }
        assert!(mapping_table_admissible(&mappings));
    }

    #[test]
    fn fixed_replacement_does_not_inherit_jit_metadata() {
        let mut mappings = BTreeMap::new();
        mappings.insert(0x10000, writable_jit_mapping(0x4000));
        let mut replacement = replacement_table(&mappings, 0x11000, 0x1000, None).unwrap();
        replacement.insert(
            0x11000,
            Mapping {
                allocation_id: 2,
                requested_length: 0x1000,
                mapped_length: 0x1000,
                protection: ANDROID_PROT_READ,
                anonymous: false,
                jit_capable: false,
                jit_activated: false,
            },
        );
        assert!(!replacement[&0x11000].jit_capable);
        assert!(replacement[&0x10000].jit_capable);
        assert!(replacement[&0x12000].jit_capable);
    }

    #[test]
    fn borrowed_range_registration_rejects_duplicate_envelopes() {
        let _serial = JIT_TEST_LOCK.lock().unwrap();
        let provider = Arc::new(Provider::new().unwrap());
        let _activation = provider.activate().unwrap();
        let address = 0x4000_0000 as *mut c_void;
        let length = 0x1000;
        assert_eq!(
            darwin_art_bionic_vm_register_borrowed_range(address, length),
            0
        );
        assert_eq!(
            darwin_art_bionic_vm_register_borrowed_range(address, length),
            -1
        );
        assert_eq!(
            darwin_art_bionic_vm_unregister_borrowed_range(address, length),
            0
        );
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn emulated_rwx_changes_faulting_host_page() {
        let _serial = JIT_TEST_LOCK.lock().unwrap();
        let provider = Arc::new(Provider::new().unwrap());
        let activation = provider.activate().unwrap();
        let length = provider.page_size();
        let address = unsafe {
            darwin_art_bionic_vm_mmap_core(
                std::ptr::null_mut(),
                length,
                0,
                ANDROID_MAP_PRIVATE | ANDROID_MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(address, MAP_FAILED);
        assert_eq!(
            unsafe {
                darwin_art_bionic_vm_mprotect_core(
                    address,
                    length,
                    ANDROID_PROT_READ | ANDROID_PROT_WRITE | ANDROID_PROT_EXEC,
                )
            },
            0
        );
        unsafe { address.cast::<u32>().write(0xd65f03c0) };
        assert_eq!(
            darwin_art_bionic_vm_recover_jit_execution_fault(address as usize, 1),
            1
        );
        let function: unsafe extern "C" fn() = unsafe { std::mem::transmute(address) };
        unsafe { function() };
        assert_eq!(
            darwin_art_bionic_vm_recover_jit_execution_fault(address as usize, 0),
            1
        );
        unsafe { address.cast::<u32>().write(0xd65f03c0) };
        assert_eq!(
            unsafe { darwin_art_bionic_vm_munmap_core(address, length) },
            0
        );
        drop(activation);
    }
}
