//! Registration owners from pinned ComponentResolver, PackageProperty and PMS.
//! Copyright AOSP, Apache-2.0. Entries follow accepted scan completion order.
use super::{
    pkg::{Instrumentation, Property, Provider},
    scan::LoadedPackage,
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug, PartialEq)]
pub struct RegisteredInstrumentation {
    pub package: String,
    pub value: Instrumentation,
}
#[derive(Clone, Debug, PartialEq)]
pub struct RegisteredProvider {
    pub package: String,
    pub value: Provider,
}
#[derive(Clone, Debug, PartialEq)]
struct PropertyGroup {
    kind: i32,
    name: String,
    packages: Vec<(String, Vec<Property>)>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Registry {
    sources: BTreeMap<String, Arc<LoadedPackage>>,
    committed_packages: Vec<String>,
    package_order: Vec<String>,
    instruments: Vec<RegisteredInstrumentation>,
    providers: Vec<RegisteredProvider>,
    declaration_providers: BTreeMap<String, Vec<Provider>>,
    authority_objects: Vec<RegisteredProvider>,
    base_authority_objects: std::collections::BTreeSet<usize>,
    authorities: Vec<(String, usize)>,
    properties: Vec<PropertyGroup>,
}
fn hash(package: &str, class: &str) -> i32 {
    super::info::java_hash(package).wrapping_add(super::info::java_hash(class))
}
impl Registry {
    pub fn validate(&self, loaded: &BTreeMap<String, Arc<LoadedPackage>>) -> Result<(), String> {
        if self.sources.len() != loaded.len()
            || self.package_order.len() != self.sources.len()
            || self.package_order.iter().collect::<std::collections::BTreeSet<_>>().len()
                != self.sources.len()
            || self.package_order.iter().any(|name| !self.sources.contains_key(name))
            || self.sources.iter().any(|(name, code)| {
                !loaded
                    .get(name)
                    .is_some_and(|other| Arc::ptr_eq(code, other))
            })
        {
            return Err("registered code generation differs from loaded packages".into());
        }
        Ok(())
    }
    pub fn package_view(
        &self,
        name: &str,
        raw: &super::pkg::AndroidPackage,
    ) -> Result<super::pkg::AndroidPackage, String> {
        let providers = self
            .declaration_providers
            .get(name)
            .ok_or("registered declaration provider view is unavailable")?;
        if providers.len() != raw.providers.len() {
            return Err("registered provider declaration inventory differs".into());
        }
        let mut view = raw.clone();
        view.providers = providers.clone();
        Ok(view)
    }
    /// PMS mPackages ArrayMap order: signed hash, stable insertion for collisions.
    pub fn ordered_package_names(&self) -> Vec<&str> {
        let mut names = self.package_order.iter().map(String::as_str).collect::<Vec<_>>();
        names.sort_by_key(|name| super::info::java_hash(name));
        names
    }
    pub(in crate::package) fn rebind(&mut self, name: &str, code: Arc<LoadedPackage>) {
        self.sources.insert(name.into(), code);
    }

    pub fn committed_packages(&self) -> &[String] { &self.committed_packages }

    /// ComponentResolver.getActivity: registration existence, independent of
    /// user visibility, enabled state and whether an activity declares filters.
    pub(crate) fn contains_activity(&self, component: &super::intent::ComponentName) -> bool {
        self.committed_packages.contains(&component.package)
            && self.sources.get(&component.package).is_some_and(|code| {
                code.runtime_package().activities.iter().any(|activity| {
                    activity.main.component.name == component.class
                })
            })
    }


