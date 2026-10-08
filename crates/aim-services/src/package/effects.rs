//! Original process/broadcast owners retained for the native package writer.
use aim_binder_host::{local::Strong, parcel::{Exception, Parcel, Reader, EX_ILLEGAL_STATE}};
use aim_service_aidl::dev_aim_server_ipackagemutationbridge as api;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

pub struct Owner { binder: Strong, revoked: AtomicBool }
impl std::fmt::Debug for Owner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("PackageEffects").field("revoked", &self.revoked.load(Ordering::Acquire)).finish() }
}
impl Owner {
    pub fn new(binder: Strong) -> Result<Arc<Self>, Exception> {
        let reply = binder.transact(u32::from_be_bytes(*b"_NTF"), &Parcel::new(), false).map_err(transport)?;
        let mut reader = reply.reader();
        if reader.read_string16().map_err(transport)?.as_deref() != Some(api::DESCRIPTOR) || reader.remaining() != 0 {
            return Err(Exception::new(EX_ILLEGAL_STATE, "package effects descriptor differs"));
        }
        Ok(Arc::new(Self { binder, revoked: AtomicBool::new(false) }))
    }
    pub fn revoke(&self) { self.revoked.store(true, Ordering::Release); }
    fn call<T>(&self, code: u32, write: impl FnOnce(&mut Parcel), read: impl FnOnce(&mut Reader<'_>) -> Result<T, i32>) -> Result<T, Exception> {
        if self.revoked.load(Ordering::Acquire) { return Err(Exception::new(EX_ILLEGAL_STATE, "package effects owner revoked")); }
        let mut request = Parcel::new(); write(&mut request);
        let reply = self.binder.transact(code, &request, false).map_err(transport)?;
        let mut reader = reply.reader(); reader.read_exception().map_err(transport)??;
        let value = read(&mut reader).map_err(transport)?;
        if reader.remaining() != 0 { return Err(Exception::new(EX_ILLEGAL_STATE, "package effects reply has trailing data")); }
        if self.revoked.load(Ordering::Acquire) { return Err(Exception::new(EX_ILLEGAL_STATE, "package effects owner changed during callback")); }
        Ok(value)
    }
    pub fn require_installer_permission(&self, uid: i32) -> Result<bool, Exception> {
        self.call(api::REQUIRE_INSTALLER_PERMISSION, |p| api::RequireInstallerPermission { calling_uid: uid }.write(p), |r| r.read_bool())
    }
    pub fn state_protected_nullable(&self, name: Option<&str>, user: i32) -> Result<bool, Exception> {
        self.call(api::IS_STATE_PROTECTED, |p| api::IsStateProtected { package_name: name.map(str::to_owned), user_id: user }.write(p), |r| r.read_bool())
    }
    pub fn device_admin_nullable(&self, name: Option<&str>, user: i32) -> Result<bool, Exception> {
        self.call(api::IS_ACTIVE_DEVICE_ADMIN, |p| api::IsActiveDeviceAdmin { package_name: name.map(str::to_owned), user_id: user }.write(p), |r| r.read_bool())
    }
    pub fn state_protected(&self, name: &str, user: i32) -> Result<bool, Exception> {
        self.call(api::IS_STATE_PROTECTED, |p| api::IsStateProtected { package_name: Some(name.into()), user_id: user }.write(p), |r| r.read_bool())
    }
    pub fn data_protected(&self, name: &str, user: i32) -> Result<bool, Exception> {
        self.call(api::IS_DATA_PROTECTED, |p| api::IsDataProtected { package_name: Some(name.into()), user_id: user }.write(p), |r| r.read_bool())
    }
    pub fn device_admin(&self, name: &str, user: i32) -> Result<bool, Exception> {
        self.call(api::IS_ACTIVE_DEVICE_ADMIN, |p| api::IsActiveDeviceAdmin { package_name: Some(name.into()), user_id: user }.write(p), |r| r.read_bool())
    }
    pub fn owner_package(&self, user: i32) -> Result<Option<String>, Exception> {
        self.call(api::GET_OWNER_PACKAGE, |p| api::GetOwnerPackage { user_id: user }.write(p), |r| r.read_string16())
    }
    pub fn kill(&self, name: &str, app: i32, user: i32, reason: &str, exit: i32) -> Result<(), Exception> {
        self.call(api::KILL_PACKAGE, |p| api::KillPackage { package_name: Some(name.into()), app_id: app, user_id: user, reason: Some(reason.into()), exit_reason: exit }.write(p), |_| Ok(()))
    }
    pub fn package_changed(&self, name: &str, uid: i32, dont_kill: bool, components: &[String], reason: Option<&str>, calling_uid: i32, delayed: bool) -> Result<(), Exception> {
        self.call(api::PACKAGE_CHANGED, |p| api::PackageChanged { package_name: Some(name.into()), package_uid: uid, dont_kill_app: dont_kill, components: Some(components.iter().cloned().map(Some).collect()), reason: reason.map(str::to_owned), calling_uid, delayed }.write(p), |_| Ok(()))
    }
    pub fn component_label_changed(&self, name: &str, uid: i32, class: &str, caller: i32) -> Result<(), Exception> {
        self.call(api::COMPONENT_LABEL_CHANGED, |p| api::ComponentLabelChanged { package_name: Some(name.into()), package_uid: uid, component_class: Some(class.into()), calling_uid: caller }.write(p), |_| Ok(()))
    }
    pub fn first_launch(&self, name: &str, installer: &str, user: i32) -> Result<(), Exception> {
        self.call(api::FIRST_LAUNCH, |p| api::FirstLaunch { package_name: Some(name.into()), installer_package: Some(installer.into()), user_id: user }.write(p), |_| Ok(()))
    }
    pub fn unhibernate(&self, name: &str, user: i32) -> Result<(), Exception> {
        self.call(api::UNHIBERNATE, |p| api::Unhibernate { package_name: Some(name.into()), user_id: user }.write(p), |_| Ok(()))
    }
    pub fn unstopped(&self, name: &str, user: i32) -> Result<(), Exception> {
        self.call(api::PACKAGE_UNSTOPPED, |p| api::PackageUnstopped { package_name: Some(name.into()), user_id: user }.write(p), |_| Ok(()))
    }
    pub fn hidden(&self, name: &str, user: i32, hidden: bool) -> Result<(), Exception> {
        self.call(api::PACKAGE_HIDDEN, |p| api::PackageHidden { package_name: Some(name.into()), user_id: user, hidden }.write(p), |_| Ok(()))
    }
    pub fn package_added(&self, name: &str, user: i32, archived: bool, loader: i32, prediction: Option<&str>) -> Result<(), Exception> {
        self.call(api::PACKAGE_ADDED, |p| api::PackageAdded { package_name: Some(name.into()), user_id: user, archived, data_loader_type: loader, prediction_package: prediction.map(str::to_owned) }.write(p), |_| Ok(()))
            .map_err(|mut error| {
                error.message = format!("Original packageAdded {name} user={user} (exception {}): {}", error.code, error.message);
                error
            })
    }
    pub fn suspension_allowed(&self, user: i32, uid: i32) -> Result<bool, Exception> {
        self.call(api::IS_SUSPENSION_ALLOWED, |p| api::IsSuspensionAllowed { user_id: user, calling_uid: uid }.write(p), |r| r.read_bool())
    }
    pub fn can_suspend(&self, packages: &[Option<String>], user: i32, uid: i32) -> Result<Vec<bool>, Exception> {
        self.call(api::CAN_SUSPEND_PACKAGES, |p| api::CanSuspendPackages { packages: Some(packages.to_vec()), user_id: user, calling_uid: uid }.write(p), |r| {
            let count = r.read_i32()?;
            if count < 0 || count as usize != packages.len() { return Err(aim_binder_host::parcel::BAD_VALUE); }
            (0..count).map(|_| r.read_bool()).collect()
        })
    }
    pub fn suspended(&self, names: &[String], uids: &[i32], user: i32, suspended: bool, quarantined: bool, changed_only: bool) -> Result<(), Exception> {
        self.call(api::PACKAGES_SUSPENDED, |p| api::PackagesSuspended { packages: Some(names.iter().cloned().map(Some).collect()), uids: Some(uids.to_vec()), user_id: user, suspended, quarantined, changed_only }.write(p), |_| Ok(()))
    }
    pub fn removed_suspensions(&self, packages: &[String], uids: &[i32], user: i32) -> Result<(), Exception> {
        self.call(api::REMOVED_SUSPENSIONS, |p| api::RemovedSuspensions { packages: Some(packages.iter().cloned().map(Some).collect()), uids: Some(uids.to_vec()), user_id: user }.write(p), |_| Ok(()))
    }
    pub fn distraction(&self, names: &[String], uids: &[i32], user: i32, flags: i32) -> Result<(), Exception> {
        self.call(api::DISTRACTION_CHANGED, |p| api::DistractionChanged { packages: Some(names.iter().cloned().map(Some).collect()), uids: Some(uids.to_vec()), user_id: user, flags }.write(p), |_| Ok(()))
    }
    pub fn user_removed(&self, user: i32) -> Result<(), Exception> {
        self.call(api::USER_REMOVED, |p| api::UserRemoved { user_id: user }.write(p), |_| Ok(()))
    }
    /// Effects run after native state publication and outside bootstrap locks.
    pub fn finish_mutation(&self, request: &super::write::mutation::Request, plan: &super::write::mutation::Plan, state: &super::model::State, calling_uid: i32) -> Result<(), Exception> {
        use super::write::mutation::{Request, Change};
        match request {
            Request::Enabled(setting) if !matches!(plan.change, Change::None) => {
                let package = state.packages.get(&plan.package).ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "changed package disappeared"))?;
                self.package_changed(&plan.package, super::apps_filter::uid(setting.user, package.app_id), setting.flags & 1 != 0,
                    &[setting.class.clone().unwrap_or_else(|| plan.package.clone())], None, calling_uid, setting.flags & 1 != 0)?;
            }
            Request::Stopped { package, user, stopped } if state.users.contains_key(user) => {
                if let Change::Stopped { first_launch_installer, was_stopped, .. } = &plan.change {
                    if let Some(installer) = first_launch_installer { self.first_launch(package, installer, *user)?; }
                    if !*stopped {
                        self.unhibernate(package, *user)?;
                        if *was_stopped { self.unstopped(package, *user)?; }
                    }
                } else if !*stopped { self.unhibernate(package, *user)?; }
            }
            _ => {}
        }
        Ok(())
    }
}
fn transport(status: i32) -> Exception { Exception::new(EX_ILLEGAL_STATE, format!("package effects transport: {status}")) }

