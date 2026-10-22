//! `IPackageBootContextLeaf` and `IPackageLifecycleLeaf` for the app host:
//! the user and resource inputs of the native package boot's first
//! capture, its filtering and compatibility settings at system ready, and
//! the users' storage state, answered as system_server's
//! `NativePackageBootContextLeaf` and `PackageLifecycleLeaf` answer them
//! (`android-16.0.0_r1`): UserManagerService's users, the resolver's theme,
//! titles and `config_customResolverActivity`, AppsFilter's
//! `package_query_filtering_enabled` (DeviceConfig, watched from system
//! ready), `Settings.Global.DEVICE_PROVISIONED` and `COMPATIBILITY_MODE`, and
//! `StorageManager.isCeStorageUnlocked`.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::dev_aim_server_ipackagebootcontextleaf as leaf;
use aim_service_aidl::dev_aim_server_ipackagefilteringupdates as updates;
use aim_service_aidl::dev_aim_server_ipackagelifecycleleaf as lifecycle;

use super::content_service::LocalObserver;
use super::settings::Table;
use super::{AppHost, reply};
use crate::package::uri::Uri;

const SYSTEM_UID: i32 = 1000;
/// `PackageManager.APP_ENUMERATION_ENABLED_BY_DEFAULT`.
const APP_ENUMERATION_ENABLED_BY_DEFAULT: bool = true;
/// `DeviceConfig.NAMESPACE_PACKAGE_MANAGER_SERVICE` and its key.
const NAMESPACE: &str = "package_manager_service";
const QUERY_FILTERING: &str = "package_query_filtering_enabled";
/// `ResolverActivity.ActionTitle`'s actions and their labels.
const ACTION_TITLES: [(Option<&str>, &str); 8] = [
    (
        Some("android.intent.action.VIEW"),
        "whichViewApplicationLabel",
    ),
    (
        Some("android.intent.action.EDIT"),
        "whichEditApplicationLabel",
    ),
    (
        Some("android.intent.action.SEND"),
        "whichSendApplicationLabel",
    ),
    (
        Some("android.intent.action.SENDTO"),
        "whichSendToApplicationLabel",
    ),
    (
        Some("android.intent.action.SEND_MULTIPLE"),
        "whichSendApplicationLabel",
    ),
    (
        Some("android.media.action.IMAGE_CAPTURE"),
        "whichImageCaptureApplicationLabel",
    ),
    (None, "whichApplicationLabel"),
    (
        Some("android.intent.action.MAIN"),
        "whichHomeApplicationLabel",
    ),
];

/// What system ready set: the filtering, its watcher's owner, and
/// whether readiness began.
#[derive(Default)]
struct State {
    ready: bool,
    query_disabled: bool,
    updates: Option<Binder>,
    observer: Option<u64>,
}

pub struct BootContext {
    host: Arc<AppHost>,
    state: Mutex<State>,
    me: Weak<BootContext>,
}

impl BootContext {
    pub fn new(host: Arc<AppHost>) -> Arc<Self> {
        Arc::new_cyclic(|me| BootContext {
            host,
            state: Mutex::new(State {
                query_disabled: !APP_ENUMERATION_ENABLED_BY_DEFAULT,
                ..Default::default()
            }),
            me: me.clone(),
        })
    }

    /// `captureBootContext`.
    fn capture(&self) -> Result<Vec<u8>, Exception> {
        let property = |name: &str| self.host.property(name).unwrap_or_default();
        let density = property("ro.sf.lcd_density").parse().unwrap_or(160);
        let platform = super::packages::device_platform(
            self.host.root()?,
            density,
            &property("persist.sys.locale"),
        )
        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        let users = super::user_service::boot_context_users(&self.host)?
            .into_iter()
            .map(|user| Ok((self.host.user_unlocking_or_unlocked(user.id)?, user)))
            .collect::<Result<Vec<_>, Exception>>()?;
        let disabled = self.state.lock().unwrap().query_disabled;
        record(&users, &platform, disabled)
    }

    /// `Settings.Global.getInt(name, default)`.
    fn global_int(&self, name: &str, default: i32) -> Result<i32, Exception> {
        Ok(self
            .host
            .platform()?
            .settings
            .get(Table::Global, name)
            .and_then(|v| v.parse().ok())
            .unwrap_or(default))
    }

    /// DeviceConfig's `package_query_filtering_enabled`: whether filtering
    /// is off.
    fn query_disabled_setting(&self) -> Result<bool, Exception> {
        let value = self
            .host
            .platform()?
            .settings
            .get(Table::Config, &format!("{NAMESPACE}/{QUERY_FILTERING}"));
        Ok(!value.map_or(APP_ENUMERATION_ENABLED_BY_DEFAULT, |v| {
            v.eq_ignore_ascii_case("true")
        }))
    }

