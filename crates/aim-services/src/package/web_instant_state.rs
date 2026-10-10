//! Retained original UM/Settings web-instant-app policy, PMS systemReady.
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception, Reader};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub epoch: u64,
    pub disabled: BTreeMap<i32, bool>,
}
impl Snapshot {
    /// Original WatchedSparseBooleanArray.get(user) defaults to false.
    pub fn is_disabled(&self, user: i32) -> bool { self.disabled.get(&user).copied().unwrap_or(false) }

    pub fn read(bytes: &[u8]) -> Result<Self, String> {
        let mut reader = Reader::new(bytes, &[]);
        let bad = |error| format!("web instant policy record: {error}");
        if reader.read_i32().map_err(bad)? != 1 {
            return Err("web instant policy record version".into());
        }
        let epoch = reader.read_i64().map_err(bad)?;
        if epoch < 0 {
            return Err("web instant policy epoch is invalid".into());
        }
        let count = reader.read_i32().map_err(bad)?;
        if !(0..=100_000).contains(&count) || (epoch == 0) != (count == 0) {
            return Err("web instant policy UM inventory is invalid".into());
        }
        let mut disabled = BTreeMap::new();
        for _ in 0..count {
            let user = reader.read_i32().map_err(bad)?;
            let value = reader.read_i32().map_err(bad)?;
            if user < 0 || !matches!(value, 0 | 1) || disabled.insert(user, value != 0).is_some() {
                return Err("web instant policy user or boolean is invalid".into());
            }
        }
        if reader.remaining() != 0 {
            return Err("web instant policy trailing data".into());
        }
        Ok(Self {
            epoch: epoch as u64,
            disabled,
        })
    }
}
pub type Publish = Arc<dyn Fn(Snapshot) -> Result<(), Exception> + Send + Sync>;
struct Live {
    snapshot: Snapshot,
    failure: Option<String>,
}
pub struct Owner {
    live: Mutex<Live>,
    publishing: Mutex<()>,
    publish: Publish,
}
impl Owner {
    pub fn new(record: &[u8], publish: Publish) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            live: Mutex::new(Live {
                snapshot: Snapshot::read(record)?,
                failure: None,
            }),
            publishing: Mutex::new(()),
            publish,
        }))
    }
    /// Publisher commits the complete map into a new native capture/version.
    /// It executes without the live state mutex and must fail atomically.
    pub fn changed(&self, record: &[u8]) -> Result<(), Exception> {
        let snapshot =
            Snapshot::read(record).map_err(|message| Exception::new(EX_ILLEGAL_STATE, message))?;
        let _serial = self.publishing.lock().unwrap();
        {
            let live = self.live.lock().unwrap();
            if let Some(error) = &live.failure {
                return Err(Exception::new(EX_ILLEGAL_STATE, error));
            }
            if snapshot.epoch <= live.snapshot.epoch {
                return Err(Exception::new(
                    EX_ILLEGAL_STATE,
                    "stale web instant policy epoch",
                ));
            }
        }
        if let Err(error) = (self.publish)(snapshot.clone()) {
            self.live.lock().unwrap().failure = Some(error.message.clone());
            return Err(error);
        }
        self.live.lock().unwrap().snapshot = snapshot;
        Ok(())
    }
    pub fn snapshot(&self) -> Result<Snapshot, String> {
        let live = self.live.lock().unwrap();
        if let Some(error) = &live.failure {
            return Err(error.clone());
        }
        Ok(live.snapshot.clone())
    }
}

