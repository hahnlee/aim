//! The Android driver interface (`hwvulkan`, `vulkan/libvulkan/driver.cpp`
//! of the original loader): proc addresses, the extension lists, instance
//! and device creation, queue families, and the queues the driver submits
//! its own work to.
//!
//! MoltenVK's dispatchable handles start with a loader word holding
//! `ICD_LOADER_MAGIC`, which is `HWVULKAN_DISPATCH_MAGIC` (0x01CDC0DE): the
//! original loader checks it and overwrites it with its dispatch table, as
//! the Khronos loader does, so the handles pass through unwrapped. MoltenVK
//! rewrites the word only where it returns a handle (and in debug
//! callbacks, which the driver never registers), and the loader sets it
//! again there. The driver therefore never asks MoltenVK for a handle the
//! application may already hold (such as a queue) on its own.

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use aim_hostcall::vulkan::Init;

use crate::thunks::{self, PROCS, WITHHELD_DEVICE, WITHHELD_INSTANCE, host};
use crate::types::*;

/// Device extensions the driver implements itself.
const DRIVER_EXTENSIONS: &[(&str, u32)] = &[
    // Swapchain images for the loader's VK_KHR_swapchain. Spec version 8:
    // vkGetSwapchainGrallocUsage2ANDROID, and the loader's usage query
    // through vkGetPhysicalDeviceImageFormatProperties2.
    ("VK_ANDROID_native_buffer", 8),
    ("VK_ANDROID_external_memory_android_hardware_buffer", 5),
    // Required by the one above: queue family transfers to and from the
    // foreign queue family are ordinary barriers here.
    ("VK_EXT_queue_family_foreign", 1),
    ("VK_KHR_external_semaphore_fd", 1),
];
/// What AHardwareBuffer memory is imported as on the host.
const HOST_MEMORY: &str = "VK_EXT_external_memory_host";

/// Bitmap of the table entries the host resolved, after [`init`].
static RESOLVED: OnceLock<Option<Vec<u64>>> = OnceLock::new();

/// Load the host's MoltenVK (once per process). False when the host has
/// none, or a different table.
pub fn init() -> bool {
    RESOLVED
        .get_or_init(|| {
            let mut bits = vec![0u64; thunks::TABLE_LEN.div_ceil(64)];
            let mut args = Init {
                table_hash: thunks::TABLE_HASH,
                table_len: thunks::TABLE_LEN as u64,
                resolved: bits.as_mut_ptr() as u64,
                resolved_words: bits.len() as u64,
            };
            aim_hostcall::guest::vulkan_init(&mut args).ok()?;
            Some(bits)
        })
        .is_some()
}

fn proc_addr(name: *const c_char) -> *const c_void {
    if name.is_null() || !init() {
        return std::ptr::null();
    }
    // SAFETY: the loader passes a NUL-terminated name.
    let Ok(name) = unsafe { CStr::from_ptr(name) }.to_str() else {
        return std::ptr::null();
    };
    let Ok(i) = PROCS.binary_search_by(|p| p.name.cmp(name)) else {
        return std::ptr::null();
    };
    let p = &PROCS[i];
    let bits = RESOLVED.get().unwrap().as_ref().unwrap();
    if p.id >= 0 && bits[p.id as usize / 64] & (1 << (p.id % 64)) == 0 {
        return std::ptr::null();
    }
    p.addr
}

pub unsafe extern "C" fn vkGetInstanceProcAddr(
    _instance: VkDispatch,
    name: *const c_char,
) -> *const c_void {
    proc_addr(name)
}

pub unsafe extern "C" fn vkGetDeviceProcAddr(
    _device: VkDispatch,
    name: *const c_char,
) -> *const c_void {
    proc_addr(name)
}

