//! `lock_settings`: LockSettingsService for the app host
//! (`android-16.0.0_r1`): the settings the original persisted in
//! `/data/system/locksettings.db` (`LockSettingsStorage`), and the
//! credential type of the device's lock, which is the Mac's (the owner's
//! choice, 2026-10-20, as `trust`'s `isDeviceSecure`): a password while
//! the Mac's user has one (`aim_host_lock`), none otherwise. No credential
//! is set, read or verified here. Of the recoverable keystore, the
//! recovery secret types an agent set (`recoverablekeystore.db`). The
//! writes, the special users (FRP, repair mode), strong auth, escrow tokens
//! and the rest of recovery are not implemented (#1888).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel,
};
use aim_service_aidl::com_android_internal_widget_ilocksettings as ls;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use super::{AppHost, reply};

const RECOVER_KEYSTORE: &str = "android.permission.RECOVER_KEYSTORE";
const PER_USER_RANGE: i32 = 100_000;

/// `LockSettingsService.PERMISSION`.
const ACCESS_KEYGUARD_SECURE_STORAGE: &str = "android.permission.ACCESS_KEYGUARD_SECURE_STORAGE";
const FEATURE_SECURE_LOCK_SCREEN: &str = "android.software.secure_lock_screen";
/// `LockSettingsService.SEPARATE_PROFILE_CHALLENGE_KEY`.
const SEPARATE_PROFILE_CHALLENGE_KEY: &str = "lockscreen.profilechallenge";
/// `LockPatternUtils.CREDENTIAL_TYPE_*`.
const CREDENTIAL_TYPE_NONE: i32 = -1;
const CREDENTIAL_TYPE_PASSWORD: i32 = 4;
/// `LockPatternUtils.USER_FRP`, `USER_REPAIR_MODE` (`USER_NULL` + 1, + 2).
const USER_FRP: i32 = -9999;
const USER_REPAIR_MODE: i32 = -9998;

/// A recoverable keystore database failure.
fn db_error(path: &Path, e: rusqlite::Error) -> Exception {
    Exception::new(BAD_VALUE, format!("{}: {e}", path.display()))
}

/// `LockPatternUtils.isSpecialUserId`.
fn is_special_user(user: i32) -> bool {
    user == USER_FRP || user == USER_REPAIR_MODE
}

/// `LockSettingsStorage.readKeyValue` from the database at `db`: none
/// where there is no database yet, which the original would create empty.
fn read_key_value(db: &Path, key: &str, user: i32) -> Result<Option<String>, String> {
    if !db.exists() {
        return Ok(None);
    }
    let db = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| e.to_string())?;
    db.query_row(
        "SELECT value FROM locksettings WHERE user=?1 AND name=?2",
        params![user.to_string(), key],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()
    .map(Option::flatten)
    .map_err(|e| e.to_string())
}

pub struct LockSettings {
    host: Arc<AppHost>,
    /// `RecoverySnapshotListenersStorage`: each agent's snapshot intent.
    /// It is sent when a snapshot is created, which needs the lock
    /// screen's knowledge factor the app host never has.
    snapshot_listeners: Mutex<HashMap<i32, Strong>>,
}

impl LockSettings {
    pub fn new(host: Arc<AppHost>) -> Self {
        LockSettings {
            host,
            snapshot_listeners: Mutex::new(HashMap::new()),
        }
    }

    /// `checkPasswordHavePermission`.
    fn check_password_have(&self, uid: i32) -> Result<(), Exception> {
        if self
            .host
            .check_permission(Some(ACCESS_KEYGUARD_SECURE_STORAGE), uid)?
            != 0
        {
            return Err(Exception::security(format!(
                "LockSettingsHave: Neither user {uid} nor current process has {ACCESS_KEYGUARD_SECURE_STORAGE}."
            )));
        }
        Ok(())
    }

    /// `checkDatabaseReadPermission`.
    fn check_database_read(&self, uid: i32, key: &str, user: i32) -> Result<(), Exception> {
        if self
            .host
            .check_permission(Some(ACCESS_KEYGUARD_SECURE_STORAGE), uid)?
            != 0
        {
            return Err(Exception::security(format!(
                "uid={uid} needs permission {ACCESS_KEYGUARD_SECURE_STORAGE} to read {key} for user {user}"
            )));
        }
        Ok(())
    }

    /// `LockSettingsStorage.getString`: none for a special user.
    fn string(&self, key: &str, user: i32) -> Result<Option<String>, Exception> {
        if is_special_user(user) {
            return Ok(None);
        }
        let db = self.host.data()?.join("system/locksettings.db");
        read_key_value(&db, key, user)
            .map_err(|e| Exception::new(BAD_VALUE, format!("{}: {e}", db.display())))
    }

    /// `LockSettingsStorage.getBoolean`.
    fn boolean(&self, key: &str, default: bool, user: i32) -> Result<bool, Exception> {
        Ok(match self.string(key, user)? {
            Some(v) if !v.is_empty() => v == "1" || v == "true",
            _ => default,
        })
    }

