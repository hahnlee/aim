//! URI permissions in the app host (`UriGrantsManagerService` and its
//! `ActivityManagerService` entry points at `android-16.0.0_r1`, ADR 0013):
//! the grants apps hold, by target uid (`mGrantedUriPermissions`), each
//! with its modes owned by an activity or a service start, granted with no
//! owner, persistable or persisted (`UriPermission`). `grantUriPermission`
//! makes them, as do the intents that start activities and services and
//! deliver new intents and results (`FLAG_GRANT_*` over the data and the
//! ClipData's URIs and intents), checked as `checkGrantUriPermissionUnlocked`
//! checks them (the provider's permissions and path permissions,
//! `grantUriPermissions` and its patterns, the caller's own access); an
//! activity's or a start's go with it (`UriPermissionOwner`).
//! Broadcasts carry none, as in the original. `revokeUriPermission`,
//! `takePersistableUriPermission`, `releasePersistableUriPermission` and
//! `clearGrantedUriPermissions` change them; a provider's process is
//! asked when the provider forces URI permissions
//! (`checkContentProviderUriPermission`). The persisted grants are read
//! from `/data/system/urigrants.xml` as `readGrantedUriPermissionsLocked`
//! reads it and written back as `writeGrantedUriPermissions` writes it
//! (binary XML), when they change rather than after the original's delay.
//! The app host hands out no owner tokens (`newUriPermissionOwner` is the
//! system's own), so `grantUriPermissionFromOwner` knows none. Not yet:
//! package visibility from grants (`grantImplicitAccess`, #1569), a target
//! activity's `requireContentUriPermissionFromCaller` (#1568), a disabled
//! package's grants (#1570); the app host removes no package or user.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use aim_android_xml::{Element, Node, Value};
use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception, Parcel, Reader,
    Result as ParcelResult,
};
use aim_service_aidl::android_app_iurigrantsmanager as ugm;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::content_service::ParcelUri;
use super::kept::RawIntent;
use super::{AppHost, GET_URI_PERMISSION_PATTERNS, MATCH_DIRECT_BOOT_AUTO, PER_USER_RANGE, is_isolated, reply};
use crate::clip::{self, ClipDescription};
use crate::package::apps_filter::{NotModelled, should_filter_application};
use crate::package::info::{ProviderInfo, java_hash};
use crate::package::intent_filter::Plain;
use crate::package::list_slice::Slice;
use crate::package::query::Query;
use crate::package::uri::{self, Uri, parse_int};

/// `Intent.FLAG_GRANT_*`.
const FLAG_GRANT_READ_URI_PERMISSION: i32 = 0x1;
const FLAG_GRANT_WRITE_URI_PERMISSION: i32 = 0x2;
const FLAG_GRANT_PERSISTABLE_URI_PERMISSION: i32 = 0x40;
const FLAG_GRANT_PREFIX_URI_PERMISSION: i32 = 0x80;
/// The access modes (`Intent.isAccessUriMode`).
const ACCESS_MODES: i32 = FLAG_GRANT_READ_URI_PERMISSION | FLAG_GRANT_WRITE_URI_PERMISSION;
/// `UriPermission.STRENGTH_*`.
const STRENGTH_NONE: i32 = 0;
const STRENGTH_OWNED: i32 = 1;
const STRENGTH_GLOBAL: i32 = 2;
const STRENGTH_PERSISTABLE: i32 = 3;
/// `UriPermission.INVALID_TIME`.
const INVALID_TIME: i64 = i64::MIN;
/// `UriGrantsManagerService.MAX_PERSISTED_URI_GRANTS`.
const MAX_PERSISTED_URI_GRANTS: usize = 512;
/// `PackageManager.MATCH_DIRECT_BOOT_AWARE | MATCH_DIRECT_BOOT_UNAWARE`,
/// `MATCH_UNINSTALLED_PACKAGES`, `MATCH_DEBUG_TRIAGED_MISSING`.
const MATCH_DIRECT_BOOT_AWARE_UNAWARE: i64 = 0x0004_0000 | 0x0008_0000;
const MATCH_UNINSTALLED_PACKAGES: i64 = 0x2000;
const MATCH_DEBUG_TRIAGED_MISSING: i64 = 0x1000_0000;
/// `Process.ROOT_UID`, `SYSTEM_UID`, `INVALID_UID`, `INVALID_PID`.
const ROOT_UID: i32 = 0;
const SYSTEM_UID: i32 = 1000;
const INVALID_UID: i32 = -1;
const INVALID_PID: i32 = -1;
/// `UserHandle.USER_ALL`, `USER_CURRENT`, `USER_NULL`.
const USER_ALL: i32 = -1;
const USER_CURRENT: i32 = -2;
const USER_NULL: i32 = -10000;
/// `ContentResolver.SCHEME_CONTENT`.
const SCHEME_CONTENT: &str = "content";
const GET_APP_GRANTED_URI_PERMISSIONS: &str = "android.permission.GET_APP_GRANTED_URI_PERMISSIONS";
const CLEAR_APP_GRANTED_URI_PERMISSIONS: &str = "android.permission.CLEAR_APP_GRANTED_URI_PERMISSIONS";
const FORCE_PERSISTABLE_URI_PERMISSIONS: &str = "android.permission.FORCE_PERSISTABLE_URI_PERMISSIONS";
const INTERACT_ACROSS_USERS: &str = "android.permission.INTERACT_ACROSS_USERS";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
const MANAGE_DOCUMENTS: &str = "android.permission.MANAGE_DOCUMENTS";
/// `IContentProvider.CHECK_URI_PERMISSION_TRANSACTION`.
const PROVIDER_DESCRIPTOR: &str = "android.content.IContentProvider";
const CHECK_URI_PERMISSION_TRANSACTION: u32 = 28;
/// The system's package, whose `AttributionSource` asks a provider.
const SYSTEM_PACKAGE: &str = "android";

/// `UriPermissionOwner`: what holds the modes granted for it, which go
/// with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Owner {
    /// An activity's (`ActivityRecord.getUriPermissionsLocked`), by its
    /// token.
    Activity(u64),
    /// A service start's (`ServiceRecord.StartItem.getUriPermissionsLocked`).
    Start(u64),
}

/// `GrantUri`: a URI in its source user, and whether it grants its
/// descendants.
#[derive(Clone, Debug)]
struct GrantUri {
    source_user: i32,
    uri: Uri,
    prefix: bool,
}

impl PartialEq for GrantUri {
    /// `GrantUri.equals`, of the URIs' strings (`Uri.equals`).
    fn eq(&self, other: &GrantUri) -> bool {
        self.uri.as_str() == other.uri.as_str() && self.source_user == other.source_user && self.prefix == other.prefix
    }
}

impl std::fmt::Display for GrantUri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} [user {}]{}", self.uri.as_str(), self.source_user, if self.prefix { " [prefix]" } else { "" })
    }
}

impl GrantUri {
    fn new(source_user: i32, uri: Uri, modes: i32) -> GrantUri {
        GrantUri { source_user, uri, prefix: modes & FLAG_GRANT_PREFIX_URI_PERMISSION != 0 }
    }

    /// `GrantUri.hashCode`, which orders a uid's grants (`ArrayMap`).
    fn hash(&self) -> i32 {
        let hash = 31i32.wrapping_add(self.source_user);
        let hash = hash.wrapping_mul(31).wrapping_add(java_hash(self.uri.as_str()));
        hash.wrapping_mul(31).wrapping_add(if self.prefix { 1231 } else { 1237 })
    }

    /// `GrantUri.resolve`: a content URI in the user its authority names
    /// (`ContentProvider.getUserIdFromUri`), without it
    /// (`getUriWithoutUserId`); another in `default_user`.
    fn resolve(default_user: i32, uri: Uri, modes: i32) -> GrantUri {
        if uri.scheme() != Some(SCHEME_CONTENT) {
            return GrantUri::new(default_user, uri, modes);
        }
        let authority = uri.authority();
        let user = match authority.as_deref().and_then(|a| a.rfind('@').map(|end| &a[..end])) {
            Some(user) => parse_int(user).unwrap_or(USER_NULL),
            None => default_user,
        };
        let without = authority.as_deref().map(|a| uri::encode(&a[a.rfind('@').map_or(0, |end| end + 1)..]));
        GrantUri::new(user, uri.with_encoded_authority(without.as_deref()), modes)
    }
}

/// `ContentProvider.maybeAddUserId`: a content URI with no user names
/// `user` (unless it is `USER_CURRENT`).
fn maybe_add_user_id(uri: &Uri, user: i32) -> Uri {
    if user == USER_CURRENT || uri.scheme() != Some(SCHEME_CONTENT) || uri.user_info().is_some_and(|u| !u.is_empty()) {
        return uri.clone();
    }
    let authority = uri.authority().map_or("null".to_string(), |a| uri::encode(&a));
    uri.with_encoded_authority(Some(&format!("{user}@{authority}")))
}

/// `UriPermission`: what a target uid may do with a URI, and why.
struct UriPermission {
    source_package: String,
    target_package: String,
    target_uid: i32,
    uri: GrantUri,
    /// The modes granted with no owner, offered for persisting, and
    /// persisted (`globalModeFlags`, `persistableModeFlags`,
    /// `persistedModeFlags`).
    global: i32,
    persistable: i32,
    persisted: i32,
    /// When `persisted` was set (`persistedCreateTime`).
    persisted_time: i64,
    /// Who holds its read and its write mode (`mReadOwners`,
    /// `mWriteOwners`), which make `ownedModeFlags`.
    read_owners: Vec<Owner>,
    write_owners: Vec<Owner>,
}

