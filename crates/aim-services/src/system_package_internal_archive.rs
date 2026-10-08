//! Original PackageArchiver setting/session operations over the native owners.
use super::*;
use crate::package::installer::{codec::SessionInfo, endpoint::Owners};
fn archive_error(message:impl Into<String>)->Exception {Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE,message)}
impl System {
    pub(crate) fn internal_clear_archive_state(&self,name:Option<String>,user:i32,_uid:i32,_pid:i32)->Result<()> {
        let Some(name)=name else{return Ok(());};
        let controller=self.public_package_removal()?.controller.clone();
        let _gate=controller.gate.lock().unwrap();
        let base=controller.store.snapshots.capture();
        let Some(mut state)=base.owner().scanned_user_states(&name).and_then(|states|states.get(&user)).cloned() else{return Ok(());};
        if state.archive_state.is_none(){return Ok(());}
        state.archive_state=None;
        let written = controller.store.user_state(&name,user,state);
        drop(_gate);
        controller.store.finish_after_unlock(base.version(), written)?;
        // User-state publication completes before the original icons-directory cleanup.
        let capture=self.capture_package_queries()?;
        let owner=capture.state().system.archive_owner.as_ref()
            .ok_or_else(||archive_error("Native archive icon owner unavailable"))?;
        if let Err(error)=owner.clear_icons(&name,user) {
            // Original FileUtils.deleteContentsAndDir reports its error by logging.
            eprintln!("Failed to clean up archive files for {name}: {}",error.message);
        }
        Ok(())
    }
    pub(crate) fn internal_active_unarchive_session(&self,name:Option<String>,user:i32,uid:i32,_pid:i32)->Result<Option<SessionInfo>> {
        let capture=self.capture_package_queries()?;
        let resolver=crate::package::resolve::Resolver::default();
        let resolution=resolver.resolution(capture.state()).map_err(|error|archive_error(format!("Unarchive sessions visibility: {error:?}")))?;
        let query=crate::package::query::Query{state:capture.state(),filter:&resolution.apps_filter,calling_uid:uid};
        self.public_package_removal()?.controller.cross_user(&query,uid as u32,user,"getAllSessions")?;
        let owner=self.package_installer_native_owner()?;
        for (session,record) in owner.sessions.records() {
            if session.user as i32!=user || session.active_count<=0 || record.params.install_flags & (1<<30)==0 {continue;}
            let package=session.resolved_package.as_deref().or(record.params.app_package_name.as_deref());
            if package!=name.as_deref(){continue;}
            if uid as u32!=session.installer_uid&&!owner.can_query(uid as u32,package)?{continue;}
            let mut info=record.info(&session,true,uid as u32);
            if !owner.can_read_paths(uid as u32)?{info.resolved_base_code_path=None;}
            if session.parameters.staged {
                if let Some(state)=owner.staged_status(session.id)? {
                    info.session_ready=state.ready;info.session_applied=state.applied;info.session_failed=state.failed;
                    info.session_error_code=state.error_code;info.session_error_message=state.error_message;
                }
            }
            return Ok(Some(info));
        }
        Ok(None)
    }
}
