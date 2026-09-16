//! Pure Android ownership-change authorization.
//!
//! This module deliberately has no connection to the host credential APIs or
//! to a file descriptor.  The caller must supply the trusted, already-resolved
//! Android credentials and inode metadata.  Persistence and filesystem errors
//! are outside this policy boundary.

/// Linux capability number for changing an inode owner or group.
pub const CAP_CHOWN: u32 = 0;
/// Linux capability number used by chmod/setattr when preserving S_ISGID.
///
/// The modern Android common kernel's chown path asks
/// `setattr_should_drop_sgid`; this capability can preserve a non-executable
/// S_ISGID bit when the caller is not in the inode's group.
pub const CAP_FSETID: u32 = 4;

/// The `-1` uid/gid value means “leave this field unchanged” in chown APIs.
pub const UNCHANGED: u32 = u32::MAX;

const CAPABILITY_BIT_WIDTH: u32 = u64::BITS;
const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_ISUID: u32 = 0o4000;
const S_ISGID: u32 = 0o2000;
const S_IXGRP: u32 = 0o0010;

/// Credentials supplied by the trusted Android process snapshot.
///
/// `fsuid`/`fsgid` are the filesystem credentials, not host uid/gid values.
/// The supplementary-group slice must remain owned by that trusted snapshot
/// for the duration of the call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrustedCredentials<'groups> {
    pub fsuid: u32,
    pub fsgid: u32,
    pub supplementary_groups: &'groups [u32],
    pub effective_capabilities: u64,
}

impl TrustedCredentials<'_> {
    /// Whether this snapshot has the requested effective capability.
    pub const fn has_effective_capability(self, capability: u32) -> bool {
        capability < CAPABILITY_BIT_WIDTH
            && (self.effective_capabilities & (1_u64 << capability)) != 0
    }

    fn is_in_group(self, gid: u32) -> bool {
        self.fsgid == gid || self.supplementary_groups.contains(&gid)
    }
}

/// The current Android uid, gid and Linux mode bits of the inode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InodeOwnership {
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
}

/// Requested owner/group.  [`UNCHANGED`] leaves the corresponding field as-is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OwnershipRequest {
    pub uid: u32,
    pub gid: u32,
}

/// The metadata that an authorized chown operation may commit.
pub type AuthorizedOwnership = InodeOwnership;

/// Policy denial.  Filesystem-specific failures (read-only, immutable,
/// invalid namespace mapping, persistence failure, and so on) are not policy
/// results and must be reported by the caller separately.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnershipPolicyError {
    /// The operation would return Linux `-EPERM`.
    PermissionDenied,
}

impl OwnershipPolicyError {
    pub const fn errno(self) -> i32 {
        1 // EPERM
    }
}

/// Authorize a Linux/AOSP-style owner/group change and compute its resulting
/// uid, gid and mode.
///
/// Without `CAP_CHOWN`, Linux permits an inode owner to retain its uid (but
/// not change it to another uid) and to select only its current inode gid, its
/// fsgid, or a supplementary group.  The kernel chown path marks regular
/// files with `ATTR_KILL_SUID` and asks `setattr_should_drop_sgid`: S_ISUID is
/// unconditional, executable S_ISGID is unconditional, and non-executable
/// S_ISGID needs either membership in the inode's current group or
/// CAP_FSETID.
pub fn authorize(
    credentials: TrustedCredentials<'_>,
    current: InodeOwnership,
    request: OwnershipRequest,
) -> Result<AuthorizedOwnership, OwnershipPolicyError> {
    if credentials.has_effective_capability(CAP_CHOWN) {
        return Ok(resulting_ownership(credentials, current, request));
    }

    // chown_ok(): an unprivileged owner may not select another uid.  If the
    // uid argument is unchanged, there is no ATTR_UID check in the kernel.
    if request.uid != UNCHANGED && (credentials.fsuid != current.uid || request.uid != current.uid)
    {
        return Err(OwnershipPolicyError::PermissionDenied);
    }

    // chgrp_ok(): an unprivileged owner may select its current inode group or
    // any group in its fsgid/supplementary-group set.
    if request.gid != UNCHANGED
        && (credentials.fsuid != current.uid
            || (request.gid != current.gid && !credentials.is_in_group(request.gid)))
    {
        return Err(OwnershipPolicyError::PermissionDenied);
    }

    Ok(resulting_ownership(credentials, current, request))
}

