//! `IInstallerExternalBridge` for the app host: the native installer's
//! outside owners, answered as system_server's `InstallerExternalBridge`
//! answers them (`android-16.0.0_r1`), to root and the system uid: the
//! uids' importance changes it watches for install constraints, and the
//! guest's install environment. Its other owners are not ported (#2157).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex, Weak};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel,
};
use aim_service_aidl::android_os_iremotecallback as remote_callback;
use aim_service_aidl::dev_aim_server_iinstallerexternalbridge as bridge;

use super::kept::{RawBundle, RawIntent};
use super::package_initial_context::InitialContext;
use super::{AppHost, reply, system_boolean};
use crate::bundle::{self, Value};
use crate::package::installer::preapproval::{IntentSender, Status};
use crate::package::query::Query;
use crate::system::ServiceOwner;

const ROOT_UID: u32 = 0;
const SYSTEM_UID: u32 = 1000;
/// `watchAppState`'s cutpoints: `IMPORTANCE_FOREGROUND` and
/// `IMPORTANCE_FOREGROUND_SERVICE`, a listener each.
const CUTPOINTS: usize = 2;

pub struct InstallerExternal {
    host: Arc<AppHost>,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    closed: bool,
    /// The importance listeners' registrations.
    listeners: Vec<u64>,
}

/// `getNativeInstallEnvironment`'s record: the page size, whether the 16
/// KiB app compat is disabled, the headless system user mode, and the
/// default zone's id.
fn install_environment(
    page_size: i64,
    compat_16kb_disabled: bool,
    headless: bool,
    zone: &str,
) -> Vec<u8> {
    let mut p = Parcel::new();
    p.write_i64(page_size);
    p.write_bool(compat_16kb_disabled);
    p.write_bool(headless);
    p.write_string16(Some(zone));
    p.data().to_vec()
}

/// The listener's event to `callback`: the uid's package
/// (`getNameForUid`, as the system).
fn send_package(host: &Weak<AppHost>, callback: &ServiceOwner, uid: i32) {
    let Some(host) = host.upgrade() else { return };
    let name = match host.packages() {
        Ok((state, resolution)) => Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID as i32,
        }
        .name_for_uid(uid)
        .map_err(|e| host.unimplemented_named("installer external", e.0)),
        Err(e) => Err(e),
    };
    let name = match name {
        Ok(name) => name,
        Err(e) => return eprintln!("app host: constraint state callback: {}", e.message),
    };
    let mut p = Parcel::new();
    p.write_interface_token(remote_callback::DESCRIPTOR);
    p.write_i32(1);
    bundle::write(&mut p, &[("package", Value::String(name))]);
    if let Err(status) = callback.transact(remote_callback::SEND_RESULT, &p, true) {
        eprintln!("app host: constraint state callback failed: {status}");
    }
}

impl InstallerExternal {
    pub fn new(host: Arc<AppHost>) -> Self {
        InstallerExternal {
            host,
            state: Default::default(),
        }
    }

