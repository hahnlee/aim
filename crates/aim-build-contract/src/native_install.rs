//! Native installation graph input for Android linker configuration generation.
//! Not a runtime SONAME resolver: API declarations come from module metadata,
//! never from scanning exports or from a successful host dlopen.
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum Placement {
    System,
    SystemExt,
    Product,
    Vendor,
    Odm,
    Apex(String),
    Host,
}

#[derive(Clone, Debug)]
pub struct NativeInstall {
    /// Unique build module identity, including selected implementation variant.
    pub module: String,
    pub placement: Placement,
    /// Installed filenames exported through declared stub/LLNDK interfaces.
    /// Empty for private modules, even when they export ELF symbols.
    pub provided_interfaces: Vec<String>,
    /// Declared stable interface used when another partition imports this module.
    pub required_interface: Option<String>,
    /// Actual build dependency edges, not every APEX visible in the source image.
    pub dependencies: Vec<String>,
}

#[derive(Debug, Eq, PartialEq)]
pub struct PartitionInterfaces {
    pub provides: BTreeSet<String>,
    pub requires: BTreeSet<String>,
}

pub fn partition_interfaces(
    installed: &[NativeInstall],
    placement: &Placement,
) -> Result<PartitionInterfaces, String> {
    if matches!(placement, Placement::Host | Placement::Apex(_)) {
        return Err("system linker config requires an Android filesystem partition".into());
    }
    let mut modules = BTreeMap::new();
    for item in installed {
        if item.module.is_empty() || modules.insert(&item.module, item).is_some() {
            return Err("empty or duplicate native installation identity".into());
        }
        for name in item
            .provided_interfaces
            .iter()
            .chain(item.required_interface.iter())
        {
            if name.is_empty()
                || name == "."
                || name == ".."
                || name
                    .bytes()
                    .any(|b| b == b'/' || b == b':' || b.is_ascii_whitespace() || b == 0)
            {
                return Err(format!("invalid native interface filename: {name:?}"));
            }
        }
    }
    let mut result = PartitionInterfaces {
        provides: BTreeSet::new(),
        requires: BTreeSet::new(),
    };
    for item in installed {
        // Reject incomplete graphs even if the missing edge is outside this
        // partition; configuration must not silently manufacture completeness.
        for dependency in &item.dependencies {
            let target = modules.get(dependency).ok_or_else(|| {
                format!("{} depends on unrecorded module {dependency}", item.module)
            })?;
            if &item.placement == placement
                && &target.placement != placement
                && target.placement != Placement::Host
            {
                if let Some(interface) = &target.required_interface {
                    result.requires.insert(interface.clone());
                }
            }
        }
        if &item.placement == placement {
            result
                .provides
                .extend(item.provided_interfaces.iter().cloned());
        }
    }
    if let Some(name) = result.provides.intersection(&result.requires).next() {
        return Err(format!("interface both provided and required: {name}"));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn module(name: &str, placement: Placement, api: Option<&str>) -> NativeInstall {
        NativeInstall {
            module: name.into(),
            placement,
            provided_interfaces: api.into_iter().map(str::to_owned).collect(),
            required_interface: api.map(str::to_owned),
            dependencies: vec![],
        }
    }
    #[test]
    fn uses_partition_edges_and_declared_interfaces() {
        let mut app = module("consumer", Placement::System, None);
        app.dependencies = vec![
            "icu".into(),
            "private".into(),
            "tool".into(),
            "public".into(),
        ];
        let inputs = vec![
            app,
            module(
                "icu",
                Placement::Apex("com.android.i18n".into()),
                Some("libicu.so"),
            ),
            module("private", Placement::Vendor, None),
            module("tool", Placement::Host, Some("host-only.dylib")),
            module(
                "unrelated",
                Placement::Apex("other".into()),
                Some("libother.so"),
            ),
            module("public", Placement::System, Some("libpublic.so")),
        ];
        let result = partition_interfaces(&inputs, &Placement::System).unwrap();
        assert_eq!(result.provides, BTreeSet::from(["libpublic.so".into()]));
        assert_eq!(result.requires, BTreeSet::from(["libicu.so".into()]));
    }
    #[test]
    fn rejects_missing_and_ambiguous_metadata() {
        let mut item = module("one", Placement::System, Some("libone.so"));
        item.dependencies.push("absent".into());
        assert!(partition_interfaces(&[item.clone()], &Placement::System).is_err());
        item.dependencies.clear();
        assert!(partition_interfaces(&[item.clone(), item.clone()], &Placement::System).is_err());
        item.required_interface = Some("../libone.so".into());
        assert!(partition_interfaces(&[item], &Placement::System).is_err());
    }

    #[test]
    fn rejects_conflicting_interfaces_and_non_partition_targets() {
        let mut item = module("one", Placement::System, Some("libone.so"));
        item.dependencies.push("two".into());
        let target = module("two", Placement::Apex("other".into()), Some("libone.so"));
        assert!(partition_interfaces(&[item, target], &Placement::System).is_err());
        assert!(partition_interfaces(&[], &Placement::Host).is_err());
        assert!(partition_interfaces(&[], &Placement::Apex("a".into())).is_err());
    }
}
