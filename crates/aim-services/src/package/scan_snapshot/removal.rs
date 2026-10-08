//! Native removal commits rebase only genuine concurrent usage/user owners.
use super::{CommitError,Error,Snapshot,Store};
use crate::package::{scan::SigningScan,owner::WriteError};
use std::{collections::BTreeSet,sync::Arc};
impl Store {
    pub(crate) fn publish_removal_after(
        &self,base:&Arc<Snapshot>,targets:&BTreeSet<String>,mut owner:SigningScan,
        persist:impl FnOnce(&Arc<Snapshot>)->Result<(),WriteError>,
    )->Result<Arc<Snapshot>,CommitError>{
        let mut current=self.current.lock().unwrap();
        if let Some(reason)=super::install_context::install_user_rebase_conflict(base,&current,&mut owner,targets)
            .map_err(|error|CommitError::Snapshot(Error::Invalid(error)))? {
            return Err(CommitError::Snapshot(Error::Invalid(format!("removal base changed: {reason}"))));
        }
        let usage=current.usage().for_install(owner.settings.packages.iter().map(|package|package.name.as_str()));
        // A native-only Store does not require a replica owner. If one was
        // actually supplied, its existing runtime values still track usage.
        if self.replica || owner.has_replica_runtime() {
            owner.rebase_removal_usage(&usage).map_err(|error|CommitError::Snapshot(Error::Invalid(error)))?;
        }
        let version=current.version().checked_add(1).filter(|version|*version<=i64::MAX as u64)
            .ok_or(CommitError::Snapshot(Error::VersionExhausted))?;
        super::validate(&owner,&usage).map_err(CommitError::Snapshot)?;
        let next=Arc::new(Snapshot{version,owner,usage,replica_validated:self.replica,metadata_revision:version});
        if self.replica{super::validate_replica(&next).map_err(CommitError::Snapshot)?;}
        match persist(&next){
            Ok(())=>{*current=next.clone();Ok(next)},
            Err(error)if error.committed=>{*current=next.clone();Err(CommitError::Disk{snapshot:Some(next),error})},
            Err(error)=>Err(CommitError::Disk{snapshot:None,error}),
        }
    }
}
