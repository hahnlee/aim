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
    instruments: Vec<RegisteredInstrumentation>,
    providers: Vec<RegisteredProvider>,
    authority_objects: Vec<RegisteredProvider>,
    authorities: Vec<(String, usize)>,
    properties: Vec<PropertyGroup>,
}
fn hash(package: &str, class: &str) -> i32 {
    super::info::java_hash(package).wrapping_add(super::info::java_hash(class))
}
impl Registry {
    pub fn validate(&self, loaded: &BTreeMap<String, Arc<LoadedPackage>>) -> Result<(), String> {
        if self.sources.len() != loaded.len()
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
        self.remove(&package.package_name);
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
        for provider in &package.providers {
            self.add_provider(&package.package_name, provider);
        }
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
        self.instruments.retain(|p| p.package != package);
        self.providers.retain(|p| p.package != package);
        self.authorities
            .retain(|(_, id)| self.authority_objects[*id].package != package);
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
    fn add_provider(&mut self, package: &str, source: &Provider) {
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
            Some("one;two;three")
        );
        assert!(!registry.authority("three").unwrap().value.syncable);
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
