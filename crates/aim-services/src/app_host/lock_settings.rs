//! `lock_settings`: LockSettingsService for the app host
//! (`android-16.0.0_r1`): the settings the original persisted in
//! `/data/system/locksettings.db` (`LockSettingsStorage`), and the
//! credential type of the device's lock, which is the Mac's (the owner's
//! choice, 2026-10-20, as `trust`'s `isDeviceSecure`): a password while
//! the Mac's user has one (`aim_host_lock`), none otherwise. No credential
//! is set, read or verified here. Of the recoverable keystore
//! (`recoverablekeystore.db`), a recovery agent's own state: its recovery
//! service's certificates (`initRecoveryServiceWithSigFile`), secret types,
//! server params, its keys' recovery statuses and grants (keystore2). It
//! never has a snapshot to take, which needs the lock screen's knowledge
//! factor. The other writes, the special users (FRP, repair mode), strong
//! auth, escrow tokens, the agents' keys' generation, import and removal
//! (the platform key) and the rest of recovery are not implemented
//! (#1888).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::Reader;
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_SERVICE_SPECIFIC,
    Exception, Parcel,
};
use aim_service_aidl::com_android_internal_widget_ilocksettings as ls;
use aim_service_aidl::{
    ParcelValue, ReadParcelable, WriteParcelable, android_system_keystore2_ikeystoreservice as ks,
};
use rusqlite::types::FromSql;
use rusqlite::{Connection, OpenFlags, OptionalExtension, ToSql, params};

use super::recovery_certificates::{
    self as certificates, CertError, CertXml, GOOGLE_CLOUD_KEY_VAULT_SERVICE_V1_ALIAS, SigXml,
    TEST_ONLY_INSECURE_CERTIFICATE_ALIAS,
};
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
/// `RecoveryController.ERROR_*`, the recoverable keystore's
/// `ServiceSpecificException`s.
const ERROR_NO_SNAPSHOT_PENDING: i32 = 21;
const ERROR_SERVICE_INTERNAL_ERROR: i32 = 22;
const ERROR_BAD_CERTIFICATE_FORMAT: i32 = 25;
const ERROR_INVALID_CERTIFICATE: i32 = 28;
const ERROR_DOWNGRADE_CERTIFICATE: i32 = 29;
const ERROR_KEY_NOT_FOUND: i32 = 30;
/// keystore2's service, and its `ResponseCode.KEY_NOT_FOUND`.
const KEYSTORE: &str = "android.system.keystore2.IKeystoreService/default";
const KEYSTORE_KEY_NOT_FOUND: i32 = 7;
/// `ApplicationKeyStorage`'s aliases of the agents' keys in the system's
/// keystore namespace, and of their grants.
const APPLICATION_KEY_ALIAS_PREFIX: &str =
    "com.android.server.locksettings.recoverablekeystore/application/";
const APPLICATION_KEY_GRANT_PREFIX: &str = "recoverable_key:";
/// `Domain.APP`, `KeyProperties.NAMESPACE_APPLICATION`.
const DOMAIN_APP: i32 = 0;
const NAMESPACE_APPLICATION: i64 = -1;
/// `KeyPermission.USE | GET_INFO | DELETE`, a grant's access.
const GRANT_ACCESS: i32 = 0x100 | 0x4 | 0x1;

/// keystore2's `KeyDescriptor`.
struct KeyDescriptor {
    domain: i32,
    nspace: i64,
    alias: Option<String>,
    blob: Option<Vec<u8>>,
}

impl WriteParcelable for KeyDescriptor {
    /// A structured parcelable: its size, then its fields.
    fn write_to(&self, p: &mut Parcel) {
        let start = p.position();
        p.write_i32(0);
        p.write_i32(self.domain);
        p.write_i64(self.nspace);
        p.write_string16(self.alias.as_deref());
        aim_service_aidl::write_byte_array(p, self.blob.as_deref());
        p.set_i32_at(start, (p.position() - start) as i32);
    }
}

impl ReadParcelable for KeyDescriptor {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        super::display::parcelable(r, |r| {
            Ok(KeyDescriptor {
                domain: r.read_i32()?,
                nspace: r.read_i64()?,
                alias: r.read_string16()?,
                blob: aim_service_aidl::read_byte_array(r)?,
            })
        })
    }
}
/// `RecoverableKeyStoreDbContract`'s tables of an agent's state.
const METADATA: &str = "recovery_service_metadata";
const ROOT_OF_TRUST: &str = "root_of_trust";

