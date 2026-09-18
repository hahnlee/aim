use super::abi::EngineSymbols;
use core::ffi::c_void;
use darwin_art_engine_sys::{
    SurfaceCloseRequestedFn, SurfaceDestroyFn, SurfaceGetSizeFn, SurfacePresentAsyncFn,
    SurfacePresentFn, SurfacePumpEventsFn, SurfaceResizeFn, SurfaceUpdateFn,
};
use darwin_art_runtime::NativeResource;
use std::{mem::size_of, ptr::NonNull};

/// Owner-thread surface handle. Its callback table and native handle stay
/// paired until `close`, so RuntimeSession can drop it before EngineSession
/// and never call into an unmapped engine image.
pub struct SurfaceSession {
    handle: Option<NonNull<c_void>>,
    resize: Option<SurfaceResizeFn>,
    get_size: Option<SurfaceGetSizeFn>,
    update: SurfaceUpdateFn,
    present: SurfacePresentFn,
    present_async: Option<SurfacePresentAsyncFn>,
    pump_events: SurfacePumpEventsFn,
    close_requested: SurfaceCloseRequestedFn,
    install_android_input_sink: darwin_art_engine_sys::SurfaceInstallAndroidInputSinkFn,
    destroy: SurfaceDestroyFn,
    armed: bool,
    close_status: Option<i32>,
}

impl SurfaceSession {
    fn from_parts(handle: *mut c_void, symbols: EngineSymbols) -> Self {
        Self {
            handle: NonNull::new(handle),
            resize: symbols.surface.resize,
            get_size: symbols.surface.get_size,
            update: symbols.surface.update,
            present: symbols.surface.present,
            present_async: symbols.surface.present_async,
            pump_events: symbols.surface.pump_events,
            close_requested: symbols.surface.close_requested,
            install_android_input_sink: symbols.surface.install_android_input_sink,
            destroy: symbols.surface.destroy,
            armed: true,
            close_status: None,
        }
    }

    pub fn handle(&self) -> *mut c_void {
        self.handle
            .filter(|_| self.armed)
            .map_or(std::ptr::null_mut(), NonNull::as_ptr)
    }

    pub(crate) fn active(symbols: EngineSymbols) -> Option<Self> {
        // SAFETY: callback belongs to the live engine image represented by
        // this symbol table.
        let handle = unsafe { (symbols.surface.active)() };
        if handle.is_null() {
            return None;
        }
        let mut session = Self::from_parts(handle, symbols);
        // A surface published by the runtime still needs an explicit owner
        // ingress before host code can enter its frame loop. Fail closed and
        // destroy the unpublished Rust hand-off if installation is rejected.
        let sink_status = unsafe { (session.install_android_input_sink)(handle) };
        if sink_status != 0 {
            let _ = session.close();
            None
        } else {
            Some(session)
        }
    }

    fn create_with_sink(
        symbols: EngineSymbols,
        info: &darwin_art_engine_sys::SurfaceCreateInfo,
        install_input_sink: bool,
    ) -> Result<Self, i32> {
        let mut status = -1;
        // SAFETY: info is a valid POD for the duration of this call.
        let handle = unsafe { (symbols.surface.create)(info, &mut status) };
        if handle.is_null() {
            Err(status)
        } else {
            let mut session = Self::from_parts(handle, symbols);
            if !install_input_sink {
                return Ok(session);
            }
            // Production display surfaces receive AppKit input only through
            // the Android InputChannel ingress. A sink installation failure
            // leaves no live surface with an implicit mailbox route.
            let sink_status = unsafe { (session.install_android_input_sink)(handle) };
            if sink_status != 0 {
                let _ = session.close();
                Err(sink_status)
            } else {
                Ok(session)
            }
        }
    }

    pub(crate) fn create(
        symbols: EngineSymbols,
        info: &darwin_art_engine_sys::SurfaceCreateInfo,
    ) -> Result<Self, i32> {
        Self::create_with_sink(symbols, info, true)
    }

