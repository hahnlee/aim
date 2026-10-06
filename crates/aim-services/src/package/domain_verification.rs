//! Domain verification's state in `packages.xml`, as
//! `DomainVerificationPersistence` and `DomainVerificationLegacySettings`
//! read it at `android-16.0.0_r1`. The active and restored maps, and the
//! legacy per-user statuses, are separate from each package's domain set
//! id and user restriction state.

pub mod collector;
pub mod domain_set;
pub mod enforcer;
pub mod owner;
pub mod names;
pub mod parcels;
pub mod service;
pub mod uri_groups;
pub mod uri_bundle;
pub mod uri_parcel;
pub mod uuid;

use aim_android_xml::Element;

use super::intent_filter::UriRelativeFilterGroup;
use super::{children, string};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub active: Vec<Package>,
    pub restored: Vec<Package>,
    /// Package name and the legacy user id/status map. A missing package
    /// name is a null key in the original's `ArrayMap`.
    pub legacy: Vec<(Option<String>, Vec<(i32, i32)>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub name: String,
    pub id: String,
    pub has_auto_verify_domains: bool,
    pub signature: Option<String>,
    pub domains: Vec<(Option<String>, i32)>,
    pub users: Vec<User>,
    pub uri_relative_filter_groups: Vec<(Option<String>, Vec<UriRelativeFilterGroup>)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct User {
    pub id: i32,
    pub allow_link_handling: bool,
    pub enabled_hosts: Vec<String>,
}

impl State {
    /// DomainVerificationService.clearPackage and Settings.removePackage.
    /// The original keeps its separate legacy migration state.
    pub fn clear_package(&mut self, name: &str) {
        self.active.retain(|p| p.name != name);
        self.restored.retain(|p| p.name != name);
    }
    /// `DomainVerificationPersistence.readFromXml`.
    pub fn read(&mut self, root: &Element) -> Result<(), String> {
        for section in root.children() {
            let target = match section.name.as_str() {
                "active" => &mut self.active,
                "restored" => &mut self.restored,
                _ => continue,
            };
            for element in children(section, "package-state") {
                if let Some(package) = package(element)? {
                    put(target, package.name.clone(), package, |p| &p.name);
                }
            }
        }
        Ok(())
    }

    /// `DomainVerificationLegacySettings.readSettings`.
    pub fn read_legacy(&mut self, root: &Element) -> Result<(), String> {
        for section in children(root, "user-states") {
            let name = string(section, "packageName");
            let index = match self.legacy.iter().position(|(key, _)| *key == name) {
                Some(index) => index,
                None => {
                    self.legacy.push((name, Vec::new()));
                    self.legacy.len() - 1
                }
            };
            let users = &mut self.legacy[index].1;
            for child in children(section, "user-state") {
                let id = number(child, "userId", -1);
                let status = number(child, "state", -1);
                put(users, id, (id, status), |(id, _)| id);
            }
        }
        Ok(())
    }
}

fn package(e: &Element) -> Result<Option<Package>, String> {
    let (Some(name), Some(id)) = (string(e, "packageName"), string(e, "id")) else {
        return Ok(None);
    };
    if name.is_empty() || id.is_empty() {
        return Ok(None);
    }
    // `UUID.fromString`: the writer emits the canonical form, and a bad
    // id makes the original reject this settings file.
    let groups: Vec<_> = id.split('-').collect();
    if groups.len() != 5
        || groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .any(|(g, n)| g.len() != n || !g.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(format!("<package-state> invalid id {id}"));
    }
    let mut package = Package {
        name,
        id: id.to_ascii_lowercase(),
        has_auto_verify_domains: boolean(e, "hasAutoVerifyDomains", false),
        signature: string(e, "signature"),
        domains: Vec::new(),
        users: Vec::new(),
        uri_relative_filter_groups: Vec::new(),
    };
    for section in e.children() {
        match section.name.as_str() {
            "state" => {
                for domain in children(section, "domain") {
                    let name = string(domain, "name");
                    let state = number(domain, "state", 0);
                    put(
                        &mut package.domains,
                        name.clone(),
                        (name, state),
                        |(name, _)| name,
                    );
                }
            }
            "user-states" => {
                for user in children(section, "user-state") {
                    let id = number(user, "userId", -1);
                    if id == -1 {
                        continue;
                    }
                    let mut enabled_hosts = Vec::new();
                    for hosts in children(user, "enabled-hosts") {
                        for host in children(hosts, "host") {
                            if let Some(name) = string(host, "name").filter(|n| !n.is_empty())
                                && !enabled_hosts.contains(&name)
                            {
                                enabled_hosts.push(name);
                            }
                        }
                    }
                    put(
                        &mut package.users,
                        id,
                        User {
                            id,
                            allow_link_handling: boolean(user, "allowLinkHandling", false),
                            enabled_hosts,
                        },
                        |u| &u.id,
                    );
                }
            }
            "uri-relative-filter-groups" => {
                for domain in children(section, "domain") {
                    let name = string(domain, "name");
                    let mut groups = Vec::new();
                    for group in children(domain, "uri-relative-filter-group") {
                        // SettingsXml sections share the parser cursor, which is
                        // on the group when the original reads its action.
                        let mut parsed = UriRelativeFilterGroup::new(number(group, "action", -1));
                        for filter in children(group, "uri-relative-filter") {
                            let value = string(filter, "filter");
                            parsed.add_nullable(number(filter, "uri-part", -1), number(filter, "pattern-type", -1), value.as_deref());
                        }
                        groups.push(parsed);
                    }
                    put(
                        &mut package.uri_relative_filter_groups,
                        name.clone(),
                        (name, groups),
                        |(name, _)| name,
                    );
                }
            }
            _ => {}
        }
    }
    Ok(Some(package))
}

/// `ArrayMap.put`/`SparseArray.put`: replace a value without changing the
/// key's position.
fn put<K: PartialEq, V>(items: &mut Vec<V>, key: K, value: V, get: impl Fn(&V) -> &K) {
    match items.iter().position(|v| *get(v) == key) {
        Some(i) => items[i] = value,
        None => items.push(value),
    }
}

// TypedXmlPullParser's defaulted accessors use the supplied default on bad types.
fn number(e: &Element, key: &str, default: i32) -> i32 {
    e.int(key).ok().flatten().unwrap_or(default)
}
fn boolean(e: &Element, key: &str, default: bool) -> bool {
    e.bool(key).ok().flatten().unwrap_or(default)
}

#[cfg(test)]
mod nullable_filter_tests {
    #[test]
    fn domain_xml_missing_and_empty_filter_remain_distinct() {
        let xml = b"<domain-verifications><active><package-state packageName='x' id='00000000-0000-0000-0000-000000000001'><uri-relative-filter-groups><domain name='x.example'><uri-relative-filter-group action='0'><uri-relative-filter uri-part='0' pattern-type='0'/><uri-relative-filter uri-part='0' pattern-type='0' filter=''/></uri-relative-filter-group></domain></uri-relative-filter-groups></package-state></active></domain-verifications>";
        let mut state = super::State::default(); state.read(&aim_android_xml::read(xml).unwrap()).unwrap();
        let filters = &state.active[0].uri_relative_filter_groups[0].1[0].filters;
        assert_eq!(filters.len(), 2); assert_eq!(filters[0].filter, None); assert_eq!(filters[1].filter.as_deref(), Some(""));
        let document = aim_android_xml::read(b"<packages/>").unwrap();
        assert!(crate::package::owner::domains::replace(&document, &state).is_err());
    }
}
