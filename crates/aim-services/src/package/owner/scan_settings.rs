//! Settings.writePackageLPr/writeDisabledSysPackageLPr, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{attribute, element, key_sets, signing};
use crate::package::{
    pkg::booleans2,
    scan::SigningScan,
    settings::{Package, Settings},
};
use aim_android_xml::{Element, Node, Value};
use std::collections::BTreeSet;

/// Write package/shared setting owners together. Other global owners must have
/// committed their changes already; the round trip rejects unresolved changes.
pub(super) fn replace(original: &Element, scan: &SigningScan) -> Result<Element, String> {
    if !scan.capture_ready() {
        return Err("cannot persist unfinished scan metadata".into());
    }
    let mut expected = scan.settings.clone();
    expected.shared_users = scan
        .identities
        .ordered_shared_users()?
        .into_iter()
        .map(|(name, group)| crate::package::settings::SharedUser {
            name: name.to_owned(),
            app_id: group.app_id,
            flags: 0,
            signatures: group.signatures.clone(),
        })
        .collect();
    let mut root = original.clone();
    let mut seen = BTreeSet::new();
    for child in original.children().filter(|e| {
        matches!(
            e.name.as_str(),
            "package" | "updated-package" | "shared-user"
        )
    }) {
        let name = child.string("name").ok_or("setting record has no name")?;
        if !seen.insert((child.name.clone(), name.into_owned())) {
            return Err("duplicate persisted setting owner".into());
        }
    }
    root.content.retain(|n| !matches!(n, Node::Element(e) if matches!(e.name.as_str(), "package" | "updated-package" | "shared-user")));
    let mut certificates = Vec::new();
    for (packages, loaded, active) in [
        (&mut expected.packages, scan.loaded_packages(), true),
        (
            &mut expected.disabled_system_packages,
            scan.disabled_loaded_packages(),
            false,
        ),
    ] {
        // The setting vectors retain insertion slots: replacement updates in
        // place, removal erases the slot and new settings append. ArrayMap
        // iterates signed Java hashes, retaining that order for collisions.
        packages.sort_by_key(|package| crate::package::info::java_hash(&package.name));
        let mut names = BTreeSet::new();
        let mut kept = Vec::new();
        for package in packages.iter() {
            if !names.insert(package.name.clone()) {
                return Err("duplicate scan setting owner".into());
            }
            let code = loaded.get(&package.name);
            if let Some(code) = code {
                scan.validate_collected_uid(package, &code.package, active)?;
                if code.package.is2(booleans2::APEX) {
                    // Settings.writeLPr omits loaded APEX records before writing.
                    continue;
                }
            }
            if package.name.is_empty() || package.code_path.is_empty() || package.app_id <= 0 {
                return Err("invalid persisted package identity".into());
            }
            let previous = original.children().find(|e| {
                e.name == (if active { "package" } else { "updated-package" })
                    && e.string("name").as_deref() == Some(package.name.as_str())
            });
            let mut node = package_node(package, active, previous)?;
            let mut saved = package.clone();
            saved.transient = Default::default();
            saved.leaving_shared_user = Some(false);
            saved.old_paths = None;
            saved.shared_user_app_id = None;
            if active {
                saved.legacy_first_install_time = 0;
                saved.is_sdk_library = !package.shared_user
                    && code
                        .is_some_and(|c| c.package.is(crate::package::pkg::booleans::SDK_LIBRARY));
                attribute(
                    &mut node,
                    "isSdkLibrary",
                    (!package.shared_user).then_some(Value::Bool(saved.is_sdk_library)),
                );
                if code.is_some() {
                    saved.split_versions.clear();
                }
                for (name, version) in &saved.split_versions {
                    let mut split = element("split-version");
                    attribute(&mut split, "name", Some(Value::String(name.clone())));
                    attribute(&mut split, "version", Some(Value::Int(*version)));
                    node.content.push(Node::Element(split));
                }
                signing::write_signatures(
                    &mut node,
                    "sigs",
                    package.signatures.as_ref(),
                    &mut certificates,
                )?;
                signing::write_signatures(
                    &mut node,
                    "install-initiator-sigs",
                    package
                        .install_source
                        .initiating_package_signatures
                        .as_ref(),
                    &mut certificates,
                )?;
                if let Some(sigs) = &mut saved.signatures {
                    sigs.public_keys = None;
                }
                if let Some(sigs) = &mut saved.install_source.initiating_package_signatures {
                    sigs.public_keys = None;
                }
            } else {
                // readDisabledSysPackageLPw restores only its original subset.
                saved = Package {
                    name: saved.name,
                    real_name: saved.real_name,
                    code_path: saved.code_path.clone(),
                    legacy_native_library_path: saved.legacy_native_library_path,
                    primary_cpu_abi: saved.primary_cpu_abi,
                    secondary_cpu_abi: saved.secondary_cpu_abi,
                    cpu_abi_override: saved.cpu_abi_override,
                    last_modified_time: saved.last_modified_time,
                    last_update_time: saved.last_update_time,
                    version_code: saved.version_code,
                    target_sdk_version: saved.target_sdk_version,
                    restrict_update_hash: saved.restrict_update_hash,
                    scanned_as_stopped_system_app: saved.scanned_as_stopped_system_app,
                    app_id: saved.app_id,
                    shared_user: saved.shared_user,
                    flags: crate::package::settings::FLAG_SYSTEM,
                    private_flags: if saved.code_path.contains("/priv-app/") {
                        crate::package::settings::PRIVATE_FLAG_PRIVILEGED
                    } else {
                        0
                    },
                    category_hint: -1,
                    leaving_shared_user: Some(false),
                    app_metadata_file_path: saved.app_metadata_file_path,
                    app_metadata_source: saved.app_metadata_source,
                    uses_sdk_libraries: saved.uses_sdk_libraries,
                    uses_static_libraries: saved.uses_static_libraries,
                    ..Default::default()
                };
            }
            root.content.push(Node::Element(node));
            kept.push(saved);
        }
        *packages = kept;
    }
    let mut groups = BTreeSet::new();
    for group in &mut expected.shared_users {
        if group.name.is_empty() || group.app_id <= 0 || !groups.insert(group.name.clone()) {
            return Err("invalid shared setting identity".into());
        }
        let mut node = original
            .children()
            .find(|e| {
                e.name == "shared-user" && e.string("name").as_deref() == Some(group.name.as_str())
            })
            .cloned()
            .unwrap_or_else(|| element("shared-user"));
        attribute(&mut node, "name", Some(Value::String(group.name.clone())));
        attribute(&mut node, "userId", Some(Value::Int(group.app_id)));
        attribute(&mut node, "system", None);
        group.flags = 0; // The pinned writer does not persist aggregate flags.
        signing::write_signatures(
            &mut node,
            "sigs",
            group.signatures.as_ref(),
            &mut certificates,
        )?;
        if let Some(sigs) = &mut group.signatures {
            sigs.public_keys = None;
        }
        root.content.push(Node::Element(node));
    }
    if expected.key_sets.version.is_some() {
        root = key_sets::replace_registered(&root, &expected)?;
    }
    if signing::persisted(Settings::parse(&root)?) != signing::persisted(expected) {
        return Err("scan settings require an unresolved global owner or do not round trip".into());
    }
    Ok(root)
}

