//! Query projection from the exact native scan capture (#951).
use super::Snapshot;
use crate::package::{model, restrictions, settings};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Owners outside the package scan. Every scoped package identity is checked
/// before these permission, domain and compatibility values can be published.
#[derive(Clone, Debug)]
pub struct PackageInputs {
    pub app_id: i32,
    pub path: String,
    pub version: i64,
    pub installed_permissions: Vec<String>,
    pub domain_verification: Option<(String, Vec<(String, i32)>)>,
    pub uri_relative_filter_groups: Vec<(
        String,
        Vec<crate::package::intent_filter::UriRelativeFilterGroup>,
    )>,
    pub filter_application_query: bool,
    pub syncable_authorities: Vec<(String, String)>,
    pub users: BTreeMap<i32, UserInputs>,
}
#[derive(Clone, Debug)]
pub struct UserInputs {
    pub gids: Vec<i32>,
    pub granted_permissions: Vec<String>,
    pub domain_selection: Option<(bool, Vec<(String, i32)>)>,
}
#[derive(Clone, Debug)]
pub struct Context {
    pub scan_version: u64,
    pub native_domains: Option<Arc<NativeDomains>>,
    pub nonce: Option<i64>,
    pub system: model::System,
    pub platform: model::Platform,
    pub users: BTreeMap<i32, model::User>,
    pub apex_inventory: crate::package::bootstrap::ApexInventory,
    pub scan_users: crate::package::bootstrap::ScanUsers,
    pub cross_user_suspensions: bool,
    pub packages: BTreeMap<(String, bool), PackageInputs>,
    pub retained_packages: BTreeMap<(i32, String), PackageInputs>,
}

/// Immutable domain owner retained with the scan/query generation.
#[derive(Clone, Debug)]
pub struct NativeDomains {
    boot: crate::package::domain_verification::owner::Boot,
    config: crate::package::system_config::SystemConfig,
    policies: BTreeMap<String, bool>,
}
impl NativeDomains {
    pub fn collector_policy(
        &self,
        name: &str,
    ) -> Result<crate::package::domain_verification::collector::Policy, String> {
        Ok(crate::package::domain_verification::collector::Policy {
            restrict_domains: *self
                .policies
                .get(name)
                .ok_or("missing current domain policy")?,
            linked_app: self.config.linked_apps.iter().any(|linked| linked == name),
        })
    }

    pub fn owner(&self) -> &crate::package::domain_verification::owner::Owner {
        &self.boot.owner
    }
    pub fn verification(
        &self,
        name: &str,
        code: &super::super::pkg::AndroidPackage,
    ) -> Result<Option<(String, Vec<(String, i32)>)>, String> {
        let restrict = self
            .policies
            .get(&code.package_name)
            .ok_or("missing current domain policy")?;
        self.owner()
            .verification(name, code, *restrict, &self.config)
    }
    pub fn user_state(
        &self,
        name: &str,
        code: &super::super::pkg::AndroidPackage,
        user: i32,
    ) -> Result<Option<(String, bool, Vec<(String, i32)>)>, String> {
        let restrict = self
            .policies
            .get(&code.package_name)
            .ok_or("missing current domain policy")?;
        self.owner()
            .user_state(name, code, *restrict, &self.config, user)
    }
    pub fn changes(&self) -> &[(String, crate::package::domain_verification::owner::Change)] {
        &self.boot.changes
    }
    pub fn owners(
        &self,
        scan: &crate::package::scan::SigningScan,
        host: &str,
        user: i32,
        mut settings_v2: impl FnMut(&str, i32) -> Result<bool, String>,
    ) -> Result<Vec<(String, bool)>, crate::package::domain_verification::owner::OwnersError> {
        self.owner().owners(host, user, |name| {
            if !scan
                .settings
                .packages
                .iter()
                .any(|setting| setting.name == name)
            {
                return Ok(None);
            }
            let code = &scan
                .loaded_packages()
                .get(name)
                .ok_or("missing current domain approval code")?
                .package;
            let user = scan
                .scanned_user_states(name)
                .and_then(|users| users.get(&user));
            if !crate::package::domain_verification::owner::approval_eligible(code, user) {
                return Ok(None);
            }
            Ok(Some(
                crate::package::domain_verification::owner::ApprovalInput {
                    code,
                    user,
                    settings_v2: settings_v2(name, code.target_sdk_version)?,
                    policy: crate::package::domain_verification::collector::Policy {
                        restrict_domains: *self
                            .policies
                            .get(name)
                            .ok_or("missing current domain policy")?,
                        linked_app: self.config.linked_apps.iter().any(|linked| linked == name),
                    },
                },
            ))
        })
    }
}

