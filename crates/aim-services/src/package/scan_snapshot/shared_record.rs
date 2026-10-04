//! SharedUserApi fields projected from the captured native UID owner (#836/#870).
//! SharedUserSetting, android-16.0.0_r1. Copyright AOSP, Apache License 2.0.
use super::{Snapshot, endpoint::write_signing};
use crate::package::{settings::PRIVATE_FLAG_PRIVILEGED, sign::SigningDetails};
use aim_binder_host::parcel::Parcel;

pub fn captured(snapshot: &Snapshot, name: &str) -> Result<Option<Vec<u8>>, String> {
    let owner = snapshot.owner();
    let Some(group) = owner.identities.shared_users.get(name) else {
        return Ok(None);
    };
    if !owner.capture_ready() {
        return Err("scan metadata is not finalized".into());
    }
    if group.member_count()
        != group
            .package_names()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    {
        return Err(
            "distinct shared setting instances require captured instance identities (#905)".into(),
        );
    }
    let members: Vec<_> = group.package_names().collect();
    let settings: std::collections::BTreeSet<_> = owner
        .settings
        .packages
        .iter()
        .filter(|p| p.shared_app_id() == Some(group.app_id))
        .map(|p| p.name.as_str())
        .collect();
    if members
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        != settings
    {
        return Err("shared UID member owner differs".into());
    }
    let signing = group
        .signatures
        .as_ref()
        .map(SigningDetails::from_saved)
        .transpose()?;
    let mut p = Parcel::new();
    p.write_i64(snapshot.version() as i64);
    p.write_string16(Some(name));
    p.write_i32(group.app_id);
    p.write_bool(group.private_flags & PRIVATE_FLAG_PRIVILEGED != 0);
    p.write_i32(group.seinfo_target_sdk());
    // The pinned snapshot copy constructor omits seInfoTargetSdkVersion.
    // This projection is distinct from the live owner used for boot labels.
    p.write_i32(0);
    write_signing(&mut p, signing.as_ref());
    p.write_i32(i32::try_from(members.len()).map_err(|_| "too many shared UID members")?);
    for member in members {
        p.write_string16(Some(member));
    }
    if p.data().len() > i32::MAX as usize {
        return Err("shared UID record exceeds transport size".into());
    }
    Ok(Some(p.data().to_vec()))
}
