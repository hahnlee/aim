//! SharedUserApi fields projected from the captured native UID owner (#836/#870).
//! SharedUserSetting, android-16.0.0_r1. Copyright AOSP, Apache License 2.0.
use super::{Snapshot, endpoint::write_signing};
use crate::package::{settings::PRIVATE_FLAG_PRIVILEGED, sign::SigningDetails};
use aim_binder_host::parcel::Parcel;

pub fn captured(snapshot: &Snapshot, name: &str) -> Result<Option<Vec<u8>>, String> {
    captured_owner(snapshot.version(), snapshot.owner(), name)
}

pub fn captured_owner(
    version: u64,
    owner: &crate::package::scan::SigningScan,
    name: &str,
) -> Result<Option<Vec<u8>>, String> {
    let Some(group) = owner.identities.shared_users.get(name) else {
        return Ok(None);
    };
    if !owner.capture_ready() {
        return Err("scan metadata is not finalized".into());
    }
    group.validate_retained()?;
    let members: Vec<_> = group.package_names().collect();
    owner.validate_displaced_shared_settings()?;
    let mut current = std::collections::BTreeSet::new();
    for setting in &owner.settings.packages {
        if setting.shared_app_id() == Some(group.app_id)
            && !owner.is_displaced_shared_setting(setting)?
        {
            current.insert(setting.name.as_str());
        }
    }
    if members
        .iter()
        .copied()
        .filter(|n| group.has_package(n))
        .collect::<std::collections::BTreeSet<_>>()
        != current
    {
        return Err("shared UID member owner differs".into());
    }
    let signing = group
        .signatures
        .as_ref()
        .map(SigningDetails::from_saved)
        .transpose()?;
    let mut p = Parcel::new();
    p.write_i64(version as i64);
    p.write_string16(Some(name));
    p.write_i32(group.app_id);
    p.write_bool(group.private_flags & PRIVATE_FLAG_PRIVILEGED != 0);
    p.write_i32(group.seinfo_target_sdk());
    // The pinned snapshot copy constructor omits seInfoTargetSdkVersion.
    // This projection is distinct from the live owner used for boot labels.
    p.write_i32(0);
    write_signing(&mut p, signing.as_ref());
    p.write_i32(i32::try_from(members.len()).map_err(|_| "too many shared UID members")?);
    // A name can own both a current and a retained instance. Emit each once.
    let names: std::collections::BTreeSet<_> = members.into_iter().collect();
    for member in names {
        if group.has_package(member) {
            p.write_string16(Some(member));
            p.write_bool(false);
        }
        if let Some(retained) = group.retained_setting(member) {
            p.write_string16(Some(member));
            p.write_bool(true);
            let bytes = super::retained_record::captured(version, name, group, retained)?;
            aim_service_aidl::write_byte_array(&mut p, Some(&bytes));
        }
    }
    if p.data().len() > i32::MAX as usize {
        return Err("shared UID record exceeds transport size".into());
    }
    Ok(Some(p.data().to_vec()))
}