impl UriPermission {
    fn new(source_package: &str, target_package: &str, target_uid: i32, uri: GrantUri) -> UriPermission {
        UriPermission {
            source_package: source_package.to_string(),
            target_package: target_package.to_string(),
            target_uid,
            uri,
            global: 0,
            persistable: 0,
            persisted: 0,
            persisted_time: INVALID_TIME,
            read_owners: Vec::new(),
            write_owners: Vec::new(),
        }
    }

    fn target_user(&self) -> i32 {
        self.target_uid / PER_USER_RANGE as i32
    }

    /// `ownedModeFlags`.
    fn owned(&self) -> i32 {
        let read = if self.read_owners.is_empty() { 0 } else { FLAG_GRANT_READ_URI_PERMISSION };
        read | if self.write_owners.is_empty() { 0 } else { FLAG_GRANT_WRITE_URI_PERMISSION }
    }

    /// `modeFlags`.
    fn modes(&self) -> i32 {
        self.owned() | self.global | self.persisted
    }

    /// `initPersistedModes`, of a grant read from the file.
    fn init_persisted_modes(&mut self, modes: i32, created: i64) {
        self.persistable = modes & ACCESS_MODES;
        self.persisted = modes & ACCESS_MODES;
        self.persisted_time = created;
    }

    /// `grantModes`: `modes` granted, owned by `owner` or by nobody, and
    /// persistable with `FLAG_GRANT_PERSISTABLE_URI_PERMISSION`.
    fn grant_modes(&mut self, modes: i32, owner: Option<Owner>) {
        if modes & FLAG_GRANT_PERSISTABLE_URI_PERMISSION != 0 {
            self.persistable |= modes & ACCESS_MODES;
        }
        match owner {
            None => self.global |= modes & ACCESS_MODES,
            Some(owner) => {
                for (flag, owners) in [(FLAG_GRANT_READ_URI_PERMISSION, &mut self.read_owners), (FLAG_GRANT_WRITE_URI_PERMISSION, &mut self.write_owners)] {
                    if modes & flag != 0 && !owners.contains(&owner) {
                        owners.push(owner);
                    }
                }
            }
        }
    }

    /// `takePersistableModes` at `now`: whether what persists changed.
    fn take_persistable_modes(&mut self, modes: i32, now: i64) -> bool {
        let modes = modes & ACCESS_MODES;
        if modes & self.persistable != modes {
            eprintln!("app host: Requested flags 0x{modes:x}, but only 0x{:x} are allowed", self.persistable);
            return false;
        }
        let before = self.persisted;
        self.persisted |= self.persistable & modes;
        if self.persisted != 0 {
            self.persisted_time = now;
        }
        self.persisted != before
    }

    /// `releasePersistableModes`: whether what persists changed.
    fn release_persistable_modes(&mut self, modes: i32) -> bool {
        let before = self.persisted;
        self.persisted &= !(modes & ACCESS_MODES);
        if self.persisted == 0 {
            self.persisted_time = INVALID_TIME;
        }
        self.persisted != before
    }

    /// `revokeModes`: `modes` revoked, persisted ones with
    /// `FLAG_GRANT_PERSISTABLE_URI_PERMISSION`, owned ones with
    /// `including_owners`; whether what persists changed.
    fn revoke_modes(&mut self, modes: i32, including_owners: bool) -> bool {
        let persistable = modes & FLAG_GRANT_PERSISTABLE_URI_PERMISSION != 0;
        let before = self.persisted;
        for (flag, owners) in [(FLAG_GRANT_READ_URI_PERMISSION, &mut self.read_owners), (FLAG_GRANT_WRITE_URI_PERMISSION, &mut self.write_owners)] {
            if modes & flag == 0 {
                continue;
            }
            if persistable {
                self.persistable &= !flag;
                self.persisted &= !flag;
            }
            self.global &= !flag;
            if including_owners {
                owners.clear();
            }
        }
        if self.persisted == 0 {
            self.persisted_time = INVALID_TIME;
        }
        self.persisted != before
    }

    /// `getStrength`: how strongly it grants `modes`.
    fn strength(&self, modes: i32) -> i32 {
        let modes = modes & ACCESS_MODES;
        if self.persistable & modes == modes {
            STRENGTH_PERSISTABLE
        } else if self.global & modes == modes {
            STRENGTH_GLOBAL
        } else if self.owned() & modes == modes {
            STRENGTH_OWNED
        } else {
            STRENGTH_NONE
        }
    }

    /// `buildPersistedPublicApiObject`.
    fn persisted_public(&self) -> PublicUriPermission {
        PublicUriPermission { uri: self.uri.uri.clone(), modes: self.persisted, persisted_time: self.persisted_time }
    }

    /// Its `<uri-grant>` (`writeGrantedUriPermissions`).
    fn element(&self) -> Element {
        let attrs = vec![
            ("sourceUserId".to_string(), Value::Int(self.uri.source_user)),
            ("targetUserId".to_string(), Value::Int(self.target_user())),
            ("sourcePkg".to_string(), Value::Interned(self.source_package.clone())),
            ("targetPkg".to_string(), Value::Interned(self.target_package.clone())),
            ("uri".to_string(), Value::String(self.uri.uri.as_str().to_string())),
            ("prefix".to_string(), Value::Bool(self.uri.prefix)),
            ("modeFlags".to_string(), Value::Int(self.persisted)),
            ("createdTime".to_string(), Value::Long(self.persisted_time)),
        ];
        Element { name: "uri-grant".into(), attrs, content: Vec::new() }
    }
}

/// `mGrantedUriPermissions`: each target uid's grants, in the order of
/// their `GrantUri`'s hash (`ArrayMap`).
#[derive(Default)]
struct Grants(BTreeMap<i32, Vec<UriPermission>>);

impl Grants {
    /// `findUriPermissionLocked`.
    fn find(&self, uid: i32, uri: &GrantUri) -> Option<&UriPermission> {
        self.0.get(&uid)?.iter().find(|p| p.uri == *uri)
    }

    fn find_mut(&mut self, uid: i32, uri: &GrantUri) -> Option<&mut UriPermission> {
        self.0.get_mut(&uid)?.iter_mut().find(|p| p.uri == *uri)
    }

    /// `findOrCreateUriPermissionLocked`.
    fn find_or_create(&mut self, source: &str, target: &str, target_uid: i32, uri: &GrantUri) -> &mut UriPermission {
        let perms = self.0.entry(target_uid).or_default();
        let at = match perms.iter().position(|p| p.uri == *uri) {
            Some(at) => at,
            None => {
                let hash = uri.hash();
                let at = perms.partition_point(|p| p.uri.hash() <= hash);
                perms.insert(at, UriPermission::new(source, target, target_uid, uri.clone()));
                at
            }
        };
        &mut perms[at]
    }

    /// `removeUriPermissionIfNeededLocked`: the grant of `uri` to `uid`
    /// goes when it has no modes left.
    fn remove_if_needed(&mut self, uid: i32, uri: &GrantUri) {
        let Some(perms) = self.0.get_mut(&uid) else { return };
        perms.retain(|p| p.uri != *uri || p.modes() != 0);
        if perms.is_empty() {
            self.0.remove(&uid);
        }
    }

    /// `checkUriPermissionLocked`: whether `uid` holds `uri` with `modes`,
    /// by its grant or a prefix one, persistable ones only with
    /// `FLAG_GRANT_PERSISTABLE_URI_PERMISSION`.
    fn check(&self, uri: &GrantUri, uid: i32, modes: i32) -> bool {
        let min = if modes & FLAG_GRANT_PERSISTABLE_URI_PERMISSION != 0 { STRENGTH_PERSISTABLE } else { STRENGTH_OWNED };
        if uid == ROOT_UID {
            return true;
        }
        let Some(perms) = self.0.get(&uid) else { return false };
        if perms.iter().find(|p| p.uri == *uri).is_some_and(|p| p.strength(modes) >= min) {
            return true;
        }
        perms.iter().any(|p| p.uri.prefix && is_path_prefix_match(&uri.uri, &p.uri.uri) && p.strength(modes) >= min)
    }

    /// `UriPermissionOwner.removeUriPermissions`: `owner`'s modes go, and
    /// with them the grants left with none.
    fn release(&mut self, owner: Owner) {
        for perms in self.0.values_mut() {
            for p in perms.iter_mut() {
                p.read_owners.retain(|o| *o != owner);
                p.write_owners.retain(|o| *o != owner);
            }
            perms.retain(|p| p.modes() != 0);
        }
        self.0.retain(|_, perms| !perms.is_empty());
    }

    /// `maybePrunePersistedUriGrantsLocked`: the oldest persisted grants
    /// of `uid` beyond `MAX_PERSISTED_URI_GRANTS` released; whether any
    /// was.
    fn prune_persisted(&mut self, uid: i32) -> bool {
        let Some(perms) = self.0.get_mut(&uid) else { return false };
        if perms.len() < MAX_PERSISTED_URI_GRANTS {
            return false;
        }
        let mut persisted: Vec<(i64, GrantUri)> =
            perms.iter().filter(|p| p.persisted != 0).map(|p| (p.persisted_time, p.uri.clone())).collect();
        let Some(trim) = persisted.len().checked_sub(MAX_PERSISTED_URI_GRANTS).filter(|&n| n > 0) else {
            return false;
        };
        persisted.sort_by_key(|(time, _)| *time);
        for (_, uri) in persisted.into_iter().take(trim) {
            if let Some(p) = self.find_mut(uid, &uri) {
                p.release_persistable_modes(!0);
            }
            self.remove_if_needed(uid, &uri);
        }
        true
    }