fn package_node(
    package: &Package,
    active: bool,
    previous: Option<&Element>,
) -> Result<Element, String> {
    let mut node = previous
        .cloned()
        .unwrap_or_else(|| element(if active { "package" } else { "updated-package" }));
    // Preserve unknown extensions; replace every field the pinned writer owns.
    node.attrs.retain(|(name, _)| {
        !matches!(
            name.as_str(),
            "name"
                | "realName"
                | "codePath"
                | "nativeLibraryPath"
                | "primaryCpuAbi"
                | "secondaryCpuAbi"
                | "cpuAbiOverride"
                | "requiredCpuAbi"
                | "publicFlags"
                | "privateFlags"
                | "ft"
                | "ts"
                | "it"
                | "ut"
                | "version"
                | "targetSdkVersion"
                | "restrictUpdateHash"
                | "scannedAsStoppedSystemApp"
                | "userId"
                | "sharedUserId"
                | "isSdkLibrary"
                | "installer"
                | "installerUid"
                | "updateOwner"
                | "installerAttributionTag"
                | "packageSource"
                | "isOrphaned"
                | "installInitiator"
                | "installInitiatorUninstalled"
                | "installOriginator"
                | "volumeUuid"
                | "categoryHint"
                | "updateAvailable"
                | "forceQueryable"
                | "pendingRestore"
                | "debuggable"
                | "isLoading"
                | "baseRevisionCode"
                | "pageSizeCompat"
                | "loadingProgress"
                | "loadingCompletedTime"
                | "domainSetId"
                | "appMetadataFilePath"
                | "appMetadataSource"
        )
    });
    node.content.retain(|n| !matches!(n, Node::Element(e) if matches!(e.name.as_str(), "uses-sdk-lib" | "uses-static-lib" | "sigs" | "install-initiator-sigs" | "proper-signing-keyset" | "upgrade-keyset" | "defined-keyset" | "mime-group" | "split-version")));
    for (name, value) in [
        ("name", Some(&package.name)),
        ("codePath", Some(&package.code_path)),
        ("realName", package.real_name.as_ref()),
        (
            "nativeLibraryPath",
            package.legacy_native_library_path.as_ref(),
        ),
        ("primaryCpuAbi", package.primary_cpu_abi.as_ref()),
        ("secondaryCpuAbi", package.secondary_cpu_abi.as_ref()),
        ("cpuAbiOverride", package.cpu_abi_override.as_ref()),
        (
            "appMetadataFilePath",
            package.app_metadata_file_path.as_ref(),
        ),
    ] {
        attribute(&mut node, name, value.cloned().map(Value::String));
    }
    for (name, value) in [
        ("ft", package.last_modified_time),
        ("ut", package.last_update_time),
        ("loadingCompletedTime", package.loading_completed_time),
    ] {
        attribute(&mut node, name, Some(Value::LongHex(value)));
    }
    attribute(
        &mut node,
        "version",
        Some(Value::Long(package.version_code)),
    );
    attribute(
        &mut node,
        "targetSdkVersion",
        Some(Value::Int(package.target_sdk_version)),
    );
    attribute(
        &mut node,
        "restrictUpdateHash",
        package.restrict_update_hash.clone().map(Value::BytesBase64),
    );
    attribute(
        &mut node,
        "scannedAsStoppedSystemApp",
        Some(Value::Bool(package.scanned_as_stopped_system_app)),
    );
    attribute(
        &mut node,
        if package.shared_user {
            "sharedUserId"
        } else {
            "userId"
        },
        Some(Value::Int(package.app_id)),
    );
    attribute(
        &mut node,
        "loadingProgress",
        Some(Value::Float(package.loading_progress)),
    );
    attribute(
        &mut node,
        "appMetadataSource",
        Some(Value::Int(package.app_metadata_source)),
    );
    for library in &package.uses_sdk_libraries {
        let mut lib = element("uses-sdk-lib");
        attribute(&mut lib, "name", Some(Value::String(library.name.clone())));
        attribute(
            &mut lib,
            "version",
            Some(Value::Long(library.version_major)),
        );
        attribute(&mut lib, "optional", Some(Value::Bool(library.optional)));
        node.content.push(Node::Element(lib));
    }
    for (name, version) in &package.uses_static_libraries {
        let mut lib = element("uses-static-lib");
        attribute(&mut lib, "name", Some(Value::String(name.clone())));
        attribute(&mut lib, "version", Some(Value::Long(*version)));
        node.content.push(Node::Element(lib));
    }
    if !active {
        return Ok(node);
    }
    for (name, value) in [
        ("publicFlags", package.flags),
        ("privateFlags", package.private_flags),
        ("packageSource", package.install_source.package_source),
    ] {
        attribute(&mut node, name, Some(Value::Int(value)));
    }
    if !(0..128).contains(&package.page_size_compat) {
        return Err("invalid page-size compatibility flags".into());
    }
    for (name, value) in [
        (
            "installerUid",
            (package.install_source.installer_uid != -1)
                .then_some(package.install_source.installer_uid),
        ),
        (
            "categoryHint",
            (package.category_hint != -1).then_some(package.category_hint),
        ),
        (
            "baseRevisionCode",
            (package.base_revision_code != 0).then_some(package.base_revision_code),
        ),
        (
            "pageSizeCompat",
            (package.page_size_compat != 0).then_some(package.page_size_compat),
        ),
    ] {
        attribute(&mut node, name, value.map(Value::Int));
    }
    for (name, value) in [
        ("installer", &package.install_source.installer),
        ("updateOwner", &package.install_source.update_owner),
        (
            "installerAttributionTag",
            &package.install_source.installer_attribution_tag,
        ),
        (
            "installInitiator",
            &package.install_source.initiating_package,
        ),
        (
            "installOriginator",
            &package.install_source.originating_package,
        ),
        ("volumeUuid", &package.volume_uuid),
    ] {
        attribute(&mut node, name, value.clone().map(Value::String));
    }
    for (name, value) in [
        ("isOrphaned", package.install_source.is_orphaned),
        (
            "installInitiatorUninstalled",
            package.install_source.initiating_package_uninstalled,
        ),
        ("updateAvailable", package.update_available),
        ("forceQueryable", package.force_queryable),
        ("pendingRestore", package.pending_restore),
        ("debuggable", package.debuggable),
        ("isLoading", package.is_loading()),
    ] {
        attribute(&mut node, name, value.then_some(Value::Bool(true)));
    }
    attribute(
        &mut node,
        "domainSetId",
        Some(Value::String(
            package
                .domain_set_id
                .clone()
                .ok_or("active setting requires its domain owner")?,
        )),
    );
    for (name, types) in &package.mime_groups {
        let mut group = element("mime-group");
        attribute(
            &mut group,
            "name",
            Some(Value::String(name.clone().ok_or(
                "original MIME writer cannot serialize a null group",
            )?)),
        );
        for value in types {
            let mut mime = element("mime-type");
            attribute(
                &mut mime,
                "value",
                Some(Value::String(value.clone().ok_or(
                    "original MIME writer cannot serialize a null type",
                )?)),
            );
            group.content.push(Node::Element(mime));
        }
        node.content.push(Node::Element(group));
    }
    Ok(node)
}
