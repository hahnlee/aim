//! `AccountsDb` (`android-16.0.0_r1`): a user's account databases as
//! AccountManagerService keeps them, the DE database
//! (`/data/system_de/<user>/accounts_de.db`: the accounts, their grants
//! and visibility, the authenticators' uids in `meta`) with the CE
//! database (`/data/system_ce/<user>/accounts_ce.db`: passwords, auth
//! tokens and user data) attached to it as `ceDb` once the user is
//! unlocked, each query as the original's. The files are SQLite's, which
//! only SQLite reads and writes faithfully, so they go through SQLite
//! itself (rusqlite's bundled amalgamation), in the journal mode the
//! original's connections use (`db_default_journal_mode`, `TRUNCATE`).
//! The databases the original made are opened at its versions (DE 3,
//! CE 10); none is created, upgraded or migrated from pre-N here (#1504).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use super::account_service::Account;

/// `DE_DATABASE_VERSION`, `CE_DATABASE_VERSION`.
const DE_DATABASE_VERSION: i32 = 3;
const CE_DATABASE_VERSION: i32 = 10;
/// `META_KEY_FOR_AUTHENTICATOR_UID_FOR_TYPE_PREFIX`.
const META_AUTH_UID_PREFIX: &str = "auth_uid_for_type:";
/// `SELECTION_ACCOUNTS_ID_BY_ACCOUNT`.
const ACCOUNT_ID_BY_ACCOUNT: &str =
    "accounts_id=(select _id FROM accounts WHERE name=? AND type=?)";

/// A failure of the databases (Java's `SQLiteException`).
#[derive(Debug)]
pub struct DbError(pub String);

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError(e.to_string())
    }
}

type Result<T> = std::result::Result<T, DbError>;

pub struct AccountsDb {
    db: Connection,
    de: PathBuf,
    ce_attached: bool,
}

/// Opens `path` as `SQLiteOpenHelper` opens a database of `version`.
fn check_version(db: &Connection, schema: &str, path: &Path, version: i32) -> Result<()> {
    let found: i32 = db.query_row(&format!("PRAGMA {schema}.user_version"), [], |r| r.get(0))?;
    if found != version {
        return Err(DbError(format!(
            "{}: version {found}, not {version}",
            path.display()
        )));
    }
    Ok(())
}

impl AccountsDb {
    /// `AccountsDb.create` of the DE database at `de`.
    pub fn open(de: &Path) -> Result<AccountsDb> {
        if !de.exists() {
            return Err(DbError(format!(
                "{}: no database (the app host does not create one, #1504)",
                de.display()
            )));
        }
        let db = Connection::open(de)?;
        db.query_row("PRAGMA journal_mode=TRUNCATE", [], |_| Ok(()))?;
        check_version(&db, "main", de, DE_DATABASE_VERSION)?;
        Ok(AccountsDb {
            db,
            de: de.to_path_buf(),
            ce_attached: false,
        })
    }

    /// `attachCeDatabase`.
    pub fn attach_ce(&mut self, ce: &Path) -> Result<()> {
        if !ce.exists() {
            return Err(DbError(format!(
                "{}: no database (the app host does not create one, #1504)",
                ce.display()
            )));
        }
        self.db
            .execute("ATTACH DATABASE ?1 AS ceDb", [ce.to_string_lossy()])?;
        self.db
            .query_row("PRAGMA ceDb.journal_mode=TRUNCATE", [], |_| Ok(()))?;
        check_version(&self.db, "ceDb", ce, CE_DATABASE_VERSION)?;
        self.ce_attached = true;
        Ok(())
    }

    pub fn is_ce_attached(&self) -> bool {
        self.ce_attached
    }

    /// After a write: the journal SQLite made keeps the database's guest
    /// owner and mode (the system's, `rw-rw----`).
    fn wrote(&self) -> Result<()> {
        let journal = self.de.with_extension("db-journal");
        if journal.exists() {
            let inode = aim_storage::guest_inode::GuestInode {
                uid: Some(1000),
                gid: Some(1000),
                mode: Some(0o660),
            };
            aim_storage::guest_inode::record(&journal, inode)
                .map_err(|e| DbError(format!("{}: {e}", journal.display())))?;
        }
        Ok(())
    }

