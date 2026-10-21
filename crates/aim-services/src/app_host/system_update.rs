//! `system_update`: SystemUpdateManagerService for the app host
//! (`android-16.0.0_r1`): the status an updater app reports
//! (`updateSystemUpdateInfo`, RECOVERY) kept in
//! `/data/system/system-update-info.xml` with its version, the reporting
//! uid and the boot count, and read back (`retrieveSystemUpdateInfo`,
//! READ_SYSTEM_UPDATE_INFO or RECOVERY) while it is valid for this boot.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_os_isystemupdatemanager as sum;

use super::usage_database::{atomic_delete, atomic_read, atomic_write};
use super::{AppHost, reply};
use crate::package::restrictions::persistable::{Bundle, Value};

const READ_SYSTEM_UPDATE_INFO: &str = "android.permission.READ_SYSTEM_UPDATE_INFO";
const RECOVERY: &str = "android.permission.RECOVERY";
const INFO_FILE: &str = "system/system-update-info.xml";
const INFO_FILE_VERSION: i32 = 0;
const TAG_INFO: &str = "info";
const KEY_VERSION: &str = "version";
const KEY_UID: &str = "uid";
const KEY_BOOT_COUNT: &str = "boot-count";
const KEY_INFO_BUNDLE: &str = "info-bundle";
/// `SystemUpdateManager.KEY_STATUS`, `STATUS_UNKNOWN`, `STATUS_IDLE`.
const KEY_STATUS: &str = "status";
const STATUS_UNKNOWN: i32 = 0;
const STATUS_IDLE: i32 = 1;
const UID_UNKNOWN: i32 = -1;

fn int(bundle: &Bundle, key: &str) -> Option<i32> {
    bundle.entries.iter().rev().find_map(|(k, v)| match v {
        Value::Int(i) if k.as_deref() == Some(key) => Some(*i),
        _ => None,
    })
}

/// A bundle written as `Bundle.writeToParcel` writes it.
struct Info(Bundle);

impl WriteParcelable for Info {
    fn write_to(&self, p: &mut Parcel) {
        // A bundle read from XML or a parcel has no duplicate keys.
        let bytes = self.0.parcel().expect("a read bundle writes");
        p.write_raw(bytes.data(), &[]);
    }
}

#[derive(Default)]
struct State {
    loaded: bool,
    last_uid: i32,
    last_status: i32,
}

pub struct SystemUpdate {
    host: Arc<AppHost>,
    state: Mutex<State>,
}

impl SystemUpdate {
    pub fn new(host: Arc<AppHost>) -> Self {
        SystemUpdate {
            host,
            state: Mutex::new(State {
                loaded: false,
                last_uid: UID_UNKNOWN,
                last_status: STATUS_UNKNOWN,
            }),
        }
    }

    fn file(&self) -> Result<PathBuf, Exception> {
        Ok(self.host.inputs()?.data.join(INFO_FILE))
    }

    fn boot_count(&self) -> i32 {
        self.host.global_setting("boot_count").unwrap_or(0) as i32
    }

