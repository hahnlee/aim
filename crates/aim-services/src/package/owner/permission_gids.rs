//! Settings.writePackageListLPrInternal's original permission-owner input.
//! Ported from android-16.0.0_r1, Copyright (C) The Android Open Source
//! Project, Apache License 2.0. AccessCheckingService remains the owner.
use aim_binder_host::{
    local::Strong,
    parcel::{Exception, Parcel},
};
use aim_service_aidl::dev_aim_server_ibridge as bridge;
use std::collections::BTreeSet;

#[derive(Debug, PartialEq, Eq)]
pub enum PermissionGidError {
    Input(String),
    Transport(i32),
    Owner(Exception),
}

/// The caller supplies the complete active-user inventory, plus a user being
/// created if applicable. Preserve user and GID order and duplicates, as the
/// original IntArray.addAll does; never substitute a saved packages.list row.
pub fn query(owner: &Strong, app_id: i32, users: &[i32]) -> Result<Vec<u32>, PermissionGidError> {
    let invalid = |message: &str| PermissionGidError::Input(message.into());
    if app_id < 0 || users.is_empty() {
        return Err(invalid(
            "permission GIDs require an app ID and resolved active users",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut uids = Vec::new();
    for &user in users {
        if user < 0 || !seen.insert(user) {
            return Err(invalid(
                "permission GIDs require distinct nonnegative users",
            ));
        }
        uids.push(
            user.checked_mul(100_000)
                .and_then(|base| base.checked_add(app_id % 100_000))
                .ok_or_else(|| {
                    invalid("permission GID user inventory produces an out-of-range UID")
                })?,
        );
    }
    let mut gids = Vec::new();
    for uid in uids {
        let mut request = Parcel::new();
        bridge::GetPermissionGidsForUid { uid }.write(&mut request);
        let reply = owner
            .transact(bridge::GET_PERMISSION_GIDS_FOR_UID, &request, false)
            .map_err(PermissionGidError::Transport)?;
        let result = bridge::read_get_permission_gids_for_uid_reply(&mut reply.reader())
            .map_err(PermissionGidError::Transport)?
            .map_err(PermissionGidError::Owner)?
            .ok_or_else(|| invalid("original permission owner returned null GIDs"))?;
        for gid in result {
            gids.push(
                gid.try_into()
                    .map_err(|_| invalid("original permission owner returned a negative GID"))?,
            );
        }
    }
    Ok(gids)
}
