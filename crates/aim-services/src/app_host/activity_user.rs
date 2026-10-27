//! `UserController.handleIncomingUser` (`android-16.0.0_r1`), with
//! the allow modes below, for this host's single current
//! user (0), with no other user in its profile group.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::parcel::Exception;

use super::activity::USER_CURRENT;
use super::user_service::SYSTEM_USER as USER_SYSTEM;
use super::{FIRST_APPLICATION_UID, FIRST_ISOLATED_UID, PER_USER_RANGE, SYSTEM_UID};
use crate::package::apps_filter::{ROOT_UID, SHELL_UID};

/// `ActivityManagerInternal`'s `ALLOW_*` modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AllowMode {
    FullOnly,
    NonFull,
    NonFullInProfile,
    ProfilesOrNonFull,
}

/// `UserHandle.USER_CURRENT_OR_SELF`.
const USER_CURRENT_OR_SELF: i32 = -3;
/// `Process.FIRST_APP_ZYGOTE_ISOLATED_UID`.
const FIRST_APP_ZYGOTE_ISOLATED_UID: i32 = 90_000;

/// `UserHandle.formatUid(StringBuilder, int)`.
fn format_uid(uid: i32) -> String {
    if uid < FIRST_APPLICATION_UID as i32 {
        return uid.to_string();
    }
    let user = uid / PER_USER_RANGE as i32;
    let app = uid % PER_USER_RANGE as i32;
    if super::is_isolated(uid) {
        if app > FIRST_ISOLATED_UID as i32 {
            format!("u{user}i{}", app - FIRST_ISOLATED_UID as i32)
        } else {
            format!("u{user}ai{}", app - FIRST_APP_ZYGOTE_ISOLATED_UID)
        }
    } else if app >= FIRST_APPLICATION_UID as i32 {
        format!("u{user}a{}", app - FIRST_APPLICATION_UID as i32)
    } else {
        format!("u{user}s{app}")
    }
}

