//! URI permissions in the app host (`ActivityManagerService
//! .checkUriPermission` and `UriGrantsManagerService` at
//! `android-16.0.0_r1`, ADR 0013): the persisted grants the original keeps
//! in `/data/system/urigrants.xml`, read as `readGrantedUriPermissionsLocked`
//! reads them, `checkUriPermission` over them, and the `uri_grants`
//! service's queries of them (`getUriPermissions`,
//! `getGrantedUriPermissions`). Grants made by intents' `FLAG_GRANT_*` and
//! by `grantUriPermission`, taking and releasing persistable grants, and
//! persisting new ones, are not implemented (#1503).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_app_iurigrantsmanager as ugm;

use super::{AppHost, PER_USER_RANGE, is_isolated, reply};
use crate::package::apps_filter::{NotModelled, should_filter_application};
use crate::package::info::java_hash;
use crate::package::list_slice::Slice;
use crate::package::query::Query;
use crate::package::uri::Uri;

/// `Intent.FLAG_GRANT_*`.
const FLAG_GRANT_READ_URI_PERMISSION: i32 = 0x1;
const FLAG_GRANT_WRITE_URI_PERMISSION: i32 = 0x2;
const FLAG_GRANT_PERSISTABLE_URI_PERMISSION: i32 = 0x40;
const FLAG_GRANT_PREFIX_URI_PERMISSION: i32 = 0x80;
/// `UriPermission.STRENGTH_*`.
const STRENGTH_OWNED: i32 = 1;
const STRENGTH_PERSISTABLE: i32 = 3;
/// `PackageManager.MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE`.
const MATCH_DIRECT_BOOT_AWARE_UNAWARE: i64 = 0x0004_0000 | 0x0008_0000;
/// `Process.SYSTEM_UID`.
const SYSTEM_UID: i32 = 1000;
/// `Manifest.permission.GET_APP_GRANTED_URI_PERMISSIONS`.
const GET_APP_GRANTED_URI_PERMISSIONS: &str = "android.permission.GET_APP_GRANTED_URI_PERMISSIONS";

/// `GrantUri`: a URI in its source user, and whether it grants its
/// descendants.
#[derive(PartialEq)]
struct GrantUri {
    source_user: i32,
    uri: Uri,
    prefix: bool,
}

impl GrantUri {
    /// `GrantUri.hashCode`, which orders a uid's grants (`ArrayMap`).
    fn hash(&self) -> i32 {
        let hash = 31i32.wrapping_add(self.source_user);
        let hash = hash.wrapping_mul(31).wrapping_add(java_hash(self.uri.as_str()));
        hash.wrapping_mul(31).wrapping_add(if self.prefix { 1231 } else { 1237 })
    }
}

/// `UriPermission` of a persisted grant (`initPersistedModes`): its read
/// and write modes are persistable and persisted.
pub(super) struct Grant {
    source_package: String,
    target_package: String,
    target_uid: i32,
    uri: GrantUri,
    persisted: i32,
    created: i64,
}

impl Grant {
    /// `UriPermission.getStrength`.
    fn strength(&self, modes: i32) -> i32 {
        let modes = modes & (FLAG_GRANT_READ_URI_PERMISSION | FLAG_GRANT_WRITE_URI_PERMISSION);
        if self.persisted & modes == modes { STRENGTH_PERSISTABLE } else { 0 }
    }

    /// `buildPersistedPublicApiObject`.
    fn persisted_public(&self) -> PublicUriPermission {
        PublicUriPermission { uri: self.uri.uri.clone(), modes: self.persisted, persisted_time: self.created }
    }
}

/// `android.content.UriPermission`.
struct PublicUriPermission {
    uri: Uri,
    modes: i32,
    persisted_time: i64,
}

impl WriteParcelable for PublicUriPermission {
    fn write_to(&self, p: &mut Parcel) {
        write_uri_parcelable(p, &self.uri);
        p.write_i32(self.modes);
        p.write_i64(self.persisted_time);
    }
}

/// `android.app.GrantedUriPermission`.
struct GrantedUriPermission {
    uri: Uri,
    package: String,
}