    /// `revokeUriPermissionLocked`: `modes` of the grants of `uri` and
    /// below it (to `target`, if named) revoked; of all its grants when
    /// the caller holds it by the provider's permissions, else of its own
    /// grants, not those owned. Whether what persists changed.
    fn revoke(&mut self, target: Option<&str>, calling_uid: i32, uri: &GrantUri, modes: i32, caller_holds: bool) -> bool {
        let mut changed = false;
        let including_owners = caller_holds && target.is_none();
        for (uid, perms) in self.0.iter_mut().rev() {
            if !caller_holds && *uid != calling_uid {
                continue;
            }
            for p in perms.iter_mut().rev() {
                if target.is_some_and(|t| t != p.target_package) {
                    continue;
                }
                if p.uri.source_user == uri.source_user && is_path_prefix_match(&p.uri.uri, &uri.uri) {
                    changed |= p.revoke_modes(modes | FLAG_GRANT_PERSISTABLE_URI_PERMISSION, including_owners);
                }
            }
            perms.retain(|p| p.modes() != 0);
        }
        self.0.retain(|_, perms| !perms.is_empty());
        changed
    }

    /// `removeUriPermissionsForPackageLocked(package, user, true, true)`:
    /// every grant to `package` (to any, if none) in `user` revoked.
    /// Whether what persists changed.
    fn clear(&mut self, package: Option<&str>, user: i32) -> bool {
        let mut changed = false;
        for (uid, perms) in self.0.iter_mut() {
            if user != USER_ALL && user != uid / PER_USER_RANGE as i32 {
                continue;
            }
            perms.retain_mut(|p| {
                if package.is_some_and(|package| package != p.target_package) {
                    return true;
                }
                changed |= p.revoke_modes(!0, true);
                p.modes() != 0
            });
        }
        self.0.retain(|_, perms| !perms.is_empty());
        changed
    }

    /// `checkAuthorityGrantsLocked`: whether `uid` holds a grant of a URI
    /// of one of `cpi`'s authorities in `user` (in any, unless
    /// `check_user`).
    fn authority_granted(&self, uid: i32, cpi: &ProviderInfo, user: i32, check_user: bool) -> bool {
        let Some(authorities) = cpi.authority.as_deref() else { return false };
        self.0.get(&uid).is_some_and(|perms| {
            perms.iter().rev().any(|p| {
                (p.uri.source_user == user || !check_user)
                    && p.uri.uri.authority().is_some_and(|a| authorities.split(';').any(|c| c == a))
            })
        })
    }

    /// The document `writeGrantedUriPermissions` writes: the persisted
    /// grants.
    fn document(&self) -> Element {
        let grants = self.0.values().flatten().filter(|p| p.persisted != 0).map(|p| Node::Element(p.element())).collect();
        Element { name: "uri-grants".into(), attrs: Vec::new(), content: grants }
    }
}

/// A grant as `urigrants.xml` keeps it.
#[derive(Debug, PartialEq)]
struct Persisted {
    source_user: i32,
    target_user: i32,
    source_package: String,
    target_package: String,
    uri: String,
    prefix: bool,
    modes: i32,
    created: i64,
}

/// The grants of `urigrants.xml`'s document (`readGrantedUriPermissionsLocked`,
/// `userHandle` before the source and target users), those with no
/// packages or URI left out; their creation `now` unless it is kept.
fn persisted(root: &Element, now: i64) -> Result<Vec<Persisted>, String> {
    let mut out = Vec::new();
    for e in root.children().filter(|e| e.name == "uri-grant") {
        let handle = e.int("userHandle")?;
        let (Some(source), Some(target), Some(uri)) = (e.string("sourcePkg"), e.string("targetPkg"), e.string("uri")) else {
            continue;
        };
        out.push(Persisted {
            source_user: handle.or(e.int("sourceUserId")?).unwrap_or(0),
            target_user: handle.or(e.int("targetUserId")?).unwrap_or(0),
            source_package: source.to_string(),
            target_package: target.to_string(),
            uri: uri.to_string(),
            prefix: e.bool("prefix")?.unwrap_or(false),
            modes: e.int("modeFlags")?.unwrap_or(0),
            created: e.long("createdTime")?.unwrap_or(now),
        });
    }
    Ok(out)
}

/// `NeededUriGrants`: what an intent grants its target.
#[derive(Clone, Debug)]
pub(super) struct NeededUriGrants {
    target_package: String,
    target_uid: i32,
    flags: i32,
    uris: Vec<GrantUri>,
}

/// What `checkGrantUriPermissionFromIntent` reads of an intent
/// (`Intent(Parcel)`): its content user hint, its data, and its ClipData's
/// items, each its URI, else its intent.
struct Carried {
    content_user: i32,
    data: Option<Uri>,
    clip: Option<Vec<ClipItem>>,
}

enum ClipItem {
    Uri(Uri),
    Intent(Carried),
}

/// `Intent(Parcel)`, read up to its content user hint.
fn carried(r: &mut Reader<'_>) -> ParcelResult<Carried> {
    r.read_string8()?; // action
    let data = Uri::read(r, &mut Plain)?;
    r.read_string8()?; // type
    r.read_string8()?; // identifier
    r.read_i32()?; // flags
    r.read_i32()?; // extended flags
    r.read_string8()?; // package
    if r.read_string16()?.is_some() {
        r.read_string16()?; // component class
    }
    if r.read_i32()? != 0 {
        r.skip(16)?; // source bounds
    }
    for _ in 0..r.read_i32()?.max(0) {
        r.read_string8()?; // category
    }
    if r.read_i32()? != 0 {
        clip::intent(r)?; // selector
    }
    let clip = if r.read_i32()? != 0 { Some(clip_items(r)?) } else { None };
    Ok(Carried { content_user: r.read_i32()?, data, clip })
}

/// `ClipData(Parcel)`: of each item, its URI, else its intent.
fn clip_items(r: &mut Reader<'_>) -> ParcelResult<Vec<ClipItem>> {
    ClipDescription::read_from(r)?;
    if r.read_i32()? != 0 {
        return Err(BAD_VALUE); // an icon
    }
    let mut items = Vec::new();
    for _ in 0..r.read_i32()?.max(0) {
        clip::char_sequence(r)?;
        r.read_string8()?; // html
        let intent = match r.read_i32()? {
            0 => None,
            _ => {
                let item = carried(&mut r.clone())?;
                clip::intent(r)?;
                Some(item)
            }
        };
        if r.read_i32()? != 0 {
            r.read_binder()?; // IntentSender
        }
        let uri = if r.read_i32()? != 0 { Uri::read(r, &mut Plain)? } else { None };
        if r.read_i32()? != 0 {
            return Err(BAD_VALUE); // ActivityInfo
        }
        if r.read_i32()? != 0 {
            clip::text_links(r)?;
        }
        match (uri, intent) {
            (Some(uri), _) => items.push(ClipItem::Uri(uri)),
            (None, Some(intent)) => items.push(ClipItem::Intent(intent)),
            (None, None) => {}
        }
    }
    Ok(items)
}