impl Context {
    pub fn attach_boot_domains(
        self,
        scan: &crate::package::scan::SigningScan,
        boot: crate::package::domain_verification::owner::Boot,
        config: &crate::package::system_config::SystemConfig,
        policies: &BTreeMap<String, bool>,
    ) -> Result<Self, String> {
        let mut context = self.resolve_domains(scan, &boot.owner, config, policies)?;
        context.native_domains = Some(Arc::new(NativeDomains {
            boot,
            config: config.clone(),
            policies: policies.clone(),
        }));
        Ok(context)
    }

    /// Replace current-package domain inputs from the native attached owner.
    /// Factory/retained domains remain separate until their owner is implemented.
    pub fn resolve_domains(
        mut self,
        owner: &crate::package::scan::SigningScan,
        domains: &crate::package::domain_verification::owner::Owner,
        config: &crate::package::system_config::SystemConfig,
        policies: &BTreeMap<String, bool>,
    ) -> Result<Self, String> {
        self.native_domains = None;
        let expected: BTreeSet<_> = owner
            .settings
            .packages
            .iter()
            .map(|s| s.name.clone())
            .collect();
        if policies.keys().cloned().collect::<BTreeSet<_>>() != expected {
            return Err("domain compatibility inventory differs".into());
        }
        for setting in &owner.settings.packages {
            let code = owner
                .loaded_packages()
                .get(&setting.name)
                .ok_or("missing domain query code")?;
            let attached = domains
                .package(&setting.name)
                .ok_or("missing attached query domain owner")?;
            if !setting
                .domain_set_id
                .as_ref()
                .is_some_and(|id| id.eq_ignore_ascii_case(&attached.id))
            {
                return Err("query domain UUID owner differs".into());
            }
            let inputs = self
                .packages
                .get_mut(&(setting.name.clone(), false))
                .ok_or("missing domain query inputs")?;
            let values = domains
                .queries(
                    &code.package,
                    policies[&setting.name],
                    config,
                    inputs.users.keys().copied(),
                )?
                .ok_or("missing attached query domain values")?;
            inputs.domain_verification = values.verification;
            inputs.uri_relative_filter_groups = values.uri_relative_filter_groups;
            for (id, selection) in values.users {
                inputs
                    .users
                    .get_mut(&id)
                    .ok_or("domain query user disappeared")?
                    .domain_selection = Some(selection);
            }
        }
        Ok(self)
    }
}

/// A fully validated replacement, retaining its exact publication base.
pub struct DomainUpdate {
    pub(crate) base: Arc<Capture>,
    pub(crate) store: super::Store,
    pub(crate) capture: Arc<Capture>,
}
/// A runtime-only replacement: cannot be passed to the disk commit API.
pub struct RuntimeDomainUpdate {
    pub(crate) base: Arc<Capture>,
    pub(crate) store: super::Store,
    pub(crate) capture: Arc<Capture>,
}

