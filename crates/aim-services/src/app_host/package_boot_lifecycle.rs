//! `IPackageBootLifecycleLeaf` for the app host: the native package boot's
//! stages that act outside the package model, answered as system_server's
//! `NativePackageBootLifecycleLeaf` answers them (`android-16.0.0_r1`): an
//! installed package's compressed blocks released (`F2fsUtils`, in the
//! guest through the agent), the carrier apps disabled until privileged
//! (`CarrierAppUtils`), private volumes' readiness (`StorageEventListener`),
//! the overlay observer, apexd's staged sessions, hibernation, and idle
//! maintenance (`StorageManager.runMaintenance` after
//! `FSTRIM_MANDATORY_INTERVAL`). ART Service's stages are not implemented
//! (#2063).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel,
};
use aim_service_aidl::dev_aim_server_ipackagebootlifecycleevents as events;
use aim_service_aidl::dev_aim_server_ipackagebootlifecycleleaf as leaf;

use super::settings::Table;
use super::{AppHost, reply};

const SYSTEM_UID: i32 = 1000;
/// The agent's `releaseCompressedBlocks(String[] paths, boolean release)`.
const RELEASE_COMPRESSED_BLOCKS: u32 = 16;
/// `Settings.Secure.RELEASE_COMPRESS_BLOCKS_ON_INSTALL`,
/// `Settings.Global.FSTRIM_MANDATORY_INTERVAL` and its default (3 days).
const RELEASE_COMPRESS_BLOCKS_ON_INSTALL: &str = "release_compress_blocks_on_install";
const FSTRIM_MANDATORY_INTERVAL: &str = "fstrim_mandatory_interval";
const DEFAULT_FSTRIM_INTERVAL: i64 = 3 * 24 * 60 * 60 * 1000;
/// `VolumeInfo.TYPE_PRIVATE`, `STATE_MOUNTED`.
const TYPE_PRIVATE: i32 = 1;
const STATE_MOUNTED: i32 = 2;

/// What the leaf registered: its storage listener and its overlay
/// observer's events.
#[derive(Default)]
struct Registered {
    storage: Option<u64>,
    observers: Option<Binder>,
}

pub struct BootLifecycle {
    host: Arc<AppHost>,
    registered: Mutex<Registered>,
}

/// `getStagedSessionInfo`'s state as the leaf reports it: 0 unknown, -1
/// failed or reverted, 1 activated or successful, 2 in progress.
fn session_state(info: Option<&super::apex_manager::SessionInfo>) -> i32 {
    match info {
        None => 0,
        Some(i) if i.activation_failed || i.reverted || i.revert_failed => -1,
        Some(i) if i.activated || i.success => 1,
        Some(_) => 2,
    }
}

/// `performFstrim`'s decision: a mandatory interval set and passed since
/// the last maintenance.
fn fstrim_due(interval: i64, now: i64, last: i64) -> bool {
    interval > 0 && now - last > interval
}

impl BootLifecycle {
    pub fn new(host: Arc<AppHost>) -> Self {
        BootLifecycle {
            host,
            registered: Mutex::new(Registered::default()),
        }
    }

