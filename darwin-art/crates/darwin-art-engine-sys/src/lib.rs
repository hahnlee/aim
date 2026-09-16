#![allow(non_camel_case_types)]
#![deny(unsafe_op_in_unsafe_fn)]

//! The sole raw FFI boundary for the native ART/HWUI engine.
//!
//! This crate declares opaque pointers and POD function tables only. Safe
//! ownership wrappers belong in the higher-level runtime crate; no Rust
//! slice, STL object, or borrowed string crosses this boundary.

use core::ffi::{c_char, c_void};
use darwin_art_abi::{AbiHeader, StatusCode};

mod process_credentials;
mod process_snapshot;
pub use process_credentials::{
    PROCESS_SNAPSHOT_CREDENTIALS_ABI_VERSION, ProcessCredentialsConfig, ProcessSnapshotConfigV2,
};
pub use process_snapshot::{
    PROCESS_SNAPSHOT_ABI_VERSION, ProcessSnapshotConfig, ProcessSnapshotEntry,
    ProcessSnapshotInstallConfiguredFn, ProcessSnapshotUninstallFn,
};
mod process_filesystem;
pub use process_filesystem::{
    PROCESS_FILESYSTEM_ALREADY_INSTALLED, PROCESS_FILESYSTEM_BUSY,
    PROCESS_FILESYSTEM_CREATE_FAILED, PROCESS_FILESYSTEM_INVALID_ARGUMENT,
    PROCESS_FILESYSTEM_NOT_INSTALLED, PROCESS_FILESYSTEM_OK, ProcessFilesystemInstallFn,
    ProcessFilesystemUninstallFn,
};

pub type FrameCallback = unsafe extern "C" fn(
    context: *mut c_void,
    argb_pixels: *const u32,
    width: u32,
    height: u32,
    stride_bytes: usize,
) -> i32;

pub type ProviderAcquireFn =
    unsafe extern "C" fn(context: *mut c_void, provider_kind: u32, authority_fd: i32) -> i32;
pub type ProviderReleaseFn = unsafe extern "C" fn(context: *mut c_void, provider_kind: u32) -> i32;
pub type LifecycleBeginFn = unsafe extern "C" fn(context: *mut c_void) -> i32;
pub type LifecycleFinishFn =
    unsafe extern "C" fn(context: *mut c_void, runtime_created: i32) -> i32;
pub type LifecycleFailedFn = unsafe extern "C" fn(context: *mut c_void, status: i32);

#[repr(C)]
pub struct ServiceSpawnRequest {
    pub header: AbiHeader,
    pub component: *const c_char,
    pub instance_name: *const c_char,
    pub process_name: *const c_char,
    pub isolated: i32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct ServiceSpawnResult {
    pub header: AbiHeader,
    pub host_pid: i32,
    pub control_fd: i32,
}

pub type SpawnServiceFn = unsafe extern "C" fn(
    context: *mut c_void,
    request: *const ServiceSpawnRequest,
    result: *mut ServiceSpawnResult,
) -> i32;
pub type ReleaseServiceFn = unsafe extern "C" fn(context: *mut c_void, host_pid: i32) -> i32;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct HostServices {
    pub header: AbiHeader,
    pub context: *mut c_void,
    pub spawn_service: Option<SpawnServiceFn>,
    pub release_service: Option<ReleaseServiceFn>,
}

pub const NATIVE_LOADER_CONFIG_ABI_VERSION: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NativeLoaderConfig {
    pub header: AbiHeader,
    pub linker_config_path: *const c_char,
    pub executable_path: *const c_char,
    pub library_search_path: *const c_char,
    pub android_unwind_path: *const c_char,
}

impl NativeLoaderConfig {
    pub const fn new(
        linker_config_path: *const c_char,
        executable_path: *const c_char,
        library_search_path: *const c_char,
        android_unwind_path: *const c_char,
    ) -> Self {
        Self {
            header: AbiHeader {
                struct_size: core::mem::size_of::<Self>() as u32,
                abi_version: NATIVE_LOADER_CONFIG_ABI_VERSION,
            },
            linker_config_path,
            executable_path,
            library_search_path,
            android_unwind_path,
        }
    }

