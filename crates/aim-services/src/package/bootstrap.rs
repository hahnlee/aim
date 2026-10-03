//! Original package owners available before native PMS scanning (#834).
use super::{
    libraries::{NativePolicyError, Policy},
    owner::permission_gids::{self, PermissionGidError},
    scan::{LibraryCompatibility, PolicyBridgeError},
    system_config::SystemConfig,
};
use aim_binder_host::{
    local::Strong,
    parcel::{EX_ILLEGAL_STATE, Exception, Parcel},
};
use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bridge;

pub struct Bridge {
    pub(crate) owner: Strong,
}

impl Bridge {
    pub(crate) fn new(owner: Strong) -> Result<Self, Exception> {
        // Binder's standard descriptor handshake, before retaining an endpoint.
        let reply = owner
            .transact(u32::from_be_bytes(*b"_NTF"), &Parcel::new(), false)
            .map_err(|status| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("package bootstrap bridge descriptor: status {status}"),
                )
            })?;
        let mut reader = reply.reader();
        if reader.read_string16().ok().flatten().as_deref() != Some(bridge::DESCRIPTOR)
            || reader.remaining() != 0
        {
            return Err(Exception::illegal_argument(
                "wrong package bootstrap bridge interface",
            ));
        }
        Ok(Self { owner })
    }

    pub fn library_compatibility(
        &self,
        config: &SystemConfig,
        prop: &dyn Fn(&str) -> Option<String>,
    ) -> Result<LibraryCompatibility, PolicyBridgeError> {
        let mut data = Parcel::new();
        bridge::IsTestBaseOnBootclasspath {}.write(&mut data);
        let reply = self
            .owner
            .transact(bridge::IS_TEST_BASE_ON_BOOTCLASSPATH, &data, false)
            .map_err(PolicyBridgeError::Transport)?;
        let on_bcp = bridge::read_is_test_base_on_bootclasspath_reply(&mut reply.reader())
            .map_err(PolicyBridgeError::Transport)?
            .map_err(PolicyBridgeError::Owner)?;
        LibraryCompatibility::new(config, prop, on_bcp).map_err(PolicyBridgeError::Policy)
    }

    pub fn library_policy(
        &self,
        package_name: &str,
        target_sdk: i32,
    ) -> Result<Policy, NativePolicyError> {
        let mut data = Parcel::new();
        bridge::AreNativeLibraryDependenciesEnforced {
            package_name: Some(package_name.into()),
            target_sdk,
        }
        .write(&mut data);
        let reply = self
            .owner
            .transact(
                bridge::ARE_NATIVE_LIBRARY_DEPENDENCIES_ENFORCED,
                &data,
                false,
            )
            .map_err(NativePolicyError::Transport)?;
        bridge::read_are_native_library_dependencies_enforced_reply(&mut reply.reader())
            .map_err(NativePolicyError::Transport)?
            .map_err(NativePolicyError::Owner)
            .map(Policy::pinned)
    }

    pub fn permission_gids(
        &self,
        app_id: i32,
        users: &[i32],
    ) -> Result<Vec<u32>, PermissionGidError> {
        permission_gids::query_with(app_id, users, |uid| {
            let mut data = Parcel::new();
            bridge::GetPermissionGidsForUid { uid }.write(&mut data);
            let reply = self
                .owner
                .transact(bridge::GET_PERMISSION_GIDS_FOR_UID, &data, false)
                .map_err(PermissionGidError::Transport)?;
            bridge::read_get_permission_gids_for_uid_reply(&mut reply.reader())
                .map_err(PermissionGidError::Transport)?
                .map_err(PermissionGidError::Owner)
        })
    }
}