/// Copy `src` to a Vulkan output array (`count` in and out).
///
/// # Safety
/// `count` must be valid; `out`, when not null, must hold `*count` values.
pub unsafe fn write_out<T: Copy>(src: &[T], count: *mut u32, out: *mut T) -> VkResult {
    // SAFETY: caller contract.
    unsafe {
        if out.is_null() {
            *count = src.len() as u32;
            return VK_SUCCESS;
        }
        let n = (*count as usize).min(src.len());
        std::ptr::copy_nonoverlapping(src.as_ptr(), out, n);
        *count = n as u32;
        if n < src.len() {
            VK_INCOMPLETE
        } else {
            VK_SUCCESS
        }
    }
}

/// Read a Vulkan output array through `query(count, out)`.
fn read_all<T: Copy>(
    mut query: impl FnMut(*mut u32, *mut T) -> VkResult,
) -> Result<Vec<T>, VkResult> {
    loop {
        let mut n = 0u32;
        let r = query(&mut n, std::ptr::null_mut());
        if r < 0 {
            return Err(r);
        }
        let mut v = Vec::with_capacity(n as usize);
        let r = query(&mut n, v.as_mut_ptr());
        if r < 0 {
            return Err(r);
        }
        if r == VK_INCOMPLETE {
            continue;
        }
        // SAFETY: the callee wrote `n` values.
        unsafe { v.set_len(n as usize) };
        return Ok(v);
    }
}

fn withheld(list: &[&str], name: &[u8]) -> bool {
    list.iter().any(|w| w.as_bytes() == name)
}

pub unsafe extern "C" fn vkEnumerateInstanceExtensionProperties(
    layer: *const c_char,
    count: *mut u32,
    out: *mut VkExtensionProperties,
) -> VkResult {
    if !layer.is_null() {
        return VK_ERROR_LAYER_NOT_PRESENT;
    }
    let host = match read_all(|n, p: *mut VkExtensionProperties| unsafe {
        host::vkEnumerateInstanceExtensionProperties(std::ptr::null(), n.cast(), p.cast())
    }) {
        Ok(v) => v,
        Err(r) => return r,
    };
    let offered: Vec<_> = host
        .into_iter()
        .filter(|e| !withheld(WITHHELD_INSTANCE, e.name()))
        .collect();
    // SAFETY: the caller's output array.
    unsafe { write_out(&offered, count, out) }
}

/// The device extensions of `physical_device`: MoltenVK's, less the
/// withheld ones, plus the driver's own.
fn device_extensions(physical_device: VkDispatch) -> Result<Vec<VkExtensionProperties>, VkResult> {
    let host = read_all(|n, p: *mut VkExtensionProperties| unsafe {
        host::vkEnumerateDeviceExtensionProperties(
            physical_device,
            std::ptr::null(),
            n.cast(),
            p.cast(),
        )
    })?;
    let host_memory = host.iter().any(|e| e.name() == HOST_MEMORY.as_bytes());
    let mut offered: Vec<_> = host
        .into_iter()
        .filter(|e| !withheld(WITHHELD_DEVICE, e.name()))
        .collect();
    for &(name, spec) in DRIVER_EXTENSIONS {
        if name == "VK_ANDROID_external_memory_android_hardware_buffer" && !host_memory {
            continue;
        }
        if !offered.iter().any(|e| e.name() == name.as_bytes()) {
            offered.push(VkExtensionProperties::new(name, spec));
        }
    }
    Ok(offered)
}

pub unsafe extern "C" fn vkEnumerateDeviceExtensionProperties(
    physical_device: VkDispatch,
    layer: *const c_char,
    count: *mut u32,
    out: *mut VkExtensionProperties,
) -> VkResult {
    if !layer.is_null() {
        return VK_ERROR_LAYER_NOT_PRESENT;
    }
    match device_extensions(physical_device) {
        // SAFETY: the caller's output array.
        Ok(v) => unsafe { write_out(&v, count, out) },
        Err(r) => r,
    }
}