    fn open(&self) -> Result<std::sync::MutexGuard<'_, State>, Exception> {
        let state = self.state.lock().unwrap();
        if state.closed {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "installer external owner closed",
            ));
        }
        Ok(state)
    }

    /// `watchAppState`: `ActivityManager.addOnUidImportanceListener` at
    /// each cutpoint, each change sent on its own thread, as
    /// ActivityManager's handler sends it.
    fn watch_app_state(&self, callback: Option<Binder>) -> Result<(), Exception> {
        let mut state = self.open()?;
        let callback = match callback {
            Some(Binder::Local(ptr)) => self
                .host
                .process
                .local_service(ptr)
                .map(ServiceOwner::Local),
            Some(Binder::Handle(h)) => {
                Some(ServiceOwner::Remote(Arc::new(self.host.process.strong(h))))
            }
            None => None,
        }
        .ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))?;
        let callback = Arc::new(callback);
        for _ in 0..CUTPOINTS {
            let host = Arc::downgrade(&self.host);
            let callback = callback.clone();
            let id = self
                .host
                .add_uid_importance_listener(Arc::new(move |uid, _change| {
                    let (host, callback) = (host.clone(), callback.clone());
                    std::thread::spawn(move || send_package(&host, &callback, uid));
                }));
            state.listeners.push(id);
        }
        Ok(())
    }

    /// `close`: the listeners removed, once.
    fn close(&self) {
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return;
        }
        state.closed = true;
        for id in state.listeners.drain(..) {
            self.host.remove_uid_importance_listener(id);
        }
    }

    /// `getNativeInstallEnvironment`: `TimeZone.getDefault()` is
    /// `persist.sys.timezone`, GMT where unset or not in the image's
    /// tzdata.
    fn install_environment(&self) -> Result<Vec<u8>, Exception> {
        self.open()?;
        let root = self.host.root()?;
        let tzdata = std::fs::read(root.join(crate::tzdata::PATH.trim_start_matches('/')))
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("tzdata: {e}")))?;
        let zone = self
            .host
            .property("persist.sys.timezone")
            .map(|z| z.trim().to_string())
            .filter(|z| crate::tzdata::has(&tzdata, z))
            .unwrap_or_else(|| "GMT".into());
        // `UserManager.isHeadlessSystemUserMode`: the system user is not
        // full.
        let headless = self.host.user_flags()? & super::user_service::FLAG_FULL == 0;
        Ok(install_environment(
            super::package_initial_context::PAGE_SIZE,
            system_boolean(
                self.host.property("pm.16kb.app_compat.disabled").as_deref(),
                false,
            ),
            headless,
            &zone,
        ))
    }

    fn user_action_policy(
        &self,
        installer: Option<&str>,
        uid: i32,
        user: i32,
    ) -> Result<i32, Exception> {
        let mut flags = 0;
        if user == uid / super::PER_USER_RANGE as i32
            && self.host.device_policies()?.can_silently_install_package(
                installer,
                uid,
                |package| self.host.owns_package(uid, package),
            )?
        {
            flags |= 1;
        }
        let config = self
            .host
            .platform()?
            .settings
            .config_flags(Some("package_manager_service/"))?;
        if config
            .get("package_manager_service/is_update_ownership_enforcement_available")
            .and_then(|v| v.as_deref())
            .is_none_or(|v| v.eq_ignore_ascii_case("true"))
        {
            flags |= 2;
        }
        let (state, _) = self.host.packages()?;
        if state
            .system
            .flags
            .iter()
            .any(|(n, on)| n == "android.content.pm.sdk_dependency_installer" && *on)
        {
            flags |= 4;
        }
        if InitialContext::new(self.host.clone()).install_disabled(
            installer.unwrap_or_default(),
            uid,
            user,
        )? {
            flags |= 8;
        }
        Ok(flags)
    }

    fn metadata_limit(&self) -> Result<i64, Exception> {
        let config = self
            .host
            .platform()?
            .settings
            .config_flags(Some("package_manager_service/"))?;
        Ok(config
            .get("package_manager_service/app_metadata_byte_size_limit")
            .and_then(|v| v.as_deref())
            .and_then(|v| v.parse().ok())
            .unwrap_or(32000))
    }

    fn zip_offset(&self, fields: [i32; 6]) -> Result<i32, Exception> {
        let tzdata = std::fs::read(
            self.host
                .root()?
                .join(crate::tzdata::PATH.trim_start_matches('/')),
        )
        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("tzdata: {e}")))?;
        let zone = self.host.property("persist.sys.timezone");
        let rules = zone
            .as_deref()
            .and_then(|z| crate::tzdata::zone(&tzdata, z.trim()))
            .or_else(|| crate::tzdata::zone(&tzdata, "GMT"))
            .and_then(crate::tzdata::Rules::parse)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "tzdata: no default zone"))?;
        Ok(rules.calendar_offset(fields))
    }

    /// `send`: system_server's identity, no background activity launch grant.
    fn send(&self, receiver: Option<IntentSender>, intent: RawIntent) -> Result<(), Exception> {
        let receiver = receiver.ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))?;
        let options = RawBundle::empty()
            .with_ints(&[("android.pendingIntent.backgroundActivityAllowed", 2)])
            .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "installer broadcast options"))?;
        let result = self.host.send_intent_sender(
            SYSTEM_UID as i32,
            None,
            Some(receiver.target),
            0,
            Some(intent),
            None,
            None,
            None,
            Some(options),
        )?;
        if result < 0 {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "installer status delivery failed",
            ));
        }
        Ok(())
    }
}