    /// Create the explicit pre-VM display target without installing the
    /// Android input sink. The sink is attached only after ART has initialized
    /// its input ownership; cleanup remains the ordinary paired surface close.
    pub(crate) fn create_display_target(
        symbols: EngineSymbols,
        info: &darwin_art_engine_sys::SurfaceCreateInfo,
    ) -> Result<Self, i32> {
        Self::create_with_sink(symbols, info, false)
    }

    pub fn update_words(&self, pixels: &[u32]) -> i32 {
        let Some(handle) = self.handle.filter(|_| self.armed) else {
            return darwin_art_engine_sys::ENGINE_STATUS_UNAVAILABLE;
        };
        let byte_count = pixels.len().saturating_mul(size_of::<u32>());
        // SAFETY: the callback and handle are paired and live; `pixels`
        // remains borrowed for the duration of the synchronous callback.
        unsafe { (self.update)(handle.as_ptr(), pixels.as_ptr().cast(), byte_count) }
    }

    pub fn resize(&self, width: u32, height: u32) -> i32 {
        let Some(handle) = self.handle.filter(|_| self.armed) else {
            return darwin_art_engine_sys::ENGINE_STATUS_UNAVAILABLE;
        };
        let Some(resize) = self.resize else {
            return darwin_art_engine_sys::ENGINE_STATUS_UNAVAILABLE;
        };
        // SAFETY: callback and handle are paired for the lifetime of this session.
        unsafe { resize(handle.as_ptr(), width, height) }
    }

    pub fn size(&self) -> Option<(u32, u32)> {
        let handle = self.handle.filter(|_| self.armed)?;
        let get_size = self.get_size?;
        let mut width = 0;
        let mut height = 0;
        // SAFETY: callback writes two valid output scalars owned by this frame.
        if unsafe { get_size(handle.as_ptr(), &mut width, &mut height) } {
            Some((width, height))
        } else {
            None
        }
    }

    pub fn present(&self) -> i32 {
        let Some(handle) = self.handle.filter(|_| self.armed) else {
            return darwin_art_engine_sys::ENGINE_STATUS_UNAVAILABLE;
        };
        // SAFETY: same invariant as update.
        unsafe { (self.present)(handle.as_ptr()) }
    }

    /// Cross-thread scanout token for the Android RenderThread-equivalent.
    /// The callback only enqueues a latest-wins AppKit blit and never enters
    /// JNI or dereferences the owner-thread graphics state. The token is
    /// dropped with FrameClock before this surface is closed.
    pub fn scanout_token(&self) -> Option<ScanoutToken> {
        Some(ScanoutToken {
            handle: self.handle.filter(|_| self.armed)?.as_ptr(),
            present: self.present_async?,
        })
    }

    pub fn pump_events(&self, visible_seconds: f64) -> i32 {
        let Some(handle) = self.handle.filter(|_| self.armed) else {
            return darwin_art_engine_sys::ENGINE_STATUS_UNAVAILABLE;
        };
        // SAFETY: same invariant as update.
        unsafe { (self.pump_events)(handle.as_ptr(), visible_seconds) }
    }

    pub fn close_requested(&self) -> bool {
        let Some(handle) = self.handle.filter(|_| self.armed) else {
            return true;
        };
        // SAFETY: the close-state query is an acquire-only worker-safe ABI
        // and the handle remains paired with this session until close.
        unsafe { (self.close_requested)(handle.as_ptr()) }
    }

    pub fn close(&mut self) -> i32 {
        if !self.armed {
            return self.close_status.unwrap_or(0);
        }
        self.armed = false;
        let Some(handle) = self.handle else {
            return self
                .close_status
                .unwrap_or(darwin_art_engine_sys::ENGINE_STATUS_UNAVAILABLE);
        };
        // SAFETY: this is the one matching destroy call for the handle.
        let status = unsafe { (self.destroy)(handle.as_ptr()) };
        self.close_status = Some(status);
        status
    }
}

