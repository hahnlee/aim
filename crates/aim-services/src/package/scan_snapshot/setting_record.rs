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
    if p.data().len() > i32::MAX as usize {
        return Err("package setting exceeds transport size".into());
    }
    Ok(Some(p.data().to_vec()))
}