    /// `releaseCompressedBlocks`: `F2fsUtils.releaseCompressedBlocks` of
    /// each code path, last first, where the guest reads its kernel's and
    /// `/data`'s features.
    fn release_compressed_blocks(
        &self,
        paths: Option<Vec<Option<String>>>,
    ) -> Result<(), Exception> {
        let paths = paths.ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))?;
        let paths: Vec<String> = paths
            .into_iter()
            .map(|p| p.ok_or_else(|| Exception::new(EX_NULL_POINTER, "")))
            .collect::<Result<_, _>>()?;
        let release = self.secure_int(RELEASE_COMPRESS_BLOCKS_ON_INSTALL, 1)? != 0;
        self.host
            .call_agent(RELEASE_COMPRESSED_BLOCKS, "releaseCompressedBlocks", |p| {
                p.write_i32(paths.len() as i32);
                for path in &paths {
                    p.write_string16(Some(path));
                }
                p.write_bool(release);
            })
            .map(drop)
    }

    /// `prepareReady`: `CarrierAppUtils.disableCarrierAppsUntilPrivileged`
    /// for user 0, which returns at once with no carrier app SystemConfig
    /// disables until used; with some, their privileges are telephony's
    /// (#1923).
    fn prepare_ready(&self) -> Result<(), Exception> {
        let config =
            crate::package::system_config::SystemConfig::read(self.host.root()?, &|name| {
                self.host.property(name)
            });
        if config.disabled_until_used_carrier_apps.is_empty() {
            return Ok(());
        }
        Err(self.host.unimplemented_named(
            "package",
            "carrier apps disabled until privileged: telephony (#1923)",
        ))
    }

    /// `registerStorageListeners`: a private volume newly mounted is ready
    /// (`volumeReady` with its file system UUID).
    fn register_storage_listeners(&self, events: Option<Binder>) -> Result<(), Exception> {
        let events = events.ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))?;
        let mut registered = self.registered.lock().unwrap();
        if registered.storage.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "storage listener already registered",
            ));
        }
        let host = Arc::downgrade(&self.host);
        let id = self
            .host
            .register_native_storage_listener(Arc::new(move |kind, state, uuid| {
                if kind != TYPE_PRIVATE || state != STATE_MOUNTED {
                    return;
                }
                let Some(host) = host.upgrade() else {
                    return;
                };
                let mut data = Parcel::new();
                data.write_interface_token(events::DESCRIPTOR);
                events::VolumeReady {
                    volume_uuid: uuid.map(Into::into),
                }
                .write(&mut data);
                let sent = match events {
                    Binder::Handle(handle) => host
                        .process
                        .transact(handle, events::VOLUME_READY, &data, false)
                        .map(drop),
                    Binder::Local(ptr) => match host.process.local_service(ptr) {
                        Some(owner) => owner.transact(events::VOLUME_READY, &data, false).map(drop),
                        None => Err(aim_binder_host::parcel::DEAD_OBJECT),
                    },
                };
                if let Err(e) = sent {
                    eprintln!("app host: package boot: volumeReady: {e}");
                }
            }));
        registered.storage = Some(id);
        Ok(())
    }

    /// `registerPackageObservers`: the overlay observer, told
    /// `ACTION_OVERLAY_CHANGED` for any user; the app host changes no
    /// overlay (#2065), so it is never told.
    fn register_package_observers(&self, events: Option<Binder>) -> Result<(), Exception> {
        let events = events.ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))?;
        let mut registered = self.registered.lock().unwrap();
        if registered.observers.is_some() {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "package observer already registered",
            ));
        }
        registered.observers = Some(events);
        Ok(())
    }

    /// `isHibernationSuppressed`: AppHibernationManagerInternal's global
    /// hibernation with oat artifact deletion; the app host runs no
    /// hibernation service (#2064), so none is hibernating.
    fn hibernation_suppressed(&self, name: Option<String>) -> Result<bool, Exception> {
        name.ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))?;
        Ok(false)
    }

    /// `performFstrim`: idle maintenance run when the mandatory interval
    /// passed since the last.
    fn perform_fstrim(&self) -> Result<(), Exception> {
        let interval = self
            .host
            .platform()?
            .settings
            .get(Table::Global, FSTRIM_MANDATORY_INTERVAL)
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_FSTRIM_INTERVAL);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        if fstrim_due(interval, now, self.host.last_maintenance()?) {
            self.host.run_maintenance()?;
        }
        Ok(())
    }

    /// `apexSessionState`: apexd's staged session's state.
    fn apex_session_state(&self, id: i32) -> Result<i32, Exception> {
        let info = super::apex_manager::staged_session_info(&self.host.process, id)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        Ok(session_state(info.as_ref()))
    }

    /// `close`: the storage listener and the overlay observer dropped.
    fn close(&self) {
        let mut registered = self.registered.lock().unwrap();
        if let Some(id) = registered.storage.take() {
            self.host.unregister_native_storage_listener(id);
        }
        registered.observers = None;
    }

    /// `Settings.Secure.getInt(name, default)` of user 0.
    fn secure_int(&self, name: &str, default: i32) -> Result<i32, Exception> {
        Ok(self
            .host
            .platform()?
            .settings
            .get(Table::Secure, name)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default))
    }

    /// ART Service's stages: not implemented (#2063).
    fn art_service(&self, what: &str) -> Exception {
        self.host
            .unimplemented_named("package", &format!("{what}: ART Service (#2063)"))
    }
}