pub(super) fn handle_incoming_user(
    uid: i32,
    user: i32,
    allow_all: bool,
    mode: AllowMode,
    name: Option<&str>,
    caller_package: Option<&str>,
    recents: impl FnOnce() -> Result<bool, Exception>,
    permission: impl Fn(&str) -> Result<bool, Exception>,
    profile_permission: impl FnOnce() -> Result<bool, Exception>,
    shell_restricted: impl FnOnce(i32) -> Result<bool, Exception>,
) -> Result<i32, Exception> {
    let calling_user = uid / PER_USER_RANGE as i32;
    if calling_user == user {
        return Ok(user);
    }
    // `unsafeConvertIncomingUser`.
    let mut target = if user == USER_CURRENT || user == USER_CURRENT_OR_SELF {
        USER_SYSTEM
    } else {
        user
    };
    if uid != ROOT_UID && uid != SYSTEM_UID as i32 {
        let same_profile_group = calling_user == target;
        let allowed = same_profile_group && recents()?
            || permission("android.permission.INTERACT_ACROSS_USERS_FULL")?
            || mode != AllowMode::FullOnly
                && (mode == AllowMode::ProfilesOrNonFull
                    && same_profile_group
                    && profile_permission()?
                    || permission("android.permission.INTERACT_ACROSS_USERS")?
                        && (mode != AllowMode::NonFullInProfile || same_profile_group));
        if !allowed {
            if user == USER_CURRENT_OR_SELF {
                target = calling_user;
            } else {
                return Err(Exception::security(format!(
                    "Permission Denial: {}{} asks to run as user {user} but is calling from uid {}; this requires android.permission.INTERACT_ACROSS_USERS_FULL{}{}",
                    name.unwrap_or("null"),
                    caller_package
                        .map(|p| format!(" from {p}"))
                        .unwrap_or_default(),
                    format_uid(uid),
                    if mode == AllowMode::NonFull
                        || mode == AllowMode::ProfilesOrNonFull
                        || mode == AllowMode::NonFullInProfile && same_profile_group
                    {
                        " or android.permission.INTERACT_ACROSS_USERS"
                    } else {
                        ""
                    },
                    if mode == AllowMode::ProfilesOrNonFull && same_profile_group {
                        " or android.permission.INTERACT_ACROSS_PROFILES"
                    } else {
                        ""
                    },
                )));
            }
        }
    }
    if !allow_all && target < USER_SYSTEM {
        return Err(Exception::illegal_argument(format!(
            "Call does not support special user #{target}"
        )));
    }
    if uid == SHELL_UID && target >= USER_SYSTEM && shell_restricted(target)? {
        return Err(Exception::security(format!(
            "Shell does not have permission to access user {target}"
        )));
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::super::USER_ALL;
    use super::*;
    use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_SECURITY};

    fn resolve_mode(
        mode: AllowMode,
        uid: i32,
        user: i32,
        grant: &str,
        profile: bool,
    ) -> Result<i32, Exception> {
        handle_incoming_user(
            uid,
            user,
            true,
            mode,
            Some("service"),
            Some("com.example"),
            || Ok(false),
            |p| Ok(p == grant),
            || Ok(profile),
            |_| Ok(false),
        )
    }

    const FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
    const NON_FULL: &str = "android.permission.INTERACT_ACROSS_USERS";

    #[test]
    fn full_only_requires_full_permission() {
        let mode = AllowMode::FullOnly;
        assert_eq!(resolve_mode(mode, 10_123, USER_CURRENT, FULL, false), Ok(0));
        assert!(resolve_mode(mode, 10_123, USER_CURRENT, NON_FULL, true).is_err());
        assert!(resolve_mode(mode, 10_123, USER_CURRENT, "", true).is_err());
    }

    #[test]
    fn non_full_allows_cross_user_permission() {
        let mode = AllowMode::NonFull;
        for grant in [FULL, NON_FULL] {
            assert_eq!(resolve_mode(mode, 10_123, 10, grant, false), Ok(10));
        }
        assert!(resolve_mode(mode, 10_123, USER_CURRENT, "", true).is_err());
    }

    #[test]
    fn non_full_in_profile_requires_the_same_group() {
        let mode = AllowMode::NonFullInProfile;
        assert_eq!(
            resolve_mode(mode, 10_123, USER_CURRENT, NON_FULL, false),
            Ok(0)
        );
        assert_eq!(resolve_mode(mode, 10_123, 10, FULL, false), Ok(10));
        assert!(resolve_mode(mode, 10_123, 10, NON_FULL, true).is_err());
        assert!(resolve_mode(mode, 10_123, USER_ALL, NON_FULL, true).is_err());
    }

    #[test]
    fn profiles_or_non_full_checks_profile_permission_only_in_the_same_group() {
        let mode = AllowMode::ProfilesOrNonFull;
        assert_eq!(resolve_mode(mode, 10_123, USER_CURRENT, "", true), Ok(0));
        assert!(resolve_mode(mode, 10_123, 10, "", true).is_err());
        assert_eq!(resolve_mode(mode, 10_123, 10, NON_FULL, false), Ok(10));
        let error = Exception::new(-5, "profile permission owner unavailable");
        assert_eq!(
            handle_incoming_user(
                10_123,
                USER_CURRENT,
                false,
                mode,
                None,
                None,
                || Ok(false),
                |_| Ok(false),
                || Err(error.clone()),
                |_| panic!("not shell"),
            ),
            Err(error)
        );
    }

    #[test]
    fn allow_modes_report_only_applicable_permissions() {
        for (mode, same, suffix) in [
            (AllowMode::FullOnly, true, ""),
            (
                AllowMode::NonFull,
                false,
                " or android.permission.INTERACT_ACROSS_USERS",
            ),
            (
                AllowMode::NonFullInProfile,
                true,
                " or android.permission.INTERACT_ACROSS_USERS",
            ),
            (AllowMode::NonFullInProfile, false, ""),
            (
                AllowMode::ProfilesOrNonFull,
                false,
                " or android.permission.INTERACT_ACROSS_USERS",
            ),
            (
                AllowMode::ProfilesOrNonFull,
                true,
                " or android.permission.INTERACT_ACROSS_USERS or android.permission.INTERACT_ACROSS_PROFILES",
            ),
        ] {
            let user = if same { USER_CURRENT } else { 10 };
            let error = resolve_mode(mode, 10_123, user, "", false).unwrap_err();
            assert_eq!(error.code, EX_SECURITY);
            assert_eq!(
                error.message,
                format!(
                    "Permission Denial: service from com.example asks to run as user {user} but is calling from uid u0a123; this requires {FULL}{suffix}"
                )
            );
            assert_eq!(
                resolve_mode(mode, 1_010_123, USER_CURRENT_OR_SELF, "", false),
                Ok(10)
            );
        }
    }

    #[test]
    fn incoming_user_denial_uses_user_handle_format_uid() {
        for (uid, formatted) in [
            (SHELL_UID, "2000"),
            (10_123, "u0a123"),
            (1_001_000, "u10s1000"),
            (90_001, "u0ai1"),
            (99_000, "u0ai9000"),
            (99_001, "u0i1"),
        ] {
            for full in [false, true] {
                let error = handle_incoming_user(
                    uid,
                    USER_ALL,
                    true,
                    if full {
                        AllowMode::FullOnly
                    } else {
                        AllowMode::NonFull
                    },
                    Some("registerPackageMonitorCallback"),
                    None,
                    || Ok(false),
                    |_| Ok(false),
                    || Ok(false),
                    |_| panic!("permission denied first"),
                )
                .unwrap_err();
                assert_eq!(
                    error.message,
                    format!(
                        "Permission Denial: registerPackageMonitorCallback asks to run as user -1 but is calling from uid {formatted}; this requires android.permission.INTERACT_ACROSS_USERS_FULL{}",
                        if full {
                            ""
                        } else {
                            " or android.permission.INTERACT_ACROSS_USERS"
                        },
                    )
                );
            }
        }
    }

    #[test]
    fn incoming_user_shell_checks_the_resolved_target_restrictions() {
        for target in [USER_SYSTEM, 10] {
            let user = if target == USER_SYSTEM {
                USER_CURRENT
            } else {
                target
            };
            let error = handle_incoming_user(
                SHELL_UID,
                user,
                true,
                AllowMode::FullOnly,
                None,
                None,
                || Ok(false),
                |_| Ok(true),
                || Ok(false),
                |checked| {
                    assert_eq!(checked, target);
                    Ok(true)
                },
            )
            .unwrap_err();
            assert_eq!(error.code, EX_SECURITY);
            assert_eq!(
                error.message,
                format!("Shell does not have permission to access user {target}")
            );
        }
        let error = Exception::new(-5, "restriction owner unavailable");
        assert_eq!(
            handle_incoming_user(
                SHELL_UID,
                10,
                true,
                AllowMode::FullOnly,
                None,
                None,
                || Ok(false),
                |_| Ok(true),
                || Ok(false),
                |_| Err(error.clone())
            ),
            Err(error)
        );
    }

    /// Own user, current/all/current-or-self, full vs non-full access,
    /// Recents' same-group exception and the shell's restriction, with
    /// the original's ordering (including the literal own-user return).
    #[test]
    fn incoming_user_checks_permissions_before_special_users_and_shell() {
        let resolve = |uid, user, all, full, recents, granted, restricted| {
            handle_incoming_user(
                uid,
                user,
                all,
                if full {
                    AllowMode::FullOnly
                } else {
                    AllowMode::NonFull
                },
                Some("test"),
                None,
                || Ok(recents),
                |p| {
                    Ok(match granted {
                        2 => p.ends_with("INTERACT_ACROSS_USERS_FULL"),
                        1 => p.ends_with("INTERACT_ACROSS_USERS"),
                        _ => false,
                    })
                },
                || Ok(false),
                |_| Ok(restricted),
            )
        };
        assert_eq!(resolve(10_123, 0, false, true, false, 0, false), Ok(0));
        assert_eq!(resolve(1_010_123, 10, false, true, false, 0, false), Ok(10));
        for uid in [ROOT_UID, SYSTEM_UID as i32] {
            assert_eq!(
                resolve(uid, USER_ALL, true, true, false, 0, false),
                Ok(USER_ALL)
            );
            for user in [USER_CURRENT, USER_CURRENT_OR_SELF] {
                assert_eq!(resolve(uid, user, true, true, false, 0, false), Ok(0));
            }
        }
        assert_eq!(
            resolve(10_123, USER_ALL, true, true, false, 1, false)
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        assert_eq!(
            resolve(10_123, USER_ALL, true, false, false, 1, false),
            Ok(USER_ALL)
        );
        assert_eq!(
            resolve(10_123, USER_ALL, true, true, false, 2, false),
            Ok(USER_ALL)
        );
        assert_eq!(
            resolve(10_123, USER_ALL, false, true, false, 2, false)
                .unwrap_err()
                .code,
            EX_ILLEGAL_ARGUMENT
        );
        assert_eq!(
            resolve(10_123, USER_CURRENT, true, true, false, 0, false)
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        assert_eq!(
            resolve(10_123, USER_CURRENT, true, true, true, 0, false),
            Ok(0)
        );
        assert_eq!(
            resolve(1_010_123, USER_CURRENT, true, true, true, 0, false)
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        assert_eq!(
            resolve(1_010_123, USER_CURRENT_OR_SELF, true, true, false, 0, false),
            Ok(10)
        );
        assert_eq!(
            resolve(1_010_123, USER_CURRENT_OR_SELF, true, true, false, 2, false),
            Ok(0)
        );
        assert_eq!(resolve(SHELL_UID, 0, true, true, false, 0, true), Ok(0));
        assert_eq!(
            resolve(SHELL_UID, USER_CURRENT, true, true, false, 2, true)
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        assert_eq!(
            resolve(SHELL_UID, USER_ALL, true, true, false, 2, true),
            Ok(USER_ALL)
        );
        // Owners' failures remain failures, never a permission denial or
        // a successful CURRENT_OR_SELF fallback.
        let error = Exception::new(-5, "permission owner unavailable");
        assert_eq!(
            handle_incoming_user(
                1_010_123,
                USER_CURRENT_OR_SELF,
                true,
                AllowMode::FullOnly,
                None,
                None,
                || panic!("different profile group"),
                |_| Err(error.clone()),
                || Ok(false),
                |_| panic!("not shell")
            ),
            Err(error)
        );
    }
}
