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

/// Write the complete native Settings owners; the whole-document round trip
/// rejects unresolved changes.
pub(super) fn replace(original: &Element, scan: &SigningScan) -> Result<Element, String> {
    if !scan.capture_ready() {
        return Err("cannot persist unfinished scan metadata".into());
    }
    let mut displaced_without_group = BTreeSet::new();
    for package in &scan.settings.packages {
        if scan.is_displaced_shared_setting(package)?
            && scan.identities.ids.get(package.uid_owner_id()).is_none()
        {
            displaced_without_group.insert(package.name.clone());
        }
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
    let mut root = super::verifier::replace(original,scan.settings.verifier.as_deref());
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
    write_versions(&mut root, &mut expected)?;
    root = super::global_permissions::replace(&root, &expected)?;
    root = super::domains::replace(&root, &expected.domain_verification)?;
    expected.permissions = super::global_permissions::persisted(&expected.permissions);
    expected.permission_trees = super::global_permissions::persisted(&expected.permission_trees);
    // writeLPr keeps the incoming table entry; readLPw drops its pending
    // shared reference when the old group was pruned. Preserve the written
    // package's keysets while validating the distinct restored inventory.
    let written = expected.clone();
    expected
        .packages
        .retain(|p| !displaced_without_group.contains(&p.name));
    if expected.key_sets.versioned {
        root = key_sets::replace_for_scan(&root, &written, &expected)?;
    }
    if signing::persisted(Settings::parse(&root)?) != signing::persisted(expected) {
        return Err("scan settings require an unresolved global owner or do not round trip".into());
    }
    Ok(root)
}

// Settings.writeLPr writes mVersion in ArrayMap order, replacing legacy tags.
fn write_versions(root: &mut Element, settings: &mut Settings) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for version in &settings.versions {
        if !seen.insert(version.volume_uuid.clone()) {
            return Err("duplicate volume version owner".into());
        }
    }
    settings.versions.sort_by_key(|version| {
        version
            .volume_uuid
            .as_deref()
            .map(crate::package::info::java_hash)
            .unwrap_or(0)
    });
    root.content.retain(|node| !matches!(node, Node::Element(e) if matches!(e.name.as_str(), "version" | "last-platform-version" | "database-version")));
    let mut nodes = Vec::new();
    for version in &settings.versions {
        let mut node = element("version");
        for (name, value) in [
            ("volumeUuid", &version.volume_uuid),
            ("buildFingerprint", &version.build_fingerprint),
            ("fingerprint", &version.fingerprint),
        ] {
            attribute(&mut node, name, value.clone().map(Value::String));
        }
        attribute(
            &mut node,
            "sdkVersion",
            Some(Value::Int(version.sdk_version)),
        );
        attribute(
            &mut node,
            "databaseVersion",
            Some(Value::Int(version.database_version)),
        );
        nodes.push(Node::Element(node));
    }
    root.content.splice(0..0, nodes);
    Ok(())
}

#[cfg(test)]
mod version_tests {
    use super::*;

    #[test]
    fn scan_version_commit_replaces_legacy_values_and_retains_collision_slots() {
        let mut root = aim_android_xml::read(b"<packages><last-platform-version internal='35' external='34'/><database-version internal='2' external='1'/><extension value='keep'/></packages>").unwrap();
        let mut settings = Settings::parse(&root).unwrap();
        settings.find_or_create_version(None).database_version = 3;
        for name in ["BB", "Aa", ""] {
            let version = settings.find_or_create_version(Some(name.into()));
            version.sdk_version = 36;
            version.database_version = 7;
            version.build_fingerprint = Some(String::new());
            version.fingerprint = Some("partitions".into());
        }
        write_versions(&mut root, &mut settings).unwrap();
        assert_eq!(Settings::parse(&root).unwrap(), settings);
        assert!(root.children().any(|node| node.name == "extension"));
        assert!(!root.children().any(|node| matches!(
            node.name.as_str(),
            "last-platform-version" | "database-version"
        )));
        let collisions: Vec<_> = settings
            .versions
            .iter()
            .filter_map(|version| version.volume_uuid.as_deref())
            .filter(|name| *name == "BB" || *name == "Aa")
            .collect();
        assert_eq!(collisions, ["BB", "Aa"]);
    }
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

impl super::Store {
    /// InitAppsHelper finishes both first and restored scans before Settings
    /// write ownership is transferred to runtime PM. Saved boot reconciliation
    /// is a new complete owner; first-boot-only persistence leaves disk stale.
    pub fn commit_completed_boot_scan(
        &mut self, snapshot:&crate::package::scan_snapshot::Snapshot, first_boot:bool,
    )->Result<(),super::WriteError>{
        self.commit_scan_settings(snapshot)?;
        let users=self.state().users.iter().map(|(id,_)|*id).collect::<Vec<_>>();
        for user in users {
            let result=if first_boot {
                self.claim_unread_restrictions(user).map_err(super::WriteError::before)
                    .and_then(|()|self.commit_initial_scan_restrictions(snapshot.owner(),user,
                        crate::package::restrictions::PINNED_CROSS_USER_SUSPENSIONS,
                        aim_android_xml::Element{name:"package-restrictions".into(),attrs:vec![],content:vec![]}))
            }else{
                self.commit_updated_scan_restrictions(snapshot.owner(),user,crate::package::restrictions::PINNED_CROSS_USER_SUSPENSIONS)
            };
            if let Err(mut error)=result {error.committed=true;return Err(error);}
        }
        self.validate_committed_scan(snapshot.owner())
    }
}
