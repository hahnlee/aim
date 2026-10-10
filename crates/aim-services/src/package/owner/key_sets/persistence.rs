//! Settings.writeSigningKeySetLPr/writeKeySetAliasesLPr and
//! KeySetManagerService.writeKeySetManagerServiceLPr, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{Settings, validated_sets};
use crate::package::owner::{attribute, element, canonical_persistent_settings as persisted};
use aim_android_xml::{Element, Node, Value};
use std::collections::BTreeSet;

fn unrelated_fields(left: &Settings, right: &Settings) -> String {
    let mut fields = Vec::new();
    macro_rules! global {
        ($($field:ident),* $(,)?) => { $(if left.$field != right.$field { fields.push(stringify!($field).to_owned()); })* };
    }
    global!(versions, verifier, permission_trees, permissions, packages, disabled_system_packages,
        shared_users, renamed_packages, key_sets, domain_verification, legacy_domain_info);
    let before = &left.domain_verification;
    let after = &right.domain_verification;
    if before.legacy != after.legacy { fields.push("domain.legacy".into()); }
    for (section, before, after) in [("active", &before.active, &after.active), ("restored", &before.restored, &after.restored)] {
        if before.len() != after.len() { fields.push(format!("domain.{section}.count={}/{}", before.len(), after.len())); }
        for package in before {
            if let Some(next) = after.iter().find(|candidate| candidate.name == package.name) {
                macro_rules! domain_fields {
                    ($($field:ident),*) => { $(if package.$field != next.$field { fields.push(format!("domain.{section}.{}:{}", package.name, stringify!($field))); })* };
                }
                domain_fields!(id, has_auto_verify_domains, signature, domains, users, uri_relative_filter_groups);
                if package.users != next.users { fields.push(format!("domain.{section}.{}.users={}/{}", package.name, package.users.len(), next.users.len())); }
            } else { fields.push(format!("domain.{section}.{}:missing", package.name)); }
        }
    }
    for (kind, before, after) in [("packages", &left.packages, &right.packages),
            ("disabled", &left.disabled_system_packages, &right.disabled_system_packages)] {
        for package in before {
            let Some(next) = after.iter().find(|candidate| candidate.name == package.name) else {
                fields.push(format!("{kind}:{}:missing", package.name));
                continue;
            };
            let mut changed = Vec::new();
            macro_rules! package_fields {
                ($($field:ident),* $(,)?) => { $(if package.$field != next.$field { changed.push(stringify!($field)); })* };
            }
            package_fields!(install_permissions_fixed, transient, name, real_name, code_path, legacy_native_library_path, primary_cpu_abi, secondary_cpu_abi, cpu_abi_override, flags, private_flags, last_modified_time, last_update_time, legacy_first_install_time, version_code, target_sdk_version, restrict_update_hash, scanned_as_stopped_system_app, app_id, shared_user, shared_user_app_id, is_sdk_library, install_source, volume_uuid, category_hint, update_available, force_queryable, pending_restore, debuggable, leaving_shared_user, old_paths, base_revision_code, page_size_compat, loading_progress, loading_completed_time, domain_set_id, app_metadata_file_path, app_metadata_source, uses_sdk_libraries, uses_static_libraries, signatures, key_set_data, mime_groups, split_versions);
            if !changed.is_empty() { fields.push(format!("{kind}:{}:{}", package.name, changed.join(","))); }
        }
    }
    format!("{}; counts packages={}/{}, disabled={}/{}, shared={}/{}", fields.join(";"),
        left.packages.len(), right.packages.len(), left.disabled_system_packages.len(), right.disabled_system_packages.len(),
        left.shared_users.len(), right.shared_users.len())
}

pub(in crate::package::owner) fn replace_registered(
    original: &Element,
    desired: &Settings,
) -> Result<Element, String> {
    replace_for_scan(original, desired, desired)
}