fn service_specific(code: i32, message: &str) -> Exception {
    Exception {
        code: EX_SERVICE_SPECIFIC,
        message: message.to_string(),
        service_specific: code,
        parcelable: None,
    }
}

/// A certificate file failure as `RecoverableKeyStoreManager` reports it.
fn certificate_error(e: CertError) -> Exception {
    match e {
        CertError::Parsing(m) => service_specific(ERROR_BAD_CERTIFICATE_FORMAT, &m),
        CertError::Validation(m) => service_specific(ERROR_INVALID_CERTIFICATE, &m),
    }
}

/// `RecoverableKeyStoreDb`'s `getLong` and `getBytes` of a column of the
/// agent's entry in `table` (of `root_of_trust`, the one of `alias`):
/// none unless there is exactly one entry.
fn agent_value<T: FromSql>(
    db: &Connection,
    table: &str,
    column: &str,
    uid: i32,
    alias: Option<&str>,
) -> rusqlite::Result<Option<T>> {
    let mut query = db.prepare(&format!(
        "SELECT {column} FROM {table} WHERE user_id=?1 AND uid=?2{}",
        root_alias(alias)
    ))?;
    let mut found: Vec<Option<T>> = query
        .query_map(
            params![uid / PER_USER_RANGE, uid, alias.unwrap_or_default()],
            |r| r.get(0),
        )?
        .collect::<Result<_, _>>()?;
    Ok(match found.len() {
        1 => found.remove(0),
        _ => None,
    })
}

/// `RecoverableKeyStoreDb`'s `setLong` and `setBytes`: the agent's entry
/// created if there is none (`ensureRecoveryServiceMetadataEntryExists`,
/// `ensureRootOfTrustEntryExists`), then its column set. The rows
/// updated.
fn set_agent_value(
    db: &Connection,
    table: &str,
    column: &str,
    uid: i32,
    alias: Option<&str>,
    value: &dyn ToSql,
) -> rusqlite::Result<usize> {
    let user = uid / PER_USER_RANGE;
    match alias {
        Some(alias) => db.execute(
            &format!(
                "INSERT OR IGNORE INTO {table} (user_id, uid, root_alias) VALUES (?1, ?2, ?3)"
            ),
            params![user, uid, alias],
        )?,
        None => db.execute(
            &format!("INSERT OR IGNORE INTO {table} (user_id, uid) VALUES (?1, ?2)"),
            params![user, uid],
        )?,
    };
    db.execute(
        &format!(
            "UPDATE {table} SET {column}=?4 WHERE user_id=?1 AND uid=?2{}",
            root_alias(alias)
        ),
        params![user, uid, alias.unwrap_or_default(), value],
    )
}

/// The selection of `root_of_trust`'s entry of `alias` (?3).
fn root_alias(alias: Option<&str>) -> &'static str {
    match alias {
        Some(_) => " AND root_alias=?3",
        None => " AND ?3=''",
    }
}

/// After a change of what the agent's snapshot holds: the snapshot, if
/// there was one (`getSnapshotVersion`), must be made again
/// (`setShouldCreateSnapshot`).
fn snapshot_outdated(db: &Connection, uid: i32) -> rusqlite::Result<()> {
    if agent_value::<i64>(db, METADATA, "snapshot_version", uid, None)?.is_some() {
        set_agent_value(db, METADATA, "should_create_snapshot", uid, None, &1)?;
    }
    Ok(())
}

