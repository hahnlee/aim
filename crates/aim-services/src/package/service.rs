//! Binder package endpoints for M4 C. They consume an owned scan snapshot,
//! never the original PMS's shadow feed. These endpoints are not yet
//! registered by guest-init: native scanning, mutation side effects and
//! the SystemServer facade must pass the C gates first (#798).

use std::sync::Arc;
#[cfg(test)]
use std::sync::RwLock;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{EX_UNSUPPORTED_OPERATION, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::android_content_pm_ipackagemanager as pm;
use aim_service_aidl::android_content_pm_ipackagemanagernative as native;

use super::apps_filter::NotModelled;
#[cfg(test)]
use super::model::State;
use super::query::Query;
use super::resolve::{QueryError, Resolver};

struct DomainAgentComponent<'a>(&'a super::intent::ComponentName);
impl aim_service_aidl::WriteParcelable for DomainAgentComponent<'_> {
    fn write_to(&self, parcel: &mut Parcel) {
        parcel.write_string16(Some(&self.0.package));
        parcel.write_string16(Some(&self.0.class));
    }
}

/// Both interfaces share the owner's published state. Each transaction
/// captures one immutable snapshot; publication does not invalidate
/// Binder references already held by clients.
pub struct PackageQueries {
    source: Source,
    resolver: Arc<Resolver>,
    native: bool,
    system: Option<std::sync::Weak<crate::system::System>>,
}

enum Source {
    Native(
        Arc<
            dyn Fn() -> Result<Arc<super::scan_snapshot::query_state::Capture>, Exception>
                + Send
                + Sync,
        >,
    ),
    #[cfg(test)]
    Fixture(Arc<RwLock<Arc<State>>>),
}
impl PackageQueries {
    pub fn from_bootstrap(system: &Arc<crate::system::System>, bridge: &Arc<super::bootstrap::Bridge>) -> (Arc<Self>, Arc<Self>) {
        let (public, native) = Self::from_system(system);
        let attach = |owner: &Arc<Self>| {
            let parent = Arc::downgrade(system);
            let bridge = bridge.clone();
            Arc::new(Self {
                source: Source::Native(Arc::new(move || {
                    let system = parent.upgrade().ok_or_else(|| Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE, "package endpoint system stopped"))?;
                    system.check_package_bootstrap(&bridge)?;
                    system.capture_package_queries()
                })),
                resolver: owner.resolver.clone(), native: owner.native, system: owner.system.clone(),
            })
        };
        (attach(&public), attach(&native))
    }

    pub fn from_system(system: &Arc<crate::system::System>) -> (Arc<Self>, Arc<Self>) {
        let system = Arc::downgrade(system);
        let runtime_system = system.clone();
        let source = Arc::new(move || {
            system
                .upgrade()
                .ok_or_else(|| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "native system owner is unavailable",
                    )
                })?
                .capture_package_queries()
        });
        let resolver = Arc::new(Resolver::default());
        (
            Arc::new(Self {
                source: Source::Native(source.clone()),
                resolver: resolver.clone(),
                native: false,
                system: Some(runtime_system.clone()),
            }),
            Arc::new(Self {
                source: Source::Native(source),
                resolver,
                native: true,
                system: Some(runtime_system),
            }),
        )
    }
    #[cfg(test)]
    pub fn new(state: Arc<RwLock<Arc<State>>>) -> (Arc<Self>, Arc<Self>) {
        let resolver = Arc::new(Resolver::default());
        let package = Arc::new(Self {
            source: Source::Fixture(state.clone()),
            resolver: resolver.clone(),
            native: false,
            system: None,
        });
        let package_native = Arc::new(Self {
            source: Source::Fixture(state),
            resolver,
            native: true,
            system: None,
        });
        (package, package_native)
    }
}

impl PackageQueries {
    fn mutation(
        &self,
        call: &mut Call<'_>,
        capture: Option<&Arc<super::scan_snapshot::query_state::Capture>>,
        query: &Query<'_>,
    ) -> Option<Result<Parcel, QueryError>> {
        if call.code == pm::CLEAR_PACKAGE_PREFERRED_ACTIVITIES
            && query.state.system.preferred_owner.is_some() {
            return None;
        }
        if call.code == pm::SET_BLOCK_UNINSTALL_FOR_USER {
            return Some((|| {
                let request = super::mutations::BlockUninstall::read(&mut call.data)
                    .map_err(QueryError::Transport)?;
                let result = if capture.is_some() {
                    let system = self
                        .system
                        .as_ref()
                        .and_then(|system| system.upgrade())
                        .ok_or(QueryError::NotModelled(NotModelled(
                            "native mutation owner unavailable",
                        )))?;
                    system.commit_package_uninstall_block(
                        &request,
                        &self.resolver,
                        call.sender_euid as i32,
                    )?
                } else {
                    match request.decide(query).map_err(QueryError::NotModelled)? {
                        Err(error) => Err(error),
                        Ok(false) => Ok(false),
                        Ok(true) => Err(Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            "native mutation capture unavailable",
                        )),
                    }
                };
                let mut reply = Parcel::new();
                match result {
                    Ok(value) => pm::write_set_block_uninstall_for_user_reply(&mut reply, value),
                    Err(error) => reply.write_exception(&error),
                }
                Ok(reply)
            })());
        }
        let request =
            super::write::mutation::Request::read(call.code, call.sender_euid, &mut call.data)?;
        Some((|| {
            let request = request.map_err(QueryError::Transport)?;
            let result = if capture.is_some() {
                let system = self
                    .system
                    .as_ref()
                    .and_then(|system| system.upgrade())
                    .ok_or_else(|| {
                        QueryError::NotModelled(NotModelled("native mutation owner unavailable"))
                    })?;
                system.commit_package_mutation(
                    &request,
                    &self.resolver,
                    call.sender_euid as i32,
                    call.sender_pid,
                )?
            } else {
                match request
                    .decide(query, call.sender_pid)
                    .map_err(QueryError::NotModelled)?
                {
                    Err(exception) => Err(exception),
                    Ok(_) => Err(Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        "native mutation capture unavailable",
                    )),
                }
            };
            let mut reply = Parcel::new();
            match result {
                Ok(()) => reply.write_no_exception(),
                Err(error) => reply.write_exception(&error),
            }
            Ok(reply)
        })())
    }

    fn runtime_version(
        &self,
        call: &mut Call<'_>,
        capture: Option<&Arc<super::scan_snapshot::query_state::Capture>>,
        query: &Query<'_>,
    ) -> Result<Parcel, QueryError> {
        let (user, version) = if call.code == pm::GET_RUNTIME_PERMISSIONS_VERSION {
            let args = pm::GetRuntimePermissionsVersion::read(&mut call.data)
                .map_err(QueryError::Transport)?;
            (args.user_id, None)
        } else {
            let args = pm::SetRuntimePermissionsVersion::read(&mut call.data)
                .map_err(QueryError::Transport)?;
            (args.user_id, Some(args.version))
        };
        if call.data.remaining() != 0 {
            return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE));
        }
        let result = (|| {
            if user < 0 || version.is_some_and(|value| value < 0) {
                return Ok(Err(Exception::illegal_argument(
                    "runtime version and user must be nonnegative",
                )));
            }
            let uid = query.calling_uid;
            if super::apps_filter::is_isolated(uid) {
                return Ok(Err(Exception::security(
                    "isolated callers cannot change runtime permission policy",
                )));
            }
            if uid % 100_000 != 0
                && uid % 100_000 != 1000
                && !query.uid_has_permission(
                    uid,
                    "android.permission.ADJUST_RUNTIME_PERMISSIONS_POLICY",
                )?
                && !query
                    .uid_has_permission(uid, "android.permission.UPGRADE_RUNTIME_PERMISSIONS")?
            {
                return Ok(Err(Exception::security(
                    "runtime permission version requires policy adjustment or upgrade permission",
                )));
            }
            let Some(system) = self.system.as_ref().and_then(|system| system.upgrade()) else {
                return Ok(Err(Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "runtime metadata system owner is unavailable",
                )));
            };
            let Some(capture) = capture else {
                return Ok(Err(Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "native runtime metadata capture is unavailable",
                )));
            };
            Ok(
                system.with_runtime_permission_metadata(capture, |metadata| {
                    if let Some(version) = version {
                        metadata.set_version(user, version);
                    }
                    metadata.version(user)
                }),
            )
        })()
        .map_err(QueryError::NotModelled)?;
        let mut reply = Parcel::new();
        match result {
            Err(error) => reply.write_exception(&error),
            Ok(value) => {
                if version.is_some() {
                    pm::write_set_runtime_permissions_version_reply(&mut reply);
                } else {
                    pm::write_get_runtime_permissions_version_reply(&mut reply, value);
                }
            }
        }
        Ok(reply)
    }
}

