//! Owned process request for the versioned ART entrypoint.
//!
//! The host may prepare paths and callback contexts, but it must not build a
//! raw `ProcessConfig` or keep the backing C strings in a separate state
//! object. This request owns every byte referenced by the synchronous native
//! call and materializes the wire struct only inside `EngineSession`.

use core::ffi::c_void;
use darwin_art_engine_sys::{
    FrameCallback, HostServices, LifecycleHooks, ProcessConfig, ProviderAcquireFn,
    ProviderReleaseFn,
};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use super::graphics::GraphicsSession;
use darwin_art_runtime::ProviderBridge;

pub use super::native_loader_input::{InputError, NativeLoaderInput};

#[derive(Debug)]
pub enum ProcessRequestError {
    InteriorNul(PathBuf),
    MissingCallbackContext(&'static str),
}

/// Validated callback/context bundle for one synchronous ART invocation.
///
/// The only raw-pointer construction lives in `from_raw`; the request and
/// engine layers carry this value by ownership and cannot accidentally omit a
/// required context or pass an unpaired provider callback.
#[derive(Clone, Copy)]
pub struct CallbackBindings<'a> {
    host_context: NonNull<c_void>,
    frame_callback: Option<FrameCallback>,
    provider: &'a ProviderBridge,
    provider_acquire: Option<ProviderAcquireFn>,
    provider_release: Option<ProviderReleaseFn>,
    graphics_session: Option<&'a GraphicsSession>,
    lifecycle_hooks: Option<&'a LifecycleHooks>,
    host_services: Option<&'a HostServices>,
}

impl<'a> CallbackBindings<'a> {
    /// # Safety
    ///
    /// The host context must remain valid and synchronized, and each callback
    /// must remain callable, until the synchronous engine call returns. The
    /// provider and optional graphics session are borrowed by the returned
    /// request, so Rust also prevents either owner from being dropped before
    /// the native call finishes. The native ART graph may invoke provider
    /// callbacks on attached ART threads during that call.
    pub unsafe fn from_raw(
        host_context: *mut c_void,
        frame_callback: Option<FrameCallback>,
        provider: &'a ProviderBridge,
        provider_acquire: Option<ProviderAcquireFn>,
        provider_release: Option<ProviderReleaseFn>,
    ) -> Result<Self, ProcessRequestError> {
        let host_context = NonNull::new(host_context)
            .ok_or(ProcessRequestError::MissingCallbackContext("host"))?;
        if provider_acquire.is_some() != provider_release.is_some() {
            return Err(ProcessRequestError::MissingCallbackContext(
                "provider callback pair",
            ));
        }
        Ok(Self {
            host_context,
            frame_callback,
            provider,
            provider_acquire,
            provider_release,
            graphics_session: None,
            lifecycle_hooks: None,
            host_services: None,
        })
    }

    /// Attach the live graphics owner without exposing its opaque ABI pointer
    /// to host orchestration. The request borrows the session for the same
    /// synchronous call in which the wire config is materialized.
    pub fn with_graphics_session(mut self, session: Option<&'a GraphicsSession>) -> Self {
        self.graphics_session = session;
        self
    }

    /// Attach the Rust-owned lifecycle bridge for the synchronous ART call.
    /// Direct/native callers may omit it and use the compatibility state
    /// machine retained inside the probe.
    pub fn with_lifecycle_hooks(mut self, hooks: Option<&'a LifecycleHooks>) -> Self {
        self.lifecycle_hooks = hooks;
        self
    }

    pub fn with_host_services(mut self, services: Option<&'a HostServices>) -> Self {
        self.host_services = services;
        self
    }
}

/// Owns the complete input lifetime for one synchronous ART process call.
pub struct ProcessRequest<'a> {
    core_oj_jar: CString,
    core_libart_jar: CString,
    framework_jar: CString,
    core_icu4j_jar: CString,
    app_dex: CString,
    heap_initial_bytes: u64,
    heap_maximum_bytes: u64,
    callbacks: CallbackBindings<'a>,
    native_loader_config: Option<NativeLoaderInput>,
}

impl<'a> ProcessRequest<'a> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        core_oj_jar: &Path,
        core_libart_jar: &Path,
        framework_jar: &Path,
        core_icu4j_jar: &Path,
        app_dex: &Path,
        heap_initial_bytes: u64,
        heap_maximum_bytes: u64,
        callbacks: CallbackBindings<'a>,
    ) -> Result<Self, ProcessRequestError> {
        Ok(Self {
            core_oj_jar: path_c_string(core_oj_jar)?,
            core_libart_jar: path_c_string(core_libart_jar)?,
            framework_jar: path_c_string(framework_jar)?,
            core_icu4j_jar: path_c_string(core_icu4j_jar)?,
            app_dex: path_c_string(app_dex)?,
            heap_initial_bytes,
            heap_maximum_bytes,
            callbacks,
            native_loader_config: None,
        })
    }

    /// Attach the Rust lifecycle bridge to the wire config without exposing
    /// raw ABI construction to the host frontend.
    pub fn with_lifecycle_hooks(mut self, hooks: Option<&'a LifecycleHooks>) -> Self {
        self.callbacks.lifecycle_hooks = hooks;
        self
    }

    pub fn with_host_services(mut self, services: Option<&'a HostServices>) -> Self {
        self.callbacks.host_services = services;
        self
    }

    pub fn with_native_loader_config(mut self, config: Option<NativeLoaderInput>) -> Self {
        self.native_loader_config = config;
        self
    }

    pub(crate) fn as_config(&self) -> ProcessConfig {
        ProcessConfig::new(
            self.core_oj_jar.as_ptr(),
            self.core_libart_jar.as_ptr(),
            self.framework_jar.as_ptr(),
            self.core_icu4j_jar.as_ptr(),
            self.app_dex.as_ptr(),
            self.heap_initial_bytes,
            self.heap_maximum_bytes,
            self.callbacks.host_context.as_ptr(),
            self.callbacks.frame_callback,
            self.callbacks.provider.context(),
            self.callbacks.provider_acquire,
            self.callbacks.provider_release,
        )
        .with_graphics_session(
            self.callbacks
                .graphics_session
                .map_or(core::ptr::null_mut(), |graphics| {
                    graphics.raw_handle().cast()
                }),
        )
        .with_lifecycle_hooks(
            self.callbacks
                .lifecycle_hooks
                .map_or(core::ptr::null(), |hooks| hooks as *const LifecycleHooks),
        )
        .with_host_services(
            self.callbacks
                .host_services
                .map_or(core::ptr::null(), |services| {
                    services as *const HostServices
                }),
        )
        .with_native_loader_config(
            self.native_loader_config
                .as_ref()
                .map_or(core::ptr::null(), NativeLoaderInput::as_ptr),
        )
    }
}

fn path_c_string(path: &Path) -> Result<CString, ProcessRequestError> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| ProcessRequestError::InteriorNul(path.to_owned()))
}
