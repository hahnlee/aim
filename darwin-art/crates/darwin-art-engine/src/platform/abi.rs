use std::ffi::{CStr, CString, c_char, c_void};
use std::mem::{size_of, transmute_copy};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use darwin_art_engine_sys::{
    AppKitPumpEventsFn, GraphicsSessionCloseFn, GraphicsSessionCreateFn, GraphicsSessionDestroyFn,
    GraphicsSessionDispatchKeyV1Fn, GraphicsSessionDispatchPointerFn,
    GraphicsSessionDispatchPointerV2Fn, GraphicsSessionPumpFrameFn,
    GraphicsSessionPumpMainLooperFn, GraphicsSessionWaitMainLooperFn,
    GraphicsSessionWakeMainLooperFn, PrepareProcessExitFn, ProcessConfig,
    ProcessFilesystemInstallFn, ProcessFilesystemUninstallFn, ProcessSnapshotInstallConfiguredFn,
    ProcessSnapshotUninstallFn, ProviderClearHooksFn, ProviderInstallHooksFn,
    ProviderNativeAcquireFn, ProviderNativeReleaseFn, RunProcessFn, ShutdownProcessFn,
    SurfaceActiveFn, SurfaceCloseRequestedFn, SurfaceCreateFn, SurfaceDestroyFn, SurfaceGetSizeFn,
    SurfaceNextKeyEventV1Fn, SurfaceNextPointerEventFn, SurfaceNextPointerEventV2Fn,
    SurfacePresentAsyncFn, SurfacePresentFn, SurfacePumpEventsFn, SurfaceResizeFn, SurfaceUpdateFn,
};
use darwin_art_engine_sys::{
    BinderCloseFileFn, BinderExportFileFn, BinderFdInstallOwnerFn, BinderFdPublishFn,
    BinderFdUninstallOwnerFn, BinderImportFileFn,
};

/// Current native RuntimeEntry images have no negotiated NativeLoader sidecar
/// contract.  Reject a non-null tail before any pointer is dereferenced or any
/// native process callback runs.  Future support must add a real versioned ABI
/// capability rather than flipping a local boolean here.
pub(crate) fn native_loader_config_allowed(config: &ProcessConfig) -> bool {
    config.native_loader_config.is_null()
}

#[derive(Clone, Copy)]
pub(crate) struct ProcessSymbols {
    pub run_process: RunProcessFn,
    pub shutdown_process: ShutdownProcessFn,
    pub prepare_process_exit: PrepareProcessExitFn,
    pub install_process_snapshot: ProcessSnapshotInstallConfiguredFn,
    pub uninstall_process_snapshot: ProcessSnapshotUninstallFn,
    pub install_process_filesystem: ProcessFilesystemInstallFn,
    pub uninstall_process_filesystem: ProcessFilesystemUninstallFn,
}

#[derive(Clone, Copy)]
pub(crate) struct SurfaceSymbols {
    pub create: SurfaceCreateFn,
    pub resize: Option<SurfaceResizeFn>,
    pub get_size: Option<SurfaceGetSizeFn>,
    pub update: SurfaceUpdateFn,
    pub present: SurfacePresentFn,
    pub present_async: Option<SurfacePresentAsyncFn>,
    pub pump_events: SurfacePumpEventsFn,
    pub close_requested: SurfaceCloseRequestedFn,
    pub next_pointer_event: SurfaceNextPointerEventFn,
    pub next_pointer_event_v2: Option<SurfaceNextPointerEventV2Fn>,
    pub next_key_event_v1: Option<SurfaceNextKeyEventV1Fn>,
    pub destroy: SurfaceDestroyFn,
    pub active: SurfaceActiveFn,
    pub appkit_pump_events: AppKitPumpEventsFn,
}

#[derive(Clone, Copy)]
pub(crate) struct GraphicsSymbols {
    pub create: Option<GraphicsSessionCreateFn>,
    pub close: Option<GraphicsSessionCloseFn>,
    pub destroy: Option<GraphicsSessionDestroyFn>,
    pub dispatch_pointer: Option<GraphicsSessionDispatchPointerFn>,
    pub dispatch_pointer_v2: Option<GraphicsSessionDispatchPointerV2Fn>,
    pub dispatch_key_v1: Option<GraphicsSessionDispatchKeyV1Fn>,
    pub pump_main_looper: Option<GraphicsSessionPumpMainLooperFn>,
    pub wait_main_looper: Option<GraphicsSessionWaitMainLooperFn>,
    pub wake_main_looper: Option<GraphicsSessionWakeMainLooperFn>,
    pub pump_frame: Option<GraphicsSessionPumpFrameFn>,
}

