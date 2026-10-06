//! Attached domain state, distinct from pending/restored persistence (#957).
//! Ports DomainVerificationService.addPackage/migrateState at android-16.0.0_r1.
use super::{
    Package, State, User,
    collector::{self, Kind, Policy},
};
use crate::package::{
    info::{array_order, java_hash},
    pkg::AndroidPackage,
    system_config::SystemConfig,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub(crate) fn approval_eligible(
    code: &AndroidPackage,
    user: Option<&crate::package::restrictions::UserState>,
) -> bool {
    let Some(user) = user else { return false };
    user.installed
        && match user.enabled {
            1 => true,
            2 | 3 | 4 => false,
            _ => code.is(crate::package::pkg::booleans::ENABLED),
        }
        && !user
            .suspensions
            .as_ref()
            .is_some_and(|owners| !owners.is_empty())
}

pub struct Input<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub code: Option<&'a AndroidPackage>,
    pub signatures: &'a [Vec<u8>],
    pub system: bool,
    /// The original compatibility owner's decision, including overrides.
    pub restrict_domains: bool,
    pub pre_verified: Option<&'a [String]>,
}

#[derive(Debug)]
pub enum OwnersError {
    Input(String),
    Ordering(String),
}

pub struct ApprovalInput<'a> {
    pub code: &'a AndroidPackage,
    pub user: Option<&'a crate::package::restrictions::UserState>,
    pub settings_v2: bool,
    pub policy: Policy,
}