    /// `beginSystemReady`: the filtering read and watched, and published.
    fn begin_system_ready(&self) -> Result<(), Exception> {
        let mut state = self.state.lock().unwrap();
        if state.ready {
            return Ok(());
        }
        state.query_disabled = self.query_disabled_setting()?;
        let content = self
            .host
            .content_service
            .get()
            .cloned()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no content service"))?;
        let observer: Arc<dyn LocalObserver> = Arc::new(FilteringObserver(self.me.clone()));
        state.observer = Some(content.register_local(
            &Uri::parse(&format!("content://settings/config/{NAMESPACE}")),
            true,
            0,
            observer,
        )?);
        state.ready = true;
        let disabled = state.query_disabled;
        self.publish(&mut state, disabled)
    }

    /// `publishFiltering`: the owner told, one-way.
    fn publish(&self, state: &mut State, disabled: bool) -> Result<(), Exception> {
        state.query_disabled = disabled;
        let Some(owner) = state.updates else {
            return Ok(());
        };
        let mut p = Parcel::new();
        p.write_interface_token(updates::DESCRIPTOR);
        updates::Changed { disabled }.write(&mut p);
        let sent = match owner {
            Binder::Handle(handle) => self
                .host
                .process
                .transact(handle, updates::CHANGED, &p, true)
                .map(drop),
            Binder::Local(ptr) => match self.host.process.local_service(ptr) {
                Some(owner) => owner.transact(updates::CHANGED, &p, true).map(drop),
                None => Err(aim_binder_host::parcel::DEAD_OBJECT),
            },
        };
        sent.map_err(|s| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("Native filtering publication owner unavailable: {s}"),
            )
        })
    }

    /// `enterSystemReady`: `COMPATIBILITY_MODE` (1 by default) as the
    /// parser's compatibility mode.
    fn enter_system_ready(&self) -> Result<bool, Exception> {
        if !self.state.lock().unwrap().ready {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "PMS filtering readiness has not begun",
            ));
        }
        Ok(self.global_int("compatibility_mode", 1)? == 1)
    }
}

/// `captureBootContext`'s record (version 2): the users (id, flags,
/// pre-created, profile group, unlocking or unlocked), the resolver's
/// theme, its actions' titles and `config_customResolverActivity`, the
/// filtering, and the parser's compatibility mode, true until system ready
/// sets it (`ParsingPackageUtils.sCompatibilityModeEnabled`).
fn record(
    users: &[(bool, super::user_service::ContextUser)],
    platform: &crate::package::parse::Platform,
    query_disabled: bool,
) -> Result<Vec<u8>, Exception> {
    let id = |kind: &str, name: &str| {
        platform
            .framework
            .id(kind, name)
            .map(|id| id as i32)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, format!("no framework {kind}/{name}")))
    };
    let resources = crate::package::parse::resources::Resources {
        tables: vec![&platform.framework],
        overlays: &platform.framework_overlays,
        config: platform.config(),
    };
    let mut p = Parcel::new();
    p.write_i32(2);
    p.write_i32(users.len() as i32);
    for (unlocked, user) in users {
        p.write_i32(user.id);
        p.write_i32(user.flags);
        p.write_bool(user.pre_created);
        p.write_i32(user.profile_group_id);
        p.write_bool(*unlocked);
    }
    p.write_i32(id("style", "Theme.Material.Dialog.Alert")?);
    p.write_i32(ACTION_TITLES.len() as i32);
    for (action, title) in ACTION_TITLES {
        p.write_string16(action);
        p.write_i32(id("string", title)?);
    }
    let custom = id("string", "config_customResolverActivity")?;
    p.write_string16(Some(
        &resources.resource_string(custom as u32).unwrap_or_default(),
    ));
    p.write_bool(query_disabled);
    p.write_bool(true);
    Ok(p.data().to_vec())
}

/// DeviceConfig's listener of the namespace: a change of the key
/// publishes the filtering again.
struct FilteringObserver(Weak<BootContext>);

impl LocalObserver for FilteringObserver {
    fn on_change(&self, _self_change: bool, uris: &[Uri], _flags: i32, _user: i32) {
        let Some(leaf) = self.0.upgrade() else {
            return;
        };
        // `content://settings/config/<namespace>/<key>`.
        if !uris
            .iter()
            .any(|u| u.path_segments().get(2).map(String::as_str) == Some(QUERY_FILTERING))
        {
            return;
        }
        let result = leaf.query_disabled_setting().and_then(|disabled| {
            let mut state = leaf.state.lock().unwrap();
            leaf.publish(&mut state, disabled)
        });
        if let Err(e) = result {
            eprintln!("app host: package filtering: {}", e.message);
        }
    }
}

fn enforce(call: &Call<'_>) -> Result<(), Exception> {
    if call.sender_euid as i32 != SYSTEM_UID {
        return Err(Exception::security("Native boot context owner required"));
    }
    Ok(())
}

