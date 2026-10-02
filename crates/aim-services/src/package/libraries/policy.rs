//! Queries the original PlatformCompat owner through the system-server
//! bridge. The install-time API takes a package name and target SDK and
//! does not ask PMS for an already-installed ApplicationInfo.

use aim_binder_host::local::Strong;
use aim_binder_host::parcel::{Exception, Parcel};
use aim_service_aidl::dev_aim_server_ibridge as bridge;

#[derive(Debug)]
pub enum NativePolicyError {
    Transport(i32),
    Owner(Exception),
}

pub fn native_dependencies_enforced(
    owner: &Strong,
    package_name: &str,
    target_sdk: i32,
) -> Result<bool, NativePolicyError> {
    let mut data = Parcel::new();
    bridge::AreNativeLibraryDependenciesEnforced {
        package_name: Some(package_name.into()),
        target_sdk,
    }
    .write(&mut data);
    let reply = owner
        .transact(
            bridge::ARE_NATIVE_LIBRARY_DEPENDENCIES_ENFORCED,
            &data,
            false,
        )
        .map_err(NativePolicyError::Transport)?;
    bridge::read_are_native_library_dependencies_enforced_reply(&mut reply.reader())
        .map_err(NativePolicyError::Transport)?
        .map_err(NativePolicyError::Owner)
}