#[derive(Clone, Copy)]
pub struct ScanoutToken {
    handle: *mut c_void,
    present: SurfacePresentAsyncFn,
}

// The async ABI only takes the opaque surface pointer and appends a bounded
// request to its presentation mutex. The owner guarantees the token is
// joined/dropped before SurfaceSession::close destroys that pointer.
unsafe impl Send for ScanoutToken {}
unsafe impl Sync for ScanoutToken {}

impl ScanoutToken {
    /// Construct a token for host-side scheduler tests. Production callers
    /// should obtain tokens from `SurfaceSession::scanout_token`, which keeps
    /// the opaque handle lifetime paired with the owning surface.
    #[doc(hidden)]
    pub unsafe fn from_raw_for_test(handle: *mut c_void, present: SurfacePresentAsyncFn) -> Self {
        Self { handle, present }
    }

    pub fn present(self) -> i32 {
        // SAFETY: the token lifetime is bounded by FrameClock's join before
        // the owning SurfaceSession is closed.
        unsafe { (self.present)(self.handle) }
    }
}

impl Drop for SurfaceSession {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

impl NativeResource for SurfaceSession {
    fn close(&mut self) -> i32 {
        SurfaceSession::close(self)
    }
}

#[cfg(test)]
mod surface_session_tests {
    use super::super::abi::{
        BinderBrokerSymbols, EngineSymbols, GraphicsSymbols, ProcessSymbols, ProviderSymbols,
        SurfaceSymbols,
    };
    use super::SurfaceSession;
    use core::ffi::c_void;
    use core::ptr::NonNull;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use darwin_art_engine_sys::*;
    use std::sync::Mutex;

    static DESTROY_CALLS: AtomicUsize = AtomicUsize::new(0);
    static DESTROY_TEST_LOCK: Mutex<()> = Mutex::new(());

    unsafe extern "C" fn destroy(_: *mut c_void) -> i32 {
        DESTROY_CALLS.fetch_add(1, Ordering::SeqCst);
        -17
    }

    unsafe extern "C" fn update(_: *mut c_void, _: *const c_void, _: usize) -> i32 {
        0
    }

    unsafe extern "C" fn present(_: *mut c_void) -> i32 {
        0
    }

    unsafe extern "C" fn pump(_: *mut c_void, _: f64) -> i32 {
        0
    }

    unsafe extern "C" fn close_requested(_: *mut c_void) -> bool {
        false
    }

    unsafe extern "C" fn install_sink(_: *mut c_void) -> i32 {
        0
    }

    static CREATE_CALLS: AtomicUsize = AtomicUsize::new(0);
    static INSTALL_CALLS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn create_surface(
        _: *const SurfaceCreateInfo,
        status: *mut i32,
    ) -> *mut c_void {
        CREATE_CALLS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: the caller supplies a valid output pointer for this test ABI.
        unsafe { *status = 0 };
        Box::into_raw(Box::new(1_u8)).cast()
    }

    unsafe extern "C" fn destroy_surface(handle: *mut c_void) -> i32 {
        DESTROY_CALLS.fetch_add(1, Ordering::SeqCst);
        // SAFETY: every test-created handle is one Box allocation.
        unsafe { drop(Box::from_raw(handle.cast::<u8>())) };
        0
    }

    unsafe extern "C" fn install_test_sink(_: *mut c_void) -> i32 {
        INSTALL_CALLS.fetch_add(1, Ordering::SeqCst);
        0
    }