pub unsafe extern "C" fn vkCreateInstance(
    info: *const VkInstanceCreateInfo,
    _allocator: *const c_void,
    instance: *mut VkDispatch,
) -> VkResult {
    // SAFETY: the loader's create info.
    let mut info = unsafe { *info };
    // Callbacks into guest code, which host code never makes (their
    // extensions are withheld; a layer may still chain them).
    // SAFETY: the chain is the caller's for this call.
    let _unlinked = unsafe {
        Unlinked::new(&mut info.pNext, |s| {
            matches!(
                (*s).sType,
                stype::DEBUG_UTILS_MESSENGER_CREATE_INFO_EXT
                    | stype::DEBUG_REPORT_CALLBACK_CREATE_INFO_EXT
                    | stype::LAYER_SETTINGS_CREATE_INFO_EXT
            )
        })
    };
    // SAFETY: a valid create info.
    unsafe { host::vkCreateInstance((&raw const info).cast(), std::ptr::null(), instance.cast()) }
}

/// Queue families. MoltenVK has one queue per family (a `VkQueue` is an
/// `MTLCommandQueue`, and Metal has no queue families) and offers several
/// alike general-purpose families instead, while HWUI, like other Vulkan
/// applications, asks for several queues of its graphics family. When all
/// of a physical device's families are alike, the driver presents them as
/// one family of all their queues: its queue `i` is queue `i % per` of the
/// host's family `i / per`.
///
/// That family is index 0, as the host's first family is, so every other
/// place a family index appears means the same to MoltenVK: command pools,
/// barriers (an ownership transfer is between two families, and there is
/// one; `VK_QUEUE_FAMILY_EXTERNAL` and `_FOREIGN_EXT` pass as they are) and
/// sharing lists (concurrent sharing names two families or more, so there
/// is none). A command buffer from a pool of family 0 may be submitted to
/// any of its queues: MoltenVK records command buffers apart from queues
/// and encodes them into the queue they are submitted to.
#[derive(Clone, Copy)]
struct Merged {
    families: u32,
    per: u32,
}

impl Merged {
    fn queues(self) -> u32 {
        self.families * self.per
    }

    /// The host's family and index of queue `index` of the one family.
    fn queue(self, index: u32) -> (u32, u32) {
        (index / self.per, index % self.per)
    }
}

fn merged(physical: VkDispatch) -> Option<Merged> {
    let host = read_all(|n, p: *mut VkQueueFamilyProperties| {
        // SAFETY: the host fills up to `*n` properties.
        unsafe { host::vkGetPhysicalDeviceQueueFamilyProperties(physical, n.cast(), p.cast()) };
        VK_SUCCESS
    })
    .ok()?;
    let first = *host.first()?;
    (host.len() > 1 && host.iter().all(|f| *f == first)).then_some(Merged {
        families: host.len() as u32,
        per: first.queueCount,
    })
}

pub unsafe extern "C" fn vkGetPhysicalDeviceQueueFamilyProperties(
    physical: VkDispatch,
    count: *mut u32,
    out: *mut VkQueueFamilyProperties,
) {
    // SAFETY (all): the application's count and output array.
    let Some(m) = merged(physical) else {
        return unsafe {
            host::vkGetPhysicalDeviceQueueFamilyProperties(physical, count.cast(), out.cast())
        };
    };
    unsafe {
        if out.is_null() || *count == 0 {
            *count = out.is_null() as u32;
            return;
        }
        *count = 1;
        host::vkGetPhysicalDeviceQueueFamilyProperties(physical, count.cast(), out.cast());
        (*out).queueCount = m.queues();
    }
}

pub unsafe extern "C" fn vkGetPhysicalDeviceQueueFamilyProperties2(
    physical: VkDispatch,
    count: *mut u32,
    out: *mut VkQueueFamilyProperties2,
) {
    // SAFETY (all): as above; the host fills the first family's chain,
    // which is every family's.
    let Some(m) = merged(physical) else {
        return unsafe {
            host::vkGetPhysicalDeviceQueueFamilyProperties2(physical, count.cast(), out.cast())
        };
    };
    unsafe {
        if out.is_null() || *count == 0 {
            *count = out.is_null() as u32;
            return;
        }
        *count = 1;
        host::vkGetPhysicalDeviceQueueFamilyProperties2(physical, count.cast(), out.cast());
        (*out).queueFamilyProperties.queueCount = m.queues();
    }
}