/// Publish this real synchronous callback node before calling the original
/// Settings leaf's start(); its reply acknowledges native generation commit.
pub struct Changed {
    pub owner: std::sync::Weak<Owner>,
}
impl aim_binder_host::local::Service for Changed {
    fn descriptor(&self) -> &str {
        aim_service_aidl::dev_aim_server_iwebinstantappschanged::DESCRIPTOR
    }
    fn transact(
        &self,
        call: &mut aim_binder_host::local::Call<'_>,
    ) -> aim_binder_host::local::Reply {
        use aim_binder_host::parcel::{BAD_VALUE, PERMISSION_DENIED, Parcel, UNKNOWN_TRANSACTION};
        use aim_service_aidl::{ReadParcelable, dev_aim_server_iwebinstantappschanged as aidl};
        if call.code != aidl::CHANGED {
            return Err(UNKNOWN_TRANSACTION);
        }
        if call.sender_euid != 1000 {
            return Err(PERMISSION_DENIED);
        }
        let args = aidl::Changed::read(&mut call.data)?;
        if call.data.remaining() != 0 {
            return Err(BAD_VALUE);
        }
        let Some(record) = args.record else {
            return Err(BAD_VALUE);
        };
        let result = self
            .owner
            .upgrade()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "web instant policy owner detached"))
            .and_then(|owner| owner.changed(&record));
        let mut reply = Parcel::new();
        match result {
            Ok(()) => reply.write_no_exception(),
            Err(error) => reply.write_exception(&error),
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{bootstrap::{ApexInventory, ScanUsers}, model, owner::usage::Usage,
        scan::SigningScan, scan_snapshot::{Store, query_state::{Capture, Context}}};
    use aim_binder_host::parcel::Parcel;

    fn record(epoch: i64, entries: &[(i32, bool)]) -> Vec<u8> {
        let mut parcel = Parcel::new();
        parcel.write_i32(1);
        parcel.write_i64(epoch);
        parcel.write_i32(entries.len() as i32);
        for &(user, disabled) in entries { parcel.write_i32(user); parcel.write_bool(disabled); }
        parcel.data().to_vec()
    }

    #[test]
    fn constructor_to_ready_web_policy_preserves_sparse_semantics_and_capture_inventory() {
        let constructor = record(0, &[]);
        let initial = Snapshot::read(&constructor).unwrap();
        assert!(!initial.is_disabled(0));
        assert!(!initial.is_disabled(10));
        assert!(Snapshot::read(&record(0, &[(0, false)])).is_err());
        assert!(Snapshot::read(&record(1, &[])).is_err());
        let mut owner = SigningScan::new(&Default::default(), &Default::default(), 36).unwrap();
        let groups = owner.identities.shared_users.keys().map(|name| (name.clone(), Default::default())).collect();
        owner.capture_legacy_permissions(&[0, 10], Default::default(), groups).unwrap();
        let orders = owner.identities.shared_users.keys().map(|name| (name.clone(), vec![])).collect();
        owner.complete_shared_processes(orders).unwrap();
        let store = Store::new_replica(owner, Usage::new(std::iter::empty::<&str>())).unwrap();
        let scan = store.capture();
        let capture = Capture::new(scan.clone(), Context {
            scan_version: scan.version(), native_domains: None, boot_classes: None, nonce: None,
            system: model::System {sdk_sandbox_package: Some(None), ..Default::default()},
            platform: Default::default(),
            users: [(0, model::User {id: 0, ..Default::default()}), (10, model::User {id: 10, ..Default::default()})].into(),
            apex_inventory: ApexInventory {packages: Some(vec![]), active: vec![]},
            scan_users: ScanUsers {users: Some([0, 10].into_iter().map(|id| crate::package::scan::User {id, pre_created: false, adb_install_disallowed: false}).collect())}, cross_user_suspensions: true,
            packages: Default::default(), retained_packages: Default::default(),
        }).unwrap();
        let seeded = capture.prepare_web_instant_policy(initial).unwrap().capture;
        let published = Arc::new(Mutex::new(seeded.clone()));
        let target = published.clone();
        let policy = Owner::new(&constructor, Arc::new(move |snapshot| {
            let mut current = target.lock().unwrap();
            let update = current.prepare_web_instant_policy(snapshot)
                .map_err(|message| Exception::new(EX_ILLEGAL_STATE, message))?;
            *current = update.capture;
            Ok(())
        })).unwrap();
        // The callback may not publish a partial or foreign live UM snapshot.
        assert!(seeded.prepare_web_instant_policy(Snapshot::read(&record(1, &[(0, true)])).unwrap()).is_err());
        assert!(seeded.prepare_web_instant_policy(Snapshot::read(&record(1, &[(0, false), (11, false)])).unwrap()).is_err());
        assert!(policy.changed(&constructor).is_err());
        assert_eq!(published.lock().unwrap().scan().version(), seeded.scan().version());
        policy.changed(&record(1, &[(0, true), (10, false)])).unwrap();
        let ready = published.lock().unwrap().clone();
        assert!(ready.scan().version() > seeded.scan().version());
        assert!(!seeded.state().system.web_instant_policy.as_ref().unwrap().is_disabled(0));
        assert!(ready.state().system.web_instant_policy.as_ref().unwrap().is_disabled(0));
        assert!(!ready.state().system.web_instant_policy.as_ref().unwrap().is_disabled(10));
        assert!(policy.changed(&record(1, &[(0, false), (10, true)])).is_err());
        assert_eq!(published.lock().unwrap().scan().version(), ready.scan().version());
        policy.changed(&record(2, &[(0, false), (10, true)])).unwrap();
        let changed = published.lock().unwrap().clone();
        assert!(changed.scan().version() > ready.scan().version());
        assert!(!changed.state().system.web_instant_policy.as_ref().unwrap().is_disabled(0));
        assert!(changed.state().system.web_instant_policy.as_ref().unwrap().is_disabled(10));
        assert_eq!(policy.snapshot().unwrap().epoch, 2);
        assert!(changed.prepare_web_instant_policy(Snapshot::read(&constructor).unwrap()).is_err());
    }
}