    pub fn instruments(&self) -> Vec<&RegisteredInstrumentation> {
        let mut entries = self.instruments.iter().collect::<Vec<_>>();
        entries.sort_by_key(|i| hash(&i.package, &i.value.component.name));
        entries
    }
    pub fn providers(&self) -> Vec<&RegisteredProvider> {
        let mut entries = self.providers.iter().collect::<Vec<_>>();
        entries.sort_by_key(|p| hash(&p.package, &p.value.main.component.name));
        entries
    }
    pub fn ordered_authorities(&self) -> Vec<(&str, &RegisteredProvider)> {
        let mut authorities = self.authorities.iter().map(|(name, id)| (name.as_str(), &self.authority_objects[*id])).collect::<Vec<_>>();
        authorities.sort_by_key(|(name, _)| super::info::java_hash(name));
        authorities
    }
    pub fn authority(&self, name: &str) -> Option<&RegisteredProvider> {
        self.authorities
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, id)| &self.authority_objects[*id])
    }
    pub fn property(&self, name: &str, package: &str, class: Option<&str>) -> Option<&Property> {
        for kind in if class.is_some() {
            vec![1, 4, 2, 3]
        } else {
            vec![5]
        } {
            let Some(group) = self
                .properties
                .iter()
                .find(|g| g.kind == kind && g.name == name)
            else {
                continue;
            };
            let Some((_, properties)) = group.packages.iter().find(|(n, _)| n == package) else {
                continue;
            };
            if class.is_none() {
                return properties.first();
            }
            if let Some(property) = properties
                .iter()
                .rev()
                .find(|p| p.class_name.as_deref() == class)
            {
                return Some(property);
            }
        }
        None
    }
    pub fn property_packages(&self, name: &str, kind: i32) -> Vec<(&str, &[Property])> {
        let Some(group) = self
            .properties
            .iter()
            .find(|g| g.kind == kind && g.name == name)
        else {
            return Vec::new();
        };
        let mut packages = group
            .packages
            .iter()
            .map(|(name, properties)| (name.as_str(), properties.as_slice()))
            .collect::<Vec<_>>();
        packages.sort_by_key(|(name, _)| super::info::java_hash(name));
        packages
    }
    /// Replacement removes the old registration before admitting the new code.
    pub fn register(&mut self, code: Arc<LoadedPackage>) -> Result<(), String> {
        let package = &code.package;
        for properties in package
            .properties
            .iter()
            .chain(
                package
                    .activities
                    .iter()
                    .filter_map(|c| c.main.component.properties.as_ref()),
            )
            .chain(
                package
                    .providers
                    .iter()
                    .filter_map(|c| c.main.component.properties.as_ref()),
            )
            .chain(
                package
                    .receivers
                    .iter()
                    .filter_map(|c| c.main.component.properties.as_ref()),
            )
            .chain(
                package
                    .services
                    .iter()
                    .filter_map(|c| c.main.component.properties.as_ref()),
            )
        {
            if properties.iter().any(|(_, p)| {
                p.name.is_none()
                    || p.package_name.is_none()
                    || matches!(p.value, super::pkg::PropertyValue::Unknown(_))
            }) {
                return Err("registered property owner is incomplete".into());
            }
        }
        // InstallPackageHelper commits with mPackages.put: replacement retains
        // its ArrayMap slot. An explicit removal followed by admission moves it.
        let position = self.package_order.iter().position(|name| name == &package.package_name);
        self.remove(&package.package_name);
        self.committed_packages.push(package.package_name.clone());
        if let Some(position) = position {
            self.package_order.insert(position, package.package_name.clone());
        } else {
            self.package_order.push(package.package_name.clone());
        }
        self.sources
            .insert(package.package_name.clone(), code.clone());
        for instrument in &package.instrumentations {
            let mut value = instrument.clone();
            value.component.package_name = package.package_name.clone();
            let row = RegisteredInstrumentation {
                package: package.package_name.clone(),
                value,
            };
            if let Some(i) = self.instruments.iter().position(|p| {
                p.package == row.package && p.value.component.name == row.value.component.name
            }) {
                self.instruments[i] = row;
            } else {
                self.instruments.push(row);
            }
        }
        let mut bases = Vec::new();
        for provider in &package.providers {
            bases.push(self.add_provider(&package.package_name, provider));
        }
        self.declaration_providers
            .insert(package.package_name.clone(), bases);
        self.add_properties(5, &package.properties);
        for c in &package.activities {
            self.add_properties(1, &c.main.component.properties);
        }
        for c in &package.providers {
            self.add_properties(4, &c.main.component.properties);
        }
        for c in &package.receivers {
            self.add_properties(2, &c.main.component.properties);
        }
        for c in &package.services {
            self.add_properties(3, &c.main.component.properties);
        }
        self.compact_authorities();
        Ok(())
    }
    pub fn remove(&mut self, package: &str) {
        self.sources.remove(package);
        self.package_order.retain(|name| name != package);
        self.declaration_providers.remove(package);
        self.instruments.retain(|p| p.package != package);
        self.providers.retain(|p| p.package != package);
        // ComponentResolver removes only entries whose object is the declared
        // provider itself. Syncable authority copies survive package removal.
        self.authorities.retain(|(_, id)| {
            !self.base_authority_objects.contains(id)
                || self.authority_objects[*id].package != package
        });
        for group in &mut self.properties {
            group.packages.retain(|(name, _)| name != package);
        }
        self.properties.retain(|g| !g.packages.is_empty());
        self.compact_authorities();
    }
    fn compact_authorities(&mut self) {
        let mut ids = BTreeMap::new();
        let mut retained = Vec::new();
        for (index, provider) in std::mem::take(&mut self.authority_objects)
            .into_iter()
            .enumerate()
        {
            if self.authorities.iter().any(|(_, id)| *id == index) {
                ids.insert(index, retained.len());
                retained.push(provider);
            }
        }
        for (_, id) in &mut self.authorities {
            *id = ids[id];
        }
        self.base_authority_objects = self.base_authority_objects.iter()
            .filter_map(|old| ids.get(old).copied()).collect();
        self.authority_objects = retained;
    }
    fn add_properties(&mut self, kind: i32, properties: &Option<Vec<(String, Property)>>) {
        for (_, property) in properties.iter().flatten() {
            let (Some(name), Some(package)) = (&property.name, &property.package_name) else {
                continue;
            };
            let i = match self
                .properties
                .iter()
                .position(|g| g.kind == kind && g.name == *name)
            {
                Some(i) => i,
                None => {
                    self.properties.push(PropertyGroup {
                        kind,
                        name: name.clone(),
                        packages: vec![],
                    });
                    self.properties.len() - 1
                }
            };
            let group = &mut self.properties[i];
            let j = match group.packages.iter().position(|(n, _)| n == package) {
                Some(j) => j,
                None => {
                    group.packages.push((package.clone(), vec![]));
                    group.packages.len() - 1
                }
            };
            group.packages[j].1.push(property.clone());
        }
    }
    fn add_provider(&mut self, package: &str, source: &Provider) -> Provider {
        let mut provider = source.clone();
        provider.main.component.package_name = package.into();
        let mut names = provider
            .authority
            .as_deref()
            .map(|a| a.split(';').collect::<Vec<_>>())
            .unwrap_or_default();
        while names.last() == Some(&"") {
            names.pop();
        }
        let names = names.into_iter().map(str::to_owned).collect::<Vec<_>>();
        provider.authority = None;
        let base = self.authority_objects.len();
        self.base_authority_objects.insert(base);
        self.authority_objects.push(RegisteredProvider {
            package: package.into(),
            value: provider,
        });
        let mut current = base;
        for (index, name) in names.iter().enumerate() {
            if index == 1 && self.authority_objects[current].value.syncable {
                let mut copy = self.authority_objects[current].clone();
                copy.value.syncable = false;
                current = self.authority_objects.len();
                self.authority_objects.push(copy);
            }
            if self.authorities.iter().any(|(n, _)| n == name) {
                continue;
            }
            let value = &mut self.authority_objects[current].value;
            value.authority = Some(match value.authority.take() {
                None => name.clone(),
                Some(old) => format!("{old};{name}"),
            });
            self.authorities.push((name.clone(), current));
        }
        let row = self.authority_objects[base].clone();
        if let Some(i) = self.providers.iter().position(|p| {
            p.package == package && p.value.main.component.name == row.value.main.component.name
        }) {
            self.providers[i] = row;
        } else {
            self.providers.push(row);
        }
        self.authority_objects[base].value.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        pkg::{AndroidPackage, Component, MainComponent, PropertyValue},
        sign::SigningDetails,
    };
    fn code(package: AndroidPackage) -> Arc<LoadedPackage> {
        Arc::new(LoadedPackage::new(package, SigningDetails::unknown()).unwrap())
    }
    fn property(package: &str, class: Option<&str>, value: i32) -> Option<Vec<(String, Property)>> {
        Some(vec![(
            "property".into(),
            Property {
                name: Some("property".into()),
                package_name: Some(package.into()),
                class_name: class.map(str::to_owned),
                value: PropertyValue::Int(value),
            },
        )])
    }
    fn provider(package: &str, name: &str, authority: &str, syncable: bool) -> Provider {
        Provider {
            main: MainComponent {
                component: Component {
                    name: name.into(),
                    package_name: package.into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            authority: Some(authority.into()),
            syncable,
            ..Default::default()
        }
    }
    #[test]
    fn activity_definition_includes_registered_activities_without_filters() {
        let component = super::super::intent::ComponentName { package: "p".into(), class: "p.Main".into() };
        let mut registry = Registry::default();
        registry.register(code(AndroidPackage {
            package_name: "p".into(),
            activities: vec![super::super::pkg::Activity {
                main: MainComponent { component: Component {
                    name: component.class.clone(), package_name: component.package.clone(),
                    ..Default::default()
                }, ..Default::default() },
                ..Default::default()
            }],
            ..Default::default()
        })).unwrap();
        assert!(registry.contains_activity(&component));
        assert!(!registry.contains_activity(&super::super::intent::ComponentName {
            package: "p".into(), class: "p.Removed".into(),
        }));
        registry.remove("p");
        assert!(!registry.contains_activity(&component));
    }

    #[test]
    fn registration_collision_order_and_removal_reinsertion_are_owned() {
        let mut registry = Registry::default();
        let first = code(AndroidPackage {
            package_name: "BB".into(),
            properties: property("BB", None, 1),
            instrumentations: ["BB", "Aa"]
                .map(|name| Instrumentation {
                    component: Component {
                        name: name.into(),
                        package_name: "BB".into(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .into(),
            ..Default::default()
        });
        let second = code(AndroidPackage {
            package_name: "Aa".into(),
            properties: property("Aa", None, 2),
            ..Default::default()
        });
        registry.register(first.clone()).unwrap();
        registry.register(second.clone()).unwrap();
        assert_eq!(registry.ordered_package_names(), ["BB", "Aa"]);
        assert_eq!(
            registry
                .property_packages("property", 5)
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>(),
            vec!["BB", "Aa"]
        );
        assert_eq!(
            registry
                .instruments()
                .iter()
                .map(|r| r.value.component.name.as_str())
                .collect::<Vec<_>>(),
            vec!["BB", "Aa"]
        );
        registry
            .validate(&[("BB".into(), first.clone()), ("Aa".into(), second.clone())].into())
            .unwrap();
        let frozen = registry.clone();
        registry.remove("BB");
        registry.register(first.clone()).unwrap();
        assert_eq!(registry.ordered_package_names(), ["Aa", "BB"]);
        assert_eq!(frozen.ordered_package_names(), ["BB", "Aa"]);
        assert_eq!(
            registry
                .property_packages("property", 5)
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>(),
            vec!["Aa", "BB"]
        );
        assert_eq!(
            frozen
                .property_packages("property", 5)
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>(),
            vec!["BB", "Aa"]
        );
        let other = code(first.package.clone());
        assert!(
            registry
                .validate(&[("BB".into(), other), ("Aa".into(), second)].into())
                .is_err()
        );
    }
    #[test]
    fn package_arraymap_replacement_retains_slot_but_removal_moves_it() {
        let first = code(AndroidPackage { package_name: "BB".into(), ..Default::default() });
        let second = code(AndroidPackage { package_name: "Aa".into(), ..Default::default() });
        let negative = code(AndroidPackage { package_name: "negative-package-hash".into(), ..Default::default() });
        let mut registry = Registry::default();
        registry.register(first.clone()).unwrap();
        registry.register(second.clone()).unwrap();
        assert_eq!(registry.ordered_package_names(), ["BB", "Aa"]);
        registry.register(first.clone()).unwrap();
        assert_eq!(registry.ordered_package_names(), ["BB", "Aa"]);
        registry.rebind("BB", first.clone());
        assert_eq!(registry.ordered_package_names(), ["BB", "Aa"]);
        registry.register(negative.clone()).unwrap();
        let expected = if super::super::info::java_hash("negative-package-hash") < super::super::info::java_hash("BB") {
            vec!["negative-package-hash", "BB", "Aa"]
        } else { vec!["BB", "Aa", "negative-package-hash"] };
        assert_eq!(registry.ordered_package_names(), expected);
        registry.remove("BB");
        registry.register(first.clone()).unwrap();
        let names = registry.ordered_package_names();
        assert!(names.iter().position(|name| *name == "Aa") < names.iter().position(|name| *name == "BB"));
        registry.validate(&[("BB".into(), first), ("Aa".into(), second), ("negative-package-hash".into(), negative)].into()).unwrap();
    }
    #[test]
    fn syncable_provider_registration_retains_base_and_alias_copy_objects() {
        let mut registry = Registry::default();
        registry
            .register(code(AndroidPackage {
                package_name: "first".into(),
                providers: vec![provider("first", "First", "one", false)],
                ..Default::default()
            }))
            .unwrap();
        let package = code(AndroidPackage {
            package_name: "sync".into(),
            providers: vec![provider("sync", "Sync", "one;two;three;", true)],
            ..Default::default()
        });
        registry.register(package.clone()).unwrap();
        let base = registry
            .providers()
            .into_iter()
            .find(|p| p.package == "sync")
            .unwrap();
        assert_eq!(base.value.authority, None);
        assert!(base.value.syncable);
        let alias = registry.authority("two").unwrap();
        assert_eq!(alias.value.authority.as_deref(), Some("two;three"));
        assert!(!alias.value.syncable);
        assert_eq!(registry.authority("one").unwrap().package, "first");
        registry.remove("first");
        assert!(registry.authority("one").is_none());
        registry.register(package).unwrap();
        assert_eq!(
            registry
                .authority("one")
                .unwrap()
                .value
                .authority
                .as_deref(),
            Some("one")
        );
        assert!(registry.authority("one").unwrap().value.syncable);
        assert_eq!(
            registry
                .authority("three")
                .unwrap()
                .value
                .authority
                .as_deref(),
            Some("two;three")
        );
        assert!(!registry.authority("three").unwrap().value.syncable);
        registry.remove("sync");
        assert!(registry.authority("one").is_none());
        assert_eq!(registry.authority("two").unwrap().value.authority.as_deref(), Some("two;three"));
        registry.register(code(AndroidPackage { package_name: "replacement".into(),
            providers: vec![provider("replacement", "Next", "two;six", true)], ..Default::default() })).unwrap();
        assert_eq!(registry.providers().into_iter().find(|provider| provider.package == "replacement").unwrap().value.authority, None);
        assert_eq!(registry.authority("two").unwrap().package, "sync");
        assert_eq!(registry.authority("six").unwrap().package, "replacement");
    }
    #[test]
    fn runtime_provider_declarations_preserve_raw_code_and_exclude_alias_copies() {
        let mut registry = Registry::default();
        registry
            .register(code(AndroidPackage {
                package_name: "first".into(),
                providers: vec![provider("first", "First", "one", false)],
                ..Default::default()
            }))
            .unwrap();
        let mut loaded = code(AndroidPackage {
            package_name: "sync".into(),
            feature_flag_state: Some(vec![]),
            providers: vec![
                provider("sync", "Sync", "one;two;three", true),
                provider("sync", "Second", "four;five", false),
            ],
            ..Default::default()
        });
        registry.register(loaded.clone()).unwrap();
        let view = registry.package_view("sync", &loaded.package).unwrap();
        Arc::make_mut(&mut loaded).set_runtime_package(view);
        registry.rebind("sync", loaded.clone());
        assert_eq!(
            loaded.package.providers[0].authority.as_deref(),
            Some("one;two;three")
        );
        assert_eq!(loaded.runtime_package().providers.len(), 2);
        assert_eq!(loaded.runtime_package().providers[0].authority, None);
        assert!(loaded.runtime_package().providers[0].syncable);
        assert_eq!(
            loaded.runtime_package().providers[1].authority.as_deref(),
            Some("four;five")
        );
        assert_eq!(
            registry
                .authority("two")
                .unwrap()
                .value
                .authority
                .as_deref(),
            Some("two;three")
        );
        assert!(!registry.authority("two").unwrap().value.syncable);
        let facade = loaded.facade_entry().unwrap();
        let decoded = AndroidPackage::read_cache_entry(&facade.cache.bytes).unwrap();
        assert_eq!(decoded.providers, loaded.runtime_package().providers);
        assert_ne!(decoded.providers, loaded.package.providers);
        registry.validate(&registry.sources.clone()).unwrap();
    }
    #[test]
    fn property_component_precedence_and_last_component_declaration_match_owner() {
        let mut package = AndroidPackage {
            package_name: "p".into(),
            properties: property("p", None, 0),
            ..Default::default()
        };
        package.activities = [1, 2]
            .map(|value| crate::package::pkg::Activity {
                main: MainComponent {
                    component: Component {
                        name: "Class".into(),
                        package_name: "p".into(),
                        properties: property("p", Some("Class"), value),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            })
            .into();
        let mut registry = Registry::default();
        registry.register(code(package)).unwrap();
        assert_eq!(
            registry.property("property", "p", None).unwrap().value,
            PropertyValue::Int(0)
        );
        assert_eq!(
            registry
                .property("property", "p", Some("Class"))
                .unwrap()
                .value,
            PropertyValue::Int(2)
        );
        assert_eq!(registry.property_packages("property", 1)[0].1.len(), 2);
    }
}

#[cfg(test)]
#[path = "registry_original_test.rs"]
mod original_test;