    /// `LockSettingsStorage.getLong`: a value that is no long fails as
    /// `Long.parseLong` does.
    fn long(&self, key: &str, default: i64, user: i32) -> Result<i64, Exception> {
        match self.string(key, user)? {
            Some(v) if !v.is_empty() => v.parse().map_err(|_| {
                Exception::new(EX_ILLEGAL_ARGUMENT, format!("For input string: \"{v}\""))
            }),
            _ => Ok(default),
        }
    }

    /// `getCredentialTypeInternal` of the Mac's lock: a password while
    /// the Mac's user has one, as `isDeviceSecure` answers.
    fn credential_type(&self, user: i32) -> Result<i32, Exception> {
        if is_special_user(user) {
            return Err(self
                .host
                .unimplemented_named("lock_settings", "a special user's credential (#1888)"));
        }
        let secure = aim_host_lock::secure().map_err(|code| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("lock_settings: the Mac's lock: LocalAuthentication error {code}"),
            )
        })?;
        Ok(if secure {
            CREDENTIAL_TYPE_PASSWORD
        } else {
            CREDENTIAL_TYPE_NONE
        })
    }

    /// `checkRecoverKeyStorePermission` for the agent `uid`:
    /// RECOVER_KEYSTORE, and its user registered
    /// (`CleanupManager.registerRecoveryAgent`, which records the user's
    /// serial number once). The keystore's database, open.
    fn check_recovery_agent(&self, uid: i32) -> Result<(Connection, PathBuf), Exception> {
        if self.host.check_permission(Some(RECOVER_KEYSTORE), uid)? != 0 {
            return Err(Exception::security(format!(
                "Caller {uid} doesn't have RecoverKeyStore permission.: Neither user {uid} nor current process has {RECOVER_KEYSTORE}."
            )));
        }
        let path = self.host.data()?.join("system/recoverablekeystore.db");
        if !path.exists() {
            return Err(self.host.unimplemented_named(
                "lock_settings",
                "registering a recovery agent's user in a new recoverable keystore (#1888)",
            ));
        }
        let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| db_error(&path, e))?;
        let serial: Option<i64> = db
            .query_row(
                "SELECT user_serial_number FROM user_metadata WHERE user_id=?1",
                params![uid / PER_USER_RANGE],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| db_error(&path, e))?;
        if serial.is_none_or(|s| s == -1) {
            return Err(self.host.unimplemented_named(
                "lock_settings",
                "registering a recovery agent's user (#1888)",
            ));
        }
        Ok((db, path))
    }

    /// `RecoverableKeyStoreManager.getRecoverySecretTypes` for `uid`: the
    /// types the agent set (`RecoverableKeyStoreDb.getRecoverySecretTypes`),
    /// none if it set none.
    fn recovery_secret_types(&self, uid: i32) -> Result<Vec<i32>, Exception> {
        let (db, path) = self.check_recovery_agent(uid)?;
        let failed = |e| db_error(&path, e);
        let user = uid / PER_USER_RANGE;
        let mut rows = db
            .prepare(
                "SELECT secret_types FROM recovery_service_metadata WHERE user_id=?1 AND uid=?2",
            )
            .map_err(failed)?;
        let found: Vec<Option<String>> = rows
            .query_map(params![user, uid], |r| r.get(0))
            .map_err(failed)?
            .collect::<Result<_, _>>()
            .map_err(failed)?;
        // One entry at most; none, more or an empty one are no types.
        let [Some(csv)] = found.as_slice() else {
            return Ok(Vec::new());
        };
        // A part that is no number is logged by the original and left 0.
        Ok(csv
            .split(',')
            .filter(|_| !csv.is_empty())
            .map(|t| t.parse().unwrap_or(0))
            .collect())
    }

    /// `RecoverableKeyStoreDb.getServerParams` of the agent `uid`.
    fn server_params(&self, uid: i32) -> Result<Option<Vec<u8>>, Exception> {
        let (db, path) = self.check_recovery_agent(uid)?;
        db.query_row(
            "SELECT server_params FROM recovery_service_metadata WHERE user_id=?1 AND uid=?2",
            params![uid / PER_USER_RANGE, uid],
            |r| r.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()
        .map(Option::flatten)
        .map_err(|e| db_error(&path, e))
    }

    /// `hasSystemFeature(FEATURE_SECURE_LOCK_SCREEN)`, as `systemReady`
    /// reads it.
    fn has_secure_lock_screen(&self) -> Result<bool, Exception> {
        let (state, _) = self.host.packages()?;
        Ok(state
            .system
            .features
            .iter()
            .any(|(n, version)| n == FEATURE_SECURE_LOCK_SCREEN && *version >= 0))
    }
}