/// The host's queue create infos for the application's `requested`.
fn host_queues(m: Merged, requested: &[VkDeviceQueueCreateInfo]) -> Vec<VkDeviceQueueCreateInfo> {
    let mut out = Vec::new();
    for q in requested {
        if q.queueFamilyIndex != 0 {
            // Not a family of the device; the host refuses it.
            out.push(*q);
            continue;
        }
        for family in 0..q.queueCount.div_ceil(m.per) {
            let first = family * m.per;
            out.push(VkDeviceQueueCreateInfo {
                queueFamilyIndex: family,
                queueCount: m.per.min(q.queueCount - first),
                // SAFETY: within the application's queueCount priorities.
                pQueuePriorities: unsafe { q.pQueuePriorities.add(first as usize) },
                ..*q
            });
        }
    }
    out
}

/// A timeline semaphore of the driver's own and the last value it was
/// given to reach.
struct Timeline {
    semaphore: u64,
    value: u64,
}

/// What the driver keeps per device.
struct Device {
    physical: usize,
    merged: Option<Merged>,
    /// The first queue the application got, which the driver's own
    /// submissions use.
    queue: Option<usize>,
    /// The timelines that connect submissions and sync_files.
    timelines: Vec<Timeline>,
}

struct State {
    devices: HashMap<usize, Device>,
    /// Queue -> (device, lock). The application synchronizes its use of a
    /// queue; the driver's own submissions (from vkAcquireNextImageKHR and
    /// sync-fd semaphores) take the same lock.
    queues: HashMap<usize, (usize, Arc<Mutex<()>>)>,
}

fn state() -> MutexGuard<'static, State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE
        .get_or_init(|| {
            Mutex::new(State {
                devices: HashMap::new(),
                queues: HashMap::new(),
            })
        })
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// The physical device of `device`.
pub fn physical_device(device: VkDispatch) -> Option<VkDispatch> {
    state()
        .devices
        .get(&(device as usize))
        .map(|d| d.physical as VkDispatch)
}

pub unsafe extern "C" fn vkCreateDevice(
    physical_device: VkDispatch,
    info: *const VkDeviceCreateInfo,
    _allocator: *const c_void,
    device: *mut VkDispatch,
) -> VkResult {
    let host_exts = match read_all(|n, p: *mut VkExtensionProperties| unsafe {
        host::vkEnumerateDeviceExtensionProperties(
            physical_device,
            std::ptr::null(),
            n.cast(),
            p.cast(),
        )
    }) {
        Ok(v) => v,
        Err(r) => return r,
    };
    // SAFETY: the loader's create info and its extension names.
    let mut info = unsafe { *info };
    let requested = unsafe {
        std::slice::from_raw_parts(
            info.ppEnabledExtensionNames,
            info.enabledExtensionCount as usize,
        )
    };
    let mut names: Vec<*const c_char> = requested
        .iter()
        .copied()
        // SAFETY: NUL-terminated names.
        .filter(|&n| {
            let n = unsafe { CStr::from_ptr(n) }.to_bytes();
            !DRIVER_EXTENSIONS.iter().any(|(d, _)| d.as_bytes() == n)
        })
        .collect();
    if host_exts.iter().any(|e| e.name() == HOST_MEMORY.as_bytes())
        && !names
            .iter()
            .any(|&n| unsafe { CStr::from_ptr(n) }.to_bytes() == HOST_MEMORY.as_bytes())
    {
        names.push(c"VK_EXT_external_memory_host".as_ptr());
    }
    info.enabledExtensionCount = names.len() as u32;
    info.ppEnabledExtensionNames = names.as_ptr();
    let merged = merged(physical_device);
    let queues;
    if let Some(m) = merged {
        // SAFETY: the application's queue create infos.
        queues = host_queues(m, unsafe {
            std::slice::from_raw_parts(
                info.pQueueCreateInfos.cast(),
                info.queueCreateInfoCount as usize,
            )
        });
        info.queueCreateInfoCount = queues.len() as u32;
        info.pQueueCreateInfos = queues.as_ptr().cast();
    }
    // SAFETY: a valid create info.
    let r = unsafe {
        host::vkCreateDevice(
            physical_device,
            (&raw const info).cast(),
            std::ptr::null(),
            device.cast(),
        )
    };
    if r == VK_SUCCESS {
        // SAFETY: set by a successful call.
        let d = unsafe { *device } as usize;
        state().devices.insert(
            d,
            Device {
                physical: physical_device as usize,
                merged,
                queue: None,
                timelines: Vec::new(),
            },
        );
    }
    r
}