/// `UriGrantsManagerService`'s state in the app host.
#[derive(Default)]
pub struct UriGrants {
    /// The grants, once read.
    grants: Mutex<Option<Grants>>,
    next_start_owner: AtomicU64,
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

/// `isContentUriWithAccessModeFlags`.
fn is_content_with_access_modes(uri: &GrantUri, modes: i32) -> bool {
    modes & ACCESS_MODES != 0 && uri.uri.scheme() == Some(SCHEME_CONTENT)
}

/// `Preconditions.checkFlagsArgument`.
fn check_flags(modes: i32, allowed: i32) -> Result<(), Exception> {
    if modes & allowed != modes {
        return Err(Exception::illegal_argument(format!("Requested flags 0x{modes:x}, but only 0x{allowed:x} are allowed")));
    }
    Ok(())
}

/// `enforceNotIsolatedCaller`.
fn not_isolated(call: &Call<'_>, method: &str) -> Result<(), Exception> {
    if is_isolated(call.sender_euid as i32) {
        return Err(Exception::security(format!("Isolated process not allowed to call {method}")));
    }
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// `AttributionSource(uid, package, null).writeToParcel`: its
/// `AttributionSourceState` (its size, then pid, uid, device id, package,
/// attribution tag, token, renounced permissions and the next sources).
fn write_attribution_source(p: &mut Parcel, uid: i32, package: &str) {
    let start = p.position();
    p.write_i32(0);
    p.write_i32(INVALID_PID);
    p.write_i32(uid);
    p.write_i32(0); // Context.DEVICE_ID_DEFAULT
    p.write_string16(Some(package));
    p.write_string16(None);
    p.write_binder(None);
    p.write_i32(-1); // no renounced permissions
    p.write_i32(0); // no next source
    let size = (p.position() - start) as i32;
    p.set_i32_at(start, size);
}

/// Writes the document to `/data/system/urigrants.xml`, as the system's,
/// through a new file (`AtomicFile`).
fn write(data: &Path, document: &Element) -> std::io::Result<()> {
    let bytes = aim_android_xml::abx::write(document).map_err(std::io::Error::other)?;
    let path = data.join("system/urigrants.xml");
    let new = data.join("system/urigrants.xml.new");
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&new)?;
    std::io::Write::write_all(&mut file, &bytes)?;
    crate::fsync(&file)?;
    aim_storage::guest_inode::record(
        &new,
        aim_storage::guest_inode::GuestInode { uid: Some(1000), gid: Some(1000), mode: Some(0o600) },
    )?;
    std::fs::rename(&new, path)
}

impl AppHost {
    /// The persisted grants (`readGrantedUriPermissionsLocked`): those
    /// whose source still provides the URI's authority and whose target is
    /// installed; a file that cannot be read gives none, logged.
    fn read_grants(&self) -> Result<Grants, Exception> {
        let path = self.data()?.join("system/urigrants.xml");
        let mut grants = Grants::default();
        let read = std::fs::read(&path).map_err(|e| (e.kind() != std::io::ErrorKind::NotFound).then(|| e.to_string()));
        let found = read.and_then(|bytes| aim_android_xml::read(&bytes).and_then(|root| persisted(&root, now_ms())).map_err(Some));
        let found = match found {
            Ok(found) => found,
            Err(None) => return Ok(grants),
            Err(Some(e)) => {
                eprintln!("app host: Failed reading Uri grants: {}: {e}", path.display());
                return Ok(grants);
            }
        };
        for g in found {
            let uri = Uri::parse(&g.uri);
            let authority = uri.authority();
            let provider = self.grant_provider(authority.as_deref(), g.source_user, MATCH_DIRECT_BOOT_AWARE_UNAWARE, SYSTEM_UID)?;
            let source = provider.as_ref().and_then(|p| p.info.item.package_name.as_deref());
            if source != Some(g.source_package.as_str()) {
                eprintln!("app host: Persisted grant for {} had source {} but instead found {source:?}", g.uri, g.source_package);
                continue;
            }
            let target_uid = self.package_uid(&g.target_package, MATCH_UNINSTALLED_PACKAGES, g.target_user)?;
            if target_uid == -1 {
                continue;
            }
            let modes = if g.prefix { FLAG_GRANT_PREFIX_URI_PERMISSION } else { 0 };
            let uri = GrantUri::new(g.source_user, uri, modes);
            grants.find_or_create(&g.source_package, &g.target_package, target_uid, &uri).init_persisted_modes(g.modes, g.created);
        }
        Ok(grants)
    }

    /// `f` of the grants, read at their first use.
    fn with_grants<T>(&self, f: impl FnOnce(&mut Grants) -> T) -> Result<T, Exception> {
        let mut grants = self.uri_grants.grants.lock().unwrap();
        if grants.is_none() {
            *grants = Some(self.read_grants()?);
        }
        Ok(f(grants.as_mut().expect("read")))
    }

    /// `f` of the grants, which says whether what persists changed: then
    /// they are written (`schedulePersistUriGrants`), a failure logged as
    /// `AtomicFile.failWrite` drops it.
    fn change_grants<T>(&self, f: impl FnOnce(&mut Grants) -> (T, bool)) -> Result<T, Exception> {
        let data = self.data()?;
        self.with_grants(|grants| {
            let (out, changed) = f(grants);
            if changed && let Err(e) = write(data, &grants.document()) {
                eprintln!("app host: writing urigrants.xml: {e}");
            }
            out
        })
    }

    /// A new owner for a service start's grants.
    pub(super) fn new_start_owner(&self) -> Owner {
        Owner::Start(self.uri_grants.next_start_owner.fetch_add(1, Ordering::Relaxed))
    }

    /// `UriPermissionOwner.removeUriPermissions`: what `owner` was granted
    /// goes with it.
    pub(super) fn release_uri_grants(&self, owner: Owner) {
        if let Some(grants) = self.uri_grants.grants.lock().unwrap().as_mut() {
            grants.release(owner);
        }
    }

    /// `PackageManagerInternal.getPackageUid` of `package` in `user`; -1
    /// for none.
    fn package_uid(&self, package: &str, flags: i64, user: i32) -> Result<i32, Exception> {
        let (state, resolution) = self.packages()?;
        let query = Query { state: &state, filter: &resolution.apps_filter, calling_uid: SYSTEM_UID };
        query
            .package_uid_internal(package, flags, user, SYSTEM_UID)
            .map_err(|NotModelled(reason)| Exception::new(EX_UNSUPPORTED_OPERATION, format!("getPackageUid: {reason}")))
    }

    /// `getProviderInfo`: the provider of `authority` in `user`, with its
    /// URI permission patterns, as `uid` may see it.
    fn grant_provider(&self, authority: Option<&str>, user: i32, flags: i64, uid: i32) -> Result<Option<ProviderInfo>, Exception> {
        match authority {
            Some(authority) => self.resolve_provider_as(authority, GET_URI_PERMISSION_PATTERNS | flags, user, uid),
            None => Ok(None),
        }
    }

    /// `PackageManagerInternal.filterAppAccess(package, uid, user)`.
    fn filter_package_access(&self, package: &str, uid: i32, user: i32) -> Result<bool, Exception> {
        let (state, resolution) = self.packages()?;
        let ps = state.packages.get(package);
        should_filter_application(&state, &resolution.apps_filter, ps, uid, user, true, true)
            .map_err(|NotModelled(reason)| Exception::new(EX_ILLEGAL_STATE, format!("filterAppAccess: {reason}")))
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
        not_isolated(call, "checkUriPermission")?;
        if uid != ROOT_UID && self.filter_app_access(uid, call.sender_euid as i32)? {
            return Ok(DENIED);
        }
        let granted = self.with_grants(|g| g.check(&GrantUri::new(user, uri, modes), uid, modes))?;
        Ok(if granted { GRANTED } else { DENIED })
    }

    /// `checkHoldingPermissionsUnlocked`: whether `uid` may use `uri` with
    /// `modes` by `pi`'s permissions; across users only with
    /// `INTERACT_ACROSS_USERS`.
    fn holds_permissions(&self, pi: &ProviderInfo, uri: &GrantUri, uid: i32, modes: i32) -> Result<bool, Exception> {
        if uid / PER_USER_RANGE as i32 != uri.source_user && !self.check_component_permission(Some(INTERACT_ACROSS_USERS), uid, -1, true)? {
            return Ok(false);
        }
        self.holds_permissions_internal(pi, uri, uid, modes, true)
    }

    /// `checkHoldingPermissionsInternalUnlocked`: the provider's app, else
    /// for an exported provider its read and write permissions and the
    /// path permissions matching the URI (the uid's permissions only with
    /// `consider_uid_permissions`), and the provider's own check when it
    /// forces URI permissions.
    fn holds_permissions_internal(&self, pi: &ProviderInfo, uri: &GrantUri, uid: i32, modes: i32, consider_uid_permissions: bool) -> Result<bool, Exception> {
        if pi.info.application_info.uid == uid {
            return Ok(true);
        }
        if !pi.info.exported {
            return Ok(false);
        }
        let holds = |permission: &str| -> Result<bool, Exception> { Ok(consider_uid_permissions && self.check_permission(Some(permission), uid)? == 0) };
        let mut read_met = modes & FLAG_GRANT_READ_URI_PERMISSION == 0;
        let mut write_met = modes & FLAG_GRANT_WRITE_URI_PERMISSION == 0;
        if !read_met && let Some(p) = &pi.read_permission {
            read_met = holds(p)?;
        }
        if !write_met && let Some(p) = &pi.write_permission {
            write_met = holds(p)?;
        }
        // Unprotected access, unless a matching path permission the uid
        // does not hold takes it away.
        let mut allow_default_read = pi.read_permission.is_none();
        let mut allow_default_write = pi.write_permission.is_none();
        let path = uri.uri.path();
        for pp in pi.path_permissions.iter().flatten().rev() {
            if read_met && write_met {
                break;
            }
            if !pp.pattern.matches(path.as_deref()) {
                continue;
            }
            if !read_met && let Some(p) = &pp.read_permission {
                if holds(p)? {
                    read_met = true;
                } else {
                    allow_default_read = false;
                }
            }
            if !write_met && let Some(p) = &pp.write_permission {
                if holds(p)? {
                    write_met = true;
                } else {
                    allow_default_write = false;
                }
            }
        }
        read_met |= allow_default_read;
        write_met |= allow_default_write;
        // A provider that forces grants decides itself; it cannot track
        // another user's.
        let force_met = if pi.force_uri_permissions {
            let provider_user = pi.info.application_info.uid / PER_USER_RANGE as i32;
            provider_user == uid / PER_USER_RANGE as i32 && self.provider_uri_permission(&uri.uri, provider_user, uid, modes)
        } else {
            true
        };
        Ok(read_met && write_met && force_met)
    }

