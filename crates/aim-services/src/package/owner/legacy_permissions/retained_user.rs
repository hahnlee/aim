//! Live new-user grants for registered detached/shared retained UID owners.
use crate::package::{bootstrap::Bridge, owner::app_ids::Owner, scan::SigningScan,
    scan_snapshot::query_state::UserInputs};

pub(crate) fn capture_retained_user(scan: &SigningScan, bridge: &Bridge,
    app_id: i32, name: &str, user: i32) -> Result<UserInputs, String> {
    let slot = scan.identities.ids.owners().find(|(id, _)| *id == app_id)
        .map(|(_, slot)| slot).ok_or("retained permission UID is not registered")?;
    match slot {
        Owner::DetachedPackage(registered) if registered == name => {
            let setting = scan.identities.ids.detached_setting(app_id)
                .ok_or("retained permission detached setting unavailable")?;
            if setting.package.name != name || setting.package.app_id != app_id {
                return Err("retained permission detached identity differs".into());
            }
        }
        Owner::SharedUser(group) => {
            let group = scan.identities.shared_users.get(group).ok_or("retained permission shared UID unavailable")?;
            if group.app_id != app_id || !group.retained_settings().any(|(member, _)| member == name) {
                return Err("retained permission shared identity differs".into());
            }
        }
        _ => return Err("retained permission UID was rebound to another setting".into()),
    }
    // The original permission service keys live grants by appId. The saved
    // LegacyPermissionState is migration input and cannot supply new-user grants.
    let state = bridge.legacy_permissions(app_id, &[user])
        .map_err(|error| format!("retained live permission UID owner: {error:?}"))?;
    let state = state.user(user).ok_or("retained permission user projection unavailable")?;
    let mut granted_permissions = Vec::new();
    for permission in &state.permissions {
        if permission.granted {
            granted_permissions.push(permission.name.clone().ok_or("retained granted permission has null name")?);
        }
    }
    granted_permissions.sort();
    let gids = bridge.permission_gids(app_id, &[user])
        .map_err(|error| format!("retained live permission GID owner: {error:?}"))?
        .into_iter().map(|gid| i32::try_from(gid).map_err(|_| "retained permission GID exceeds Android range".to_owned()))
        .collect::<Result<_, _>>()?;
    Ok(UserInputs { gids, granted_permissions, domain_selection: None })
}