pub unsafe extern "C" fn vkDestroyDevice(device: VkDispatch, _allocator: *const c_void) {
    let gone = {
        let mut s = state();
        s.queues.retain(|_, (d, _)| *d != device as usize);
        s.devices.remove(&(device as usize))
    };
    // SAFETY: the application's device and the driver's semaphores.
    unsafe {
        for t in gone.iter().flat_map(|d| &d.timelines) {
            host::vkDestroySemaphore(device, t.semaphore, std::ptr::null());
        }
        host::vkDestroyDevice(device, std::ptr::null());
    }
}

fn got_queue(device: VkDispatch, queue: VkDispatch) {
    if queue.is_null() {
        return;
    }
    let mut s = state();
    s.queues
        .entry(queue as usize)
        .or_insert_with(|| (device as usize, Arc::default()));
    if let Some(d) = s.devices.get_mut(&(device as usize))
        && d.queue.is_none()
    {
        d.queue = Some(queue as usize);
    }
}

/// The host's family and index of the application's queue `index` of
/// `family`.
fn host_queue(device: VkDispatch, family: u32, index: u32) -> (u32, u32) {
    match state()
        .devices
        .get(&(device as usize))
        .and_then(|d| d.merged)
    {
        Some(m) if family == 0 => m.queue(index),
        _ => (family, index),
    }
}

pub unsafe extern "C" fn vkGetDeviceQueue(
    device: VkDispatch,
    family: u32,
    index: u32,
    queue: *mut VkDispatch,
) {
    let (family, index) = host_queue(device, family, index);
    // SAFETY: the application's arguments.
    unsafe {
        host::vkGetDeviceQueue(device, family, index, queue.cast());
        got_queue(device, *queue);
    }
}

pub unsafe extern "C" fn vkGetDeviceQueue2(
    device: VkDispatch,
    info: *const VkDeviceQueueInfo2,
    queue: *mut VkDispatch,
) {
    // SAFETY: the application's arguments.
    let mut info = unsafe { *info };
    (info.queueFamilyIndex, info.queueIndex) =
        host_queue(device, info.queueFamilyIndex, info.queueIndex);
    // SAFETY: as above.
    unsafe {
        host::vkGetDeviceQueue2(device, (&raw const info).cast(), queue.cast());
        got_queue(device, *queue);
    }
}

/// The lock of `queue`, and its device.
fn queue_lock(queue: VkDispatch) -> Option<(VkDispatch, Arc<Mutex<()>>)> {
    state()
        .queues
        .get(&(queue as usize))
        .map(|(d, l)| (*d as VkDispatch, l.clone()))
}

fn locked<R>(queue: VkDispatch, f: impl FnOnce() -> R) -> R {
    match queue_lock(queue) {
        Some((_, lock)) => {
            let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
            f()
        }
        None => f(),
    }
}

pub unsafe extern "C" fn vkQueueSubmit(
    queue: VkDispatch,
    n: u32,
    submits: *const c_void,
    fence: u64,
) -> VkResult {
    // SAFETY: the application's arguments.
    locked(queue, || unsafe {
        host::vkQueueSubmit(queue, n, submits, fence)
    })
}

