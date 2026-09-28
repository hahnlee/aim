//! The host module against the pinned MoltenVK (`cargo aim build
//! moltenvk`): the table resolves, forwarded calls create an instance, and
//! MoltenVK's dispatchable handles start with the loader magic the original
//! Android loader checks (`HWVULKAN_DISPATCH_MAGIC`), which is why the
//! driver passes them through unwrapped.

use std::ffi::{CStr, c_void};

use aim_host_vulkan::{MODULE, function, set_library_dir, table_identity};
use aim_hostcall::vulkan::{FN_INIT, Init};

const HWVULKAN_DISPATCH_MAGIC: usize = 0x01CD_C0DE;

/// Call a forwarded entry point with the given x-register values.
fn forward(name: &CStr, regs: &[u64]) -> i64 {
    let f = function(name).unwrap_or_else(|| panic!("{name:?} not in the table"));
    let mut image = regs.to_vec();
    // SAFETY: the register image of `name` with valid arguments.
    unsafe { (MODULE.call)(f, image.as_mut_ptr() as u64, (image.len() * 8) as u64) }
}

#[repr(C)]
struct InstanceCreateInfo {
    s_type: i32,
    next: *const c_void,
    flags: u32,
    app: *const c_void,
    layers: u32,
    layer_names: *const *const i8,
    extensions: u32,
    extension_names: *const *const i8,
}

#[repr(C)]
struct ApplicationInfo {
    s_type: i32,
    next: *const c_void,
    name: *const i8,
    version: u32,
    engine: *const i8,
    engine_version: u32,
    api_version: u32,
}

#[test]
fn instances_and_handles_through_the_table() {
    let Some(dir) = aim_paths::input(aim_paths::moltenvk().join("libMoltenVK.dylib"), "moltenvk")
    else {
        return;
    };
    set_library_dir(dir.parent().unwrap());
    let (hash, len) = table_identity();
    let mut bits = vec![0u64; (len as usize).div_ceil(64)];
    let mut init = Init {
        table_hash: hash,
        table_len: len,
        resolved: bits.as_mut_ptr() as u64,
        resolved_words: bits.len() as u64,
    };
    // SAFETY: FN_INIT's argument block.
    let r = unsafe { (MODULE.call)(FN_INIT, (&raw mut init) as u64, size_of::<Init>() as u64) };
    assert_eq!(r, 0);
    let resolved: u32 = bits.iter().map(|b| b.count_ones()).sum();
    assert_eq!(
        resolved as u64, len,
        "every table entry resolves in MoltenVK"
    );

    // MoltenVK reports no newer a version than the instance asks for.
    let app = ApplicationInfo {
        s_type: 0,
        next: std::ptr::null(),
        name: std::ptr::null(),
        version: 0,
        engine: std::ptr::null(),
        engine_version: 0,
        api_version: 1 << 22 | 4 << 12,
    };
    let info = InstanceCreateInfo {
        s_type: 1,
        next: std::ptr::null(),
        flags: 0,
        app: (&raw const app).cast(),
        layers: 0,
        layer_names: std::ptr::null(),
        extensions: 0,
        extension_names: std::ptr::null(),
    };
    let mut instance: *mut usize = std::ptr::null_mut();
    let r = forward(
        c"vkCreateInstance",
        &[(&raw const info) as u64, 0, (&raw mut instance) as u64],
    );
    assert_eq!(r as i32, 0);
    // SAFETY: a dispatchable handle points at its loader word.
    assert_eq!(unsafe { *instance }, HWVULKAN_DISPATCH_MAGIC);

    let mut count = 0u32;
    let r = forward(
        c"vkEnumeratePhysicalDevices",
        &[instance as u64, (&raw mut count) as u64, 0],
    );
    assert_eq!(r as i32, 0);
    assert!(count >= 1, "MoltenVK finds the Mac's GPU");
    let mut devices = vec![std::ptr::null_mut::<usize>(); count as usize];
    let r = forward(
        c"vkEnumeratePhysicalDevices",
        &[
            instance as u64,
            (&raw mut count) as u64,
            devices.as_mut_ptr() as u64,
        ],
    );
    assert_eq!(r as i32, 0);
    // SAFETY: as above.
    assert_eq!(unsafe { *devices[0] }, HWVULKAN_DISPATCH_MAGIC);

    // VkPhysicalDeviceProperties: apiVersion first, deviceName at 20.
    let mut props = vec![0u8; 1024];
    forward(
        c"vkGetPhysicalDeviceProperties",
        &[devices[0] as u64, props.as_mut_ptr() as u64],
    );
    let api = u32::from_le_bytes(props[..4].try_into().unwrap());
    let name = CStr::from_bytes_until_nul(&props[20..276]).unwrap();
    eprintln!(
        "{name:?}: Vulkan {}.{}.{}",
        api >> 22,
        (api >> 12) & 0x3ff,
        api & 0xfff
    );
    assert!(api >> 22 == 1 && (api >> 12) & 0x3ff >= 3);

    forward(c"vkDestroyInstance", &[instance as u64, 0]);
}