    pub const fn is_compatible(&self) -> bool {
        self.header.abi_version == NATIVE_LOADER_CONFIG_ABI_VERSION
            && self.header.struct_size as usize >= core::mem::size_of::<Self>()
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct LifecycleHooks {
    pub struct_size: u32,
    pub abi_version: u32,
    pub context: *mut c_void,
    pub begin_run: Option<LifecycleBeginFn>,
    pub finish_run: Option<LifecycleFinishFn>,
    pub begin_shutdown: Option<LifecycleBeginFn>,
    pub mark_failed: Option<LifecycleFailedFn>,
}

#[repr(C)]
pub struct ProcessConfig {
    pub header: AbiHeader,
    pub core_oj_jar: *const c_char,
    pub core_libart_jar: *const c_char,
    pub framework_jar: *const c_char,
    pub core_icu4j_jar: *const c_char,
    pub app_dex: *const c_char,
    pub heap_initial_bytes: u64,
    pub heap_maximum_bytes: u64,
    pub host_context: *mut c_void,
    pub frame_callback: Option<FrameCallback>,
    pub provider_context: *mut c_void,
    pub provider_acquire: Option<ProviderAcquireFn>,
    pub provider_release: Option<ProviderReleaseFn>,
    pub graphics_session_context: *mut c_void,
    pub lifecycle_hooks: *const LifecycleHooks,
    pub host_services: *const HostServices,
    pub native_loader_config: *const NativeLoaderConfig,
}

impl ProcessConfig {
    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        core_oj_jar: *const c_char,
        core_libart_jar: *const c_char,
        framework_jar: *const c_char,
        core_icu4j_jar: *const c_char,
        app_dex: *const c_char,
        heap_initial_bytes: u64,
        heap_maximum_bytes: u64,
        host_context: *mut c_void,
        frame_callback: Option<FrameCallback>,
        provider_context: *mut c_void,
        provider_acquire: Option<ProviderAcquireFn>,
        provider_release: Option<ProviderReleaseFn>,
    ) -> Self {
        Self {
            header: AbiHeader::new(core::mem::size_of::<Self>()),
            core_oj_jar,
            core_libart_jar,
            framework_jar,
            core_icu4j_jar,
            app_dex,
            heap_initial_bytes,
            heap_maximum_bytes,
            host_context,
            frame_callback,
            provider_context,
            provider_acquire,
            provider_release,
            graphics_session_context: core::ptr::null_mut(),
            lifecycle_hooks: core::ptr::null(),
            host_services: core::ptr::null(),
            native_loader_config: core::ptr::null(),
        }
    }

    pub const fn is_compatible(&self) -> bool {
        self.header.accepts(core::mem::size_of::<Self>())
    }

    pub const fn with_graphics_session(mut self, context: *mut c_void) -> Self {
        self.graphics_session_context = context;
        self
    }

    pub const fn with_lifecycle_hooks(mut self, hooks: *const LifecycleHooks) -> Self {
        self.lifecycle_hooks = hooks;
        self
    }

    pub const fn with_host_services(mut self, services: *const HostServices) -> Self {
        self.host_services = services;
        self
    }

    pub const fn with_native_loader_config(mut self, config: *const NativeLoaderConfig) -> Self {
        self.native_loader_config = config;
        self
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct ProcessResult {
    pub header: AbiHeader,
    pub hello_answer: i32,
    pub native_round_trip: i32,
    pub arraycopy_result: i32,
    pub activity_probe_result: i32,
    pub lifecycle_result: i32,
    pub frame_width: u32,
    pub frame_height: u32,
}

impl ProcessResult {
    pub const fn new() -> Self {
        Self {
            header: AbiHeader::new(core::mem::size_of::<Self>()),
            hello_answer: 0,
            native_round_trip: 0,
            arraycopy_result: 0,
            activity_probe_result: 0,
            lifecycle_result: 0,
            frame_width: 0,
            frame_height: 0,
        }
    }

    pub const fn is_compatible(&self) -> bool {
        self.header.accepts(core::mem::size_of::<Self>())
    }
}

pub type RunProcessFn = unsafe extern "C" fn(*const ProcessConfig, *mut ProcessResult) -> i32;
pub type ShutdownProcessFn = unsafe extern "C" fn() -> i32;
pub type PrepareProcessExitFn = unsafe extern "C" fn() -> i32;

#[derive(Clone, Copy)]
#[repr(C)]
pub struct SurfaceCreateInfo {
    pub width: u32,
    pub height: u32,
    pub title: *const c_char,
    pub visible: bool,
    pub scale_to_display: bool,
}

pub type SurfaceCreateFn = unsafe extern "C" fn(*const SurfaceCreateInfo, *mut i32) -> *mut c_void;
pub type SurfaceResizeFn = unsafe extern "C" fn(*mut c_void, u32, u32) -> i32;
pub type SurfaceGetSizeFn = unsafe extern "C" fn(*mut c_void, *mut u32, *mut u32) -> bool;
pub type SurfaceUpdateFn = unsafe extern "C" fn(*mut c_void, *const c_void, usize) -> i32;
pub type SurfacePresentFn = unsafe extern "C" fn(*mut c_void) -> i32;
pub type SurfacePresentAsyncFn = unsafe extern "C" fn(*mut c_void) -> i32;
pub type SurfacePumpEventsFn = unsafe extern "C" fn(*mut c_void, f64) -> i32;
pub type SurfaceCloseRequestedFn = unsafe extern "C" fn(*mut c_void) -> bool;
pub type AppKitPumpEventsFn = unsafe extern "C" fn(f64) -> i32;

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, offset_of, size_of};

    #[test]
    fn process_config_layout_is_owned_by_raw_ffi_crate() {
        assert_eq!(size_of::<ProcessConfig>(), 136);
        assert_eq!(align_of::<ProcessConfig>(), 8);
        assert_eq!(offset_of!(ProcessConfig, header), 0);
        assert_eq!(
            offset_of!(ProcessConfig, header) + offset_of!(AbiHeader, struct_size),
            0
        );
        assert_eq!(
            offset_of!(ProcessConfig, header) + offset_of!(AbiHeader, abi_version),
            4
        );
        assert_eq!(offset_of!(ProcessConfig, core_oj_jar), 8);
        assert_eq!(offset_of!(ProcessConfig, core_libart_jar), 16);
        assert_eq!(offset_of!(ProcessConfig, framework_jar), 24);
        assert_eq!(offset_of!(ProcessConfig, core_icu4j_jar), 32);
        assert_eq!(offset_of!(ProcessConfig, app_dex), 40);
        assert_eq!(offset_of!(ProcessConfig, heap_initial_bytes), 48);
        assert_eq!(offset_of!(ProcessConfig, heap_maximum_bytes), 56);
        assert_eq!(offset_of!(ProcessConfig, host_context), 64);
        assert_eq!(offset_of!(ProcessConfig, frame_callback), 72);
        assert_eq!(offset_of!(ProcessConfig, provider_context), 80);
        assert_eq!(offset_of!(ProcessConfig, provider_acquire), 88);
        assert_eq!(offset_of!(ProcessConfig, provider_release), 96);
        assert_eq!(offset_of!(ProcessConfig, graphics_session_context), 104);
        assert_eq!(offset_of!(ProcessConfig, lifecycle_hooks), 112);
        assert_eq!(offset_of!(ProcessConfig, host_services), 120);
        assert_eq!(offset_of!(ProcessConfig, native_loader_config), 128);
        let defaults = ProcessConfig::new(
            core::ptr::null(),
            core::ptr::null(),
            core::ptr::null(),
            core::ptr::null(),
            core::ptr::null(),
            0,
            0,
            core::ptr::null_mut(),
            None,
            core::ptr::null_mut(),
            None,
            None,
        );
        assert!(defaults.native_loader_config.is_null());
    }

    #[test]
    fn native_loader_config_layout_is_owned_by_raw_ffi_crate() {
        assert_eq!(size_of::<NativeLoaderConfig>(), 40);
        assert_eq!(align_of::<NativeLoaderConfig>(), 8);
        assert_eq!(offset_of!(NativeLoaderConfig, header), 0);
        assert_eq!(offset_of!(NativeLoaderConfig, linker_config_path), 8);
        assert_eq!(offset_of!(NativeLoaderConfig, executable_path), 16);
        assert_eq!(offset_of!(NativeLoaderConfig, library_search_path), 24);
        assert_eq!(offset_of!(NativeLoaderConfig, android_unwind_path), 32);
        assert!(
            NativeLoaderConfig::new(
                core::ptr::null(),
                core::ptr::null(),
                core::ptr::null(),
                core::ptr::null(),
            )
            .is_compatible()
        );
    }

    #[test]
    fn host_services_layout_matches_native_abi() {
        assert_eq!(size_of::<ServiceSpawnRequest>(), 40);
        assert_eq!(offset_of!(ServiceSpawnRequest, component), 8);
        assert_eq!(offset_of!(ServiceSpawnRequest, instance_name), 16);
        assert_eq!(offset_of!(ServiceSpawnRequest, process_name), 24);
        assert_eq!(offset_of!(ServiceSpawnRequest, isolated), 32);
        assert_eq!(size_of::<ServiceSpawnResult>(), 16);
        assert_eq!(offset_of!(ServiceSpawnResult, host_pid), 8);
        assert_eq!(offset_of!(ServiceSpawnResult, control_fd), 12);
        assert_eq!(size_of::<HostServices>(), 32);
        assert_eq!(offset_of!(HostServices, context), 8);
        assert_eq!(offset_of!(HostServices, spawn_service), 16);
        assert_eq!(offset_of!(HostServices, release_service), 24);
    }

    #[test]
    fn lifecycle_hooks_layout_matches_native_abi() {
        assert_eq!(size_of::<LifecycleHooks>(), 48);
        assert_eq!(align_of::<LifecycleHooks>(), 8);
        assert_eq!(offset_of!(LifecycleHooks, struct_size), 0);
        assert_eq!(offset_of!(LifecycleHooks, abi_version), 4);
        assert_eq!(offset_of!(LifecycleHooks, context), 8);
        assert_eq!(offset_of!(LifecycleHooks, begin_run), 16);
        assert_eq!(offset_of!(LifecycleHooks, finish_run), 24);
        assert_eq!(offset_of!(LifecycleHooks, begin_shutdown), 32);
        assert_eq!(offset_of!(LifecycleHooks, mark_failed), 40);
    }

    #[test]
    fn process_result_layout_is_owned_by_raw_ffi_crate() {
        assert_eq!(size_of::<ProcessResult>(), 36);
        assert_eq!(align_of::<ProcessResult>(), 4);
        assert_eq!(offset_of!(ProcessResult, header), 0);
        assert_eq!(
            offset_of!(ProcessResult, header) + offset_of!(AbiHeader, struct_size),
            0
        );
        assert_eq!(
            offset_of!(ProcessResult, header) + offset_of!(AbiHeader, abi_version),
            4
        );
        assert_eq!(offset_of!(ProcessResult, hello_answer), 8);
        assert_eq!(offset_of!(ProcessResult, native_round_trip), 12);
        assert_eq!(offset_of!(ProcessResult, arraycopy_result), 16);
        assert_eq!(offset_of!(ProcessResult, activity_probe_result), 20);
        assert_eq!(offset_of!(ProcessResult, lifecycle_result), 24);
        assert_eq!(offset_of!(ProcessResult, frame_width), 28);
        assert_eq!(offset_of!(ProcessResult, frame_height), 32);
    }

    #[test]
    fn surface_and_callback_layout_is_owned_by_raw_ffi_crate() {
        assert_eq!(size_of::<SurfaceCreateInfo>(), 24);
        assert_eq!(align_of::<SurfaceCreateInfo>(), 8);
        assert_eq!(offset_of!(SurfaceCreateInfo, width), 0);
        assert_eq!(offset_of!(SurfaceCreateInfo, height), 4);
        assert_eq!(offset_of!(SurfaceCreateInfo, title), 8);
        assert_eq!(offset_of!(SurfaceCreateInfo, visible), 16);
        assert_eq!(offset_of!(SurfaceCreateInfo, scale_to_display), 17);
        assert_eq!(size_of::<Option<FrameCallback>>(), size_of::<*mut c_void>());
        assert_eq!(
            align_of::<Option<FrameCallback>>(),
            align_of::<*mut c_void>()
        );
    }

    #[test]
    fn pointer_event_v2_layout_is_stable() {
        assert_eq!(size_of::<PointerEventV2>(), 72);
        assert_eq!(align_of::<PointerEventV2>(), 8);
        assert_eq!(offset_of!(PointerEventV2, sequence), 16);
        assert_eq!(offset_of!(PointerEventV2, event_time_nanos), 24);
        assert_eq!(offset_of!(PointerEventV2, x), 48);
    }

    #[test]
    fn key_event_v1_layout_is_stable() {
        assert_eq!(size_of::<KeyEventV1>(), 72);
        assert_eq!(align_of::<KeyEventV1>(), 8);
        assert_eq!(offset_of!(KeyEventV1, sequence), 16);
        assert_eq!(offset_of!(KeyEventV1, key_code), 40);
        assert_eq!(offset_of!(KeyEventV1, device_id), 56);
    }
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct PointerEvent {
    pub action: u32,
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct PointerEventV2 {
    pub version: u32,
    pub size: u32,
    pub action: u32,
    pub flags: u32,
    pub sequence: u64,
    pub event_time_nanos: u64,
    pub down_time_nanos: u64,
    pub pointer_id: u32,
    pub pointer_count: u32,
    pub x: f32,
    pub y: f32,
    pub raw_x: f32,
    pub raw_y: f32,
    pub pressure: f32,
    pub size_value: f32,
}

pub const POINTER_EVENT_FLAG_MOUSE: u32 = 1 << 0;

#[derive(Clone, Copy, Debug, Default)]
#[repr(C)]
pub struct KeyEventV1 {
    pub version: u32,
    pub size: u32,
    pub action: u32,
    pub flags: u32,
    pub sequence: u64,
    pub event_time_nanos: u64,
    pub down_time_nanos: u64,
    pub key_code: u32,
    pub scan_code: u32,
    pub meta_state: u32,
    pub repeat_count: u32,
    pub device_id: i32,
    pub source: u32,
    pub unicode_char: u32,
}

pub type SurfaceNextPointerEventFn = unsafe extern "C" fn(*mut c_void, *mut PointerEvent) -> bool;
pub type SurfaceNextPointerEventV2Fn =
    unsafe extern "C" fn(*mut c_void, *mut PointerEventV2) -> bool;
pub type SurfaceNextKeyEventV1Fn = unsafe extern "C" fn(*mut c_void, *mut KeyEventV1) -> bool;
pub type SurfaceDestroyFn = unsafe extern "C" fn(*mut c_void) -> i32;
pub type SurfaceActiveFn = unsafe extern "C" fn() -> *mut c_void;
pub type DispatchPointerFn = unsafe extern "C" fn(u32, f32, f32) -> i32;
pub type PumpFrameworkFrameFn = unsafe extern "C" fn(i64) -> i32;
pub type GraphicsSessionHandle = c_void;
pub type GraphicsSessionCreateFn = unsafe extern "C" fn() -> *mut GraphicsSessionHandle;
pub type GraphicsSessionCloseFn = unsafe extern "C" fn(*mut GraphicsSessionHandle) -> i32;
pub type GraphicsSessionDestroyFn = unsafe extern "C" fn(*mut GraphicsSessionHandle) -> i32;
pub type GraphicsSessionDispatchPointerFn =
    unsafe extern "C" fn(*mut GraphicsSessionHandle, u32, f32, f32) -> i32;
pub type GraphicsSessionDispatchPointerV2Fn =
    unsafe extern "C" fn(*mut GraphicsSessionHandle, *const PointerEventV2) -> i32;
pub type GraphicsSessionDispatchKeyV1Fn =
    unsafe extern "C" fn(*mut GraphicsSessionHandle, *const KeyEventV1) -> i32;
pub type GraphicsSessionPumpFrameFn = unsafe extern "C" fn(*mut GraphicsSessionHandle, i64) -> i32;
pub type GraphicsSessionPumpMainLooperFn = unsafe extern "C" fn(*mut GraphicsSessionHandle) -> i32;
pub type GraphicsSessionWaitMainLooperFn =
    unsafe extern "C" fn(*mut GraphicsSessionHandle, i32) -> i32;
pub type GraphicsSessionWakeMainLooperFn = unsafe extern "C" fn(*mut GraphicsSessionHandle) -> i32;
pub type ProviderInstallHooksFn = unsafe extern "C" fn(
    context: *mut c_void,
    acquire: Option<ProviderAcquireFn>,
    release: Option<ProviderReleaseFn>,
);
pub type ProviderClearHooksFn = unsafe extern "C" fn();
pub type ProviderNativeAcquireFn = unsafe extern "C" fn(kind: u32, authority_fd: i32) -> i32;
pub type ProviderNativeReleaseFn = unsafe extern "C" fn(kind: u32) -> i32;
pub type BinderFdInstallOwnerFn =
    unsafe extern "C" fn(callbacks: *const c_void, owner: *mut u64) -> u32;
pub type BinderFdPublishFn =
    unsafe extern "C" fn(owner: u64, object: u64, guest_fd: *mut i32) -> u32;
pub type BinderFdUninstallOwnerFn = unsafe extern "C" fn(owner: u64) -> u32;
pub type BinderExportFileFn = unsafe extern "C" fn(guest_fd: i32) -> i32;
pub type BinderImportFileFn = unsafe extern "C" fn(host_fd: i32) -> i32;
pub type BinderCloseFileFn = unsafe extern "C" fn(guest_fd: i32) -> i32;

/// Sentinel used by a missing optional callback without exporting a C++ type.
pub const ENGINE_STATUS_UNAVAILABLE: i32 = StatusCode::Unsupported as i32;
