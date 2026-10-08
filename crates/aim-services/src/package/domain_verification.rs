//! Domain verification's state in `packages.xml`, as
//! `DomainVerificationPersistence` and `DomainVerificationLegacySettings`
//! read it at `android-16.0.0_r1`. The active and restored maps, and the
//! legacy per-user statuses, are separate from each package's domain set
//! id and user restriction state.

pub mod collector;
pub mod agent;
pub mod backup;
mod legacy_read;
mod read;
pub use read::ReadResult;
pub use legacy_read::SectionError;
pub mod domain_set;
pub mod enforcer;
pub mod owner;
pub mod original_bridge;
pub mod names;
pub mod parcels;
pub mod service;
pub mod uri_groups;
pub mod uri_bundle;
pub mod uri_parcel;
pub mod uuid;

use aim_android_xml::Element;

use super::intent_filter::UriRelativeFilterGroup;

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
        let bytes = aim_android_xml::abx::write(root)?;
        let mut reader = aim_android_xml::pull::Reader::new(&bytes)?;
        reader.next()?;
        let result = Self::read_events(&mut reader, |id| uuid::parse(id, true).map_err(crate::package::settings::ReadError::File))
            .map_err(|error| error.to_string())?;
        if let Some(error) = result.diagnostics.first() { return Err(error.message.clone()); }
        for package in result.state.active { put(&mut self.active, package.name.clone(), package, |p| &p.name); }
        for package in result.state.restored { put(&mut self.restored, package.name.clone(), package, |p| &p.name); }
        Ok(())
    }

    /// `DomainVerificationLegacySettings.readSettings`.
    pub fn read_legacy(&mut self, root: &Element) -> Result<(), String> {
        let bytes = aim_android_xml::abx::write(root)?;
        let mut reader = aim_android_xml::pull::Reader::new(&bytes)?;
        reader.next()?;
        let errors = self.read_legacy_events(&mut reader);
        if let Some(error) = errors.first() { return Err(error.message.clone()); }
        Ok(())
    }
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
    fn domain_xml_skips_missing_filter_and_keeps_empty_filter() {
        let xml = b"<domain-verifications><active><package-state packageName='x' id='00000000-0000-0000-0000-000000000001'><uri-relative-filter-groups><domain name='x.example'><uri-relative-filter-group action='0'><uri-relative-filter uri-part='0' pattern-type='0'/><uri-relative-filter uri-part='0' pattern-type='0' filter=''/></uri-relative-filter-group></domain></uri-relative-filter-groups></package-state></active></domain-verifications>";
        let mut state = super::State::default(); state.read(&aim_android_xml::read(xml).unwrap()).unwrap();
        let filters = &state.active[0].uri_relative_filter_groups[0].1[0].filters;
        assert_eq!(filters.len(), 1); assert_eq!(filters[0].filter.as_deref(), Some(""));
        state.active[0].uri_relative_filter_groups[0].1[0].add_nullable(0, 0, None);
        let document = aim_android_xml::read(b"<packages/>").unwrap();
        let written = crate::package::owner::domains::replace(&document, &state).unwrap();
        let saved = crate::package::settings::Settings::parse(&written).unwrap().domain_verification;
        assert_eq!(saved.active[0].uri_relative_filter_groups[0].1[0].filters.len(), 1);
        assert_eq!(state.active[0].uri_relative_filter_groups[0].1[0].filters.len(), 2);
    }
}