/// Query code and the Java replica retain the same native package owner.
pub struct Capture {
    scan: Arc<Snapshot>,
    state: Arc<model::State>,
    domains: Option<Arc<NativeDomains>>,
    context: Arc<Context>,
}
impl Capture {
    pub fn scan(&self) -> &Arc<Snapshot> {
        &self.scan
    }
    pub fn state(&self) -> &Arc<model::State> {
        &self.state
    }
    pub fn domains(&self) -> Option<&Arc<NativeDomains>> {
        self.domains.as_ref()
    }
    pub fn prepare_domain_update(
        self: &Arc<Self>,
        owner: crate::package::domain_verification::owner::Owner,
    ) -> Result<DomainUpdate, String> {
        self.prepare_domains(owner, true)
    }
    pub fn prepare_runtime_domain_update(
        self: &Arc<Self>,
        owner: crate::package::domain_verification::owner::Owner,
    ) -> Result<RuntimeDomainUpdate, String> {
        let update = self.prepare_domains(owner, false)?;
        Ok(RuntimeDomainUpdate {
            base: update.base,
            store: update.store,
            capture: update.capture,
        })
    }
    fn prepare_domains(
        self: &Arc<Self>,
        owner: crate::package::domain_verification::owner::Owner,
        persist: bool,
    ) -> Result<DomainUpdate, String> {
        let current = self
            .domains
            .as_ref()
            .ok_or("missing captured domain owner")?;
        let version = self
            .scan
            .version()
            .checked_add(1)
            .ok_or("domain version exhausted")?;
        let mut scan_owner = self.scan.owner().clone();
        if persist {
            scan_owner.settings.domain_verification = owner.persisted();
        }
        let store =
            super::Store::new_replica_at_version(scan_owner, self.scan.usage().clone(), version)
                .map_err(|e| format!("domain scan validation: {e:?}"))?;
        let mut context = (*self.context).clone();
        context.scan_version = version;
        let mut boot = current.boot.clone();
        boot.owner = owner;
        let context = context.attach_boot_domains(
            store.capture().owner(),
            boot,
            &current.config,
            &current.policies,
        )?;
        let capture = Self::new(store.capture(), context)?;
        Ok(DomainUpdate {
            base: self.clone(),
            store,
            capture,
        })
    }

