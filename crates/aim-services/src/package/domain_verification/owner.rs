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

/// A request for the verifier driver; this does not claim broadcast delivery.
#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    pub broadcast_requested: bool,
    pub recovered_missing_owner: bool,
}
#[derive(Clone, Debug)]
pub struct Owner {
    saved: State,
    attached: Vec<Package>,
    ids: BTreeMap<String, String>,
    legacy_info: BTreeMap<String, i32>,
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
    pub fn package(&self, name: &str) -> Option<&Package> {
        self.attached.iter().find(|p| p.name == name)
    }
    pub fn package_by_id(&self, id: &str) -> Option<&Package> {
        self.ids
            .get(&id.to_ascii_lowercase())
            .and_then(|name| self.package(name))
    }
    pub fn remove(&mut self, name: &str) -> Option<Package> {
        let i = self.attached.iter().position(|p| p.name == name)?;
        let p = self.attached.remove(i);
        if self.ids.get(&p.id).is_some_and(|value| value == name) {
            self.ids.remove(&p.id);
        }
        Some(p)
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
}