#[derive(Clone, Copy)]
pub(crate) struct ProviderSymbols {
    pub install_hooks: ProviderInstallHooksFn,
    pub clear_hooks: ProviderClearHooksFn,
    pub native_acquire: ProviderNativeAcquireFn,
    pub native_release: ProviderNativeReleaseFn,
}

#[derive(Clone, Copy)]
pub struct BinderBrokerSymbols {
    pub install_owner: BinderFdInstallOwnerFn,
    pub publish: BinderFdPublishFn,
    pub uninstall_owner: BinderFdUninstallOwnerFn,
    pub export_file: BinderExportFileFn,
    pub import_file: BinderImportFileFn,
    pub close_file: BinderCloseFileFn,
}

#[derive(Clone, Copy)]
pub(crate) struct EngineSymbols {
    pub process: ProcessSymbols,
    pub surface: SurfaceSymbols,
    pub graphics: GraphicsSymbols,
    pub provider: ProviderSymbols,
    pub binder_broker: BinderBrokerSymbols,
}

pub(crate) struct LoadedEngine {
    _library: DynamicLibrary,
    symbols: EngineSymbols,
}

impl LoadedEngine {
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let library = DynamicLibrary::open(path)?;
        // SAFETY: Every name and type is fixed by the Darwin ART v1 ABI.
        let symbols = unsafe {
            EngineSymbols {
                process: ProcessSymbols {
                    run_process: library.symbol(b"darwin_art_run_process\0")?,
                    shutdown_process: library.symbol(b"darwin_art_shutdown_process\0")?,
                    prepare_process_exit: library.symbol(b"darwin_art_prepare_process_exit\0")?,
                    install_process_snapshot: library
                        .symbol(b"darwin_art_bionic_process_state_install_configured\0")?,
                    uninstall_process_snapshot: library
                        .symbol(b"darwin_art_bionic_process_state_process_uninstall\0")?,
                    install_process_filesystem: library
                        .symbol(b"darwin_art_bionic_fs_process_install\0")?,
                    uninstall_process_filesystem: library
                        .symbol(b"darwin_art_bionic_fs_process_uninstall\0")?,
                },
                surface: SurfaceSymbols {
                    create: library.symbol(b"darwin_art_surface_create\0")?,
                    resize: library.symbol(b"darwin_art_surface_resize\0").ok(),
                    get_size: library.symbol(b"darwin_art_surface_get_size\0").ok(),
                    update: library.symbol(b"darwin_art_surface_update\0")?,
                    present: library.symbol(b"darwin_art_surface_present\0")?,
                    present_async: library.symbol(b"darwin_art_surface_present_async\0").ok(),
                    pump_events: library.symbol(b"darwin_art_surface_pump_events\0")?,
                    close_requested: library.symbol(b"darwin_art_surface_close_requested\0")?,
                    next_pointer_event: library
                        .symbol(b"darwin_art_surface_next_pointer_event\0")?,
                    next_pointer_event_v2: library
                        .symbol(b"darwin_art_surface_next_pointer_event_v2\0")
                        .ok(),
                    next_key_event_v1: library
                        .symbol(b"darwin_art_surface_next_key_event_v1\0")
                        .ok(),
                    destroy: library.symbol(b"darwin_art_surface_destroy\0")?,
                    active: library.symbol(b"darwin_art_surface_active_gpu\0")?,
                    appkit_pump_events: library.symbol(b"darwin_art_appkit_pump_events\0")?,
                },
                graphics: GraphicsSymbols {
                    create: library.symbol(b"darwin_art_graphics_session_create\0").ok(),
                    close: library.symbol(b"darwin_art_graphics_session_close\0").ok(),
                    destroy: library
                        .symbol(b"darwin_art_graphics_session_destroy\0")
                        .ok(),
                    dispatch_pointer: library
                        .symbol(b"darwin_art_graphics_session_dispatch_pointer\0")
                        .ok(),
                    dispatch_pointer_v2: library
                        .symbol(b"darwin_art_graphics_session_dispatch_pointer_v2\0")
                        .ok(),
                    dispatch_key_v1: library
                        .symbol(b"darwin_art_graphics_session_dispatch_key_v1\0")
                        .ok(),
                    pump_main_looper: library
                        .symbol(b"darwin_art_graphics_session_pump_main_looper\0")
                        .ok(),
                    wait_main_looper: library
                        .symbol(b"darwin_art_graphics_session_wait_main_looper\0")
                        .ok(),
                    wake_main_looper: library
                        .symbol(b"darwin_art_graphics_session_wake_main_looper\0")
                        .ok(),
                    pump_frame: library
                        .symbol(b"darwin_art_graphics_session_pump_frame\0")
                        .ok(),
                },
                provider: ProviderSymbols {
                    install_hooks: library.symbol(b"darwin_art_provider_install_hooks\0")?,
                    clear_hooks: library.symbol(b"darwin_art_provider_clear_hooks\0")?,
                    native_acquire: library.symbol(b"darwin_art_provider_native_acquire\0")?,
                    native_release: library.symbol(b"darwin_art_provider_native_release\0")?,
                },
                binder_broker: BinderBrokerSymbols {
                    install_owner: library
                        .symbol(b"darwin_art_bionic_binder_fd_install_owner\0")?,
                    publish: library.symbol(b"darwin_art_bionic_binder_fd_publish\0")?,
                    uninstall_owner: library
                        .symbol(b"darwin_art_bionic_binder_fd_uninstall_owner\0")?,
                    export_file: library.symbol(b"darwin_art_binder_export_file_descriptor\0")?,
                    import_file: library.symbol(b"darwin_art_binder_import_file_descriptor\0")?,
                    close_file: library.symbol(b"darwin_art_binder_close_file_descriptor\0")?,
                },
            }
        };
        Ok(Self {
            _library: library,
            symbols,
        })
    }

    pub(crate) fn symbols(&self) -> EngineSymbols {
        self.symbols
    }
}

