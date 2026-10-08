//! Query projection from the exact native scan capture (#951).
#[path="isolated_delta.rs"]
mod isolated_delta;
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
    pub boot_classes: Option<Arc<aim_android_image::linkage::Hierarchy>>,
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
    classes: Option<Arc<aim_android_image::linkage::Hierarchy>>,
    config: crate::package::system_config::SystemConfig,
    policies: BTreeMap<String, bool>,
}
impl NativeDomains {
    pub fn classes(&self) -> Option<&aim_android_image::linkage::Hierarchy> {
        self.classes.as_deref()
    }

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
    /// Called with the sole disk owner before constructing a query capture.
    pub fn with_uninstall_blocks(mut self, state: &crate::package::State) -> Self {
        self.system.uninstall_blocks = Some(Arc::new(
            crate::package::mutations::UninstallBlocks::from_state(state),
        ));
        self
    }

    /// Load immutable class relations from the same image used by native scanning.
    pub fn with_boot_classpath(mut self, image: &std::path::Path) -> Result<Self, String> {
        use aim_android_image::{classpath, linkage::ClassPath};
        let jars = classpath::jars(image, "bootclasspath.pb", classpath::BOOTCLASSPATH)?;
        self.boot_classes = Some(Arc::new(ClassPath::read(image, &jars)?.hierarchy()?));
        Ok(self)
    }

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
            classes: context.boot_classes.clone(),
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
        self.system.system_permissions = Some(config.system_permissions.clone());
        let mut initial: Vec<_> = config
            .initial_non_stopped_system_packages
            .iter()
            .cloned()
            .collect();
        initial.sort_by_key(|name| crate::package::info::java_hash(name));
        self.system.initial_non_stopped_system_packages = Some(initial);
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
                .ok_or_else(|| format!("missing attached query domain owner for {}", setting.name))?;
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

pub(crate) struct PackageUpdate {
    pub(crate) store: super::Store,
    pub(crate) capture: Arc<Capture>,
}

/// Query code and the Java replica retain the same native package owner.
pub struct Capture {
    scan: Arc<Snapshot>,
    state: Arc<model::State>,
    domains: Option<Arc<NativeDomains>>,
    context: Arc<Context>,
    frozen: Option<BTreeMap<String, i32>>,
    resolver: crate::package::resolve::Resolver,
}
impl Capture {
    /// One resolver per immutable native capture. Live permission/profile and
    /// settings leaves are still called by query policy at request time.
    /// Publishing any new Capture allocates a fresh cache; retained captures
    /// keep their own bounded component/AppsFilter index.
    pub(crate) fn resolver(&self)->&crate::package::resolve::Resolver{&self.resolver}
    pub fn resolution(&self) -> Result<Arc<crate::package::resolve::Resolution>, crate::package::component_resolver::MimeGroupError> {
        self.resolver.resolution(&self.state)
    }

    pub fn frozen_packages(&self) -> Result<&BTreeMap<String, i32>, crate::package::apps_filter::NotModelled> {
        if self.state.system.lifecycle.as_ref().is_some_and(|owner| owner.check_frozen_publication().is_err()) {
            return Err(crate::package::apps_filter::NotModelled("freezer generation publication failed"));
        }
        self.frozen.as_ref().ok_or(crate::package::apps_filter::NotModelled("captured frozen package owner unavailable"))
    }
    /// Root commits a scan generation first, then atomically publishes this
    /// prepared capture and its Java version page. Existing captures retain their map.
    pub fn with_frozen_packages(
        self: &Arc<Self>,
        scan: Arc<Snapshot>,
        frozen: BTreeMap<String, i32>,
    ) -> Result<Arc<Self>, String> {
        if scan.version() <= self.scan.version() {
            return Err("freezer publication requires a new scan generation".into());
        }
        if frozen.values().any(|count| *count <= 0) {
            return Err("freezer publication contains an invalid reference count".into());
        }
        let mut context=(*self.context).clone();
        context.scan_version=scan.version();
        let mut capture = Self::new(scan, context)?;
        Arc::get_mut(&mut capture).unwrap().frozen = Some(frozen);
        Ok(capture)
    }
    pub(crate) fn with_frozen_view(self: &Arc<Self>, frozen: BTreeMap<String, i32>) -> Result<Arc<Self>, String> {
        if frozen.values().any(|count| *count <= 0) {
            return Err("freezer publication contains an invalid reference count".into());
        }
        let mut capture = Self::new(self.scan.clone(), (*self.context).clone())?;
        Arc::get_mut(&mut capture).unwrap().frozen = Some(frozen);
        Ok(capture)
    }
    pub fn scan(&self) -> &Arc<Snapshot> {
        &self.scan
    }
    pub fn state(&self) -> &Arc<model::State> {
        &self.state
    }
    pub(crate) fn context(&self) -> &Arc<Context> {
        &self.context
    }
    pub(crate) fn permission_upgrade_needed(&self, user: i32) -> Result<bool, crate::package::apps_filter::NotModelled> {
        let owner = self.state.system.runtime_permission_queries.as_ref()
            .ok_or(crate::package::apps_filter::NotModelled("runtime permission metadata query unavailable"))?;
        (owner.0)(user).map_err(|_| crate::package::apps_filter::NotModelled("runtime permission metadata query failed"))
    }
    pub(crate) fn settings_read_messages(&self) -> Result<String, crate::package::apps_filter::NotModelled> {
        self.state.system.settings_read_messages.as_ref().map(|owner| owner.text().to_owned())
            .ok_or(crate::package::apps_filter::NotModelled("native Settings reader messages unavailable"))
    }
    pub(crate) fn maintenance_diagnostic_text(&self, kind: i32, package: Option<&str>) -> Result<String, crate::package::apps_filter::NotModelled> {
        self.state.system.maintenance_diagnostics.as_ref().ok_or(
            crate::package::apps_filter::NotModelled("native maintenance diagnostic owner unavailable"))?
            .text(&self.state, kind, package)
    }
    pub(crate) fn prepare_compatibility_mode(self: &Arc<Self>, enabled: bool) -> Result<PackageUpdate, String> {
        let update = self.prepare_package_update(self.scan.owner().clone())?;
        let mut context = (*update.capture.context).clone();
        context.system.compatibility_mode = enabled;
        let capture = Self::new(update.store.capture(), context)?;
        Ok(PackageUpdate { store: update.store, capture })
    }
    pub(crate) fn prepare_web_instant_policy(self: &Arc<Self>, policy: crate::package::web_instant_state::Snapshot) -> Result<PackageUpdate, String> {
        // Epoch zero is the original empty constructor SparseBooleanArray.
        // Every provider-backed publication retains the complete UM guard.
        if !(policy.epoch == 0 && policy.disabled.is_empty())
            && policy.disabled.keys().copied().collect::<BTreeSet<_>>() != self.state.users.keys().copied().collect() {
            return Err("web instant policy user inventory differs".into());
        }
        if self.state.system.web_instant_policy.as_ref().is_some_and(|previous| policy.epoch <= previous.epoch) {
            return Err("web instant policy epoch did not advance".into());
        }
        let update = self.prepare_package_update(self.scan.owner().clone())?;
        let mut context = (*update.capture.context).clone();
        context.system.web_instant_policy = Some(Arc::new(policy));
        let capture = Self::new(update.store.capture(), context)?;
        Ok(PackageUpdate { store: update.store, capture })
    }