impl Service for BootContext {
    fn descriptor(&self) -> &str {
        leaf::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if let Err(e) = enforce(call) {
            return reply(Err(e));
        }
        let mut p = Parcel::new();
        let result = match call.code {
            leaf::CAPTURE_BOOT_CONTEXT => {
                leaf::CaptureBootContext::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.capture()
                    .map(|v| leaf::write_capture_boot_context_reply(&mut p, &Some(v)))
            }
            leaf::DEVICE_PROVISIONED => {
                leaf::DeviceProvisioned::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if self.state.lock().unwrap().ready {
                    self.global_int("device_provisioned", 0)
                        .map(|v| leaf::write_device_provisioned_reply(&mut p, v != 0))
                } else {
                    Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        "SettingsProvider phase has not reached PMS.systemReady",
                    ))
                }
            }
            leaf::QUERY_FILTERING_DISABLED => {
                leaf::QueryFilteringDisabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let disabled = self.state.lock().unwrap().query_disabled;
                leaf::write_query_filtering_disabled_reply(&mut p, disabled);
                Ok(())
            }
            leaf::ENTER_SYSTEM_READY => {
                leaf::EnterSystemReady::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enter_system_ready()
                    .map(|v| leaf::write_enter_system_ready_reply(&mut p, v))
            }
            leaf::REGISTER_FILTERING_UPDATES => {
                let a =
                    leaf::RegisterFilteringUpdates::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                a.owner
                    .ok_or_else(|| {
                        Exception::new(
                            aim_binder_host::parcel::EX_NULL_POINTER,
                            "java.lang.NullPointerException",
                        )
                    })
                    .and_then(|owner| {
                        let mut state = self.state.lock().unwrap();
                        state.updates = Some(owner);
                        let disabled = state.query_disabled;
                        self.publish(&mut state, disabled)
                    })
                    .map(|()| leaf::write_register_filtering_updates_reply(&mut p))
            }
            leaf::BEGIN_SYSTEM_READY => {
                leaf::BeginSystemReady::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.begin_system_ready()
                    .map(|()| leaf::write_begin_system_ready_reply(&mut p))
            }
            leaf::USER_UNLOCKING_OR_UNLOCKED => {
                let a =
                    leaf::UserUnlockingOrUnlocked::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.host
                    .user_unlocking_or_unlocked(a.user_id)
                    .map(|v| leaf::write_user_unlocking_or_unlocked_reply(&mut p, v))
            }
            code => Err(self.host.unimplemented("package", leaf::DESCRIPTOR, code)),
        };
        reply(result.map(|()| p))
    }
}

/// `PackageLifecycleLeaf`: `StorageManager.isCeStorageUnlocked`, which the
/// app host's storage answers from the user's unlock.
pub struct Lifecycle {
    host: Arc<AppHost>,
}

impl Lifecycle {
    pub fn new(host: Arc<AppHost>) -> Self {
        Lifecycle { host }
    }
}

impl Service for Lifecycle {
    fn descriptor(&self) -> &str {
        lifecycle::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid as i32 != SYSTEM_UID {
            return reply(Err(Exception::security(
                "package lifecycle leaf requires native system UID",
            )));
        }
        let mut p = Parcel::new();
        let result = match call.code {
            lifecycle::IS_CE_STORAGE_UNLOCKED => {
                let a =
                    lifecycle::IsCeStorageUnlocked::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.host
                    .user_unlocking_or_unlocked(a.user_id)
                    .map(|v| lifecycle::write_is_ce_storage_unlocked_reply(&mut p, v))
            }
            code => Err(self
                .host
                .unimplemented("package", lifecycle::DESCRIPTOR, code)),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The native boot reads the record back: the users, the resolver's
    /// resources from the image's framework, the filtering.
    #[test]
    fn the_record_is_what_the_native_boot_reads() {
        let Some(root) = aim_paths::original_image_with("system/framework/framework-res.apk")
        else {
            return;
        };
        let platform = super::super::packages::device_platform(&root, 320, "en-US").unwrap();
        let user = super::super::user_service::ContextUser {
            id: 0,
            flags: 0xc13,
            pre_created: false,
            profile_group_id: -10000,
        };
        let bytes = record(&[(false, user)], &platform, true).unwrap();
        let read = crate::package::scan_snapshot::boot_context::Original::decode(&bytes).unwrap();
        assert_eq!(read.users.len(), 1);
        assert_eq!(
            (
                read.users[0].id,
                read.users[0].flags,
                read.users[0].profile_group_id
            ),
            (0, 0xc13, -10000)
        );
        assert!(!read.users[0].unlocking_or_unlocked);
        assert_eq!(
            read.resolver.resolver_theme as u32,
            platform
                .framework
                .id("style", "Theme.Material.Dialog.Alert")
                .unwrap()
        );
        assert_eq!(read.resolver.resolver_titles.len(), 8);
        assert_eq!(read.resolver.resolver_titles[6].0, None);
        assert_eq!(
            read.resolver.resolver_titles[7],
            (
                Some("android.intent.action.MAIN".into()),
                platform
                    .framework
                    .id("string", "whichHomeApplicationLabel")
                    .unwrap() as i32
            )
        );
        assert!(read.resolver.query_filtering_disabled);
        assert!(read.compatibility_mode);
    }
}