    /// `findMetaAuthUid`: the authenticators' uids by type, in key order.
    pub fn find_meta_auth_uid(&self) -> Result<Vec<(String, i32)>> {
        let mut q = self
            .db
            .prepare("SELECT key, value FROM meta WHERE key LIKE ? ORDER BY key")?;
        let rows = q.query_map([format!("{META_AUTH_UID_PREFIX}%")], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?;
        let mut map = Vec::new();
        for row in rows {
            let (key, value) = row?;
            // `TextUtils.split(key, ":")[1]`.
            let kind = key.split(':').nth(1).unwrap_or_default().to_string();
            let value = value.unwrap_or_default();
            if kind.is_empty() || value.is_empty() {
                eprintln!("app host: accounts: meta {key}: auth type or uid empty");
                continue;
            }
            let uid = value
                .parse()
                .map_err(|_| DbError(format!("meta {key}: uid {value}")))?;
            map.push((kind, uid));
        }
        Ok(map)
    }

    /// `insertOrReplaceMetaAuthTypeAndUid`.
    pub fn insert_or_replace_meta_auth_type_and_uid(&self, kind: &str, uid: i32) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            params![format!("{META_AUTH_UID_PREFIX}{kind}"), uid],
        )?;
        self.wrote()
    }

    /// `deleteMetaByAuthTypeAndUid`.
    pub fn delete_meta_by_auth_type_and_uid(&self, kind: &str, uid: i32) -> Result<bool> {
        let n = self.db.execute(
            "DELETE FROM meta WHERE key=?1 AND value=?2",
            params![format!("{META_AUTH_UID_PREFIX}{kind}"), uid.to_string()],
        )?;
        self.wrote()?;
        Ok(n > 0)
    }

    /// `findAllDeAccounts`: by id.
    pub fn find_all_de_accounts(&self) -> Result<Vec<(i64, Account)>> {
        let mut q = self
            .db
            .prepare("SELECT _id, type, name FROM accounts ORDER BY _id")?;
        let rows = q.query_map([], |r| Ok((r.get(0)?, Account::new(r.get(2)?, r.get(1)?))))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// `findAllVisibilityValues`: each account's packages and their
    /// visibility.
    pub fn find_all_visibility_values(&self) -> Result<HashMap<Account, HashMap<String, i32>>> {
        let mut q = self.db.prepare(
            "SELECT visibility._package, visibility.value, accounts.name, accounts.type FROM visibility \
             JOIN accounts ON accounts._id = visibility.accounts_id",
        )?;
        let rows = q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<i32>>(1)?.unwrap_or(0),
                Account::new(r.get(2)?, r.get(3)?),
            ))
        })?;
        let mut result: HashMap<Account, HashMap<String, i32>> = HashMap::new();
        for row in rows {
            let (package, visibility, account) = row?;
            result
                .entry(account)
                .or_default()
                .insert(package, visibility);
        }
        Ok(result)
    }

    /// `findDeAccountPreviousName`.
    pub fn find_de_account_previous_name(&self, account: &Account) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row(
                "SELECT previous_name FROM accounts WHERE name=? AND type=?",
                params![account.name, account.kind],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
    }

    /// `findAccountPasswordByNameAndType`.
    pub fn find_account_password_by_name_and_type(
        &self,
        name: &str,
        kind: &str,
    ) -> Result<Option<String>> {
        Ok(self
            .db
            .query_row(
                "SELECT password FROM ceDb.accounts WHERE name=? AND type=?",
                params![name, kind],
                |r| r.get(0),
            )
            .optional()?
            .flatten())
    }

    /// The CE `table`'s two columns of `account`'s rows.
    fn ce_pairs(
        &self,
        table: &str,
        columns: &str,
        account: &Account,
    ) -> Result<HashMap<String, Option<String>>> {
        let mut q = self.db.prepare(&format!(
            "SELECT {columns} FROM ceDb.{table} WHERE {ACCOUNT_ID_BY_ACCOUNT}"
        ))?;
        let rows = q.query_map(params![account.name, account.kind], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// `findUserExtrasForAccount`.
    pub fn find_user_extras_for_account(
        &self,
        account: &Account,
    ) -> Result<HashMap<String, Option<String>>> {
        self.ce_pairs("extras", "key, value", account)
    }

    /// `findAuthTokensByAccount`.
    pub fn find_auth_tokens_by_account(
        &self,
        account: &Account,
    ) -> Result<HashMap<String, Option<String>>> {
        self.ce_pairs("authtokens", "type, authtoken", account)
    }

    /// `findMatchingGrantsCountAnyToken`.
    pub fn find_matching_grants_count_any_token(&self, uid: i32, account: &Account) -> Result<i64> {
        Ok(self.db.query_row(
            "SELECT COUNT(*) FROM grants, accounts WHERE accounts_id=_id AND uid=? AND name=? AND type=?",
            params![uid.to_string(), account.name, account.kind],
            |r| r.get(0),
        )?)
    }

    /// `findAllUidGrants`.
    pub fn find_all_uid_grants(&self) -> Result<Vec<i32>> {
        let mut q = self.db.prepare("SELECT uid FROM grants GROUP BY uid")?;
        let rows = q.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// `deleteGrantsByUid`.
    pub fn delete_grants_by_uid(&self, uid: i32) -> Result<bool> {
        let n = self
            .db
            .execute("DELETE FROM grants WHERE uid=?", [uid.to_string()])?;
        self.wrote()?;
        Ok(n > 0)
    }

    /// `findCeAccountsNotInDe`.
    pub fn find_ce_accounts_not_in_de(&self) -> Result<Vec<Account>> {
        let mut q = self.db.prepare(
            "SELECT name,type FROM ceDb.accounts WHERE NOT EXISTS \
             (SELECT _id FROM accounts WHERE _id=ceDb.accounts._id )",
        )?;
        let rows = q.query_map([], |r| Ok(Account::new(r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A user's databases as `DeDatabaseHelper.onCreate` and
    /// `CeDatabaseHelper.onCreate` make them, in a directory of their own.
    fn create(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("aim-accounts-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (de, ce) = (dir.join("accounts_de.db"), dir.join("accounts_ce.db"));
        Connection::open(&de)
            .unwrap()
            .execute_batch(
                "CREATE TABLE android_metadata (locale TEXT);
                 CREATE TABLE accounts ( _id INTEGER PRIMARY KEY, name TEXT NOT NULL, type TEXT NOT NULL, previous_name TEXT, last_password_entry_time_millis_epoch INTEGER DEFAULT 0, UNIQUE(name,type));
                 CREATE TABLE meta ( key TEXT PRIMARY KEY NOT NULL, value TEXT);
                 CREATE TABLE grants (  accounts_id INTEGER NOT NULL, auth_token_type STRING NOT NULL,  uid INTEGER NOT NULL,  UNIQUE (accounts_id,auth_token_type,uid));
                 CREATE TABLE shared_accounts ( _id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, type TEXT NOT NULL, UNIQUE(name,type));
                 CREATE TRIGGER accountsDelete DELETE ON accounts BEGIN   DELETE FROM grants     WHERE accounts_id=OLD._id ; END;
                 CREATE TABLE debug_table ( _id INTEGER,action_type TEXT NOT NULL, time DATETIME,caller_uid INTEGER NOT NULL,table_name TEXT NOT NULL,primary_key INTEGER PRIMARY KEY);
                 CREATE INDEX timestamp_index ON debug_table (time);
                 CREATE TABLE visibility ( accounts_id INTEGER NOT NULL, _package TEXT NOT NULL, value INTEGER, PRIMARY KEY(accounts_id,_package));
                 CREATE TRIGGER accountsDeleteVisibility DELETE ON accounts BEGIN   DELETE FROM visibility     WHERE accounts_id=OLD._id ; END;
                 PRAGMA user_version = 3;",
            )
            .unwrap();
        Connection::open(&ce)
            .unwrap()
            .execute_batch(
                "CREATE TABLE android_metadata (locale TEXT);
                 CREATE TABLE accounts ( _id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, type TEXT NOT NULL, password TEXT, UNIQUE(name,type));
                 CREATE TABLE authtokens (  _id INTEGER PRIMARY KEY AUTOINCREMENT,  accounts_id INTEGER NOT NULL, type TEXT NOT NULL,  authtoken TEXT,  UNIQUE (accounts_id,type));
                 CREATE TABLE extras ( _id INTEGER PRIMARY KEY AUTOINCREMENT, accounts_id INTEGER, key TEXT NOT NULL, value TEXT, UNIQUE(accounts_id,key));
                 CREATE TRIGGER accountsDelete DELETE ON accounts BEGIN   DELETE FROM authtokens     WHERE accounts_id=OLD._id ;   DELETE FROM extras     WHERE accounts_id=OLD._id ; END;
                 PRAGMA user_version = 10;",
            )
            .unwrap();
        (de, ce)
    }

    /// Rows as `addAccountInternal`, `setUserData`, `setAuthToken`,
    /// `grantAppPermission` and `setAccountVisibility` write them.
    fn populate(de: &Path, ce: &Path) {
        Connection::open(de)
            .unwrap()
            .execute_batch(
                "INSERT INTO accounts (_id, name, type, previous_name) VALUES (1, 'a@example.com', 'com.example', 'old@example.com');
                 INSERT INTO accounts (_id, name, type) VALUES (2, 'b', 'org.other');
                 INSERT INTO meta VALUES ('auth_uid_for_type:com.example', '10100');
                 INSERT INTO meta VALUES ('auth_uid_for_type:org.other', '10101');
                 INSERT INTO meta VALUES ('other_key', 'x');
                 INSERT INTO grants VALUES (1, 'android.accounts.AccountManager.ACCOUNT_ACCESS_TOKEN_TYPE', 10200);
                 INSERT INTO grants VALUES (2, 'mail', 10200);
                 INSERT INTO grants VALUES (2, 'mail', 10300);
                 INSERT INTO visibility VALUES (1, 'com.app', 1);
                 INSERT INTO visibility VALUES (1, 'android:accounts:key_legacy_visible', 4);",
            )
            .unwrap();
        Connection::open(ce)
            .unwrap()
            .execute_batch(
                "INSERT INTO accounts (_id, name, type, password) VALUES (1, 'a@example.com', 'com.example', 'secret');
                 INSERT INTO accounts (_id, name, type, password) VALUES (3, 'gone', 'com.example', NULL);
                 INSERT INTO extras (accounts_id, key, value) VALUES (1, 'k', 'v');
                 INSERT INTO authtokens (accounts_id, type, authtoken) VALUES (1, 'mail', 'token');",
            )
            .unwrap();
    }

    fn account(name: &str, kind: &str) -> Account {
        Account::new(name.into(), kind.into())
    }

    #[test]
    fn reads_the_original_tables() {
        let (de, ce) = create("reads");
        populate(&de, &ce);
        let mut db = AccountsDb::open(&de).unwrap();
        assert_eq!(
            db.find_meta_auth_uid().unwrap(),
            vec![("com.example".into(), 10100), ("org.other".into(), 10101)]
        );
        let all = db.find_all_de_accounts().unwrap();
        assert_eq!(
            all,
            vec![
                (1, account("a@example.com", "com.example")),
                (2, account("b", "org.other"))
            ]
        );
        let a = account("a@example.com", "com.example");
        assert_eq!(
            db.find_de_account_previous_name(&a).unwrap().as_deref(),
            Some("old@example.com")
        );
        assert_eq!(
            db.find_de_account_previous_name(&account("b", "org.other"))
                .unwrap(),
            None
        );
        let visibility = db.find_all_visibility_values().unwrap();
        assert_eq!(visibility[&a]["com.app"], 1);
        assert_eq!(visibility[&a]["android:accounts:key_legacy_visible"], 4);
        assert_eq!(
            db.find_matching_grants_count_any_token(10200, &a).unwrap(),
            1
        );
        assert_eq!(
            db.find_matching_grants_count_any_token(10300, &a).unwrap(),
            0
        );
        assert_eq!(db.find_all_uid_grants().unwrap(), vec![10200, 10300]);
        assert!(!db.is_ce_attached());
        db.attach_ce(&ce).unwrap();
        assert_eq!(
            db.find_account_password_by_name_and_type("a@example.com", "com.example")
                .unwrap()
                .as_deref(),
            Some("secret")
        );
        assert_eq!(
            db.find_account_password_by_name_and_type("b", "org.other")
                .unwrap(),
            None
        );
        assert_eq!(
            db.find_user_extras_for_account(&a).unwrap()["k"].as_deref(),
            Some("v")
        );
        assert_eq!(
            db.find_auth_tokens_by_account(&a).unwrap()["mail"].as_deref(),
            Some("token")
        );
        assert!(
            db.find_auth_tokens_by_account(&account("b", "org.other"))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            db.find_ce_accounts_not_in_de().unwrap(),
            vec![account("gone", "com.example")]
        );
        let _ = std::fs::remove_dir_all(de.parent().unwrap());
    }

    #[test]
    fn writes_meta_and_grants_as_the_original() {
        let (de, ce) = create("writes");
        populate(&de, &ce);
        let db = AccountsDb::open(&de).unwrap();
        db.insert_or_replace_meta_auth_type_and_uid("com.example", 10111)
            .unwrap();
        db.insert_or_replace_meta_auth_type_and_uid("net.new", 10112)
            .unwrap();
        assert!(!db.delete_meta_by_auth_type_and_uid("org.other", 1).unwrap());
        assert!(
            db.delete_meta_by_auth_type_and_uid("org.other", 10101)
                .unwrap()
        );
        assert_eq!(
            db.find_meta_auth_uid().unwrap(),
            vec![("com.example".into(), 10111), ("net.new".into(), 10112)]
        );
        assert!(db.delete_grants_by_uid(10200).unwrap());
        assert!(!db.delete_grants_by_uid(10200).unwrap());
        assert_eq!(db.find_all_uid_grants().unwrap(), vec![10300]);
        let _ = std::fs::remove_dir_all(de.parent().unwrap());
    }

    #[test]
    fn only_the_original_versions_open() {
        let (de, ce) = create("versions");
        Connection::open(&ce)
            .unwrap()
            .execute_batch("PRAGMA user_version = 9;")
            .unwrap();
        let mut db = AccountsDb::open(&de).unwrap();
        assert!(db.attach_ce(&ce).is_err());
        assert!(AccountsDb::open(&de.with_file_name("none.db")).is_err());
        let _ = std::fs::remove_dir_all(de.parent().unwrap());
    }
}
