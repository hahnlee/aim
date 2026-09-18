#![deny(unsafe_op_in_unsafe_fn)]

//! Rust-owned runtime lifecycle state.
//!
//! The native engine remains behind `darwin-art-engine-sys`; this crate owns
//! the ordering and rollback state so C++ callbacks cannot invent a second,
//! conflicting lifecycle machine.

use darwin_art_abi::StatusCode;

#[cfg(target_os = "macos")]
mod bound_service_launch_capability;
#[cfg(target_os = "macos")]
mod bound_service_process_ffi;
mod lifecycle;
mod linker_load_flags;
pub mod linker_namespace;
#[cfg(unix)]
mod linker_namespace_ffi;
mod native_owner;
#[cfg(target_os = "macos")]
mod network_path;
mod owners;
mod package_records;
mod process_registry;
mod provider;
mod provider_bridge;
mod provider_kind;
#[cfg(target_os = "macos")]
mod scheduling;
#[cfg(target_os = "macos")]
mod scheduling_groups;
#[cfg(target_os = "macos")]
mod service_endpoint;
#[cfg(target_os = "macos")]
mod service_endpoint_ffi;
#[cfg(target_os = "macos")]
mod service_readiness;
mod session;
mod shutdown;
#[cfg(test)]
mod shutdown_tests;

pub use lifecycle::RuntimeLifecycle;
pub use native_owner::{
    RuntimeNativeOwner, RuntimeNativeOwnerDropFn, darwin_art_runtime_native_owner_attach,
    darwin_art_runtime_native_owner_create, darwin_art_runtime_native_owner_destroy,
    darwin_art_runtime_native_owner_lookup,
};
pub use provider::{ProviderLeaseError, ProviderLeaseTable};
pub use provider_bridge::{ProviderBridge, ProviderLease};
pub use provider_kind::ProviderKind;
pub use session::{NativeResource, RuntimeSession};
pub use shutdown::ShutdownGuard;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimePhase {
    New,
    Bootstrapping,
    Running,
    ShuttingDown,
    Stopped,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeError {
    WrongOwnerThread,
    InvalidTransition {
        from: RuntimePhase,
        to: RuntimePhase,
    },
    AlreadyFailed,
    EngineFailure {
        status: i32,
    },
    SubsystemNotActive {
        subsystem: Subsystem,
    },
    InvalidShutdownOrder {
        expected: Subsystem,
        requested: Subsystem,
    },
}

impl RuntimeError {
    pub const fn status(self) -> StatusCode {
        match self {
            Self::WrongOwnerThread => StatusCode::InvalidState,
            Self::InvalidTransition { .. } => StatusCode::InvalidState,
            Self::AlreadyFailed => StatusCode::Internal,
            Self::EngineFailure { .. } => StatusCode::Internal,
            Self::SubsystemNotActive { .. } => StatusCode::InvalidState,
            Self::InvalidShutdownOrder { .. } => StatusCode::InvalidState,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Subsystem {
    Engine,
    ElfNamespace,
    Filesystem,
    Network,
    Surface,
    Graphics,
    Input,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SubsystemLease {
    subsystem: Subsystem,
    generation: u64,
    session_id: u64,
}

impl SubsystemLease {
    pub const fn subsystem(self) -> Subsystem {
        self.subsystem
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_is_monotonic_and_supports_bootstrap_rollback() {
        let mut runtime = RuntimeLifecycle::new();
        assert_eq!(runtime.phase(), RuntimePhase::New);
        runtime.start().unwrap();
        runtime.begin_shutdown().unwrap();
        runtime.finish_shutdown().unwrap();
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);

        let mut failed = RuntimeLifecycle::new();
        failed.start().unwrap();
        failed.fail(RuntimeError::AlreadyFailed);
        assert_eq!(failed.phase(), RuntimePhase::Failed);
        assert_eq!(failed.failure(), Some(RuntimeError::AlreadyFailed));
        assert_eq!(failed.mark_running(), Err(RuntimeError::AlreadyFailed));
    }

    #[test]
    fn invalid_transitions_are_rejected() {
        let mut runtime = RuntimeLifecycle::new();
        assert!(matches!(
            runtime.finish_shutdown(),
            Err(RuntimeError::InvalidTransition { .. })
        ));
        runtime.start().unwrap();
        runtime.mark_running().unwrap();
        assert!(matches!(
            runtime.start(),
            Err(RuntimeError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn subsystem_leases_enforce_reverse_teardown_and_generation() {
        let mut runtime = RuntimeLifecycle::new();
        runtime.start().unwrap();
        let engine = runtime.install_subsystem(Subsystem::Engine).unwrap();
        let fs = runtime.install_subsystem(Subsystem::Filesystem).unwrap();

        assert_eq!(
            runtime.uninstall_subsystem(engine),
            Err(RuntimeError::InvalidShutdownOrder {
                expected: Subsystem::Filesystem,
                requested: Subsystem::Engine,
            })
        );
        runtime.begin_shutdown().unwrap();
        assert!(runtime.finish_shutdown().is_err());
        runtime.uninstall_subsystem(fs).unwrap();
        runtime.uninstall_subsystem(engine).unwrap();
        runtime.finish_shutdown().unwrap();
        assert_eq!(runtime.phase(), RuntimePhase::Stopped);
    }
}