fn session_status(status: Status, warnings: Vec<Option<String>>) -> RawIntent {
    let mut extras = Vec::new();
    if !warnings.is_empty() {
        extras.push((
            "android.content.pm.extra.WARNINGS",
            Value::NullableStringList(warnings),
        ));
    }
    extras.push((
        "android.content.pm.extra.SESSION_ID",
        Value::Int(status.session_id),
    ));
    extras.push((
        "android.content.pm.extra.PRE_APPROVAL",
        Value::Bool(status.preapproval),
    ));
    if let Some(package) = &status.pending_installer {
        let action = RawIntent::with_data(
            Some(if status.preapproval {
                "android.content.pm.action.CONFIRM_PRE_APPROVAL"
            } else {
                "android.content.pm.action.CONFIRM_INSTALL"
            }),
            None,
            Some(package),
            0,
            &[(
                "android.content.pm.extra.SESSION_ID",
                Value::Int(status.session_id),
            )],
        );
        extras.push(("android.content.pm.extra.STATUS", Value::Int(-1)));
        extras.push((
            "android.intent.extra.INTENT",
            Value::Parcelable {
                class: "android.content.Intent".into(),
                data: action.parcel().data().to_vec(),
            },
        ));
    } else {
        let (public, message) = install_status(status.legacy_status, status.message.as_deref());
        extras.push((
            "android.content.pm.extra.PACKAGE_NAME",
            Value::String(status.package),
        ));
        extras.push(("android.content.pm.extra.STATUS", Value::Int(public)));
        extras.push((
            "android.content.pm.extra.STATUS_MESSAGE",
            Value::String(Some(message)),
        ));
        extras.push((
            "android.content.pm.extra.LEGACY_STATUS",
            Value::Int(status.legacy_status),
        ));
    }
    RawIntent::of(None, 0, &extras)
}

fn pending_streaming(id: i32, cause: Option<&str>) -> RawIntent {
    let message = match cause.filter(|c| !c.is_empty()) {
        Some(cause) => format!("Staging Image Not Ready [{cause}]"),
        None => "Staging Image Not Ready".into(),
    };
    RawIntent::of(
        None,
        0,
        &[
            ("android.content.pm.extra.SESSION_ID", Value::Int(id)),
            ("android.content.pm.extra.STATUS", Value::Int(-2)),
            (
                "android.content.pm.extra.STATUS_MESSAGE",
                Value::String(Some(message)),
            ),
        ],
    )
}

/// `PackageManager.installStatusToPublicStatus` and `installStatusToString`, android-16.0.0_r1.
fn install_status(status: i32, message: Option<&str>) -> (i32, String) {
    let public = match status {
        1 => 0,
        -110 => 1,
        -129 => 2,
        -21 | -22 | -115 => 3,
        -2 | -3 | -11 | -15 | -23 | -24 | -25 | -26 | -100 | -101 | -102 | -103 | -104 | -105
        | -106 | -107 | -108 | -109 | -117 | -118 => 4,
        -1 | -5 | -6 | -7 | -8 | -10 | -13 | -112 => 5,
        -4 | -18 | -19 | -20 => 6,
        -9 | -12 | -14 | -16 | -17 | -111 | -113 | -28 | -29 => 7,
        _ => 1,
    };
    let name = install_status_name(status).map_or_else(|| status.to_string(), str::to_string);
    (
        public,
        match message {
            Some(message) => format!("{name}: {message}"),
            None => name,
        },
    )
}

