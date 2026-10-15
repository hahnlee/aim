//! The verifier resumes the same commit owner, including status and stage cleanup.
use super::*;
impl NativeOwners {
    pub fn complete_native_package_verification(
        &self,
        id: i32,
        allowed: bool,
    ) -> Result<(), Exception> {
        let session = self.sessions.snapshot(id)?;
        if session.destroyed || !session.sealed || !session.committed || session.parent != -1 {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                "Verified installation root is no longer sealed",
            ));
        }
        if allowed {
            self.run_install(id)
        } else {
            self.deliver_install(id, -22, Some("Package verification failed".into()))?;
            let ids = self.sessions.abandon(id, 1000)?;
            self.abandon_stage(&ids)?;
            self.receivers.lock().unwrap().remove(&id);
            Ok(())
        }
    }
}

impl NativeOwners {
    pub fn historical_session_infos(
        &self,
        user: i32,
        uid: u32,
    ) -> Result<Vec<crate::package::installer::codec::SessionInfo>, Exception> {
        use crate::package::installer::endpoint::Owners;
        let mut result = Vec::new();
        for (session, record) in self.sessions.historical_records() {
            if user != -1 && session.user as i32 != user {
                continue;
            }
            let package = session
                .resolved_package
                .as_deref()
                .or(record.params.app_package_name.as_deref());
            if uid != session.installer_uid && !self.can_query(uid, package)? {
                continue;
            }
            let full = record.info(&session, false, uid);
            result.push(crate::package::installer::codec::SessionInfo {
                session_id: full.session_id,
                user_id: full.user_id,
                installer_package_name: full.installer_package_name,
                installer_attribution_tag: full.installer_attribution_tag,
                progress: full.progress,
                sealed: full.sealed,
                committed: full.committed,
                parent_session_id: full.parent_session_id,
                child_session_ids: full.child_session_ids,
                session_applied: full.session_applied,
                session_ready: full.session_ready,
                session_failed: full.session_failed,
                session_error_code: full.session_error_code,
                session_error_message: full.session_error_message,
                created_millis: full.created_millis,
                preapproval_requested: full.preapproval_requested,
                installer_uid: full.installer_uid,
                app_package_name: full.app_package_name,
                ..Default::default()
            });
        }
        Ok(result)
    }
}

impl NativeOwners {
    pub fn record_finished_install_history(&self, id: i32) -> Result<(), Exception> {
        self.sessions.record_historical(id)
    }
}

impl NativeOwners {
    pub fn internal_pending_install_session(&self, id: i32) -> Result<super::Session, Exception> {
        self.sessions.snapshot(id)
    }
}

impl NativeOwners {
    pub fn restore_native_staged_on_boot(
        &self,
        leaf: &aim_binder_host::local::Strong,
    ) -> Result<(), Exception> {
        use aim_service_aidl::dev_aim_server_ipackagebootlifecycleleaf as api;
        let staged = self
            .production_staged
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "Boot staged owner unavailable"))?;
        let records = self.sessions.records();
        for (root, _) in &records {
            if !root.parameters.staged || root.parent != -1 || root.destroyed {
                continue;
            }
            let status = staged.status(root.id).ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "Recovered staged status unavailable")
            })?;
            if !status.ready || status.applied || status.failed {
                continue;
            }
            let ids: Vec<_> = if root.parameters.multi_package {
                root.children.iter().copied().collect()
            } else {
                vec![root.id]
            };
            let mut apk = Vec::new();
            let mut has_apex = false;
            for id in ids {
                let (session, record) = records
                    .iter()
                    .find(|(session, _)| session.id == id)
                    .ok_or_else(|| {
                        Exception::new(EX_ILLEGAL_STATE, "Recovered staged child absent")
                    })?;
                if session.parameters.install_flags & 0x20000 != 0 {
                    has_apex = true;
                    continue;
                }
                let path = self
                    .disk
                    .lock()
                    .unwrap()
                    .validation_path(session, record)
                    .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.message))?;
                apk.push((session.clone(), record.clone(), path));
            }
            if has_apex {
                let mut request = aim_binder_host::parcel::Parcel::new();
                api::ApexSessionState {
                    session_id: root.id,
                }
                .write(&mut request);
                let reply = leaf
                    .transact(api::APEX_SESSION_STATE, &request, false)
                    .map_err(|status| {
                        Exception::new(
                            EX_ILLEGAL_STATE,
                            format!("Staged APEX boot state: {status}"),
                        )
                    })?;
                let state = api::read_apex_session_state_reply(&mut reply.reader()).map_err(
                    |status| {
                        Exception::new(
                            EX_ILLEGAL_STATE,
                            format!("Staged APEX state reply: {status}"),
                        )
                    },
                )??;
                if state != 1 {
                    return Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        format!("Staged APEX {} not activated: {state}", root.id),
                    ));
                }
            }
            if !apk.is_empty() {
                let config = self.install_config.lock().unwrap().clone().ok_or_else(|| {
                    Exception::new(EX_ILLEGAL_STATE, "Boot APK install owner unavailable")
                })?;
                let lite = self.lite_policy.lock().unwrap().clone().ok_or_else(|| {
                    Exception::new(EX_ILLEGAL_STATE, "Boot APK lite owner unavailable")
                })?;
                let code =
                    crate::package::installer::pipeline::verify_batch(&config.apks, apk, &lite()?)
                        .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.message))?;
                crate::package::installer::pipeline::install(
                    config.owners.as_ref(),
                    code,
                    &self.source,
                )
                .map_err(|error| match error {
                    crate::package::installer::pipeline::Error::Pending => {
                        Exception::new(EX_ILLEGAL_STATE, "Boot staged APK unexpectedly pending")
                    }
                    crate::package::installer::pipeline::Error::Install(error) => {
                        Exception::new(EX_ILLEGAL_STATE, error.message)
                    }
                    crate::package::installer::pipeline::Error::Owner { error, .. } => error,
                })?;
            }
            staged.applied(root.id, has_apex)?;
            if let Some(staging) = self.staging.lock().unwrap().as_ref() {
                staging.remove(root.id)?;
            }
            self.record_finished_install_history(root.id)?;
        }
        Ok(())
    }
}
