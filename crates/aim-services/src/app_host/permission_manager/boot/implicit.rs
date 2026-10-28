//! AppIdPermissionPolicy.kt:1112-1180,1238-1287, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::*;

const RUNTIME_MASK: i32 =
    ((1 << 24) - 1) & !(INSTALL_GRANTED | INSTALL_REVOKED | PROTECTION_GRANTED);

fn sources<'a>(name: &'a str, policy: &'a Policy) -> impl Iterator<Item = &'a str> {
    // The external state's implicitToSourcePermissions includes every split,
    // irrespective of target SDK; parsing already selected implicit requests.
    policy
        .splits
        .iter()
        .filter(move |s| s.new_permissions.iter().any(|p| p == name))
        .map(|s| s.name.as_str())
}

fn permission<'a>(system: &'a AccessSystem, name: &str) -> Result<&'a Permission, Exception> {
    system
        .permissions
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| {
            Exception::new(
                EX_ILLEGAL_STATE,
                format!("Unknown permission {name} in split permissions"),
            )
        })
}

pub(super) fn non_runtime_source(
    system: &AccessSystem,
    name: &str,
    policy: &Policy,
) -> Result<bool, Exception> {
    for source in sources(name, policy) {
        if !is_runtime(permission(system, source)?) {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(super) fn inherit(
    base: &permissions::AccessView,
    system: &AccessSystem,
    user: &mut AccessUser,
    id: i32,
    packages: &[&PackageState],
    policy: &Policy,
) -> Result<(), Exception> {
    let target = packages
        .iter()
        .filter_map(|p| p.pkg.as_ref())
        .map(|p| p.target_sdk_version)
        .min()
        .unwrap_or(CUR_DEVELOPMENT);
    let mut seen = BTreeSet::new();
    for name in packages
        .iter()
        .filter_map(|p| p.pkg.as_ref())
        .flat_map(|p| p.implicit_permissions.iter())
    {
        if !seen.insert(name)
            || !is_runtime(permission(system, name)?)
            || base.user0.permission_flags(id, name) != 0
        {
            continue;
        }
        let mut sources = sources(name, policy).peekable();
        if sources.peek().is_none() {
            continue;
        }
        let mut flags = user.permission_flags(id, name);
        // AppIdPermissionPolicy.kt:1262-1276. As in PMSImpl.java:3076-3114,
        // granted sources win over denied sources; merge only runtime flags.
        for source in sources {
            permission(system, source)?;
            let source_flags = user.permission_flags(id, source);
            let source_granted = permission_granted(source_flags);
            let granted = permission_granted(flags);
            if source_granted == granted || source_granted {
                if source_granted && !granted {
                    flags = 0;
                }
                flags |= source_flags & RUNTIME_MASK;
            }
        }
        // AppIdPermissionPolicy.kt:1278-1287,1934-1944; PMSImpl.java:3183-3195.
        if target >= M
            && matches!(
                name.as_str(),
                "android.permission.ACCESS_MEDIA_LOCATION"
                    | "android.permission.ACTIVITY_RECOGNITION"
                    | "android.permission.READ_MEDIA_AUDIO"
                    | "android.permission.READ_MEDIA_IMAGES"
                    | "android.permission.READ_MEDIA_VIDEO"
                    | "android.permission.READ_MEDIA_VISUAL_USER_SELECTED"
            )
        {
            flags &= !IMPLICIT;
        } else {
            flags |= IMPLICIT;
        }
        user.set_permission_flags(id, name, flags);
    }
    Ok(())
}
