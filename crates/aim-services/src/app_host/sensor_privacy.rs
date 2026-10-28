//! `sensor_privacy`: SensorPrivacyService for the app host
//! (`SensorPrivacyService` at `android-16.0.0_r1`), which audioserver's
//! policy waits for at its start: whether the microphone and camera
//! toggles are supported, and the all-sensor and toggle states the
//! original last persisted (`/data/system/sensor_privacy.xml`,
//! `sensor_privacy_impl.xml`), and the ops the toggles restrict for
//! `appops`. Camera privacy also applies the original SystemConfig allowlist
//! in the software toggle's allowlisted-apps state. Listeners are kept;
//! nothing changes the toggle states (the Mac's
//! microphone and camera privacy: #1476).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel,
};
use aim_service_aidl::android_hardware_isensorprivacymanager as spm;

use super::settings::Table;
use super::{AppHost, reply};

/// `SensorPrivacyManager.TOGGLE_TYPE_*`, `Sensors.*`, `StateTypes.*`.
const TOGGLE_TYPE_SOFTWARE: i32 = 1;
const TOGGLE_TYPE_HARDWARE: i32 = 2;
const MICROPHONE: i32 = 1;
const CAMERA: i32 = 2;
const ENABLED: i32 = 1;
const DISABLED: i32 = 2;
const ENABLED_EXCEPT_ALLOWLISTED_APPS: i32 = 3;
const OBSERVE_SENSOR_PRIVACY: &str = "android.permission.OBSERVE_SENSOR_PRIVACY";
const MANAGE_SENSOR_PRIVACY: &str = "android.permission.MANAGE_SENSOR_PRIVACY";
/// The ops the toggles restrict: `AppOpsManager.OP_CAMERA`,
/// `OP_RECORD_AUDIO`, `OP_PHONE_CALL_MICROPHONE`, `OP_PHONE_CALL_CAMERA`,
/// `OP_RECEIVE_AMBIENT_TRIGGER_AUDIO`,
/// `OP_RECEIVE_EXPLICIT_USER_INTERACTION_AUDIO`,
/// `OP_RECEIVE_SANDBOX_TRIGGER_AUDIO`.
const OP_CAMERA: i32 = 26;
const OP_RECORD_AUDIO: i32 = 27;
const OP_PHONE_CALL_MICROPHONE: i32 = 100;
const OP_PHONE_CALL_CAMERA: i32 = 101;
const OP_RECEIVE_AMBIENT_TRIGGER_AUDIO: i32 = 120;
const OP_RECEIVE_EXPLICIT_USER_INTERACTION_AUDIO: i32 = 121;
const OP_RECEIVE_SANDBOX_TRIGGER_AUDIO: i32 = 136;

/// What the original persisted: the all-sensor state, and each toggle's
/// (toggle type, user, sensor, state type).
#[derive(Debug, Default, PartialEq)]
pub(super) struct Persisted {
    all_sensors: bool,
    toggles: Vec<(i32, i32, i32, i32)>,
}