/// `RecoverableKeyStoreManager.initRecoveryService` of the agent `uid`
/// with its certificate file, `root` the root `alias` names and `date`
/// its validation date.
fn init_recovery_service(
    db: &Connection,
    path: &Path,
    uid: i32,
    alias: &str,
    cert_file: &[u8],
    root: &[u8],
    date: i64,
) -> Result<(), Exception> {
    // The alias becomes the agent's active root of trust even if the file
    // turns out not to be valid.
    let failed = |e| db_error(path, e);
    set_agent_value(db, METADATA, "active_root_of_trust", uid, None, &alias).map_err(failed)?;
    let cert_xml = CertXml::parse(cert_file)
        .map_err(certificate_error)?
        .map_err(|m| Exception::new(EX_ILLEGAL_ARGUMENT, m))?;
    let old: Option<i64> =
        agent_value(db, ROOT_OF_TRUST, "cert_serial", uid, Some(alias)).map_err(failed)?;
    if let Some(old) = old
        && old >= cert_xml.serial
        && alias != TEST_ONLY_INSECURE_CERTIFICATE_ALIAS
    {
        if old == cert_xml.serial {
            return Ok(());
        }
        return Err(service_specific(
            ERROR_DOWNGRADE_CERTIFICATE,
            "The cert file serial number is older than the one in database.",
        ));
    }
    // `getRandomEndpointCert`.
    // SAFETY: arc4random_uniform has no preconditions.
    let index = unsafe { libc::arc4random_uniform(cert_xml.endpoints.len() as u32) } as usize;
    let cert_path = cert_xml
        .endpoint_path(index, root, date)
        .map_err(certificate_error)?;
    let encoded = certificates::pki_path(&cert_path);
    let updated = set_agent_value(db, ROOT_OF_TRUST, "cert_path", uid, Some(alias), &encoded)
        .map_err(failed)?;
    if updated == 0 {
        return Ok(());
    }
    let mut counter_id = 0i64;
    // SAFETY: the buffer is the value's own bytes (`SecureRandom.nextLong`).
    unsafe { libc::arc4random_buf((&raw mut counter_id).cast(), size_of::<i64>()) };
    set_agent_value(
        db,
        ROOT_OF_TRUST,
        "cert_serial",
        uid,
        Some(alias),
        &cert_xml.serial,
    )
    .and_then(|_| snapshot_outdated(db, uid))
    .and_then(|()| set_agent_value(db, METADATA, "counter_id", uid, None, &counter_id))
    .map(drop)
    .map_err(failed)
}

