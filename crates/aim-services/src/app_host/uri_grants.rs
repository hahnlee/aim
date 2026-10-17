//! URI permissions in the app host (`ActivityManagerService
//! .checkUriPermission` and `UriGrantsManagerService` at
//! `android-16.0.0_r1`, ADR 0013): the persisted grants the original keeps
//! in `/data/system/urigrants.xml`, read as `readGrantedUriPermissionsLocked`
//! reads them, and `checkUriPermission` over them. Grants made by intents'
//! `FLAG_GRANT_*` and by `grantUriPermission`, and persisting new ones, are
//! not implemented (#1503).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::local::Call;
use aim_binder_host::parcel::{EX_ILLEGAL_STATE, Exception};

use super::{AppHost, PER_USER_RANGE, is_isolated};
use crate::package::apps_filter::{NotModelled, should_filter_application};
use crate::package::uri::Uri;

/// `Intent.FLAG_GRANT_*`.
const FLAG_GRANT_READ_URI_PERMISSION: i32 = 0x1;
const FLAG_GRANT_WRITE_URI_PERMISSION: i32 = 0x2;
const FLAG_GRANT_PERSISTABLE_URI_PERMISSION: i32 = 0x40;
const FLAG_GRANT_PREFIX_URI_PERMISSION: i32 = 0x80;
/// `UriPermission.STRENGTH_*`.
const STRENGTH_OWNED: i32 = 1;
const STRENGTH_PERSISTABLE: i32 = 3;

/// `GrantUri`: a URI in its source user, and whether it grants its
/// descendants.
#[derive(PartialEq)]
struct GrantUri {
    source_user: i32,
    uri: Uri,
    prefix: bool,
}

/// `UriPermission` of a persisted grant (`initPersistedModes`): its modes
/// are persistable and persisted.
pub(super) struct Grant {
    target_uid: i32,
    uri: GrantUri,
    persisted: i32,
}

impl Grant {
    /// `UriPermission.getStrength`.
    fn strength(&self, modes: i32) -> i32 {
        let modes = modes & (FLAG_GRANT_READ_URI_PERMISSION | FLAG_GRANT_WRITE_URI_PERMISSION);
        if self.persisted & modes == modes { STRENGTH_PERSISTABLE } else { 0 }
    }
}

/// `Uri.isPathPrefixMatch`.
fn is_path_prefix_match(uri: &Uri, prefix: &Uri) -> bool {
    if uri.scheme() != prefix.scheme() || uri.authority() != prefix.authority() {
        return false;
    }
    let (segments, prefix) = (uri.path_segments(), prefix.path_segments());
    segments.len() >= prefix.len() && segments.iter().zip(&prefix).all(|(a, b)| a == b)
}