pub unsafe extern "C" fn vkQueueSubmit2(
    queue: VkDispatch,
    n: u32,
    submits: *const c_void,
    fence: u64,
) -> VkResult {
    // SAFETY: the application's arguments.
    locked(queue, || unsafe {
        host::vkQueueSubmit2(queue, n, submits, fence)
    })
}

pub unsafe extern "C" fn vkQueueBindSparse(
    queue: VkDispatch,
    n: u32,
    info: *const c_void,
    fence: u64,
) -> VkResult {
    // SAFETY: the application's arguments.
    locked(queue, || unsafe {
        host::vkQueueBindSparse(queue, n, info, fence)
    })
}

pub unsafe extern "C" fn vkQueueWaitIdle(queue: VkDispatch) -> VkResult {
    // SAFETY: the application's queue.
    locked(queue, || unsafe { host::vkQueueWaitIdle(queue) })
}

/// A semaphore operation of the driver's own submissions: a binary
/// semaphore (value 0) or a value of a timeline semaphore.
pub type Op = (u64, u64);

/// Submit, on `queue` (or the device's first queue), a batch that waits
/// for `waits` and signals `signals` and `fence`.
pub fn submit(
    device: VkDispatch,
    queue: Option<VkDispatch>,
    waits: &[Op],
    signals: &[Op],
    fence: u64,
) -> VkResult {
    let Some(queue) = queue.or_else(|| first_queue(device)) else {
        // The application has no queue yet; nothing it submitted can be
        // pending.
        return VK_ERROR_INITIALIZATION_FAILED;
    };
    let Some((_, lock)) = queue_lock(queue) else {
        return VK_ERROR_INITIALIZATION_FAILED;
    };
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    let (wait_semaphores, wait_values): (Vec<u64>, Vec<u64>) = waits.iter().copied().unzip();
    let (signal_semaphores, signal_values): (Vec<u64>, Vec<u64>) = signals.iter().copied().unzip();
    let stages = vec![PIPELINE_STAGE_ALL_COMMANDS; waits.len()];
    let values = VkTimelineSemaphoreSubmitInfo {
        sType: stype::TIMELINE_SEMAPHORE_SUBMIT_INFO,
        pNext: std::ptr::null(),
        waitSemaphoreValueCount: waits.len() as u32,
        pWaitSemaphoreValues: wait_values.as_ptr(),
        signalSemaphoreValueCount: signals.len() as u32,
        pSignalSemaphoreValues: signal_values.as_ptr(),
    };
    let batch = VkSubmitInfo {
        sType: stype::SUBMIT_INFO,
        pNext: (&raw const values).cast(),
        waitSemaphoreCount: waits.len() as u32,
        pWaitSemaphores: wait_semaphores.as_ptr(),
        pWaitDstStageMask: stages.as_ptr(),
        commandBufferCount: 0,
        pCommandBuffers: std::ptr::null(),
        signalSemaphoreCount: signals.len() as u32,
        pSignalSemaphores: signal_semaphores.as_ptr(),
    };
    // SAFETY: a valid batch on the application's queue.
    unsafe { host::vkQueueSubmit(queue, 1, (&raw const batch).cast(), fence) }
}

fn first_queue(device: VkDispatch) -> Option<VkDispatch> {
    state()
        .devices
        .get(&(device as usize))
        .and_then(|d| d.queue)
        .map(|q| q as VkDispatch)
}

