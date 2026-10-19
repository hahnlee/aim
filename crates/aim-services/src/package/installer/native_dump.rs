//! PackageInstallerService's install-session sections, from retained native owners.
use super::{NativeOwners, Record, Session, Sessions};
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write,
};

struct Registry {
    active: Vec<(Session, Option<Record>)>,
    historical: Vec<(Session, Record)>,
    allocated: BTreeSet<i32>,
}
fn capture(sessions: &Sessions) -> Registry {
    let state = sessions.0.lock().unwrap();
    Registry {
        active: state
            .sessions
            .values()
            .map(|session| (session.clone(), state.records.get(&session.id).cloned()))
            .collect(),
        historical: state.historical.clone(),
        allocated: state.allocated.clone(),
    }
}
fn session(
    out: &mut String,
    value: &Session,
    record: Option<&Record>,
    stage: Option<&str>,
    status: Option<&super::super::staged_owner::Status>,
) {
    let _ = writeln!(out, "  Session {}:", value.id);
    let _ = writeln!(
        out,
        "    userId={} mOriginalInstallerUid={} mInstallerUid={} committedMillis={}",
        value.user, value.original_installer_uid, value.installer_uid, value.committed_millis
    );
    let _ = writeln!(
        out,
        "    mClientProgress={} mProgress={} mCommitted={} mSealed={} mDestroyed={} mActiveCount={} mPrepared={}",
        value.client_progress,
        value.reported_progress,
        value.committed,
        value.sealed,
        value.destroyed,
        value.active_count,
        value.prepared
    );
    let _ = writeln!(
        out,
        "    params.isMultiPackage={} params.isStaged={} mParentSessionId={} mChildSessionIds={:?}",
        value.parameters.multi_package, value.parameters.staged, value.parent, value.children
    );
    let _ = writeln!(
        out,
        "    resolvedPackageName={:?} hasAppMetadata={} installationFiles={} preVerifiedDomains={:?}",
        value.resolved_package,
        value.has_app_metadata,
        value.installation_files.len(),
        value.pre_verified_domains
    );
    if let Some(record) = record {
        let _ = writeln!(
            out,
            "    installerPackageName={:?} installInitiatingPackageName={:?} installOriginatingPackageName={:?} installerAttributionTag={:?} createdMillis={}",
            record.installer_package,
            record.initiating_package,
            record.originating_package,
            record.installer_attribution_tag,
            record.created_millis
        );
        let _ = writeln!(out, "    stageDir={stage:?}");
        let p = &record.params;
        let _ = writeln!(
            out,
            "    mode={} installFlags=0x{:x} installLocation={} installReason={} installScenario={} sizeBytes={} appPackageName={:?} appLabel={:?}",
            p.mode,
            p.install_flags,
            p.install_location,
            p.install_reason,
            p.install_scenario,
            p.size_bytes,
            p.app_package_name,
            p.app_label
        );
        let _ = writeln!(
            out,
            "    originatingUid={} abiOverride={:?} volumeUuid={:?} requireUserAction={} packageSource={} requiredInstalledVersionCode={}",
            p.originating_uid,
            p.abi_override,
            p.volume_uuid,
            p.require_user_action,
            p.package_source,
            p.required_installed_version_code
        );
    } else {
        out.push_str("    install metadata record not yet attached\n");
    }
    if let Some(status) = status {
        let _ = writeln!(
            out,
            "    mSessionApplied={} mSessionFailed={} mSessionReady={} mSessionErrorCode={} mSessionErrorMessage={:?}",
            status.applied, status.failed, status.ready, status.error_code, status.error_message
        );
    }
}
fn render(
    registry: &Registry,
    mut stage: impl FnMut(&Session, &Record) -> Result<Option<String>, Exception>,
    mut status: impl FnMut(i32) -> Result<Option<super::super::staged_owner::Status>, Exception>,
) -> Result<String, Exception> {
    let mut statuses = BTreeMap::new();
    let ids = registry
        .active
        .iter()
        .map(|(value, _)| value)
        .chain(registry.historical.iter().map(|(value, _)| value))
        .filter(|value| value.parameters.staged)
        .map(|value| value.id)
        .collect::<BTreeSet<_>>();
    for id in ids {
        statuses.insert(id, status(id)?);
    }
    let mut active = Vec::new();
    let mut orphaned = Vec::new();
    let mut finalized = Vec::new();
    for (value, record) in &registry.active {
        let root = if value.parent == -1 {
            Some(value)
        } else {
            registry
                .active
                .iter()
                .find(|(parent, _)| parent.id == value.parent)
                .map(|(parent, _)| parent)
        };
        let root_status = match root {
            Some(root) if root.parameters.staged => statuses.get(&root.id).and_then(Option::as_ref),
            _ => None,
        };
        let group = if root.is_none() {
            &mut orphaned
        } else if root_status
            .as_ref()
            .is_some_and(|state| state.applied || state.failed)
        {
            &mut finalized
        } else {
            &mut active
        };
        group.push((value, record.as_ref()));
    }
    let mut out = String::new();
    for (title, values) in [
        ("Active", active),
        ("Orphaned", orphaned),
        ("Finalized", finalized),
    ] {
        if title == "Orphaned" && values.is_empty() {
            continue;
        }
        let _ = writeln!(out, "{title} install sessions:");
        for (value, record) in values {
            let path = record
                .map(|record| stage(value, record))
                .transpose()?
                .flatten();
            let state = statuses.get(&value.id).and_then(Option::as_ref);
            session(&mut out, value, record, path.as_deref(), state);
        }
        out.push('\n');
    }
    out.push_str("Historical install sessions:\n");
    for (value, record) in &registry.historical {
        let path = stage(value, record)?;
        let state = statuses.get(&value.id).and_then(Option::as_ref);
        session(&mut out, value, Some(record), path.as_deref(), state);
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "Allocated native session IDs: {:?}",
        registry.allocated
    );
    Ok(out)
}
impl NativeOwners {
    pub(crate) fn dump_installs(&self) -> Result<String, Exception> {
        let registry = capture(&self.sessions);
        // No registry lock is held while consulting disk/staged owners.
        render(
            &registry,
            |session, record| {
                if session.parameters.multi_package {
                    return Ok(None);
                }
                self.disk
                    .lock()
                    .unwrap()
                    .validation_path(session, record)
                    .map(Some)
                    .map_err(|error| {
                        Exception::new(
                            EX_ILLEGAL_STATE,
                            format!("installer dump stage owner: {}", error.message),
                        )
                    })
            },
            |id| {
                let owner = self
                    .production_staged
                    .lock()
                    .unwrap()
                    .clone()
                    .ok_or_else(|| {
                        Exception::new(EX_ILLEGAL_STATE, "installer dump staged owner unavailable")
                    })?;
                Ok(owner.status(id))
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::{codec::SessionParams, storage};
    use super::*;
    use std::sync::Arc;
    fn record(staged: bool, multi: bool) -> Record {
        Record {
            params: SessionParams {
                mode: 1,
                staged,
                multi_package: multi,
                size_bytes: 12695,
                app_package_name: Some("fixture.installed".into()),
                app_label: Some("retained label".into()),
                ..Default::default()
            },
            installer_uid: 2000,
            user: 10,
            installer_package: Some("fixture.installer".into()),
            installer_attribution_tag: None,
            created_millis: 123456,
            initiating_package: Some("fixture.initiator".into()),
            originating_package: None,
            installer_package_uid: 2000,
        }
    }
    struct Data(std::path::PathBuf);
    impl Data {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "aim-installer-dump-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(path.join("system")).unwrap();
            Self(path)
        }
        fn store(&self) -> storage::Store {
            let inode = aim_storage::guest_inode::GuestInode {
                uid: Some(1000),
                gid: Some(1000),
                mode: Some(0o600),
            };
            storage::Store::open(
                self.0.clone(),
                inode,
                inode,
                Arc::new(|_, _| panic!("read-only dump must not label or create a stage")),
            )
            .unwrap()
        }
    }
    impl Drop for Data {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    fn dump(registry: &Registry, store: &storage::Store) -> String {
        render(
            registry,
            |session, record| {
                if session.parameters.multi_package {
                    Ok(None)
                } else {
                    store
                        .validation_path(session, record)
                        .map(Some)
                        .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.message))
                }
            },
            |_| panic!("nonstaged records must not consult staged owner"),
        )
        .unwrap()
    }
    #[test]
    fn installer_dump_retains_actual_active_history_and_absent_stage_paths() {
        let sessions = Sessions::default();
        let id = sessions.create_record(record(false, false), true).unwrap();
        sessions.prepared(id).unwrap();
        sessions.open(id, 2000).unwrap();
        sessions.progress(id, 2000, 0.25, false).unwrap();
        let retained = capture(&sessions);
        let data = Data::new();
        let store = data.store();
        let before = dump(&retained, &store);
        assert!(before.contains(&format!("Session {id}:")) && before.contains("userId=10"));
        assert!(
            before.contains("mClientProgress=0.25 mProgress=0.2")
                && before.contains("mActiveCount=1")
        );
        assert!(
            before.contains("fixture.installer")
                && before.contains("fixture.initiator")
                && before.contains("sizeBytes=12695")
        );
        assert!(before.contains(&format!("/data/app/vmdl{id}.tmp")));
        assert!(!data.0.join(format!("app/vmdl{id}.tmp")).exists());
        sessions.abandon(id, 2000).unwrap();
        sessions.finish_nonstaged(id).unwrap();
        let after = dump(&capture(&sessions), &store);
        assert!(
            !after
                .split("Historical install sessions:")
                .next()
                .unwrap()
                .contains(&format!("Session {id}:"))
        );
        assert!(
            after.contains(&format!("Historical install sessions:\n  Session {id}:"))
                && after.contains("mDestroyed=true")
        );
        assert!(after.contains(&format!("Allocated native session IDs: {{{id}}}")));
        assert_eq!(
            dump(&retained, &store),
            before,
            "retained registry view must not change after abandonment"
        );
    }
    #[test]
    fn installer_dump_classifies_finalized_family_with_one_status_capture_per_id() {
        let sessions = Sessions::default();
        let parent = sessions.create_record(record(true, true), true).unwrap();
        let child = sessions.create_record(record(true, false), true).unwrap();
        sessions.prepared(parent).unwrap();
        sessions.prepared(child).unwrap();
        sessions.add_child(parent, child, 2000).unwrap();
        let registry = capture(&sessions);
        let data = Data::new();
        let store = data.store();
        let mut read = BTreeSet::new();
        let text = render(
            &registry,
            |session, record| {
                if session.parameters.multi_package {
                    Ok(None)
                } else {
                    store
                        .validation_path(session, record)
                        .map(Some)
                        .map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.message))
                }
            },
            |id| {
                assert!(
                    read.insert(id),
                    "staged status must not be recaptured for heading and fields"
                );
                Ok(Some(super::super::super::staged_owner::Status {
                    failed: true,
                    error_code: -7,
                    error_message: Some("retained staged failure".into()),
                    ..Default::default()
                }))
            },
        )
        .unwrap();
        assert_eq!(read, BTreeSet::from([parent, child]));
        assert!(
            !text
                .split("Finalized install sessions:")
                .next()
                .unwrap()
                .contains("  Session ")
        );
        let finalized = text.split("Finalized install sessions:").nth(1).unwrap();
        assert!(
            finalized.contains(&format!("Session {parent}:"))
                && finalized.contains(&format!("Session {child}:"))
        );
        assert!(
            finalized.contains("mSessionFailed=true") && finalized.contains("mSessionErrorCode=-7")
        );
        let error = render(
            &registry,
            |_, _| panic!("failed staged owner must not render paths"),
            |_| Err(Exception::security("actual staged owner failure")),
        )
        .unwrap_err();
        assert_eq!(error.code, aim_binder_host::parcel::EX_SECURITY);
        assert_eq!(error.message, "actual staged owner failure");
    }
}