    unsafe extern "C" fn update_test(_: *mut c_void, _: *const c_void, _: usize) -> i32 {
        0
    }
    unsafe extern "C" fn present_test(_: *mut c_void) -> i32 {
        0
    }
    unsafe extern "C" fn pump_test(_: *mut c_void, _: f64) -> i32 {
        0
    }
    unsafe extern "C" fn close_requested_test(_: *mut c_void) -> bool {
        false
    }
    unsafe extern "C" fn active_surface_test() -> *mut c_void {
        std::ptr::null_mut()
    }
    unsafe extern "C" fn appkit_pump_test(_: f64) -> i32 {
        0
    }
    unsafe extern "C" fn run_process_test(_: *const ProcessConfig, _: *mut ProcessResult) -> i32 {
        0
    }
    unsafe extern "C" fn shutdown_process_test() -> i32 {
        0
    }
    unsafe extern "C" fn prepare_process_exit_test() -> i32 {
        0
    }
    unsafe extern "C" fn install_snapshot_test(_: *const ProcessSnapshotConfig) -> i32 {
        0
    }
    unsafe extern "C" fn uninstall_snapshot_test() -> i32 {
        0
    }
    unsafe extern "C" fn install_filesystem_test(
        _: i32,
        _: *const u8,
        _: usize,
        _: *const u8,
        _: usize,
    ) -> i32 {
        0
    }
    unsafe extern "C" fn uninstall_filesystem_test() -> i32 {
        0
    }
    unsafe extern "C" fn install_fd_boundary_test(
        _: Option<darwin_art_engine_sys::FdInheritanceBoundaryFn>,
    ) -> i32 {
        0
    }

    unsafe extern "C" fn install_provider_test(
        _: *mut c_void,
        _: Option<ProviderAcquireFn>,
        _: Option<ProviderReleaseFn>,
    ) {
    }
    unsafe extern "C" fn install_scm_test(
        _: *const darwin_art_engine_sys::ScmEndpointProviderV1,
    ) -> i32 {
        0
    }
    unsafe extern "C" fn uninstall_scm_test() -> i32 {
        0
    }
    unsafe extern "C" fn socket_broker_inactive_test() -> i32 {
        0
    }
    unsafe extern "C" fn clear_provider_test() {}
    unsafe extern "C" fn acquire_provider_test(_: u32, _: i32) -> i32 {
        0
    }
    unsafe extern "C" fn release_provider_test(_: u32) -> i32 {
        0
    }
    unsafe extern "C" fn binder_install_test(_: *const c_void, _: *mut u64) -> u32 {
        0
    }
    unsafe extern "C" fn binder_publish_test(_: u64, _: u64, _: *mut i32) -> u32 {
        0
    }
    unsafe extern "C" fn binder_uninstall_test(_: u64) -> u32 {
        0
    }
    unsafe extern "C" fn binder_file_test(_: i32) -> i32 {
        0
    }
    unsafe extern "C" fn binder_retained_file_test(
        _: i32,
        _: *const darwin_art_engine_sys::DescriptorTransferBinding,
        _: *mut darwin_art_engine_sys::RetainedExportedDescriptor,
    ) -> i32 {
        -1
    }
    unsafe extern "C" fn binder_release_export_test(_: *mut c_void) {}

