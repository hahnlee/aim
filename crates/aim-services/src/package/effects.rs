//! Original process/broadcast owners retained for the native package writer.
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception, Parcel, Reader};
use aim_service_aidl::dev_aim_server_ipackagemutationbridge as api;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub struct Owner {
    binder: crate::system::ServiceOwner,
    revoked: AtomicBool,
}
impl std::fmt::Debug for Owner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackageEffects")
            .field("revoked", &self.revoked.load(Ordering::Acquire))
            .finish()
    }
}
impl Owner {
    pub fn new(binder: crate::system::ServiceOwner) -> Result<Arc<Self>, Exception> {
        let code = u32::from_be_bytes(*b"_NTF");
        let reply = binder
            .transact(code, &Parcel::new(), false)
            .map_err(|status| diagnostic(transport(status), code, &binder, "transact"))?;
        let mut reader = reply.reader();
        if reader
            .read_string16()
            .map_err(|status| diagnostic(transport(status), code, &binder, "read"))?
            .as_deref()
            != Some(api::DESCRIPTOR)
            || reader.remaining() != 0
        {
            return Err(diagnostic(
                Exception::new(EX_ILLEGAL_STATE, "package effects descriptor differs"),
                code,
                &binder,
                "descriptor",
            ));
        }
        Ok(Arc::new(Self {
            binder,
            revoked: AtomicBool::new(false),
        }))
    }
    pub fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }
    fn call<T>(
        &self,
        code: u32,
        write: impl FnOnce(&mut Parcel),
        read: impl FnOnce(&mut Reader<'_>) -> Result<T, i32>,
    ) -> Result<T, Exception> {
        if self.revoked.load(Ordering::Acquire) {
            return Err(diagnostic(
                Exception::new(EX_ILLEGAL_STATE, "package effects owner revoked"),
                code,
                &self.binder,
                "admission",
            ));
        }
        let mut request = Parcel::new();
        write(&mut request);
        let reply = self
            .binder
            .transact(code, &request, false)
            .map_err(|status| diagnostic(transport(status), code, &self.binder, "transact"))?;
        let mut reader = reply.reader();
        let value = read_reply(code, &self.binder, &mut reader, read)?;
        if self.revoked.load(Ordering::Acquire) {
            return Err(diagnostic(
                Exception::new(
                    EX_ILLEGAL_STATE,
                    "package effects owner changed during callback",
                ),
                code,
                &self.binder,
                "owner_epoch",
            ));
        }
        Ok(value)
    }
    pub fn require_installer_permission(&self, uid: i32) -> Result<bool, Exception> {
        self.call(
            api::REQUIRE_INSTALLER_PERMISSION,
            |p| api::RequireInstallerPermission { calling_uid: uid }.write(p),
            |r| r.read_bool(),
        )
    }
    pub fn state_protected_nullable(
        &self,
        name: Option<&str>,
        user: i32,
    ) -> Result<bool, Exception> {
        self.call(
            api::IS_STATE_PROTECTED,
            |p| {
                api::IsStateProtected {
                    package_name: name.map(str::to_owned),
                    user_id: user,
                }
                .write(p)
            },
            |r| r.read_bool(),
        )
    }
    pub fn device_admin_nullable(&self, name: Option<&str>, user: i32) -> Result<bool, Exception> {
        self.call(
            api::IS_ACTIVE_DEVICE_ADMIN,
            |p| {
                api::IsActiveDeviceAdmin {
                    package_name: name.map(str::to_owned),
                    user_id: user,
                }
                .write(p)
            },
            |r| r.read_bool(),
        )
    }
    pub fn state_protected(&self, name: &str, user: i32) -> Result<bool, Exception> {
        self.call(
            api::IS_STATE_PROTECTED,
            |p| {
                api::IsStateProtected {
                    package_name: Some(name.into()),
                    user_id: user,
                }
                .write(p)
            },
            |r| r.read_bool(),
        )
    }
    pub fn data_protected(&self, name: &str, user: i32) -> Result<bool, Exception> {
        self.call(
            api::IS_DATA_PROTECTED,
            |p| {
                api::IsDataProtected {
                    package_name: Some(name.into()),
                    user_id: user,
                }
                .write(p)
            },
            |r| r.read_bool(),
        )
    }
    pub fn device_admin(&self, name: &str, user: i32) -> Result<bool, Exception> {
        self.call(
            api::IS_ACTIVE_DEVICE_ADMIN,
            |p| {
                api::IsActiveDeviceAdmin {
                    package_name: Some(name.into()),
                    user_id: user,
                }
                .write(p)
            },
            |r| r.read_bool(),
        )
    }
    pub fn owner_package(&self, user: i32) -> Result<Option<String>, Exception> {
        self.call(
            api::GET_OWNER_PACKAGE,
            |p| api::GetOwnerPackage { user_id: user }.write(p),
            |r| r.read_string16(),
        )
    }
    pub fn kill(
        &self,
        name: &str,
        app: i32,
        user: i32,
        reason: &str,
        exit: i32,
    ) -> Result<(), Exception> {
        self.call(
            api::KILL_PACKAGE,
            |p| {
                api::KillPackage {
                    package_name: Some(name.into()),
                    app_id: app,
                    user_id: user,
                    reason: Some(reason.into()),
                    exit_reason: exit,
                }
                .write(p)
            },
            |_| Ok(()),
        )
    }
    pub fn package_changed(
        &self,
        name: &str,
        uid: i32,
        dont_kill: bool,
        components: &[String],
        reason: Option<&str>,
        calling_uid: i32,
        delayed: bool,
    ) -> Result<(), Exception> {
        self.call(
            api::PACKAGE_CHANGED,
            |p| {
                api::PackageChanged {
                    package_name: Some(name.into()),
                    package_uid: uid,
                    dont_kill_app: dont_kill,
                    components: Some(components.iter().cloned().map(Some).collect()),
                    reason: reason.map(str::to_owned),
                    calling_uid,
                    delayed,
                }
                .write(p)
            },
            |_| Ok(()),
        )
    }
    pub fn component_label_changed(
        &self,
        name: &str,
        uid: i32,
        class: &str,
        caller: i32,
    ) -> Result<(), Exception> {
        self.call(
            api::COMPONENT_LABEL_CHANGED,
            |p| {
                api::ComponentLabelChanged {
                    package_name: Some(name.into()),
                    package_uid: uid,
                    component_class: Some(class.into()),
                    calling_uid: caller,
                }
                .write(p)
            },
            |_| Ok(()),
        )
    }
    pub fn first_launch(&self, name: &str, installer: &str, user: i32) -> Result<(), Exception> {
        self.call(
            api::FIRST_LAUNCH,
            |p| {
                api::FirstLaunch {
                    package_name: Some(name.into()),
                    installer_package: Some(installer.into()),
                    user_id: user,
                }
                .write(p)
            },
            |_| Ok(()),
        )
    }
    pub fn unhibernate(&self, name: &str, user: i32) -> Result<(), Exception> {
        self.call(
            api::UNHIBERNATE,
            |p| {
                api::Unhibernate {
                    package_name: Some(name.into()),
                    user_id: user,
                }
                .write(p)
            },
            |_| Ok(()),
        )
    }
    pub fn unstopped(&self, name: &str, user: i32) -> Result<(), Exception> {
        self.call(
            api::PACKAGE_UNSTOPPED,
            |p| {
                api::PackageUnstopped {
                    package_name: Some(name.into()),
                    user_id: user,
                }
                .write(p)
            },
            |_| Ok(()),
        )
    }
    pub fn hidden(&self, name: &str, user: i32, hidden: bool) -> Result<(), Exception> {
        self.call(
            api::PACKAGE_HIDDEN,
            |p| {
                api::PackageHidden {
                    package_name: Some(name.into()),
                    user_id: user,
                    hidden,
                }
                .write(p)
            },
            |_| Ok(()),
        )
    }
    pub fn package_added(
        &self,
        name: &str,
        user: i32,
        archived: bool,
        loader: i32,
        prediction: Option<&str>,
    ) -> Result<(), Exception> {
        self.call(
            api::PACKAGE_ADDED,
            |p| {
                api::PackageAdded {
                    package_name: Some(name.into()),
                    user_id: user,
                    archived,
                    data_loader_type: loader,
                    prediction_package: prediction.map(str::to_owned),
                }
                .write(p)
            },
            |_| Ok(()),
        )
        .map_err(|mut error| {
            error.message = format!(
                "Original packageAdded {name} user={user} (exception {}): {}",
                error.code, error.message
            );
            error
        })
    }
    pub fn capture_post_install_visibility(
        &self,
        setting: &super::settings::Package,
        users: &[i32],
    ) -> Result<(u64, Vec<i32>), Exception> {
        let values = self
            .call(
                api::CAPTURE_POST_INSTALL_VISIBILITY,
                |p| {
                    api::CapturePostInstallVisibility {
                        name: Some(setting.name.clone()),
                        app_id: setting.uid_owner_id(),
                        code_path: Some(setting.code_path.clone()),
                        version: setting.version_code,
                        users: Some(users.to_vec()),
                    }
                    .write(p)
                },
                aim_service_aidl::read_int_array,
            )?
            .ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, "original prior visibility reply null")
            })?;
        decode_visibility_capture(values)
    }
    pub fn post_install(
        &self,
        plan: &super::installer::post_install::Plan,
    ) -> Result<(), Exception> {
        self.call(
            api::POST_INSTALL_PACKAGE,
            |p| {
                api::PostInstallPackage {
                    name: Some(plan.name.clone()),
                    app_id: plan.app_id,
                    replacing: plan.replacing,
                    dont_kill: plan.dont_kill,
                    installer: plan.installer.clone(),
                    old_installer: plan.old_installer.clone(),
                    data_loader_type: plan.data_loader,
                    system: plan.system,
                    virtual_preload: plan.virtual_preload,
                    static_library: plan.static_library,
                    first_users: Some(plan.first_users.clone()),
                    first_instant_users: Some(plan.first_instant.clone()),
                    update_users: Some(plan.update_users.clone()),
                    update_instant_users: Some(plan.update_instant.clone()),
                    removed_users: Some(plan.removed_users.clone()),
                    removed_instant_users: Some(plan.removed_instant.clone()),
                    prior_visibility: Some(plan.prior_visibility.clone()),
                }
                .write(p)
            },
            |_| Ok(()),
        )
    }
    pub fn suspension_allowed(&self, user: i32, uid: i32) -> Result<bool, Exception> {
        self.call(
            api::IS_SUSPENSION_ALLOWED,
            |p| {
                api::IsSuspensionAllowed {
                    user_id: user,
                    calling_uid: uid,
                }
                .write(p)
            },
            |r| r.read_bool(),
        )
    }
    pub fn can_suspend(
        &self,
        packages: &[Option<String>],
        user: i32,
        uid: i32,
    ) -> Result<Vec<bool>, Exception> {
        self.call(
            api::CAN_SUSPEND_PACKAGES,
            |p| {
                api::CanSuspendPackages {
                    packages: Some(packages.to_vec()),
                    user_id: user,
                    calling_uid: uid,
                }
                .write(p)
            },
            |r| {
                let count = r.read_i32()?;
                if count < 0 || count as usize != packages.len() {
                    return Err(aim_binder_host::parcel::BAD_VALUE);
                }
                (0..count).map(|_| r.read_bool()).collect()
            },
        )
    }
    pub fn suspended(
        &self,
        names: &[String],
        uids: &[i32],
        user: i32,
        suspended: bool,
        quarantined: bool,
        changed_only: bool,
    ) -> Result<(), Exception> {
        self.call(
            api::PACKAGES_SUSPENDED,
            |p| {
                api::PackagesSuspended {
                    packages: Some(names.iter().cloned().map(Some).collect()),
                    uids: Some(uids.to_vec()),
                    user_id: user,
                    suspended,
                    quarantined,
                    changed_only,
                }
                .write(p)
            },
            |_| Ok(()),
        )
    }
    pub fn removed_suspensions(
        &self,
        packages: &[String],
        uids: &[i32],
        user: i32,
    ) -> Result<(), Exception> {
        self.call(
            api::REMOVED_SUSPENSIONS,
            |p| {
                api::RemovedSuspensions {
                    packages: Some(packages.iter().cloned().map(Some).collect()),
                    uids: Some(uids.to_vec()),
                    user_id: user,
                }
                .write(p)
            },
            |_| Ok(()),
        )
    }
    pub fn distraction(
        &self,
        names: &[String],
        uids: &[i32],
        user: i32,
        flags: i32,
    ) -> Result<(), Exception> {
        self.call(
            api::DISTRACTION_CHANGED,
            |p| {
                api::DistractionChanged {
                    packages: Some(names.iter().cloned().map(Some).collect()),
                    uids: Some(uids.to_vec()),
                    user_id: user,
                    flags,
                }
                .write(p)
            },
            |_| Ok(()),
        )
    }
    pub fn user_removed(&self, user: i32) -> Result<(), Exception> {
        self.call(
            api::USER_REMOVED,
            |p| api::UserRemoved { user_id: user }.write(p),
            |_| Ok(()),
        )
    }
    /// Effects run after native state publication and outside bootstrap locks.
    pub fn finish_mutation(
        &self,
        request: &super::write::mutation::Request,
        plan: &super::write::mutation::Plan,
        state: &super::model::State,
        calling_uid: i32,
    ) -> Result<(), Exception> {
        use super::write::mutation::{Change, Request};
        match request {
            Request::Enabled(setting) if !matches!(plan.change, Change::None) => {
                let package = state.packages.get(&plan.package).ok_or_else(|| {
                    Exception::new(EX_ILLEGAL_STATE, "changed package disappeared")
                })?;
                self.package_changed(
                    &plan.package,
                    super::apps_filter::uid(setting.user, package.app_id),
                    setting.flags & 1 != 0,
                    &[setting
                        .class
                        .clone()
                        .unwrap_or_else(|| plan.package.clone())],
                    None,
                    calling_uid,
                    setting.flags & 1 != 0,
                )?;
            }
            Request::Stopped {
                package,
                user,
                stopped,
            } if state.users.contains_key(user) => {
                if let Change::Stopped {
                    first_launch_installer,
                    was_stopped,
                    ..
                } = &plan.change
                {
                    if let Some(installer) = first_launch_installer {
                        self.first_launch(package, installer, *user)?;
                    }
                    if !*stopped {
                        self.unhibernate(package, *user)?;
                        if *was_stopped {
                            self.unstopped(package, *user)?;
                        }
                    }
                } else if !*stopped {
                    self.unhibernate(package, *user)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
}
fn decode_visibility_capture(values: Vec<i32>) -> Result<(u64, Vec<i32>), Exception> {
    if values.len() < 3 || values[0] != 0x56495332 {
        return Err(Exception::new(
            EX_ILLEGAL_STATE,
            "original prior visibility capture header invalid",
        ));
    }
    let metadata = ((values[1] as u32 as u64) << 32) | (values[2] as u32 as u64);
    Ok((metadata, values[3..].to_vec()))
}
#[cfg(test)]
mod visibility_capture_tests {
    use super::*;
    #[test]
    fn header_carries_metadata_generation_and_rejects_the_snapshot_version_protocol() {
        let (metadata, values) = decode_visibility_capture(vec![
            0x56495332,
            0x12345678,
            0x90abcdefu32 as i32,
            0,
            1,
            10000,
        ])
        .unwrap();
        assert_eq!(metadata, 0x1234567890abcdef);
        assert_eq!(values, [0, 1, 10000]);
        assert!(decode_visibility_capture(vec![0x56495331, 0, 7]).is_err());
    }
}
fn transport(status: i32) -> Exception {
    Exception::new(
        EX_ILLEGAL_STATE,
        format!("package effects transport: {status}"),
    )
}
fn diagnostic(
    mut error: Exception,
    code: u32,
    node: &dyn std::fmt::Display,
    phase: &str,
) -> Exception {
    error.message = format!(
        "{} [package effects tx={code} {node} phase={phase}]",
        error.message
    );
    error
}
fn read_reply<T>(
    code: u32,
    node: &dyn std::fmt::Display,
    reader: &mut Reader<'_>,
    read: impl FnOnce(&mut Reader<'_>) -> Result<T, i32>,
) -> Result<T, Exception> {
    reader
        .read_exception()
        .map_err(|status| diagnostic(transport(status), code, node, "read_exception"))?
        .map_err(|error| diagnostic(error, code, node, "read_exception"))?;
    let value = read(reader).map_err(|status| diagnostic(transport(status), code, node, "read"))?;
    if reader.remaining() != 0 {
        return Err(diagnostic(
            Exception::new(EX_ILLEGAL_STATE, "package effects reply has trailing data"),
            code,
            node,
            "trailing",
        ));
    }
    Ok(value)
}

/// The resolver rebuild uses every group referenced by each registered filter.
/// Clearing preferred choices is needed only when that effective index changes.
pub fn mime_index_changed(
    before: &super::model::State,
    after: &super::model::State,
    package: &str,
    group: Option<&str>,
) -> Result<bool, Exception> {
    let before = before
        .packages
        .get(package)
        .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "old MIME package absent"))?;
    let after = after
        .packages
        .get(package)
        .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "new MIME package absent"))?;
    let Some(code) = before.pkg.as_ref() else {
        return Ok(false);
    };
    let effective = |filter: &super::intent_filter::IntentFilter,
                     state: &super::model::PackageState|
     -> Result<_, Exception> {
        let mut filter = filter.clone();
        filter.clear_dynamic_data_types();
        for group in filter.mime_groups.clone().iter().flatten().rev() {
            let types = state
                .mime_groups
                .iter()
                .find(|(name, _)| name.as_deref() == Some(group.as_str()))
                .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "MIME group owner absent"))?;
            for ty in &types.1 {
                let ty = ty.as_deref().ok_or_else(|| {
                    Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "null MIME type")
                })?;
                // Original MimeGroupsAwareIntentResolver ignores malformed MIME
                // entries instead of registering an invalid index key.
                if filter.add_dynamic_data_type(ty).is_err() {
                    continue;
                }
            }
        }
        Ok((filter.types, filter.has_dynamic_partial_types))
    };
    for component in code
        .activities
        .iter()
        .chain(&code.receivers)
        .map(|component| &component.main.component)
        .chain(
            code.services
                .iter()
                .map(|component| &component.main.component),
        )
        .chain(
            code.providers
                .iter()
                .map(|component| &component.main.component),
        )
    {
        for intent in &component.intents {
            if intent.filter.mime_groups.as_ref().is_some_and(|groups| {
                group.is_some_and(|group| groups.iter().any(|value| value == group))
            }) && effective(&intent.filter, before)? != effective(&intent.filter, after)?
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_SERVICE_SPECIFIC, NOT_ENOUGH_DATA, UNKNOWN_TRANSACTION};

    #[test]
    fn effects_diagnostic_reply_preserves_results_and_reports_parse_stage() {
        let mut reply = Parcel::new();
        reply.write_no_exception();
        reply.write_bool(true);
        assert!(
            read_reply(
                api::REQUIRE_INSTALLER_PERMISSION,
                &"handle=17",
                &mut Reader::new(reply.data(), reply.objects()),
                |r| r.read_bool()
            )
            .unwrap()
        );
        let mut empty = Reader::new(&[], &[]);
        let error = read_reply(api::POST_INSTALL_PACKAGE, &"handle=17", &mut empty, |_| {
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.code, EX_ILLEGAL_STATE);
        assert!(
            error
                .message
                .contains(&format!("transport: {NOT_ENOUGH_DATA}"))
        );
        assert!(error.message.contains(&format!(
            "tx={} handle=17 phase=read_exception",
            api::POST_INSTALL_PACKAGE
        )));
        let mut reply = Parcel::new();
        reply.write_no_exception();
        let error = read_reply(
            api::REQUIRE_INSTALLER_PERMISSION,
            &"handle=19",
            &mut Reader::new(reply.data(), reply.objects()),
            |r| r.read_bool(),
        )
        .unwrap_err();
        assert!(error.message.contains("handle=19 phase=read]"));
        reply.write_i32(23);
        let error = read_reply(
            api::POST_INSTALL_PACKAGE,
            &"handle=19",
            &mut Reader::new(reply.data(), reply.objects()),
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(error.message.contains("phase=trailing]"));
    }

    #[test]
    fn effects_diagnostic_preserves_remote_service_and_parcelable_exceptions() {
        let mut service = Exception::new(EX_SERVICE_SPECIFIC, "original owner failure");
        service.service_specific = 71;
        let mut payload = Parcel::new();
        payload.write_string16(Some("original.OwnerException"));
        payload.write_string16(Some("opaque owner payload"));
        let parcelable =
            Exception::parcelable(Some("original parcelable failure"), &payload).unwrap();
        for expected in [service, parcelable] {
            let mut reply = Parcel::new();
            reply.write_exception(&expected);
            let error = read_reply::<()>(
                api::KILL_PACKAGE,
                &"handle=31",
                &mut Reader::new(reply.data(), reply.objects()),
                |_| panic!("failed exception must not read a return value"),
            )
            .unwrap_err();
            assert_eq!(error.code, expected.code);
            assert_eq!(error.service_specific, expected.service_specific);
            assert_eq!(error.parcelable, expected.parcelable);
            assert!(error.message.starts_with(&expected.message));
            assert!(error.message.contains(&format!(
                "tx={} handle=31 phase=read_exception",
                api::KILL_PACKAGE
            )));
        }
        let error = diagnostic(
            transport(UNKNOWN_TRANSACTION),
            api::KILL_PACKAGE,
            &"handle=31",
            "transact",
        );
        assert_eq!(error.code, EX_ILLEGAL_STATE);
        assert!(
            error
                .message
                .contains(&format!("transport: {UNKNOWN_TRANSACTION}"))
        );
        assert!(error.message.contains(&format!(
            "tx={} handle=31 phase=transact",
            api::KILL_PACKAGE
        )));
    }
}