impl WriteParcelable for GrantedUriPermission {
    fn write_to(&self, p: &mut Parcel) {
        write_uri_parcelable(p, &self.uri);
        p.write_string16(Some(&self.package));
    }
}

/// `Parcel.writeParcelable` of a `Uri`: its class, then the `Uri`.
fn write_uri_parcelable(p: &mut Parcel, uri: &Uri) {
    p.write_string16(Some(uri.class_name()));
    uri.write(p);
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
                let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
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
                    let created = e.long("createdTime").map_err(state)?.unwrap_or(now);
                    let Some(target_state) = packages.packages.get(target.as_ref()) else { continue };
                    let provider = uri.authority().map(|a| self.provider_package(&a, source_user)).transpose()?.flatten();
                    if provider.as_deref() != Some(source.as_ref()) {
                        eprintln!("app host: persisted grant for {} had source {source} but instead found {provider:?}", uri.as_str());
                        continue;
                    }
                    let target_uid = target_user * PER_USER_RANGE as i32 + target_state.app_id;
                    let uri = GrantUri { source_user, uri, prefix };
                    let persisted = modes & (FLAG_GRANT_READ_URI_PERMISSION | FLAG_GRANT_WRITE_URI_PERMISSION);
                    // `findOrCreateUriPermissionLocked`, then `initPersistedModes`.
                    match grants.iter_mut().find(|g: &&mut Grant| g.target_uid == target_uid && g.uri == uri) {
                        Some(g) => (g.persisted, g.created) = (persisted, created),
                        None => grants.push(Grant {
                            source_package: source.to_string(),
                            target_package: target.to_string(),
                            target_uid,
                            uri,
                            persisted,
                            created,
                        }),
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(state(format!("{}: {e}", path.display()))),
        }
        grants.sort_by_key(|g| (g.target_uid, g.uri.hash()));
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

/// `uri_grants`: UriGrantsManagerService's queries of the grants.
pub struct UriGrantsManager(pub Arc<AppHost>);

impl UriGrantsManager {
    /// `getUriPermissions`: the persisted grants `package`, the caller's,
    /// was given (`incoming`) or gave.
    fn uri_permissions(&self, call: &Call<'_>, package: Option<String>, incoming: bool, persisted_only: bool) -> Result<Vec<PublicUriPermission>, Exception> {
        let calling_uid = call.sender_euid as i32;
        if is_isolated(calling_uid) {
            return Err(Exception::security("Isolated process not allowed to call getUriPermissions"));
        }
        let package = package.ok_or_else(|| Exception::new(EX_NULL_POINTER, "packageName"))?;
        let user = calling_uid / PER_USER_RANGE as i32;
        let (state, resolution) = self.0.packages()?;
        let query = Query { state: &state, filter: &resolution.apps_filter, calling_uid: SYSTEM_UID };
        let package_uid = query
            .package_uid(&package, MATCH_DIRECT_BOOT_AWARE_UNAWARE, user)
            .map_err(|NotModelled(reason)| Exception::new(EX_UNSUPPORTED_OPERATION, format!("uri_grants: {reason}")))??;
        if package_uid != calling_uid {
            return Err(Exception::security(format!("Package {package} does not belong to calling UID {calling_uid}")));
        }
        let grants = self.0.uri_grants()?;
        let listed = grants.iter().filter(|g| {
            let ours = if incoming { g.target_uid == calling_uid && g.target_package == package } else { g.source_package == package };
            ours && (!persisted_only || g.persisted != 0)
        });
        let listed: Vec<_> = listed.map(Grant::persisted_public).collect();
        if incoming && !grants.iter().any(|g| g.target_uid == calling_uid) {
            eprintln!("app host: No permission grants found for {package}");
        }
        Ok(listed)
    }

    /// `getGrantedUriPermissions`: the persisted grants to `package` (to
    /// all, if none) in `user`.
    fn granted_uri_permissions(&self, call: &Call<'_>, package: Option<String>, user: i32) -> Result<Vec<GrantedUriPermission>, Exception> {
        if self.0.check_permission(Some(GET_APP_GRANTED_URI_PERMISSIONS), call.sender_euid as i32)? != 0 {
            return Err(Exception::security(format!(
                "Permission Denial: getGrantedUriPermissions from pid={}, uid={} requires {GET_APP_GRANTED_URI_PERMISSIONS}",
                call.sender_pid, call.sender_euid
            )));
        }
        let grants = self.0.uri_grants()?;
        Ok(grants
            .iter()
            .filter(|g| package.as_ref().is_none_or(|p| *p == g.target_package))
            .filter(|g| g.target_uid / PER_USER_RANGE as i32 == user && g.persisted != 0)
            .map(|g| GrantedUriPermission { uri: g.uri.uri.clone(), package: g.target_package.clone() })
            .collect())
    }
}

impl Service for UriGrantsManager {
    fn descriptor(&self) -> &str {
        ugm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match call.code {
            ugm::GET_URI_PERMISSIONS => {
                let a = ugm::GetUriPermissions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let listed = self.uri_permissions(call, a.package_name, a.incoming, a.persisted_only);
                return reply(listed.map(|listed| {
                    let slice = Slice::new(self.0.process.clone(), "android.content.UriPermission", listed);
                    ugm::write_get_uri_permissions_reply(&mut p, Some(&slice));
                    p
                }));
            }
            ugm::GET_GRANTED_URI_PERMISSIONS => {
                let a = ugm::GetGrantedUriPermissions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let listed = self.granted_uri_permissions(call, a.package_name, a.user_id);
                return reply(listed.map(|listed| {
                    let slice = Slice::new(self.0.process.clone(), "android.app.GrantedUriPermission", listed);
                    ugm::write_get_granted_uri_permissions_reply(&mut p, Some(&slice));
                    p
                }));
            }
            ugm::TAKE_PERSISTABLE_URI_PERMISSION => reply(Err(self.0.unimplemented_named("uri_grants", "takePersistableUriPermission (#1503)"))),
            ugm::RELEASE_PERSISTABLE_URI_PERMISSION => reply(Err(self.0.unimplemented_named("uri_grants", "releasePersistableUriPermission (#1503)"))),
            ugm::GRANT_URI_PERMISSION_FROM_OWNER => reply(Err(self.0.unimplemented_named("uri_grants", "grantUriPermissionFromOwner (#1503)"))),
            ugm::CLEAR_GRANTED_URI_PERMISSIONS => reply(Err(self.0.unimplemented_named("uri_grants", "clearGrantedUriPermissions (#1503)"))),
            code => reply(Err(self.0.unimplemented("uri_grants", code))),
        }
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
            source_package: "a".into(),
            target_package: "b".into(),
            target_uid: 10_000,
            uri: GrantUri { source_user: 0, uri: Uri::parse("content://a.b/x"), prefix: false },
            persisted: FLAG_GRANT_READ_URI_PERMISSION,
            created: 0,
        };
        assert_eq!(grant.strength(FLAG_GRANT_READ_URI_PERMISSION), STRENGTH_PERSISTABLE);
        assert_eq!(grant.strength(FLAG_GRANT_READ_URI_PERMISSION | FLAG_GRANT_WRITE_URI_PERMISSION), 0);
    }

    #[test]
    fn a_grant_uri_hashes_as_java_does() {
        let uri = Uri::parse("content://com.android.externalstorage.documents/tree/primary%3AAlarms");
        assert_eq!(GrantUri { source_user: 0, uri, prefix: false }.hash(), 1780322610);
    }

    #[test]
    fn a_uri_permission_parcels_as_the_original() {
        let permission = PublicUriPermission { uri: Uri::parse("content://a.b/x"), modes: FLAG_GRANT_READ_URI_PERMISSION, persisted_time: 7 };
        let mut p = Parcel::new();
        permission.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("android.net.Uri$StringUri"));
        assert_eq!(r.read_i32().unwrap(), 1); // `Uri.StringUri.TYPE_ID`
        assert_eq!(r.read_string8().unwrap().as_deref(), Some("content://a.b/x"));
        assert_eq!(r.read_i32().unwrap(), FLAG_GRANT_READ_URI_PERMISSION);
        assert_eq!(r.read_i64().unwrap(), 7);
    }
}