impl Persisted {
    /// `AllSensorStateController` and `PersistedState` (version 2); a file
    /// that is not there is the default, all disabled.
    fn read(system: &std::path::Path) -> Result<Persisted, String> {
        let read = |name: &str| -> Result<Option<aim_android_xml::Element>, String> {
            let path = system.join(name);
            match std::fs::read(&path) {
                Ok(bytes) => aim_android_xml::read(&bytes)
                    .map(Some)
                    .map_err(|e| format!("{}: {e}", path.display())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(format!("{}: {e}", path.display())),
            }
        };
        let mut persisted = Persisted::default();
        if let Some(root) = read("sensor_privacy.xml")?
            && matches!(root.name.as_str(), "all-sensor-privacy" | "sensor-privacy")
        {
            persisted.all_sensors = root.bool("enabled")?.unwrap_or(false);
        }
        if let Some(root) = read("sensor_privacy_impl.xml")? {
            if root.int("persistence-version")?.unwrap_or(0) != 2 {
                return Err("sensor_privacy_impl.xml: only persistence version 2 is read".into());
            }
            for e in root.children().filter(|e| e.name == "sensor-state") {
                let int = |n: &str| e.int(n)?.ok_or_else(|| format!("sensor-state without {n}"));
                persisted.toggles.push((
                    int("toggle-type")?,
                    int("user-id")?,
                    int("sensor")?,
                    int("state-type")?,
                ));
            }
        }
        Ok(persisted)
    }

    /// `getState(toggleType, user, sensor).getState()`.
    fn state(&self, toggle: i32, user: i32, sensor: i32) -> i32 {
        self.toggles
            .iter()
            .find(|t| (t.0, t.1, t.2) == (toggle, user, sensor))
            .map_or(DISABLED, |t| t.3)
    }
}

/// A call that does not read, or that throws.
enum Failure {
    Status(i32),
    Exception(Exception),
}

impl From<Exception> for Failure {
    fn from(e: Exception) -> Failure {
        Failure::Exception(e)
    }
}

/// What the original persisted, read once.
fn persisted(host: &AppHost) -> Result<&Persisted, Exception> {
    if let Some(p) = host.sensor_privacy.get() {
        return Ok(p);
    }
    let persisted = Persisted::read(&host.data()?.join("system"))
        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
    Ok(host.sensor_privacy.get_or_init(|| persisted))
}

/// `isToggleSensorPrivacyEnabled` of the current user, user 0.
fn toggle_enabled(host: &AppHost, toggle: i32, sensor: i32) -> Result<bool, Exception> {
    Ok(persisted(host)?.state(toggle, 0, sensor) == ENABLED)
}

/// `isCombinedToggleSensorPrivacyEnabled`.
fn combined_enabled(host: &AppHost, sensor: i32) -> Result<bool, Exception> {
    Ok(toggle_enabled(host, TOGGLE_TYPE_SOFTWARE, sensor)?
        || toggle_enabled(host, TOGGLE_TYPE_HARDWARE, sensor)?)
}

/// `SensorPrivacyServiceImpl.isCameraPrivacyEnabled`: only the software
/// toggle decides this package query. Hardware privacy is a global op restriction.
pub(super) fn camera_privacy_enabled(
    host: &AppHost,
    package: Option<&str>,
) -> Result<bool, Exception> {
    match persisted(host)?.state(TOGGLE_TYPE_SOFTWARE, 0, CAMERA) {
        ENABLED => Ok(true),
        ENABLED_EXCEPT_ALLOWLISTED_APPS => {
            let allowlist = host.camera_privacy_allowlist.lock().unwrap();
            let allowlist = allowlist.as_ref().ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    "camera privacy SystemConfig not initialized",
                )
            })?;
            for entry in allowlist {
                let package =
                    package.ok_or_else(|| Exception::new(EX_NULL_POINTER, "packageName"))?;
                if entry.as_deref() == Some(package) {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

/// `setGlobalRestriction` as the service sets it for the current user:
/// whether its toggles restrict `op` for every app (an app ops global
/// restriction), the explicit user interaction audio unless
/// `receive_explicit_user_interaction_audio_enabled` (by default) allows it.
pub(super) fn restricts(host: &AppHost, op: i32) -> Result<bool, Exception> {
    Ok(match op {
        OP_RECORD_AUDIO
        | OP_RECEIVE_SANDBOX_TRIGGER_AUDIO
        | OP_PHONE_CALL_MICROPHONE
        | OP_RECEIVE_AMBIENT_TRIGGER_AUDIO => combined_enabled(host, MICROPHONE)?,
        OP_RECEIVE_EXPLICIT_USER_INTERACTION_AUDIO if combined_enabled(host, MICROPHONE)? => {
            let setting = host.inputs()?.settings.get(
                Table::Global,
                "receive_explicit_user_interaction_audio_enabled",
            );
            setting.and_then(|v| v.parse::<i32>().ok()).unwrap_or(1) != 1
        }
        OP_CAMERA | OP_PHONE_CALL_CAMERA => combined_enabled(host, CAMERA)?,
        _ => false,
    })
}

pub struct SensorPrivacy {
    host: Arc<AppHost>,
    /// The listeners of the all-sensor state and of the toggles, held.
    listeners: Mutex<Vec<(bool, Strong)>>,
}

impl SensorPrivacy {
    pub fn new(host: Arc<AppHost>) -> Self {
        SensorPrivacy {
            host,
            listeners: Mutex::new(Vec::new()),
        }
    }

    /// `enforceObserveSensorPrivacyPermission`.
    fn observe(&self, uid: i32) -> Result<(), Exception> {
        if self
            .host
            .check_permission(Some(OBSERVE_SENSOR_PRIVACY), uid)?
            == 0
        {
            return Ok(());
        }
        Err(Exception::security(format!(
            "Observing sensor privacy changes requires the following permission: {OBSERVE_SENSOR_PRIVACY}"
        )))
    }

    fn listen(
        &self,
        uid: i32,
        toggles: bool,
        add: bool,
        listener: Option<Binder>,
    ) -> Result<(), Exception> {
        self.observe(uid)?;
        let Some(Binder::Handle(handle)) = listener else {
            return Err(if toggles {
                Exception::new(EX_ILLEGAL_ARGUMENT, "listener cannot be null")
            } else {
                Exception::new(EX_NULL_POINTER, "listener cannot be null")
            });
        };
        let mut listeners = self.listeners.lock().unwrap();
        if add {
            if !listeners
                .iter()
                .any(|(t, l)| *t == toggles && l.handle == handle)
            {
                listeners.push((toggles, self.host.process.strong(handle)));
            }
        } else {
            listeners.retain(|(t, l)| !(*t == toggles && l.handle == handle));
        }
        Ok(())
    }
}

impl Service for SensorPrivacy {
    fn descriptor(&self) -> &str {
        spm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result: Result<(), Failure> = (|| {
            match call.code {
                spm::GET_CAMERA_PRIVACY_ALLOWLIST => {
                    spm::GetCameraPrivacyAllowlist::read(&mut call.data)
                        .map_err(|_| Failure::Status(BAD_VALUE))?;
                    self.observe(uid)?;
                    let allowlist = self.host.camera_privacy_allowlist.lock().unwrap();
                    if allowlist.is_none() {
                        return Err(Exception::new(
                            EX_ILLEGAL_STATE,
                            "camera privacy SystemConfig not initialized",
                        )
                        .into());
                    }
                    spm::write_get_camera_privacy_allowlist_reply(&mut p, &allowlist);
                }
                spm::SET_CAMERA_PRIVACY_ALLOWLIST => {
                    let a = spm::SetCameraPrivacyAllowlist::read(&mut call.data)
                        .map_err(|_| Failure::Status(BAD_VALUE))?;
                    if self
                        .host
                        .check_permission(Some(MANAGE_SENSOR_PRIVACY), uid)?
                        != 0
                    {
                        return Err(Exception::security(format!(
                            "Changing sensor privacy requires the following permission: {MANAGE_SENSOR_PRIVACY}"
                        )).into());
                    }
                    let allowlist = a
                        .allowlist
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "allowlist"))?;
                    *self.host.camera_privacy_allowlist.lock().unwrap() = Some(allowlist);
                    spm::write_set_camera_privacy_allowlist_reply(&mut p);
                }
                spm::IS_CAMERA_PRIVACY_ENABLED => {
                    let a = spm::IsCameraPrivacyEnabled::read(&mut call.data)
                        .map_err(|_| Failure::Status(BAD_VALUE))?;
                    self.observe(uid)?;
                    spm::write_is_camera_privacy_enabled_reply(
                        &mut p,
                        camera_privacy_enabled(&self.host, a.package_name.as_deref())?,
                    );
                }
                spm::SUPPORTS_SENSOR_TOGGLE => {
                    let a = spm::SupportsSensorToggle::read(&mut call.data)
                        .map_err(|_| Failure::Status(BAD_VALUE))?;
                    let name = match (a.toggle_type, a.sensor) {
                        (TOGGLE_TYPE_SOFTWARE, MICROPHONE) => Some("config_supportsMicToggle"),
                        (TOGGLE_TYPE_SOFTWARE, CAMERA) => Some("config_supportsCamToggle"),
                        (TOGGLE_TYPE_HARDWARE, MICROPHONE) => {
                            Some("config_supportsHardwareMicToggle")
                        }
                        (TOGGLE_TYPE_HARDWARE, CAMERA) => Some("config_supportsHardwareCamToggle"),
                        _ => None,
                    };
                    let packages = self.host.framework()?;
                    spm::write_supports_sensor_toggle_reply(
                        &mut p,
                        name.and_then(|n| packages.framework_bool(n))
                            .unwrap_or(false),
                    );
                }
                spm::ADD_SENSOR_PRIVACY_LISTENER => {
                    let a = spm::AddSensorPrivacyListener::read(&mut call.data)
                        .map_err(|_| Failure::Status(BAD_VALUE))?;
                    self.listen(uid, false, true, a.listener)?;
                    spm::write_add_sensor_privacy_listener_reply(&mut p);
                }
                spm::ADD_TOGGLE_SENSOR_PRIVACY_LISTENER => {
                    let a = spm::AddToggleSensorPrivacyListener::read(&mut call.data)
                        .map_err(|_| Failure::Status(BAD_VALUE))?;
                    self.listen(uid, true, true, a.listener)?;
                    spm::write_add_toggle_sensor_privacy_listener_reply(&mut p);
                }
                spm::REMOVE_SENSOR_PRIVACY_LISTENER => {
                    let a = spm::RemoveSensorPrivacyListener::read(&mut call.data)
                        .map_err(|_| Failure::Status(BAD_VALUE))?;
                    self.listen(uid, false, false, a.listener)?;
                    spm::write_remove_sensor_privacy_listener_reply(&mut p);
                }
                spm::REMOVE_TOGGLE_SENSOR_PRIVACY_LISTENER => {
                    let a = spm::RemoveToggleSensorPrivacyListener::read(&mut call.data)
                        .map_err(|_| Failure::Status(BAD_VALUE))?;
                    self.listen(uid, true, false, a.listener)?;
                    spm::write_remove_toggle_sensor_privacy_listener_reply(&mut p);
                }
                spm::IS_SENSOR_PRIVACY_ENABLED => {
                    spm::IsSensorPrivacyEnabled::read(&mut call.data)
                        .map_err(|_| Failure::Status(BAD_VALUE))?;
                    self.observe(uid)?;
                    spm::write_is_sensor_privacy_enabled_reply(
                        &mut p,
                        persisted(&self.host)?.all_sensors,
                    );
                }
                spm::IS_COMBINED_TOGGLE_SENSOR_PRIVACY_ENABLED => {
                    let a = spm::IsCombinedToggleSensorPrivacyEnabled::read(&mut call.data)
                        .map_err(|_| Failure::Status(BAD_VALUE))?;
                    self.observe(uid)?;
                    spm::write_is_combined_toggle_sensor_privacy_enabled_reply(
                        &mut p,
                        combined_enabled(&self.host, a.sensor)?,
                    );
                }
                spm::IS_TOGGLE_SENSOR_PRIVACY_ENABLED => {
                    let a = spm::IsToggleSensorPrivacyEnabled::read(&mut call.data)
                        .map_err(|_| Failure::Status(BAD_VALUE))?;
                    self.observe(uid)?;
                    spm::write_is_toggle_sensor_privacy_enabled_reply(
                        &mut p,
                        toggle_enabled(&self.host, a.toggle_type, a.sensor)?,
                    );
                }
                spm::GET_TOGGLE_SENSOR_PRIVACY_STATE => {
                    let a = spm::GetToggleSensorPrivacyState::read(&mut call.data)
                        .map_err(|_| Failure::Status(BAD_VALUE))?;
                    self.observe(uid)?;
                    spm::write_get_toggle_sensor_privacy_state_reply(
                        &mut p,
                        persisted(&self.host)?.state(a.toggle_type, 0, a.sensor),
                    );
                }
                code => {
                    return Err(self
                        .host
                        .unimplemented("sensor_privacy", self.descriptor(), code)
                        .into());
                }
            }
            Ok(())
        })();
        match result {
            Ok(()) => Ok(p),
            Err(Failure::Status(s)) => Err(s),
            Err(Failure::Exception(e)) => reply(Err(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SensorPrivacyServiceImpl.isCameraPrivacyEnabled, android-16.0.0_r1.
    #[test]
    fn camera_software_states_and_global_hardware_restriction() {
        for (state, allowed, other) in [
            (ENABLED, true, true),
            (DISABLED, false, false),
            (ENABLED_EXCEPT_ALLOWLISTED_APPS, false, true),
            (99, false, false),
        ] {
            let host = AppHost::new(crate::test_process(1000));
            host.sensor_privacy
                .set(Persisted {
                    all_sensors: false,
                    toggles: vec![
                        (TOGGLE_TYPE_SOFTWARE, 0, CAMERA, state),
                        (TOGGLE_TYPE_HARDWARE, 0, CAMERA, ENABLED),
                    ],
                })
                .unwrap();
            *host.camera_privacy_allowlist.lock().unwrap() = Some(vec![Some("adas".into()), None]);
            assert_eq!(
                camera_privacy_enabled(&host, Some("adas")).unwrap(),
                allowed
            );
            assert_eq!(camera_privacy_enabled(&host, Some("other")).unwrap(), other);
            assert!(restricts(&host, OP_CAMERA).unwrap());
        }
    }

    fn call(
        service: &dyn Service,
        uid: u32,
        code: u32,
        data: &Parcel,
    ) -> Result<Parcel, Exception> {
        let reply = service
            .transact(&mut Call {
                code,
                flags: 0,
                sender_pid: 77,
                sender_euid: uid,
                data: data.reader(),
            })
            .unwrap();
        reply.reader().read_exception().unwrap()?;
        Ok(reply)
    }

    #[test]
    fn camera_allowlist_binder_permissions_initialization_and_updates() {
        use aim_binder_host::parcel::EX_SECURITY;
        use aim_service_aidl::dev_aim_server_isystemprocess as sp;
        const INIT: u32 = sp::INITIALIZE_CAMERA_PRIVACY_ALLOWLIST;
        crate::system::with_discovery_session_flags(&[], |host| {
            for path in [
                "misc/apexdata/com.android.permission/access.abx",
                "misc_de/0/apexdata/com.android.permission/access.abx",
            ] {
                let path = host.data().unwrap().join(path);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, b"<access/>").unwrap();
            }
            let permissions = Arc::new(super::super::permission_manager::PermissionManager::new(
                host.clone(),
            ));
            host.permission_manager.set(permissions).ok().unwrap();
            host.sensor_privacy
                .set(Persisted {
                    all_sensors: false,
                    toggles: vec![(
                        TOGGLE_TYPE_SOFTWARE,
                        0,
                        CAMERA,
                        ENABLED_EXCEPT_ALLOWLISTED_APPS,
                    )],
                })
                .unwrap();
            let system = super::super::system_process::SystemProcess::new(host.clone());
            let sensor = SensorPrivacy::new(host.clone());
            let mut init = Parcel::new();
            sp::InitializeCameraPrivacyAllowlist {
                packages: Some(vec![Some("adas".into())]),
            }
            .write(&mut init);
            assert_eq!(
                call(&system, 10100, INIT, &init).unwrap_err().code,
                EX_SECURITY
            );
            call(&system, 1000, INIT, &init).unwrap();
            let mut query = Parcel::new();
            spm::IsCameraPrivacyEnabled {
                package_name: Some("adas".into()),
            }
            .write(&mut query);
            let private = || {
                spm::read_is_camera_privacy_enabled_reply(
                    &mut call(&sensor, 1000, spm::IS_CAMERA_PRIVACY_ENABLED, &query)
                        .unwrap()
                        .reader(),
                )
                .unwrap()
                .unwrap()
            };
            assert!(!private());
            let mut set = Parcel::new();
            spm::SetCameraPrivacyAllowlist {
                allowlist: Some(vec![]),
            }
            .write(&mut set);
            assert_eq!(
                call(&sensor, 10100, spm::SET_CAMERA_PRIVACY_ALLOWLIST, &set)
                    .unwrap_err()
                    .code,
                EX_SECURITY
            );
            assert_eq!(
                call(&sensor, 10100, spm::IS_CAMERA_PRIVACY_ENABLED, &query)
                    .unwrap_err()
                    .code,
                EX_SECURITY
            );
            call(&sensor, 1000, spm::SET_CAMERA_PRIVACY_ALLOWLIST, &set).unwrap();
            // Restarting aim-system must not overwrite a live setter's list.
            call(&system, 1000, INIT, &init).unwrap();
            assert!(private());
            let mut get = Parcel::new();
            spm::GetCameraPrivacyAllowlist {}.write(&mut get);
            assert_eq!(
                spm::read_get_camera_privacy_allowlist_reply(
                    &mut call(&sensor, 1000, spm::GET_CAMERA_PRIVACY_ALLOWLIST, &get)
                        .unwrap()
                        .reader()
                )
                .unwrap()
                .unwrap(),
                Some(vec![])
            );
            let mut null = Parcel::new();
            spm::SetCameraPrivacyAllowlist { allowlist: None }.write(&mut null);
            assert_eq!(
                call(&sensor, 1000, spm::SET_CAMERA_PRIVACY_ALLOWLIST, &null)
                    .unwrap_err()
                    .code,
                EX_NULL_POINTER
            );
        });
    }

    #[test]
    fn a_missing_toggle_is_disabled() {
        let p = Persisted {
            all_sensors: false,
            toggles: vec![(TOGGLE_TYPE_SOFTWARE, 0, MICROPHONE, ENABLED)],
        };
        assert_eq!(p.state(TOGGLE_TYPE_SOFTWARE, 0, MICROPHONE), ENABLED);
        assert_eq!(p.state(TOGGLE_TYPE_SOFTWARE, 0, CAMERA), DISABLED);
        assert_eq!(p.state(TOGGLE_TYPE_HARDWARE, 0, MICROPHONE), DISABLED);
    }
}