    /// `ContentProviderHelper.checkContentProviderUriPermission`: the
    /// provider's own `checkUriPermission` of `uri` in `user` for `uid`,
    /// asked as the system; denied when it fails.
    fn provider_uri_permission(&self, uri: &Uri, user: i32, uid: i32, modes: i32) -> bool {
        let Some(authority) = uri.authority() else { return false };
        let handle = match self.provider(None, SYSTEM_UID, &authority, user, true) {
            Ok(holder) => match holder.and_then(|h| h.provider) {
                Some(Binder::Handle(handle)) => handle,
                _ => return false,
            },
            Err(e) => {
                eprintln!("app host: Exception while determining type of {}: {}", uri.as_str(), e.message);
                return false;
            }
        };
        let mut p = Parcel::new();
        p.write_interface_token(PROVIDER_DESCRIPTOR);
        write_attribution_source(&mut p, uid, SYSTEM_PACKAGE);
        uri.write(&mut p);
        p.write_i32(uid);
        p.write_i32(modes);
        match self.process.transact(handle, CHECK_URI_PERMISSION_TRANSACTION, &p, false) {
            Ok(reply) => {
                let mut r = reply.reader();
                r.read_i32() == Ok(0) && r.read_i32() == Ok(0)
            }
            Err(status) => {
                eprintln!("app host: Content provider dead retrieving {}: {status}", uri.as_str());
                false
            }
        }
    }

    /// `checkGrantUriPermissionUnlocked`: the uid of `target` (or
    /// `last_target_uid`, when known) when `calling_uid` may grant it
    /// `uri` with `modes` and it needs the grant; -1 when there is none to
    /// make. A `SecurityException` when the provider or the caller does
    /// not allow it.
    fn check_grant(&self, calling_uid: i32, target: Option<&str>, uri: &GrantUri, modes: i32, last_target_uid: i32) -> Result<i32, Exception> {
        if !is_content_with_access_modes(uri, modes) {
            return Ok(-1);
        }
        let authority = uri.uri.authority();
        // The system grants only on behalf of someone.
        if matches!(calling_uid.rem_euclid(PER_USER_RANGE as i32), SYSTEM_UID | ROOT_UID)
            && !matches!(authority.as_deref(), Some("com.android.settings.files" | "com.android.settings.module_licenses"))
        {
            eprintln!("app host: For security reasons, the system cannot issue a Uri permission grant to {uri}; use startActivityAsCaller() instead");
            return Ok(-1);
        }
        let Some(pi) = self.grant_provider(authority.as_deref(), uri.source_user, MATCH_DIRECT_BOOT_AUTO, calling_uid)? else {
            eprintln!("app host: No content provider found for permission check: {}", uri.uri.as_str());
            return Ok(-1);
        };
        let mut target_uid = last_target_uid;
        if target_uid < 0
            && let Some(target) = target
        {
            target_uid = self.package_uid(target, MATCH_DEBUG_TRIAGED_MISSING, calling_uid / PER_USER_RANGE as i32)?;
            if target_uid < 0 {
                return Ok(-1);
            }
        }
        let path = uri.uri.path();
        let mut target_holds = if target_uid >= 0 {
            self.holds_permissions(&pi, uri, target_uid, modes)?
        } else {
            // No target: whether anyone may use it.
            let read = modes & FLAG_GRANT_READ_URI_PERMISSION != 0;
            let write = modes & FLAG_GRANT_WRITE_URI_PERMISSION != 0;
            let mut allowed = pi.info.exported && !(read && pi.read_permission.is_some()) && !(write && pi.write_permission.is_some());
            if let Some(pp) = pi.path_permissions.iter().flatten().find(|pp| pp.pattern.matches(path.as_deref())) {
                allowed &= !(read && pp.read_permission.is_some() || write && pp.write_permission.is_some());
            }
            allowed
        };
        // A provider that forces grants gets one for each item.
        if pi.force_uri_permissions {
            target_holds = false;
        }
        let basic = modes & (FLAG_GRANT_PERSISTABLE_URI_PERMISSION | FLAG_GRANT_PREFIX_URI_PERMISSION) == 0;
        if basic && target_holds {
            return Ok(-1);
        }
        // Another user's target may get a basic grant of what anyone in
        // the source user may use.
        let special_cross_user = target_uid >= 0
            && target_uid / PER_USER_RANGE as i32 != uri.source_user
            && self.holds_permissions_internal(&pi, uri, calling_uid, modes, false)?;
        let grant_allowed = match pi.uri_permission_patterns.as_deref().filter(|p| !p.is_empty()) {
            Some(patterns) => patterns.iter().any(|p| p.matches(path.as_deref())),
            None => pi.grant_uri_permissions,
        };
        if !grant_allowed {
            let name = format!("{}/{}", pi.info.item.package_name.as_deref().unwrap_or("null"), pi.info.item.name.as_deref().unwrap_or("null"));
            if !special_cross_user {
                return Err(Exception::security(format!("Provider {name} does not allow granting of Uri permissions (uri {uri})")));
            }
            if !basic {
                return Err(Exception::security(format!("Provider {name} does not allow granting of advanced Uri permissions (uri {uri})")));
            }
        }
        // The caller itself must hold it, by the provider's permissions or
        // a grant strong enough.
        if !self.holds_permissions(&pi, uri, calling_uid, modes)? && !self.with_grants(|g| g.check(uri, calling_uid, modes))? {
            let hint = if pi.read_permission.as_deref() == Some(MANAGE_DOCUMENTS) {
                "; you could obtain access using ACTION_OPEN_DOCUMENT or related APIs"
            } else {
                ""
            };
            return Err(Exception::security(format!("UID {calling_uid} does not have permission to {uri}{hint}")));
        }
        Ok(target_uid)
    }

    /// `checkGrantUriPermissionFromIntent`: what `intent`'s flags grant
    /// `target` in `target_user` over its data and its ClipData's URIs
    /// and intents, checked for `calling_uid`; none when the target needs
    /// nothing.
    pub(super) fn check_grant_from_intent(&self, intent: &RawIntent, calling_uid: i32, target: &str, target_user: i32) -> Result<Option<NeededUriGrants>, Exception> {
        let carried = carried(&mut intent.parcel().reader()).map_err(|_| Exception::new(EX_ILLEGAL_STATE, "an intent read again"))?;
        let mut needed = None;
        self.check_carried(calling_uid, target, &carried, intent.intent.flags, &mut needed, target_user)?;
        Ok(needed)
    }

    /// `checkGrantUriPermissionFromIntentUnlocked` of `intent` with `mode`:
    /// its data, then its ClipData's items in order (a URI, else an
    /// intent), added to `needed`.
    fn check_carried(&self, calling_uid: i32, target: &str, intent: &Carried, mode: i32, needed: &mut Option<NeededUriGrants>, target_user: i32) -> Result<(), Exception> {
        let user = if intent.content_user == USER_CURRENT { calling_uid / PER_USER_RANGE as i32 } else { intent.content_user };
        if intent.data.is_none() && intent.clip.is_none() {
            return Ok(());
        }
        let mut target_uid = match needed {
            Some(n) => n.target_uid,
            None => self.package_uid(target, MATCH_DEBUG_TRIAGED_MISSING, target_user)?,
        };
        if target_uid < 0 {
            return Ok(());
        }
        if let Some(data) = &intent.data {
            self.check_carried_uri(calling_uid, target, data, user, mode, &mut target_uid, needed)?;
        }
        for item in intent.clip.iter().flatten() {
            match item {
                ClipItem::Uri(uri) => self.check_carried_uri(calling_uid, target, uri, user, mode, &mut target_uid, needed)?,
                ClipItem::Intent(nested) => self.check_carried(calling_uid, target, nested, mode, needed, target_user)?,
            }
        }
        Ok(())
    }

    /// One URI of an intent, in `user` unless it names its own, checked
    /// for the target (`target_uid` as it stands) and added to `needed`.
    #[allow(clippy::too_many_arguments)]
    fn check_carried_uri(
        &self,
        calling_uid: i32,
        target: &str,
        uri: &Uri,
        user: i32,
        mode: i32,
        target_uid: &mut i32,
        needed: &mut Option<NeededUriGrants>,
    ) -> Result<(), Exception> {
        let uri = GrantUri::resolve(user, uri.clone(), mode);
        *target_uid = self.check_grant(calling_uid, Some(target), &uri, mode, *target_uid)?;
        if *target_uid > 0 {
            let n = needed.get_or_insert_with(|| NeededUriGrants { target_package: target.to_string(), target_uid: *target_uid, flags: mode, uris: Vec::new() });
            if !n.uris.contains(&uri) {
                n.uris.push(uri);
            }
        }
        Ok(())
    }

    /// `grantUriPermissionUnchecked`: `target` (`target_uid`) gets `modes`
    /// of `uri` from the provider's package, owned by `owner` (by nobody,
    /// if none).
    fn grant_unchecked(&self, target_uid: i32, target: &str, uri: &GrantUri, modes: i32, owner: Option<Owner>) -> Result<(), Exception> {
        if modes & ACCESS_MODES == 0 {
            return Ok(());
        }
        let authority = uri.uri.authority();
        let Some(pi) = self.grant_provider(authority.as_deref(), uri.source_user, MATCH_DEBUG_TRIAGED_MISSING, SYSTEM_UID)? else {
            eprintln!("app host: No content provider found for grant: {uri}");
            return Ok(());
        };
        let source = pi.info.item.package_name.unwrap_or_default();
        self.with_grants(|g| g.find_or_create(&source, target, target_uid, uri).grant_modes(modes, owner))
    }

