//! AppIdPermissionUpgrade.kt, android-16.0.0_r1 (line references below).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::*;

const BODY: &str = "android.permission.BODY_SENSORS";
const BACKGROUND_BODY: &str = "android.permission.BODY_SENSORS_BACKGROUND";
const HEART: &str = "android.permission.health.READ_HEART_RATE";
const BACKGROUND_HEALTH: &str = "android.permission.health.READ_HEALTH_DATA_IN_BACKGROUND";
const STORAGE: [&str; 2] = [
    "android.permission.READ_EXTERNAL_STORAGE",
    "android.permission.WRITE_EXTERNAL_STORAGE",
];
const VISUAL: [&str; 3] = [
    "android.permission.READ_MEDIA_IMAGES",
    "android.permission.READ_MEDIA_VIDEO",
    "android.permission.ACCESS_MEDIA_LOCATION",
];
const SELECTED: &str = "android.permission.READ_MEDIA_VISUAL_USER_SELECTED";

fn app_op_granted(user: &AccessUser, id: i32, name: &str) -> bool {
    let flags = user.permission_flags(id, name);
    // PermissionFlags.kt:349-361, also used by upgrade's isRuntimePermissionGranted.
    permission_granted(flags)
        && flags & (RESTRICTION_REVOKED | SOFT_RESTRICTED | APP_OP_REVOKED) == 0
}

fn grant(
    system: &AccessSystem,
    user: &mut AccessUser,
    ps: &PackageState,
    name: &str,
) -> Result<(), Exception> {
    // AppIdPermissionUpgrade.kt:381-415: respect instant and all fixed grants.
    let permission = system
        .permissions
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("Unknown upgrade permission {name}"),
            )
        })?;
    if ps.users.get(&0).is_some_and(|u| u.instant_app)
        && permission.protection_level & PROTECTION_FLAG_INSTANT == 0
    {
        return Ok(());
    }
    let id = ps.shared_user_app_id.unwrap_or(ps.app_id);
    let mut flags = user.permission_flags(id, name);
    if flags & (USER_SET | ONE_TIME | USER_FIXED | POLICY_FIXED | SYSTEM_FIXED) == 0 {
        flags |= RUNTIME_GRANTED;
        flags &= !(APP_OP_REVOKED | IMPLICIT | LEGACY_GRANTED | HIBERNATION | ONE_TIME);
        user.set_permission_flags(id, name, flags);
    }
    Ok(())
}

fn revoke(user: &mut AccessUser, id: i32, name: &str) -> bool {
    // AppIdPermissionUpgrade.kt:422-451: system/policy fixed grants survive.
    let flags = user.permission_flags(id, name);
    if flags & (SYSTEM_FIXED | POLICY_FIXED) != 0 {
        return false;
    }
    user.set_permission_flags(
        id,
        name,
        flags
            & !(RUNTIME_GRANTED
                | USER_SET
                | USER_FIXED
                | APP_OP_REVOKED
                | ONE_TIME
                | HIBERNATION
                | USER_SELECTED
                | PREGRANT
                | ROLE),
    );
    true
}

pub(super) fn package(
    system: &AccessSystem,
    user: &mut AccessUser,
    ps: &PackageState,
    version: i32,
) -> Result<(), Exception> {
    let code = ps.pkg.as_ref().expect("upgrade code");
    let id = ps.shared_user_app_id.unwrap_or(ps.app_id);
    let requested = |name: &str| code.requested_permissions.iter().any(|p| p == name);
    // AppIdPermissionUpgrade.kt:38-91,94-140: version 3 restriction and location.
    if version <= 3 {
        for name in &code.requested_permissions {
            if matches!(
                name.as_str(),
                ACCESS_BACKGROUND_LOCATION
                    | "android.permission.READ_EXTERNAL_STORAGE"
                    | "android.permission.WRITE_EXTERNAL_STORAGE"
                    | "android.permission.SEND_SMS"
                    | "android.permission.RECEIVE_SMS"
                    | "android.permission.RECEIVE_WAP_PUSH"
                    | "android.permission.RECEIVE_MMS"
                    | "android.permission.READ_CELL_BROADCASTS"
                    | "android.permission.READ_CALL_LOG"
                    | "android.permission.WRITE_CALL_LOG"
                    | "android.permission.PROCESS_OUTGOING_CALLS"
            ) {
                user.set_permission_flags(
                    id,
                    name,
                    user.permission_flags(id, name) | UPGRADE_EXEMPT,
                );
            }
        }
        if requested(ACCESS_BACKGROUND_LOCATION)
            && [
                "android.permission.ACCESS_FINE_LOCATION",
                "android.permission.ACCESS_COARSE_LOCATION",
            ]
            .iter()
            .any(|name| app_op_granted(user, id, name))
        {
            grant(system, user, ps, ACCESS_BACKGROUND_LOCATION)?;
        }
    }
    // AppIdPermissionUpgrade.kt:143-167.
    if version <= 10 && requested(VISUAL[2]) && app_op_granted(user, id, STORAGE[0]) {
        grant(system, user, ps, VISUAL[2])?;
    }
    // AppIdPermissionUpgrade.kt:170-239: scoped media and restricted background sensors.
    if version <= 12 {
        if code.target_sdk_version >= 33
            && STORAGE.iter().any(|name| {
                requested(name)
                    && app_op_granted(user, id, name)
                    && user.permission_flags(id, name) & USER_SET != 0
            })
        {
            for name in [
                "android.permission.READ_MEDIA_AUDIO",
                VISUAL[0],
                VISUAL[1],
                VISUAL[2],
                SELECTED,
            ] {
                if requested(name) {
                    grant(system, user, ps, name)?;
                }
            }
        }
        if requested(BACKGROUND_BODY)
            && user.permission_flags(id, BACKGROUND_BODY)
                & (INSTALLER_EXEMPT | SYSTEM_EXEMPT | UPGRADE_EXEMPT)
                == 0
        {
            user.set_permission_flags(
                id,
                BACKGROUND_BODY,
                user.permission_flags(id, BACKGROUND_BODY) | UPGRADE_EXEMPT,
            );
            if app_op_granted(user, id, BODY) {
                grant(system, user, ps, BACKGROUND_BODY)?;
            }
        }
    }
    // AppIdPermissionUpgrade.kt:242-271.
    if version <= 14
        && code.target_sdk_version >= 33
        && requested(SELECTED)
        && VISUAL.iter().any(|name| {
            requested(name)
                && app_op_granted(user, id, name)
                && user.permission_flags(id, name) & USER_SET != 0
        })
    {
        grant(system, user, ps, SELECTED)?;
    }
    // AppIdPermissionUpgrade.kt:283-368: conservative health grant sync.
    if version <= 16 && code.target_sdk_version < 36 {
        let mut body = app_op_granted(user, id, BODY);
        let heart = app_op_granted(user, id, HEART);
        if requested(BODY) && requested(HEART) && body != heart {
            if body && revoke(user, id, BODY) {
                body = false;
            }
            if heart {
                revoke(user, id, HEART);
            }
        }
        let mut background = app_op_granted(user, id, BACKGROUND_BODY);
        if !body && background && revoke(user, id, BACKGROUND_BODY) {
            background = false;
        }
        let health = app_op_granted(user, id, BACKGROUND_HEALTH);
        if requested(BACKGROUND_BODY) && requested(BACKGROUND_HEALTH) && background != health {
            if background {
                revoke(user, id, BACKGROUND_BODY);
            }
            if health {
                revoke(user, id, BACKGROUND_HEALTH);
            }
        }
    }
    Ok(())
}
