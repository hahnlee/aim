#![forbid(unsafe_op_in_unsafe_fn)]

use std::ffi::{c_char, c_void};

/* Keep one production core entrypoint from each stateful Rust provider in the
 * final staticlib. The native resolver tables retain the remaining C ABI
 * entries by name when the embedding link extracts their shim objects. */
#[unsafe(no_mangle)]
pub extern "C" fn aim_bionic_rust_provider_closure_anchor() -> usize {
    let core = (bionic_fdsan_owner::aim_fdsan_exchange as usize)
        | (bionic_fs_facade::aim_bionic_fs_close_core as usize)
        | (bionic_fs_facade::aim_bionic_fs_process_install as usize)
        | (bionic_fs_facade::aim_bionic_fs_process_uninstall as usize)
        | (bionic_fs_facade::aim_bionic_fs_seed_private_directory as usize)
        | (bionic_process_state_facade::aim_bionic_process_getauxval_core as usize)
        | (bionic_process_state_facade::aim_bionic_process_state_process_install as usize)
        | (bionic_process_state_facade::aim_bionic_process_state_install_configured as usize)
        | (bionic_process_state_facade::aim_bionic_process_state_is_installed as usize)
        | (bionic_stdio_facade::aim_bionic_stdio_fclose_core as usize)
        | (bionic_dso_lifecycle_facade::aim_bionic_dso_cxa_finalize_core as usize)
        | (bionic_vm_facade::aim_bionic_vm_mmap_core as usize)
        | (bionic_vm_facade::aim_bionic_vm_process_install as usize)
        | (android_dso_namespace::aim_bionic_dlopen as usize)
        | (android_aaudio_provider::aim_android_aaudio_resolve as usize);
    #[cfg(feature = "legacy-binder-ndk")]
    let core = core | (android_binder_ndk_provider::aim_android_binder_ndk_resolve as usize);
    core
}

#[allow(dead_code)]
fn _abi_types(_: *const c_char, _: *mut c_void) {}
