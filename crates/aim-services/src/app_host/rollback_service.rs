//! `rollback`: RollbackManagerService for the app host
//! (`android-16.0.0_r1`): the rollbacks `RollbackStore` persisted under
//! `/data/rollback`, of which `getAvailableRollbacks` and
//! `getRecentlyCommittedRollbacks` answer those available or committed,
//! with MANAGE_ROLLBACKS or TEST_MANAGE_ROLLBACKS. With none persisted
//! both are empty. Persisted rollbacks need the service's processing at
//! start (deleted when the build fingerprint changed, expired by their
//! lifetime and by their packages' changes), which is not implemented,
//! nor are the writes (#1910).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_content_rollback_irollbackmanager as rm;

use super::{AppHost, reply};
use crate::package::list_slice::Slice;

const MANAGE_ROLLBACKS: &str = "android.permission.MANAGE_ROLLBACKS";
const TEST_MANAGE_ROLLBACKS: &str = "android.permission.TEST_MANAGE_ROLLBACKS";
const ROLLBACK_INFO_CLASS: &str = "android.content.rollback.RollbackInfo";

/// A `RollbackInfo`, of which none is answered.
enum NoRollback {}

impl WriteParcelable for NoRollback {
    fn write_to(&self, _: &mut Parcel) {
        match *self {}
    }
}

pub struct Rollback {
    host: Arc<AppHost>,
}

impl Rollback {
    pub fn new(host: Arc<AppHost>) -> Self {
        Rollback { host }
    }

    /// `enforceManageRollbacks`.
    fn enforce_manage(&self, uid: i32, method: &str) -> Result<(), Exception> {
        if self.host.check_permission(Some(MANAGE_ROLLBACKS), uid)? != 0
            && self
                .host
                .check_permission(Some(TEST_MANAGE_ROLLBACKS), uid)?
                != 0
        {
            return Err(Exception::security(format!(
                "{method} requires {MANAGE_ROLLBACKS} or {TEST_MANAGE_ROLLBACKS}"
            )));
        }
        Ok(())
    }

    /// `mRollbacks` as `RollbackStore.loadRollbacks` reads it: each
    /// directory under `/data/rollback` is a rollback. With none, the list
    /// is empty; with some, the start's processing is not implemented.
    fn rollbacks(&self) -> Result<ParcelList, Exception> {
        let dir = self.host.data()?.join("rollback");
        let persisted = match std::fs::read_dir(&dir) {
            Ok(entries) => entries.flatten().any(|e| e.path().is_dir()),
            // "Folder doesn't exist": none.
            Err(_) => false,
        };
        if persisted {
            return Err(self
                .host
                .unimplemented_named("rollback", "persisted rollbacks (#1910)"));
        }
        Ok(Slice::new(
            self.host.process.clone(),
            ROLLBACK_INFO_CLASS,
            Vec::new(),
        ))
    }
}

type ParcelList = Slice<NoRollback>;

impl Service for Rollback {
    fn descriptor(&self) -> &str {
        rm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            rm::GET_AVAILABLE_ROLLBACKS => {
                rm::GetAvailableRollbacks::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce_manage(uid, "getAvailableRollbacks")
                    .and_then(|()| self.rollbacks())
                    .map(|list| rm::write_get_available_rollbacks_reply(&mut p, Some(&list)))
            }
            rm::GET_RECENTLY_COMMITTED_ROLLBACKS => {
                rm::GetRecentlyCommittedRollbacks::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce_manage(uid, "getRecentlyCommittedRollbacks")
                    .and_then(|()| self.rollbacks())
                    .map(|list| {
                        rm::write_get_recently_committed_rollbacks_reply(&mut p, Some(&list))
                    })
            }
            code => {
                let name = rm::METHODS
                    .iter()
                    .find(|(c, _)| *c == code)
                    .map_or("an unknown call", |(_, n)| n);
                Err(self
                    .host
                    .unimplemented_named("rollback", &format!("{name} (#1910)")))
            }
        };
        reply(result.map(|()| p))
    }
}
