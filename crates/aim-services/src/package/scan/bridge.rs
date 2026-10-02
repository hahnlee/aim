//! Scan policy inputs from their original SystemServer owners. No installed-
//! package query or original parser/feed is used. Initial boot scanning still
//! requires the owners to be exposed before PMS publication (#808).
use super::{LibraryCompatibility, ScanPolicy};
use crate::package::{
    info, model::System, pkg::AndroidPackage, system_config::SystemConfig, write::Apks,
};
use aim_binder_host::{
    local::Strong,
    parcel::{Exception, Parcel},
};
use aim_service_aidl::{
    com_android_internal_compat_iplatformcompat as compat, dev_aim_server_ibridge as bridge,
};

#[derive(Debug, PartialEq, Eq)]
pub enum PolicyBridgeError {
    Transport(i32),
    Owner(Exception),
    Policy(String),
}

impl LibraryCompatibility {
    pub fn from_bridge(
        config: &SystemConfig,
        prop: &dyn Fn(&str) -> Option<String>,
        owner: &Strong,
    ) -> Result<Self, PolicyBridgeError> {
        let mut data = Parcel::new();
        bridge::IsTestBaseOnBootclasspath {}.write(&mut data);
        let reply = owner
            .transact(bridge::IS_TEST_BASE_ON_BOOTCLASSPATH, &data, false)
            .map_err(PolicyBridgeError::Transport)?;
        let on_bcp = bridge::read_is_test_base_on_bootclasspath_reply(&mut reply.reader())
            .map_err(PolicyBridgeError::Transport)?
            .map_err(PolicyBridgeError::Owner)?;
        Self::new(config, prop, on_bcp).map_err(PolicyBridgeError::Policy)
    }
}

impl ScanPolicy {
    /// Stage manifest policy, query the original compatibility owner only when
    /// its updater does, then commit library policy atomically. Failure never
    /// substitutes a guessed compatibility decision or changes the package.
    /// `owner` is the original platform_compat binder, registered before PMS.
    pub fn apply_from_platform_compat(
        self,
        pkg: &mut AndroidPackage,
        signing: &crate::package::sign::SigningDetails,
        platform: Option<&crate::package::sign::SigningDetails>,
        updated_system_app: bool,
        apks: &Apks,
        compatibility: &LibraryCompatibility,
        system: &System,
        owner: &Strong,
    ) -> Result<(), PolicyBridgeError> {
        let mut next = pkg.clone();
        self.apply_manifest(&mut next, signing, platform, updated_system_app, apks)
            .map_err(PolicyBridgeError::Policy)?;
        let is_system = self.system || updated_system_app;
        let change = if !compatibility.test_base_on_bootclasspath && !is_system {
            Some(change_enabled(
                owner,
                info::app_info_without_state(&next, system),
                133396946,
            )?)
        } else {
            None
        };
        compatibility
            .apply(&mut next, is_system, updated_system_app, change)
            .map_err(PolicyBridgeError::Policy)?;
        *pkg = next;
        Ok(())
    }
}

fn change_enabled(
    owner: &Strong,
    info: info::ApplicationInfo,
    change_id: i64,
) -> Result<bool, PolicyBridgeError> {
    let mut data = Parcel::new();
    compat::IsChangeEnabled {
        change_id,
        app_info: Some(info),
    }
    .write(&mut data);
    let reply = owner
        .transact(compat::IS_CHANGE_ENABLED, &data, false)
        .map_err(PolicyBridgeError::Transport)?;
    compat::read_is_change_enabled_reply(&mut reply.reader())
        .map_err(PolicyBridgeError::Transport)?
        .map_err(PolicyBridgeError::Owner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_driver::{
        Credentials, Device, Driver, Errno, File, GuestProcess, errno, uapi::*,
    };
    use aim_binder_host::{
        local::{Call, LocalProcess, Reply, Service},
        parcel::{BAD_VALUE, Binder, UNKNOWN_TRANSACTION},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
    };

    struct NoMemory;
    impl GuestProcess for NoMemory {
        fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), Errno> {
            Err(errno::EFAULT)
        }
        fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), Errno> {
            Err(errno::EFAULT)
        }
        fn get_file(&mut self, _: u32) -> Result<File, Errno> {
            Err(errno::EBADF)
        }
        fn install_file(&mut self, _: File) -> Result<u32, Errno> {
            Err(errno::EBADF)
        }
        fn close_fd(&mut self, _: u32) {
            unreachable!()
        }
    }
    struct Owner(AtomicI32);
    impl Service for Owner {
        fn descriptor(&self) -> &str {
            bridge::DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.sender_euid, 1000);
            match call.code {
                bridge::IS_TEST_BASE_ON_BOOTCLASSPATH => {
                    bridge::IsTestBaseOnBootclasspath::read(&mut call.data)?;
                }
                compat::IS_CHANGE_ENABLED => {
                    call.data.enforce_interface(compat::DESCRIPTOR)?;
                    assert_eq!(call.data.read_i64()?, 133396946);
                    assert_eq!(call.data.read_i32()?, 1); // present ApplicationInfo
                    assert_eq!(call.data.read_i32()?, 0); // no parcel squashing
                }
                _ => return Err(UNKNOWN_TRANSACTION),
            }
            let mut reply = Parcel::new();
            match self.0.load(Ordering::Relaxed) {
                -2 => return Err(BAD_VALUE),
                -1 => reply.write_exception(&Exception::security("owner denied")),
                value => {
                    reply.write_no_exception();
                    reply.write_i32(value);
                }
            }
            Ok(reply)
        }
    }

    #[test]
    fn binder_policy_inputs_preserve_false_and_owner_transport_errors() {
        let driver = Driver::new();
        let open = |pid| {
            LocalProcess::open(
                &driver,
                Device::Binder,
                Credentials {
                    pid,
                    euid: 1000,
                    security_context: None,
                },
            )
        };
        let server = open(100);
        let owner = Arc::new(Owner(AtomicI32::new(0)));
        let Binder::Local(ptr) = server.add_service(owner.clone()) else {
            unreachable!()
        };
        let mut object = FlatBinderObject {
            kind: BINDER_TYPE_BINDER,
            flags: 0,
            binder: ptr,
            cookie: ptr,
        }
        .encode();
        driver
            .ioctl(
                server.proc_handle(),
                1,
                BINDER_SET_CONTEXT_MGR_EXT,
                &mut object,
                &mut NoMemory,
            )
            .unwrap();
        server.start();
        let client = open(200);
        let strong = client.strong(0);
        for value in [0, 1, -1, -2] {
            owner.0.store(value, Ordering::Relaxed);
            let config = LibraryCompatibility::from_bridge(&Default::default(), &|_| None, &strong);
            let change = change_enabled(&strong, Default::default(), 133396946);
            match value {
                0 | 1 => {
                    assert_eq!(config.unwrap().test_base_on_bootclasspath, value == 1);
                    assert_eq!(change.unwrap(), value == 1);
                }
                -1 => {
                    assert_eq!(
                        config.unwrap_err(),
                        PolicyBridgeError::Owner(Exception::security("owner denied"))
                    );
                    assert_eq!(
                        change.unwrap_err(),
                        PolicyBridgeError::Owner(Exception::security("owner denied"))
                    );
                }
                _ => {
                    assert_eq!(config.unwrap_err(), PolicyBridgeError::Transport(BAD_VALUE));
                    assert_eq!(change.unwrap_err(), PolicyBridgeError::Transport(BAD_VALUE));
                }
            }
        }
        drop(strong);
        driver.release(client.proc_handle());
        driver.release(server.proc_handle());
    }
}