    pub(crate) fn prepare_archive_owner(self: &Arc<Self>, owner: Arc<crate::package::archive::Owner>) -> Result<PackageUpdate, String> {
        let update = self.prepare_package_update(self.scan.owner().clone())?;
        let mut context = (*update.capture.context).clone();
        context.system.archive_owner = Some(owner);
        let capture = Self::new(update.store.capture(), context)?;
        Ok(PackageUpdate { store: update.store, capture })
    }
    pub(crate) fn prepare_launch_owner(self: &Arc<Self>, owner: Arc<crate::package::launch::Owner>) -> Result<PackageUpdate, String> {
        let update = self.prepare_package_update(self.scan.owner().clone())?;
        let mut context = (*update.capture.context).clone();
        context.system.launch_sender = Some(owner);
        let capture = Self::new(update.store.capture(), context)?;
        Ok(PackageUpdate { store: update.store, capture })
    }
    pub fn preferred_record_tokens(&self, user: i32, kind: i32) -> Result<Option<Vec<aim_binder_host::parcel::Binder>>, crate::package::apps_filter::NotModelled> {
        self.state.system.preferred_owner.as_ref().ok_or(
            crate::package::apps_filter::NotModelled("preferred record owner unavailable"))?
            .captured.record_tokens(user, kind).map_err(|_|
                crate::package::apps_filter::NotModelled("preferred record lease unavailable"))
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
    /// Merge a completed persistence read against this exact scan/code/policy
    /// capture. Preparation is unpublished and does not claim a settings write.
    pub fn prepare_domain_settings_read(
        self: &Arc<Self>,
        result: crate::package::domain_verification::ReadResult,
    ) -> Result<
        (
            RuntimeDomainUpdate,
            Vec<crate::package::domain_verification::SectionError>,
        ),
        String,
    > {
        use crate::package::domain_verification::collector::{self, Kind};
        let current = self
            .domains
            .as_ref()
            .ok_or("missing captured domain owner")?;
        let mut owner = current.owner().clone();
        let diagnostics = owner.read_settings(result, |name| {
            let Some(code) = self.scan.owner().loaded_packages().get(name) else {
                return Ok(Vec::new());
            };
            Ok(collector::collect(
                &code.package,
                current.collector_policy(name)?,
                Kind::ValidAutoVerify,
            ))
        })?;
        Ok((self.prepare_runtime_domain_update(owner)?, diagnostics))
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
            scan_owner.settings.domain_verification = owner.xml_projection();
        }
        let store =
            super::Store::new_replica_after(&self.scan, scan_owner, self.scan.usage().clone(), version)
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

    /// Prepare a complete replacement before any native mutation commits disk.
    pub(crate) fn prepare_shared_package_snapshot(self:&Arc<Self>,next:Arc<Snapshot>)->Result<Arc<Self>,String> {
        if next.version()!=self.scan.version().checked_add(1).ok_or("package generation exhausted")?{return Err("shared snapshot generation differs".into());}
        self.prepare_committed_package_snapshot(next)
    }

    pub(crate) fn prepare_committed_package_snapshot(self:&Arc<Self>,next:Arc<Snapshot>)->Result<Arc<Self>,String> {
        if next.version()<self.scan.version(){return Err("committed snapshot generation regressed".into());}
        let mut context=(*self.context).clone();context.scan_version=next.version();
        let active=next.owner().settings.packages.iter().map(|package|package.name.as_str()).collect::<BTreeSet<_>>();
        let factory=next.owner().settings.disabled_system_packages.iter().map(|package|package.name.as_str()).collect::<BTreeSet<_>>();
        // Re-enabling a retained factory must bind its own scoped code inputs,
        // rather than retaining the removed data package's path/version.
        for setting in &next.owner().settings.packages {
            let key=(setting.name.clone(),false);
            if context.packages.get(&key).is_some_and(|inputs|inputs.path!=setting.code_path||inputs.version!=setting.version_code||inputs.app_id!=setting.app_id) {
                let mut inputs=context.packages.get(&(setting.name.clone(),true)).cloned().ok_or("replacement query owners unavailable")?;
                if inputs.path!=setting.code_path||inputs.version!=setting.version_code||inputs.app_id!=setting.app_id{return Err("factory query code identity differs".into());}
                if let Some(previous)=context.packages.get(&key){inputs.users=previous.users.clone();}
                context.packages.insert(key,inputs);
            }
        }
        context.packages.retain(|(name,disabled),_|if *disabled{factory.contains(name.as_str())}else{active.contains(name.as_str())});
        context.retained_packages.retain(|(_,name),_|active.contains(name.as_str()));
        if let Some(domains)=context.native_domains.as_ref(){
            let mut domains=(**domains).clone();
            domains.boot.owner.rebase_persisted_projection(
                &self.scan.owner().settings.domain_verification,
                next.owner().settings.domain_verification.clone(),
                &active,
            )?;
            domains.policies.retain(|name,_|active.contains(name.as_str()));
            context=context.resolve_domains(next.owner(),domains.owner(),&domains.config,&domains.policies)?;
            context.native_domains=Some(Arc::new(domains));
        }
        Self::new(next,context)
    }
    pub(crate) fn prepare_user_transition(self: &Arc<Self>, mut owner: crate::package::scan::SigningScan,
        user: i32, created: bool, record: Option<crate::package::user_operations::UserRecord>,
        bridge: &crate::package::bootstrap::Bridge,
    ) -> Result<PackageUpdate, String> {
        let version = self.scan.version().checked_add(1).ok_or("user package generation exhausted")?;
        let mut context = (*self.context).clone();
        context.scan_version = version;
        let users = context.scan_users.users.as_mut().ok_or("native scan user inventory unavailable")?;
        if created {
            let record = record.ok_or("original new user metadata unavailable")?;
            if record.user.id != user || record.scan.id != user || context.users.contains_key(&user)
                || users.iter().any(|entry| entry.id == user) { return Err("new user metadata identity differs".into()); }
            context.users.insert(user, record.user);
            users.push(record.scan);
            users.sort_by_key(|entry| entry.id);
        } else {
            if record.is_some() { return Err("removed user carries creation metadata".into()); }
            context.users.remove(&user);
            users.retain(|entry| entry.id != user);
        }
        owner.transition_permission_user_inventory(user, created, bridge)?;
        let store = super::Store::new_replica_after(&self.scan, owner, self.scan.usage().clone(), version)
            .map_err(|error| format!("user transition scan: {error:?}"))?;
        let scan = store.capture();
        for ((name, factory), inputs) in &mut context.packages {
            let settings = if *factory { &scan.owner().settings.disabled_system_packages } else { &scan.owner().settings.packages };
            let setting = settings.iter().find(|setting| setting.name == *name).ok_or("user transition package owner disappeared")?;
            if created {
                let code=if *factory {scan.owner().disabled_loaded_packages()} else {scan.owner().loaded_packages()}.get(name);
                let permissions=match super::boot_context::permission_uid_owner(setting,code.map(|code|code.runtime_package()))? {
                    None => UserInputs {gids:Vec::new(),granted_permissions:Vec::new(),domain_selection:None},
                    Some(app_id) => {
                        let live=bridge.legacy_permissions(app_id,&[user]).map_err(|error|format!("new user actual UID permission owner: {error:?}"))?;
                        let state=live.user(user).ok_or("new user live UID projection unavailable")?;
                        let stored=if *factory {scan.owner().disabled_user_states(name)} else {scan.owner().scanned_user_states(name)}
                            .ok_or("new user package state owner unavailable")?;
                        let mut granted_permissions=Vec::new();
                        if stored.get(&user).is_none_or(|state|state.installed) {
                            for permission in &state.permissions {if permission.granted {
                                granted_permissions.push(permission.name.clone().ok_or("new user granted permission identity null")?);
                            }}
                        }
                        let gids=bridge.permission_gids(app_id,&[user]).map_err(|error|format!("new user actual UID GIDs: {error:?}"))?
                            .into_iter().map(|gid|gid as i32).collect();
                        UserInputs {gids,granted_permissions,domain_selection:None}
                    }
                };
                inputs.users.insert(user,permissions);
            } else { inputs.users.remove(&user); }
        }
        for ((app_id, name), inputs) in &mut context.retained_packages {
            if created {
                let permissions = crate::package::owner::legacy_permissions::retained_user::capture_retained_user(
                    scan.owner(), bridge, *app_id, name, user)?;
                inputs.users.insert(user, permissions);
            } else { inputs.users.remove(&user); }
        }
        if let Some(domains) = context.native_domains.clone() {
            let mut domains = (*domains).clone();
            if !created { domains.boot.owner.clear_user(user); }
            context = context.resolve_domains(scan.owner(), domains.owner(), &domains.config, &domains.policies)?;
            context.native_domains = Some(Arc::new(domains));
        }
        let capture = Self::new(scan, context)?;
        Ok(PackageUpdate { store, capture })
    }

    pub(crate) fn prepare_internal_mutation(self: &Arc<Self>, owner: crate::package::scan::SigningScan,
        usage: crate::package::owner::usage::Usage,
    ) -> Result<PackageUpdate, String> {
        let version = self.scan.version().checked_add(1).ok_or("internal mutation generation exhausted")?;
        let store = super::Store::new_replica_after(&self.scan, owner, usage, version)
            .map_err(|error| format!("internal mutation scan: {error:?}"))?;
        let mut context = (*self.context).clone();
        context.scan_version = version;
        let capture = Self::new(store.capture(), context)?;
        Ok(PackageUpdate { store, capture })
    }

    pub(crate) fn prepare_package_update(
        self: &Arc<Self>,
        owner: crate::package::scan::SigningScan,
    ) -> Result<PackageUpdate, String> {
        let version = self
            .scan
            .version()
            .checked_add(1)
            .ok_or("package version exhausted")?;
        let store = super::Store::new_replica_after(&self.scan, owner, self.scan.usage().clone(), version)
            .map_err(|error| format!("package mutation scan: {error:?}"))?;
        let mut context = (*self.context).clone();
        context.scan_version = version;
        let capture = Self::new(store.capture(), context)?;
        Ok(PackageUpdate { store, capture })
    }

    /// Prepare a new query version retaining the immutable package scan while
    /// publishing Settings' global uninstall-block owner.
    pub(crate) fn prepare_uninstall_blocks_update(
        self: &Arc<Self>,
        blocks: crate::package::mutations::UninstallBlocks,
    ) -> Result<PackageUpdate, String> {
        let version = self
            .scan
            .version()
            .checked_add(1)
            .ok_or("package version exhausted")?;
        let store = super::Store::new_replica_after(&self.scan,
            self.scan.owner().clone(),
            self.scan.usage().clone(),
            version,
        )
        .map_err(|error| format!("uninstall-block mutation scan: {error:?}"))?;
        let mut context = (*self.context).clone();
        context.scan_version = version;
        context.system.uninstall_blocks = Some(Arc::new(blocks));
        let capture = Self::new(store.capture(), context)?;
        Ok(PackageUpdate { store, capture })
    }

    pub(crate) fn prepare_preferred_update(
        self: &Arc<Self>,
        handle: Arc<crate::package::preferred::registry::Handle>,
        documents: &[(i32, Vec<u8>)],
    ) -> Result<PackageUpdate, String> {
        let version = self.scan.version().checked_add(1).ok_or("package version exhausted")?;
        let store = super::Store::new_replica_after(&self.scan,
            self.scan.owner().clone(), self.scan.usage().clone(), version,
        ).map_err(|error| format!("preferred publication: {error:?}"))?;
        let mut context = (*self.context).clone();
        context.scan_version = version;
        context.system.preferred_owner = Some(handle);
        for (user, bytes) in documents {
            let user = context.users.get_mut(user).ok_or("preferred user capture unavailable")?;
            user.preferred_activities = Some(bytes.clone());
            user.restrictions = Some(bytes.clone());
        }
        let capture = Self::new(store.capture(), context)?;
        Ok(PackageUpdate { store, capture })
    }

    pub(crate) fn prepare_app_metadata_files(
        self: &Arc<Self>, owner: Arc<crate::package::app_metadata::Owner>,
    ) -> Result<PackageUpdate, String> {
        let version = self.scan.version().checked_add(1).ok_or("package version exhausted")?;
        let store = super::Store::new_replica_after(&self.scan, self.scan.owner().clone(), self.scan.usage().clone(), version)
            .map_err(|error| format!("app metadata publication: {error:?}"))?;
        let mut context = (*self.context).clone();
        context.scan_version = version;
        context.system.app_metadata_files = Some(owner);
        let capture = Self::new(store.capture(), context)?;
        Ok(PackageUpdate { store, capture })
    }

    /// The full native boot owner calls this after resolution policy is ready.
    /// Partial query captures remain usable without publishing a guessed selection.
    pub fn select_permission_controller(self: &Arc<Self>) -> Result<Arc<Self>, String> {
        if self.state.system.permission_controller_package.is_some() {
            return Ok(self.clone());
        }
        let package = select_permission_controller(&self.state)?
            .ok_or("there must be exactly one permissions manager")?;
        let mut context = (*self.context).clone();
        context.system.permission_controller_package = Some(Some(package));
        Self::new(self.scan.clone(), context)
    }

    /// systemReady binds real provider resource metadata to this generation.
    pub fn load_module_metadata(
        self: &Arc<Self>,
        platform: &crate::package::parse::Platform,
        config: crate::package::parse::resources::Config,
        files: &dyn Fn(&str) -> Result<std::path::PathBuf, String>,
        apex: &crate::package::module_metadata::ApexLinks,
    ) -> Result<Arc<Self>, String> {
        let owner = crate::package::module_metadata::Owner::load(
            &self.state,
            platform,
            config,
            files,
            apex,
        )?;
        let mut context = (*self.context).clone();
        context.system.module_metadata = Some(Arc::new(owner));
        Self::new(self.scan.clone(), context)
    }

    pub fn select_configured_roles(
        self: &Arc<Self>,
        platform: &crate::package::parse::Platform,
        config: crate::package::parse::resources::Config,
    ) -> Result<Arc<Self>, String> {
        let roles = crate::package::roles::Owner::load(&self.state, platform, config)?;
        let mut context = (*self.context).clone();
        context.system.roles = Some(Arc::new(roles));
        Self::new(self.scan.clone(), context)
    }

    pub fn bind_instant_registry(self: &Arc<Self>, owner: Arc<crate::package::instant::Owner>) -> Result<Arc<Self>, String> {
        let mut context = (*self.context).clone();
        context.system.instant_access = Some(Arc::new(owner.snapshot()));
        context.system.instant_registry = Some(owner);
        Self::new(self.scan.clone(), context)
    }

    /// Retain the actual original RoleManager leaf before default permission
    /// grants invoke KnownPackages. Role changes remain live for old captures,
    /// matching DefaultAppProvider rather than caching a browser name.
    pub fn bind_default_browser_source(self: &Arc<Self>, bridge: Arc<crate::package::bootstrap::Bridge>) -> Result<Arc<Self>, String> {
        let roles = self.context.system.roles.as_ref().ok_or("configured role owner unavailable")?;
        let mut context = (*self.context).clone();
        context.system.roles = Some(Arc::new(roles.with_browser_source(crate::package::roles::BrowserSource::original(bridge))));
        Self::new(self.scan.clone(), context)
    }

    pub fn prepare_known_packages(
        self: &Arc<Self>, platform: &crate::package::parse::Platform,
        config: crate::package::parse::resources::Config, overlay_signature_package: Option<&str>,
    ) -> Result<Arc<Self>, String> {
        let roles = self.context.system.roles.as_ref().ok_or("configured role owner unavailable")?;
        let roles = roles.prepare_known_packages(&self.state, platform, config, overlay_signature_package)?;
        let mut context = (*self.context).clone();
        context.system.roles = Some(Arc::new(roles));
        Self::new(self.scan.clone(), context)
    }

    pub(crate) fn prepare_instant_registry_update(
        self: &Arc<Self>, owner: Arc<crate::package::instant::Owner>,
    ) -> Result<PackageUpdate, String> {
        let version = self.scan.version().checked_add(1).ok_or("package version exhausted")?;
        let store = super::Store::new_replica_after(&self.scan,
            self.scan.owner().clone(), self.scan.usage().clone(), version,
        ).map_err(|error| format!("instant registry publication: {error:?}"))?;
        let mut context = (*self.context).clone();
        context.scan_version = version;
        context.system.instant_access = Some(Arc::new(owner.snapshot()));
        context.system.instant_registry = Some(owner);
        let capture = Self::new(store.capture(), context)?;
        Ok(PackageUpdate { store, capture })
    }

    pub fn prepare_instant_components(
        self: &Arc<Self>,
        platform: &crate::package::parse::Platform,
        config: crate::package::parse::resources::Config,
        debuggable: bool,
        eng_build: bool,
    ) -> Result<Arc<Self>, String> {
        let owner = crate::package::instant_components::Owner::select_boot(
            &self.state, platform, config, debuggable, eng_build)?;
        let mut context = (*self.context).clone();
        context.system.instant_components = Some(Arc::new(owner));
        Self::new(self.scan.clone(), context)
    }

    pub fn load_page_size_compat_resources(
        self: &Arc<Self>,
        platform: &crate::package::parse::Platform,
        config: crate::package::parse::resources::Config,
    ) -> Result<Arc<Self>, String> {
        let owner = crate::package::page_size_compat::Owner::load(platform, config)?;
        let mut context = (*self.context).clone();
        context.system.page_size_compat = Some(Arc::new(owner));
        Self::new(self.scan.clone(), context)
    }

    pub fn refresh_role_resources(
        self: &Arc<Self>,
        platform: &crate::package::parse::Platform,
        config: crate::package::parse::resources::Config,
    ) -> Result<Arc<Self>, String> {
        let current = self
            .context
            .system
            .roles
            .as_ref()
            .ok_or("configured role owner unavailable")?;
        let mut context = (*self.context).clone();
        context.system.roles = Some(Arc::new(current.refresh_resources(platform, config)?));
        Self::new(self.scan.clone(), context)
    }

    pub fn prepare_keysets(
        self: &Arc<Self>,
        process: Arc<aim_binder_host::local::LocalProcess>,
    ) -> Result<Arc<Self>, String> {
        let mut context = (*self.context).clone();
        if context.system.key_set_tokens.is_some() {
            return Err("keyset token owner already prepared".into());
        }
        context.system.key_set_tokens =
            Some(Arc::new(crate::package::keysets::Tokens::new(process)));
        Self::new(self.scan.clone(), context)
    }

    pub fn new(scan: Arc<Snapshot>, mut context: Context) -> Result<Arc<Self>, String> {
        if context.scan_version != scan.version() {
            return Err(format!("query context scan version differs: context={}, snapshot={}",context.scan_version,scan.version()));
        }
        super::validate_replica(&scan).map_err(|error| format!("query replica: {error:?}"))?;
        let mut source = Arc::new(context.clone());
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
        let mut settings_order = owner.settings.packages.iter().map(|package| package.name.clone()).collect::<Vec<_>>();
        settings_order.sort_by_key(|name| crate::package::info::java_hash(name));
        context.system.settings_package_order = Some(settings_order);
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
        let mut state = model::State {
            generation: scan.version(),
            nonce: context.nonce,
            packages: active,
            disabled_system_packages: disabled,
            shared_users,
            uid_owners: Some(uid_owners),
            renamed_packages: Some(owner.settings.renamed_packages.clone()),
            shared_libraries: Some(scan.owner().libraries.entries().cloned().collect()),
            legacy_domains: domains
                .as_ref()
                .map(|domains| domains.owner().legacy_user_states().to_vec()),
            key_sets: Some(owner.settings.key_sets.clone()),
            package_registry: Some(Arc::new(scan.owner().package_registry()?.clone())),
            protected_broadcasts: Some(
                scan.owner()
                    .loaded_packages()
                    .values()
                    .flat_map(|p| p.package.protected_broadcasts.iter().cloned())
                    .collect(),
            ),
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
        if let Some(setting) = state.packages.get("android").filter(|setting| setting.pkg.is_some()) {
            let target = crate::package::info::Target {
                sys: &state.system,
                pkg: setting.pkg.as_deref().unwrap(),
                ps: setting,
                state: &model::PackageUserState::default(),
                user: 0,
            };
            let mut application = crate::package::info::generate_application_info(&target, 0)
                .ok_or("accepted platform application metadata unavailable")?;
            if let Some(paths) = setting.users.get(&0).and_then(|user| user.overlay_paths.as_ref()) {
                application.overlay_paths = Some(paths.overlay_paths.clone());
                application.resource_dirs = Some(paths.resource_dirs.clone());
            } else {
                application.overlay_paths = None;
                application.resource_dirs = None;
            }
            state.platform.android_application = Some(Arc::new(application));
            Arc::make_mut(&mut source).platform.android_application = state.platform.android_application.clone();
        }
        if state.packages.get("android").is_some_and(|package|package.pkg.is_some()) {
            let resolver=crate::package::resolver_owner::Owner::capture(&state,scan.owner().package_registry()?.committed_packages(),state.system.resolver_owner.as_deref())?;
            state.system.custom_resolver_activity=resolver.replaced.then(||Arc::new(resolver.activity.clone()));
            state.system.resolver_owner=Some(Arc::new(resolver));
            Arc::make_mut(&mut source).system.resolver_owner=state.system.resolver_owner.clone();
            Arc::make_mut(&mut source).system.custom_resolver_activity=state.system.custom_resolver_activity.clone();
        }
        let frozen = state.system.lifecycle.as_ref().map(|owner| owner.frozen_snapshot()).transpose()?;
        Ok(Arc::new(Self {
            scan,
            state: Arc::new(state),
            domains,
            context: source,
            frozen,
            resolver: crate::package::resolve::Resolver::default(),
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
    let runtime = snapshot
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
    let projection=code.map(|code|code.code_projection()).transpose()?;
    let pkg=projection.as_ref().map(|code|code.package.clone());
    let parcel=projection.map(|code|code.parcel);
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
        setting_flags: Some((s.flags, s.private_flags)),
        page_size_compat: Some(s.page_size_compat),
        real_name: Some(s.real_name.clone()),
        key_set_data: Some(s.key_set_data.clone()),
        app_metadata_source: Some(s.app_metadata_source),
        app_metadata_file_path: Some(s.app_metadata_file_path.clone()),
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
        suspensions: Some(
            s.resolved_suspensions(id, cross_user)
                .into_iter()
                .map(|(user, suspension)| restrictions::Suspension {
                    package: suspension.package.clone(),
                    user: restrictions::SuspendingUser::Resolved(user),
                    params: suspension.params.clone(),
                })
                .collect(),
        ),
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

/// PMS selects this owner once from finalized native activity registration.
fn select_permission_controller(state: &model::State) -> Result<Option<String>, String> {
    let state = Arc::new(state.clone());
    let resolution = crate::package::resolve::Resolution::new(
        state.clone(),
        &crate::package::apps_filter::Config {
            force_system_packages_queryable: state.system.force_system_packages_queryable,
            force_queryable_packages: state.system.force_queryable_packages.clone(),
        },
    )
    .map_err(|e| format!("permission-controller resolver: {e:?}"))?;
    let intent = crate::package::intent::Intent {
        action: Some("android.intent.action.MANAGE_PERMISSIONS".into()),
        categories: Some(vec!["android.intent.category.DEFAULT".into()]),
        ..Default::default()
    };
    let matches = resolution
        .query_intent_activities(
            &intent,
            None,
            crate::package::info::flags::MATCH_SYSTEM_ONLY
                | crate::package::info::flags::MATCH_DIRECT_BOOT_AWARE
                | crate::package::info::flags::MATCH_DIRECT_BOOT_UNAWARE,
            0,
            1000,
        )
        .map_err(|e| format!("permission-controller activity query: {e:?}"))?;
    if matches.is_empty() {
        return Ok(None);
    }
    if matches.len() != 1 {
        return Err("there must be exactly one permissions manager".into());
    }
    let crate::package::component_resolver::Info::Activity(info) = &matches[0].info else {
        return Err("permissions manager is not an activity".into());
    };
    if info.info.application_info.private_flags & (1 << 3) == 0 {
        return Err("the permissions manager must be a privileged app".into());
    }
    info.info
        .item
        .package_name
        .clone()
        .map(Some)
        .ok_or_else(|| "permissions manager has no package name".into())
}

#[cfg(test)]
mod controller_tests {
    use super::*;
    #[test]
    fn permission_controller_selection_requires_one_privileged_system_activity() {
        use crate::package::{
            intent_filter::{IntentFilter, ParsedIntentInfo},
            pkg::{Activity, AndroidPackage, Component, MainComponent, booleans},
        };
        let mut filter = IntentFilter::default();
        filter.add_action("android.intent.action.MANAGE_PERMISSIONS");
        filter.add_category("android.intent.category.DEFAULT");
        let activity = Activity {
            main: MainComponent {
                component: Component {
                    name: "p.Manage".into(),
                    package_name: "p".into(),
                    intents: vec![ParsedIntentInfo {
                        filter,
                        has_default: true,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                enabled: true,
                exported: true,
                direct_boot_aware: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let package = model::PackageState {
            name: "p".into(),
            app_id: 10100,
            is: model::StateFlags {
                system: true,
                privileged: true,
                ..Default::default()
            },
            pkg: Some(Arc::new(AndroidPackage {
                package_name: "p".into(),
                uid: 10100,
                booleans: booleans::SYSTEM | booleans::PRIVILEGED | booleans::ENABLED,
                activities: vec![activity],
                ..Default::default()
            })),
            users: [(0, model::PackageUserState::default())].into(),
            ..Default::default()
        };
        let mut state = model::State {
            packages: [("p".into(), package)].into(),
            users: [(
                0,
                model::User {
                    id: 0,
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        };
        assert_eq!(
            select_permission_controller(&state).unwrap().as_deref(),
            Some("p")
        );
        let package = state.packages.get_mut("p").unwrap();
        Arc::make_mut(package.pkg.as_mut().unwrap()).booleans &= !booleans::PRIVILEGED;
        assert!(
            select_permission_controller(&state)
                .unwrap_err()
                .contains("privileged")
        );
        state.packages.clear();
        assert_eq!(select_permission_controller(&state).unwrap(), None);
    }
}

impl Capture {
    pub(crate) fn with_visibility_view(self: &Arc<Self>, grants: crate::package::apps_filter::ImplicitAccess)
        -> Result<Arc<Self>, String> {
        let mut context = (*self.context).clone();
        context.system.implicit_access = grants;
        let mut capture = Self::new(self.scan.clone(), context)?;
        Arc::get_mut(&mut capture).unwrap().frozen = self.frozen.clone();
        Ok(capture)
    }

    pub(crate) fn prepare_visibility_update(self: &Arc<Self>, grants: crate::package::apps_filter::ImplicitAccess)
        -> Result<PackageUpdate, String> {
        let version = self.scan.version().checked_add(1).ok_or("package version exhausted")?;
        let store = super::Store::new_replica_after(&self.scan, self.scan.owner().clone(), self.scan.usage().clone(), version)
            .map_err(|error| format!("visibility mutation replica: {error:?}"))?;
        let mut context = (*self.context).clone();
        context.scan_version = version;
        context.system.implicit_access = grants;
        let capture = Self::new(store.capture(), context)?;
        Ok(PackageUpdate { store, capture })
    }
}

impl Capture {
    /// Refresh live permission grants/GIDs after the original permission owner
    /// completes a mutation, preserving native code and every unrelated owner.
    pub(crate) fn prepare_permission_refresh(self: &Arc<Self>, bridge: &crate::package::bootstrap::Bridge)
        -> Result<PackageUpdate, String> {
        let version = self.scan.version().checked_add(1).ok_or("package version exhausted")?;
        let store = super::Store::new_replica_after(&self.scan, self.scan.owner().clone(), self.scan.usage().clone(), version)
            .map_err(|error| format!("permission refresh replica: {error:?}"))?;
        let mut context = (*self.context).clone();
        context.scan_version = version;
        let context = bridge.resolve_query_context(self.scan.owner(), context)
            .map_err(|error| format!("permission refresh original owner: {error:?}"))?;
        let capture = Self::new(store.capture(), context)?;
        Ok(PackageUpdate { store, capture })
    }
}
impl Capture {
    pub(crate) fn prepare_usage_update(self:&Arc<Self>,usage:crate::package::owner::usage::Usage)
        -> Result<PackageUpdate,String> {
        let store=super::Store::prepare_usage_store(&self.scan, usage)
            .map_err(|error|format!("usage query replica: {error:?}"))?;
        let version=store.capture().version();
        let mut context=(*self.context).clone();context.scan_version=version;
        // Usage is owned by Snapshot.usage and its active ReplicaRuntime rows;
        // PackageState (including shared/UID views) contains no usage fields.
        // This validated transaction changed no code, user, domain, or policy
        // owner, so retain their immutable projections and serialized code Arcs.
        let mut state=(*self.state).clone();state.generation=version;
        let capture=Arc::new(Self{scan:store.capture(),state:Arc::new(state),domains:self.domains.clone(),
            context:Arc::new(context),frozen:self.frozen.clone(),resolver:Default::default()});
        Ok(PackageUpdate{store,capture})
    }
}
impl Capture {
    pub fn with_launch_sender(self:&Arc<Self>,owner:Arc<crate::package::launch::Owner>)->Result<Arc<Self>,String>{
        let mut context=(*self.context).clone();context.system.launch_sender=Some(owner);
        Self::new(self.scan.clone(),context)
    }
}
impl Capture {
    pub(crate) fn prepare_isolated_owner_update(self:&Arc<Self>,isolated:i32,owner:Option<i32>)->Result<PackageUpdate,String>{
        self.prepare_isolated_owner_delta(isolated, owner)
    }
}

impl Capture {
    pub(crate) fn apks_in_apex(&self, name: Option<&str>) -> Result<Option<Vec<String>>, crate::package::apps_filter::NotModelled> {
        use crate::package::apps_filter::NotModelled;
        let registry = self.scan().owner().package_registry().map_err(|_| NotModelled("APEX registration owner unavailable"))?;
        let module = name.and_then(|name| self.state().packages.get(name))
            .filter(|package| package.is.apex).and_then(|package| package.apex_module_name.as_deref());
        let inventory = self.state().apex_inventory.as_ref().ok_or(NotModelled("captured active APEX inventory unavailable"))?;
        let mut packages = Vec::new();
        if let Some(module) = module {
            for name in registry.committed_packages() {
                let Some(loaded) = self.scan().owner().loaded_packages().get(name) else { continue; };
                let Some(path) = loaded.package.base_apk_path.as_deref() else { continue; };
                for apex in &inventory.active {
                    if apex.module_name.as_deref() == Some(module)
                        && path.starts_with(&format!("{}/", apex.mount_path.trim_end_matches('/'))) {
                        packages.push(loaded.package.package_name.clone());
                    }
                }
            }
        }
        Ok(Some(packages))
    }

    /// SigningDetails.CREATOR body, distinct from the native signing-state record.
    pub(crate) fn platform_signing_record(&self) -> Result<Vec<u8>, crate::package::apps_filter::NotModelled> {
        use crate::package::apps_filter::NotModelled;
        use aim_service_aidl::write_byte_array;
        let loaded = self.scan().owner().loaded_packages().get("android")
            .ok_or(NotModelled("accepted platform signing owner unavailable"))?;
        let signing = &loaded.collected_signing;
        let mut parcel = aim_binder_host::parcel::Parcel::new();
        parcel.write_bool(signing.unknown);
        if !signing.unknown {
            let details = signing.parcel_details().map_err(|_| NotModelled("platform signing public key serialization failed"))?;
            fn signatures(parcel: &mut aim_binder_host::parcel::Parcel, values: Option<&[Vec<u8>]>) {
                match values {
                    None => parcel.write_i32(-1),
                    Some(values) => { parcel.write_i32(values.len() as i32); for value in values { parcel.write_i32(1); write_byte_array(parcel, Some(value)); } },
                }
            }
            signatures(&mut parcel, details.signatures.as_deref());
            parcel.write_i32(details.scheme_version);
            match details.public_keys {
                None => parcel.write_i32(-1),
                Some(keys) => {
                    parcel.write_i32(keys.len() as i32);
                    for key in keys {
                        match key {
                            None => parcel.write_i32(-1),
                            Some(key) => {
                                let mut value = aim_binder_host::parcel::Parcel::new();
                                value.write_string16(Some(&key.class));
                                write_byte_array(&mut value, Some(&key.bytes));
                                parcel.write_i32(21);
                                parcel.write_i32(value.data().len() as i32);
                                parcel.write_raw(value.data(), &[]);
                            },
                        }
                    }
                },
            }
            signatures(&mut parcel, details.past_signing_certificates.as_deref());
        }
        Ok(parcel.data().to_vec())
    }
}

#[cfg(test)]
mod resolver_tests {
    use super::*;
    use crate::package::{bootstrap::{ApexInventory,ScanUsers},owner::usage::Usage,
        scan::SigningScan,scan_snapshot::Store,query::Query,user_policy};
    use std::sync::atomic::{AtomicBool,AtomicUsize,Ordering};

    #[test]
    fn resolution_cache_is_owned_by_capture_and_replaced_on_publication() {
        let mut owner=SigningScan::new(&Default::default(),&Default::default(),36).unwrap();
        let groups=owner.identities.shared_users.keys().map(|name|(name.clone(),Default::default())).collect();
        owner.capture_legacy_permissions(&[0],Default::default(),groups).unwrap();
        let orders=owner.identities.shared_users.keys().map(|name|(name.clone(),vec![])).collect();
        owner.complete_shared_processes(orders).unwrap();
        let store=Store::new_replica(owner,Usage::new(std::iter::empty::<&str>())).unwrap();
        let restricted=Arc::new(AtomicBool::new(false));
        let reads=Arc::new(AtomicUsize::new(0));
        let live=restricted.clone();let calls=reads.clone();
        let policy=Arc::new(user_policy::Owner::new(Box::new(move|user| {
            assert_eq!(user,0);calls.fetch_add(1,Ordering::AcqRel);
            Ok(live.load(Ordering::Acquire))
        })));
        let scan=store.capture();
        let capture=Capture::new(scan.clone(),Context {
            scan_version:scan.version(),native_domains:None,boot_classes:None,nonce:None,
            system:model::System {sdk_sandbox_package:Some(None),user_policy:Some(policy),compatibility_mode:false,..Default::default()},
            platform:Default::default(),users:[(0,model::User{id:0,..Default::default()})].into(),
            apex_inventory:ApexInventory{packages:Some(vec![]),active:vec![]},
            scan_users:ScanUsers{users:Some(vec![crate::package::scan::User{id:0,pre_created:false,adb_install_disallowed:false}])},
            cross_user_suspensions:true,packages:Default::default(),retained_packages:Default::default(),
        }).unwrap();
        // Concurrent first calls must receive the same index, not separately
        // rebuilt component/AppsFilter graphs or a process-global cache.
        let joins=(0..4).map(|_|{let capture=capture.clone();std::thread::spawn(move||capture.resolution().unwrap())}).collect::<Vec<_>>();
        let indices=joins.into_iter().map(|join|join.join().unwrap()).collect::<Vec<_>>();
        let first=indices[0].clone();
        assert!(indices.iter().all(|index|Arc::ptr_eq(index,&first)));
        assert!(Arc::ptr_eq(&first,&capture.resolution().unwrap()));
        assert!(Arc::ptr_eq(&first.state,capture.state()));
        let query=Query{state:&first.state,filter:&first.apps_filter,calling_uid:2000};
        assert!(query.internal_enforce_cross_user(2000,0,false,true,"cache test").unwrap().is_ok());
        restricted.store(true,Ordering::Release);
        assert!(query.internal_enforce_cross_user(2000,0,false,true,"cache test").unwrap().is_err());
        assert_eq!(reads.load(Ordering::Acquire),2);
        assert!(Arc::ptr_eq(&first,&capture.resolution().unwrap()));
        // Use the real runtime-only publication constructor and canonical Store
        // publication boundary; retained old queries keep their original state.
        let update=capture.prepare_compatibility_mode(true).unwrap();
        store.publish_validated_store(&update.store);
        let published=update.capture;
        assert!(Arc::ptr_eq(published.scan(),&store.capture()));
        assert!(published.scan().version()>capture.scan().version());
        let next=published.resolution().unwrap();
        assert!(!Arc::ptr_eq(&first,&next));
        assert!(Arc::ptr_eq(&next,&published.resolution().unwrap()));
        assert!(Arc::ptr_eq(&first,&capture.resolution().unwrap()));
        assert!(!first.state.system.compatibility_mode);
        assert!(next.state.system.compatibility_mode);
        let query=Query{state:&next.state,filter:&next.apps_filter,calling_uid:2000};
        assert!(query.internal_enforce_cross_user(2000,0,false,true,"cache test").unwrap().is_err());
        assert_eq!(reads.load(Ordering::Acquire),3);
    }
}
