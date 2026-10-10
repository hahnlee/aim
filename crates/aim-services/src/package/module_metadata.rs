//! Native ModuleInfoProvider, android-16.0.0_r1. Copyright AOSP, Apache-2.0.
//! Resource configuration and APK/APEX registration are supplied by their owners.
use super::{
    model::State,
    parse::{
        Platform,
        resources::{Config, Resources, Selected, TYPE_REFERENCE, TYPE_STRING, Table},
    },
    pkg::{AndroidPackage, Value as Metadata},
};
use aim_apps::{
    apk::Apk,
    res::{self, Element, Value},
};
use aim_binder_host::parcel::{Parcel, Reader, Result as ParcelResult};
use aim_service_aidl::{ReadParcelable, WriteParcelable};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct ModuleInfo {
    pub name: Option<String>,
    pub package: Option<String>,
    pub hidden: bool,
    pub apex: Option<String>,
    pub apks: Option<Vec<Option<String>>>,
}
impl WriteParcelable for ModuleInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(1);
        p.write_string8(self.name.as_deref());
        p.write_string16(self.package.as_deref());
        p.write_bool(self.hidden);
        p.write_string16(self.apex.as_deref());
        aim_service_aidl::write_string_list(p, self.apks.as_deref());
    }
}
impl ReadParcelable for ModuleInfo {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let kind = r.read_i32()?;
        let name = r.read_string8()?;
        if kind != 1 && name.is_some() {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        }
        Ok(Self {
            name,
            package: r.read_string16()?,
            hidden: r.read_bool()?,
            apex: r.read_string16()?,
            apks: aim_service_aidl::read_string_list(r)?,
        })
    }
}