impl Service for LockSettings {
    fn descriptor(&self) -> &str {
        ls::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            ls::GET_BOOLEAN => {
                let a = ls::GetBoolean::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let key = a.key.unwrap_or_default();
                self.check_database_read(uid, &key, a.user_id)
                    .and_then(|()| self.boolean(&key, a.default_value, a.user_id))
                    .map(|v| ls::write_get_boolean_reply(&mut p, v))
            }
            ls::GET_LONG => {
                let a = ls::GetLong::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let key = a.key.unwrap_or_default();
                self.check_database_read(uid, &key, a.user_id)
                    .and_then(|()| self.long(&key, a.default_value, a.user_id))
                    .map(|v| ls::write_get_long_reply(&mut p, v))
            }
            ls::GET_STRING => {
                let a = ls::GetString::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let key = a.key.unwrap_or_default();
                self.check_database_read(uid, &key, a.user_id)
                    .and_then(|()| self.string(&key, a.user_id))
                    .map(|v| ls::write_get_string_reply(&mut p, &v.or(a.default_value)))
            }
            ls::GET_CREDENTIAL_TYPE => {
                let a = ls::GetCredentialType::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.check_password_have(uid)
                    .and_then(|()| self.credential_type(a.user_id))
                    .map(|t| ls::write_get_credential_type_reply(&mut p, t))
            }
            ls::GET_SEPARATE_PROFILE_CHALLENGE_ENABLED => {
                let a = ls::GetSeparateProfileChallengeEnabled::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.check_database_read(uid, SEPARATE_PROFILE_CHALLENGE_KEY, a.user_id)
                    .and_then(|()| self.boolean(SEPARATE_PROFILE_CHALLENGE_KEY, false, a.user_id))
                    .map(|v| ls::write_get_separate_profile_challenge_enabled_reply(&mut p, v))
            }
            ls::GET_RECOVERY_SECRET_TYPES => {
                ls::GetRecoverySecretTypes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.recovery_secret_types(uid)
                    .map(|t| ls::write_get_recovery_secret_types_reply(&mut p, &Some(t)))
            }
            // `RecoverableKeyStoreManager.setRecoverySecretTypes`: the
            // types the agent already has change nothing; setting others
            // is a write (#1888).
            ls::SET_RECOVERY_SECRET_TYPES => {
                let a = ls::SetRecoverySecretTypes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.recovery_secret_types(uid).and_then(|current| {
                    let types = a
                        .secret_types
                        .ok_or_else(|| Exception::new(EX_NULL_POINTER, "secretTypes is null"))?;
                    if types != current {
                        return Err(self.host.unimplemented_named(
                            "lock_settings",
                            "setting other recovery secret types (#1888)",
                        ));
                    }
                    ls::write_set_recovery_secret_types_reply(&mut p);
                    Ok(())
                })
            }
            // `RecoverableKeyStoreManager.setServerParams`: the agent's
            // own params again change nothing; others are a write (#1888).
            ls::SET_SERVER_PARAMS => {
                let a = ls::SetServerParams::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.server_params(uid).and_then(|current| {
                    if a.server_params != current {
                        return Err(self.host.unimplemented_named(
                            "lock_settings",
                            "setting other recovery server params (#1888)",
                        ));
                    }
                    ls::write_set_server_params_reply(&mut p);
                    Ok(())
                })
            }
            // `RecoverableKeyStoreManager.setSnapshotCreatedPendingIntent`:
            // the agent's intent kept (`setSnapshotListener`).
            ls::SET_SNAPSHOT_CREATED_PENDING_INTENT => {
                let a = ls::SetSnapshotCreatedPendingIntent::<
                    crate::location::parcels::PendingIntent,
                >::read(&mut call.data)
                .map_err(|_| BAD_VALUE)?;
                self.check_recovery_agent(uid).map(|_| {
                    let mut listeners = self.snapshot_listeners.lock().unwrap();
                    match a.intent {
                        Some(crate::location::parcels::PendingIntent(Binder::Handle(h))) => {
                            listeners.insert(uid, self.host.process.strong(h));
                        }
                        _ => {
                            listeners.remove(&uid);
                        }
                    }
                    ls::write_set_snapshot_created_pending_intent_reply(&mut p);
                })
            }
            ls::HAS_SECURE_LOCK_SCREEN => {
                ls::HasSecureLockScreen::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.has_secure_lock_screen()
                    .map(|v| ls::write_has_secure_lock_screen_reply(&mut p, v))
            }
            code => {
                let name = ls::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("lock_settings", &format!("{name} (#1888)")))
            }
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values are read by user and key, as `readKeyValue` queries them.
    #[test]
    fn values_are_read_by_user_and_key() {
        let dir = std::env::temp_dir().join(format!("aim-locksettings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("locksettings.db");
        let c = Connection::open(&db).unwrap();
        c.execute_batch(
            "CREATE TABLE locksettings (_id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT, user INTEGER, value TEXT);
             INSERT INTO locksettings (name, user, value) VALUES ('sp-handle', 0, '7641559019651954907');
             INSERT INTO locksettings (name, user, value) VALUES ('sp-handle', 10, '5');",
        )
        .unwrap();
        drop(c);
        assert_eq!(
            read_key_value(&db, "sp-handle", 0).unwrap().as_deref(),
            Some("7641559019651954907")
        );
        assert_eq!(read_key_value(&db, "lockscreen.disabled", 0).unwrap(), None);
        assert_eq!(
            read_key_value(&dir.join("none.db"), "sp-handle", 0).unwrap(),
            None
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
