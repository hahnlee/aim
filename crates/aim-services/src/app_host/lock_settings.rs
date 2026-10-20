//! `lock_settings`: LockSettingsService for the app host
//! (`android-16.0.0_r1`): the settings the original persisted in
//! `/data/system/locksettings.db` (`LockSettingsStorage`), and the
//! credential type of the device's lock, which is the Mac's (the owner's
//! choice, 2026-10-20, as `trust`'s `isDeviceSecure`): a password while
//! the Mac's user has one (`aim_host_lock`), none otherwise. No credential
//! is set, read or verified here. The writes, the special users (FRP,
//! repair mode), strong auth, escrow tokens and recovery are not
//! implemented (#1888).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::path::Path;
use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, Exception, Parcel,
};
use aim_service_aidl::com_android_internal_widget_ilocksettings as ls;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use super::{AppHost, reply};

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
}

impl LockSettings {
    pub fn new(host: Arc<AppHost>) -> Self {
        LockSettings { host }
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