/// `RecoverableKeyStoreDb.getRecoverySecretTypes` of the agent `uid`:
/// none if it set none.
fn secret_types(db: &Connection, path: &Path, uid: i32) -> Result<Vec<i32>, Exception> {
    let csv: Option<String> =
        agent_value(db, METADATA, "secret_types", uid, None).map_err(|e| db_error(path, e))?;
    let Some(csv) = csv.filter(|c| !c.is_empty()) else {
        return Ok(Vec::new());
    };
    // A part that is no number is logged by the original and left 0.
    Ok(csv.split(',').map(|t| t.parse().unwrap_or(0)).collect())
}

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
        let db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)
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

    /// `RecoverableKeyStoreManager.getRecoverySecretTypes` for `uid`.
    fn recovery_secret_types(&self, uid: i32) -> Result<Vec<i32>, Exception> {
        let (db, path) = self.check_recovery_agent(uid)?;
        secret_types(&db, &path, uid)
    }

    /// `RecoverableKeyStoreManager.setRecoverySecretTypes`.
    fn set_recovery_secret_types(
        &self,
        uid: i32,
        types: Option<Vec<i32>>,
    ) -> Result<(), Exception> {
        let (db, path) = self.check_recovery_agent(uid)?;
        let types = types.ok_or_else(|| Exception::new(EX_NULL_POINTER, "secretTypes is null"))?;
        let current = secret_types(&db, &path, uid)?;
        if types == current {
            return Ok(());
        }
        let csv = types
            .iter()
            .map(i32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        set_agent_value(&db, METADATA, "secret_types", uid, None, &csv)
            .and_then(|_| match current.is_empty() {
                true => Ok(()),
                false => snapshot_outdated(&db, uid),
            })
            .map_err(|e| db_error(&path, e))
    }

    /// `RecoverableKeyStoreManager.setServerParams`.
    fn set_server_params(&self, uid: i32, server_params: Option<Vec<u8>>) -> Result<(), Exception> {
        let (db, path) = self.check_recovery_agent(uid)?;
        let failed = |e| db_error(&path, e);
        let current: Option<Vec<u8>> =
            agent_value(&db, METADATA, "server_params", uid, None).map_err(failed)?;
        if server_params == current {
            return Ok(());
        }
        set_agent_value(&db, METADATA, "server_params", uid, None, &server_params)
            .and_then(|_| match current {
                None => Ok(()),
                Some(_) => snapshot_outdated(&db, uid),
            })
            .map_err(failed)
    }

    /// `RecoverableKeyStoreManager.initRecoveryServiceWithSigFile`: the
    /// signature over the certificate file verified against the root
    /// `alias` names, then `initRecoveryService`.
    fn init_recovery_service_with_sig_file(
        &self,
        uid: i32,
        alias: Option<String>,
        cert_file: Option<Vec<u8>>,
        sig_file: Option<Vec<u8>>,
    ) -> Result<(), Exception> {
        let (db, path) = self.check_recovery_agent(uid)?;
        let alias = alias
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| GOOGLE_CLOUD_KEY_VAULT_SERVICE_V1_ALIAS.to_string());
        let cert_file = cert_file
            .ok_or_else(|| Exception::new(EX_NULL_POINTER, "recoveryServiceCertFile is null"))?;
        let sig_file = sig_file
            .ok_or_else(|| Exception::new(EX_NULL_POINTER, "recoveryServiceSigFile is null"))?;
        let sig = SigXml::parse(&sig_file).map_err(certificate_error)?;
        // `getRootCertificate`; each alias it knows also passes
        // `initRecoveryService`'s `isValidRootCertificateAlias`.
        let root = certificates::root_certificate(&alias).ok_or_else(|| {
            service_specific(
                ERROR_INVALID_CERTIFICATE,
                "The provided root certificate alias is invalid",
            )
        })?;
        let date = certificates::validation_date(&alias);
        sig.verify_file_signature(&root, &cert_file, date)
            .map_err(certificate_error)?;

        init_recovery_service(&db, &path, uid, &alias, &cert_file, &root, date)
    }

    /// `RecoverableKeyStoreManager.setRecoveryStatus`
    /// (`RecoverableKeyStoreDb.setRecoveryStatus`).
    fn set_recovery_status(
        &self,
        uid: i32,
        alias: Option<String>,
        status: i32,
    ) -> Result<(), Exception> {
        let (db, path) = self.check_recovery_agent(uid)?;
        let alias = alias.ok_or_else(|| Exception::new(EX_NULL_POINTER, "alias is null"))?;
        db.execute(
            "UPDATE keys SET recovery_status=?3 WHERE uid=?1 AND alias=?2",
            params![uid, alias, status],
        )
        .map(drop)
        .map_err(|e| db_error(&path, e))
    }

    /// `RecoverableKeyStoreManager.getKey`: a grant to the agent of its
    /// key in the system's keystore namespace
    /// (`ApplicationKeyStorage.getGrantAlias`), as keystore2 makes it.
    fn key(&self, uid: i32, alias: Option<String>) -> Result<Option<String>, Exception> {
        self.check_recovery_agent(uid)?;
        let alias = alias.ok_or_else(|| Exception::new(EX_NULL_POINTER, "alias is null"))?;
        let key = KeyDescriptor {
            domain: DOMAIN_APP,
            nspace: NAMESPACE_APPLICATION,
            alias: Some(format!(
                "{APPLICATION_KEY_ALIAS_PREFIX}{}/{uid}/{alias}",
                uid / PER_USER_RANGE
            )),
            blob: None,
        };
        let mut data = Parcel::new();
        ks::Grant {
            key: Some(key),
            grantee_uid: uid,
            access_vector: GRANT_ACCESS,
        }
        .write(&mut data);
        // `KeyStore2.grant`: a failure is a `KeyStoreException` of
        // keystore2's response code, a transport failure a system error.
        let internal = |m: String| service_specific(ERROR_SERVICE_INTERNAL_ERROR, &m);
        let reply =
            super::display::transact_service(&self.host.process, KEYSTORE, ks::GRANT, &data)
                .map_err(internal)?;
        match ks::read_grant_reply::<KeyDescriptor>(&mut reply.reader())
            .map_err(|s| internal(format!("{KEYSTORE}: {s}")))?
        {
            Ok(Some(grant)) => Ok(Some(format!(
                "{APPLICATION_KEY_GRANT_PREFIX}{:016X}",
                grant.nspace
            ))),
            Ok(None) => Err(internal(format!("{KEYSTORE}: no grant"))),
            Err(e)
                if e.code == EX_SERVICE_SPECIFIC
                    && e.service_specific == KEYSTORE_KEY_NOT_FOUND =>
            {
                Err(service_specific(ERROR_KEY_NOT_FOUND, &e.message))
            }
            Err(e) => Err(internal(e.message)),
        }
    }

    /// `RecoverableKeyStoreManager.getRecoveryStatus`
    /// (`RecoverableKeyStoreDb.getStatusForAllKeys`): the status of each
    /// of the agent's keys, by alias.
    fn recovery_status(&self, uid: i32) -> Result<Vec<(ParcelValue, ParcelValue)>, Exception> {
        let (db, path) = self.check_recovery_agent(uid)?;
        let failed = |e| db_error(&path, e);
        let mut query = db
            .prepare("SELECT alias, recovery_status FROM keys WHERE uid=?1")
            .map_err(failed)?;
        let rows = query
            .query_map(params![uid], |r| {
                Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<i32>>(1)?))
            })
            .map_err(failed)?;
        let mut statuses: Vec<(ParcelValue, ParcelValue)> = Vec::new();
        for row in rows {
            let (alias, status) = row.map_err(failed)?;
            // `HashMap.put`: an alias again replaces its status.
            let alias = ParcelValue::String(alias);
            statuses.retain(|(a, _)| *a != alias);
            statuses.push((alias, ParcelValue::Integer(status.unwrap_or(0))));
        }
        Ok(statuses)
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
            ls::SET_RECOVERY_SECRET_TYPES => {
                let a = ls::SetRecoverySecretTypes::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.set_recovery_secret_types(uid, a.secret_types)
                    .map(|()| ls::write_set_recovery_secret_types_reply(&mut p))
            }
            ls::SET_SERVER_PARAMS => {
                let a = ls::SetServerParams::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.set_server_params(uid, a.server_params)
                    .map(|()| ls::write_set_server_params_reply(&mut p))
            }
            ls::INIT_RECOVERY_SERVICE_WITH_SIG_FILE => {
                let a = ls::InitRecoveryServiceWithSigFile::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.init_recovery_service_with_sig_file(
                    uid,
                    a.root_certificate_alias,
                    a.recovery_service_cert_file,
                    a.recovery_service_sig_file,
                )
                .map(|()| ls::write_init_recovery_service_with_sig_file_reply(&mut p))
            }
            // `RecoverableKeyStoreManager.getKeyChainSnapshot`: none is
            // pending, as one is made with the lock screen's knowledge
            // factor (`KeySyncTask`).
            ls::GET_KEY_CHAIN_SNAPSHOT => {
                ls::GetKeyChainSnapshot::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.check_recovery_agent(uid)
                    .and_then(|_| Err(service_specific(ERROR_NO_SNAPSHOT_PENDING, "")))
            }
            ls::SET_RECOVERY_STATUS => {
                let a = ls::SetRecoveryStatus::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.set_recovery_status(uid, a.alias, a.status)
                    .map(|()| ls::write_set_recovery_status_reply(&mut p))
            }
            ls::GET_KEY => {
                let a = ls::GetKey::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.key(uid, a.alias)
                    .map(|k| ls::write_get_key_reply(&mut p, &k))
            }
            ls::GET_RECOVERY_STATUS => {
                ls::GetRecoveryStatus::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.recovery_status(uid)
                    .map(|s| ls::write_get_recovery_status_reply(&mut p, &Some(s)))
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

    /// `RecoverableKeyStoreDbContract`'s tables of the agents' state.
    fn keystore_db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE recovery_service_metadata (_id INTEGER PRIMARY KEY,user_id INTEGER,uid INTEGER,snapshot_version INTEGER,should_create_snapshot INTEGER,active_root_of_trust TEXT,public_key BLOB,cert_path BLOB,cert_serial INTEGER,secret_types TEXT,counter_id INTEGER,server_params BLOB,UNIQUE(user_id,uid));
             CREATE TABLE root_of_trust (_id INTEGER PRIMARY KEY,user_id INTEGER,uid INTEGER,root_alias TEXT,cert_path BLOB,cert_serial INTEGER,UNIQUE(user_id,uid,root_alias));",
        )
        .unwrap();
        db
    }

    /// An agent's recovery service is recorded, kept or refused by its
    /// file's serial as `initRecoveryService` records it, here with the
    /// original's certificate test file (serial 1000), its root and a date
    /// all its certificates are valid at (`TestData`).
    #[test]
    fn a_recovery_service_is_recorded_by_its_serial() {
        const CERT_XML: &[u8] =
            include_bytes!("../../tests/fixtures/recoverable_keystore/valid-cert-file.xml");
        const ROOT: &str =
            include_str!("../../tests/fixtures/recoverable_keystore/root-ca-trusted.b64");
        const DATE_ALL_CERTS_VALID: i64 = 1_516_406_400;
        let root: String = ROOT.chars().filter(|c| !c.is_whitespace()).collect();
        let root = aim_android_xml::base64::decode(&root).unwrap();
        let db = keystore_db();
        let (uid, path) = (10_150, Path::new("db"));
        let alias = GOOGLE_CLOUD_KEY_VAULT_SERVICE_V1_ALIAS;
        let init = |db: &Connection, alias| {
            init_recovery_service(db, path, uid, alias, CERT_XML, &root, DATE_ALL_CERTS_VALID)
        };
        let value = |column, alias| agent_value::<i64>(&db, ROOT_OF_TRUST, column, uid, alias);
        let counter = || agent_value::<i64>(&db, METADATA, "counter_id", uid, None).unwrap();
        set_agent_value(&db, METADATA, "snapshot_version", uid, None, &3).unwrap();

        init(&db, alias).unwrap();
        assert_eq!(value("cert_serial", Some(alias)).unwrap(), Some(1000));
        let cert_path: Vec<u8> = agent_value(&db, ROOT_OF_TRUST, "cert_path", uid, Some(alias))
            .unwrap()
            .unwrap();
        let cert_xml = CertXml::parse(CERT_XML).unwrap().unwrap();
        assert!(cert_xml.endpoints.iter().any(|e| cert_path.ends_with(e)));
        assert_eq!(
            agent_value::<String>(&db, METADATA, "active_root_of_trust", uid, None).unwrap(),
            Some(alias.to_string())
        );
        assert_eq!(
            agent_value::<i64>(&db, METADATA, "should_create_snapshot", uid, None).unwrap(),
            Some(1)
        );
        let counter_id = counter();
        assert!(counter_id.is_some());

        // The same serial again changes nothing.
        init(&db, alias).unwrap();
        assert_eq!(counter(), counter_id);

        // An older file is refused, but for the test-only root.
        set_agent_value(&db, ROOT_OF_TRUST, "cert_serial", uid, Some(alias), &1001).unwrap();
        let e = init(&db, alias).unwrap_err();
        assert_eq!(
            (e.code, e.service_specific),
            (EX_SERVICE_SPECIFIC, ERROR_DOWNGRADE_CERTIFICATE)
        );
        let test_only = TEST_ONLY_INSECURE_CERTIFICATE_ALIAS;
        set_agent_value(
            &db,
            ROOT_OF_TRUST,
            "cert_serial",
            uid,
            Some(test_only),
            &1001,
        )
        .unwrap();
        init(&db, test_only).unwrap();
        assert_eq!(value("cert_serial", Some(test_only)).unwrap(), Some(1000));
    }

    /// An agent's values are written to its entry, created first, as
    /// `RecoverableKeyStoreDb` writes them, a root of trust's per alias.
    #[test]
    fn agent_values_are_written_as_the_keystore_db_writes_them() {
        let db = keystore_db();
        let uid = 1_010_150;
        assert_eq!(
            agent_value::<String>(&db, METADATA, "secret_types", uid, None).unwrap(),
            None
        );
        set_agent_value(&db, METADATA, "secret_types", uid, None, &"100").unwrap();
        assert_eq!(secret_types(&db, Path::new("db"), uid).unwrap(), vec![100]);
        let user: i32 = db
            .query_row("SELECT user_id FROM recovery_service_metadata", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(user, 10);

        // No snapshot was made: none is outdated.
        snapshot_outdated(&db, uid).unwrap();
        assert_eq!(
            agent_value::<i64>(&db, METADATA, "should_create_snapshot", uid, None).unwrap(),
            None
        );
        set_agent_value(&db, METADATA, "snapshot_version", uid, None, &1).unwrap();
        snapshot_outdated(&db, uid).unwrap();
        assert_eq!(
            agent_value::<i64>(&db, METADATA, "should_create_snapshot", uid, None).unwrap(),
            Some(1)
        );

        let alias = Some(GOOGLE_CLOUD_KEY_VAULT_SERVICE_V1_ALIAS);
        set_agent_value(&db, ROOT_OF_TRUST, "cert_serial", uid, alias, &10_020i64).unwrap();
        set_agent_value(&db, ROOT_OF_TRUST, "cert_serial", uid, Some("other"), &1i64).unwrap();
        assert_eq!(
            agent_value::<i64>(&db, ROOT_OF_TRUST, "cert_serial", uid, alias).unwrap(),
            Some(10_020)
        );
        assert_eq!(
            agent_value::<i64>(&db, ROOT_OF_TRUST, "cert_serial", uid, Some("none")).unwrap(),
            None
        );
    }
}