/// A timeline of `device` that no pending operation uses, and the next
/// value it is to reach. MoltenVK makes a timeline semaphore of an
/// `MTLSharedEvent` whatever features the application enabled.
fn timeline(device: VkDispatch) -> Result<Op, VkResult> {
    let mut s = state();
    let d = s
        .devices
        .get_mut(&(device as usize))
        .ok_or(VK_ERROR_INITIALIZATION_FAILED)?;
    for t in &mut d.timelines {
        let mut now = 0u64;
        // SAFETY: the driver's semaphore.
        unsafe { host::vkGetSemaphoreCounterValue(device, t.semaphore, (&raw mut now).cast()) };
        if now >= t.value {
            t.value += 1;
            return Ok((t.semaphore, t.value));
        }
    }
    let kind = VkSemaphoreTypeCreateInfo {
        sType: stype::SEMAPHORE_TYPE_CREATE_INFO,
        pNext: std::ptr::null(),
        semaphoreType: SEMAPHORE_TYPE_TIMELINE,
        initialValue: 0,
    };
    let info = VkSemaphoreCreateInfo {
        sType: stype::SEMAPHORE_CREATE_INFO,
        pNext: (&raw const kind).cast(),
        flags: 0,
    };
    let mut semaphore = 0u64;
    // SAFETY: a valid create info.
    let r = unsafe {
        host::vkCreateSemaphore(
            device,
            (&raw const info).cast(),
            std::ptr::null(),
            (&raw mut semaphore).cast(),
        )
    };
    if r != VK_SUCCESS {
        return Err(r);
    }
    d.timelines.push(Timeline {
        semaphore,
        value: 1,
    });
    Ok((semaphore, 1))
}

/// A sync_file fd that signals once `waits` have signaled and, on `queue`
/// (or the device's first queue), what was submitted before.
pub fn fence_after(
    device: VkDispatch,
    queue: Option<VkDispatch>,
    waits: &[Op],
) -> Result<i32, VkResult> {
    let device = match queue {
        Some(q) => queue_lock(q).ok_or(VK_ERROR_INITIALIZATION_FAILED)?.0,
        None => device,
    };
    let op = timeline(device)?;
    let r = submit(device, queue, waits, &[op], 0);
    let mut args = aim_hostcall::vulkan::Timeline {
        device: device as u64,
        semaphore: op.0,
        value: op.1,
        ..Default::default()
    };
    if r != VK_SUCCESS {
        // Nothing will reach the value: reach it here, so the timeline is
        // free again.
        signal_now(device, op);
        return Err(r);
    }
    aim_hostcall::guest::vulkan_fence(&mut args).map_err(|_| VK_ERROR_OUT_OF_HOST_MEMORY)
}

/// Submit, on the device's first queue, a batch that signals `signals` and
/// `fence` once the sync_file `fd` has signaled. Takes `fd`; -1 is a
/// signaled sync_file.
pub fn signal_after(device: VkDispatch, fd: i32, signals: &[Op], fence: u64) -> VkResult {
    if fd < 0 {
        return submit(device, None, &[], signals, fence);
    }
    if signals.is_empty() && fence == 0 {
        // SAFETY: the fd is ours now.
        unsafe { libc::close(fd) };
        return VK_SUCCESS;
    }
    let r = timeline(device).and_then(|op| {
        let mut args = aim_hostcall::vulkan::Timeline {
            device: device as u64,
            semaphore: op.0,
            value: op.1,
            fd,
            ..Default::default()
        };
        if aim_hostcall::guest::vulkan_signal(&mut args).is_err() {
            signal_now(device, op);
            return Err(VK_ERROR_INVALID_EXTERNAL_HANDLE);
        }
        Ok(submit(device, None, &[op], signals, fence))
    });
    // SAFETY: the fd is ours now; the host holds its own reference.
    unsafe { libc::close(fd) };
    r.unwrap_or_else(|e| e)
}

/// Reach a timeline value on the CPU.
fn signal_now(device: VkDispatch, (semaphore, value): Op) {
    let info = VkSemaphoreSignalInfo {
        sType: stype::SEMAPHORE_SIGNAL_INFO,
        pNext: std::ptr::null(),
        semaphore,
        value,
    };
    // SAFETY: the driver's semaphore.
    unsafe { host::vkSignalSemaphore(device, (&raw const info).cast()) };
}
