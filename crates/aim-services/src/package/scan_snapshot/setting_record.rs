//! Captured PackageSetting scalar owners, before complete facade assembly (#836).
use super::Snapshot;
use aim_binder_host::parcel::Parcel;
use aim_service_aidl::write_byte_array;

pub fn captured(snapshot: &Snapshot, name: &str, factory: bool) -> Result<Option<Vec<u8>>, String> {
    let settings = &snapshot.owner().settings;
    let packages = if factory {
        &settings.disabled_system_packages
    } else {
        &settings.packages
    };
    let Some(s) = packages.iter().find(|s| s.name == name) else {
        return Ok(None);
    };
    let mut p = Parcel::new();
    p.write_i64(snapshot.version() as i64);
    p.write_string16(Some(name));
    p.write_bool(factory);
    p.write_string16(s.real_name.as_deref());
    p.write_string16(Some(&s.code_path));
    p.write_string16(s.legacy_native_library_path.as_deref());
    p.write_string16(s.primary_cpu_abi.as_deref());
    p.write_string16(s.secondary_cpu_abi.as_deref());
    p.write_string16(s.cpu_abi_override.as_deref());
    p.write_i32(s.flags);
    p.write_i32(s.private_flags);
    p.write_i64(s.last_modified_time);
    p.write_i64(s.last_update_time);
    p.write_i64(s.legacy_first_install_time);
    p.write_i64(s.version_code);
    p.write_i32(s.target_sdk_version);
    p.write_i32(s.app_id);
    p.write_bool(s.shared_user);
    p.write_bool(s.is_sdk_library);
    p.write_string16(s.volume_uuid.as_deref());
    p.write_i32(s.category_hint);
    p.write_bool(s.update_available);
    p.write_bool(s.force_queryable);
    p.write_bool(s.pending_restore);
    p.write_bool(s.debuggable);
    p.write_bool(s.scanned_as_stopped_system_app);
    p.write_i32(s.base_revision_code);
    p.write_i32(s.page_size_compat);
    p.write_f32(s.loading_progress);
    p.write_i64(s.loading_completed_time);
    p.write_string16(s.domain_set_id.as_deref());
    p.write_string16(s.app_metadata_file_path.as_deref());
    p.write_i32(s.app_metadata_source);
    write_byte_array(&mut p, s.restrict_update_hash.as_deref());
    match &s.old_paths {
        None => p.write_i32(-1),
        Some(paths) => {
            p.write_i32(i32::try_from(paths.len()).map_err(|_| "too many old paths")?);
            for path in paths {
                p.write_string16(path.as_deref());
            }
        }
    }
    let source = &s.install_source;
    if source.clone().normalized()? != *source {
        return Err("captured install source is not normalized".into());
    }

    p.write_string16(source.initiating_package.as_deref());
    p.write_string16(source.originating_package.as_deref());
    p.write_string16(source.installer.as_deref());
    p.write_i32(source.installer_uid);
    p.write_string16(source.update_owner.as_deref());
    p.write_string16(source.installer_attribution_tag.as_deref());
    p.write_i32(source.package_source);
    p.write_bool(source.is_orphaned);
    p.write_bool(source.initiating_package_uninstalled);
    let signing = source
        .initiating_package_signatures
        .as_ref()
        .map(crate::package::sign::SigningDetails::from_saved)
        .transpose()?;
    super::endpoint::write_signing(&mut p, signing.as_ref());
    let keys = &s.key_set_data;
    p.write_i64(keys.proper_signing_key_set);
    aim_service_aidl::write_long_array(
        &mut p,
        (!keys.upgrade_key_sets.is_empty()).then_some(keys.upgrade_key_sets.as_slice()),
    );
    p.write_i32(i32::try_from(keys.defined_key_sets.len()).map_err(|_| "too many keyset aliases")?);
    for (alias, id) in &keys.defined_key_sets {
        p.write_string16(alias.as_deref());
        p.write_i64(*id);
    }
    p.write_i32(i32::try_from(s.uses_sdk_libraries.len()).map_err(|_| "too many SDK libraries")?);
    for library in &s.uses_sdk_libraries {
        p.write_string16(Some(&library.name));
        p.write_i64(library.version_major);
        p.write_bool(library.optional);
    }
    p.write_i32(
        i32::try_from(s.uses_static_libraries.len()).map_err(|_| "too many static libraries")?,
    );
    for (name, version) in &s.uses_static_libraries {
        p.write_string16(Some(name));
        p.write_i64(*version);
    }
    p.write_i32(i32::try_from(s.mime_groups.len()).map_err(|_| "too many MIME groups")?);
    let mut groups = std::collections::BTreeSet::new();
    for (name, types) in &s.mime_groups {
        if !groups.insert(name) {
            return Err("duplicate MIME group owner".into());
        }
        p.write_string16(name.as_deref());
        p.write_i32(i32::try_from(types.len()).map_err(|_| "too many MIME types")?);
        for value in types {
            p.write_string16(value.as_deref());
        }
    }
    if p.data().len() > i32::MAX as usize {
        return Err("package setting exceeds transport size".into());
    }
    p.write_bool(snapshot.owner().has_legacy_permissions());
    if snapshot.owner().has_legacy_permissions() {
        let legacy = snapshot
            .owner()
            .legacy_permissions(name, factory)?
            .ok_or("missing legacy setting owner")?;
        let users: Vec<_> = legacy.users().iter().map(|user| user.id).collect();
        aim_service_aidl::write_int_array(&mut p, Some(&users));
        write_byte_array(&mut p, Some(&legacy.bytes()));
    }
    p.write_i32(
        snapshot
            .owner()
            .install_permissions_fixed(name, factory)?
            .map_or(-1, i32::from),
    );
    Ok(Some(p.data().to_vec()))
}
