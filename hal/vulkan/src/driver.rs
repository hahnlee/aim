//! The Android driver interface (`hwvulkan`, `vulkan/libvulkan/driver.cpp`
//! of the original loader): proc addresses, the extension lists, instance
//! and device creation, and the queues the driver submits its own work to.
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

/// What the driver keeps per device.
struct Device {
    physical: usize,
    /// The first queue the application got, which the driver's own
    /// submissions use, and a fence for waiting on them.
    queue: Option<usize>,
    fence: u64,
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
                queue: None,
                fence: 0,
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
    // SAFETY: the application's device.
    unsafe {
        if let Some(d) = gone
            && d.fence != 0
        {
            host::vkDestroyFence(device, d.fence, std::ptr::null());
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

pub unsafe extern "C" fn vkGetDeviceQueue(
    device: VkDispatch,
    family: u32,
    index: u32,
    queue: *mut VkDispatch,
) {
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
    unsafe {
        host::vkGetDeviceQueue2(device, info.cast(), queue.cast());
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

/// Submit, on `queue` (or the device's first queue), a batch that waits
/// for `waits` and signals `signals` and `fence`; with `wait`, wait on the
/// CPU until it has run (and everything submitted before it).
pub fn submit(
    device: VkDispatch,
    queue: Option<VkDispatch>,
    waits: &[u64],
    signals: &[u64],
    fence: u64,
    wait: bool,
) -> VkResult {
    let queue = match queue {
        Some(q) => q,
        None => match state()
            .devices
            .get(&(device as usize))
            .and_then(|d| d.queue)
        {
            Some(q) => q as VkDispatch,
            // The application has no queue yet; nothing it submitted can
            // be pending.
            None => return VK_ERROR_INITIALIZATION_FAILED,
        },
    };
    let Some((device, lock)) = queue_lock(queue) else {
        return VK_ERROR_INITIALIZATION_FAILED;
    };
    let _held = lock.lock().unwrap_or_else(|e| e.into_inner());
    let stages = vec![PIPELINE_STAGE_ALL_COMMANDS; waits.len()];
    let batch = VkSubmitInfo {
        sType: stype::SUBMIT_INFO,
        pNext: std::ptr::null(),
        waitSemaphoreCount: waits.len() as u32,
        pWaitSemaphores: waits.as_ptr(),
        pWaitDstStageMask: stages.as_ptr(),
        commandBufferCount: 0,
        pCommandBuffers: std::ptr::null(),
        signalSemaphoreCount: signals.len() as u32,
        pSignalSemaphores: signals.as_ptr(),
    };
    let own = if wait {
        match own_fence(device) {
            Ok(f) => f,
            Err(r) => return r,
        }
    } else {
        fence
    };
    // SAFETY: a valid batch on the application's queue.
    let r = unsafe { host::vkQueueSubmit(queue, 1, (&raw const batch).cast(), own) };
    if r != VK_SUCCESS || !wait {
        return r;
    }
    // SAFETY: the device's own fence.
    unsafe {
        let r = host::vkWaitForFences(device, 1, (&raw const own).cast(), 1, u64::MAX);
        host::vkResetFences(device, 1, (&raw const own).cast());
        r
    }
}

/// The device's fence for the driver's own waits (under a queue lock).
fn own_fence(device: VkDispatch) -> Result<u64, VkResult> {
    if let Some(d) = state().devices.get(&(device as usize))
        && d.fence != 0
    {
        return Ok(d.fence);
    }
    let info = VkFenceCreateInfo {
        sType: stype::FENCE_CREATE_INFO,
        pNext: std::ptr::null(),
        flags: 0,
    };
    let mut fence = 0u64;
    // SAFETY: a valid create info.
    let r = unsafe {
        host::vkCreateFence(
            device,
            (&raw const info).cast(),
            std::ptr::null(),
            (&raw mut fence).cast(),
        )
    };
    if r != VK_SUCCESS {
        return Err(r);
    }
    match state().devices.get_mut(&(device as usize)) {
        Some(d) => d.fence = fence,
        None => return Err(VK_ERROR_INITIALIZATION_FAILED),
    }
    Ok(fence)
}

/// Wait for a sync-file fd (consumed; -1 is signaled) on the CPU.
pub fn wait_fd(fd: i32) {
    if fd < 0 {
        return;
    }
    let mut p = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: a pollfd on the stack; the fd is ours to close.
    unsafe {
        while libc::poll(&mut p, 1, -1) < 0 && *libc::__errno() == libc::EINTR {}
        libc::close(fd);
    }
}
