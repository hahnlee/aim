//! Trusted profile selection for the narrow native Unix endpoint provider.
use crate::config::HostError;
use darwin_art_engine::EngineSession;
use darwin_art_profile::NativeScmEndpointProvider;
use std::path::Path;

pub(crate) fn install(engine: &EngineSession, socket: &Path) -> Result<(), HostError> {
    let owner = NativeScmEndpointProvider::new(socket.to_owned())
        .map_err(|error| HostError::HostService(error.to_string()))?;
    // SAFETY: owner holds the Arc context throughout synchronous installation.
    // Native copies the table and retains it before this local owner drops;
    // Rust callbacks are process-lifetime code and contain their FFI unwinds.
    unsafe { engine.install_scm_endpoint_provider(&owner.hooks()) }.map_err(HostError::HostService)
}