    pub fn new(scan: Arc<Snapshot>, mut context: Context) -> Result<Arc<Self>, String> {
        if context.scan_version != scan.version() {
            return Err("query context scan version differs".into());
        }
        super::validate_replica(&scan).map_err(|error| format!("query replica: {error:?}"))?;
        let source = Arc::new(context.clone());
        let domains = context.native_domains.take();
        if let Some(domains) = &domains {
            let expected = context.clone().resolve_domains(
                scan.owner(),
                domains.owner(),
                &domains.config,
                &domains.policies,
            )?;
            for ((name, factory), value) in &context.packages {
                if *factory {
                    continue;
                }
                let target = expected
                    .packages
                    .get(&(name.clone(), false))
                    .ok_or("foreign bound domain query package")?;
                if value.domain_verification != target.domain_verification
                    || value.uri_relative_filter_groups != target.uri_relative_filter_groups
                    || value.users.iter().any(|(id, user)| {
                        user.domain_selection != target.users[id].domain_selection
                    })
                {
                    return Err("bound native domain query projection differs".into());
                }
            }
        }

        if context
            .users
            .iter()
            .any(|(id, user)| *id < 0 || user.id != *id)
        {
            return Err("query user identity differs".into());
        }
        if context.system.sdk_sandbox_package.is_none() {
            return Err("missing query SDK sandbox selection".into());
        }
        let owner = scan.owner();
        let mut active = BTreeMap::new();
        let mut disabled = BTreeMap::new();
        for (settings, target, factory) in [
            (&owner.settings.packages, &mut active, false),
            (
                &owner.settings.disabled_system_packages,
                &mut disabled,
                true,
            ),
        ] {
            for setting in settings {
                let extra = context
                    .packages
                    .remove(&(setting.name.clone(), factory))
                    .ok_or_else(|| format!("missing query owners: {}/{factory}", setting.name))?;
                let package = package(&scan, setting, factory, extra, &context)?;
                if target.insert(setting.name.clone(), package).is_some() {
                    return Err("duplicate query package".into());
                }
            }
        }
        if !context.packages.is_empty() {
            return Err("foreign query package owners".into());
        }
        let mut shared_users = BTreeMap::new();
        for (name, group) in &owner.identities.shared_users {
            // Validate membership/retained owners through the facade's same gate.
            super::shared_record::captured(&scan, name)?.ok_or("missing query shared owner")?;
            let mut members = group
                .package_names()
                .filter(|name| group.has_package(name))
                .map(|name| {
                    active
                        .get(name)
                        .cloned()
                        .ok_or("missing query shared member")
                })
                .collect::<Result<Vec<_>, _>>()?;
            for (name, value) in group.retained_settings() {
                let extra = context
                    .retained_packages
                    .remove(&(group.app_id, name.into()))
                    .ok_or("missing retained shared query inputs")?;
                members.push(retained(owner, value, extra, &context)?);
            }
            shared_users.insert(
                name.clone(),
                model::SharedUser {
                    name: name.clone(),
                    app_id: group.app_id,
                    flags: group.flags,
                    private_flags: group.private_flags,
                    // The pinned SharedUserSetting snapshot copy omits this field.
                    seinfo_target_sdk_version: 0,
                    packages: group
                        .package_names()
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .map(str::to_owned)
                        .collect(),
                    native_packages: Some(members),
                    signatures: group.signatures.clone(),
                },
            );
        }
        let mut uid_owners = BTreeMap::new();
        for (id, slot) in owner.identities.ids.owners() {
            use crate::package::owner::app_ids::Owner;
            let value = match slot {
                Owner::Package(name) => model::UidOwner::Package(Box::new(
                    active
                        .get(name)
                        .cloned()
                        .ok_or("missing registered query package")?,
                )),
                Owner::SharedUser(name) => {
                    if !shared_users.contains_key(name) {
                        return Err("missing registered query shared user".into());
                    }
                    model::UidOwner::SharedUser(name.clone())
                }
                Owner::DetachedPackage(name) => {
                    let value = owner
                        .identities
                        .ids
                        .detached_setting(id)
                        .ok_or("missing detached query UID owner")?;
                    let extra = context
                        .retained_packages
                        .remove(&(id, name.clone()))
                        .ok_or("missing detached UID query inputs")?;
                    model::UidOwner::Package(Box::new(retained(owner, value, extra, &context)?))
                }
            };
            uid_owners.insert(id, value);
        }
        if !context.retained_packages.is_empty() {
            return Err("foreign retained query inputs".into());
        }
        let state = model::State {
            generation: scan.version(),
            nonce: context.nonce,
            packages: active,
            disabled_system_packages: disabled,
            shared_users,
            uid_owners: Some(uid_owners),
            // These records are original-feed import inputs, not query owners.
            shared_process_inputs: BTreeMap::new(),
            user_scopes: BTreeMap::new(),
            runtime_inputs: BTreeMap::new(),
            apex_inventory: Some(context.apex_inventory),
            scan_users: Some(context.scan_users),
            users: context.users,
            system: context.system,
            platform: context.platform,
        };
        Ok(Arc::new(Self {
            scan,
            state: Arc::new(state),
            domains,
            context: source,
        }))
    }
}