/// `PackageManager.installStatusToString`'s names.
fn install_status_name(status: i32) -> Option<&'static str> {
    Some(match status {
        1 => "INSTALL_SUCCEEDED",
        -1 => "INSTALL_FAILED_ALREADY_EXISTS",
        -2 => "INSTALL_FAILED_INVALID_APK",
        -3 => "INSTALL_FAILED_INVALID_URI",
        -4 => "INSTALL_FAILED_INSUFFICIENT_STORAGE",
        -5 => "INSTALL_FAILED_DUPLICATE_PACKAGE",
        -6 => "INSTALL_FAILED_NO_SHARED_USER",
        -7 => "INSTALL_FAILED_UPDATE_INCOMPATIBLE",
        -8 => "INSTALL_FAILED_SHARED_USER_INCOMPATIBLE",
        -9 => "INSTALL_FAILED_MISSING_SHARED_LIBRARY",
        -10 => "INSTALL_FAILED_REPLACE_COULDNT_DELETE",
        -11 => "INSTALL_FAILED_DEXOPT",
        -12 => "INSTALL_FAILED_OLDER_SDK",
        -13 => "INSTALL_FAILED_CONFLICTING_PROVIDER",
        -14 => "INSTALL_FAILED_NEWER_SDK",
        -15 => "INSTALL_FAILED_TEST_ONLY",
        -16 => "INSTALL_FAILED_CPU_ABI_INCOMPATIBLE",
        -17 => "INSTALL_FAILED_MISSING_FEATURE",
        -18 => "INSTALL_FAILED_CONTAINER_ERROR",
        -19 => "INSTALL_FAILED_INVALID_INSTALL_LOCATION",
        -20 => "INSTALL_FAILED_MEDIA_UNAVAILABLE",
        -21 => "INSTALL_FAILED_VERIFICATION_TIMEOUT",
        -22 => "INSTALL_FAILED_VERIFICATION_FAILURE",
        -23 => "INSTALL_FAILED_PACKAGE_CHANGED",
        -24 => "INSTALL_FAILED_UID_CHANGED",
        -25 => "INSTALL_FAILED_VERSION_DOWNGRADE",
        -100 => "INSTALL_PARSE_FAILED_NOT_APK",
        -101 => "INSTALL_PARSE_FAILED_BAD_MANIFEST",
        -102 => "INSTALL_PARSE_FAILED_UNEXPECTED_EXCEPTION",
        -103 => "INSTALL_PARSE_FAILED_NO_CERTIFICATES",
        -104 => "INSTALL_PARSE_FAILED_INCONSISTENT_CERTIFICATES",
        -105 => "INSTALL_PARSE_FAILED_CERTIFICATE_ENCODING",
        -106 => "INSTALL_PARSE_FAILED_BAD_PACKAGE_NAME",
        -107 => "INSTALL_PARSE_FAILED_BAD_SHARED_USER_ID",
        -108 => "INSTALL_PARSE_FAILED_MANIFEST_MALFORMED",
        -109 => "INSTALL_PARSE_FAILED_MANIFEST_EMPTY",
        -110 => "INSTALL_FAILED_INTERNAL_ERROR",
        -111 => "INSTALL_FAILED_USER_RESTRICTED",
        -112 => "INSTALL_FAILED_DUPLICATE_PERMISSION",
        -113 => "INSTALL_FAILED_NO_MATCHING_ABIS",
        -115 => "INSTALL_FAILED_ABORTED",
        -117 => "INSTALL_FAILED_BAD_DEX_METADATA",
        -28 => "INSTALL_FAILED_MISSING_SPLIT",
        -29 => "INSTALL_FAILED_DEPRECATED_SDK_VERSION",
        -118 => "INSTALL_FAILED_BAD_SIGNATURE",
        -121 => "INSTALL_FAILED_WRONG_INSTALLED_VERSION",
        -122 => "INSTALL_FAILED_PROCESS_NOT_DEFINED",
        -116 => "INSTALL_FAILED_SESSION_INVALID",
        -130 => "INSTALL_FAILED_SHARED_LIBRARY_BAD_CERTIFICATE_DIGEST",
        -131 => "INSTALL_FAILED_MULTI_ARCH_NOT_MATCH_ALL_NATIVE_ABIS",
        _ => return None,
    })
}

