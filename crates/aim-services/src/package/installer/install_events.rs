//! Actual native publication receipts drive package monitor delivery.
use super::pipeline::{Environment, Failure, PublishedInstall, Reservation, VerifiedCode};
use crate::package::{scan_snapshot::Snapshot, write::Apks};
use aim_binder_host::parcel::Exception;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};
struct Publication {
    before: Arc<Snapshot>,
    after: Arc<Snapshot>,
}
/// Delivery follows the retained committed target, while unrelated published
/// state may advance. Never accept a newer replacement of that target's code.
pub(crate) fn validate_monitor_publication(
    committed:&Snapshot,current:&Snapshot,receipt:&PublishedInstall,
)->Result<(),String>{
    if receipt.generation!=committed.version()||current.version()<receipt.generation{
        return Err(format!("install monitor generation differs: receipt={}, committed={}, current={}",receipt.generation,committed.version(),current.version()));
    }
    for target in &receipt.packages{
        let original=committed.owner().settings.packages.iter().find(|package|package.name==target.name)
            .ok_or_else(||format!("install monitor committed target unavailable: {}",target.name))?;
        let latest=current.owner().settings.packages.iter().find(|package|package.name==target.name)
            .ok_or_else(||format!("install monitor current target unavailable: {}",target.name))?;
        if original.app_id!=latest.app_id||original.shared_user!=latest.shared_user||original.code_path!=latest.code_path
            ||original.version_code!=target.version_code||latest.version_code!=target.version_code
            ||original.signatures!=latest.signatures {
            return Err(format!("install monitor target identity differs: {} committed={:?} current={:?}",target.name,
                (original.app_id,&original.code_path,original.version_code),(latest.app_id,&latest.code_path,latest.version_code)));
        }
        let code=committed.owner().loaded_packages().get(&target.name).ok_or("install monitor committed code unavailable")?;
        let live=current.owner().loaded_packages().get(&target.name).ok_or("install monitor current code unavailable")?;
        if code.package!=live.package||code.collected_signing!=live.collected_signing{
            return Err(format!("install monitor current parsed code differs: {}",target.name));
        }
        let user=i32::try_from(target.user).map_err(|_|"install monitor user exceeds Android range")?;
        if !committed.owner().scanned_user_states(&target.name).and_then(|users|users.get(&user)).is_some_and(|state|state.installed)
            ||!current.owner().scanned_user_states(&target.name).and_then(|users|users.get(&user)).is_some_and(|state|state.installed){
            return Err(format!("install monitor target user is not installed: {} user={user}",target.name));
        }
    }
    Ok(())
}
/// Wraps a real install environment: admission, runtime preparation, filesystem
/// ownership and IntentSender status remain its mandatory concrete operations.
pub struct WithEvents {
    inner: Arc<dyn Environment>,
    system: Weak<crate::system::System>,
    bridge: Arc<crate::package::bootstrap::Bridge>,
    published: Arc<Mutex<BTreeMap<u64, Publication>>>,
}
impl WithEvents {
    pub fn new(
        inner: Arc<dyn Environment>,
        system: &Arc<crate::system::System>,
        bridge: Arc<crate::package::bootstrap::Bridge>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            system: Arc::downgrade(system),
            bridge,
            published: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }
}
struct Tracked {
    inner: Box<dyn Reservation>,
    before: Arc<Snapshot>,
    published: Arc<Mutex<BTreeMap<u64, Publication>>>,
}
impl Reservation for Tracked {
    fn requests(&self) -> Result<Vec<crate::package::scan::live_install::Request>, Failure> {
        self.inner.requests()
    }
    fn users(&self) -> &[crate::package::scan::User] {
        self.inner.users()
    }
    fn build_debuggable(&self) -> bool {
        self.inner.build_debuggable()
    }
    fn complete_metadata(
        &self,
        admission: crate::package::scan::live_install::Admission,
        apks: &Apks,
    ) -> Result<crate::package::scan::live_install::CompletedAdmission, Failure> {
        self.inner.complete_metadata(admission, apks)
    }
    fn prepare_runtime(
        &self,
        admission: &mut crate::package::scan::live_install::CompletedAdmission,
    ) -> Result<(), Failure> {
        self.inner.prepare_runtime(admission)
    }
    fn cross_user_suspensions(&self) -> bool {
        self.inner.cross_user_suspensions()
    }
    fn mark_committed(&self, snapshot: &Arc<Snapshot>) {
        self.inner.mark_committed(snapshot);
    }
    fn publication_finished(&self, snapshot: &Arc<Snapshot>) -> Result<(), Exception> {
        self.inner.publication_finished(snapshot)?;
        self.published.lock().unwrap().insert(
            snapshot.version(),
            Publication {
                before: self.before.clone(),
                after: snapshot.clone(),
            },
        );
        Ok(())
    }
}
impl Environment for WithEvents {
    fn reserve(
        &self,
        code: Vec<VerifiedCode>,
        base: &Arc<Snapshot>,
    ) -> Result<Box<dyn Reservation>, Failure> {
        let inner = self.inner.reserve(code, base)?;
        Ok(Box::new(Tracked {
            inner,
            before: base.clone(),
            published: self.published.clone(),
        }))
    }
    fn publish_queries(&self, snapshot: &Arc<Snapshot>) -> Result<(), String> {
        self.inner.publish_queries(snapshot)
    }
    fn finish(&self, receipt: PublishedInstall) -> Result<(), Exception> {
        let publication = self
            .published
            .lock()
            .unwrap()
            .remove(&receipt.generation)
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "install event publication receipt unavailable",
                )
            })?;
        let system = self.system.upgrade().ok_or_else(|| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "install event system owner unavailable",
            )
        })?;
        system.package_install_monitor_events(
            &self.bridge,
            &publication.before,
            &publication.after,
            &receipt,
        ).map_err(|mut error| {
            error.message = format!("Package monitor delivery (exception {}): {}", error.code, error.message);
            error
        })?;
        self.inner.finish(receipt).map_err(|mut error| {
            error.message = format!("Installed package completion (exception {}): {}", error.code, error.message);
            error
        })
    }
}