    /// `grantUriPermissionUncheckedFromIntent`: what `needed` grants,
    /// owned by `owner` (by nobody, if none).
    pub(super) fn grant_from_intent(&self, needed: Option<&NeededUriGrants>, owner: Option<Owner>) -> Result<(), Exception> {
        let Some(n) = needed else { return Ok(()) };
        for uri in &n.uris {
            self.grant_unchecked(n.target_uid, &n.target_package, uri, n.flags, owner)?;
        }
        Ok(())
    }

    /// `checkAuthorityGrants` (`ContentProviderHelper
    /// .checkContentProviderPermission`): whether `uid` holds a grant of a
    /// URI of `cpi` in `user`.
    pub(super) fn authority_granted(&self, uid: i32, cpi: &ProviderInfo, user: i32) -> Result<bool, Exception> {
        self.with_grants(|g| g.authority_granted(uid, cpi, user, true))
    }

    /// `ActivityManagerService.grantUriPermission`: the caller's process
    /// grants `target` `modes` of `uri` in `user`, owned by nobody.
    pub(super) fn grant_uri_permission(&self, call: &Call<'_>, caller: Option<Binder>, target: Option<String>, uri: Option<Uri>, modes: i32, user: i32) -> Result<(), Exception> {
        not_isolated(call, "grantUriPermission")?;
        let Some((_, _, uid, _)) = self.app_of_thread(caller) else {
            let uri = uri.as_ref().map_or("null", Uri::as_str);
            return Err(Exception::security(format!("Unable to find app for caller when granting permission to uri {uri} [user {user}]")));
        };
        let target = target.ok_or_else(|| Exception::illegal_argument("null target"))?;
        let calling_user = uid / PER_USER_RANGE as i32;
        if self.filter_package_access(&target, uid, calling_user)? {
            return Ok(());
        }
        check_flags(modes, ACCESS_MODES | FLAG_GRANT_PERSISTABLE_URI_PERMISSION | FLAG_GRANT_PREFIX_URI_PERMISSION)?;
        // An intent of the URI with `user` in it, the modes its flags.
        let intent = Carried { content_user: USER_CURRENT, data: uri.map(|uri| maybe_add_user_id(&uri, user)), clip: None };
        let mut needed = None;
        self.check_carried(uid, &target, &intent, modes, &mut needed, calling_user)?;
        self.grant_from_intent(needed.as_ref(), None)
    }

    /// `ActivityManagerService.revokeUriPermission`: the caller's process
    /// revokes `modes` of `uri` in `user` and below it, from `target` or
    /// from all.
    pub(super) fn revoke_uri_permission(&self, call: &Call<'_>, caller: Option<Binder>, target: Option<String>, uri: Option<Uri>, modes: i32, user: i32) -> Result<(), Exception> {
        not_isolated(call, "revokeUriPermission")?;
        let Some((_, _, uid, _)) = self.app_of_thread(caller) else {
            let uri = uri.as_ref().map_or("null", Uri::as_str);
            return Err(Exception::security(format!("Unable to find app for caller when revoking permission to uri {uri}")));
        };
        let Some(uri) = uri else {
            eprintln!("app host: revokeUriPermission: null uri");
            return Ok(());
        };
        if modes & ACCESS_MODES == 0 {
            return Ok(());
        }
        let authority = uri.authority();
        // `UriGrantsManagerService.revokeUriPermission`, with the provider
        // as the caller sees it.
        let Some(pi) = self.grant_provider(authority.as_deref(), user, MATCH_DIRECT_BOOT_AWARE_UNAWARE, uid)? else {
            eprintln!("app host: No content provider found for permission revoke: {}", uri.as_str());
            return Ok(());
        };
        let uri = GrantUri::new(user, uri, modes);
        let holds = self.holds_permissions(&pi, &uri, uid, modes)?;
        self.change_grants(|g| ((), g.revoke(target.as_deref(), uid, &uri, modes, holds)))
    }
}

/// `uri_grants`: UriGrantsManagerService.
pub struct UriGrantsManager(pub Arc<AppHost>);

impl UriGrantsManager {
    /// `enforceCallingPermission`.
    fn enforce_calling_permission(&self, call: &Call<'_>, permission: &str, method: &str) -> Result<(), Exception> {
        if self.0.check_permission(Some(permission), call.sender_euid as i32)? != 0 {
            return Err(Exception::security(format!(
                "Permission Denial: {method} from pid={}, uid={} requires {permission}",
                call.sender_pid, call.sender_euid
            )));
        }
        Ok(())
    }

    /// The uid whose persistable grants `method` takes or releases: of
    /// `package`, for a caller that may force it, else the caller's.
    fn persistable_uid(&self, call: &Call<'_>, package: Option<&str>, user: i32, method: &str) -> Result<i32, Exception> {
        match package {
            Some(package) => {
                self.enforce_calling_permission(call, FORCE_PERSISTABLE_URI_PERMISSIONS, method)?;
                self.0.package_uid(package, 0, user)
            }
            None => {
                not_isolated(call, method)?;
                Ok(call.sender_euid as i32)
            }
        }
    }

    /// `takePersistableUriPermission`: the persistable `modes` of the
    /// grant of `uri` and of a prefix one persisted, the oldest beyond the
    /// most a uid keeps released.
    fn take_persistable(&self, call: &Call<'_>, uri: Uri, modes: i32, package: Option<String>, user: i32) -> Result<(), Exception> {
        let uid = self.persistable_uid(call, package.as_deref(), user, "takePersistableUriPermission")?;
        check_flags(modes, ACCESS_MODES)?;
        let now = now_ms();
        let exact = GrantUri::new(user, uri.clone(), 0);
        let prefix = GrantUri::new(user, uri, FLAG_GRANT_PREFIX_URI_PERMISSION);
        self.0.change_grants(|g| {
            let valid = |g: &Grants, u: &GrantUri| g.find(uid, u).is_some_and(|p| modes & p.persistable == modes);
            let (exact_valid, prefix_valid) = (valid(g, &exact), valid(g, &prefix));
            if !exact_valid && !prefix_valid {
                let message = format!("No persistable permission grants found for UID {uid} and Uri {}", exact.uri.as_str());
                return (Err(Exception::security(message)), false);
            }
            let mut changed = false;
            for (valid, u) in [(exact_valid, &exact), (prefix_valid, &prefix)] {
                if valid && let Some(p) = g.find_mut(uid, u) {
                    changed |= p.take_persistable_modes(modes, now);
                }
            }
            changed |= g.prune_persisted(uid);
            (Ok(()), changed)
        })?
    }

    /// `releasePersistableUriPermission`: `modes` of the grant of `uri`
    /// and of a prefix one no longer persisted.
    fn release_persistable(&self, call: &Call<'_>, uri: Uri, modes: i32, package: Option<String>, user: i32) -> Result<(), Exception> {
        let uid = self.persistable_uid(call, package.as_deref(), user, "releasePersistableUriPermission")?;
        check_flags(modes, ACCESS_MODES)?;
        let exact = GrantUri::new(user, uri.clone(), 0);
        let prefix = GrantUri::new(user, uri, FLAG_GRANT_PREFIX_URI_PERMISSION);
        self.0.change_grants(|g| {
            if g.find(uid, &exact).is_none() && g.find(uid, &prefix).is_none() && package.is_none() {
                let message = format!("No permission grants found for UID {uid} and Uri {}", exact.uri.as_str());
                return (Err(Exception::security(message)), false);
            }
            let mut changed = false;
            for u in [&exact, &prefix] {
                if let Some(p) = g.find_mut(uid, u) {
                    changed |= p.release_persistable_modes(modes);
                    g.remove_if_needed(uid, u);
                }
            }
            (Ok(()), changed)
        })?
    }

    /// `clearGrantedUriPermissions`: every grant to `package` (to any, if
    /// none) in `user` revoked.
    fn clear(&self, call: &Call<'_>, package: Option<String>, user: i32) -> Result<(), Exception> {
        self.enforce_calling_permission(call, CLEAR_APP_GRANTED_URI_PERMISSIONS, "clearGrantedUriPermissions")?;
        if user == USER_ALL && package.is_none() {
            return Err(Exception::illegal_argument("Must narrow by either package or user"));
        }
        self.0.change_grants(|g| ((), g.clear(package.as_deref(), user)))
    }

    /// `grantUriPermissionFromOwner`: a grant held by `owner`, a token of
    /// `newUriPermissionOwner`, of which the app host hands out none.
    fn grant_from_owner(&self, call: &Call<'_>, owner: Option<Binder>, target_user: i32) -> Result<(), Exception> {
        self.0.handle_incoming_user(call.sender_euid as i32, target_user, false, true, Some("grantUriPermissionFromOwner"))?;
        let owner = owner.map_or("null".to_string(), |b| format!("{b:?}"));
        Err(Exception::illegal_argument(format!("Unknown owner: {owner}")))
    }

    /// `checkGrantUriPermission_ignoreNonSystem`: for the system or a
    /// caller across users, the target's uid when `source_uid` may grant
    /// it `uri` and it needs the grant, else -1.
    fn check_grant_for(&self, call: &Call<'_>, source_uid: i32, target: Option<String>, uri: Uri, modes: i32, user: i32) -> Result<i32, Exception> {
        let calling = call.sender_euid as i32;
        if calling != SYSTEM_UID && calling != ROOT_UID && !self.0.check_component_permission(Some(INTERACT_ACROSS_USERS_FULL), calling, -1, true)? {
            return Ok(INVALID_UID);
        }
        self.0.check_grant(source_uid, target.as_deref(), &GrantUri::new(user, uri, modes), modes, -1)
    }