impl Service for BootLifecycle {
    fn descriptor(&self) -> &str {
        leaf::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // `enforce`: the native boot lifecycle owner only.
        if call.sender_euid as i32 != SYSTEM_UID {
            return reply(Err(Exception::security(
                "Native boot lifecycle owner required",
            )));
        }
        let mut p = Parcel::new();
        let result = match call.code {
            leaf::RELEASE_COMPRESSED_BLOCKS => {
                let a =
                    leaf::ReleaseCompressedBlocks::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.release_compressed_blocks(a.compressed_code_paths)
                    .map(|()| leaf::write_release_compressed_blocks_reply(&mut p))
            }
            leaf::PREPARE_READY => {
                leaf::PrepareReady::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.prepare_ready()
                    .map(|()| leaf::write_prepare_ready_reply(&mut p))
            }
            leaf::REGISTER_STORAGE_LISTENERS => {
                let a =
                    leaf::RegisterStorageListeners::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.register_storage_listeners(a.events)
                    .map(|()| leaf::write_register_storage_listeners_reply(&mut p))
            }
            leaf::DEX_OPTIMIZER_READY => {
                leaf::DexOptimizerReady::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Err(self.art_service("dexOptimizerReady"))
            }
            leaf::REGISTER_PACKAGE_OBSERVERS => {
                let a =
                    leaf::RegisterPackageObservers::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.register_package_observers(a.events)
                    .map(|()| leaf::write_register_package_observers_reply(&mut p))
            }
            leaf::UPGRADE_DEXOPT => {
                let a = leaf::UpgradeDexopt::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                match (a.reason, a.completion) {
                    (Some(_), Some(_)) => Err(self.art_service("upgradeDexopt")),
                    _ => Err(Exception::new(EX_NULL_POINTER, "")),
                }
            }
            leaf::IS_HIBERNATION_SUPPRESSED => {
                let a =
                    leaf::IsHibernationSuppressed::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.hibernation_suppressed(a.package_name)
                    .map(|v| leaf::write_is_hibernation_suppressed_reply(&mut p, v))
            }
            leaf::PERFORM_FSTRIM => {
                leaf::PerformFstrim::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.perform_fstrim()
                    .map(|()| leaf::write_perform_fstrim_reply(&mut p))
            }
            leaf::SCHEDULE_MAINTENANCE => {
                leaf::ScheduleMaintenance::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Err(self.art_service("scheduleMaintenance"))
            }
            leaf::APEX_SESSION_STATE => {
                let a = leaf::ApexSessionState::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.apex_session_state(a.session_id)
                    .map(|v| leaf::write_apex_session_state_reply(&mut p, v))
            }
            leaf::CLOSE => {
                leaf::Close::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.close();
                leaf::write_close_reply(&mut p);
                Ok(())
            }
            _ => return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::super::apex_manager::SessionInfo;
    use super::*;

    /// `apexSessionState`'s mapping of apexd's states.
    #[test]
    fn apex_session_states_map_as_the_leaf_maps_them() {
        assert_eq!(session_state(None), 0);
        let failed = SessionInfo {
            activated: true,
            activation_failed: true,
            ..Default::default()
        };
        assert_eq!(session_state(Some(&failed)), -1);
        for s in [
            SessionInfo {
                reverted: true,
                ..Default::default()
            },
            SessionInfo {
                revert_failed: true,
                ..Default::default()
            },
        ] {
            assert_eq!(session_state(Some(&s)), -1);
        }
        for s in [
            SessionInfo {
                activated: true,
                ..Default::default()
            },
            SessionInfo {
                success: true,
                ..Default::default()
            },
        ] {
            assert_eq!(session_state(Some(&s)), 1);
        }
        assert_eq!(session_state(Some(&SessionInfo::default())), 2);
    }

    /// The leaf served as the app host's own node, called as the native
    /// boot calls it: carrier apps (none in the image), the registrations
    /// once each until closed, hibernation, the first maintenance's record,
    /// ART Service's stages and the null arguments the original refuses.
    #[test]
    fn the_leaf_answers_the_native_boot_as_its_own_node() {
        let Some(root) = aim_paths::original_image_with("system/etc/permissions") else {
            return;
        };
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: SYSTEM_UID as u32,
                security_context: None,
            },
        );
        let host = AppHost::new(process.clone());
        let data = std::env::temp_dir().join(format!("aim-boot-lifecycle-{}", std::process::id()));
        std::fs::create_dir_all(data.join("system")).unwrap();
        host.configure(
            &root,
            &data,
            &data,
            Box::new(|name| {
                Some(
                    match name {
                        "ro.build.version.sdk" => "36",
                        "ro.build.version.codename" => "REL",
                        "persist.sys.locale" => "en-US",
                        "ro.sf.lcd_density" => "320",
                        _ => return None,
                    }
                    .into(),
                )
            }),
            Box::new(|_, _| {}),
            Box::new(|| Err(std::io::Error::other("no zygote"))),
        )
        .unwrap();
        let Binder::Local(ptr) = process.add_service(Arc::new(BootLifecycle::new(host.clone())))
        else {
            unreachable!()
        };
        let node = process.local_service(ptr).unwrap();
        let call = |code: u32, write: &dyn Fn(&mut Parcel)| {
            let mut data = Parcel::new();
            data.write_interface_token(leaf::DESCRIPTOR);
            write(&mut data);
            let reply = node.transact(code, &data, false).unwrap();
            let mut r = reply.reader();
            r.read_exception().unwrap().map(|()| {
                if code == leaf::IS_HIBERNATION_SUPPRESSED {
                    r.read_bool().unwrap()
                } else {
                    false
                }
            })
        };
        let events = process.add_service(Arc::new(NoEvents));
        assert!(call(leaf::PREPARE_READY, &|_| {}).is_ok());
        assert!(
            call(leaf::REGISTER_STORAGE_LISTENERS, &|p| p
                .write_binder(Some(events)))
            .is_ok()
        );
        let again = call(leaf::REGISTER_STORAGE_LISTENERS, &|p| {
            p.write_binder(Some(events))
        });
        assert_eq!(again.unwrap_err().code, EX_ILLEGAL_STATE);
        assert!(
            call(leaf::REGISTER_PACKAGE_OBSERVERS, &|p| p
                .write_binder(Some(events)))
            .is_ok()
        );
        let again = call(leaf::REGISTER_PACKAGE_OBSERVERS, &|p| {
            p.write_binder(Some(events))
        });
        assert_eq!(again.unwrap_err().code, EX_ILLEGAL_STATE);
        assert!(call(leaf::CLOSE, &|_| {}).is_ok());
        assert!(
            call(leaf::REGISTER_STORAGE_LISTENERS, &|p| p
                .write_binder(Some(events)))
            .is_ok()
        );
        assert!(
            call(leaf::REGISTER_PACKAGE_OBSERVERS, &|p| p
                .write_binder(Some(events)))
            .is_ok()
        );
        let null = call(leaf::REGISTER_STORAGE_LISTENERS, &|p| p.write_binder(None));
        assert_eq!(null.unwrap_err().code, EX_NULL_POINTER);
        assert_eq!(
            call(leaf::IS_HIBERNATION_SUPPRESSED, &|p| p
                .write_string16(Some("a.b"))),
            Ok(false)
        );
        let null = call(leaf::IS_HIBERNATION_SUPPRESSED, &|p| p.write_string16(None));
        assert_eq!(null.unwrap_err().code, EX_NULL_POINTER);
        // No record yet: one is made, and the maintenance is due.
        assert!(call(leaf::PERFORM_FSTRIM, &|_| {}).is_ok());
        assert!(data.join("system/last-fstrim").exists());
        for code in [leaf::DEX_OPTIMIZER_READY, leaf::SCHEDULE_MAINTENANCE] {
            let art = call(code, &|_| {}).unwrap_err();
            assert!(art.message.contains("#2063"), "{}", art.message);
        }
        let null = call(leaf::RELEASE_COMPRESSED_BLOCKS, &|p| p.write_i32(-1));
        assert_eq!(null.unwrap_err().code, EX_NULL_POINTER);
        assert!(call(leaf::CLOSE, &|_| {}).is_ok());
        std::fs::remove_dir_all(&data).unwrap();
    }

    /// Events the test never expects to receive.
    struct NoEvents;

    impl Service for NoEvents {
        fn descriptor(&self) -> &str {
            events::DESCRIPTOR
        }

        fn transact(&self, _: &mut Call<'_>) -> Reply {
            panic!("no event expected")
        }
    }

    /// `performFstrim`: only a positive interval, strictly passed.
    #[test]
    fn fstrim_runs_once_the_mandatory_interval_passed() {
        assert!(fstrim_due(10, 21, 10));
        assert!(!fstrim_due(10, 20, 10));
        assert!(!fstrim_due(0, i64::MAX, 0));
        assert!(!fstrim_due(-1, i64::MAX, 0));
        assert!(fstrim_due(
            DEFAULT_FSTRIM_INTERVAL,
            DEFAULT_FSTRIM_INTERVAL + 1,
            0
        ));
    }
}