/// A request for the verifier driver; this does not claim broadcast delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub broadcast_requested: bool,
    pub recovered_missing_owner: bool,
}
/// Public query values at the retained package/domain owner boundary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Queries {
    pub verification: Option<(String, Vec<(String, i32)>)>,
    pub users: BTreeMap<i32, (bool, Vec<(String, i32)>)>,
    pub uri_relative_filter_groups: Vec<(
        String,
        Vec<super::super::intent_filter::UriRelativeFilterGroup>,
    )>,
}
#[derive(Clone, Debug)]
pub struct Owner {
    saved: State,
    attached: Vec<Package>,
    ids: BTreeMap<String, String>,
    legacy_info: BTreeMap<String, i32>,
}
#[derive(Clone, Debug)]
pub struct Boot {
    pub owner: Owner,
    /// Requests only; the verifier driver must apply its boot-readiness policy.
    pub changes: Vec<(String, Change)>,
}
impl Owner {
    /// Legacy global approvals are a separate owner, not inferred from user state.
    pub fn new(saved: State, legacy_info: BTreeMap<String, i32>) -> Self {
        Self {
            saved,
            attached: vec![],
            ids: BTreeMap::new(),
            legacy_info,
        }
    }
    /// Attach the actual current scan's code, Settings IDs and signer arrays.
    pub fn from_boot(
        scan: &crate::package::scan::SigningScan,
        config: &SystemConfig,
        policies: &BTreeMap<String, bool>,
    ) -> Result<Boot, String> {
        use std::collections::BTreeSet;
        let names: BTreeSet<_> = scan
            .settings
            .packages
            .iter()
            .map(|s| s.name.clone())
            .collect();
        if names.len() != scan.settings.packages.len()
            || policies.keys().cloned().collect::<BTreeSet<_>>() != names
        {
            return Err("boot domain policy inventory differs".into());
        }
        let mut inputs = Vec::new();
        for setting in &scan.settings.packages {
            let code = scan
                .loaded_packages()
                .get(&setting.name)
                .ok_or("missing boot domain code")?;
            let signing = setting
                .signatures
                .as_ref()
                .ok_or("missing boot domain signing owner")?;
            if code.collected_signing.unknown
                || code.collected_signing.package_details()? != code.package.signing_details
                || signing.signatures != code.collected_signing.signatures
            {
                return Err("boot domain signing owners differ".into());
            }
            let input = Input {
                id: setting
                    .domain_set_id
                    .as_deref()
                    .ok_or("missing boot domain UUID")?,
                name: &setting.name,
                code: Some(&code.package),
                signatures: &signing.signatures,
                system: setting.flags & crate::package::settings::FLAG_SYSTEM != 0,
                restrict_domains: policies[&setting.name],
                pre_verified: None,
            };
            validate(&input)?;
            inputs.push(input);
        }
        let mut owner = Self::new(
            scan.settings.domain_verification.clone(),
            scan.settings.legacy_domain_info.clone(),
        );
        let mut changes = Vec::new();
        for input in inputs {
            let name = input.name.to_owned();
            changes.push((name, owner.add(input, config)?));
        }
        Ok(Boot { owner, changes })
    }
    /// Attached auto-verification packages in the original ArrayMap order.
    pub fn valid_verification_package_names(&self) -> Vec<String> {
        self.attached
            .iter()
            .filter(|p| p.has_auto_verify_domains)
            .map(|p| p.name.clone())
            .collect()
    }
    pub fn package(&self, name: &str) -> Option<&Package> {
        self.attached.iter().find(|p| p.name == name)
    }
    pub fn attached_names(&self) -> impl Iterator<Item = &str> {
        self.attached.iter().map(|package| package.name.as_str())
    }
    pub fn owners<'a>(
        &self,
        host: &str,
        user_id: i32,
        mut lookup: impl FnMut(&str) -> Result<Option<ApprovalInput<'a>>, String>,
    ) -> Result<Vec<(String, bool)>, OwnersError> {
        let mut levels: BTreeMap<i32, Vec<(String, i64)>> = BTreeMap::new();
        for name in self.attached_names() {
            let Some(input) = lookup(name).map_err(OwnersError::Input)? else {
                continue;
            };
            let level = self
                .approval(
                    name,
                    input.code,
                    input.user,
                    user_id,
                    input.settings_v2,
                    input.policy,
                    host,
                )
                .map_err(OwnersError::Input)?;
            if level > 0 {
                levels.entry(level).or_default().push((
                    name.to_owned(),
                    input.user.map_or(0, |user| user.first_install_time),
                ));
            }
        }
        let mut owners = Vec::new();
        for (level, packages) in levels {
            let order = crate::package::timsort::sort(packages.len(), |a, b| {
                let (first, second) = (&packages[a], &packages[b]);
                Ok(if first.1 != second.1 {
                    (first.1.wrapping_sub(second.1) as i32).cmp(&0)
                } else {
                    super::names::compare(&first.0, &second.0)
                })
            })
            .map_err(OwnersError::Ordering)?;
            owners.extend(
                order
                    .into_iter()
                    .map(|i| (packages[i].0.clone(), level <= 3)),
            );
        }
        Ok(owners)
    }

    pub fn package_by_id(&self, id: &str) -> Option<&Package> {
        self.ids
            .get(&id.to_ascii_lowercase())
            .and_then(|name| self.package(name))
    }
    /// DomainVerificationService's public info/user projections. Compatibility
    /// is a current owner decision, not inferred from the package's SDK.
    pub fn queries(
        &self,
        code: &AndroidPackage,
        restrict_domains: bool,
        config: &SystemConfig,
        users: impl IntoIterator<Item = i32>,
    ) -> Result<Option<Queries>, String> {
        let Some(p) = self.package(&code.package_name) else {
            return Ok(None);
        };
        let policy = Policy {
            restrict_domains,
            linked_app: config.linked_apps.contains(&code.package_name),
        };
        let web = collector::collect(code, policy, Kind::Web);
        let verification = self.verification(&code.package_name, code, restrict_domains, config)?;
        let mut selections = BTreeMap::new();
        for id in users {
            if id < 0 || selections.contains_key(&id) {
                return Err("invalid domain query users".into());
            }
            let (_, allowed, states) = self.user_state(&code.package_name, code, restrict_domains, config, id)?.ok_or("missing attached user owner")?;
            selections.insert(id, (allowed, states));
        }
        // The package feed asks for groups of the queried users' web hosts.
        let groups = if selections.is_empty() {
            vec![]
        } else {
            p.uri_relative_filter_groups
                .iter()
                .filter_map(|(host, groups)| {
                    host.as_ref()
                        .filter(|host| web.contains(host))
                        .map(|host| (host.clone(), groups.clone()))
                })
                .collect::<BTreeMap<_, _>>()
                .into_iter()
                .collect()
        };
        Ok(Some(Queries {
            verification,
            users: selections,
            uri_relative_filter_groups: groups,
        }))
    }
    /// Info looks up its attached state by the requested name, while Computer
    /// resolves renamed/static-library package names for the parsed code.
    pub fn verification(
        &self,
        name: &str,
        code: &AndroidPackage,
        restrict_domains: bool,
        config: &SystemConfig,
    ) -> Result<Option<(String, Vec<(String, i32)>)>, String> {
        let Some(package) = self.package(name) else {
            return Ok(None);
        };
        let auto = collector::collect(
            code,
            Policy {
                restrict_domains,
                linked_app: config.linked_apps.contains(&code.package_name),
            },
            Kind::ValidAutoVerify,
        );
        if auto.is_empty() {
            return Ok(None);
        }
        let mut states = BTreeMap::<String, (usize, i32)>::new();
        for (index, (host, state)) in package.domains.iter().enumerate() {
            let host = host.as_ref().ok_or("null attached domain host")?;
            states
                .entry(host.clone())
                .and_modify(|value| value.1 = info_state(*state))
                .or_insert((index, info_state(*state)));
        }
        for (index, host) in auto.into_iter().enumerate() {
            states
                .entry(host)
                .or_insert((package.domains.len() + index, 0));
        }
        let mut states: Vec<_> = states
            .into_iter()
            .map(|(host, (index, state))| (host, index, state))
            .collect();
        states.sort_by_key(|(host, index, _)| (java_hash(host), *index));
        Ok(Some((
            package.id.clone(),
            states
                .into_iter()
                .map(|(host, _, state)| (host, state))
                .collect(),
        )))
    }
    pub fn user_state(
        &self,
        name: &str,
        code: &AndroidPackage,
        restrict_domains: bool,
        config: &SystemConfig,
        user: i32,
    ) -> Result<Option<(String, bool, Vec<(String, i32)>)>, String> {
        let Some(package) = self.package(name) else {
            return Ok(None);
        };
        let domains = collector::collect(
            code,
            Policy {
                restrict_domains,
                linked_app: config.linked_apps.contains(&code.package_name),
            },
            Kind::Web,
        );
        let selected = package.users.iter().find(|u| u.id == user);
        let states = domains
            .into_iter()
            .map(|host| {
                let state = package
                    .domains
                    .iter()
                    .find(|(h, _)| h.as_ref() == Some(&host))
                    .map(|(_, state)| *state);
                let value = if state.is_some_and(|state| matches!(state, 1 | 2 | 4 | 5 | 7 | 8)) {
                    2
                } else if selected.is_some_and(|user| user.enabled_hosts.contains(&host)) {
                    1
                } else {
                    0
                };
                (host, value)
            })
            .collect();
        Ok(Some((
            package.id.clone(),
            selected.is_none_or(|user| user.allow_link_handling),
            states,
        )))
    }
    /// Positive approval levels used by getOwnersForDomain and user selection.
    /// User state is explicit (not PackageUserStateDefault); v2 is an original
    /// compatibility-owner decision independent of collector restrictions.
    pub fn approval(
        &self,
        name: &str,
        code: &AndroidPackage,
        user: Option<&crate::package::restrictions::UserState>,
        user_id: i32,
        settings_v2: bool,
        policy: Policy,
        host: &str,
    ) -> Result<i32, String> {
        if !approval_eligible(code, user) {
            return Ok(0);
        }
        let user = user.unwrap();
        if !settings_v2 {
            let legacy = self
                .saved
                .legacy
                .iter()
                .find(|(pkg, _)| pkg.as_deref() == Some(name))
                .and_then(|(_, users)| users.iter().find(|(id, _)| *id == user_id))
                .map_or(0, |(_, state)| *state);
            match legacy {
                3 => return Ok(0),
                1 | 4 => return Ok(1),
                2 => return Ok(2),
                _ => {}
            }
        }
        let Some(package) = self.package(name) else {
            return Ok(0);
        };
        let selected = package.users.iter().find(|u| u.id == user_id);
        if selected.is_some_and(|u| !u.allow_link_handling) {
            return Ok(0);
        }
        if user.instant_app
            && collector::collect(code, policy, Kind::ValidAutoVerify)
                .iter()
                .any(|domain| domain == host)
        {
            return Ok(5);
        }
        let verified = |state: i32| matches!(state, 1 | 2 | 4 | 5 | 7 | 8);
        if package
            .domains
            .iter()
            .any(|(domain, state)| domain.as_deref() == Some(host) && verified(*state))
        {
            return Ok(4);
        }
        for (domain, state) in &package.domains {
            if verified(*state) {
                let domain = domain.as_deref().ok_or("null attached approval host")?;
                if domain
                    .strip_prefix("*.")
                    .is_some_and(|suffix| host.ends_with(suffix))
                {
                    return Ok(4);
                }
            }
        }
        if selected.is_some_and(|user| {
            user.enabled_hosts.iter().any(|domain| {
                domain == host
                    || domain
                        .strip_prefix("*.")
                        .is_some_and(|suffix| host.ends_with(suffix))
            })
        }) {
            return Ok(3);
        }
        Ok(0)
    }

    pub fn remove(&mut self, name: &str) -> Option<Package> {
        let i = self.attached.iter().position(|p| p.name == name)?;
        let p = self.attached.remove(i);
        if self.ids.get(&p.id).is_some_and(|value| value == name) {
            self.ids.remove(&p.id);
        }
        Some(p)
    }
    /// Bundle update after DOMAIN_VERIFICATION_AGENT authorization. The
    /// original merges keys, removes null/empty lists and schedules no write.
    pub fn set_uri_groups(
        &mut self,
        name: &str,
        updates: &[(
            String,
            Option<Vec<super::super::intent_filter::UriRelativeFilterGroup>>,
        )],
    ) -> Result<(), String> {
        if updates.is_empty() {
            return Ok(());
        }
        let package = self
            .attached
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or("URI-group package is unavailable")?;
        for (host, groups) in updates {
            if !super::uri_groups::valid_domain(host)? {
                continue;
            }
            if let Some(groups) = groups.as_ref().filter(|g| !g.is_empty()) {
                let groups: Vec<_> = groups
                    .iter()
                    .map(|g| {
                        let mut normalized =
                            super::super::intent_filter::UriRelativeFilterGroup::new(g.action);
                        for f in &g.filters {
                            normalized.add(f.uri_part, f.pattern_type, &f.filter);
                        }
                        normalized
                    })
                    .collect();
                if let Some((_, target)) = package
                    .uri_relative_filter_groups
                    .iter_mut()
                    .find(|(name, _)| name.as_ref() == Some(host))
                {
                    *target = groups.clone();
                } else {
                    package
                        .uri_relative_filter_groups
                        .push((Some(host.clone()), groups.clone()));
                }
            } else {
                package
                    .uri_relative_filter_groups
                    .retain(|(name, _)| name.as_ref() != Some(host));
            }
        }
        package.uri_relative_filter_groups = array_order(
            std::mem::take(&mut package.uri_relative_filter_groups),
            |(name, _)| name.as_deref().unwrap_or(""),
        );
        Ok(())
    }
    pub fn uri_groups(
        &self,
        name: &str,
        domains: &[String],
    ) -> Vec<(
        String,
        Vec<super::super::intent_filter::UriRelativeFilterGroup>,
    )> {
        let Some(package) = self.package(name) else {
            return vec![];
        };
        let mut out = Vec::new();
        for name in domains {
            if !out.iter().any(|(host, _)| host == name)
                && let Some((_, groups)) = package
                    .uri_relative_filter_groups
                    .iter()
                    .find(|(host, _)| host.as_ref() == Some(name))
            {
                out.push((name.clone(), groups.clone()));
            }
        }
        array_order(out, |(name, _)| name.as_str())
    }
    pub fn uri_groups_query(
        &self,
        name: Option<&str>,
        domains: Option<&[Option<String>]>,
    ) -> Result<
        Vec<(
            Option<String>,
            Vec<super::super::intent_filter::UriRelativeFilterGroup>,
        )>,
        String,
    > {
        let Some(package) = name.and_then(|name| self.package(name)) else {
            return Ok(vec![]);
        };
        let domains = domains.ok_or("Attempt to invoke interface method 'int java.util.List.size()' on a null object reference")?;
        let mut out = Vec::new();
        for name in domains {
            if !out.iter().any(|(host, _)| host == name)
                && let Some((_, groups)) = package
                    .uri_relative_filter_groups
                    .iter()
                    .find(|(host, _)| host == name)
            {
                out.push((name.clone(), groups.clone()));
            }
        }
        out.sort_by_key(|(name, _)| java_hash(name.as_deref().unwrap_or("")));
        Ok(out)
    }

    /// Verifier-state mutation after caller authorization. Error statuses do
    /// not request persistence; success must be scheduled by the driver.
    pub fn set_verifier_status(
        &mut self,
        id: &str,
        code: Option<&AndroidPackage>,
        policy: Policy,
        hosts: &mut std::collections::BTreeSet<String>,
        state: i32,
    ) -> Result<i32, String> {
        if state != 1 && state < 1024 {
            return Err("invalid verifier state".into());
        }
        let Some(name) = self.ids.get(&id.to_ascii_lowercase()).cloned() else {
            return Ok(1);
        };
        let code = code
            .filter(|code| code.package_name == name)
            .ok_or("verification package code is unavailable")?;
        if hosts.is_empty() {
            return Err("verification domain set is empty".into());
        }
        let declared = collector::collect(code, policy, Kind::ValidAutoVerify);
        let size = hosts.len();
        hosts.retain(|host| declared.contains(host));
        if hosts.len() != size {
            return Ok(2);
        }
        let package = self
            .attached
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or("missing indexed domain owner")?;
        let mut verified = Vec::new();
        for host in hosts.iter() {
            let old = package
                .domains
                .iter()
                .find(|(h, _)| h.as_ref() == Some(host))
                .map(|(_, state)| *state);
            if old.is_some_and(|old| {
                old == state || !(matches!(old, 0 | 1 | 4 | 5 | 6 | 8) || old >= 1024)
            }) {
                continue;
            }
            if state == 1 && old.is_none_or(|old| !matches!(old, 1 | 2 | 4 | 5 | 7 | 8)) {
                verified.push(host.clone());
            }
            set(package, host, state);
        }
        let disabled: std::collections::BTreeSet<_> = package
            .users
            .iter()
            .filter(|u| !u.allow_link_handling)
            .map(|u| u.id)
            .collect();
        for p in &mut self.attached {
            for user in &mut p.users {
                if !disabled.contains(&user.id) {
                    user.enabled_hosts.retain(|host| !verified.contains(host));
                }
            }
        }
        Ok(0)
    }

    /// After ID/code/host validation: public user selection's two-pass revocation.
    /// Even approval failure retains the newly allocated user state, as original.
    pub fn set_user_selection<'a>(
        &mut self,
        name: &str,
        user_id: i32,
        hosts: &std::collections::BTreeSet<String>,
        enabled: bool,
        mut lookup: impl FnMut(&str) -> Result<Option<ApprovalInput<'a>>, String>,
    ) -> Result<i32, String> {
        let at = self
            .attached
            .iter()
            .position(|p| p.name == name)
            .ok_or("selection package is unavailable")?;
        if !self.attached[at].users.iter().any(|u| u.id == user_id) {
            self.attached[at].users.push(User {
                id: user_id,
                allow_link_handling: true,
                enabled_hosts: vec![],
            });
            self.attached[at].users.sort_by_key(|u| u.id);
        }
        let selected = self.attached[at]
            .users
            .iter()
            .find(|u| u.id == user_id)
            .unwrap()
            .enabled_hosts
            .clone();
        let mut revoke = Vec::new();
        if enabled {
            for host in hosts {
                if selected.contains(host) {
                    continue;
                }
                let mut highest = 1;
                let mut approved = Vec::new();
                for package in &self.attached {
                    let Some(input) = lookup(&package.name)? else {
                        continue;
                    };
                    let level = self.approval(
                        &package.name,
                        input.code,
                        input.user,
                        user_id,
                        input.settings_v2,
                        input.policy,
                        host,
                    )?;
                    if level < 1 {
                        continue;
                    }
                    if level > highest {
                        approved.clear();
                        highest = level;
                    }
                    if level == highest {
                        approved.push((
                            package.name.clone(),
                            input.user.map_or(0, |u| u.first_install_time),
                        ));
                    }
                }
                if highest > 3 {
                    return Ok(3);
                }
                if let Some(latest) = approved.iter().map(|(_, time)| *time).max() {
                    revoke.push((
                        host.clone(),
                        approved
                            .into_iter()
                            .filter(|(_, time)| *time == latest)
                            .map(|(name, _)| name)
                            .collect::<Vec<_>>(),
                    ));
                }
            }
            for (host, packages) in revoke {
                for package in &mut self.attached {
                    if packages.contains(&package.name)
                        && let Some(user) = package.users.iter_mut().find(|u| u.id == user_id)
                    {
                        user.enabled_hosts.retain(|selected| selected != &host);
                    }
                }
            }
        }
        let user = self.attached[at]
            .users
            .iter_mut()
            .find(|u| u.id == user_id)
            .unwrap();
        if enabled {
            for host in hosts {
                if !user.enabled_hosts.contains(host) {
                    user.enabled_hosts.push(host.clone());
                }
            }
            user.enabled_hosts =
                array_order(std::mem::take(&mut user.enabled_hosts), String::as_str);
        } else {
            user.enabled_hosts.retain(|host| !hosts.contains(host));
        }
        Ok(0)
    }

    /// State part of the original internal setter; caller enforces identity and
    /// schedules persistence after success. Only attached packages are changed.
    pub fn set_link_handling_internal(
        &mut self,
        name: Option<&str>,
        allowed: bool,
        user_id: i32,
        all_user_ids: &[i32],
    ) -> Result<(), String> {
        if name.is_some_and(|name| self.package(name).is_none()) {
            return Err("link-handling package is unavailable".into());
        }
        let ids = if user_id == -1 {
            all_user_ids
        } else {
            std::slice::from_ref(&user_id)
        };
        for package in self
            .attached
            .iter_mut()
            .filter(|p| name.is_none_or(|name| p.name == name))
        {
            for id in ids {
                let index = if let Some(index) = package.users.iter().position(|u| u.id == *id) {
                    index
                } else {
                    package.users.push(User {
                        id: *id,
                        allow_link_handling: true,
                        enabled_hosts: vec![],
                    });
                    package.users.len() - 1
                };
                package.users[index].allow_link_handling = allowed;
            }
            package.users.sort_by_key(|u| u.id);
        }
        Ok(())
    }
    /// Caller schedules a settings write after each clear, even for a missing row.
    pub fn clear_package(&mut self, name: &str) {
        self.remove(name);
        self.saved.clear_package(name);
    }
    pub fn clear_package_for_user(&mut self, name: &str, id: i32) {
        for p in self
            .attached
            .iter_mut()
            .chain(&mut self.saved.active)
            .chain(&mut self.saved.restored)
            .filter(|p| p.name == name)
        {
            p.users.retain(|u| u.id != id);
        }
    }
    pub fn clear_user(&mut self, id: i32) {
        for p in self
            .attached
            .iter_mut()
            .chain(&mut self.saved.active)
            .chain(&mut self.saved.restored)
        {
            p.users.retain(|u| u.id != id);
        }
        // The original does not remove its separate legacy migration owner.
    }
    fn put(&mut self, p: Package) {
        self.remove(&p.name);
        self.ids.insert(p.id.clone(), p.name.clone());
        self.attached.push(p);
        self.attached.sort_by_key(|p| java_hash(&p.name));
    }
    /// Logical persistence maps; XML writing orders package values separately.
    pub fn persisted(&self) -> State {
        let mut saved = self.saved.clone();
        saved.active.extend(self.attached.iter().cloned());
        saved.active.sort_by_key(|p| java_hash(&p.name));
        saved
    }
    pub fn add(&mut self, input: Input<'_>, config: &SystemConfig) -> Result<Change, String> {
        validate(&input)?;
        let name = input.name;
        let code = input
            .code
            .ok_or("missing package code for domain attachment")?;
        let (auto, web) = domains(&input, code, config);
        let pending = take(&mut self.saved.active, name);
        let broadcast = pending.is_none();
        let source = pending.or_else(|| {
            take(&mut self.saved.restored, name)
                .filter(|p| p.signature.as_deref() == Some(&signature_hash(input.signatures)))
        });
        let inherited = source.is_some();
        let mut p = source.unwrap_or_else(|| blank(&input, !auto.is_empty()));
        p.id = input.id.to_ascii_lowercase();
        p.has_auto_verify_domains = !auto.is_empty();
        // The original copy constructor clears backup signatures and URI groups.
        p.signature = None;
        p.uri_relative_filter_groups.clear();
        p.domains
            .retain(|(host, _)| host.as_ref().is_some_and(|h| auto.contains(h)));
        p.domains = array_order(p.domains, |(host, _)| host.as_deref().unwrap_or(""));
        for user in &mut p.users {
            user.enabled_hosts.retain(|h| web.contains(h));
            user.enabled_hosts =
                array_order(std::mem::take(&mut user.enabled_hosts), String::as_str);
        }
        if immutable(&mut p, &input, config, &auto) && !inherited {
            if let Some((_, users)) = self
                .saved
                .legacy
                .iter()
                .find(|(pkg, _)| pkg.as_deref() == Some(name))
            {
                for (id, status) in users {
                    if *status == 2 {
                        p.users.push(User {
                            id: *id,
                            allow_link_handling: true,
                            enabled_hosts: web.clone(),
                        });
                    }
                }
            }
            if self.legacy_info.remove(name) == Some(2) {
                p.domains = auto.iter().map(|h| (Some(h.clone()), 4)).collect();
            }
            pre_verified(&mut p, &input, &auto);
        }
        p.users.sort_by_key(|u| u.id);
        self.put(p);
        Ok(Change {
            broadcast_requested: broadcast && !auto.is_empty(),
            recovered_missing_owner: false,
        })
    }
    pub fn migrate(
        &mut self,
        old_id: &str,
        old_code: Option<&AndroidPackage>,
        input: Input<'_>,
        config: &SystemConfig,
    ) -> Result<Change, String> {
        validate(&input)?;
        let old = self
            .ids
            .get(&old_id.to_ascii_lowercase())
            .cloned()
            .and_then(|name| self.remove(&name));
        let Some((old, code)) = old.zip(input.code).filter(|_| old_code.is_some()) else {
            // Original recovery path: a blank attached state, no verifier request.
            self.put(blank(&input, true));
            return Ok(Change {
                broadcast_requested: false,
                recovered_missing_owner: true,
            });
        };
        let (auto, web) = domains(&input, code, config);
        let mut p = blank(&input, !auto.is_empty());
        p.domains = auto
            .iter()
            .filter_map(|host| {
                old.domains
                    .iter()
                    .find(|(key, _)| key.as_ref() == Some(host))
                    .filter(|(_, state)| matches!(state, 1 | 2 | 3 | 4 | 5 | 8))
                    .cloned()
            })
            .collect();
        p.users = old
            .users
            .into_iter()
            .map(|mut u| {
                u.enabled_hosts.retain(|h| web.contains(h));
                u
            })
            .collect();
        p.uri_relative_filter_groups = old.uri_relative_filter_groups;
        let broadcast = immutable(&mut p, &input, config, &auto) && !auto.is_empty();
        pre_verified(&mut p, &input, &auto);
        self.put(p);
        Ok(Change {
            broadcast_requested: broadcast,
            recovered_missing_owner: false,
        })
    }
}
fn info_state(state: i32) -> i32 {
    match state {
        0 | 1 => state,
        4 | 5 | 8 => 4,
        6 => 3,
        1024.. => state,
        _ => 2,
    }
}
fn validate(input: &Input<'_>) -> Result<(), String> {
    let parts: Vec<_> = input.id.split('-').collect();
    if input.name.is_empty()
        || input
            .code
            .is_some_and(|code| code.package_name != input.name)
        || parts.len() != 5
        || parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .any(|(p, n)| p.len() != n || !p.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("invalid attached domain identity".into());
    }
    Ok(())
}
fn blank(input: &Input<'_>, has_auto: bool) -> Package {
    Package {
        name: input.name.into(),
        id: input.id.to_ascii_lowercase(),
        has_auto_verify_domains: has_auto,
        signature: None,
        domains: vec![],
        users: vec![],
        uri_relative_filter_groups: vec![],
    }
}
fn take(packages: &mut Vec<Package>, name: &str) -> Option<Package> {
    packages
        .iter()
        .position(|p| p.name == name)
        .map(|i| packages.remove(i))
}
fn domains(
    input: &Input<'_>,
    code: &AndroidPackage,
    config: &SystemConfig,
) -> (Vec<String>, Vec<String>) {
    let policy = Policy {
        restrict_domains: input.restrict_domains,
        linked_app: config.linked_apps.iter().any(|name| name == input.name),
    };
    (
        collector::collect(code, policy, Kind::ValidAutoVerify),
        collector::collect(code, policy, Kind::Web),
    )
}
fn immutable(p: &mut Package, input: &Input<'_>, config: &SystemConfig, auto: &[String]) -> bool {
    if input.system && config.linked_apps.contains(&p.name) {
        for host in auto {
            set(p, host, 7);
        }
        false
    } else {
        p.domains.retain(|(_, state)| *state != 7);
        true
    }
}
fn set(p: &mut Package, host: &str, state: i32) {
    if let Some((_, value)) = p
        .domains
        .iter_mut()
        .find(|(h, _)| h.as_deref() == Some(host))
    {
        *value = state;
    } else {
        p.domains.push((Some(host.into()), state));
    }
    p.domains = array_order(std::mem::take(&mut p.domains), |(h, _)| {
        h.as_deref().unwrap_or("")
    });
}
fn pre_verified(p: &mut Package, input: &Input<'_>, auto: &[String]) {
    for host in input.pre_verified.iter().copied().flatten() {
        if auto.contains(host) && !p.domains.iter().any(|(h, _)| h.as_ref() == Some(host)) {
            set(p, host, 8);
        }
    }
}
/// PackageUtils.computeSignaturesSha256Digest, including sorted multi-signers.
pub fn signature_hash(signatures: &[Vec<u8>]) -> String {
    let hex = |bytes: &[u8]| {
        Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<String>()
    };
    let mut hashes: Vec<_> = signatures.iter().map(|s| hex(s)).collect();
    if hashes.len() == 1 {
        return hashes.remove(0);
    }
    hashes.sort();
    hex(hashes.concat().as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        intent_filter::{
            ACTION_VIEW, CATEGORY_BROWSABLE, IntentFilter, ParsedIntentInfo, UriRelativeFilterGroup,
        },
        pkg::Activity,
    };
    const OLD: &str = "00000000-0000-0000-0000-00000000000a";
    const NEW: &str = "00000000-0000-0000-0000-00000000000b";
    fn code(hosts: &[&str]) -> AndroidPackage {
        let mut filter = IntentFilter {
            auto_verify: true,
            ..Default::default()
        };
        filter.add_action(ACTION_VIEW);
        filter.add_category(CATEGORY_BROWSABLE);
        filter.add_category("android.intent.category.DEFAULT");
        filter.add_data_scheme("https");
        for host in hosts {
            filter.add_data_authority(host, None);
        }
        let mut activity = Activity::default();
        activity.main.component.intents.push(ParsedIntentInfo {
            filter,
            ..Default::default()
        });
        AndroidPackage {
            package_name: "fixture".into(),
            activities: vec![activity],
            ..Default::default()
        }
    }
    fn input(code: &AndroidPackage) -> Input<'_> {
        Input {
            id: NEW,
            name: "fixture",
            code: Some(code),
            signatures: &[],
            system: false,
            restrict_domains: true,
            pre_verified: None,
        }
    }
    #[test]
    fn info_keeps_hash_collision_insertion_order_and_requested_name() {
        let code = code(&["BB.example", "Aa.example"]);
        let config = SystemConfig::default();
        let mut owner = Owner::new(State::default(), BTreeMap::new());
        owner.add(input(&code), &config).unwrap();
        owner.attached[0].domains = vec![
            (Some("BB.example".into()), 1),
            (Some("Aa.example".into()), 2),
        ];
        assert_eq!(java_hash("BB.example"), java_hash("Aa.example"));
        let (_, states) = owner
            .verification("fixture", &code, true, &config)
            .unwrap()
            .unwrap();
        assert_eq!(
            states,
            vec![("BB.example".into(), 1), ("Aa.example".into(), 2)]
        );
        assert!(
            owner
                .verification("alias", &code, true, &config)
                .unwrap()
                .is_none()
        );
    }
    fn saved() -> Package {
        Package {
            name: "fixture".into(),
            id: OLD.into(),
            has_auto_verify_domains: true,
            signature: Some("old".into()),
            domains: vec![
                (Some("keep.example".into()), 1),
                (Some("gone.example".into()), 4),
                (None, 2),
            ],
            users: vec![User {
                id: 10,
                allow_link_handling: false,
                enabled_hosts: vec!["keep.example".into(), "gone.example".into()],
            }],
            uri_relative_filter_groups: vec![(
                Some("keep.example".into()),
                vec![UriRelativeFilterGroup {
                    action: 0,
                    filters: vec![],
                }],
            )],
        }
    }
    #[test]
    fn attachment_consumes_pending_before_restore_and_resets_copy_only_fields() {
        let code = code(&["keep.example", "new.example"]);
        let state = State {
            active: vec![saved()],
            restored: vec![saved()],
            ..Default::default()
        };
        let mut owner = Owner::new(state, BTreeMap::new());
        let pre = ["new.example".into()];
        let mut args = input(&code);
        args.pre_verified = Some(&pre);
        let change = owner.add(args, &SystemConfig::default()).unwrap();
        assert!(!change.broadcast_requested);
        assert!(!change.recovered_missing_owner);
        let p = owner.package("fixture").unwrap();
        assert_eq!(p.id, NEW);
        assert_eq!(p.domains, vec![(Some("keep.example".into()), 1)]);
        assert_eq!(p.users[0].enabled_hosts, ["keep.example"]);
        assert!(!p.users[0].allow_link_handling);
        assert!(p.signature.is_none());
        assert!(p.uri_relative_filter_groups.is_empty());
        assert_eq!(owner.persisted().restored.len(), 1);
        assert!(owner.package_by_id(OLD).is_none());
        assert!(owner.package_by_id(&NEW.to_uppercase()).is_some());
        let before = owner.persisted();
        let mut bad = input(&code);
        bad.id = "invalid";
        assert!(owner.add(bad, &SystemConfig::default()).is_err());
        assert_eq!(owner.persisted(), before);
    }
    #[test]
    fn restore_identity_and_fresh_legacy_preverified_and_immutable_order() {
        let code = code(&["keep.example", "new.example"]);
        let pre = ["new.example".into()];
        for matches in [false, true] {
            let mut p = saved();
            if matches {
                p.signature = Some(signature_hash(&[]));
            }
            let state = State {
                restored: vec![p],
                legacy: vec![(Some("fixture".into()), vec![(10, 2), (11, 3)])],
                ..Default::default()
            };
            let mut owner = Owner::new(state, BTreeMap::new());
            let mut args = input(&code);
            args.pre_verified = Some(&pre);
            assert!(
                owner
                    .add(args, &SystemConfig::default())
                    .unwrap()
                    .broadcast_requested
            );
            let p = owner.package("fixture").unwrap();
            assert!(owner.persisted().restored.is_empty());
            if matches {
                assert_eq!(p.domains, vec![(Some("keep.example".into()), 1)]);
                assert!(!p.users[0].allow_link_handling);
            } else {
                assert_eq!(p.domains, vec![(Some("new.example".into()), 8)]);
                assert_eq!(p.users.len(), 1);
                assert!(p.users[0].allow_link_handling);
                assert_eq!(p.users[0].enabled_hosts.len(), 2);
            }
        }
        let state = State {
            legacy: vec![(Some("fixture".into()), vec![(10, 2)])],
            ..Default::default()
        };
        let mut owner = Owner::new(state.clone(), BTreeMap::from([("fixture".into(), 2)]));
        let mut args = input(&code);
        args.pre_verified = Some(&pre);
        owner.add(args, &SystemConfig::default()).unwrap();
        assert!(
            owner
                .package("fixture")
                .unwrap()
                .domains
                .iter()
                .all(|(_, state)| *state == 4)
        );
        let mut config = SystemConfig::default();
        config.linked_apps = vec!["fixture".into()];
        let mut owner = Owner::new(state, BTreeMap::from([("fixture".into(), 2)]));
        let mut args = input(&code);
        args.system = true;
        args.pre_verified = Some(&pre);
        // addPackage requests a broadcast even though immutable state skips legacy work.
        assert!(owner.add(args, &config).unwrap().broadcast_requested);
        let p = owner.package("fixture").unwrap();
        assert!(p.users.is_empty());
        assert!(p.domains.iter().all(|(_, state)| *state == 7));
    }
    #[test]
    fn update_migrates_only_selected_states_retains_groups_and_reports_recovery() {
        let hosts: Vec<_> = (0..=8)
            .chain([1024])
            .map(|n| format!("h{n}.example"))
            .collect();
        let code = code(&hosts.iter().map(String::as_str).collect::<Vec<_>>());
        let mut state = saved();
        state.domains = (0..=8)
            .chain([1024])
            .map(|n| (Some(format!("h{n}.example")), n))
            .collect();
        let mut owner = Owner::new(
            State {
                active: vec![state],
                ..Default::default()
            },
            BTreeMap::new(),
        );
        let mut args = input(&code);
        args.id = OLD;
        owner.add(args, &SystemConfig::default()).unwrap();
        // Recreate an attached URI group, as a verifier update can do after initial add.
        owner.attached[0].uri_relative_filter_groups = saved().uri_relative_filter_groups;
        let change = owner
            .migrate(
                &OLD.to_uppercase(),
                Some(&code),
                input(&code),
                &SystemConfig::default(),
            )
            .unwrap();
        assert!(change.broadcast_requested);
        assert!(!change.recovered_missing_owner);
        let p = owner.package("fixture").unwrap();
        let mut states: Vec<_> = p.domains.iter().map(|(_, state)| *state).collect();
        states.sort();
        assert_eq!(states, [1, 2, 3, 4, 5, 8]);
        assert_eq!(p.uri_relative_filter_groups.len(), 1);
        assert!(owner.package_by_id(OLD).is_none());
        let mut args = input(&code);
        args.id = OLD;
        args.code = None;
        let change = owner
            .migrate(NEW, Some(&code), args, &SystemConfig::default())
            .unwrap();
        assert!(change.recovered_missing_owner);
        assert!(!change.broadcast_requested);
        let p = owner.package("fixture").unwrap();
        assert!(p.has_auto_verify_domains);
        assert!(p.domains.is_empty());
        assert!(p.users.is_empty());
    }
    #[test]
    fn queries_project_public_states_selection_priority_and_absent_user_defaults() {
        let hosts: Vec<_> = (0..=9).map(|i| format!("h{i}.example")).collect();
        let refs: Vec<_> = hosts.iter().map(String::as_str).collect();
        let code = code(&refs);
        let mut pending = blank(&input(&code), true);
        pending.domains = hosts
            .iter()
            .enumerate()
            .map(|(i, h)| (Some(h.clone()), i as i32))
            .collect();
        pending.domains[9].1 = 1024;
        pending.users = vec![User {
            id: 10,
            allow_link_handling: false,
            enabled_hosts: hosts.clone(),
        }];
        let mut owner = Owner::new(
            State {
                active: vec![pending],
                ..Default::default()
            },
            Default::default(),
        );
        owner.add(input(&code), &SystemConfig::default()).unwrap();
        let queries = owner
            .queries(&code, true, &SystemConfig::default(), [0, 10])
            .unwrap()
            .unwrap();
        let states: BTreeMap<_, _> = queries.verification.unwrap().1.into_iter().collect();
        for (i, expected) in [0, 1, 2, 2, 4, 4, 3, 0, 4, 1024].into_iter().enumerate() {
            assert_eq!(states[&hosts[i]], expected);
        }
        assert!(queries.users[&0].0);
        assert!(!queries.users[&10].0);
        for (host, state) in &queries.users[&10].1 {
            let index = hosts.iter().position(|h| h == host).unwrap();
            assert_eq!(
                *state,
                if matches!(index, 1 | 2 | 4 | 5 | 8) {
                    2
                } else {
                    1
                }
            );
        }
        assert!(
            owner
                .queries(&code, true, &SystemConfig::default(), [10, 10])
                .is_err()
        );
        assert!(
            owner
                .queries(&code, true, &SystemConfig::default(), [-1])
                .is_err()
        );
        let mut web_only = code.clone();
        web_only.activities[0].main.component.intents[0]
            .filter
            .auto_verify = false;
        let queries = owner
            .queries(&web_only, true, &SystemConfig::default(), [0])
            .unwrap()
            .unwrap();
        assert!(queries.verification.is_none());
        assert_eq!(queries.users[&0].1.len(), hosts.len());
    }
}
