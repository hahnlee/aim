//! Settings.writePackageLPr's updateOwner attribute, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use crate::package::{
    owner::{attribute, signing::persisted},
    settings::Settings,
};
use aim_android_xml::{Element, Node};
use std::collections::BTreeSet;

pub(in crate::package::owner) fn replace_clearings(
    original: &Element,
    desired: &Settings,
) -> Result<Element, String> {
    let mut allowed = Settings::parse(original)?;
    for package in &mut allowed.packages {
        let next = desired
            .packages
            .iter()
            .find(|p| p.name == package.name)
            .ok_or("update ownership commit removed a retained package (#798)")?;
        if next.install_source.update_owner != package.install_source.update_owner {
            if next.install_source.update_owner.is_some() {
                return Err("denylist commit assigned an update owner".into());
            }
            package.install_source.update_owner = None;
        }
    }
    if persisted(allowed) != persisted(desired.clone()) {
        return Err("update ownership commit changed unrelated settings (#798)".into());
    }
    let mut root = original.clone();
    let mut seen = BTreeSet::new();
    for node in &mut root.content {
        let Node::Element(owner) = node else { continue };
        if owner.name != "package" {
            continue;
        }
        let name = owner
            .string("name")
            .ok_or("update ownership package has no name")?
            .into_owned();
        if !seen.insert(name.clone()) {
            return Err("duplicate update ownership package".into());
        }
        let package = desired
            .packages
            .iter()
            .find(|p| p.name == name)
            .ok_or("unmodelled update ownership package")?;
        if owner
            .attrs
            .iter()
            .filter(|(name, _)| name == "updateOwner")
            .count()
            > 1
        {
            return Err("duplicate updateOwner attribute".into());
        }
        if package.install_source.update_owner.is_none() {
            attribute(owner, "updateOwner", None);
        }
    }
    if Settings::parse(&root)? != persisted(desired.clone()) {
        return Err("update ownership document did not preserve desired settings".into());
    }
    Ok(root)
}