impl AppHost {
    /// The persisted grants, read once (`readGrantedUriPermissionsLocked`):
    /// those whose target is installed and whose source still provides the
    /// URI's authority.
    fn uri_grants(&self) -> Result<&[Grant], Exception> {
        if let Some(grants) = self.uri_grants.get() {
            return Ok(grants);
        }
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let path = self.data()?.join("system/urigrants.xml");
        let mut grants = Vec::new();
        match std::fs::read(&path) {
            Ok(bytes) => {
                let root = aim_android_xml::read(&bytes).map_err(|e| state(format!("{}: {e}", path.display())))?;
                let (packages, _) = self.packages()?;
                for e in root.children().filter(|e| e.name == "uri-grant") {
                    let int = |name: &str| e.int(name).map_err(|err| state(format!("{}: {err}", path.display())));
                    // `userHandle`, before source and target users.
                    let handle = int("userHandle")?;
                    let source_user = handle.or(int("sourceUserId")?).unwrap_or(0);
                    let target_user = handle.or(int("targetUserId")?).unwrap_or(0);
                    let (Some(source), Some(target), Some(uri)) = (e.string("sourcePkg"), e.string("targetPkg"), e.string("uri")) else {
                        continue;
                    };
                    let uri = Uri::parse(&uri);
                    let prefix = e.bool("prefix").map_err(state)?.unwrap_or(false);
                    let modes = int("modeFlags")?.unwrap_or(0);
                    let Some(target) = packages.packages.get(target.as_ref()) else { continue };
                    let provider = uri.authority().map(|a| self.provider_package(&a, source_user)).transpose()?.flatten();
                    if provider.as_deref() != Some(source.as_ref()) {
                        eprintln!("app host: persisted grant for {} had source {source} but instead found {provider:?}", uri.as_str());
                        continue;
                    }
                    grants.push(Grant {
                        target_uid: target_user * PER_USER_RANGE as i32 + target.app_id,
                        uri: GrantUri { source_user, uri, prefix },
                        persisted: modes,
                    });
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(state(format!("{}: {e}", path.display()))),
        }
        Ok(self.uri_grants.get_or_init(|| grants))
    }

    /// `PackageManagerInternal.filterAppAccess`: whether the app of `uid`
    /// is hidden from `calling_uid` (all its shared user's packages, if it
    /// has one).
    fn filter_app_access(&self, uid: i32, calling_uid: i32) -> Result<bool, Exception> {
        let (state, resolution) = self.packages()?;
        let user = uid / PER_USER_RANGE as i32;
        let app_id = uid.rem_euclid(PER_USER_RANGE as i32);
        let mut packages = state.packages.values().filter(|ps| ps.app_id == app_id).peekable();
        if packages.peek().is_none() {
            return Ok(true);
        }
        for ps in packages {
            match should_filter_application(&state, &resolution.apps_filter, Some(ps), calling_uid, user, true, true) {
                Ok(true) => {}
                Ok(false) => return Ok(false),
                Err(NotModelled(reason)) => return Err(Exception::new(EX_ILLEGAL_STATE, format!("filterAppAccess: {reason}"))),
            }
        }
        Ok(true)
    }

    /// `checkUriPermission` of `uri` in `user` for `uid` with `modes`
    /// (`checkUriPermissionLocked`): granted to root, else by a grant to
    /// the uid of the URI or of a prefix of it.
    pub(super) fn check_uri_permission(&self, call: &Call<'_>, uri: Uri, uid: i32, modes: i32, user: i32) -> Result<i32, Exception> {
        const GRANTED: i32 = 0;
        const DENIED: i32 = -1;
        if is_isolated(call.sender_euid as i32) {
            return Err(Exception::security("Isolated process not allowed to call checkUriPermission"));
        }
        if uid != 0 && self.filter_app_access(uid, call.sender_euid as i32)? {
            return Ok(DENIED);
        }
        if uid == 0 {
            return Ok(GRANTED);
        }
        let min = if modes & FLAG_GRANT_PERSISTABLE_URI_PERMISSION != 0 { STRENGTH_PERSISTABLE } else { STRENGTH_OWNED };
        let grant = GrantUri { source_user: user, uri, prefix: modes & FLAG_GRANT_PREFIX_URI_PERMISSION != 0 };
        let granted = self.uri_grants()?.iter().filter(|g| g.target_uid == uid).any(|g| {
            g.strength(modes) >= min && (g.uri == grant || g.uri.prefix && is_path_prefix_match(&grant.uri, &g.uri.uri))
        });
        Ok(if granted { GRANTED } else { DENIED })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefix_matches_its_descendants_only() {
        let prefix = Uri::parse("content://a.b/x/y");
        assert!(is_path_prefix_match(&Uri::parse("content://a.b/x/y/z"), &prefix));
        assert!(is_path_prefix_match(&Uri::parse("content://a.b/x/y"), &prefix));
        assert!(!is_path_prefix_match(&Uri::parse("content://a.b/x/yz"), &prefix));
        assert!(!is_path_prefix_match(&Uri::parse("content://a.c/x/y/z"), &prefix));
    }

    #[test]
    fn a_persisted_grant_is_as_strong_as_its_modes() {
        let grant = Grant {
            target_uid: 10_000,
            uri: GrantUri { source_user: 0, uri: Uri::parse("content://a.b/x"), prefix: false },
            persisted: FLAG_GRANT_READ_URI_PERMISSION,
        };
        assert_eq!(grant.strength(FLAG_GRANT_READ_URI_PERMISSION), STRENGTH_PERSISTABLE);
        assert_eq!(grant.strength(FLAG_GRANT_READ_URI_PERMISSION | FLAG_GRANT_WRITE_URI_PERMISSION), 0);
    }
}
