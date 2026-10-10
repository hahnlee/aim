//! Deferred UID namespace cleanup from AppDataHelper.clearKeystoreData (#822).
//! Ported from android-16.0.0_r1, Copyright (C) The Android Open Source
//! Project, Apache License 2.0. Requests use the original maintenance service.
use crate::system::System;
use aim_binder_host::local::LocalProcess;
use aim_service_aidl::{
    android_security_maintenance_ikeystoremaintenance as maintenance,
    android_system_keystore2_domain as domain,
};
use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

pub struct KeystoreCleanup {
    system: Arc<System>,
    pending: Mutex<VecDeque<i64>>,
}

impl KeystoreCleanup {
    pub fn new(process: Arc<LocalProcess>) -> Self {
        Self {
            system: System::new(process, &[]),
            pending: Mutex::new(VecDeque::new()),
        }
    }

    /// Capture the app ID and complete resolved user inventory when posting.
    /// Shared app IDs are cleared too, even while surviving members retain it.
    pub fn post(&self, app_id: i32, users: &[i32]) -> Result<(), String> {
        if app_id < 0 || users.is_empty() {
            return Err("keystore cleanup requires a valid app ID and resolved users".into());
        }
        let mut seen = BTreeSet::new();
        let mut namespaces = Vec::new();
        for &user in users {
            if user < 0 || !seen.insert(user) {
                return Err("keystore cleanup requires distinct nonnegative users".into());
            }
            let uid = user
                .checked_mul(100_000)
                .and_then(|base| base.checked_add(app_id % 100_000))
                .ok_or("keystore cleanup user inventory produces an out-of-range UID")?;
            namespaces.push(i64::from(uid));
        }
        self.pending.lock().unwrap().extend(namespaces);
        Ok(())
    }

    pub fn pending(&self) -> usize {
        self.pending.lock().unwrap().len()
    }

    /// Execute one namespace request on the background owner. A failed request
    /// remains at the head; successful earlier users are not repeated on retry.
    pub fn complete_next(&self) -> Result<bool, String> {
        let mut pending = self.pending.lock().unwrap();
        let Some(&nspace) = pending.front() else {
            return Ok(false);
        };
        let request = maintenance::ClearNamespace {
            domain: domain::APP as i32,
            nspace,
        };
        self.system
            .call(
                "android.security.maintenance",
                maintenance::CLEAR_NAMESPACE,
                |p| request.write(p),
                maintenance::read_clear_namespace_reply,
            )
            .map_err(|e| format!("clearNamespace UID {nspace}: {e:?}"))?;
        pending.pop_front();
        Ok(true)
    }
}