fn package(
    snapshot: &Snapshot,
    s: &settings::Package,
    factory: bool,
    extra: PackageInputs,
    context: &Context,
) -> Result<model::PackageState, String> {
    let owner = snapshot.owner();
    let runtime = owner
        .replica_runtime(&s.name, factory)?
        .ok_or("missing query runtime")?;
    let code = if factory {
        owner.disabled_loaded_packages()
    } else {
        owner.loaded_packages()
    }
    .get(&s.name);
    if !factory {
        let selected = extra
            .users
            .values()
            .any(|user| user.domain_selection.is_some());
        if selected {
            let package = code.ok_or("domain selection requires current package code")?;
            use crate::package::domain_verification::collector::{self, Kind, Policy};
            // Web-domain collection uses the modern rules even for legacy apps.
            let hosts = collector::collect(
                &package.package,
                Policy {
                    restrict_domains: true,
                    linked_app: false,
                },
                Kind::Web,
            );
            let expected: BTreeSet<_> = hosts.iter().map(String::as_str).collect();
            for user in extra.users.values() {
                if let Some((_, domains)) = &user.domain_selection {
                    let actual: BTreeSet<_> =
                        domains.iter().map(|(host, _)| host.as_str()).collect();
                    if actual != expected
                        || actual.len() != domains.len()
                        || domains.iter().any(|(_, state)| !(0..=2).contains(state))
                    {
                        return Err(
                            "query domain selection differs from current package code".into()
                        );
                    }
                }
            }
        }
    }
    let stored = if factory {
        owner.disabled_user_states(&s.name)
    } else {
        owner.scanned_user_states(&s.name)
    }
    .ok_or("missing query user states")?;
    project(
        owner,
        s,
        extra,
        context,
        runtime,
        code.map(Arc::as_ref),
        stored,
        owner
            .install_permissions_fixed(&s.name, factory)?
            .ok_or("missing query install permission owner")?,
        owner
            .hidden_api_enforcement_policy(&s.name, factory)?
            .ok_or("missing query hidden API owner")?,
    )
}
fn retained(
    owner: &crate::package::scan::SigningScan,
    value: &crate::package::owner::app_ids::DetachedSetting,
    extra: PackageInputs,
    context: &Context,
) -> Result<model::PackageState, String> {
    project(
        owner,
        &value.package,
        extra,
        context,
        value
            .runtime
            .as_ref()
            .ok_or("missing retained query runtime")?,
        None,
        &value.users,
        value
            .install_fixed
            .ok_or("missing retained query install permission owner")?,
        2,
    )
}
fn project(
    owner: &crate::package::scan::SigningScan,
    s: &settings::Package,
    extra: PackageInputs,
    context: &Context,
    runtime: &crate::package::scan::ReplicaRuntime,
    code: Option<&crate::package::scan::LoadedPackage>,
    stored: &BTreeMap<i32, restrictions::UserState>,
    fixed: bool,
    hidden: i32,
) -> Result<model::PackageState, String> {
    if (extra.app_id, extra.path.as_str(), extra.version)
        != (s.app_id, s.code_path.as_str(), s.version_code)
    {
        return Err(format!("query package identity differs: {}", s.name));
    }
    let pkg = code.map(|code| Arc::new(code.package.clone()));
    let parcel = code
        .map(|code| {
            code.package
                .to_cache_entry()
                .map(|entry| Arc::<[u8]>::from(entry.bytes))
        })
        .transpose()?;
    let ids: BTreeSet<_> = stored.keys().chain(context.users.keys()).copied().collect();
    if extra.users.keys().copied().collect::<BTreeSet<_>>() != ids {
        return Err(format!("query permission/domain users differ: {}", s.name));
    }
    let mut users = BTreeMap::new();
    for (id, external) in extra.users {
        // The original setting's absent sparse entry uses PackageUserStateDefault.
        let default = restrictions::UserState::default();
        let state = stored.get(&id).unwrap_or(&default);
        users.insert(
            id,
            user(
                state,
                id,
                context.cross_user_suspensions,
                stored.get(&id).is_none(),
                external,
            )?,
        );
    }
    let shared_user = s.shared_app_id().and_then(|id| {
        owner
            .identities
            .shared_users
            .iter()
            .find(|(_, group)| group.app_id == id)
            .map(|(name, _)| name.clone())
    });
    let private = |bit| s.private_flags & (1 << bit) != 0;
    Ok(model::PackageState {
        name: s.name.clone(),
        app_id: s.app_id,
        shared_user,
        shared_user_app_id: s.shared_app_id(),
        path: s.code_path.clone(),
        volume_uuid: s.volume_uuid.clone(),
        primary_cpu_abi: s.primary_cpu_abi.clone(),
        secondary_cpu_abi: s.secondary_cpu_abi.clone(),
        cpu_abi_override: s.cpu_abi_override.clone(),
        seinfo: runtime
            .override_seinfo
            .as_ref()
            .filter(|value| !value.is_empty())
            .or(runtime.seinfo.as_ref())
            .cloned(),
        version_code: s.version_code,
        target_sdk_version: s.target_sdk_version,
        category_override: s.category_hint,
        hidden_api_enforcement_policy: hidden,
        last_modified_time: s.last_modified_time,
        last_update_time: s.last_update_time,
        restrict_update_hash: s.restrict_update_hash.clone(),
        apex_module_name: s.transient.apex_module_name.clone(),
        mime_groups: s.mime_groups.clone(),
        uses_static_libraries: s.uses_static_libraries.clone(),
        uses_sdk_libraries: s.uses_sdk_libraries.clone(),
        uses_library_files: runtime.library_files.clone(),
        uses_library_infos: runtime.libraries.clone(),
        is: model::StateFlags {
            system: s.flags & settings::FLAG_SYSTEM != 0,
            privileged: private(3),
            oem: private(17),
            vendor: private(18),
            product: private(19),
            system_ext: private(21),
            odm: private(30),
            updated_system_app: s.transient.updated_system_app,
            apex: pkg
                .as_ref()
                .is_some_and(|pkg| pkg.is2(crate::package::pkg::booleans2::APEX)),
            apk_in_updated_apex: s.transient.apk_in_updated_apex,
            hidden_until_installed: s.transient.hidden_until_installed,
            default_to_device_protected_storage: private(5),
            force_queryable_override: s.force_queryable,
            scanned_as_stopped_system_app: s.scanned_as_stopped_system_app,
            update_available: s.update_available,
            install_permissions_fixed: fixed,
            pending_restore: s.pending_restore,
            debuggable: s.debuggable,
            loading: (1.0 - s.loading_progress).abs() >= 0.00000001,
        },
        signatures: s.signatures.clone(),
        install_source: model::InstallSource {
            installer: s.install_source.installer.clone(),
            installer_uid: s.install_source.installer_uid,
            initiating_package: s.install_source.initiating_package.clone(),
            originating_package: s.install_source.originating_package.clone(),
            update_owner: s.install_source.update_owner.clone(),
            installer_attribution_tag: s.install_source.installer_attribution_tag.clone(),
            initiating_package_signatures: s.install_source.initiating_package_signatures.clone(),
            package_source: s.install_source.package_source,
            is_orphaned: s.install_source.is_orphaned,
            initiating_package_uninstalled: s.install_source.initiating_package_uninstalled,
        },
        installed_permissions: extra.installed_permissions,
        domain_verification: extra.domain_verification,
        uri_relative_filter_groups: extra.uri_relative_filter_groups,
        filter_application_query: Some(extra.filter_application_query),
        syncable_authorities: extra.syncable_authorities,
        parcel,
        pkg,
        users,
    })
}
fn user(
    s: &restrictions::UserState,
    id: i32,
    cross_user: bool,
    default_user: bool,
    external: UserInputs,
) -> Result<model::PackageUserState, String> {
    Ok(model::PackageUserState {
        ce_data_inode: s.ce_data_inode,
        de_data_inode: s.de_data_inode,
        installed: s.installed,
        stopped: s.stopped,
        not_launched: s.not_launched,
        hidden: s.hidden,
        instant_app: s.instant_app,
        virtual_preload: s.virtual_preload,
        quarantined: s
            .is_quarantined(id, cross_user)
            .map_err(|_| "null query suspension params")?,
        distraction_flags: s.distraction_flags,
        suspended_by: s
            .resolved_suspensions(id, cross_user)
            .iter()
            .map(|(_, suspension)| suspension.package.clone())
            .collect(),
        enabled: s.enabled,
        last_disable_app_caller: s.last_disable_app_caller.clone(),
        enabled_components: s.enabled_components.clone().unwrap_or_default(),
        disabled_components: s.disabled_components.clone().unwrap_or_default(),
        install_reason: s.install_reason,
        uninstall_reason: s.uninstall_reason,
        harmful_app_warning: s.harmful_app_warning.clone(),
        splash_screen_theme: s.splash_screen_theme.clone(),
        first_install_time: s.first_install_time,
        min_aspect_ratio: s.min_aspect_ratio,
        archive_state: s.archive_state.clone(),
        overlay_paths: s.runtime.all_overlay_paths(),
        component_label_icon_overrides: s
            .runtime
            .overrides()
            .into_iter()
            .flatten()
            .map(|(component, value)| (component.class.clone(), value.label.clone(), value.icon))
            .collect(),
        // PackageUserState.DEFAULT reports dataExists=true; an explicit
        // PackageUserStateImpl instead derives it from its inode owners.
        data_exists: default_user || s.ce_data_inode > 0 || s.de_data_inode > 0,
        gids: external.gids,
        granted_permissions: external.granted_permissions,
        domain_selection: external.domain_selection,
    })
}