    /// `getUriPermissions`: the grants `package`, the caller's, was given
    /// (`incoming`) or gave, the persisted ones only if asked.
    fn uri_permissions(&self, call: &Call<'_>, package: Option<String>, incoming: bool, persisted_only: bool) -> Result<Vec<PublicUriPermission>, Exception> {
        let calling_uid = call.sender_euid as i32;
        not_isolated(call, "getUriPermissions")?;
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
        let listed = |p: &&UriPermission| !persisted_only || p.persisted != 0;
        self.0.with_grants(|g| {
            if !incoming {
                return g.0.values().flatten().filter(|p| p.source_package == package).filter(listed).map(UriPermission::persisted_public).collect();
            }
            match g.0.get(&calling_uid) {
                Some(perms) => perms.iter().filter(|p| p.target_package == package).filter(listed).map(UriPermission::persisted_public).collect(),
                None => {
                    eprintln!("app host: No permission grants found for {package}");
                    Vec::new()
                }
            }
        })
    }

    /// `getGrantedUriPermissions`: the persisted grants to `package` (to
    /// all, if none) in `user`.
    fn granted_uri_permissions(&self, call: &Call<'_>, package: Option<String>, user: i32) -> Result<Vec<GrantedUriPermission>, Exception> {
        self.enforce_calling_permission(call, GET_APP_GRANTED_URI_PERMISSIONS, "getGrantedUriPermissions")?;
        self.0.with_grants(|g| {
            g.0.values()
                .flatten()
                .filter(|p| package.as_ref().is_none_or(|package| *package == p.target_package))
                .filter(|p| p.target_user() == user && p.persisted != 0)
                .map(|p| GrantedUriPermission { uri: p.uri.uri.clone(), package: p.target_package.clone() })
                .collect()
        })
    }
}

/// The reply of a void method, written by `write`.
fn done(result: Result<(), Exception>, write: fn(&mut Parcel)) -> Reply {
    reply(result.map(|()| {
        let mut p = Parcel::new();
        write(&mut p);
        p
    }))
}

impl Service for UriGrantsManager {
    fn descriptor(&self) -> &str {
        ugm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let null_uri = || Exception::new(EX_NULL_POINTER, "uri");
        let mut p = Parcel::new();
        match call.code {
            ugm::GET_URI_PERMISSIONS => {
                let a = ugm::GetUriPermissions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let listed = self.uri_permissions(call, a.package_name, a.incoming, a.persisted_only);
                reply(listed.map(|listed| {
                    let slice = Slice::new(self.0.process.clone(), "android.content.UriPermission", listed);
                    ugm::write_get_uri_permissions_reply(&mut p, Some(&slice));
                    p
                }))
            }
            ugm::GET_GRANTED_URI_PERMISSIONS => {
                let a = ugm::GetGrantedUriPermissions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let listed = self.granted_uri_permissions(call, a.package_name, a.user_id);
                reply(listed.map(|listed| {
                    let slice = Slice::new(self.0.process.clone(), "android.app.GrantedUriPermission", listed);
                    ugm::write_get_granted_uri_permissions_reply(&mut p, Some(&slice));
                    p
                }))
            }
            ugm::TAKE_PERSISTABLE_URI_PERMISSION => {
                let a = ugm::TakePersistableUriPermission::<ParcelUri>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let taken = a.uri.ok_or_else(null_uri).and_then(|ParcelUri(uri)| self.take_persistable(call, uri, a.mode_flags, a.to_package, a.user_id));
                done(taken, ugm::write_take_persistable_uri_permission_reply)
            }
            ugm::RELEASE_PERSISTABLE_URI_PERMISSION => {
                let a = ugm::ReleasePersistableUriPermission::<ParcelUri>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let released = a.uri.ok_or_else(null_uri).and_then(|ParcelUri(uri)| self.release_persistable(call, uri, a.mode_flags, a.to_package, a.user_id));
                done(released, ugm::write_release_persistable_uri_permission_reply)
            }
            ugm::GRANT_URI_PERMISSION_FROM_OWNER => {
                let a = ugm::GrantUriPermissionFromOwner::<ParcelUri>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                done(self.grant_from_owner(call, a.owner, a.target_user_id), ugm::write_grant_uri_permission_from_owner_reply)
            }
            ugm::CLEAR_GRANTED_URI_PERMISSIONS => {
                let a = ugm::ClearGrantedUriPermissions::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                done(self.clear(call, a.package_name, a.user_id), ugm::write_clear_granted_uri_permissions_reply)
            }
            ugm::CHECK_GRANT_URI_PERMISSION_IGNORE_NON_SYSTEM => {
                let a = ugm::CheckGrantUriPermissionIgnoreNonSystem::<ParcelUri>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let checked = a.uri.ok_or_else(null_uri).and_then(|ParcelUri(uri)| self.check_grant_for(call, a.source_uid, a.target_pkg, uri, a.mode_flags, a.user_id));
                reply(checked.map(|uid| {
                    ugm::write_check_grant_uri_permission_ignore_non_system_reply(&mut p, uid);
                    p
                }))
            }
            code => reply(Err(self.0.unimplemented("uri_grants", code))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const READ: i32 = FLAG_GRANT_READ_URI_PERMISSION;
    const WRITE: i32 = FLAG_GRANT_WRITE_URI_PERMISSION;

    fn grant_uri(uri: &str, modes: i32) -> GrantUri {
        GrantUri::new(0, Uri::parse(uri), modes)
    }

    /// Grants of `uri` to uid 10001 of `modes`, owned by `owner`.
    fn granted(uri: &GrantUri, modes: i32, owner: Option<Owner>) -> Grants {
        let mut g = Grants::default();
        g.find_or_create("p.provider", "p.target", 10_001, uri).grant_modes(modes, owner);
        g
    }

    #[test]
    fn a_prefix_matches_its_descendants_only() {
        let prefix = Uri::parse("content://a.b/x/y");
        assert!(is_path_prefix_match(&Uri::parse("content://a.b/x/y/z"), &prefix));
        assert!(is_path_prefix_match(&Uri::parse("content://a.b/x/y"), &prefix));
        assert!(!is_path_prefix_match(&Uri::parse("content://a.b/x/yz"), &prefix));
        assert!(!is_path_prefix_match(&Uri::parse("content://a.c/x/y/z"), &prefix));
    }

    #[test]
    fn a_grant_is_as_strong_as_how_it_was_made() {
        let uri = grant_uri("content://a.b/x", 0);
        let mut p = UriPermission::new("a", "b", 10_000, uri);
        p.init_persisted_modes(READ, 0);
        assert_eq!(p.strength(READ), STRENGTH_PERSISTABLE);
        assert_eq!(p.strength(READ | WRITE), STRENGTH_NONE);
        p.grant_modes(WRITE, Some(Owner::Activity(1)));
        assert_eq!(p.strength(WRITE), STRENGTH_OWNED);
        p.grant_modes(READ | WRITE, None);
        assert_eq!(p.strength(READ | WRITE), STRENGTH_GLOBAL);
        assert_eq!(p.modes(), READ | WRITE);
    }

    #[test]
    fn grants_are_checked_exactly_or_by_prefix() {
        let g = granted(&grant_uri("content://a.b/x", FLAG_GRANT_PREFIX_URI_PERMISSION), READ, None);
        assert!(g.check(&grant_uri("content://a.b/x/y", 0), 10_001, READ));
        assert!(!g.check(&grant_uri("content://a.b/x/y", 0), 10_001, WRITE));
        assert!(!g.check(&grant_uri("content://a.b/y", 0), 10_001, READ));
        assert!(!g.check(&grant_uri("content://a.b/x/y", 0), 10_002, READ));
        assert!(g.check(&grant_uri("content://a.b/x/y", 0), ROOT_UID, WRITE));
        // A grant not offered for persisting is too weak for a check that
        // asks for one.
        assert!(!g.check(&grant_uri("content://a.b/x/y", 0), 10_001, READ | FLAG_GRANT_PERSISTABLE_URI_PERMISSION));
        let g = granted(&grant_uri("content://a.b/x", 0), READ, None);
        assert!(g.check(&grant_uri("content://a.b/x", 0), 10_001, READ));
        assert!(!g.check(&grant_uri("content://a.b/x/y", 0), 10_001, READ));
    }

    #[test]
    fn an_owners_grants_go_with_it() {
        let uri = grant_uri("content://a.b/x", 0);
        let mut g = granted(&uri, READ | WRITE, Some(Owner::Activity(7)));
        g.find_or_create("p.provider", "p.target", 10_001, &uri).grant_modes(READ, Some(Owner::Start(1)));
        g.release(Owner::Activity(7));
        assert!(g.check(&uri, 10_001, READ) && !g.check(&uri, 10_001, WRITE));
        g.release(Owner::Start(1));
        assert!(g.0.is_empty());
        // A grant also made with no owner stays.
        let mut g = granted(&uri, READ, Some(Owner::Activity(7)));
        g.find_or_create("p.provider", "p.target", 10_001, &uri).grant_modes(READ, None);
        g.release(Owner::Activity(7));
        assert!(g.check(&uri, 10_001, READ));
    }

    #[test]
    fn a_revoke_takes_what_its_caller_may() {
        let uri = grant_uri("content://a.b/x/y", 0);
        // A caller without the provider's permission revokes only its own
        // grants not owned by others.
        let mut g = granted(&uri, READ, Some(Owner::Activity(7)));
        g.find_or_create("p.provider", "p.target", 10_001, &uri).grant_modes(READ, None);
        assert!(!g.revoke(None, 10_001, &grant_uri("content://a.b/x", 0), READ, false));
        assert!(g.check(&uri, 10_001, READ));
        // One that holds it revokes them all, owned ones too.
        g.revoke(None, 10_005, &grant_uri("content://a.b/x", 0), READ, true);
        assert!(g.0.is_empty());
        // Of another target only when it names none.
        let mut g = granted(&uri, READ, None);
        g.revoke(Some("other"), 10_005, &grant_uri("content://a.b/x", 0), READ, true);
        assert!(g.check(&uri, 10_001, READ));
    }

    #[test]
    fn persisted_grants_are_taken_released_and_pruned() {
        let uri = grant_uri("content://a.b/x", 0);
        let mut g = granted(&uri, READ | FLAG_GRANT_PERSISTABLE_URI_PERMISSION, Some(Owner::Activity(1)));
        let p = g.find_mut(10_001, &uri).unwrap();
        assert!(!p.take_persistable_modes(WRITE, 5));
        assert!(p.take_persistable_modes(READ, 5));
        assert_eq!((p.persisted, p.persisted_time), (READ, 5));
        g.release(Owner::Activity(1));
        assert!(g.check(&uri, 10_001, READ | FLAG_GRANT_PERSISTABLE_URI_PERMISSION));
        let p = g.find_mut(10_001, &uri).unwrap();
        assert!(p.release_persistable_modes(READ));
        assert_eq!(p.persisted_time, INVALID_TIME);
        g.remove_if_needed(10_001, &uri);
        assert!(g.0.is_empty());
        // The oldest persisted grants beyond the most a uid keeps go.
        let mut g = Grants::default();
        for i in 0..MAX_PERSISTED_URI_GRANTS + 2 {
            let uri = grant_uri(&format!("content://a.b/{i}"), 0);
            let p = g.find_or_create("p.provider", "p.target", 10_001, &uri);
            p.init_persisted_modes(READ, 1_000 - i as i64);
        }
        assert!(g.prune_persisted(10_001));
        assert_eq!(g.0[&10_001].len(), MAX_PERSISTED_URI_GRANTS);
        let last = MAX_PERSISTED_URI_GRANTS + 1;
        assert!(g.find(10_001, &grant_uri(&format!("content://a.b/{last}"), 0)).is_none());
        assert!(g.find(10_001, &grant_uri("content://a.b/0", 0)).is_some());
    }

    #[test]
    fn persisted_grants_are_written_and_read_back() {
        let mut g = Grants::default();
        let uri = grant_uri("content://a.b/x", FLAG_GRANT_PREFIX_URI_PERMISSION);
        g.find_or_create("p.provider", "p.target", 10_001, &uri).init_persisted_modes(READ | WRITE, 42);
        // Not persisted: not written.
        g.find_or_create("p.provider", "p.target", 10_001, &grant_uri("content://a.b/y", 0)).grant_modes(READ, None);
        let bytes = aim_android_xml::abx::write(&g.document()).unwrap();
        assert!(bytes.starts_with(aim_android_xml::abx::MAGIC));
        let read = persisted(&aim_android_xml::read(&bytes).unwrap(), 0).unwrap();
        assert_eq!(
            read,
            vec![Persisted {
                source_user: 0,
                target_user: 0,
                source_package: "p.provider".into(),
                target_package: "p.target".into(),
                uri: "content://a.b/x".into(),
                prefix: true,
                modes: READ | WRITE,
                created: 42,
            }]
        );
        // A file of an older release names one user for both.
        let root = aim_android_xml::read(br#"<uri-grants><uri-grant userHandle="10" sourcePkg="s" targetPkg="t" uri="content://a/b" modeFlags="1" /></uri-grants>"#).unwrap();
        let read = persisted(&root, 9).unwrap();
        assert_eq!((read[0].source_user, read[0].target_user, read[0].created, read[0].prefix), (10, 10, 9, false));
    }

    #[test]
    fn a_grant_uri_hashes_as_java_does() {
        let uri = Uri::parse("content://com.android.externalstorage.documents/tree/primary%3AAlarms");
        assert_eq!(GrantUri { source_user: 0, uri, prefix: false }.hash(), 1780322610);
    }

    #[test]
    fn a_content_uri_names_its_user() {
        let g = GrantUri::resolve(0, Uri::parse("content://10@a.b/x"), FLAG_GRANT_PREFIX_URI_PERMISSION);
        assert_eq!((g.source_user, g.uri.as_str(), g.prefix), (10, "content://a.b/x", true));
        let g = GrantUri::resolve(3, Uri::parse("content://a.b/x"), 0);
        assert_eq!((g.source_user, g.uri.as_str()), (3, "content://a.b/x"));
        let g = GrantUri::resolve(3, Uri::parse("file:///x"), 0);
        assert_eq!((g.source_user, g.uri.as_str()), (3, "file:///x"));
        assert_eq!(GrantUri::resolve(0, Uri::parse("content://me@a.b/x"), 0).source_user, USER_NULL);
        // `grantUriPermission`'s user goes in and comes out again.
        let added = maybe_add_user_id(&Uri::parse("content://a.b/x"), 10);
        assert_eq!(added.as_str(), "content://10@a.b/x");
        assert_eq!(maybe_add_user_id(&added, 11).as_str(), "content://10@a.b/x");
    }

    /// An intent with a data URI and a clip of a URI and of an intent with
    /// a data URI, as `Intent.writeToParcel` writes it.
    fn intent_with_clip() -> Parcel {
        fn intent(p: &mut Parcel, data: &str, clip: impl FnOnce(&mut Parcel), user: i32) {
            p.write_string8(Some("a.SEND"));
            Uri::parse(data).write(p);
            p.write_string8(None); // type
            p.write_string8(None); // identifier
            p.write_i32(READ); // flags
            p.write_i32(0); // extended flags
            p.write_string8(None); // package
            p.write_string16(None); // component
            p.write_i32(0); // source bounds
            p.write_i32(0); // categories
            p.write_i32(0); // selector
            clip(p);
            p.write_i32(user); // content user hint
            p.write_i32(-1); // extras
            p.write_i32(0); // original intent
            p.write_i32(0); // creator token
        }
        fn item(p: &mut Parcel, intent_of: Option<&str>, uri: Option<&str>) {
            clip::write_char_sequence(p, None);
            p.write_string8(None); // html
            match intent_of {
                Some(data) => {
                    p.write_i32(1);
                    intent(p, data, |p| p.write_i32(0), USER_CURRENT);
                }
                None => p.write_i32(0),
            }
            p.write_i32(0); // IntentSender
            match uri {
                Some(uri) => {
                    p.write_i32(1);
                    Uri::parse(uri).write(p);
                }
                None => p.write_i32(0),
            }
            p.write_i32(0); // ActivityInfo
            p.write_i32(0); // TextLinks
        }
        let mut p = Parcel::new();
        intent(
            &mut p,
            "content://a.b/data",
            |p| {
                p.write_i32(1);
                clip::ClipData::host_text("", 0).description.write_to(p);
                p.write_i32(0); // icon
                p.write_i32(3);
                item(p, None, Some("content://a.b/clip"));
                item(p, Some("content://a.b/nested"), None);
                item(p, None, None);
            },
            10,
        );
        p
    }

    #[test]
    fn an_intent_carries_its_data_and_clip() {
        let p = intent_with_clip();
        let c = carried(&mut p.reader()).unwrap();
        assert_eq!((c.content_user, c.data.as_ref().map(Uri::as_str)), (10, Some("content://a.b/data")));
        let clip = c.clip.unwrap();
        assert_eq!(clip.len(), 2);
        assert!(matches!(&clip[0], ClipItem::Uri(u) if u.as_str() == "content://a.b/clip"));
        assert!(matches!(&clip[1], ClipItem::Intent(i) if i.data.as_ref().map(Uri::as_str) == Some("content://a.b/nested") && i.content_user == USER_CURRENT));
    }

    #[test]
    fn grants_keep_the_original_order() {
        let mut g = Grants::default();
        let uris: Vec<GrantUri> = ["content://a/1", "content://a/2", "content://a/3"].iter().map(|u| grant_uri(u, 0)).collect();
        for uri in &uris {
            g.find_or_create("s", "t", 10_001, uri);
        }
        let hashes: Vec<i32> = g.0[&10_001].iter().map(|p| p.uri.hash()).collect();
        assert!(hashes.is_sorted());
    }

    #[test]
    fn a_uri_permission_parcels_as_the_original() {
        let permission = PublicUriPermission { uri: Uri::parse("content://a.b/x"), modes: READ, persisted_time: 7 };
        let mut p = Parcel::new();
        permission.write_to(&mut p);
        let mut r = p.reader();
        assert_eq!(r.read_string16().unwrap().as_deref(), Some("android.net.Uri$StringUri"));
        assert_eq!(r.read_i32().unwrap(), 1); // `Uri.StringUri.TYPE_ID`
        assert_eq!(r.read_string8().unwrap().as_deref(), Some("content://a.b/x"));
        assert_eq!(r.read_i32().unwrap(), READ);
        assert_eq!(r.read_i64().unwrap(), 7);
    }
}
