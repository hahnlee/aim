//! Scoped runtime projection for complete PackageStateInternal assembly (#873).
use super::Snapshot;
use aim_binder_host::parcel::Parcel;

pub fn captured(snapshot: &Snapshot, name: &str, factory: bool) -> Result<Option<Vec<u8>>, String> {
    let owner = snapshot.owner();
    let packages = if factory {
        &owner.settings.disabled_system_packages
    } else {
        &owner.settings.packages
    };
    let Some(setting) = packages.iter().find(|p| p.name == name) else {
        return Ok(None);
    };
    owner.validate_replica_runtime(Some(snapshot.usage()))?;
    let state = owner
        .replica_runtime(name, factory)?
        .ok_or("missing replica runtime owner")?;
    write(
        snapshot.version(),
        setting,
        factory,
        if factory {
            owner.disabled_loaded_packages()
        } else {
            owner.loaded_packages()
        }
        .contains_key(name),
        state,
    )
    .map(Some)
}

pub(super) fn write(
    version: u64,
    setting: &crate::package::settings::Package,
    factory: bool,
    has_code: bool,
    state: &crate::package::scan::ReplicaRuntime,
) -> Result<Vec<u8>, String> {
    let name = setting.name.as_str();
    let mut p = Parcel::new();
    p.write_i64(version as i64);
    p.write_string16(Some(name));
    p.write_i32(setting.app_id);
    p.write_bool(factory);
    p.write_bool(has_code);
    p.write_string16(state.seinfo.as_deref());
    p.write_string16(state.override_seinfo.as_deref());
    aim_service_aidl::write_long_array(&mut p, Some(&state.usage));
    // Reuse the original library owner envelope, preserving nullable file slots.
    let mut libraries = Parcel::new();
    crate::package::info::write_libraries(&mut libraries, Some(&state.libraries));
    p.write_i64(version as i64);
    p.write_string16(Some(name));
    p.write_i32(setting.app_id);
    p.write_i32(
        i32::try_from(state.library_files.len()).map_err(|_| "too many runtime library files")?,
    );
    for path in &state.library_files {
        p.write_string16(path.as_deref());
    }
    aim_service_aidl::write_byte_array(&mut p, Some(libraries.data()));
    if p.data().len() > i32::MAX as usize {
        return Err("runtime state exceeds transport size".into());
    }
    Ok(p.data().to_vec())
}