impl Service for PackageQueries {
    fn accepts_fds(&self) -> bool { true }
    fn descriptor(&self) -> &str {
        if self.native {
            native::DESCRIPTOR
        } else {
            pm::DESCRIPTOR
        }
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if !self.native && call.code == super::dump::DUMP_TRANSACTION {
            let system = self.system.as_ref().and_then(|owner| owner.upgrade()).ok_or(UNKNOWN_TRANSACTION)?;
            return super::dump::run(&system, call, || match &self.source {
                Source::Native(source) => source().map(|capture| capture.state().clone()),
                #[cfg(test)]
                Source::Fixture(state) => Ok(state.read().unwrap().clone()),
            });
        }
        if !self.native && call.code==crate::shell::SHELL_COMMAND_TRANSACTION {
            let mut reply=Parcel::new();
            if !matches!(call.sender_euid,0|2000){reply.write_exception(&Exception::security("Shell commands are only callable by ADB"));return Ok(reply);}
            let system=self.system.as_ref().and_then(|owner|owner.upgrade()).ok_or(UNKNOWN_TRANSACTION)?;
            let context=super::shell::Context::new(&system,self,call)?;
            super::shell::dispatch(context);
            reply.write_no_exception();return Ok(reply);
        }
        let methods = if self.native {
            native::METHODS
        } else {
            pm::METHODS
        };
        if !methods.iter().any(|(code, _)| *code == call.code) {
            return Err(UNKNOWN_TRANSACTION);
        }
        // Check before dispatch so an unsupported method cannot accept
        // another interface's token. Generated readers check it again.
        let position = call.data.position();
        call.data.enforce_interface(self.descriptor())?;
        call.data.set_position(position);
        let uid = call.sender_euid as i32;
        let capture = match &self.source {
            Source::Native(source) => match source() {
                Ok(capture) => Some(capture),
                Err(error) => {
                    let mut reply = Parcel::new();
                    reply.write_exception(&error);
                    return Ok(reply);
                }
            },
            #[cfg(test)]
            Source::Fixture(_) => None,
        };
        let state = match (&self.source, &capture) {
            (Source::Native(_), Some(capture)) => capture.state().clone(),
            #[cfg(test)]
            (Source::Fixture(state), _) => state.read().unwrap().clone(),
            _ => unreachable!(),
        };
        let paged = if !self.native && capture.is_some()
            && matches!(call.code, pm::GET_INSTALLED_APPLICATIONS | pm::GET_INSTALLED_PACKAGES) {
            Some((|| -> Result<Parcel, QueryError> {
                use aim_service_aidl::ReadParcelable;
                let process=self.system.as_ref().and_then(|owner|owner.upgrade())
                    .ok_or(NotModelled("native list Binder process unavailable"))?.binder_process();
                let resolution=match capture.as_ref().unwrap().resolution() {
                    Ok(resolution)=>resolution,
                    Err(error)=>return error.reply().map_err(QueryError::Transport),
                };
                let query=Query{state:&state,filter:&resolution.apps_filter,calling_uid:uid};
                let mut reply=Parcel::new();
                if call.code==pm::GET_INSTALLED_APPLICATIONS {
                    let args=pm::GetInstalledApplications::read(&mut call.data).map_err(QueryError::Transport)?;
                    if call.data.remaining()!=0{return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE));}
                    match query.internal_installed_applications(args.flags,args.user_id,uid,false)? {
                        Ok(items)=>{let slice=super::list_slice::Slice::new(process,"android.content.pm.ApplicationInfo",items);pm::write_get_installed_applications_reply(&mut reply,Some(&slice));},
                        Err(error)=>reply.write_exception(&error),
                    }
                } else {
                    let args=pm::GetInstalledPackages::read(&mut call.data).map_err(QueryError::Transport)?;
                    if call.data.remaining()!=0{return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE));}
                    match query.installed_packages(args.flags,args.user_id)? {
                        Ok(items)=>{let slice=super::list_slice::Slice::new(process,"android.content.pm.PackageInfo",items);pm::write_get_installed_packages_reply(&mut reply,Some(&slice));},
                        Err(error)=>reply.write_exception(&error),
                    }
                }
                Ok(reply)
            })())
        } else {None};
        let resolved = paged.or_else(|| (!self.native)
            .then(|| capture.as_ref().map_or(&*self.resolver,|capture|capture.resolver()).query(&state, call.code, uid, &mut call.data))
            .flatten());
        let answer = resolved.unwrap_or_else(|| {
            let resolution = match capture.as_ref().map_or_else(||self.resolver.resolution(&state),|capture|capture.resolution()) {
                Ok(resolution) => resolution,
                Err(error) => return error.reply().map_err(QueryError::Transport),
            };
            let query = Query {
                state: &state,
                filter: &resolution.apps_filter,
                calling_uid: uid,
            };
            if !self.native && matches!(call.code, pm::VERIFY_INTENT_FILTER | pm::UPDATE_INTENT_VERIFICATION_STATUS) {
                let system = self.system.as_ref().and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("legacy domain system unavailable")))?;
                return super::legacy_domain_routes::dispatch(&system, call.sender_pid, uid, call.code, &mut call.data)
                    .ok_or(QueryError::Transport(UNKNOWN_TRANSACTION))
                    .and_then(|reply| reply.map_err(QueryError::Transport));
            }
            if !self.native && matches!(call.code, pm::DELETE_PACKAGE_VERSIONED | pm::DELETE_EXISTING_PACKAGE_AS_USER | pm::DELETE_PACKAGE_AS_USER) {
                let system = self.system.as_ref().and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("public removal system unavailable")))?;
                let owner = match system.public_package_removal() {
                    Ok(owner) => owner,
                    Err(error) => { let mut reply = Parcel::new(); reply.write_exception(&error); return Ok(reply); }
                };
                return owner.dispatch(call.sender_euid, call.sender_pid, call.code, &mut call.data)
                    .ok_or(QueryError::Transport(UNKNOWN_TRANSACTION))
                    .and_then(|reply| reply.map_err(QueryError::Transport));
            }
            if !self.native && call.code == pm::SET_INSTALLER_PACKAGE_NAME {
                let request = super::installer_attribution::Request::read(&mut call.data).map_err(QueryError::Transport)?;
                let system = self.system.as_ref().and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("installer attribution system unavailable")))?;
                let mut reply = Parcel::new();
                match system.set_native_installer_attribution(request, uid) {
                    Ok(()) => reply.write_no_exception(), Err(error) => reply.write_exception(&error),
                }
                return Ok(reply);
            }
            if !self.native && matches!(call.code, pm::FLUSH_PACKAGE_RESTRICTIONS_AS_USER
                | pm::NOTIFY_PACKAGES_REPLACED_RECEIVED | pm::SET_KEEP_UNINSTALLED_PACKAGES) {
                let system = self.system.as_ref().and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("package customization system unavailable")))?;
                let result = if call.code == pm::FLUSH_PACKAGE_RESTRICTIONS_AS_USER {
                    let args = pm::FlushPackageRestrictionsAsUser::read(&mut call.data).map_err(QueryError::Transport)?;
                    if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                    let base = capture.as_ref().ok_or(QueryError::NotModelled(NotModelled("restriction capture unavailable")))?;
                    system.flush_native_package_restrictions(&query, args.user_id, base)
                } else if call.code == pm::SET_KEEP_UNINSTALLED_PACKAGES {
                    let args = pm::SetKeepUninstalledPackages::read(&mut call.data).map_err(QueryError::Transport)?;
                    if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                    system.set_native_keep_uninstalled_packages(&query, args.package_list)
                } else {
                    let args = pm::NotifyPackagesReplacedReceived::read(&mut call.data).map_err(QueryError::Transport)?;
                    if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                    system.package_customization().and_then(|owner| owner.notify_replaced(&query, args.packages))
                };
                let mut reply = Parcel::new();
                match result { Ok(()) => reply.write_no_exception(), Err(error) => reply.write_exception(&error) }
                return Ok(reply);
            }
            if !self.native && call.code == pm::SET_PAGE_SIZE_APP_COMPAT_FLAGS_SETTINGS_OVERRIDE {
                let args = pm::SetPageSizeAppCompatFlagsSettingsOverride::read(&mut call.data).map_err(QueryError::Transport)?;
                if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                let system = self.system.as_ref().and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("page size system unavailable")))?;
                let mut reply = Parcel::new();
                match system.set_package_page_size_override(args.package_name.as_deref(), args.enabled, uid) {
                    Ok(()) => reply.write_no_exception(), Err(error) => reply.write_exception(&error),
                }
                return Ok(reply);
            }
            if !self.native && matches!(call.code, pm::OVERRIDE_LABEL_AND_ICON | pm::RESTORE_LABEL_AND_ICON) {
                let request = super::customization::Label::read(call.code, &mut call.data).map_err(QueryError::Transport)?;
                if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                let system = self.system.as_ref().and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("label override system unavailable")))?;
                let base = capture.as_ref().ok_or(QueryError::NotModelled(NotModelled("label capture unavailable")))?;
                let mut reply = Parcel::new();
                match system.apply_package_label_override(&query, request, base) {
                    Ok(()) => reply.write_no_exception(), Err(error) => reply.write_exception(&error),
                }
                return Ok(reply);
            }
            if !self.native && matches!(call.code, pm::VERIFY_PENDING_INSTALL | pm::EXTEND_VERIFICATION_TIMEOUT) {
                let system = self.system.as_ref().and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("verification system unavailable")))?;
                let mut reply = Parcel::new();
                let result = if call.code == pm::VERIFY_PENDING_INSTALL {
                    let args = pm::VerifyPendingInstall::read(&mut call.data).map_err(QueryError::Transport)?;
                    if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                    let permitted = args.id < 0 || query.uid_has_permission(uid, "android.permission.PACKAGE_VERIFICATION_AGENT")
                        .map_err(QueryError::NotModelled)?;
                    if args.id >= 0 && !permitted {
                        Err(Exception::security("Only package verification agents can verify applications"))
                    } else {
                        system.pending_package_verification().and_then(|owner|
                            owner.verify(args.id, args.verification_code, uid, permitted))
                    }
                } else {
                    let args = pm::ExtendVerificationTimeout::read(&mut call.data).map_err(QueryError::Transport)?;
                    if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                    let permitted = args.id < 0 || query.uid_has_permission(uid, "android.permission.PACKAGE_VERIFICATION_AGENT")
                        .map_err(QueryError::NotModelled)?;
                    if args.id >= 0 && !permitted {
                        Err(Exception::security("Only package verification agents can extend verification timeouts"))
                    } else {
                        system.pending_package_verification().and_then(|owner|
                            owner.extend(args.id, args.verification_code_at_timeout, args.milliseconds_to_delay, uid, permitted))
                    }
                };
                match result { Ok(()) => reply.write_no_exception(), Err(error) => reply.write_exception(&error) }
                return Ok(reply);
            }
            if !self.native && matches!(call.code, pm::GET_DOMAIN_VERIFICATION_AGENT
                | pm::GET_DOMAIN_VERIFICATION_BACKUP | pm::RESTORE_DOMAIN_VERIFICATION) {
                let system = self.system.as_ref().and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("native verification system unavailable")))?;
                let mut reply = Parcel::new();
                match call.code {
                    pm::GET_DOMAIN_VERIFICATION_AGENT => {
                        let args = pm::GetDomainVerificationAgent::read(&mut call.data).map_err(QueryError::Transport)?;
                        if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                        match system.get_native_domain_verification_agent(uid, args.user_id) {
                            Ok(value) => {
                                let component = value.as_ref().map(DomainAgentComponent);
                                pm::write_get_domain_verification_agent_reply(&mut reply, component.as_ref());
                            }
                            Err(error) => reply.write_exception(&error),
                        }
                    }
                    pm::GET_DOMAIN_VERIFICATION_BACKUP => {
                        let args = pm::GetDomainVerificationBackup::read(&mut call.data).map_err(QueryError::Transport)?;
                        if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                        match system.get_native_domain_backup(uid, args.user_id) {
                            Ok(value) => pm::write_get_domain_verification_backup_reply(&mut reply, &value),
                            Err(error) => reply.write_exception(&error),
                        }
                    }
                    _ => {
                        let args = pm::RestoreDomainVerification::read(&mut call.data).map_err(QueryError::Transport)?;
                        if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                        match system.restore_native_domain_backup(uid, args.backup.as_deref(), args.user_id) {
                            Ok(()) => reply.write_no_exception(), Err(error) => reply.write_exception(&error),
                        }
                    }
                }
                return Ok(reply);
            }
            if !self.native && matches!(call.code, pm::CLEAR_APPLICATION_USER_DATA
                | pm::DELETE_PRELOADS_FILE_CACHE | pm::SEND_DEVICE_CUSTOMIZATION_READY_BROADCAST) {
                let system = self.system.as_ref().and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("application data system unavailable")))?;
                let result = (|| {
                    let owner = system.package_application_data()?;
                    match call.code {
                        pm::CLEAR_APPLICATION_USER_DATA => {
                            let args = pm::ClearApplicationUserData::read(&mut call.data).map_err(|status|
                                Exception::new(aim_binder_host::parcel::EX_BAD_PARCELABLE, format!("clear data parcel: {status}")))?;
                            if call.data.remaining() != 0 { return Err(Exception::new(aim_binder_host::parcel::EX_BAD_PARCELABLE, "trailing clear data arguments")); }
                            let bridge = system.package_bootstrap()?;
                            let effects = system.package_effects_owner(&bridge)?;
                            owner.clear(args.package_name, args.user_id, call.sender_pid, uid, args.observer,
                                effects.as_ref())
                        }
                        pm::DELETE_PRELOADS_FILE_CACHE => {
                            pm::DeletePreloadsFileCache::read(&mut call.data).map_err(|status|
                                Exception::new(aim_binder_host::parcel::EX_BAD_PARCELABLE, format!("preload parcel: {status}")))?;
                            if call.data.remaining() != 0 { return Err(Exception::new(aim_binder_host::parcel::EX_BAD_PARCELABLE, "trailing preload arguments")); }
                            owner.delete_preloads(call.sender_pid, uid)
                        }
                        _ => {
                            pm::SendDeviceCustomizationReadyBroadcast::read(&mut call.data).map_err(|status|
                                Exception::new(aim_binder_host::parcel::EX_BAD_PARCELABLE, format!("customization parcel: {status}")))?;
                            if call.data.remaining() != 0 { return Err(Exception::new(aim_binder_host::parcel::EX_BAD_PARCELABLE, "trailing customization arguments")); }
                            owner.customization_ready(call.sender_pid, uid)
                        }
                    }
                })();
                let mut reply = Parcel::new();
                match result { Ok(()) => reply.write_no_exception(), Err(error) => reply.write_exception(&error) }
                return Ok(reply);
            }
            if !self.native && call.code == pm::MOVE_PACKAGE {
                let args = pm::MovePackage::read(&mut call.data).map_err(QueryError::Transport)?;
                if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                let system = self.system.as_ref().and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("native relocation system unavailable")))?;
                let mut reply = Parcel::new();
                let result = system.package_relocation_owner().and_then(|owner|
                    owner.move_package(args.package_name, args.volume_uuid, call.sender_euid, super::apps_filter::user_id(uid)));
                match result { Ok(id) => pm::write_move_package_reply(&mut reply, id), Err(error) => reply.write_exception(&error) }
                return Ok(reply);
            }
            if !self.native {
                if let (Some(capture), Some(system)) = (capture.as_ref(),
                    self.system.as_ref().and_then(|owner| owner.upgrade())) {
                    if let Some(result) = system.dispatch_package_mutation(call, &query, capture) {
                        return Ok(match result {
                            Ok(reply) => reply,
                            Err(error) => { let mut reply = Parcel::new(); reply.write_exception(&error); reply },
                        });
                    }
                }
            }
            if !self.native && call.code == pm::MOVE_PRIMARY_STORAGE {
                let args = pm::MovePrimaryStorage::read(&mut call.data).map_err(QueryError::Transport)?;
                if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                let mut reply = Parcel::new();
                if !query.uid_has_permission(uid, "android.permission.MOVE_PACKAGE").map_err(QueryError::NotModelled)? {
                    reply.write_exception(&Exception::security("Requires android.permission.MOVE_PACKAGE"));
                } else {
                    let system = self.system.as_ref().and_then(|owner| owner.upgrade())
                        .ok_or(QueryError::NotModelled(NotModelled("native move system unavailable")))?;
                    match system.package_move_primary(args.volume_uuid) {
                        Ok(id) => pm::write_move_primary_storage_reply(&mut reply, id),
                        Err(error) => reply.write_exception(&error),
                    }
                }
                return Ok(reply);
            }
            if !self.native && matches!(call.code,
                pm::GET_MOVE_STATUS | pm::REGISTER_MOVE_CALLBACK | pm::UNREGISTER_MOVE_CALLBACK) {
                let permitted = query.uid_has_permission(uid, "android.permission.MOUNT_UNMOUNT_FILESYSTEMS")
                    .map_err(QueryError::NotModelled)?;
                if !permitted {
                    let mut reply = Parcel::new();
                    reply.write_exception(&Exception::security("Requires android.permission.MOUNT_UNMOUNT_FILESYSTEMS"));
                    return Ok(reply);
                }
                let system = self.system.as_ref().and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("native move system unavailable")))?;
                let owner = match system.package_moves() {
                    Ok(owner) => owner,
                    Err(error) => { let mut reply = Parcel::new(); reply.write_exception(&error); return Ok(reply); }
                };
                let mut reply = Parcel::new();
                match call.code {
                    pm::GET_MOVE_STATUS => {
                        let args = pm::GetMoveStatus::read(&mut call.data).map_err(QueryError::Transport)?;
                        if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                        match owner.status(args.move_id) {
                            Ok(status) => pm::write_get_move_status_reply(&mut reply, status),
                            Err(error) => reply.write_exception(&error),
                        }
                    }
                    pm::REGISTER_MOVE_CALLBACK => {
                        let args = pm::RegisterMoveCallback::read(&mut call.data).map_err(QueryError::Transport)?;
                        if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                        match owner.register(args.callback) {
                            Ok(()) => reply.write_no_exception(), Err(error) => reply.write_exception(&error),
                        }
                    }
                    _ => {
                        let args = pm::UnregisterMoveCallback::read(&mut call.data).map_err(QueryError::Transport)?;
                        if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                        match owner.unregister(args.callback) {
                            Ok(()) => reply.write_no_exception(), Err(error) => reply.write_exception(&error),
                        }
                    }
                }
                return Ok(reply);
            }
            if !self.native && super::diagnostics::Runtime::handles(call.code) {
                let system = self.system.as_ref().and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("maintenance system unavailable")))?;
                let owner = match system.package_maintenance_owner() {
                    Ok(owner) => owner,
                    Err(error) => { let mut reply = Parcel::new(); reply.write_exception(&error); return Ok(reply); }
                };
                return owner.answer(call, &query).ok_or(QueryError::Transport(UNKNOWN_TRANSACTION))
                    .and_then(|reply| reply.map_err(QueryError::Transport));
            }
            if !self.native
                && matches!(
                    call.code,
                    pm::GET_INSTALL_LOCATION
                        | pm::SET_INSTALL_LOCATION
                        | pm::CAN_REQUEST_PACKAGE_INSTALLS
                        | pm::IS_PACKAGE_STATE_PROTECTED
                        | pm::IS_PACKAGE_DEVICE_ADMIN_ON_ANY_USER
                        | pm::IS_STORAGE_LOW
                )
            {
                let system = self
                    .system
                    .as_ref()
                    .and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled(
                        "native policy system unavailable",
                    )));
                let system = match system {
                    Ok(system) => system,
                    Err(error) => return Err(error),
                };
                let bridge = match system.package_bootstrap() {
                    Ok(bridge) => bridge,
                    Err(error) => {
                        let mut reply = Parcel::new();
                        reply.write_exception(&error);
                        return Ok(reply);
                    }
                };
                let owner = match system.package_policy_owner(&bridge) {
                    Ok(owner) => owner,
                    Err(error) => {
                        let mut reply = Parcel::new();
                        reply.write_exception(&error);
                        return Ok(reply);
                    }
                };
                let reply = owner
                    .answer(&query, call)
                    .ok_or(QueryError::Transport(UNKNOWN_TRANSACTION))
                    .and_then(|reply| reply.map_err(QueryError::Transport))?;
                if let Err(error) = system.check_package_bootstrap(&bridge) {
                    let mut reply = Parcel::new();
                    reply.write_exception(&error);
                    return Ok(reply);
                }
                return Ok(reply);
            }
            if !self.native
                && matches!(
                    call.code,
                    pm::INSTALL_EXISTING_PACKAGE_AS_USER | pm::FINISH_PACKAGE_INSTALL
                )
            {
                let system = self
                    .system
                    .as_ref()
                    .and_then(|system| system.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled(
                        "native existing installer system unavailable",
                    )))?;
                let mut reply = Parcel::new();
                if call.code == pm::INSTALL_EXISTING_PACKAGE_AS_USER {
                    let args = pm::InstallExistingPackageAsUser::read(&mut call.data)
                        .map_err(QueryError::Transport)?;
                    if call.data.remaining() != 0 {
                        return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE));
                    }
                    let request = super::installer::existing::Request {
                        package: args.package_name,
                        user: args.user_id,
                        flags: args.install_flags,
                        reason: args.install_reason,
                        allowlisted_permissions: args.white_listed_permissions,
                        receiver: None,
                    };
                    let result = system.existing_package_owner().and_then(|owner| {
                        owner.install_existing(call.sender_euid, call.sender_pid, request)
                    });
                    match result {
                        Ok(status) => {
                            pm::write_install_existing_package_as_user_reply(&mut reply, status)
                        }
                        Err(error) => reply.write_exception(&error),
                    }
                } else {
                    let args = pm::FinishPackageInstall::read(&mut call.data)
                        .map_err(QueryError::Transport)?;
                    if call.data.remaining() != 0 {
                        return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE));
                    }
                    match system.finish_existing_package_install(
                        call.sender_euid,
                        args.token,
                        args.did_launch,
                    ) {
                        Ok(_) => pm::write_finish_package_install_reply(&mut reply),
                        Err(error) => reply.write_exception(&error),
                    }
                }
                return Ok(reply);
            }
            if !self.native && call.code == pm::REQUEST_PACKAGE_CHECKSUMS {
                let request = super::query::checksums::Request::read(&mut call.data)
                    .map_err(QueryError::Transport)?;
                let result = match request.prepare(&query).map_err(QueryError::NotModelled)? {
                    Err(error) => Err(error),
                    Ok(prepared) => (|| {
                        let system = self
                            .system
                            .as_ref()
                            .and_then(|owner| owner.upgrade())
                            .ok_or_else(|| {
                                Exception::new(
                                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                                    "native checksum system owner unavailable",
                                )
                            })?;
                        let bridge = system.package_bootstrap()?;
                        let owner = system.package_installer_files(&bridge)?;
                        prepared.send(&owner)
                    })(),
                };
                let mut reply = Parcel::new();
                match result {
                    Ok(()) => pm::write_request_package_checksums_reply(&mut reply),
                    Err(error) => reply.write_exception(&error),
                }
                return Ok(reply);
            }
            if !self.native && call.code == pm::GET_PACKAGE_INSTALLER {
                pm::GetPackageInstaller::read(&mut call.data).map_err(QueryError::Transport)?;
                if call.data.remaining() != 0 {
                    return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE));
                }
                let mut reply = Parcel::new();
                let result = (|| {
                    let system = self
                        .system
                        .as_ref()
                        .and_then(|system| system.upgrade())
                        .ok_or_else(|| {
                            Exception::new(
                                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                                "installer system owner unavailable",
                            )
                        })?;
                    if capture.is_none() {
                        return Err(Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            "installer native capture unavailable",
                        ));
                    }
                    system.package_installer()
                })();
                match result {
                    Ok(binder) => pm::write_get_package_installer_reply(&mut reply, Some(binder)),
                    Err(error) => reply.write_exception(&error),
                }
                return Ok(reply);
            }
            if !self.native
                && call.code == pm::GET_HARMFUL_APP_WARNING
                && uid == 2000
                && capture.is_some()
            {
                let position = call.data.position();
                let args = pm::GetHarmfulAppWarning::read(&mut call.data)
                    .map_err(QueryError::Transport)?;
                if call.data.remaining() != 0 {
                    return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE));
                }
                call.data.set_position(position);
                if args.user_id >= 0 {
                    let system = self
                        .system
                        .as_ref()
                        .and_then(|owner| owner.upgrade())
                        .ok_or(QueryError::NotModelled(NotModelled(
                            "native system owner unavailable",
                        )))?;
                    let restricted = match system.package_shell_debugging_policy(args.user_id) {
                        Ok(value) => value,
                        Err(error) => {
                            let mut reply = Parcel::new();
                            reply.write_exception(&error);
                            return Ok(reply);
                        }
                    };
                    return query
                        .user_status_with_shell(call.code, &mut call.data, Some(restricted))
                        .map_err(QueryError::NotModelled);
                }
            }
            if !self.native
                && matches!(
                    call.code,
                    pm::GET_RUNTIME_PERMISSIONS_VERSION | pm::SET_RUNTIME_PERMISSIONS_VERSION
                )
            {
                return self.runtime_version(call, capture.as_ref(), &query);
            }
            if !self.native
                && matches!(
                    call.code,
                    pm::SET_SPLASH_SCREEN_THEME
                        | pm::SET_BLOCK_UNINSTALL_FOR_USER
                        | pm::SET_USER_MIN_ASPECT_RATIO
                        | pm::SET_UPDATE_AVAILABLE
                        | pm::SET_HARMFUL_APP_WARNING
                        | pm::SET_APPLICATION_CATEGORY_HINT
                        | pm::RELINQUISH_UPDATE_OWNERSHIP
                )
            {
                if let Some(answer) = self.mutation(call, capture.as_ref(), &query) {
                    return answer;
                }
            }
            if !self.native && matches!(call.code,pm::WAIT_FOR_HANDLER|pm::REGISTER_PACKAGE_MONITOR_CALLBACK|pm::UNREGISTER_PACKAGE_MONITOR_CALLBACK) {
                enum Action {Wait(i64,bool),Register(Option<aim_binder_host::parcel::Binder>,i32),Unregister(Option<aim_binder_host::parcel::Binder>)}
                let action=match call.code {
                    pm::WAIT_FOR_HANDLER=>{let args=pm::WaitForHandler::read(&mut call.data).map_err(QueryError::Transport)?;Action::Wait(args.timeout_millis,args.for_background_handler)}
                    pm::REGISTER_PACKAGE_MONITOR_CALLBACK=>{let args=pm::RegisterPackageMonitorCallback::read(&mut call.data).map_err(QueryError::Transport)?;Action::Register(args.callback,args.user_id)}
                    _=>{let args=pm::UnregisterPackageMonitorCallback::read(&mut call.data).map_err(QueryError::Transport)?;Action::Unregister(args.callback)}
                };
                if call.data.remaining()!=0 {return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE));}
                let system=self.system.as_ref().and_then(|owner|owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("package events system owner unavailable")))?;
                let mut reply=Parcel::new();let result=match action {
                    Action::Wait(timeout,background)=>system.package_wait_for_handler(timeout,background)
                        .map(|value|pm::write_wait_for_handler_reply(&mut reply,value)),
                    Action::Register(callback,user)=>system.package_register_monitor(callback,uid,call.sender_pid,user)
                        .map(|()|pm::write_register_package_monitor_callback_reply(&mut reply)),
                    Action::Unregister(callback)=>system.package_unregister_monitor(callback)
                        .map(|()|pm::write_unregister_package_monitor_callback_reply(&mut reply)),
                };
                if let Err(error)=result{reply.write_exception(&error);}return Ok(reply);
            }
            if !self.native && call.code == pm::LOG_APP_PROCESS_START_IF_NEEDED {
                let args=pm::LogAppProcessStartIfNeeded::read(&mut call.data).map_err(QueryError::Transport)?;
                if call.data.remaining()!=0 {return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE));}
                let system=self.system.as_ref().and_then(|owner|owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("process logging system owner unavailable")))?;
                let mut reply=Parcel::new();match system.package_log_process_start(uid,args) {
                    Ok(())=>pm::write_log_app_process_start_if_needed_reply(&mut reply),Err(error)=>reply.write_exception(&error) }
                return Ok(reply);
            }
            if !self.native && call.code==pm::NOTIFY_PACKAGE_USE {
                let args=pm::NotifyPackageUse::read(&mut call.data).map_err(QueryError::Transport)?;
                if call.data.remaining()!=0 {return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE));}
                let system=self.system.as_ref().and_then(|owner|owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("package usage system owner unavailable")))?;
                let mut reply=Parcel::new();if let Err(error)=system.package_notify_use(uid,args.package_name,args.reason){reply.write_exception(&error);}
                return Ok(reply);
            }
            if !self.native && call.code == pm::GET_VERIFIER_DEVICE_IDENTITY {
                pm::GetVerifierDeviceIdentity::read(&mut call.data).map_err(QueryError::Transport)?;
                if call.data.remaining() != 0 { return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE)); }
                let system = self.system.as_ref().and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled("verifier system owner unavailable")))?;
                let mut reply = Parcel::new();
                match system.package_verifier_identity(uid, &self.resolver)? {
                    Ok(identity) => pm::write_get_verifier_device_identity_reply(&mut reply, Some(&identity)),
                    Err(error) => reply.write_exception(&error),
                }
                return Ok(reply);
            }
            if !self.native && matches!(call.code, pm::GET_HOLD_LOCK_TOKEN | pm::HOLD_LOCK) {
                let duration = if call.code == pm::GET_HOLD_LOCK_TOKEN {
                    pm::GetHoldLockToken::read(&mut call.data).map_err(QueryError::Transport)?;
                    None
                } else {
                    let args = pm::HoldLock::read(&mut call.data).map_err(QueryError::Transport)?;
                    Some((args.token, args.duration_ms))
                };
                if call.data.remaining() != 0 {
                    return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE));
                }
                let system = self
                    .system
                    .as_ref()
                    .and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled(
                        "hold lock system owner unavailable",
                    )))?;
                let mut reply = Parcel::new();
                if let Some((token, duration)) = duration {
                    match system.package_hold_lock(uid, token, duration) {
                        Ok(()) => pm::write_hold_lock_reply(&mut reply),
                        Err(error) => reply.write_exception(&error),
                    }
                } else {
                    match system
                        .package_hold_lock_token(&query)
                        .map_err(QueryError::NotModelled)?
                    {
                        Ok(token) => pm::write_get_hold_lock_token_reply(&mut reply, Some(token)),
                        Err(error) => reply.write_exception(&error),
                    }
                }
                return Ok(reply);
            }
            if !self.native && call.code == pm::GET_PERMISSION_GROUP_INFO {
                let system = self
                    .system
                    .as_ref()
                    .and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled(
                        "permission group system owner unavailable",
                    )))?;
                return super::permission_mutation::group_info(&system, call)
                    .map_err(QueryError::Transport);
            }
            if !self.native
                && matches!(
                    call.code,
                    pm::ADD_PERMISSION
                        | pm::ADD_PERMISSION_ASYNC
                        | pm::REMOVE_PERMISSION
                        | pm::GRANT_RUNTIME_PERMISSION
                )
            {
                let system = self
                    .system
                    .as_ref()
                    .and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled(
                        "permission mutation system owner unavailable",
                    )))?;
                return super::permission_mutation::transact(&system, call)
                    .expect("permission mutation cohort")
                    .map_err(QueryError::Transport);
            }
            if !self.native && call.code == pm::IS_AUTO_REVOKE_WHITELISTED {
                let args = pm::IsAutoRevokeWhitelisted::read(&mut call.data)
                    .map_err(QueryError::Transport)?;
                if call.data.remaining() != 0 {
                    return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE));
                }
                let system = self
                    .system
                    .as_ref()
                    .and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled(
                        "auto revoke system owner unavailable",
                    )))?;
                let mut reply = Parcel::new();
                match system.package_is_auto_revoke_whitelisted(uid, args.package_name) {
                    Ok(value) => pm::write_is_auto_revoke_whitelisted_reply(&mut reply, value),
                    Err(error) => reply.write_exception(&error),
                }
                return Ok(reply);
            }
            if !self.native && call.code == pm::MAKE_PROVIDER_VISIBLE {
                let args =
                    pm::MakeProviderVisible::read(&mut call.data).map_err(QueryError::Transport)?;
                if call.data.remaining() != 0 {
                    return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE));
                }
                let system = self
                    .system
                    .as_ref()
                    .and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled(
                        "provider visibility system owner unavailable",
                    )))?;
                let result = system.package_make_provider_visible(
                    uid,
                    &super::visibility_mutation::MakeProviderVisible {
                        recipient: args.recipient_app_id,
                        authority: args.visible_authority,
                    },
                    &self.resolver,
                )?;
                let mut reply = Parcel::new();
                match result {
                    Ok(()) => pm::write_make_provider_visible_reply(&mut reply),
                    Err(error) => reply.write_exception(&error),
                }
                return Ok(reply);
            }
            if !self.native && call.code == pm::MAKE_UID_VISIBLE {
                let args =
                    pm::MakeUidVisible::read(&mut call.data).map_err(QueryError::Transport)?;
                if call.data.remaining() != 0 {
                    return Err(QueryError::Transport(aim_binder_host::parcel::BAD_VALUE));
                }
                let system = self
                    .system
                    .as_ref()
                    .and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled(
                        "visibility system owner unavailable",
                    )))?;
                let result = system.package_make_uid_visible(
                    uid,
                    &super::visibility_mutation::MakeUidVisible {
                        recipient: args.recipient_app_id,
                        visible: args.visible_uid,
                    },
                    &self.resolver,
                )?;
                let mut reply = Parcel::new();
                match result {
                    Ok(()) => pm::write_make_uid_visible_reply(&mut reply),
                    Err(error) => reply.write_exception(&error),
                }
                return Ok(reply);
            }
            if self.native
                && matches!(
                    call.code,
                    native::REGISTER_STAGED_APEX_OBSERVER
                        | native::UNREGISTER_STAGED_APEX_OBSERVER
                        | native::GET_STAGED_APEX_INFOS
                )
            {
                let system = self
                    .system
                    .as_ref()
                    .and_then(|owner| owner.upgrade())
                    .ok_or(QueryError::NotModelled(NotModelled(
                        "staging system owner unavailable",
                    )))?;
                let owner = match system.package_staging_owner() {
                    Ok(owner) => owner,
                    Err(error) => {
                        let mut reply = Parcel::new();
                        reply.write_exception(&error);
                        return Ok(reply);
                    }
                };
                return super::staging::transact(&owner, call)
                    .expect("staging method cohort")
                    .map_err(QueryError::Transport);
            }
            query
                .answer(self.descriptor(), call.code, &mut call.data)
                .map_err(QueryError::NotModelled)
        });
        Ok(match answer {
            Ok(reply) => reply,
            Err(QueryError::Transport(status)) => return Err(status),
            Err(QueryError::Original(error)) => { let mut reply = Parcel::new(); reply.write_exception(&error); reply },
            Err(QueryError::NotModelled(NotModelled(reason))) => {
                let mut reply = Parcel::new();
                reply.write_exception(&Exception::new(EX_UNSUPPORTED_OPERATION, reason));
                reply
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::{PackageState, PackageUserState, User};
    use super::*;
    use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, Reader};

    fn endpoints() -> (Arc<PackageQueries>, Arc<PackageQueries>) {
        let state = State {
            packages: [(
                "example.app".into(),
                PackageState {
                    name: "example.app".into(),
                    app_id: 10100,
                    users: [(
                        0,
                        PackageUserState {
                            enabled: 2,
                            ..Default::default()
                        },
                    )]
                    .into(),
                    ..Default::default()
                },
            )]
            .into(),
            users: [(
                0,
                User {
                    id: 0,
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        };
        PackageQueries::new(Arc::new(RwLock::new(Arc::new(state))))
    }

    fn call(service: &PackageQueries, code: u32, uid: u32, data: &Parcel) -> Reply {
        service.transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 77,
            sender_euid: uid,
            data: Reader::new(data.data(), &[]),
        })
    }

    #[test]
    fn runtime_version_checks_arguments_before_caller_permissions() {
        let state = State {
            packages: [(
                "caller".into(),
                PackageState {
                    name: "caller".into(),
                    app_id: 10100,
                    users: [(0, PackageUserState::default())].into(),
                    ..Default::default()
                },
            )]
            .into(),
            users: [(
                0,
                User {
                    id: 0,
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        };
        let (endpoint, _) = PackageQueries::new(Arc::new(RwLock::new(Arc::new(state))));
        for user in [-1, 0] {
            let mut data = Parcel::new();
            pm::GetRuntimePermissionsVersion { user_id: user }.write(&mut data);
            let reply = call(&endpoint, pm::GET_RUNTIME_PERMISSIONS_VERSION, 10100, &data).unwrap();
            let error =
                pm::read_get_runtime_permissions_version_reply(&mut Reader::new(reply.data(), &[]))
                    .unwrap()
                    .unwrap_err();
            assert_eq!(
                error.code,
                if user < 0 {
                    EX_ILLEGAL_ARGUMENT
                } else {
                    aim_binder_host::parcel::EX_SECURITY
                }
            );
        }
        let mut isolated = Parcel::new();
        pm::GetRuntimePermissionsVersion { user_id: 0 }.write(&mut isolated);
        let reply = call(
            &endpoint,
            pm::GET_RUNTIME_PERMISSIONS_VERSION,
            99000,
            &isolated,
        )
        .unwrap();
        assert_eq!(
            pm::read_get_runtime_permissions_version_reply(&mut Reader::new(reply.data(), &[]))
                .unwrap()
                .unwrap_err()
                .code,
            aim_binder_host::parcel::EX_SECURITY
        );
        let mut data = Parcel::new();
        pm::SetRuntimePermissionsVersion {
            version: -1,
            user_id: 0,
        }
        .write(&mut data);
        let reply = call(&endpoint, pm::SET_RUNTIME_PERMISSIONS_VERSION, 10100, &data).unwrap();
        assert_eq!(
            pm::read_set_runtime_permissions_version_reply(&mut Reader::new(reply.data(), &[]))
                .unwrap()
                .unwrap_err()
                .code,
            EX_ILLEGAL_ARGUMENT
        );
    }

    #[test]
    fn binder_identity_controls_package_visibility() {
        let (package, _) = endpoints();
        let mut args = Parcel::new();
        args.write_interface_token(pm::DESCRIPTOR);
        args.write_string16(Some("example.app"));
        args.write_i32(0);
        let reply = call(&package, pm::GET_APPLICATION_ENABLED_SETTING, 10100, &args).unwrap();
        let mut read = Reader::new(reply.data(), &[]);
        assert!(read.read_exception().unwrap().is_ok());
        assert_eq!(read.read_i32().unwrap(), 2);
        let reply = call(&package, pm::GET_APPLICATION_ENABLED_SETTING, 10101, &args).unwrap();
        assert_eq!(
            Reader::new(reply.data(), &[])
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_ILLEGAL_ARGUMENT
        );
    }

    #[test]
    fn endpoints_enforce_their_own_generated_interface() {
        let (package, package_native) = endpoints();
        assert_eq!(package.descriptor(), pm::DESCRIPTOR);
        assert_eq!(package_native.descriptor(), native::DESCRIPTOR);
        let mut args = Parcel::new();
        args.write_interface_token(pm::DESCRIPTOR);
        assert!(call(&package_native, native::GET_NAMES_FOR_UIDS, 0, &args).is_err());
        assert_eq!(
            call(&package, u32::MAX, 0, &args).unwrap_err(),
            UNKNOWN_TRANSACTION
        );
        let reply = call(&package, pm::SET_APPLICATION_ENABLED_SETTING, 10100, &args).unwrap();
        assert_eq!(
            Reader::new(reply.data(), &[])
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_UNSUPPORTED_OPERATION
        );
    }

    #[test]
    fn existing_binder_endpoints_follow_owner_publication() {
        let (package, package_native) = endpoints();
        let mut names = Parcel::new();
        names.write_interface_token(native::DESCRIPTOR);
        names.write_i32(1);
        names.write_i32(10100);
        let name = || {
            let reply = call(&package_native, native::GET_NAMES_FOR_UIDS, 0, &names).unwrap();
            let mut read = Reader::new(reply.data(), &[]);
            assert!(read.read_exception().unwrap().is_ok());
            assert_eq!(read.read_i32().unwrap(), 1);
            read.read_string16().unwrap()
        };
        assert_eq!(name().as_deref(), Some("example.app"));
        let Source::Fixture(state) = &package.source else {
            unreachable!()
        };
        let mut next = (**state.read().unwrap()).clone();
        next.packages.clear();
        *state.write().unwrap() = Arc::new(next);
        assert_eq!(name().as_deref(), Some(""));
        let mut enabled = Parcel::new();
        enabled.write_interface_token(pm::DESCRIPTOR);
        enabled.write_string16(Some("example.app"));
        enabled.write_i32(0);
        let reply = call(&package, pm::GET_APPLICATION_ENABLED_SETTING, 0, &enabled).unwrap();
        assert_eq!(
            Reader::new(reply.data(), &[])
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
            EX_ILLEGAL_ARGUMENT
        );
    }
}
