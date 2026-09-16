#[cfg(target_os = "macos")]
mod app_display;
#[cfg(target_os = "macos")]
mod bootstrap;
mod config;
pub mod external_storage;
mod frame;
#[cfg(target_os = "macos")]
mod frame_clock;
mod frame_timing;
#[cfg(target_os = "macos")]
mod gpu_input;
#[cfg(target_os = "macos")]
mod gpu_loop;
#[cfg(target_os = "macos")]
mod gpu_test_config;
#[cfg(target_os = "macos")]
mod host_services;
mod macho_load_commands;
#[cfg(target_os = "macos")]
mod process_credentials;
mod process_filesystem;
#[cfg(target_os = "macos")]
mod process_signal;
#[cfg(target_os = "macos")]
mod process_snapshot;
mod run;
#[cfg(target_os = "macos")]
mod runtime;
pub mod runtime_identity;
mod runtime_native_inventory;
#[cfg(target_os = "macos")]
mod surface;
pub mod system_image;
mod system_service_configuration;
pub mod system_service_start;
#[cfg(target_os = "macos")]
mod teardown;

pub use config::{HostError, HostOutcome, RunOptions};
// The host surface intentionally exports only the value result. Raw config
// structs and callback function pointers belong to `darwin-art-engine-sys` and
// are kept behind the owned `ProcessRequest` path.
pub use darwin_art_engine_sys::ProcessResult;
pub use frame::OwnedFrame;
#[cfg(target_os = "macos")]
pub use host_services::run_service_child;
pub use run::run;

#[cfg(test)]
mod tests;
