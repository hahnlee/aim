//! Production installer archiver/removal factory. Included from System so the
//! actual publisher, serialization gate and retained bridge remain shared.
use super::*;
use crate::package::installer::{archive_queries, archiver, removal};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

pub struct ArchiveOwners {
    pub installer: Arc<archiver::Owner>,
    pub queries: Arc<crate::package::archive::Owner>,
    pub removal: Arc<removal::Controller>,
    pub keep_uninstalled: Arc<crate::package::customization::Owner>,
}
impl System {
    pub fn configure_installer_archive(
        self: &Arc<Self>,
        bridge: &Arc<crate::package::bootstrap::Bridge>,
        native_store: Arc<removal::NativeStore>,
        removal_bridge: aim_binder_host::local::Strong,
        query_bridge: aim_binder_host::local::Strong,
        data: PathBuf,
        density: i32,
        storage_manager_package: Option<String>,
    ) -> Result<ArchiveOwners> {
        self.check_package_bootstrap(bridge)?;
        let installer = self
            .package_bootstrap
            .lock()
            .unwrap()
            .current
            .as_ref()
            .filter(|current| Arc::ptr_eq(&current.bridge, bridge))
            .and_then(|current| current.installer.as_ref().map(|(owner, _)| owner.clone()))
            .ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "archive installer owner unavailable",
                )
            })?;
        let external = removal::External::new(removal_bridge, self.process.clone());
        let system = Arc::downgrade(self);
        let retained = bridge.clone();
        let guard: Arc<dyn Fn() -> Result<()> + Send + Sync> = Arc::new(move || {
            let system = system.upgrade().ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "archive system stopped",
                )
            })?;
            system.check_package_bootstrap(&retained)
        });
        let leaf = archive_queries::Leaf::new(query_bridge, guard.clone());
        let query_owner = leaf.owner(&data, density);
        let system = Arc::downgrade(self);
        let retained = bridge.clone();
        let source = Arc::new(move || {
            let system = system.upgrade().ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "archive system stopped",
                )
            })?;
            system.check_package_bootstrap(&retained)?;
            Ok(system.capture_package_queries()?.state().clone())
        });
        let keep = self.package_customization()?;
        let policy_external = external.clone();
        let policy_installer = Arc::downgrade(&installer);
        let retained_keep = keep.clone();
        let policy: removal::PolicySource = Arc::new(move |query, request| {
            let device = policy_installer.upgrade().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native archive installer owner closed"))?.archiver_device_policy(request.uid, request.user)?;
            let users = policy_external.users()?;
            let mut admins = BTreeSet::new();
            let mut protected = BTreeSet::new();
            let mut restricted = BTreeSet::new();
            let mut children = BTreeMap::new();
            let security = query.state.system.security_policy.as_ref().ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "native removal protection owner unavailable",
                )
            })?;
            for &user in &users {
                if policy_external.active_admin(&request.package, user)? {
                    admins.insert(user);
                }
                if security.protected(user, Some(&request.package)) {
                    protected.insert(user);
                }
                if policy_external.restricted(user)? {
                    restricted.insert(user);
                }
                children.insert(user, policy_external.children(user)?);
            }
            let roles = query.state.system.roles.as_ref().ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "native removal known-package owner unavailable",
                )
            })?;
            let verifiers = roles
                .known_packages(query, 4, request.user)
                .map_err(crate::package::installer::policy::unknown)?
                .into_iter()
                .flatten()
                .collect();
            let uninstaller = roles
                .known_packages(query, 3, request.user)
                .map_err(crate::package::installer::policy::unknown)?
                .into_iter()
                .flatten()
                .next();
            let um = query.state.system.user_policy.as_ref().ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "native removal role owner unavailable",
                )
            })?;
            let caller = query
                .packages_for_uid(request.uid as i32)
                .map_err(crate::package::installer::policy::unknown)?
                .unwrap_or_default();
            let has_role = |role: &str| -> Result<bool> {
                let holders = um.role_holders(role, request.user)?;
                Ok(caller.iter().flatten().any(|name| holders.contains(name)))
            };
            let independence = policy_external.sdk_library_independence()?;
            Ok(removal::Policy {
                device,
                can_silently_install: policy_external
                    .silent(request.uid, request.caller_package.as_deref())?,
                emergency_installer: has_role("android.app.role.EMERGENCY")?,
                system_protection_role: has_role("android.app.role.SYSTEM_PROTECTION")?,
                pinned: policy_external.pinned(&request.package)?,
                admins,
                protected,
                uninstall_restricted: restricted,
                users,
                child_users: children,
                verifier_packages: verifiers,
                uninstaller_package: uninstaller,
                storage_manager_package: storage_manager_package.clone(),
                keep_uninstalled: retained_keep.should_keep(Some(&request.package)),
                sdk_library_independence: independence,
            })
        });
        let system = Arc::downgrade(self);
        let retained = bridge.clone();
        let factory_restore = Arc::new(move |plan: &removal::Plan| {
            let system = system.upgrade().ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "factory restore system stopped",
                )
            })?;
            system.restore_installer_factory_package(&retained, plan)
        });
        let weak = Arc::downgrade(self);
        let retained = bridge.clone();
        let preferred_removal: removal::PreferredRemoval = Arc::new(move |name, user, keep_data| {
            let system = weak.upgrade().ok_or_else(|| Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE, "removed preferred system stopped"))?;
            let effects = system.clear_removed_package_preferred(&retained, name, user, keep_data)?;
            let weak = Arc::downgrade(&system);
            let retained = retained.clone();
            Ok(removal::PostCommitEffect::Deferred(Arc::new(move || {
                let system = weak.upgrade().ok_or_else(|| Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE, "removed preferred completion stopped"))?;
                system.finish_removed_package_preferred(&retained, effects.clone())
            })))
        });
        let removal = Arc::new(removal::Controller {
            store: native_store.clone(),
            source: source.clone(),
            resolver: Default::default(),
            effects: self.package_effects_owner(bridge)?,
            external: external.clone(),
            policy,
            publisher: self.package_installer_publisher()?,
            factory_restore,
            preferred_removal,
            gate: self.package_install_lock.clone(),
            keystore: Arc::new(crate::package::owner::keystore::KeystoreCleanup::new(
                self.process.clone(),
            )),
        });
        let draft_installer = Arc::downgrade(&installer);
        let draft_source = source.clone();
        let draft_query = query_owner.clone();
        let draft_leaf = leaf.clone();
        let draft_bridge = bridge.clone();
        let create = Arc::new(move |name: &str, store: &str, user: i32, caller: &str| {
            let state = draft_source()?;
            let resolver = crate::package::resolve::Resolver::default();
            let resolution = resolver.resolution(&state).map_err(|error| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("draft resolver {error:?}"),
                )
            })?;
            let query = crate::package::query::Query {
                state: &state,
                filter: &resolution.apps_filter,
                calling_uid: 1000,
            };
            let launcher = draft_bridge
                .preferred_role_holder("android.app.role.HOME", user)
                .map_err(|error| {
                    Exception::new(
                        aim_binder_host::parcel::EX_ILLEGAL_STATE,
                        format!("draft role {error:?}"),
                    )
                })?;
            let launcher_uid = launcher
                .as_deref()
                .map(|name| query.package_uid_internal(name, 0, user, 1000))
                .transpose()
                .map_err(crate::package::installer::policy::unknown)?
                .unwrap_or(1000);
            let overlay =
                draft_query.overlay_enabled(launcher_uid, launcher.as_deref().unwrap_or(caller))?;
            let package = state
                .packages
                .get(name)
                .ok_or_else(|| archiver::name_error("archived draft package missing"))?;
            let mut params = draft_leaf.draft_params(
                name,
                user,
                launcher_uid,
                launcher.as_deref().unwrap_or(caller),
            )?;
            if let Some(icon) = draft_query.icon(package, user, overlay)? {
                use aim_service_aidl::WriteParcelable;
                let mut parcel = aim_binder_host::parcel::Parcel::new();
                parcel.write_string16(Some("android.graphics.Bitmap"));
                icon.write_to(&mut parcel);
                params.app_icon = Some(crate::package::installer::codec::Object {
                    bytes: parcel.data().to_vec(),
                    objects: parcel.objects().to_vec(),
                });
            }
            draft_installer.upgrade().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native archive installer owner closed"))?.archiver_record(params, store, user)
        });
        let save_installer = Arc::downgrade(&installer);
        let save = Arc::new(move || save_installer.upgrade().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native archive installer owner closed"))?.archiver_save_sessions());
        let drafts =
            archiver::Drafts::new(installer.sessions.clone(), create, save, Arc::downgrade(&(installer.clone() as Arc<dyn crate::package::installer::SessionOperations>)));
        let record_installer = Arc::downgrade(&installer);
        let archived_record = Arc::new(move |params, store: &str, user, uid| {
            record_installer.upgrade().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native archive installer owner closed"))?.archived_install_record(params, store, user, uid)
        });
        let stage_installer = Arc::downgrade(&installer);
        let prepare = Arc::new(
            move |session: &crate::package::installer::Session,
                  record: &crate::package::installer::Record,
                  _archived: &archiver::ArchivedPackage| {
                stage_installer.upgrade().ok_or_else(|| Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, "native archive installer owner closed"))?.archiver_prepare_stage(session, record)
            },
        );
        let archiver = Arc::new(archiver::Owner {
            store: native_store,
            external,
            removal: removal.clone(),
            drafts,
            install: installer.archiver_installation_owner()?,
            archived_record,
            prepare_archived_session: prepare,
        });
        installer.configure_archiver(archiver.clone())?;
        self.install_public_package_removal(bridge, removal.clone())?;
        self.install_archive_query_owner(bridge, query_owner.clone())?;
        Ok(ArchiveOwners {
            installer: archiver,
            queries: query_owner,
            removal,
            keep_uninstalled: keep,
        })
    }
}