/// Scan writes can retain pending shared entries that readLPw later drops.
/// The scan owner supplies and validates that separate read result.
pub(in crate::package::owner) fn replace_for_scan(
    original: &Element,
    desired: &Settings,
    restored: &Settings,
) -> Result<Element, String> {
    validated_sets(desired)?;
    if !desired.key_sets.versioned
        || desired.key_sets.last_issued_key_id < 0
        || desired.key_sets.last_issued_key_set_id < 0
        || desired
            .key_sets
            .public_keys
            .iter()
            .any(|(id, _)| *id > desired.key_sets.last_issued_key_id)
        || desired
            .key_sets
            .key_sets
            .iter()
            .any(|(id, _)| *id > desired.key_sets.last_issued_key_set_id)
    {
        return Err("invalid persisted keyset version/counters".into());
    }
    let mut allowed = Settings::parse(original)?;
    if desired.key_sets.last_issued_key_id < allowed.key_sets.last_issued_key_id
        || desired.key_sets.last_issued_key_set_id < allowed.key_sets.last_issued_key_set_id
    {
        return Err("keyset commit rewound allocation counters".into());
    }
    for package in &mut allowed.packages {
        let next = desired
            .packages
            .iter()
            .find(|p| p.name == package.name)
            .ok_or("keyset commit removed a retained package (#798)")?;
        if next.key_set_data.upgrade_key_sets.iter().any(|id| {
            !next
                .key_set_data
                .defined_key_sets
                .iter()
                .any(|(_, defined)| defined == id)
        }) {
            return Err("persisted upgrade keyset has no definition".into());
        }
        package.key_set_data = next.key_set_data.clone();
    }
    allowed.key_sets = desired.key_sets.clone();
    let allowed = persisted(allowed);
    let restored_projection = persisted(restored.clone());
    if allowed != restored_projection {
        return Err(format!("keyset commit changed unrelated settings (#798): {}",
            unrelated_fields(&allowed, &restored_projection)));
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
            .ok_or("keyset package has no name")?
            .into_owned();
        if !seen.insert(name.clone()) {
            return Err("duplicate keyset package owner".into());
        }
        let package = desired
            .packages
            .iter()
            .find(|p| p.name == name)
            .ok_or("unmodelled keyset package owner")?;
        owner.content.retain(|n| !matches!(n, Node::Element(e) if matches!(e.name.as_str(), "proper-signing-keyset" | "upgrade-keyset" | "defined-keyset")));
        owner.content.push(Node::Element(identifier(
            "proper-signing-keyset",
            package.key_set_data.proper_signing_key_set,
        )));
        for id in &package.key_set_data.upgrade_key_sets {
            owner
                .content
                .push(Node::Element(identifier("upgrade-keyset", *id)));
        }
        for (alias, id) in &package.key_set_data.defined_key_sets {
            let mut keyset = identifier("defined-keyset", *id);
            let alias = alias
                .as_ref()
                .ok_or("original keyset writer cannot serialize a null alias")?;
            attribute(&mut keyset, "alias", Some(Value::String(alias.clone())));
            owner.content.push(Node::Element(keyset));
        }
    }
    let owners: Vec<_> = original
        .children()
        .filter(|e| e.name == "keyset-settings")
        .collect();
    if owners.len() > 1 {
        return Err("duplicate global keyset owner".into());
    }
    let mut global = owners
        .first()
        .map(|e| (*e).clone())
        .unwrap_or_else(|| element("keyset-settings"));
    attribute(&mut global, "version", Some(Value::Int(1)));
    let mut keys = section(&global, "keys")?;
    keys.content
        .retain(|n| !matches!(n, Node::Element(e) if e.name == "public-key"));
    for (id, encoded) in &desired.key_sets.public_keys {
        let mut key = identifier("public-key", *id);
        attribute(&mut key, "value", Some(Value::BytesBase64(encoded.clone())));
        keys.content.push(Node::Element(key));
    }
    let mut sets = section(&global, "keysets")?;
    sets.content
        .retain(|n| !matches!(n, Node::Element(e) if e.name == "keyset"));
    for (id, members) in &desired.key_sets.key_sets {
        let mut set = identifier("keyset", *id);
        for key in members {
            set.content.push(Node::Element(identifier("key-id", *key)));
        }
        sets.content.push(Node::Element(set));
    }
    global.content.retain(|n| !matches!(n, Node::Element(e) if matches!(e.name.as_str(), "keys" | "keysets" | "lastIssuedKeyId" | "lastIssuedKeySetId")));
    global
        .content
        .extend([Node::Element(keys), Node::Element(sets)]);
    for (name, value) in [
        ("lastIssuedKeyId", desired.key_sets.last_issued_key_id),
        (
            "lastIssuedKeySetId",
            desired.key_sets.last_issued_key_set_id,
        ),
    ] {
        let mut counter = element(name);
        attribute(&mut counter, "value", Some(Value::Long(value)));
        global.content.push(Node::Element(counter));
    }
    root.content
        .retain(|n| !matches!(n, Node::Element(e) if e.name == "keyset-settings"));
    root.content.push(Node::Element(global));
    if persisted(Settings::parse(&root)?) != persisted(restored.clone()) {
        return Err("keyset document did not preserve desired settings".into());
    }
    Ok(root)
}

fn identifier(name: &str, id: i64) -> Element {
    let mut node = element(name);
    attribute(&mut node, "identifier", Some(Value::Long(id)));
    node
}

fn section(global: &Element, name: &str) -> Result<Element, String> {
    let sections: Vec<_> = global.children().filter(|e| e.name == name).collect();
    if sections.len() > 1 {
        return Err("duplicate global keyset section".into());
    }
    Ok(sections
        .first()
        .map(|e| (*e).clone())
        .unwrap_or_else(|| element(name)))
}