#[cfg(test)]
mod native_loader_config_tests {
    use super::native_loader_config_allowed;
    use darwin_art_engine_sys::ProcessConfig;

    fn process_config() -> ProcessConfig {
        ProcessConfig::new(
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
        )
    }

    #[test]
    fn null_optional_sidecar_remains_legacy_compatible() {
        let config = process_config();
        assert!(native_loader_config_allowed(&config));
    }

    #[test]
    fn configured_sidecar_fails_closed_without_dereference() {
        let config = process_config()
            .with_native_loader_config(1usize as *const darwin_art_engine_sys::NativeLoaderConfig);
        assert!(!native_loader_config_allowed(&config));
    }
}

struct DynamicLibrary(*mut c_void);

impl DynamicLibrary {
    fn open(path: &Path) -> Result<Self, String> {
        let path = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| "dynamic-library path contains an interior NUL".to_owned())?;
        // ART is the process runtime provider. Android loads libart into a
        // linker namespace where JNI libraries can resolve their declared
        // libart dependency; publish the Darwin runtime image equivalently.
        // Application-library path visibility is still enforced separately by
        // the Android native-loader namespace and is not widened here.
        // SAFETY: path is NUL terminated and flags are valid Darwin flags.
        let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW | RTLD_GLOBAL) };
        if handle.is_null() {
            Err(loader_error())
        } else {
            Ok(Self(handle))
        }
    }

    unsafe fn symbol<T: Copy>(&self, name: &'static [u8]) -> Result<T, String> {
        debug_assert_eq!(name.last(), Some(&0));
        // SAFETY: clearing and reading the loader error is required by dlsym.
        unsafe { dlerror() };
        let symbol = unsafe { dlsym(self.0, name.as_ptr().cast()) };
        let error = unsafe { dlerror() };
        if !error.is_null() {
            return Err(unsafe { CStr::from_ptr(error).to_string_lossy().into_owned() });
        }
        if size_of::<T>() != size_of::<*mut c_void>() {
            return Err("function pointer has an unexpected size".to_owned());
        }
        // SAFETY: T is the fixed function-pointer type associated with name.
        Ok(unsafe { transmute_copy(&symbol) })
    }
}

impl Drop for DynamicLibrary {
    fn drop(&mut self) {
        if self.0.is_null() {
            return;
        }
        // SAFETY: the handle was returned by dlopen and remains owned by this
        // value until Drop. EngineSession closes ART before this owner is
        // dropped, so no callback can execute from the image after dlclose.
        unsafe {
            let _ = dlclose(self.0);
        }
        self.0 = core::ptr::null_mut();
    }
}

fn loader_error() -> String {
    // SAFETY: dlerror returns a process-owned C string.
    let error = unsafe { dlerror() };
    if error.is_null() {
        "unknown error".to_owned()
    } else {
        unsafe { CStr::from_ptr(error).to_string_lossy().into_owned() }
    }
}

const RTLD_GLOBAL: i32 = 0x8;
const RTLD_NOW: i32 = 0x2;

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlerror() -> *const c_char;
    fn dlclose(handle: *mut c_void) -> i32;
}