    /// `loadSystemUpdateInfoLocked`: the info bundle if the file is valid
    /// for this boot, else the file removed and an UNKNOWN status.
    fn load(&self, state: &mut State) -> Result<Bundle, Exception> {
        state.loaded = true;
        let path = self.file()?;
        let loaded = match atomic_read(&path) {
            Ok(bytes) => match aim_android_xml::read(&bytes) {
                Ok(root) if root.name == TAG_INFO => Bundle::restore(&root)
                    .map_err(|e| {
                        eprintln!("app host: system_update: Failed to parse the info file: {e}")
                    })
                    .ok(),
                Ok(_) => None,
                Err(e) => {
                    eprintln!("app host: system_update: Failed to parse the info file: {e}");
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                eprintln!("app host: system_update: Failed to read the info file: {e}");
                None
            }
        };
        let valid = loaded.and_then(|b| {
            let uid = int(&b, KEY_UID).filter(|&u| u != -1)?;
            int(&b, KEY_VERSION).filter(|&v| v != -1)?;
            int(&b, KEY_BOOT_COUNT).filter(|&c| c != -1 && c == self.boot_count())?;
            let info = b.entries.into_iter().rev().find_map(|(k, v)| match v {
                Value::Bundle(info) if k.as_deref() == Some(KEY_INFO_BUNDLE) => Some(info),
                _ => None,
            })?;
            let status = int(&info, KEY_STATUS).filter(|&s| s != STATUS_UNKNOWN)?;
            Some((uid, status, info))
        });
        Ok(match valid {
            Some((uid, status, info)) => {
                state.last_uid = uid;
                state.last_status = status;
                info
            }
            // `removeInfoFileAndGetDefaultInfoBundleLocked`.
            None => {
                if path.exists() {
                    atomic_delete(&path);
                }
                state.last_uid = UID_UNKNOWN;
                state.last_status = STATUS_UNKNOWN;
                Bundle {
                    entries: vec![(Some(KEY_STATUS.into()), Value::Int(STATUS_UNKNOWN))],
                }
            }
        })
    }

    fn retrieve(&self, uid: i32) -> Result<Bundle, Exception> {
        if self
            .host
            .check_permission(Some(READ_SYSTEM_UPDATE_INFO), uid)?
            != 0
            && self.host.check_permission(Some(RECOVERY), uid)? != 0
        {
            return Err(Exception::security(
                "Can't read system update info. Requiring READ_SYSTEM_UPDATE_INFO or RECOVERY permission.",
            ));
        }
        self.load(&mut self.state.lock().unwrap())
    }

    /// `updateSystemUpdateInfo`: kept unless an inactive updater reports
    /// IDLE over another's status.
    fn update(&self, uid: i32, info: Bundle) -> Result<(), Exception> {
        if self.host.check_permission(Some(RECOVERY), uid)? != 0 {
            return Err(Exception::security(format!(
                "Access denied, requires: {RECOVERY}"
            )));
        }
        let status = int(&info, KEY_STATUS).unwrap_or(STATUS_UNKNOWN);
        if status == STATUS_UNKNOWN {
            eprintln!("app host: system_update: Invalid status info. Ignored");
            return Ok(());
        }
        let mut state = self.state.lock().unwrap();
        if !state.loaded {
            self.load(&mut state)?;
        }
        if !(state.last_uid == UID_UNKNOWN || state.last_uid == uid || status != STATUS_IDLE) {
            eprintln!("app host: system_update: Inactive updater reporting IDLE status. Ignored");
            return Ok(());
        }
        // `saveSystemUpdateInfoLocked`.
        let out = Bundle {
            entries: vec![
                (Some(KEY_INFO_BUNDLE.into()), Value::Bundle(info)),
                (Some(KEY_VERSION.into()), Value::Int(INFO_FILE_VERSION)),
                (Some(KEY_UID.into()), Value::Int(uid)),
                (Some(KEY_BOOT_COUNT.into()), Value::Int(self.boot_count())),
            ],
        };
        let written = out
            .save(TAG_INFO)
            .map_err(|e| e.to_string())
            .and_then(|root| aim_android_xml::abx::write(&root).map_err(|e| e.to_string()))
            .and_then(|bytes| {
                atomic_write(&self.file().map_err(|e| e.message)?, &bytes)
                    .map_err(|e| e.to_string())
            });
        match written {
            Ok(()) => {
                state.last_uid = uid;
                state.last_status = status;
            }
            Err(e) => eprintln!("app host: system_update: Failed to save the info file: {e}"),
        }
        Ok(())
    }
}

impl Service for SystemUpdate {
    fn descriptor(&self) -> &str {
        sum::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            sum::RETRIEVE_SYSTEM_UPDATE_INFO => {
                sum::RetrieveSystemUpdateInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.retrieve(uid).map(|info| {
                    sum::write_retrieve_system_update_info_reply(&mut p, Some(&Info(info)))
                })
            }
            sum::UPDATE_SYSTEM_UPDATE_INFO => {
                let a = sum::UpdateSystemUpdateInfo::<Bundle>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                // A null bundle fails as `infoBundle.getInt` does.
                match a.data {
                    Some(info) => self
                        .update(uid, info)
                        .map(|()| sum::write_update_system_update_info_reply(&mut p)),
                    None => Err(Exception::new(
                        aim_binder_host::parcel::EX_NULL_POINTER,
                        "infoBundle",
                    )),
                }
            }
            code => Err(self
                .host
                .unimplemented("system_update", self.descriptor(), code)),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file `saveSystemUpdateInfoLocked` writes reads back as the
    /// bundle it saved.
    #[test]
    fn the_info_file_reads_back() {
        let info = Bundle {
            entries: vec![(Some(KEY_STATUS.into()), Value::Int(2))],
        };
        let out = Bundle {
            entries: vec![
                (Some(KEY_INFO_BUNDLE.into()), Value::Bundle(info.clone())),
                (Some(KEY_VERSION.into()), Value::Int(0)),
                (Some(KEY_UID.into()), Value::Int(1000)),
                (Some(KEY_BOOT_COUNT.into()), Value::Int(3)),
            ],
        };
        let bytes = aim_android_xml::abx::write(&out.save(TAG_INFO).unwrap()).unwrap();
        let root = aim_android_xml::read(&bytes).unwrap();
        assert_eq!(root.name, TAG_INFO);
        let back = Bundle::restore(&root).unwrap();
        assert_eq!(int(&back, KEY_BOOT_COUNT), Some(3));
        assert!(
            back.entries
                .contains(&(Some(KEY_INFO_BUNDLE.into()), Value::Bundle(info)))
        );
    }
}
