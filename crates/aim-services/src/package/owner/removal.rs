//! Persist Settings.removePackageAndAppIdLPw, android-16.0.0_r1 (#798/#822).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{attribute, install_sources::Installers, canonical_persistent_settings as persisted};
use crate::package::settings::Settings;
use aim_android_xml::{Element, Node, Value};
use std::collections::BTreeMap;

fn settings_difference(before: &Settings, after: &Settings) -> String {
    let mut fields = Vec::new();
    for (kind, old, new) in [("active", &before.packages, &after.packages),
        ("factory", &before.disabled_system_packages, &after.disabled_system_packages)] {
        let old_order = old.iter().map(|package| package.name.as_str()).collect::<Vec<_>>();
        let new_order = new.iter().map(|package| package.name.as_str()).collect::<Vec<_>>();
        if old_order != new_order {
            fields.push(format!("{kind} inventory/order {} -> {}; first differing position {:?}",old.len(),new.len(),old_order.iter().zip(&new_order).position(|(a,b)|a!=b)));
        }
        for package in old {
            let Some(candidate) = new.iter().find(|value| value.name == package.name) else {
                fields.push(format!("{kind} missing {}", package.name)); continue;
            };
            if package == candidate { continue; }
            let mut changed = Vec::new();
            macro_rules! compare {($($field:ident),*) => {$(if package.$field != candidate.$field {changed.push(stringify!($field));})*};}
            compare!(install_permissions_fixed,transient,real_name,code_path,legacy_native_library_path,
                primary_cpu_abi,secondary_cpu_abi,cpu_abi_override,flags,private_flags,last_modified_time,
                last_update_time,legacy_first_install_time,version_code,target_sdk_version,restrict_update_hash,
                scanned_as_stopped_system_app,app_id,shared_user,shared_user_app_id,is_sdk_library,
                install_source,volume_uuid,category_hint,update_available,force_queryable,pending_restore,
                debuggable,leaving_shared_user,old_paths,base_revision_code,page_size_compat,loading_progress,
                loading_completed_time,domain_set_id,app_metadata_file_path,app_metadata_source,
                uses_sdk_libraries,uses_static_libraries,signatures,key_set_data,mime_groups,split_versions);
            fields.push(format!("{kind} {} fields={changed:?}; identity {:?} -> {:?}; timestamps {:?} -> {:?}; transient {:?} -> {:?}",package.name,
                (package.app_id,&package.code_path,package.version_code),(candidate.app_id,&candidate.code_path,candidate.version_code),
                (package.last_modified_time,package.last_update_time,package.legacy_first_install_time),
                (candidate.last_modified_time,candidate.last_update_time,candidate.legacy_first_install_time),package.transient,candidate.transient));
        }
    }
    if before.shared_users != after.shared_users {
        let summarize = |values: &[crate::package::settings::SharedUser]| values.iter()
            .map(|group| (group.name.clone(),group.app_id,group.flags,group.signatures.as_ref().map(|s|s.signatures.len())))
            .collect::<Vec<_>>();
        fields.push(format!("shared UID records {:?} -> {:?}",summarize(&before.shared_users),summarize(&after.shared_users)));
    }
    macro_rules! globals {($($field:ident),*) => {$(if before.$field != after.$field {fields.push(format!("{} differs",stringify!($field)));})*};}
    globals!(versions,verifier,permission_trees,permissions,renamed_packages,key_sets,domain_verification,legacy_domain_info);
    fields.join("; ")
}