    fn test_symbols() -> EngineSymbols {
        EngineSymbols {
            process: ProcessSymbols {
                run_process: run_process_test,
                shutdown_process: shutdown_process_test,
                prepare_process_exit: prepare_process_exit_test,
                install_process_snapshot: install_snapshot_test,
                uninstall_process_snapshot: uninstall_snapshot_test,
                install_process_filesystem: install_filesystem_test,
                uninstall_process_filesystem: uninstall_filesystem_test,
            },
            surface: SurfaceSymbols {
                create: create_surface,
                resize: None,
                get_size: None,
                update: update_test,
                present: present_test,
                present_async: None,
                pump_events: pump_test,
                close_requested: close_requested_test,
                install_android_input_sink: install_test_sink,
                destroy: destroy_surface,
                active: active_surface_test,
                appkit_pump_events: appkit_pump_test,
            },
            graphics: GraphicsSymbols {
                create: None,
                close: None,
                destroy: None,
                dispatch_pointer: None,
                dispatch_pointer_v2: None,
                dispatch_key_v1: None,
                pump_main_looper: None,
                wait_main_looper: None,
                wake_main_looper: None,
                pump_frame: None,
            },
            provider: ProviderSymbols {
                install_fd_inheritance: install_fd_boundary_test,
                install_scm_endpoint: install_scm_test,
                uninstall_scm_endpoint: uninstall_scm_test,
                socket_broker_is_active: socket_broker_inactive_test,
                install_hooks: install_provider_test,
                clear_hooks: clear_provider_test,
                native_acquire: acquire_provider_test,
                native_release: release_provider_test,
            },
            binder_broker: BinderBrokerSymbols {
                install_owner: binder_install_test,
                publish: binder_publish_test,
                uninstall_owner: binder_uninstall_test,
                export_file: binder_file_test,
                export_retained_file: binder_retained_file_test,
                release_export_lease: binder_release_export_test,
                import_file: binder_file_test,
                close_file: binder_file_test,
            },
        }
    }

    #[test]
    fn destroy_failure_is_reported_once_and_not_reentered_by_drop() {
        let _lock = DESTROY_TEST_LOCK.lock().unwrap();
        DESTROY_CALLS.store(0, Ordering::SeqCst);
        let mut session = SurfaceSession {
            handle: NonNull::new(std::ptr::dangling_mut::<c_void>()),
            resize: None,
            get_size: None,
            update,
            present,
            present_async: None,
            pump_events: pump,
            close_requested,
            install_android_input_sink: install_sink,
            destroy,
            armed: true,
            close_status: None,
        };
        assert_eq!(session.close(), -17);
        assert_eq!(session.close(), -17);
        drop(session);
        assert_eq!(DESTROY_CALLS.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn closed_surface_fails_closed_before_foreign_operations() {
        let _lock = DESTROY_TEST_LOCK.lock().unwrap();
        let mut session = SurfaceSession {
            handle: NonNull::new(std::ptr::dangling_mut::<c_void>()),
            resize: None,
            get_size: None,
            update,
            present,
            present_async: None,
            pump_events: pump,
            close_requested,
            install_android_input_sink: install_sink,
            destroy,
            armed: true,
            close_status: None,
        };
        assert_eq!(session.close(), -17);
        assert!(session.handle().is_null());
        assert_eq!(
            session.update_words(&[1, 2, 3]),
            darwin_art_engine_sys::ENGINE_STATUS_UNAVAILABLE
        );
        assert_eq!(
            session.present(),
            darwin_art_engine_sys::ENGINE_STATUS_UNAVAILABLE
        );
        assert_eq!(
            session.pump_events(0.1),
            darwin_art_engine_sys::ENGINE_STATUS_UNAVAILABLE
        );
    }

    #[test]
    fn display_target_skips_sink_but_closes_native_surface() {
        let _lock = DESTROY_TEST_LOCK.lock().unwrap();
        CREATE_CALLS.store(0, Ordering::SeqCst);
        INSTALL_CALLS.store(0, Ordering::SeqCst);
        DESTROY_CALLS.store(0, Ordering::SeqCst);
        let info = SurfaceCreateInfo {
            width: 320,
            height: 240,
            title: core::ptr::null(),
            visible: true,
            scale_to_display: false,
        };
        let symbols = test_symbols();
        let session = SurfaceSession::create_display_target(symbols, &info).unwrap();
        assert_eq!(CREATE_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(INSTALL_CALLS.load(Ordering::SeqCst), 0);
        drop(session);
        assert_eq!(DESTROY_CALLS.load(Ordering::SeqCst), 1);

        let session = SurfaceSession::create(symbols, &info).unwrap();
        assert_eq!(INSTALL_CALLS.load(Ordering::SeqCst), 1);
        drop(session);
        assert_eq!(DESTROY_CALLS.load(Ordering::SeqCst), 2);
    }
}