impl Service for InstallerExternal {
    fn descriptor(&self) -> &str {
        bridge::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != SYSTEM_UID && call.sender_euid != ROOT_UID {
            return reply(Err(Exception::security(
                "untrusted installer bridge caller",
            )));
        }
        if matches!(
            call.code,
            bridge::IS_COMMIT_MUTABLE_RECEIVER_ENFORCED
                | bridge::IS_INTENT_SENDER_IMMUTABLE
                | bridge::IS_SECURE_FRP_ACTIVE
                | bridge::GET_USER_ACTION_POLICY
                | bridge::IS_SILENT_INSTALL_TARGET_ALLOWED
                | bridge::GET_APP_METADATA_SIZE_LIMIT
                | bridge::GET_ZIP_LOCAL_UTC_OFFSET
                | bridge::SEND_SESSION_STATUS
                | bridge::SEND_SESSION_STATUS_WITH_WARNINGS
                | bridge::SEND_PENDING_STREAMING
        ) && let Err(e) = self.open()
        {
            return reply(Err(e));
        }
        let mut p = Parcel::new();
        let result = match call.code {
            bridge::WATCH_APP_STATE => {
                let a = bridge::WatchAppState::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.watch_app_state(a.callback)
            }
            bridge::CLOSE => {
                bridge::Close::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.close();
                bridge::write_close_reply(&mut p);
                Ok(())
            }
            bridge::GET_NATIVE_INSTALL_ENVIRONMENT => {
                bridge::GetNativeInstallEnvironment::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.install_environment().map(|record| {
                    bridge::write_get_native_install_environment_reply(&mut p, &Some(record))
                })
            }
            bridge::IS_COMMIT_MUTABLE_RECEIVER_ENFORCED => {
                let a = bridge::IsCommitMutableReceiverEnforced::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.host.change_enabled(240618202, a.uid).map(|enabled| {
                    bridge::write_is_commit_mutable_receiver_enforced_reply(&mut p, enabled)
                })
            }
            bridge::IS_INTENT_SENDER_IMMUTABLE => {
                let a = bridge::IsIntentSenderImmutable::<IntentSender>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                a.receiver
                    .ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))
                    .and_then(|r| self.host.intent_sender_immutable(r.target))
                    .map(|immutable| {
                        bridge::write_is_intent_sender_immutable_reply(&mut p, immutable)
                    })
            }
            bridge::IS_SECURE_FRP_ACTIVE => {
                bridge::IsSecureFrpActive::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // With frpEnforcement on, this image's PersistentDataBlockManager is null.
                bridge::write_is_secure_frp_active_reply(&mut p, false);
                Ok(())
            }
            bridge::GET_USER_ACTION_POLICY => {
                let a = bridge::GetUserActionPolicy::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.user_action_policy(a.installer.as_deref(), a.installer_uid, a.user_id)
                    .map(|flags| bridge::write_get_user_action_policy_reply(&mut p, flags))
            }
            bridge::IS_SILENT_INSTALL_TARGET_ALLOWED => {
                let a = bridge::IsSilentInstallTargetAllowed::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let enabled = if a.target_sdk == i32::MAX {
                    Ok(false)
                } else {
                    self.host.platform().map(|inputs| {
                        inputs
                            .compat
                            .is_enabled(325888262, a.package_name.as_deref(), a.target_sdk)
                    })
                };
                enabled.map(|enabled| {
                    bridge::write_is_silent_install_target_allowed_reply(&mut p, enabled)
                })
            }
            bridge::GET_APP_METADATA_SIZE_LIMIT => {
                bridge::GetAppMetadataSizeLimit::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.metadata_limit()
                    .map(|limit| bridge::write_get_app_metadata_size_limit_reply(&mut p, limit))
            }
            bridge::GET_ZIP_LOCAL_UTC_OFFSET => {
                let a =
                    bridge::GetZipLocalUtcOffset::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.zip_offset([a.year, a.month, a.day, a.hour, a.minute, a.second])
                    .map(|offset| bridge::write_get_zip_local_utc_offset_reply(&mut p, offset))
            }
            bridge::SEND_SESSION_STATUS | bridge::SEND_SESSION_STATUS_WITH_WARNINGS => {
                let (a, warnings) = if call.code == bridge::SEND_SESSION_STATUS {
                    let a = bridge::SendSessionStatus::<IntentSender>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                    (a, Some(Vec::new()))
                } else {
                    let a =
                        bridge::SendSessionStatusWithWarnings::<IntentSender>::read(&mut call.data)
                            .map_err(|_| BAD_VALUE)?;
                    (
                        bridge::SendSessionStatus {
                            receiver: a.receiver,
                            session_id: a.session_id,
                            package_name: a.package_name,
                            legacy_status: a.legacy_status,
                            message: a.message,
                            preapproval: a.preapproval,
                            pending_installer_package: a.pending_installer_package,
                        },
                        a.warnings,
                    )
                };
                a.receiver
                    .ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))
                    .and_then(|receiver| {
                        let warnings =
                            warnings.ok_or_else(|| Exception::new(EX_NULL_POINTER, ""))?;
                        let intent = session_status(
                            Status {
                                receiver,
                                session_id: a.session_id,
                                package: a.package_name,
                                legacy_status: a.legacy_status,
                                message: a.message,
                                preapproval: a.preapproval,
                                pending_installer: a.pending_installer_package,
                            },
                            warnings,
                        );
                        self.send(Some(receiver), intent)
                    })
                    .map(|()| bridge::write_send_session_status_reply(&mut p))
            }
            bridge::SEND_PENDING_STREAMING => {
                let a = bridge::SendPendingStreaming::<IntentSender>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.send(
                    a.receiver,
                    pending_streaming(a.session_id, a.message.as_deref()),
                )
                .map(|()| bridge::write_send_pending_streaming_reply(&mut p))
            }
            code => {
                let name = bridge::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("installer external", &format!("{name} (#2157)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::Reader;
    use std::fs;

    fn call(
        leaf: &InstallerExternal,
        code: u32,
        args: impl FnOnce(&mut Parcel),
    ) -> Result<Vec<u8>, Exception> {
        let mut p = Parcel::new();
        p.write_interface_token(bridge::DESCRIPTOR);
        args(&mut p);
        let response = leaf
            .transact(&mut Call {
                code,
                flags: 0,
                sender_pid: 77,
                sender_euid: SYSTEM_UID,
                data: p.reader(),
            })
            .unwrap();
        let mut r = response.reader();
        r.read_exception().unwrap()?;
        Ok(response.data()[r.position()..].to_vec())
    }

    fn integer(leaf: &InstallerExternal, code: u32, args: impl FnOnce(&mut Parcel)) -> i32 {
        Reader::new(&call(leaf, code, args).unwrap(), &[])
            .read_i32()
            .unwrap()
    }

    fn setting(host: &AppHost, name: &str, value: &str) {
        host.platform()
            .unwrap()
            .settings
            .insert(
                super::super::settings::Table::Config,
                &format!("package_manager_service/{name}"),
                &super::super::settings::Change {
                    value: Some(value),
                    tag: None,
                    make_default: false,
                    package: Some("android"),
                    caller_system: true,
                    overrideable_by_restore: false,
                },
                false,
            )
            .unwrap();
    }

    #[test]
    fn commit_policy_reads_its_owners() {
        crate::system::with_discovery_session(|host, publish, _| {
            publish(&mut |p| p.target_sdk_version = if p.package_name == "p" { 36 } else { 32 });
            let dir = host.root().unwrap().join("system/etc/compatconfig");
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("installer.xml"),
                br#"<config>
                <compat-change id="240618202" enableSinceTargetSdk="35"/>
                <compat-change id="325888262" enableSinceTargetSdk="35"/>
                </config>"#,
            )
            .unwrap();
            let data = host.data().unwrap();
            fs::write(data.join("system/device_owner_2.xml"),
                br#"<root><device-owner component="p/.Admin"/><device-owner-context userId="0"/></root>"#).unwrap();
            host.user_restrictions
                .set(["no_install_unknown_sources".into()].into())
                .unwrap();
            let leaf = InstallerExternal::new(host.clone());
            for (uid, expected) in [(10100, 1), (10101, 0)] {
                assert_eq!(
                    integer(&leaf, bridge::IS_COMMIT_MUTABLE_RECEIVER_ENFORCED, |p| p
                        .write_i32(uid)),
                    expected
                );
            }
            for (sdk, expected) in [(34, 0), (35, 1), (i32::MAX, 0)] {
                assert_eq!(
                    integer(&leaf, bridge::IS_SILENT_INSTALL_TARGET_ALLOWED, |p| {
                        p.write_string16(Some("p"));
                        p.write_i32(sdk);
                    }),
                    expected
                );
            }
            let policy = |uid| leaf.user_action_policy(Some("p"), uid, 0).unwrap();
            assert_eq!(policy(10100) & !4, 1 | 2 | 8);
            assert_eq!(policy(10101) & !4, 2 | 8);
            setting(host, "is_update_ownership_enforcement_available", "false");
            assert_eq!(policy(10100) & !4, 1 | 8);
            assert_eq!(integer(&leaf, bridge::IS_SECURE_FRP_ACTIVE, |_| {}), 0);
            assert!(
                call(&leaf, bridge::IS_PREAPPROVAL_REQUEST_AVAILABLE, |_| {})
                    .unwrap_err()
                    .message
                    .contains("#2157")
            );
        });
    }

    fn extras(intent: &RawIntent) -> (Parcel, std::collections::HashMap<String, Value>) {
        let p = intent.parcel();
        let mut r = p.reader();
        r.read_string8().unwrap();
        r.read_i32().unwrap(); // action, null Uri
        r.read_string8().unwrap();
        r.read_string8().unwrap(); // type, identifier
        r.read_i32().unwrap();
        r.read_i32().unwrap(); // flags
        r.read_string8().unwrap();
        r.read_string16().unwrap(); // package, component
        for _ in 0..5 {
            r.read_i32().unwrap();
        } // bounds, categories, selector, clip, user
        let values = bundle::read(&mut r).unwrap().unwrap();
        (p, values)
    }

    #[test]
    fn intent_sender_records_and_status_delivery() {
        crate::system::with_discovery_session(|host, _, _| {
            let leaf = InstallerExternal::new(host.clone());
            let make = |flags| IntentSender {
                target: host
                    .get_intent_sender(
                        1000,
                        super::super::pending_intents::Request {
                            kind: super::super::pending_intents::INTENT_SENDER_BROADCAST,
                            package: Some("android".into()),
                            feature: None,
                            token: None,
                            result_who: None,
                            request_code: flags,
                            intents: Some(vec![Some(RawIntent::new("test.status", 0, &[]))]),
                            resolved_types: None,
                            flags,
                            options: None,
                            user: 0,
                        },
                    )
                    .unwrap()
                    .unwrap(),
            };
            let immutable = make(1 << 26);
            let receiver = make(1 << 30);
            for (sender, expected) in [(immutable, 1), (receiver, 0)] {
                assert_eq!(
                    integer(&leaf, bridge::IS_INTENT_SENDER_IMMUTABLE, |p| {
                        p.write_i32(1);
                        p.write_binder(Some(sender.target));
                    }),
                    expected
                );
            }
            let status = Status {
                receiver,
                session_id: 7,
                package: Some("p".into()),
                legacy_status: -2,
                message: Some("bad".into()),
                preapproval: false,
                pending_installer: None,
            };
            let (p, values) = extras(&session_status(
                status.clone(),
                vec![Some("warning".into()), None],
            ));
            assert_eq!(values["android.content.pm.extra.STATUS"], Value::Int(4));
            assert_eq!(
                values["android.content.pm.extra.STATUS_MESSAGE"],
                Value::String(Some("INSTALL_FAILED_INVALID_APK: bad".into()))
            );
            let Value::Lazy { start, .. } = values["android.content.pm.extra.WARNINGS"] else {
                panic!()
            };
            let mut r = p.reader();
            r.set_position(start);
            assert_eq!(r.read_i32().unwrap(), 2);
            assert_eq!(r.read_i32().unwrap(), bundle::VAL_STRING);
            assert_eq!(r.read_string16().unwrap().as_deref(), Some("warning"));
            assert_eq!(r.read_i32().unwrap(), -1);
            let (_, pending) = extras(&session_status(
                Status {
                    preapproval: true,
                    pending_installer: Some("installer".into()),
                    ..status.clone()
                },
                vec![],
            ));
            assert_eq!(pending["android.content.pm.extra.STATUS"], Value::Int(-1));
            assert!(pending.contains_key("android.intent.extra.INTENT"));
            let (_, streaming) = extras(&pending_streaming(7, Some("wait")));
            assert_eq!(streaming["android.content.pm.extra.STATUS"], Value::Int(-2));
            assert_eq!(
                streaming["android.content.pm.extra.STATUS_MESSAGE"],
                Value::String(Some("Staging Image Not Ready [wait]".into()))
            );
            call(&leaf, bridge::SEND_SESSION_STATUS, |p| {
                p.write_i32(1);
                p.write_binder(Some(receiver.target));
                p.write_i32(7);
                p.write_string16(Some("p"));
                p.write_i32(1);
                p.write_string16(None);
                p.write_bool(false);
                p.write_string16(None);
            })
            .unwrap();
            assert!(host.intent_sender_immutable(receiver.target).is_err()); // one-shot consumed
            assert!(
                leaf.send(Some(receiver), pending_streaming(7, None))
                    .is_err()
            );
            assert!(
                leaf.send(
                    Some(IntentSender {
                        target: Binder::Handle(u32::MAX)
                    }),
                    pending_streaming(7, None)
                )
                .is_err()
            );
        });
    }

    #[test]
    fn time_and_metadata_use_the_image_and_config() {
        let path = crate::tzdata::PATH.trim_start_matches('/');
        let root = aim_paths::original_image_with(path).expect("pinned tzdata image");
        let tzdata = fs::read(root.join(path)).unwrap();
        let zone = |name| {
            crate::tzdata::Rules::parse(crate::tzdata::zone(&tzdata, name).unwrap()).unwrap()
        };
        let york = zone("America/New_York");
        for (fields, offset) in [
            ([2024, 1, 1, 12, 0, 0], -18000),
            ([2024, 7, 1, 12, 0, 0], -14400),
            ([2024, 3, 10, 2, 30, 0], -14400),
            ([2024, 11, 3, 1, 30, 0], -18000),
            ([2023, 13, 1, 12, 0, 0], -18000),
        ] {
            assert_eq!(york.calendar_offset(fields), offset);
        }
        assert_eq!(
            zone("Asia/Seoul").calendar_offset([2024, 6, 1, 12, 0, 0]),
            32400
        );
        crate::system::with_discovery_session(|host, _, _| {
            let dest = host.root().unwrap().join(path);
            fs::create_dir_all(dest.parent().unwrap()).unwrap();
            fs::write(dest, &tzdata).unwrap();
            let leaf = InstallerExternal::new(host.clone());
            assert_eq!(
                integer(&leaf, bridge::GET_ZIP_LOCAL_UTC_OFFSET, |p| {
                    for field in [2024, 1, 1, 0, 0, 0] {
                        p.write_i32(field);
                    }
                }),
                0
            ); // unset default zone is GMT
            for (value, expected) in [
                (None, 32000),
                (Some("64000"), 64000),
                (Some("bad"), 32000),
                (Some("-1"), -1),
            ] {
                if let Some(value) = value {
                    setting(host, "app_metadata_byte_size_limit", value);
                }
                let bytes = call(&leaf, bridge::GET_APP_METADATA_SIZE_LIMIT, |_| {}).unwrap();
                assert_eq!(Reader::new(&bytes, &[]).read_i64().unwrap(), expected);
            }
        });
    }

    /// The record reads as the native installer reads it.
    #[test]
    fn the_install_environment_is_the_originals_record() {
        let record = install_environment(16_384, false, true, "Asia/Seoul");
        let mut r = aim_binder_host::parcel::Reader::new(&record, &[]);
        assert_eq!(r.read_i64().unwrap(), 16_384);
        assert!(!r.read_bool().unwrap());
        assert!(r.read_bool().unwrap());
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("Asia/Seoul"));
        assert_eq!(r.remaining(), 0);
    }

    /// The uids' changes reach the importance listeners until they are
    /// removed; a closed owner refuses to watch.
    #[test]
    fn importance_listeners_hear_the_uids_changes() {
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: SYSTEM_UID,
                security_context: None,
            },
        );
        let host = AppHost::new(process);
        let heard = Arc::new(Mutex::new(Vec::new()));
        let into = heard.clone();
        let id = host.add_uid_importance_listener(Arc::new(move |uid, change| {
            into.lock().unwrap().push((
                uid,
                matches!(change, super::super::uid_observers::Change::Gone),
            ));
        }));
        host.uid_changed(10_100, super::super::uid_observers::Change::Active);
        host.uid_changed(10_100, super::super::uid_observers::Change::Gone);
        host.remove_uid_importance_listener(id);
        host.uid_changed(10_101, super::super::uid_observers::Change::Active);
        assert_eq!(*heard.lock().unwrap(), [(10_100, false), (10_100, true)]);
        let external = InstallerExternal::new(host);
        external.close();
        let e = external.watch_app_state(None).unwrap_err();
        assert_eq!(e.message, "installer external owner closed");
    }
}
