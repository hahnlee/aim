//! Original SharedLibraryInfo objects from a finalized native dependency owner.
use super::Snapshot;
use aim_binder_host::parcel::Parcel;

pub fn captured(snapshot: &Snapshot, name: &str) -> Result<Option<Vec<u8>>, String> {
    let Some(setting) = snapshot
        .owner()
        .settings
        .packages
        .iter()
        .find(|p| p.name == name)
    else {
        return Ok(None);
    };
    let (files, infos) = snapshot
        .owner()
        .library_dependencies(name)?
        .ok_or("missing active library dependency owner")?;
    let mut libraries = Parcel::new();
    crate::package::info::write_libraries(&mut libraries, Some(infos));
    let mut p = Parcel::new();
    p.write_i64(snapshot.version() as i64);
    p.write_string16(Some(name));
    p.write_i32(setting.app_id);
    p.write_i32(i32::try_from(files.len()).map_err(|_| "too many library files")?);
    for path in files {
        p.write_string16(path.as_deref());
    }
    aim_service_aidl::write_byte_array(&mut p, Some(libraries.data()));
    if p.data().len() > i32::MAX as usize {
        return Err("library state exceeds transport size".into());
    }
    Ok(Some(p.data().to_vec()))
}