fn resulting_ownership(
    credentials: TrustedCredentials<'_>,
    current: InodeOwnership,
    request: OwnershipRequest,
) -> AuthorizedOwnership {
    let mut mode = current.mode;
    // chown_common sets ATTR_KILL_SUID and evaluates setattr_should_drop_sgid
    // for every regular-file invocation, including a request whose two fields
    // are UNCHANGED.  Keep this separate from the uid/gid result:
    // `chown(path, -1, -1)` can leave ownership untouched while still
    // clearing applicable set-ID bits.
    if (mode & S_IFMT) != S_IFDIR {
        // notify_change handles ATTR_KILL_SUID first and always removes S_ISUID.
        mode &= !S_ISUID;

        // The kernel always removes executable S_ISGID.  For a
        // non-executable S_ISGID, setattr_should_drop_sgid retains it when
        // the caller belongs to the inode's current group or has CAP_FSETID.
        let in_inode_group = credentials.is_in_group(current.gid);
        let has_fsetid = credentials.has_effective_capability(CAP_FSETID);
        if (mode & S_ISGID) != 0 && ((mode & S_IXGRP) != 0 || (!in_inode_group && !has_fsetid)) {
            mode &= !S_ISGID;
        }
    }

    AuthorizedOwnership {
        uid: if request.uid == UNCHANGED {
            current.uid
        } else {
            request.uid
        },
        gid: if request.gid == UNCHANGED {
            current.gid
        } else {
            request.gid
        },
        mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials(
        fsuid: u32,
        fsgid: u32,
        groups: &'static [u32],
        effective_capabilities: u64,
    ) -> TrustedCredentials<'static> {
        TrustedCredentials {
            fsuid,
            fsgid,
            supplementary_groups: groups,
            effective_capabilities,
        }
    }

    fn file() -> InodeOwnership {
        InodeOwnership {
            uid: 1000,
            gid: 1000,
            mode: 0o100750 | S_ISUID | S_ISGID,
        }
    }

    #[test]
    fn owner_can_keep_uid_and_change_to_owned_group() {
        let result = authorize(
            credentials(1000, 1000, &[2000], 0),
            file(),
            OwnershipRequest {
                uid: 1000,
                gid: 2000,
            },
        )
        .unwrap();
        assert_eq!(result.uid, 1000);
        assert_eq!(result.gid, 2000);
        assert_eq!(result.mode, 0o100750);
    }

    #[test]
    fn owner_can_restate_current_inode_group_without_group_membership() {
        let current = InodeOwnership {
            mode: 0o100640,
            ..file()
        };
        let result = authorize(
            credentials(1000, 3000, &[], 0),
            current,
            OwnershipRequest {
                uid: UNCHANGED,
                gid: current.gid,
            },
        )
        .unwrap();
        assert_eq!(result, current);
    }

    #[test]
    fn owner_cannot_change_uid_or_choose_unowned_group() {
        let current = file();
        assert_eq!(
            authorize(
                credentials(1000, 1000, &[], 0),
                current,
                OwnershipRequest {
                    uid: 2000,
                    gid: UNCHANGED
                },
            ),
            Err(OwnershipPolicyError::PermissionDenied)
        );
        assert_eq!(
            authorize(
                credentials(1000, 1000, &[], 0),
                current,
                OwnershipRequest {
                    uid: UNCHANGED,
                    gid: 2000
                },
            ),
            Err(OwnershipPolicyError::PermissionDenied)
        );
    }

    #[test]
    fn non_owner_cannot_change_any_selected_field() {
        assert_eq!(
            authorize(
                credentials(2000, 1000, &[2000], 0),
                file(),
                OwnershipRequest {
                    uid: UNCHANGED,
                    gid: 1000
                },
            ),
            Err(OwnershipPolicyError::PermissionDenied)
        );
    }

    #[test]
    fn cap_chown_allows_arbitrary_ids_but_clears_regular_setid() {
        let result = authorize(
            credentials(2000, 3000, &[], 1_u64 << CAP_CHOWN),
            file(),
            OwnershipRequest {
                uid: 4000,
                gid: 5000,
            },
        )
        .unwrap();
        assert_eq!(
            result,
            InodeOwnership {
                uid: 4000,
                gid: 5000,
                mode: 0o100750
            }
        );
    }

    #[test]
    fn fsetid_does_not_preserve_setid_on_chown() {
        let result = authorize(
            credentials(
                2000,
                3000,
                &[],
                (1_u64 << CAP_CHOWN) | (1_u64 << CAP_FSETID),
            ),
            file(),
            OwnershipRequest {
                uid: 4000,
                gid: UNCHANGED,
            },
        )
        .unwrap();
        assert_eq!(result.mode, 0o100750);
    }

    #[test]
    fn directory_setid_bits_survive_ownership_change() {
        let current = InodeOwnership {
            mode: S_IFDIR | 0o750 | S_ISUID | S_ISGID,
            ..file()
        };
        let result = authorize(
            credentials(2000, 3000, &[], 1_u64 << CAP_CHOWN),
            current,
            OwnershipRequest {
                uid: 4000,
                gid: 5000,
            },
        )
        .unwrap();
        assert_eq!(result.mode, current.mode);
    }

    #[test]
    fn non_executable_setgid_is_cleared_when_caller_is_not_in_inode_group() {
        let current = InodeOwnership {
            mode: 0o100640 | S_ISGID,
            ..file()
        };
        let result = authorize(
            credentials(2000, 3000, &[], 1_u64 << CAP_CHOWN),
            current,
            OwnershipRequest {
                uid: 4000,
                gid: UNCHANGED,
            },
        )
        .unwrap();
        assert_eq!(result.mode, 0o100640);
    }

    #[test]
    fn owner_inode_group_match_preserves_non_executable_setgid() {
        let current = InodeOwnership {
            mode: 0o100640 | S_ISGID,
            ..file()
        };
        let result = authorize(
            credentials(1000, 1000, &[], 0),
            current,
            OwnershipRequest {
                uid: UNCHANGED,
                gid: UNCHANGED,
            },
        )
        .unwrap();
        assert_eq!(result.mode, current.mode);
    }

    #[test]
    fn cap_fsetid_preserves_non_executable_setgid() {
        let current = InodeOwnership {
            mode: 0o100640 | S_ISGID,
            ..file()
        };
        let result = authorize(
            credentials(2000, 3000, &[], 1_u64 << CAP_FSETID),
            current,
            OwnershipRequest {
                uid: UNCHANGED,
                gid: UNCHANGED,
            },
        )
        .unwrap();
        assert_eq!(result.mode, current.mode);
    }

    #[test]
    fn unchanged_request_still_clears_regular_setid() {
        let current = file();
        let result = authorize(
            credentials(2000, 3000, &[], 0),
            current,
            OwnershipRequest {
                uid: UNCHANGED,
                gid: UNCHANGED,
            },
        )
        .unwrap();
        assert_eq!(result.uid, current.uid);
        assert_eq!(result.gid, current.gid);
        assert_eq!(result.mode, 0o100750);
    }

    #[test]
    fn capability_queries_are_bounded() {
        let creds = credentials(0, 0, &[], 1_u64 << CAP_FSETID);
        assert!(creds.has_effective_capability(CAP_FSETID));
        assert!(!creds.has_effective_capability(64));
    }
}
