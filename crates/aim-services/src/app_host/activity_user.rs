//! `UserController.handleIncomingUser` (`android-16.0.0_r1`), with
//! `ALLOW_FULL_ONLY` or `ALLOW_NON_FULL`, for this host's single current
//! user (0), with no other user in its profile group.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::parcel::Exception;

use super::activity::USER_CURRENT;
use super::user_service::SYSTEM_USER as USER_SYSTEM;
use super::{FIRST_APPLICATION_UID, FIRST_ISOLATED_UID, PER_USER_RANGE, SYSTEM_UID};
use crate::package::apps_filter::{ROOT_UID, SHELL_UID};

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
    require_full: bool,
    name: Option<&str>,
    recents: impl FnOnce() -> Result<bool, Exception>,
    permission: impl Fn(&str) -> Result<bool, Exception>,
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
        let allowed = calling_user == target && recents()?
            || permission("android.permission.INTERACT_ACROSS_USERS_FULL")?
            || !require_full && permission("android.permission.INTERACT_ACROSS_USERS")?;
        if !allowed {
            if user == USER_CURRENT_OR_SELF {
                target = calling_user;
            } else {
                return Err(Exception::security(format!(
                    "Permission Denial: {} asks to run as user {user} but is calling from uid {}; this requires android.permission.INTERACT_ACROSS_USERS_FULL{}",
                    name.unwrap_or("null"),
                    format_uid(uid),
                    if require_full {
                        ""
                    } else {
                        " or android.permission.INTERACT_ACROSS_USERS"
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
                    full,
                    Some("registerPackageMonitorCallback"),
                    || Ok(false),
                    |_| Ok(false),
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
                true,
                None,
                || Ok(false),
                |_| Ok(true),
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
                true,
                None,
                || Ok(false),
                |_| Ok(true),
                |_| Err(error.clone()),
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
                full,
                Some("test"),
                || Ok(recents),
                |p| {
                    Ok(match granted {
                        2 => p.ends_with("INTERACT_ACROSS_USERS_FULL"),
                        1 => p.ends_with("INTERACT_ACROSS_USERS"),
                        _ => false,
                    })
                },
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
                true,
                None,
                || panic!("different profile group"),
                |_| Err(error.clone()),
                |_| panic!("not shell")
            ),
            Err(error)
        );
    }
}
