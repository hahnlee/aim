//! Safe owner for the process-scoped Darwin engine image.
//!
//! The public owner types are split by responsibility while the platform
//! module keeps the macOS-only ABI surface private.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(target_os = "macos")]
mod platform {
    pub(crate) mod abi;
    pub(crate) mod engine;
    pub(crate) mod graphics;
    pub(crate) mod native_loader_input;
    pub(crate) mod process;
    pub(crate) mod process_credentials;
    pub(crate) mod process_filesystem;
    pub(crate) mod process_snapshot;
    pub(crate) mod scm_endpoint;
    pub(crate) mod surface;

    pub use abi::BinderBrokerSymbols;
    pub use engine::EngineSession;
    pub use graphics::{GraphicsSession, LooperWakeToken};
    pub use process::{
        CallbackBindings, InputError, NativeLoaderInput, ProcessRequest, ProcessRequestError,
    };
    pub use process_credentials::ProcessCredentialsInputs;
    pub use process_filesystem::ProcessFilesystemError;
    pub use process_snapshot::{ProcessSnapshotError, ProcessSnapshotInputs};
    pub use surface::{ScanoutToken, SurfaceSession};
}

#[cfg(target_os = "macos")]
pub use platform::{
    BinderBrokerSymbols, CallbackBindings, EngineSession, GraphicsSession, InputError,
    LooperWakeToken, NativeLoaderInput, ProcessCredentialsInputs, ProcessFilesystemError,
    ProcessRequest, ProcessRequestError, ProcessSnapshotError, ProcessSnapshotInputs, ScanoutToken,
    SurfaceSession,
};