pub(super) fn replace(
    original: &Element,
    desired: &Settings,
    name: &str,
) -> Result<Element, String> {
    let mut allowed = Settings::parse(original)?;
    let at = allowed
        .packages
        .iter()
        .position(|p| p.name == name)
        .ok_or("removed setting is not present")?;
    let package = allowed.packages[at].clone();
    let mut installers = Installers::restore(&allowed);
    allowed.packages.remove(at);
    installers.remove(name, &mut allowed);
    let mut removed_group = None;
    if package.shared_user
        && !allowed
            .packages
            .iter()
            .chain(&allowed.disabled_system_packages)
            .any(|p| p.shared_app_id() == package.shared_app_id() && p.shared_user)
    {
        let groups: Vec<_> = allowed
            .shared_users
            .iter()
            .filter(|g| Some(g.app_id) == package.shared_app_id())
            .collect();
        if groups.len() != 1 {
            return Err("removed package has ambiguous shared UID ownership".into());
        }
        removed_group = Some(groups[0].name.clone());
        allowed
            .shared_users
            .retain(|g| Some(g.app_id) != package.shared_app_id());
    }
    if persisted(allowed.clone()) != persisted(desired.clone()) {
        // The live scan registry can retain an installer name after its last
        // installer/initiator/originator role disappears. That registry is not
        // persisted, so also accept the registered-name cleanup result.
        let mut registered = Installers::restore(&allowed);
        registered.add(&crate::package::settings::InstallSource {
            installer: Some(name.into()),
            ..Default::default()
        });
        registered.remove(name, &mut allowed);
        let allowed = persisted(allowed);
        let desired = persisted(desired.clone());
        if allowed != desired {
            return Err(format!("setting removal changed unrelated settings: {}", settings_difference(&allowed, &desired)));
        }
    }
    let mut root = original.clone();
    // A deleted package can define the certificate index used by a later
    // package, shared user, past signer or install initiator. Materialize each
    // reference before dropping owners, retaining its original index and XML.
    materialize_certificates(&mut root)?;
    let mut removed = 0;
    root.content.retain(|node| {
        let Node::Element(e) = node else { return true };
        if e.name == "package" && e.string("name").as_deref() == Some(name) {
            removed += 1;
            return false;
        }
        e.name != "shared-user"
            || removed_group.is_none()
            || removed_group.as_deref() != e.string("name").as_deref()
    });
    if removed != 1 {
        return Err("duplicate removed package record".into());
    }
    for node in &mut root.content {
        let Node::Element(e) = node else { continue };
        if e.name != "package" {
            continue;
        }
        let package = desired
            .packages
            .iter()
            .find(|p| e.string("name").as_deref() == Some(p.name.as_str()))
            .ok_or("unmodelled retained package")?;
        let source = &package.install_source;
        for (key, value) in [
            ("installer", source.installer.clone()),
            ("installOriginator", source.originating_package.clone()),
            ("updateOwner", source.update_owner.clone()),
            (
                "installerAttributionTag",
                source.installer_attribution_tag.clone(),
            ),
        ] {
            attribute(e, key, value.map(Value::String));
        }
        attribute(
            e,
            "installerUid",
            (source.installer_uid != -1).then_some(Value::Int(source.installer_uid)),
        );
        attribute(
            e,
            "isOrphaned",
            source.is_orphaned.then_some(Value::Bool(true)),
        );
        attribute(
            e,
            "installInitiatorUninstalled",
            source
                .initiating_package_uninstalled
                .then_some(Value::Bool(true)),
        );
    }
    if persisted(Settings::parse(&root)?) != persisted(desired.clone()) {
        return Err("removed setting document did not preserve desired settings".into());
    }
    Ok(root)
}

fn materialize_certificates(root: &mut Element) -> Result<(), String> {
    let mut table = BTreeMap::new();
    fn certificate(e: &mut Element, table: &mut BTreeMap<i32, Vec<u8>>) -> Result<(), String> {
        let index = e
            .int("index")?
            .filter(|i| *i >= 0)
            .ok_or("invalid signer index")?;
        if let Some(key) = e.bytes_hex("key")? {
            table.insert(index, key);
        } else {
            let key = table
                .get(&index)
                .ok_or("unresolved signer index before removal")?;
            attribute(e, "key", Some(Value::BytesHex(key.clone())));
        }
        Ok(())
    }
    fn signatures(e: &mut Element, table: &mut BTreeMap<i32, Vec<u8>>) -> Result<(), String> {
        let count = e.int("count")?.ok_or("missing signer count")?;
        if count < 0 || e.children().filter(|c| c.name == "cert").count() != count as usize {
            return Err("invalid signer count before removal".into());
        }
        for node in &mut e.content {
            let Node::Element(child) = node else { continue };
            match child.name.as_str() {
                "cert" => certificate(child, table)?,
                "pastSigs" => signatures(child, table)?,
                _ => {}
            }
        }
        Ok(())
    }
    for node in &mut root.content {
        let Node::Element(owner) = node else { continue };
        if !matches!(
            owner.name.as_str(),
            "package" | "updated-package" | "shared-user"
        ) {
            continue;
        }
        for node in &mut owner.content {
            let Node::Element(child) = node else { continue };
            if matches!(child.name.as_str(), "sigs" | "install-initiator-sigs") {
                signatures(child, &mut table)?;
            }
        }
    }
    Ok(())
}