/// The maps ApexManager owns after the native scan notifications.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApexLinks {
    modules: BTreeMap<String, Option<String>>,
    apks: BTreeMap<Option<String>, Vec<String>>,
}
impl ApexLinks {
    pub fn from_scan(
        results: &[super::scan::ApexScanResult],
        active: &super::bootstrap::ApexInventory,
        registrations: &[AndroidPackage],
    ) -> Result<Self, String> {
        let mut links = Self::default();
        for active in &active.active {
            if !results.iter().any(|r| {
                r.info.active
                    && r.info.module_path == active.module_path
                    && r.info.module_name == active.module_name
            }) {
                return Err("active APEX has no accepted native scan notification".into());
            }
        }
        for result in results {
            links.modules.insert(
                result.package.package_name.clone(),
                result.info.module_name.clone(),
            );
        }
        // Preserve the APK registration order; do not reconstruct it from a map.
        for package in registrations {
            let path = package
                .base_apk_path
                .as_ref()
                .ok_or("registered APK has no base path")?;
            for apex in &active.active {
                if path.starts_with(&format!("{}/", apex.mount_path)) {
                    links
                        .apks
                        .entry(apex.module_name.clone())
                        .or_default()
                        .push(package.package_name.clone());
                }
            }
        }
        Ok(links)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Owner {
    provider: Option<String>,
    loaded: bool,
    modules: Vec<ModuleInfo>,
    diagnostic: Option<String>,
}
impl Owner {
    pub fn loaded(&self) -> bool {
        self.loaded
    }
    pub fn provider(&self) -> Option<&str> {
        self.provider.as_deref()
    }
    pub fn modules(&self) -> &[ModuleInfo] {
        &self.modules
    }
    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }

    /// Like systemReady, this freezes text in the applied configuration.
    /// The file resolver must map accepted guest APK paths to the owned image/data.
    pub fn load(
        state: &State,
        platform: &Platform,
        config: Config,
        files: &dyn Fn(&str) -> Result<PathBuf, String>,
        apex: &ApexLinks,
    ) -> Result<Self, String> {
        let framework = Resources {
            tables: vec![&platform.framework],
            overlays: &platform.framework_overlays,
            config: config.clone(),
        };
        let id = platform
            .framework
            .id("string", "config_defaultModuleMetadataProvider")
            .ok_or("module metadata provider configuration is unavailable")?;
        let provider = framework
            .resource_string(id)
            .ok_or("module metadata provider configuration is not text")?;
        let mut owner = Self {
            provider: Some(provider.clone()),
            loaded: false,
            modules: Vec::new(),
            diagnostic: None,
        };
        if provider.is_empty() {
            owner.diagnostic = Some("no configured module metadata provider".into());
            return Ok(owner);
        }
        let filter = super::apps_filter::AppsFilter::new(
            state,
            &super::apps_filter::Config {
                force_system_packages_queryable: state.system.force_system_packages_queryable,
                force_queryable_packages: state.system.force_queryable_packages.clone(),
            },
        )
        .map_err(|e| format!("module provider visibility: {e:?}"))?;
        let query = super::query::Query {
            state,
            filter: &filter,
            calling_uid: 1000,
        };
        match query
            .package_info(&provider, -1, super::info::flags::GET_META_DATA, 0)
            .map_err(|e| format!("module provider package info: {e:?}"))?
        {
            Err(e) => return Err(format!("module provider package info: {e:?}")),
            Ok(None) => {
                owner.diagnostic = Some("module metadata provider package is unavailable".into());
                return Ok(owner);
            }
            Ok(Some(_)) => {}
        }
        let Some(setting) = state.packages.get(&provider) else {
            owner.diagnostic = Some("module metadata provider package is unavailable".into());
            return Ok(owner);
        };
        let Some(package) = setting.pkg.as_ref() else {
            owner.diagnostic = Some("module metadata provider has no accepted code".into());
            return Ok(owner);
        };
        if package.is(super::pkg::booleans::ISOLATED_SPLIT_LOADING)
            && package
                .split_code_paths
                .as_ref()
                .is_some_and(|p| !p.is_empty())
        {
            return Err("isolated module-provider split resource owner is unavailable".into());
        }
        if super::info::user_state(setting, 0)
            .overlay_paths
            .as_ref()
            .is_some_and(|p| !p.resource_dirs.is_empty() || !p.overlay_paths.is_empty())
        {
            return Err("module-provider overlay resource owner is unavailable".into());
        }
        let metadata = package.meta_data.as_ref().and_then(|m| {
            m.0.iter()
                .find(|(name, _)| name == "android.content.pm.MODULE_METADATA")
        });
        let xml_id = match metadata {
            Some((_, Metadata::Int(id))) => *id as u32,
            _ => return Err("module metadata XML resource reference is unavailable".into()),
        };
        let mut paths = vec![
            package
                .base_apk_path
                .as_ref()
                .ok_or("module provider base APK is unavailable")?
                .clone(),
        ];
        paths.extend(
            package
                .split_code_paths
                .iter()
                .flatten()
                .map(|p| p.clone().ok_or("null module provider split path"))
                .collect::<Result<Vec<_>, _>>()?,
        );
        let apks = paths
            .iter()
            .map(|path| files(path).and_then(|path| Apk::open(&path).map_err(|e| e.to_string())))
            .collect::<Result<Vec<_>, _>>()?;
        let tables = apks
            .iter()
            .map(|apk| {
                apk.file_if_present("resources.arsc")
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| {
                        bytes
                            .map(|b| Table::parse(&b).map_err(|e| e.to_string()))
                            .transpose()
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut resources = vec![&platform.framework];
        resources.extend(tables.iter().filter_map(Option::as_ref));
        // Record exactly which APK each resource table came from.
        let mut table_apks = vec![None];
        table_apks.extend(
            tables
                .iter()
                .enumerate()
                .filter_map(|(i, t)| t.as_ref().map(|_| Some(i))),
        );
        let resources = Resources {
            tables: resources,
            overlays: &platform.framework_overlays,
            config,
        };
        let (table, path) = resources
            .resource_string_source(xml_id)
            .ok_or("module XML resource is unavailable")?;
        let apk = table_apks
            .get(table)
            .and_then(|i| *i)
            .ok_or("module XML resolved outside its accepted APK assets")?;
        let document = res::xml(&apks[apk].file(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let provide_apks = platform
            .flags
            .get("android.content.pm.provide_info_of_apk_in_apex")
            .copied()
            .ok_or("module APK-info aconfig owner is unavailable")?;
        owner.parse(
            &document,
            &|id| {
                let mut selected = Selected {
                    kind: TYPE_REFERENCE,
                    data: id,
                    table: None,
                    resid: 0,
                    flags: 0,
                };
                resources.resolve(&mut selected);
                if selected.kind != TYPE_STRING {
                    return Err("module name resource is not text".into());
                }
                let table = selected
                    .table
                    .ok_or("module name resource table is unavailable")?;
                if resources
                    .has_styled_text(table, selected.data)
                    .ok_or("module text asset is unavailable")?
                {
                    return Err("styled module name requires the resource span owner".into());
                }
                resources
                    .string(table, selected.data)
                    .map(str::to_owned)
                    .ok_or_else(|| "module name text is unavailable".into())
            },
            apex,
            provide_apks,
        )?;
        Ok(owner)
    }

    fn parse(
        &mut self,
        root: &Element,
        text: &dyn Fn(u32) -> Result<String, String>,
        apex: &ApexLinks,
        provide_apks: bool,
    ) -> Result<(), String> {
        self.modules.clear();
        if root.name != "module-metadata" {
            self.loaded = true;
            self.diagnostic = Some("module metadata root differs".into());
            return Ok(());
        }
        let mut children = Vec::new();
        fn visit<'a>(element: &'a Element, out: &mut Vec<&'a Element>) {
            for child in &element.children {
                out.push(child);
                visit(child, out);
            }
        }
        visit(root, &mut children);
        for child in children {
            if child.name != "module" {
                self.modules.clear();
                self.loaded = true;
                self.diagnostic =
                    Some(format!("unexpected module metadata element {}", child.name));
                return Ok(());
            }
            let id = match attribute(child, "name") {
                Some(Value::Ref(id)) => *id,
                value => {
                    let name = coerce_string(value)?.ok_or("module name reference is absent")?;
                    let rest = String::from_utf16(&name.encode_utf16().skip(1).collect::<Vec<_>>())
                        .map_err(|_| "module name reference is malformed")?;
                    super::system_config::decimal_uid(&rest)
                        .map(|id| id as u32)
                        .ok_or("module name reference is malformed")?
                }
            };
            let package = coerce_string(attribute(child, "packageName"))?;
            let module = package
                .as_ref()
                .and_then(|p| apex.modules.get(p))
                .cloned()
                .flatten();
            let info = ModuleInfo {
                name: Some(text(id)?),
                package: package.clone(),
                hidden: match attribute(child, "isHidden") {
                    Some(Value::Bool(value)) => *value,
                    Some(Value::String(value)) => value.eq_ignore_ascii_case("true"),
                    _ => false,
                },
                apex: module.clone(),
                apks: provide_apks.then(|| {
                    apex.apks
                        .get(&module)
                        .filter(|_| {
                            package
                                .as_ref()
                                .is_some_and(|p| apex.modules.contains_key(p))
                        })
                        .into_iter()
                        .flatten()
                        .cloned()
                        .map(Some)
                        .collect()
                }),
            };
            match self.modules.iter().position(|m| m.package == package) {
                Some(i) => self.modules[i] = info,
                None => self.modules.push(info),
            }
        }
        self.modules
            .sort_by_key(|m| m.package.as_deref().map_or(0, super::info::java_hash));
        self.loaded = true;
        Ok(())
    }
}
fn coerce_string(value: Option<&Value>) -> Result<Option<String>, String> {
    Ok(match value {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Ref(id)) => Some(format!("@{}", *id as i32)),
        Some(Value::Int(n)) => Some(n.to_string()),
        Some(Value::Bool(b)) => Some(b.to_string()),
        _ => return Err("module XML attribute string coercion is unavailable".into()),
    })
}

fn attribute<'a>(element: &'a Element, name: &str) -> Option<&'a Value> {
    element
        .attrs
        .iter()
        .find(|a| a.ns.is_empty() && a.name == name)
        .map(|a| &a.value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn module(package: &str, id: u32) -> Element {
        let attr = |name: &str, value: Value| res::Attr {
            ns: String::new(),
            name: name.into(),
            id: 0,
            kind: 0,
            data: 0,
            value,
        };
        Element {
            name: "module".into(),
            attrs: vec![
                attr("name", Value::Ref(id)),
                attr("packageName", Value::String(package.into())),
                attr("isHidden", Value::String("TrUe".into())),
            ],
            children: vec![],
        }
    }
    #[test]
    fn parsed_resource_metadata_preserves_collision_order_last_owner_and_parse_failure() {
        let mut owner = Owner {
            provider: Some("provider".into()),
            loaded: false,
            modules: vec![],
            diagnostic: None,
        };
        let apex = ApexLinks {
            modules: [("BB".into(), Some("module.bb".into()))].into(),
            apks: [(
                Some("module.bb".into()),
                vec!["apk.second".into(), "apk.first".into()],
            )]
            .into(),
        };
        let root = Element {
            name: "module-metadata".into(),
            attrs: vec![],
            children: vec![module("BB", 1), module("Aa", 2), module("BB", 3)],
        };
        owner
            .parse(&root, &|id| Ok(format!("localized-{id}")), &apex, true)
            .unwrap();
        assert!(owner.loaded());
        assert_eq!(owner.modules()[0].package.as_deref(), Some("BB"));
        assert_eq!(owner.modules()[0].name.as_deref(), Some("localized-3"));
        assert!(owner.modules()[0].hidden);
        assert_eq!(
            owner.modules()[0].apks,
            Some(vec![Some("apk.second".into()), Some("apk.first".into())])
        );
        assert_eq!(owner.modules()[1].apks, Some(vec![]));
        let mut invalid = root.clone();
        invalid.children[0].children.push(Element {
            name: "unexpected".into(),
            ..Default::default()
        });
        owner
            .parse(&invalid, &|_| Ok("name".into()), &apex, false)
            .unwrap();
        assert!(owner.loaded());
        assert!(owner.modules().is_empty());
        assert!(owner.diagnostic().unwrap().contains("unexpected"));
    }
    #[test]
    fn plain_module_info_wire_preserves_nullable_and_empty_lists() {
        for apks in [None, Some(vec![]), Some(vec![Some("apk".into()), None])] {
            let info = ModuleInfo {
                name: Some("모듈".into()),
                package: Some("package".into()),
                hidden: true,
                apex: None,
                apks,
            };
            let mut parcel = Parcel::new();
            info.write_to(&mut parcel);
            let mut reader = Reader::new(parcel.data(), parcel.objects());
            assert_eq!(ModuleInfo::read_from(&mut reader).unwrap(), info);
            assert_eq!(reader.remaining(), 0);
        }
    }
    #[test]
    fn native_apex_links_preserve_container_map_and_apk_registration_boundary_order() {
        let result = super::super::scan::ApexScanResult {
            info: super::super::bootstrap::ApexPackage {
                module_name: Some("module".into()),
                module_path: "/system/apex/module.apex".into(),
                preinstalled_path: "/system/apex/module.apex".into(),
                version_code: 1,
                factory: true,
                active: true,
                active_changed: false,
            },
            package: AndroidPackage {
                package_name: "apex.package".into(),
                ..Default::default()
            },
            signing: super::super::sign::SigningDetails::unknown(),
        };
        let active = super::super::bootstrap::ApexInventory {
            packages: Some(vec![]),
            active: vec![super::super::bootstrap::ActiveApex {
                module_name: Some("module".into()),
                mount_path: "/apex/module".into(),
                preinstalled_path: String::new(),
                factory: true,
                module_path: "/system/apex/module.apex".into(),
                active_changed: false,
            }],
        };
        let registrations = [
            ("second", "/apex/module/app/second.apk"),
            ("not-child", "/apex/module-neighbor/app.apk"),
            ("first", "/apex/module/app/first.apk"),
        ]
        .map(|(package, path)| AndroidPackage {
            package_name: package.into(),
            base_apk_path: Some(path.into()),
            ..Default::default()
        });
        let links = ApexLinks::from_scan(&[result], &active, &registrations).unwrap();
        assert_eq!(links.modules["apex.package"].as_deref(), Some("module"));
        assert_eq!(links.apks[&Some("module".into())], vec!["second", "first"]);
    }
}