/// The resolver rebuild uses every group referenced by each registered filter.
/// Clearing preferred choices is needed only when that effective index changes.
pub fn mime_index_changed(before: &super::model::State, after: &super::model::State, package: &str, group: Option<&str>) -> Result<bool, Exception> {
    let before = before.packages.get(package).ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "old MIME package absent"))?;
    let after = after.packages.get(package).ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "new MIME package absent"))?;
    let Some(code) = before.pkg.as_ref() else { return Ok(false); };
    let effective = |filter: &super::intent_filter::IntentFilter, state: &super::model::PackageState| -> Result<_, Exception> {
        let mut filter = filter.clone();
        filter.clear_dynamic_data_types();
        for group in filter.mime_groups.clone().iter().flatten().rev() {
            let types = state.mime_groups.iter().find(|(name, _)| name.as_deref() == Some(group.as_str())).ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "MIME group owner absent"))?;
            for ty in &types.1 {
                let ty = ty.as_deref().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "null MIME type"))?;
                // Original MimeGroupsAwareIntentResolver ignores malformed MIME
                // entries instead of registering an invalid index key.
                if filter.add_dynamic_data_type(ty).is_err() { continue; }
            }
        }
        Ok((filter.types, filter.has_dynamic_partial_types))
    };
    for component in code.activities.iter().chain(&code.receivers).map(|component| &component.main.component)
        .chain(code.services.iter().map(|component| &component.main.component))
        .chain(code.providers.iter().map(|component| &component.main.component)) {
        for intent in &component.intents {
            if intent.filter.mime_groups.as_ref().is_some_and(|groups| group.is_some_and(|group| groups.iter().any(|value| value == group)))
                && effective(&intent.filter, before)? != effective(&intent.filter, after)? { return Ok(true); }
        }
    }
    Ok(false)
}
