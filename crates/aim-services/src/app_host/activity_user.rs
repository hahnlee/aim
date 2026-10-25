//! `UserController.handleIncomingUser` (`android-16.0.0_r1`), with
//! `ALLOW_FULL_ONLY` or `ALLOW_NON_FULL`, for this host's single current
//! user (0), with no other user in its profile group.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::parcel::Exception;

pub(super) fn handle_incoming_user(
    uid: i32,
    user: i32,
    allow_all: bool,
    require_full: bool,
    name: Option<&str>,
    recents: impl FnOnce() -> Result<bool, Exception>,
    permission: impl Fn(&str) -> Result<bool, Exception>,
    shell_restricted: impl FnOnce() -> Result<bool, Exception>,
) -> Result<i32, Exception> {
    let calling_user = uid / super::PER_USER_RANGE as i32;
    if calling_user == user {
        return Ok(user);
    }
    // `unsafeConvertIncomingUser`.
    let mut target = if user == -2 || user == -3 { 0 } else { user };
    if uid != 0 && uid != 1000 {
        let allowed = calling_user == target && recents()?
            || permission("android.permission.INTERACT_ACROSS_USERS_FULL")?
            || !require_full && permission("android.permission.INTERACT_ACROSS_USERS")?;
        if !allowed {
            if user == -3 {
                target = calling_user;
            } else {
                return Err(Exception::security(format!(
                    "Permission Denial: {} asks to run as user {user} but is calling from uid {uid}; this requires android.permission.INTERACT_ACROSS_USERS_FULL{}",
                    name.unwrap_or("null"),
                    if require_full {
                        ""
                    } else {
                        " or android.permission.INTERACT_ACROSS_USERS"
                    },
                )));
            }
        }
    }
    if !allow_all && target < 0 {
        return Err(Exception::illegal_argument(format!(
            "Call does not support special user #{target}"
        )));
    }
    if uid == 2000 && target == 0 && shell_restricted()? {
        return Err(Exception::security(format!(
            "Shell does not have permission to access user {target}"
        )));
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_SECURITY};

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
                || Ok(restricted),
            )
        };
        assert_eq!(resolve(10_123, 0, false, true, false, 0, false), Ok(0));
        assert_eq!(resolve(1_010_123, 10, false, true, false, 0, false), Ok(10));
        for uid in [0, 1000] {
            assert_eq!(resolve(uid, -1, true, true, false, 0, false), Ok(-1));
            for user in [-2, -3] {
                assert_eq!(resolve(uid, user, true, true, false, 0, false), Ok(0));
            }
        }
        assert_eq!(
            resolve(10_123, -1, true, true, false, 1, false)
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        assert_eq!(resolve(10_123, -1, true, false, false, 1, false), Ok(-1));
        assert_eq!(resolve(10_123, -1, true, true, false, 2, false), Ok(-1));
        assert_eq!(
            resolve(10_123, -1, false, true, false, 2, false)
                .unwrap_err()
                .code,
            EX_ILLEGAL_ARGUMENT
        );
        assert_eq!(
            resolve(10_123, -2, true, true, false, 0, false)
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        assert_eq!(resolve(10_123, -2, true, true, true, 0, false), Ok(0));
        assert_eq!(
            resolve(1_010_123, -2, true, true, true, 0, false)
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        assert_eq!(resolve(1_010_123, -3, true, true, false, 0, false), Ok(10));
        assert_eq!(resolve(1_010_123, -3, true, true, false, 2, false), Ok(0));
        assert_eq!(resolve(2000, 0, true, true, false, 0, true), Ok(0));
        assert_eq!(
            resolve(2000, -2, true, true, false, 2, true)
                .unwrap_err()
                .code,
            EX_SECURITY
        );
        assert_eq!(resolve(2000, -1, true, true, false, 2, true), Ok(-1));
        // Owners' failures remain failures, never a permission denial or
        // a successful CURRENT_OR_SELF fallback.
        let error = Exception::new(-5, "permission owner unavailable");
        assert_eq!(
            handle_incoming_user(
                1_010_123,
                -3,
                true,
                true,
                None,
                || panic!("different profile group"),
                |_| Err(error.clone()),
                || panic!("not shell")
            ),
            Err(error)
        );
    }
}
