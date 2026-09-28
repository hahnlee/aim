//! `vulkan.aim.so`: the Vulkan driver of the derived image
//! (`docs/vulkan-driver.md`). The original `/system/lib64/libvulkan.so`
//! loads it from `/vendor/lib64/hw` for `ro.hardware.vulkan=aim` and opens
//! it through the `hwvulkan` HAL interface ([`HMI`]).
//!
//! Every Vulkan command is a thunk that makes one host call to the host
//! module `vulkan`, which calls the host's MoltenVK (Vulkan on Metal). Guest
//! pointers are host pointers, so arguments pass through untouched. The
//! thunks are generated from the Khronos registry
//! (`tools/gen-vulkan-thunks.py`); the other modules hold what the driver
//! does itself: the Android driver interface ([`driver`]), swapchain images
//! over gralloc buffers ([`native_buffer`]), AHardwareBuffer memory
//! ([`ahb`]) and sync-fd semaphores ([`sync_fd`]).

// Entry points keep their Vulkan names, and their safety contract is
// Vulkan's valid usage.
#![allow(non_snake_case, clippy::missing_safety_doc)]

pub mod ahb;
pub mod buffers;
pub mod driver;
mod hostcall;
pub mod native_buffer;
pub mod sync_fd;
#[rustfmt::skip]
pub mod thunks;
pub mod types;

use core::ffi::{c_char, c_int, c_void};

/// `hw_module_t` (`<hardware/hardware.h>`, LP64).
#[repr(C)]
pub struct HwModule {
    tag: u32,
    module_api_version: u16,
    hal_api_version: u16,
    id: *const c_char,
    name: *const c_char,
    author: *const c_char,
    methods: *const HwModuleMethods,
    /// Set by the loader to the library's handle.
    dso: *mut c_void,
    reserved: [u64; 32 - 7],
}

#[repr(C)]
pub struct HwModuleMethods {
    open: unsafe extern "C" fn(*const HwModule, *const c_char, *mut *mut HwDevice) -> c_int,
}

/// `hw_device_t`.
#[repr(C)]
pub struct HwDevice {
    tag: u32,
    version: u32,
    module: *const HwModule,
    reserved: [u64; 12],
    close: unsafe extern "C" fn(*mut HwDevice) -> c_int,
}

/// `hwvulkan_device_t` (`<hardware/hwvulkan.h>`).
#[repr(C)]
pub struct HwVulkanDevice {
    common: HwDevice,
    enumerate_instance_extension_properties:
        unsafe extern "C" fn(*const c_char, *mut u32, *mut types::VkExtensionProperties) -> i32,
    create_instance: unsafe extern "C" fn(
        *const types::VkInstanceCreateInfo,
        *const c_void,
        *mut types::VkDispatch,
    ) -> i32,
    get_instance_proc_addr: unsafe extern "C" fn(types::VkDispatch, *const c_char) -> *const c_void,
}

const HARDWARE_MODULE_TAG: u32 = u32::from_be_bytes(*b"HWMT");
const HARDWARE_DEVICE_TAG: u32 = u32::from_be_bytes(*b"HWDT");
/// `HWVULKAN_MODULE_API_VERSION_0_1`.
const MODULE_API_VERSION: u16 = 0x0001;
/// `HWVULKAN_DEVICE_API_VERSION_0_1`.
const DEVICE_API_VERSION: u32 = 1 << 16;

static METHODS: HwModuleMethods = HwModuleMethods { open };

/// The HAL module (`HAL_MODULE_INFO_SYM`), `hwvulkan_module_t`. The loader
/// writes `dso`, so it is mutable; Rust code never touches it.
#[unsafe(no_mangle)]
pub static mut HMI: HwModule = HwModule {
    tag: HARDWARE_MODULE_TAG,
    module_api_version: MODULE_API_VERSION,
    hal_api_version: 0,
    id: c"vulkan".as_ptr(),
    name: c"AIM Vulkan driver (MoltenVK)".as_ptr(),
    author: c"AIM".as_ptr(),
    methods: &METHODS,
    dso: core::ptr::null_mut(),
    reserved: [0; 25],
};

struct Device(HwVulkanDevice);

// SAFETY: immutable after construction; the pointers are statics.
unsafe impl Sync for Device {}

static DEVICE: Device = Device(HwVulkanDevice {
    common: HwDevice {
        tag: HARDWARE_DEVICE_TAG,
        version: DEVICE_API_VERSION,
        // SAFETY: only the address of the module is taken.
        module: &raw const HMI,
        reserved: [0; 12],
        close,
    },
    enumerate_instance_extension_properties: driver::vkEnumerateInstanceExtensionProperties,
    create_instance: driver::vkCreateInstance,
    get_instance_proc_addr: driver::vkGetInstanceProcAddr,
});

/// `hw_module_methods_t::open` for `HWVULKAN_DEVICE_0` ("vk0"). Fails with
/// `-ENODEV` when the host has no MoltenVK.
unsafe extern "C" fn open(
    _module: *const HwModule,
    id: *const c_char,
    device: *mut *mut HwDevice,
) -> c_int {
    // SAFETY: the loader passes a NUL-terminated id.
    if id.is_null() || unsafe { core::ffi::CStr::from_ptr(id) } != c"vk0" {
        return -libc::EINVAL;
    }
    if !driver::init() {
        return -libc::ENODEV;
    }
    // SAFETY: the loader's out pointer; the device is never written.
    unsafe { *device = &DEVICE.0.common as *const HwDevice as *mut HwDevice };
    0
}

unsafe extern "C" fn close(_device: *mut HwDevice) -> c_int {
    0
}
