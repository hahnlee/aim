//! Settings.writePackageLPr/writeDisabledSysPackageLPr at android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{attribute, signing::persisted};
use crate::package::settings::Settings;
use aim_android_xml::{Element, Node, Value};
use std::collections::BTreeSet;

pub(super) fn replace(original: &Element, desired: &Settings) -> Result<Element, String> {
    let mut allowed = Settings::parse(original)?;
    for (packages, next, active) in [
        (&mut allowed.packages, &desired.packages, true),
        (
            &mut allowed.disabled_system_packages,
            &desired.disabled_system_packages,
            false,
        ),
    ] {
        for package in packages {
            let new = next
                .iter()
                .find(|p| p.name == package.name)
                .ok_or("native-library commit removed a package (#798)")?;
            package.legacy_native_library_path = new.legacy_native_library_path.clone();
            package.primary_cpu_abi = new.primary_cpu_abi.clone();
            package.secondary_cpu_abi = new.secondary_cpu_abi.clone();
            package.cpu_abi_override = new.cpu_abi_override.clone();
            if active {
                if !(0..128).contains(&new.page_size_compat) {
                    return Err("invalid persisted page-size compatibility flags".into());
                }
                package.page_size_compat = new.page_size_compat;
            }
        }
    }
    if persisted(allowed) != persisted(desired.clone()) {
        return Err("native-library commit changed unrelated settings (#798)".into());
    }
    let mut root = original.clone();
    let mut seen = BTreeSet::new();
    for node in &mut root.content {
        let Node::Element(owner) = node else { continue };
        let packages = match owner.name.as_str() {
            "package" => &desired.packages,
            "updated-package" => &desired.disabled_system_packages,
            _ => continue,
        };
        let name = owner
            .string("name")
            .ok_or("package record has no name (#798)")?
            .into_owned();
        if !seen.insert((owner.name.clone(), name.clone())) {
            return Err("duplicate native-library setting owner (#798)".into());
        }
        let package = packages
            .iter()
            .find(|p| p.name == name)
            .ok_or("unmodelled native-library setting owner (#798)")?;
        for (name, value) in [
            ("nativeLibraryPath", &package.legacy_native_library_path),
            ("primaryCpuAbi", &package.primary_cpu_abi),
            ("secondaryCpuAbi", &package.secondary_cpu_abi),
            ("cpuAbiOverride", &package.cpu_abi_override),
        ] {
            attribute(owner, name, value.clone().map(Value::String));
        }
        // The old requiredCpuAbi reader fallback must not resurrect a cleared ABI.
        attribute(owner, "requiredCpuAbi", None);
        if owner.name == "package" {
            attribute(
                owner,
                "pageSizeCompat",
                (package.page_size_compat != 0).then_some(Value::Int(package.page_size_compat)),
            );
        }
    }
    if persisted(Settings::parse(&root)?) != persisted(desired.clone()) {
        return Err("native-library settings did not round trip (#798)".into());
    }
    Ok(root)
}
