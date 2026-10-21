//! `slice`: SliceManagerService for the app host (`android-16.0.0_r1`):
//! the pinned slices, each pin a listener kept until it unpins or its
//! binder dies, the provider told when its slice is first pinned and when
//! it is last unpinned (`PinnedSliceState`); the access of the default
//! home, the assistant and of what a provider or the user granted, kept in
//! `/data/system/slice` as `SlicePermissionManager` keeps it, one file per
//! client and per provider, written as KXmlSerializer writes them; a
//! package's grants go when its data is cleared (`mReceiver`). A removed
//! package's grants (the app host removes none, #1757), the shell command
//! and dump are #1941.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception,
    Parcel, Reader, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::android_app_slice_islicemanager as sm;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::content_service::ParcelUri;
use super::package_writes::{DataCleared, PackageMonitor};
use super::{AppHost, PER_USER_RANGE, reply, usage_events, usage_stats};

/// `UserHandle.USER_ALL`.
const USER_ALL: i32 = -1;
use crate::bundle::Value;
use crate::package::info::java_hash;
use crate::package::uri::{self, Uri};

/// `SlicePermissionManager.SLICE_DIR`, under the system directory.
const SLICE_DIR: &str = "system/slice";
/// `SlicePermissionManager.DB_VERSION`, of a backup's list.
const DB_VERSION: i32 = 2;
const TAG_LIST: &str = "slice-access-list";
const TAG_CLIENT: &str = "client";
const TAG_PROVIDER: &str = "provider";
const TAG_AUTHORITY: &str = "authority";
const TAG_PATH: &str = "path";
const TAG_PKG: &str = "pkg";
/// `SlicePermissionManager.WRITE_GRACE_PERIOD`: changes come in bursts.
const WRITE_GRACE_PERIOD: Duration = Duration::from_millis(500);
/// `SliceProvider.EXTRA_BIND_URI`, `METHOD_PIN`, `METHOD_UNPIN`.
const EXTRA_BIND_URI: &str = "slice_uri";
const METHOD_PIN: &str = "pin";
const METHOD_UNPIN: &str = "unpin";
const MANAGE_SLICE_PERMISSIONS: &str = "android.permission.MANAGE_SLICE_PERMISSIONS";
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
/// `Settings.Secure.ASSISTANT`.
const ASSISTANT: &str = "assistant";
/// `ApplicationInfo.FLAG_SYSTEM`.
const FLAG_SYSTEM: i32 = 1;
/// `PackageManager.PERMISSION_GRANTED`, `PERMISSION_DENIED`.
const PERMISSION_GRANTED: i32 = 0;
const PERMISSION_DENIED: i32 = -1;
/// `UserHandle.USER_CURRENT`, `USER_NULL`, `USER_SYSTEM`.
const USER_CURRENT: i32 = -2;
const USER_NULL: i32 = -10_000;
const USER_SYSTEM: i32 = 0;
const SYSTEM_UID: i32 = 1000;
/// `ContentResolver.NOTIFY_SYNC_TO_NETWORK`, of `notifyChange(uri, null)`.
const NOTIFY_SYNC_TO_NETWORK: i32 = 1;
/// `IBinder.DUMP_TRANSACTION`.
const DUMP_TRANSACTION: u32 = u32::from_be_bytes(*b"_DMP");

type Result<T> = std::result::Result<T, Exception>;

fn null_pointer(method: &str) -> Exception {
    Exception::new(
        EX_NULL_POINTER,
        format!("Attempt to invoke virtual method '{method}' on a null object reference"),
    )
}

fn null_uri() -> Exception {
    null_pointer("java.lang.String android.net.Uri.getAuthority()")
}

/// `PkgUser.hashCode` of a null package.
fn null_package() -> Exception {
    null_pointer("int java.lang.String.hashCode()")
}

fn not_modelled(e: impl fmt::Display) -> Exception {
    Exception::new(EX_UNSUPPORTED_OPERATION, format!("slice: {e}"))
}

// `ContentProvider`'s users of a URI's authority.

/// `getUserIdFromAuthority`: the user before the authority's last `@`,
/// `USER_NULL` if it is no number.
fn user_from_uri(uri: &Uri, default: i32) -> i32 {
    let Some(authority) = uri.authority() else {
        return default;
    };
    match authority.rfind('@') {
        Some(end) => authority[..end].parse().unwrap_or(USER_NULL),
        None => default,
    }
}

/// `getUriWithoutUserId`: the authority after its last `@`.
fn without_user(uri: &Uri) -> Uri {
    let authority = uri
        .authority()
        .map(|a| uri::encode(&a[a.rfind('@').map_or(0, |i| i + 1)..]));
    uri.with_encoded_authority(authority.as_deref())
}

/// `maybeAddUserId`: a `content` URI with no user of its own gets `user`.
fn with_user(uri: &Uri, user: i32) -> Uri {
    if user == USER_CURRENT
        || uri.scheme() != Some("content")
        || uri.user_info().is_some_and(|u| !u.is_empty())
    {
        return uri.clone();
    }
    let authority = format!("{user}@{}", uri.encoded_authority().unwrap_or("null"));
    uri.with_encoded_authority(Some(&authority))
}

/// `ComponentName.unflattenFromString`'s package.
fn component_package(flat: &str) -> Option<&str> {
    let sep = flat.find('/')?;
    (sep + 1 < flat.len()).then(|| &flat[..sep])
}

/// A parcelled `Uri`, written back as it was read.
struct SliceUri(Uri);

impl ReadParcelable for SliceUri {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        ParcelUri::read_from(r).map(|ParcelUri(u)| SliceUri(u))
    }
}

impl WriteParcelable for SliceUri {
    fn write_to(&self, p: &mut Parcel) {
        self.0.write(p);
    }
}

/// `SliceSpec`: its type and revision.
#[derive(Clone, Debug, PartialEq)]
struct Spec {
    kind: Option<String>,
    revision: i32,
}

impl ReadParcelable for Spec {
    fn read_from(r: &mut Reader<'_>) -> aim_binder_host::parcel::Result<Self> {
        Ok(Spec {
            kind: r.read_string16()?,
            revision: r.read_i32()?,
        })
    }
}

impl WriteParcelable for Spec {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(self.kind.as_deref());
        p.write_i32(self.revision);
    }
}

/// `PinnedSliceState.mergeSpecs`: of the specs kept, those `incoming` has
/// too, each at the lower of the two revisions.
fn merge_specs(
    kept: Option<Vec<Option<Spec>>>,
    incoming: Option<Vec<Option<Spec>>>,
) -> Result<Option<Vec<Option<Spec>>>> {
    let Some(kept) = kept else {
        return Ok(incoming);
    };
    let incoming = incoming.ok_or_else(|| null_pointer("int java.lang.Object.length"))?;
    let spec_type = || null_pointer("java.lang.String android.app.slice.SliceSpec.getType()");
    let mut merged = Vec::new();
    for s in kept {
        let s = s.ok_or_else(spec_type)?;
        let mut other = None;
        for o in &incoming {
            let o = o.as_ref().ok_or_else(spec_type)?;
            if o.kind == s.kind {
                other = Some(o);
                break;
            }
        }
        match other {
            Some(o) if o.revision < s.revision => merged.push(Some(o.clone())),
            Some(_) => merged.push(Some(s)),
            None => {}
        }
    }
    Ok(Some(merged))
}

// The permission store (`SlicePermissionManager`).

/// `SlicePermissionManager.PkgUser`: a package (or an authority) and a
/// user, written `pkg@user`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PkgUser {
    pkg: String,
    user: i32,
}

impl PkgUser {
    fn new(pkg: &str, user: i32) -> PkgUser {
        PkgUser {
            pkg: pkg.to_string(),
            user,
        }
    }

    /// `PkgUser(String)`: none for a string with no user after `@`.
    fn parse(s: &str) -> Option<PkgUser> {
        let (pkg, user) = s.split_once('@')?;
        Some(PkgUser::new(pkg, user.parse().ok()?))
    }

    fn hash(&self) -> i32 {
        java_hash(&self.pkg).wrapping_add(self.user)
    }
}

impl fmt::Display for PkgUser {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.pkg, self.user)
    }
}

/// `SliceClientPermissions.SliceAuthority`: the paths of a provider's
/// authority a client was granted.
#[derive(Clone, Debug, PartialEq)]
struct ClientAuthority {
    authority: String,
    provider: PkgUser,
    paths: Vec<Vec<String>>,
}

impl ClientAuthority {
    /// Its key in the client's map: the authority and its provider's user.
    fn key(&self) -> PkgUser {
        PkgUser::new(&self.authority, self.provider.user)
    }

    /// `addPath`: unless a granted prefix covers it, the paths it covers
    /// replaced by it.
    fn add_path(&mut self, path: Vec<String>) -> bool {
        if self.paths.iter().any(|p| is_prefix(p, &path)) {
            return false;
        }
        self.paths.retain(|p| !is_prefix(&path, p));
        self.paths.push(path);
        true
    }

    /// `removePath`: the paths it is a prefix of.
    fn remove_path(&mut self, path: &[String]) -> bool {
        let before = self.paths.len();
        self.paths.retain(|p| !is_prefix(path, p));
        self.paths.len() != before
    }
}

/// `isPathPrefixMatch`.
fn is_prefix(prefix: &[String], path: &[String]) -> bool {
    path.len() >= prefix.len() && prefix.iter().zip(path).all(|(a, b)| a == b)
}

/// `SliceClientPermissions`.
#[derive(Clone, Debug, PartialEq)]
struct Client {
    pkg: PkgUser,
    full_access: bool,
    /// As added; written in their map's order.
    auths: Vec<ClientAuthority>,
}

impl Client {
    fn new(pkg: PkgUser) -> Client {
        Client {
            pkg,
            full_access: false,
            auths: Vec::new(),
        }
    }

    /// `getOrCreateAuthority`, and whether it was created.
    fn authority(&mut self, authority: &str, provider: &PkgUser) -> (&mut ClientAuthority, bool) {
        let key = PkgUser::new(authority, provider.user);
        let at = match self.auths.iter().position(|a| a.key() == key) {
            Some(at) => (at, false),
            None => {
                self.auths.push(ClientAuthority {
                    authority: authority.to_string(),
                    provider: provider.clone(),
                    paths: Vec::new(),
                });
                (self.auths.len() - 1, true)
            }
        };
        (&mut self.auths[at.0], at.1)
    }

    /// `hasPermission` of a URI without its user.
    fn has_permission(&self, uri: &Uri, user: i32) -> bool {
        if uri.scheme() != Some("content") {
            return false;
        }
        let key = PkgUser::new(&uri.authority().unwrap_or_default(), user);
        let path = uri.path_segments();
        self.auths
            .iter()
            .find(|a| a.key() == key)
            .is_some_and(|a| a.paths.iter().any(|p| is_prefix(p, &path)))
    }

    fn write(&self, x: &mut Kxml) {
        x.start(
            TAG_CLIENT,
            &[
                ("pkg", &self.pkg.to_string()),
                ("fullAccess", if self.full_access { "1" } else { "0" }),
            ],
        );
        let mut auths: Vec<_> = self.auths.iter().collect();
        auths.sort_by_key(|a| a.key().hash());
        for a in auths {
            x.start(
                TAG_AUTHORITY,
                &[
                    ("authority", &a.authority),
                    ("pkg", &a.provider.to_string()),
                ],
            );
            for p in &a.paths {
                x.start(TAG_PATH, &[]);
                let encoded: Vec<String> = p.iter().map(|s| uri::encode(s)).collect();
                x.text(&encoded.join("/"));
                x.end();
            }
            x.end();
        }
        x.end();
    }

    /// `createFrom`: an authority whose provider is no `pkg@user` is
    /// skipped, as the original logs it.
    fn read(e: &aim_android_xml::Element) -> std::result::Result<Client, String> {
        let pkg = e
            .string("pkg")
            .and_then(|p| PkgUser::parse(&p))
            .ok_or("bad client package")?;
        let full_access = match e.string("fullAccess") {
            Some(v) => v.parse::<i32>().map_err(|e| e.to_string())? != 0,
            None => false,
        };
        let mut client = Client::new(pkg);
        client.full_access = full_access;
        for a in e.children().filter(|c| c.name == TAG_AUTHORITY) {
            let Some(provider) = a.string("pkg").and_then(|p| PkgUser::parse(&p)) else {
                eprintln!("app host: slice: Couldn't read PkgUser");
                continue;
            };
            let paths = a
                .children()
                .filter(|c| c.name == TAG_PATH)
                .map(|c| text(c).split('/').map(uri::decode).collect())
                .collect();
            let authority = ClientAuthority {
                authority: a.string("authority").unwrap_or_default().into_owned(),
                provider,
                paths,
            };
            client.auths.retain(|o| o.key() != authority.key());
            client.auths.push(authority);
        }
        Ok(client)
    }
}

/// `SliceProviderPermissions`: by authority, the clients granted some of
/// it.
#[derive(Clone, Debug, PartialEq)]
struct Provider {
    pkg: PkgUser,
    auths: Vec<(String, Vec<PkgUser>)>,
}

impl Provider {
    fn new(pkg: PkgUser) -> Provider {
        Provider {
            pkg,
            auths: Vec::new(),
        }
    }

    /// `getOrCreateAuthority(authority).addPkg(client)`: whether it changed.
    fn add(&mut self, authority: &str, client: &PkgUser) -> bool {
        let at = match self.auths.iter().position(|(a, _)| a == authority) {
            Some(at) => at,
            None => {
                self.auths.push((authority.to_string(), Vec::new()));
                self.auths.len() - 1
            }
        };
        let pkgs = &mut self.auths[at].1;
        if pkgs.contains(client) {
            return false;
        }
        pkgs.push(client.clone());
        true
    }

    fn write(&self, x: &mut Kxml) {
        x.start(TAG_PROVIDER, &[("pkg", &self.pkg.to_string())]);
        let mut auths: Vec<_> = self.auths.iter().collect();
        auths.sort_by_key(|(a, _)| java_hash(a));
        for (authority, pkgs) in auths {
            x.start(TAG_AUTHORITY, &[("authority", authority)]);
            let mut pkgs: Vec<_> = pkgs.iter().collect();
            pkgs.sort_by_key(|p| p.hash());
            for p in pkgs {
                x.start(TAG_PKG, &[]);
                x.text(&p.to_string());
                x.end();
            }
            x.end();
        }
        x.end();
    }

    /// `createFrom`: a client that is no `pkg@user` ends the read, as the
    /// original's exception does.
    fn read(e: &aim_android_xml::Element) -> std::result::Result<Provider, String> {
        let pkg = e
            .string("pkg")
            .and_then(|p| PkgUser::parse(&p))
            .ok_or("bad provider package")?;
        let mut provider = Provider::new(pkg);
        for a in e.children().filter(|c| c.name == TAG_AUTHORITY) {
            let mut pkgs = Vec::new();
            for p in a.children().filter(|c| c.name == TAG_PKG) {
                let p = PkgUser::parse(&text(p)).ok_or("bad provider client")?;
                if !pkgs.contains(&p) {
                    pkgs.push(p);
                }
            }
            let authority = a.string("authority").unwrap_or_default().into_owned();
            provider.auths.retain(|(o, _)| *o != authority);
            provider.auths.push((authority, pkgs));
        }
        Ok(provider)
    }
}

/// An element's text (`nextText`).
fn text(e: &aim_android_xml::Element) -> String {
    e.content
        .iter()
        .filter_map(|n| match n {
            aim_android_xml::Node::Token(
                aim_android_xml::TEXT | aim_android_xml::CDSECT | aim_android_xml::ENTITY_REF,
                Some(t),
            ) => Some(t.as_str()),
            _ => None,
        })
        .collect()
}

/// Which file a change dirtied (`Persistable.getFileName`).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Persisted {
    Client(PkgUser),
    Provider(PkgUser),
}

impl Persisted {
    fn file_name(&self) -> String {
        match self {
            Persisted::Client(p) => format!("client_{p}"),
            Persisted::Provider(p) => format!("provider_{p}"),
        }
    }
}

/// `SlicePermissionManager`'s clients and providers, read when first
/// needed, and the changes not yet written.
struct Store {
    dir: PathBuf,
    clients: HashMap<PkgUser, Client>,
    providers: HashMap<PkgUser, Provider>,
    dirty: Vec<Persisted>,
}

impl Store {
    fn new(dir: PathBuf) -> Store {
        Store {
            dir,
            clients: HashMap::new(),
            providers: HashMap::new(),
            dirty: Vec::new(),
        }
    }

    /// `onPersistableDirty`.
    fn dirty(&mut self, p: Persisted) {
        if !self.dirty.contains(&p) {
            self.dirty.push(p);
        }
    }

    /// `getClient`: as read, a clean one if there is none or it cannot be.
    fn client(&mut self, pkg: &PkgUser) -> &mut Client {
        if !self.clients.contains_key(pkg) {
            let name = Persisted::Client(pkg.clone()).file_name();
            let client = match read_document(&self.dir.join(&name)) {
                Ok(Some(e)) => find(&e, TAG_CLIENT)
                    .ok_or_else(|| "Can't find client tag in xml".to_string())
                    .and_then(Client::read),
                Ok(None) => Err(String::new()),
                Err(e) => Err(e),
            };
            let client = client.unwrap_or_else(|e| {
                if !e.is_empty() {
                    eprintln!("app host: slice: Can't read client: {e}");
                }
                Client::new(pkg.clone())
            });
            self.clients.insert(pkg.clone(), client);
        }
        self.clients.get_mut(pkg).unwrap()
    }

    /// `getProvider`, as `getClient` gets a client.
    fn provider(&mut self, pkg: &PkgUser) -> &mut Provider {
        if !self.providers.contains_key(pkg) {
            let name = Persisted::Provider(pkg.clone()).file_name();
            let provider = match read_document(&self.dir.join(&name)) {
                Ok(Some(e)) => find(&e, TAG_PROVIDER)
                    .ok_or_else(|| "Can't find provider tag in xml".to_string())
                    .and_then(Provider::read),
                Ok(None) => Err(String::new()),
                Err(e) => Err(e),
            };
            let provider = provider.unwrap_or_else(|e| {
                if !e.is_empty() {
                    eprintln!("app host: slice: Can't read provider: {e}");
                }
                Provider::new(pkg.clone())
            });
            self.providers.insert(pkg.clone(), provider);
        }
        self.providers.get_mut(pkg).unwrap()
    }

    fn grant_full_access(&mut self, pkg: &str, user: i32) {
        let pkg = PkgUser::new(pkg, user);
        let client = self.client(&pkg);
        if !client.full_access {
            client.full_access = true;
            self.dirty(Persisted::Client(pkg));
        }
    }

    /// `grantSliceAccess`: the client's path, then the provider's client;
    /// a null provider fails there, as the original's `hashCode` does.
    fn grant(
        &mut self,
        pkg: &str,
        user: i32,
        provider: Option<&str>,
        provider_user: i32,
        uri: &Uri,
    ) -> Result<()> {
        let client = PkgUser::new(pkg, user);
        let provider_pkg = PkgUser::new(provider.unwrap_or("null"), provider_user);
        let authority = uri.authority().unwrap_or_default();
        let (auth, created) = self.client(&client).authority(&authority, &provider_pkg);
        let added = auth.add_path(uri.path_segments());
        if created || added {
            self.dirty(Persisted::Client(client.clone()));
        }
        if provider.is_none() {
            return Err(null_package());
        }
        let authority = without_user(uri).authority().unwrap_or_default();
        if self.provider(&provider_pkg).add(&authority, &client) {
            self.dirty(Persisted::Provider(provider_pkg));
        }
        Ok(())
    }

    /// `revokeSliceAccess`: the client's paths under it.
    fn revoke(
        &mut self,
        pkg: &str,
        user: i32,
        provider: Option<&str>,
        provider_user: i32,
        uri: &Uri,
    ) {
        let client = PkgUser::new(pkg, user);
        let provider_pkg = PkgUser::new(provider.unwrap_or("null"), provider_user);
        let authority = uri.authority().unwrap_or_default();
        let (auth, created) = self.client(&client).authority(&authority, &provider_pkg);
        let removed = auth.remove_path(&uri.path_segments());
        if created || removed {
            self.dirty(Persisted::Client(client));
        }
    }

    /// `removePkg`: the package's authorities taken from the clients they
    /// were granted to, then (`handleRemove`) its files deleted and its
    /// client and provider forgotten with their unwritten changes.
    fn remove_pkg(&mut self, pkg: &str, user: i32) {
        let pkg_user = PkgUser::new(pkg, user);
        for (authority, clients) in self.provider(&pkg_user).auths.clone() {
            let key = PkgUser::new(&authority, user);
            for c in clients {
                let client = self.client(&c);
                let before = client.auths.len();
                client.auths.retain(|a| a.key() != key);
                if client.auths.len() != before {
                    self.dirty(Persisted::Client(c));
                }
            }
        }
        let own = [
            Persisted::Client(pkg_user.clone()),
            Persisted::Provider(pkg_user.clone()),
        ];
        for p in &own {
            let file = self.dir.join(p.file_name());
            if let Err(e) = std::fs::remove_file(&file)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                eprintln!("app host: slice: deleting {}: {e}", file.display());
            }
        }
        self.clients.remove(&pkg_user);
        self.providers.remove(&pkg_user);
        self.dirty.retain(|p| !own.contains(p));
    }

    fn has_full_access(&mut self, pkg: &str, user: i32) -> bool {
        self.client(&PkgUser::new(pkg, user)).full_access
    }

    fn has_permission(&mut self, pkg: &str, user: i32, uri: &Uri) -> bool {
        let provider_user = user_from_uri(uri, user);
        let client = self.client(&PkgUser::new(pkg, user));
        client.full_access || client.has_permission(&without_user(uri), provider_user)
    }

    /// `handlePersist`: each changed client and provider written, as
    /// `AtomicFile` writes it; a failure is logged.
    fn persist(&mut self) {
        for p in std::mem::take(&mut self.dirty) {
            let mut x = Kxml::default();
            match &p {
                Persisted::Client(pkg) => self.clients[pkg].write(&mut x),
                Persisted::Provider(pkg) => self.providers[pkg].write(&mut x),
            }
            if let Err(e) = write_atomic(&self.dir, &p.file_name(), x.0.as_bytes()) {
                eprintln!("app host: slice: Failed to save access file: {e}");
            }
        }
    }

    /// `writeBackup`: the list of every file's client or provider, the
    /// changes written first.
    fn backup(&mut self) -> std::result::Result<Vec<u8>, String> {
        self.persist();
        let mut x = Kxml::default();
        x.start(TAG_LIST, &[("version", &DB_VERSION.to_string())]);
        let mut names: Vec<_> = match std::fs::read_dir(&self.dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(e) => return Err(e.to_string()),
        };
        names.sort();
        for name in names {
            let Some(e) = read_document(&self.dir.join(&name))? else {
                eprintln!("app host: slice: Invalid or empty slice permissions file: {name}");
                continue;
            };
            if e.name == TAG_CLIENT {
                Client::read(&e)?.write(&mut x);
            } else {
                let e = find(&e, TAG_PROVIDER).ok_or("Can't find provider tag in xml")?;
                Provider::read(e)?.write(&mut x);
            }
        }
        x.end();
        Ok(x.0.into_bytes())
    }

    /// `readRestore`: of a list at the current version, each client and
    /// provider, replacing what is kept and written.
    fn restore(&mut self, payload: &[u8]) -> std::result::Result<(), String> {
        let root = aim_android_xml::read(payload)?;
        let Some(list) = find(&root, TAG_LIST) else {
            return Ok(());
        };
        let version = list
            .string("version")
            .and_then(|v| v.parse::<i32>().ok())
            .unwrap_or(0);
        if version < DB_VERSION {
            return Ok(());
        }
        self.restore_children(list)
    }

    fn restore_children(
        &mut self,
        e: &aim_android_xml::Element,
    ) -> std::result::Result<(), String> {
        for c in e.children() {
            match c.name.as_str() {
                TAG_CLIENT => {
                    let client = Client::read(c)?;
                    let pkg = client.pkg.clone();
                    self.clients.insert(pkg.clone(), client);
                    self.dirty(Persisted::Client(pkg));
                }
                TAG_PROVIDER => {
                    let provider = Provider::read(c)?;
                    let pkg = provider.pkg.clone();
                    self.providers.insert(pkg.clone(), provider);
                    self.dirty(Persisted::Provider(pkg));
                }
                _ => self.restore_children(c)?,
            }
        }
        Ok(())
    }
}

/// The first element named `name`, in document order.
fn find<'a>(e: &'a aim_android_xml::Element, name: &str) -> Option<&'a aim_android_xml::Element> {
    if e.name == name {
        return Some(e);
    }
    e.children().find_map(|c| find(c, name))
}

/// The document of `path` as `AtomicFile.openRead` reads it (its legacy
/// backup in its place, a new file beside it dropped); none without it.
fn read_document(path: &Path) -> std::result::Result<Option<aim_android_xml::Element>, String> {
    let with = |suffix: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    let backup = with(".bak");
    if backup.exists() {
        std::fs::rename(&backup, path).map_err(|e| e.to_string())?;
    }
    let new = with(".new");
    if new.exists() && path.exists() {
        let _ = std::fs::remove_file(&new);
    }
    match std::fs::read(path) {
        Ok(bytes) => aim_android_xml::read(&bytes).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Writes `name` in `dir` as `AtomicFile` writes it, making `dir` as
/// `File.mkdir` does: the system's, 0700, its files 0600.
pub(super) fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let system = aim_storage::guest_inode::GuestInode {
        uid: Some(1000),
        gid: Some(1000),
        mode: Some(0o700),
    };
    if !dir.is_dir() {
        std::fs::create_dir(dir)?;
        aim_storage::guest_inode::record(dir, system)?;
    }
    let path = dir.join(name);
    let backup = dir.join(format!("{name}.bak"));
    if backup.exists() {
        std::fs::rename(&backup, &path)?;
    }
    let new = dir.join(format!("{name}.new"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&new)?;
    std::io::Write::write_all(&mut file, bytes)?;
    crate::fsync(&file)?;
    aim_storage::guest_inode::record(
        &new,
        aim_storage::guest_inode::GuestInode {
            mode: Some(0o600),
            ..system
        },
    )?;
    std::fs::rename(&new, &path)
}

/// What KXmlSerializer writes, with no declaration and no indent: an
/// element with no content closed with ` />`.
#[derive(Default)]
struct Kxml(String, Vec<&'static str>, bool);

impl Kxml {
    fn close_start(&mut self) {
        if self.2 {
            self.0.push('>');
            self.2 = false;
        }
    }

    fn start(&mut self, name: &'static str, attrs: &[(&str, &str)]) {
        self.close_start();
        self.0.push('<');
        self.0.push_str(name);
        for (n, v) in attrs {
            self.0.push(' ');
            self.0.push_str(n);
            self.0.push_str("=\"");
            escape(v, true, &mut self.0);
            self.0.push('"');
        }
        self.1.push(name);
        self.2 = true;
    }

    fn text(&mut self, text: &str) {
        self.close_start();
        escape(text, false, &mut self.0);
    }

    fn end(&mut self) {
        let name = self.1.pop().unwrap();
        if self.2 {
            self.0.push_str(" />");
            self.2 = false;
        } else {
            self.0.push_str("</");
            self.0.push_str(name);
            self.0.push('>');
        }
    }
}

/// `KXmlSerializer.writeEscaped`: markup characters, and in an attribute
/// its quote and whitespace other than a space, as references.
fn escape(s: &str, attribute: bool, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\n' | '\r' | '\t' if attribute => out.push_str(&format!("&#{};", c as u32)),
            c => out.push(c),
        }
    }
}

// The pinned slices (`PinnedSliceState`).

/// A pin's token, linked to its death when it is another process's.
struct Listener {
    token: Option<Binder>,
    link: Option<(Strong, u64)>,
}

struct Pinned {
    uri: Uri,
    pkg: Option<String>,
    listeners: Vec<Listener>,
    specs: Option<Vec<Option<Spec>>>,
    pinned: bool,
}

/// What the service's handler thread does, in order.
enum Work {
    /// The provider told its slice is pinned, or unpinned.
    Notify(Uri, bool),
    /// `reportEvent` of a slice pinned by another package.
    Usage(String, i32),
    /// A change to write after the grace period.
    Persist,
    /// `ACTION_PACKAGE_DATA_CLEARED` of a package in a user.
    DataCleared(String, i32),
}

struct State {
    /// As pinned; kept in their map's order (`ArrayMap<Uri, ...>`).
    pinned: Vec<Pinned>,
    store: Store,
}

impl State {
    /// `checkSelfRemove`: the slice dropped once it has no listener
    /// (`removePinnedSlice`); whether its provider is to be told
    /// (`destroy`).
    fn remove_if_unpinned(&mut self, at: usize) -> bool {
        if !self.pinned[at].listeners.is_empty() {
            return false;
        }
        self.pinned.remove(at).pinned
    }
}

pub struct SliceManager {
    host: Arc<AppHost>,
    state: Mutex<Option<State>>,
    work: Mutex<Option<Sender<Work>>>,
    me: Weak<SliceManager>,
}

impl SliceManager {
    pub fn new(host: Arc<AppHost>) -> Arc<SliceManager> {
        let service = Arc::new_cyclic(|me| SliceManager {
            host,
            state: Mutex::new(None),
            work: Mutex::new(None),
            me: me.clone(),
        });
        // `registerReceiverAsUser(mReceiver, UserHandle.ALL, ...)`.
        let receiver = service.clone() as Arc<dyn PackageMonitor>;
        service
            .host
            .package_monitors
            .register(USER_ALL, Arc::downgrade(&receiver));
        service
    }

    fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> Result<T> {
        let mut state = self.state.lock().unwrap();
        if state.is_none() {
            let dir = self.host.data()?.join(SLICE_DIR);
            *state = Some(State {
                pinned: Vec::new(),
                store: Store::new(dir),
            });
        }
        let state = state.as_mut().unwrap();
        let dirty = state.store.dirty.len();
        let result = f(state);
        if state.store.dirty.len() != dirty {
            self.post(Work::Persist);
        }
        Ok(result)
    }

    /// Posts to the service's handler thread, started when first needed.
    fn post(&self, work: Work) {
        let mut sender = self.work.lock().unwrap();
        let tx = sender.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel();
            let me = self.me.clone();
            std::thread::spawn(move || handler(me, rx));
            tx
        });
        if tx.send(work).is_err() {
            eprintln!("app host: slice: the handler thread has ended");
        }
    }

    /// `AppOpsManager.checkPackage`.
    fn verify_caller(&self, uid: i32, pkg: Option<&str>) -> Result<()> {
        let owns = match pkg {
            Some(p) => self.host.owns_package(uid, p)?,
            None => false,
        };
        if !owns {
            return Err(Exception::security(format!(
                "Package {} does not belong to {uid}",
                pkg.unwrap_or("null")
            )));
        }
        Ok(())
    }

    /// `getProviderPkg`: the package of the provider of the URI's
    /// authority, as the system resolves it.
    fn provider_pkg(&self, uri: &Uri, user: i32) -> Result<Option<String>> {
        let Some(authority) = without_user(uri).authority() else {
            return Ok(None);
        };
        self.host
            .provider_package(&authority, user_from_uri(uri, user))
    }

    fn enforce_owner(&self, pkg: Option<&str>, uri: &Uri, user: i32) -> Result<()> {
        let owner = self.provider_pkg(uri, user)?;
        if pkg.is_none() || owner.as_deref() != pkg {
            return Err(Exception::security(format!(
                "Caller must own {}",
                uri.as_str()
            )));
        }
        Ok(())
    }

    /// `enforceCrossUser`.
    fn enforce_cross_user(&self, uri: &Uri, uid: i32) -> Result<()> {
        let user = uid / PER_USER_RANGE as i32;
        if user_from_uri(uri, user) != user
            && self
                .host
                .check_permission(Some(INTERACT_ACROSS_USERS_FULL), uid)?
                != 0
        {
            return Err(Exception::security(format!(
                "Slice interaction across users requires INTERACT_ACROSS_USERS_FULL: Neither user {uid} nor current process has {INTERACT_ACROSS_USERS_FULL}."
            )));
        }
        Ok(())
    }

    /// `enforceAccess`: the access, or the provider's own package.
    fn enforce_access(&self, pkg: &str, uri: &Uri, uid: i32) -> Result<()> {
        if self.check(uri, None, Some(pkg), uid, uid, None)? != PERMISSION_GRANTED {
            let user = user_from_uri(uri, uid / PER_USER_RANGE as i32);
            if self.provider_pkg(uri, user)?.as_deref() != Some(pkg) {
                return Err(Exception::security(format!(
                    "Access to slice {} is required",
                    uri.as_str()
                )));
            }
        }
        self.enforce_cross_user(uri, uid)
    }

    /// `checkSlicePermissionInternal` of `uid`'s packages (`pkg`, or each of
    /// them); a permission `auto_grant` names, held by `uid`, grants the
    /// slice when `calling_pkg`, `caller`'s, owns it.
    fn check(
        &self,
        uri: &Uri,
        calling_pkg: Option<&str>,
        pkg: Option<&str>,
        uid: i32,
        caller: i32,
        auto_grant: Option<&[Option<String>]>,
    ) -> Result<i32> {
        let user = uid / PER_USER_RANGE as i32;
        let Some(pkg) = pkg else {
            let packages = self
                .packages_for_uid(uid)?
                .ok_or_else(|| null_pointer("int java.lang.Object.length"))?;
            for p in packages.iter().flatten() {
                if self.check(uri, calling_pkg, Some(p), uid, caller, auto_grant)?
                    == PERMISSION_GRANTED
                {
                    return Ok(PERMISSION_GRANTED);
                }
            }
            return Ok(PERMISSION_DENIED);
        };
        if self.has_full_access(pkg, user)?
            || self.with(|s| s.store.has_permission(pkg, user, uri))?
        {
            return Ok(PERMISSION_GRANTED);
        }
        if let (Some(permissions), Some(calling_pkg)) = (auto_grant, calling_pkg) {
            self.enforce_owner(Some(calling_pkg), uri, user)?;
            self.verify_caller(caller, Some(calling_pkg))?;
            for perm in permissions {
                let perm = perm
                    .as_deref()
                    .ok_or_else(|| Exception::illegal_argument("permission is null"))?;
                if self.host.check_permission(Some(perm), uid)? == PERMISSION_GRANTED {
                    let provider_user = user_from_uri(uri, user);
                    let provider = self.provider_pkg(uri, provider_user)?;
                    self.with(|s| {
                        s.store
                            .grant(pkg, user, provider.as_deref(), provider_user, uri)
                    })??;
                    return Ok(PERMISSION_GRANTED);
                }
            }
        }
        Ok(PERMISSION_DENIED)
    }

    /// `getPackagesForUid`.
    fn packages_for_uid(&self, uid: i32) -> Result<Option<Vec<Option<String>>>> {
        let (state, resolution) = self.host.packages()?;
        let query = crate::package::query::Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        query
            .packages_for_uid(uid)
            .map_err(|e| not_modelled(format!("getPackagesForUid: {}", e.0)))
    }

    /// `hasFullSliceAccess`: the default home's, the assistant's, or
    /// granted.
    fn has_full_access(&self, pkg: &str, user: i32) -> Result<bool> {
        Ok(self.default_home(user)?.as_deref() == Some(pkg)
            || self.assistant(user)?.as_deref() == Some(pkg)
            || self.with(|s| s.store.has_full_access(pkg, user))?)
    }

    /// `AssistUtils.getAssistComponentForUser`'s package: the user's
    /// `assistant` setting. The app host keeps user 0's settings only.
    fn assistant(&self, user: i32) -> Result<Option<String>> {
        if user != USER_SYSTEM {
            return Ok(None);
        }
        let setting = self
            .host
            .inputs()?
            .settings
            .get(super::settings::Table::Secure, ASSISTANT);
        Ok(setting.and_then(|s| component_package(&s).map(str::to_string)))
    }

    /// `getDefaultHome`: the default home activity's package, else the
    /// home of the system app of the highest priority, the last of
    /// equals.
    fn default_home(&self, user: i32) -> Result<Option<String>> {
        let (state, resolution) = self.host.packages()?;
        let query = crate::package::query::Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID,
        };
        let plan = crate::package::query::preferred::home_plan(&query, user)
            .map_err(|e| not_modelled(e.0))?;
        if let Some(chosen) = plan.chosen {
            return Ok(Some(chosen.package));
        }
        let mut detected = None;
        let mut last_priority = i32::MIN;
        for ri in &plan.candidates {
            let crate::package::component_resolver::Info::Activity(a) = &ri.info else {
                continue;
            };
            if a.info.application_info.flags & FLAG_SYSTEM == 0 || ri.priority < last_priority {
                continue;
            }
            detected = Some(ri.component().0.to_string());
            last_priority = ri.priority;
        }
        Ok(detected)
    }

    /// `getPinnedSlices`: the caller's pins of the caller's user, without
    /// their user.
    fn pinned_slices(&self, pkg: &str, uid: i32) -> Result<Vec<Option<SliceUri>>> {
        let user = uid / PER_USER_RANGE as i32;
        self.with(|s| {
            let mut pinned: Vec<_> = s.pinned.iter().collect();
            pinned.sort_by_key(|p| java_hash(p.uri.as_str()));
            pinned
                .into_iter()
                .filter(|p| p.pkg.as_deref() == Some(pkg) && user_from_uri(&p.uri, user) == user)
                .map(|p| Some(SliceUri(without_user(&p.uri))))
                .collect()
        })
    }

    /// `pinSlice`.
    fn pin(
        &self,
        pkg: &str,
        uri: &Uri,
        specs: Option<Vec<Option<Spec>>>,
        token: Option<Binder>,
        uid: i32,
    ) -> Result<()> {
        self.enforce_access(pkg, uri, uid)?;
        let user = uid / PER_USER_RANGE as i32;
        let uri = with_user(uri, user);
        let slice_pkg = self.provider_pkg(&uri, user)?;
        let link = match token {
            Some(Binder::Handle(handle)) => {
                let strong = self.host.process.strong(handle);
                let me = self.me.clone();
                let key = uri.clone();
                let death = self.host.process.link_to_death(
                    &strong,
                    Box::new(move || {
                        if let Some(service) = me.upgrade() {
                            service.token_died(&key, handle);
                        }
                    }),
                );
                Some((strong, death))
            }
            _ => None,
        };
        let notify = self.with(|s| {
            let at = match s.pinned.iter().position(|p| p.uri.as_str() == uri.as_str()) {
                Some(at) => at,
                None => {
                    s.pinned.push(Pinned {
                        uri: uri.clone(),
                        pkg: slice_pkg.clone(),
                        listeners: Vec::new(),
                        specs: None,
                        pinned: false,
                    });
                    s.pinned.len() - 1
                }
            };
            let p = &mut s.pinned[at];
            if let Some(old) = p.listeners.iter().position(|l| l.token == token) {
                let old = p.listeners.remove(old);
                if let Some((strong, death)) = old.link {
                    self.host.process.clear_death(&strong, death);
                }
            }
            p.listeners.push(Listener { token, link });
            if token.is_none() {
                return Err(null_pointer(
                    "void android.os.IBinder.linkToDeath(android.os.IBinder$DeathRecipient, int)",
                ));
            }
            p.specs = merge_specs(p.specs.take(), specs)?;
            Ok(!std::mem::replace(&mut p.pinned, true))
        })??;
        if notify {
            self.post(Work::Notify(uri.clone(), true));
        }
        if let Some(slice_pkg) = slice_pkg
            && slice_pkg != pkg
        {
            let event = if self.assistant(user)?.as_deref() == Some(pkg)
                || self.default_home(user)?.as_deref() == Some(pkg)
            {
                usage_events::SLICE_PINNED_PRIV
            } else {
                usage_events::SLICE_PINNED
            };
            if user == USER_SYSTEM {
                self.post(Work::Usage(slice_pkg, event));
            }
        }
        Ok(())
    }

    /// `unpinSlice`: the token's pin, and the slice once it has none. A
    /// token of another process that never pinned it fails as its
    /// `unlinkToDeath` does.
    fn unpin(&self, pkg: &str, uri: &Uri, token: Option<Binder>, uid: i32) -> Reply {
        if let Err(e) = self.enforce_access(pkg, uri, uid) {
            return reply(Err(e));
        }
        let uri = with_user(uri, uid / PER_USER_RANGE as i32);
        let unpinned = self.with(|s| {
            let Some(at) = s.pinned.iter().position(|p| p.uri.as_str() == uri.as_str()) else {
                eprintln!("app host: slice: Slice {} not pinned", uri.as_str());
                return Ok(false);
            };
            let p = &mut s.pinned[at];
            match p.listeners.iter().position(|l| l.token == token) {
                Some(i) => {
                    if let Some((strong, death)) = p.listeners.remove(i).link {
                        self.host.process.clear_death(&strong, death);
                    }
                }
                None if token.is_none() => {
                    return Err(Some(null_pointer(
                        "boolean android.os.IBinder.unlinkToDeath(android.os.IBinder$DeathRecipient, int)",
                    )));
                }
                None if matches!(token, Some(Binder::Handle(_))) => return Err(None),
                None => {}
            }
            Ok(s.remove_if_unpinned(at))
        });
        match unpinned {
            Err(e) | Ok(Err(Some(e))) => reply(Err(e)),
            Ok(Err(None)) => Err(UNKNOWN_TRANSACTION),
            Ok(Ok(notify)) => {
                if notify {
                    self.post(Work::Notify(uri, false));
                }
                let mut p = Parcel::new();
                sm::write_unpin_slice_reply(&mut p);
                Ok(p)
            }
        }
    }

    /// `handleRecheckListeners`: the dead token's pins dropped, and the
    /// slice once it has none.
    fn token_died(&self, uri: &Uri, handle: u32) {
        let removed = self.with(|s| {
            let Some(at) = s.pinned.iter().position(|p| p.uri.as_str() == uri.as_str()) else {
                return false;
            };
            s.pinned[at]
                .listeners
                .retain(|l| l.token != Some(Binder::Handle(handle)));
            s.remove_if_unpinned(at)
        });
        if let Ok(true) = removed {
            self.post(Work::Notify(uri.clone(), false));
        }
    }

    /// `getPinnedSpecs`.
    fn pinned_specs(&self, pkg: &str, uri: &Uri, uid: i32) -> Result<Option<Vec<Option<Spec>>>> {
        self.enforce_access(pkg, uri, uid)?;
        let uri = with_user(uri, uid / PER_USER_RANGE as i32);
        self.with(|s| {
            s.pinned
                .iter()
                .find(|p| p.uri.as_str() == uri.as_str())
                .map(|p| p.specs.clone())
                .ok_or_else(|| {
                    Exception::new(
                        EX_ILLEGAL_STATE,
                        format!("Slice {} not pinned", uri.as_str()),
                    )
                })
        })?
    }

    /// `grantPermissionFromUser`: full access, or the provider's every
    /// slice; then the URI's observers told.
    fn grant_from_user(
        &self,
        uri: &Uri,
        pkg: Option<&str>,
        all_slices: bool,
        uid: i32,
    ) -> Result<()> {
        if self
            .host
            .check_permission(Some(MANAGE_SLICE_PERMISSIONS), uid)?
            != 0
        {
            return Err(Exception::security(format!(
                "Slice granting requires MANAGE_SLICE_PERMISSIONS: Neither user {uid} nor current process has {MANAGE_SLICE_PERMISSIONS}."
            )));
        }
        let user = uid / PER_USER_RANGE as i32;
        let pkg = pkg.ok_or_else(null_package)?;
        if all_slices {
            self.with(|s| s.store.grant_full_access(pkg, user))?;
        } else {
            let grant = uri.with_empty_path();
            let provider_user = user_from_uri(&grant, user);
            let provider = self.provider_pkg(&grant, provider_user)?;
            self.with(|s| {
                s.store
                    .grant(pkg, user, provider.as_deref(), provider_user, &grant)
            })??;
        }
        if let Some(content) = self.host.content_service.get()
            && let Err(e) = content.notify_from_system(uri.clone(), NOTIFY_SYNC_TO_NETWORK, 0)
        {
            eprintln!("app host: slice: notifyChange: {}", e.message);
        }
        Ok(())
    }

    /// `handleSendPinned`, `handleSendUnpinned`: the provider called as the
    /// system, outside the service's lock.
    fn notify(&self, uri: &Uri, pinned: bool) {
        let mut data = Parcel::new();
        uri.write(&mut data);
        let extras = [(
            EXTRA_BIND_URI,
            Value::Parcelable {
                class: uri.class_name().to_string(),
                data: data.data().to_vec(),
            },
        )];
        let method = if pinned { METHOD_PIN } else { METHOD_UNPIN };
        if let Err(e) = self.host.call_provider(uri, method, &extras) {
            eprintln!("app host: slice: Unable to contact {}: {e}", uri.as_str());
        }
    }

    /// `UsageStatsManagerInternal.reportEvent(pkg, user, event_type)`.
    fn report(&self, pkg: String, event_type: i32) {
        self.host.report_usage_stats_event(usage_events::Event {
            package: pkg,
            class: None,
            instance_id: 0,
            task_root_package: None,
            task_root_class: None,
            event_type,
            time_stamp: usage_stats::now_millis(),
            flags: 0,
            extra: usage_events::Extra::None,
        });
    }

    /// `handlePersist`.
    fn persist(&self) {
        if let Some(state) = self.state.lock().unwrap().as_mut() {
            state.store.persist();
        }
    }
}

/// The service's handler thread: work in order, the store written once no
/// change came for the grace period.
fn handler(me: Weak<SliceManager>, rx: mpsc::Receiver<Work>) {
    let mut persist_at: Option<Instant> = None;
    loop {
        let next = match persist_at {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let Some(service) = me.upgrade() else {
            return;
        };
        match next {
            Ok(Work::Persist) => persist_at = Some(Instant::now() + WRITE_GRACE_PERIOD),
            Ok(Work::Notify(uri, pinned)) => service.notify(&uri, pinned),
            Ok(Work::Usage(pkg, event_type)) => service.report(pkg, event_type),
            Ok(Work::DataCleared(pkg, user)) => {
                if let Err(e) = service.with(|s| s.store.remove_pkg(&pkg, user)) {
                    eprintln!("app host: slice: {pkg}'s grants: {}", e.message);
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                persist_at = None;
                service.persist();
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

impl PackageMonitor for SliceManager {
    /// `mReceiver` on the service's handler: the package's grants removed
    /// (`SlicePermissionManager.removePkg`).
    fn package_data_cleared(&self, cleared: &DataCleared<'_>) {
        self.post(Work::DataCleared(cleared.package.into(), cleared.user));
    }
}

impl Service for SliceManager {
    fn descriptor(&self) -> &str {
        sm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let user = uid / PER_USER_RANGE as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            sm::GET_PINNED_SLICES => {
                let a = sm::GetPinnedSlices::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let pkg = a.pkg.as_deref();
                self.verify_caller(uid, pkg)
                    .and_then(|()| self.pinned_slices(pkg.unwrap_or_default(), uid))
                    .map(|slices| sm::write_get_pinned_slices_reply(&mut p, Some(&slices)))
            }
            sm::PIN_SLICE => {
                let a =
                    sm::PinSlice::<SliceUri, Spec>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let pkg = a.pkg.as_deref();
                self.verify_caller(uid, pkg)
                    .and_then(|()| {
                        let SliceUri(uri) = a.uri.ok_or_else(null_uri)?;
                        self.pin(pkg.unwrap_or_default(), &uri, a.specs, a.token, uid)
                    })
                    .map(|()| sm::write_pin_slice_reply(&mut p))
            }
            sm::UNPIN_SLICE => {
                let a = sm::UnpinSlice::<SliceUri>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let pkg = a.pkg.as_deref();
                if let Err(e) = self.verify_caller(uid, pkg) {
                    return reply(Err(e));
                }
                let Some(SliceUri(uri)) = a.uri else {
                    return reply(Err(null_uri()));
                };
                return self.unpin(pkg.unwrap_or_default(), &uri, a.token, uid);
            }
            sm::HAS_SLICE_ACCESS => {
                let a = sm::HasSliceAccess::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let pkg = a.pkg.as_deref();
                self.verify_caller(uid, pkg)
                    .and_then(|()| self.has_full_access(pkg.unwrap_or_default(), user))
                    .map(|v| sm::write_has_slice_access_reply(&mut p, v))
            }
            sm::GET_PINNED_SPECS => {
                let a =
                    sm::GetPinnedSpecs::<SliceUri>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let pkg = a.pkg.as_deref();
                self.verify_caller(uid, pkg)
                    .and_then(|()| {
                        let SliceUri(uri) = a.uri.ok_or_else(null_uri)?;
                        self.pinned_specs(pkg.unwrap_or_default(), &uri, uid)
                    })
                    .map(|specs| sm::write_get_pinned_specs_reply(&mut p, specs.as_deref()))
            }
            sm::GRANT_SLICE_PERMISSION | sm::REVOKE_SLICE_PERMISSION => {
                let (pkg, to_pkg, uri) = if call.code == sm::GRANT_SLICE_PERMISSION {
                    let a = sm::GrantSlicePermission::<SliceUri>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                    (a.calling_pkg, a.to_pkg, a.uri)
                } else {
                    let a = sm::RevokeSlicePermission::<SliceUri>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                    (a.calling_pkg, a.to_pkg, a.uri)
                };
                let grant = call.code == sm::GRANT_SLICE_PERMISSION;
                self.verify_caller(uid, pkg.as_deref())
                    .and_then(|()| {
                        let SliceUri(uri) = uri.ok_or_else(null_uri)?;
                        self.enforce_owner(pkg.as_deref(), &uri, user)?;
                        let to_pkg = to_pkg.ok_or_else(null_package)?;
                        self.with(|s| {
                            if grant {
                                s.store.grant(&to_pkg, user, pkg.as_deref(), user, &uri)
                            } else {
                                s.store.revoke(&to_pkg, user, pkg.as_deref(), user, &uri);
                                Ok(())
                            }
                        })?
                    })
                    .map(|()| {
                        if grant {
                            sm::write_grant_slice_permission_reply(&mut p)
                        } else {
                            sm::write_revoke_slice_permission_reply(&mut p)
                        }
                    })
            }
            sm::CHECK_SLICE_PERMISSION => {
                let a = sm::CheckSlicePermission::<SliceUri>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                a.uri
                    .ok_or_else(null_uri)
                    .and_then(|SliceUri(uri)| {
                        self.check(
                            &uri,
                            a.calling_pkg.as_deref(),
                            None,
                            a.uid,
                            uid,
                            a.auto_grant_permissions.as_deref(),
                        )
                    })
                    .map(|v| sm::write_check_slice_permission_reply(&mut p, v))
            }
            sm::GRANT_PERMISSION_FROM_USER => {
                let a = sm::GrantPermissionFromUser::<SliceUri>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.verify_caller(uid, a.calling_pkg.as_deref())
                    .and_then(|()| {
                        let SliceUri(uri) = a.uri.ok_or_else(null_uri)?;
                        self.grant_from_user(&uri, a.pkg.as_deref(), a.all_slices, uid)
                    })
                    .map(|()| sm::write_grant_permission_from_user_reply(&mut p))
            }
            sm::GET_BACKUP_PAYLOAD => {
                let a = sm::GetBackupPayload::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if uid != SYSTEM_UID {
                    Err(Exception::security("Caller must be system"))
                } else if a.user != USER_SYSTEM {
                    eprintln!(
                        "app host: slice: getBackupPayload: cannot backup policy for user {}",
                        a.user
                    );
                    sm::write_get_backup_payload_reply(&mut p, &None);
                    Ok(())
                } else {
                    self.with(|s| s.store.backup()).map(|payload| {
                        let payload = payload
                            .map_err(|e| {
                                eprintln!(
                                    "app host: slice: getBackupPayload: error writing payload for user {}: {e}",
                                    a.user
                                )
                            })
                            .ok();
                        sm::write_get_backup_payload_reply(&mut p, &payload)
                    })
                }
            }
            sm::APPLY_RESTORE => {
                let a = sm::ApplyRestore::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if uid != SYSTEM_UID {
                    Err(Exception::security("Caller must be system"))
                } else {
                    match (a.payload, a.user) {
                        (None, user) => eprintln!(
                            "app host: slice: applyRestore: no payload to restore for user {user}"
                        ),
                        (Some(_), user) if user != USER_SYSTEM => eprintln!(
                            "app host: slice: applyRestore: cannot restore policy for user {user}"
                        ),
                        (Some(payload), _) => match self.with(|s| s.store.restore(&payload)) {
                            Err(e) => return reply(Err(e)),
                            Ok(Err(e)) => eprintln!(
                                "app host: slice: applyRestore: error reading payload: {e}"
                            ),
                            Ok(Ok(())) => {}
                        },
                    }
                    sm::write_apply_restore_reply(&mut p);
                    Ok(())
                }
            }
            crate::shell::SHELL_COMMAND_TRANSACTION | DUMP_TRANSACTION => Err(self
                .host
                .unimplemented_named("slice", "its shell command and dump (#1941)")),
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aim-slice-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// `ContentProvider`'s URI users: added to a `content` URI that has
    /// none, read back, and dropped.
    #[test]
    fn uris_carry_their_users_as_content_provider_writes_them() {
        let uri = Uri::parse("content://com.example.slices/wifi?x=1");
        let with = with_user(&uri, 0);
        assert_eq!(with.as_str(), "content://0@com.example.slices/wifi?x=1");
        assert_eq!(user_from_uri(&with, 10), 0);
        assert_eq!(user_from_uri(&uri, 10), 10);
        assert_eq!(with_user(&with, 10).as_str(), with.as_str());
        assert_eq!(
            without_user(&with).as_str(),
            "content://com.example.slices/wifi?x=1"
        );
        assert_eq!(
            user_from_uri(&Uri::parse("content://x@com.example/a"), 0),
            USER_NULL
        );
        assert_eq!(
            with_user(&Uri::parse("http://example.com/a"), 0).as_str(),
            "http://example.com/a"
        );
        assert_eq!(
            Uri::parse("content://0@com.example/a/b")
                .with_empty_path()
                .as_str(),
            "content://0@com.example"
        );
    }

    /// `mergeSpecs`: the types both have, at the lower revision.
    #[test]
    fn specs_merge_to_what_every_pin_supports() {
        let spec = |kind: &str, revision| {
            Some(Spec {
                kind: Some(kind.into()),
                revision,
            })
        };
        let first = Some(vec![spec("list", 2), spec("message", 1)]);
        assert_eq!(merge_specs(None, first.clone()).unwrap(), first);
        let merged = merge_specs(first, Some(vec![spec("list", 1), spec("grid", 3)])).unwrap();
        assert_eq!(merged, Some(vec![spec("list", 1)]));
        assert!(merge_specs(Some(vec![None]), Some(vec![])).is_err());
    }

    /// `addPath` and `removePath`: a granted prefix covers its paths.
    #[test]
    fn paths_are_kept_as_their_shortest_prefixes() {
        let path = |s: &str| -> Vec<String> { s.split('/').map(str::to_string).collect() };
        let mut a = ClientAuthority {
            authority: "com.example".into(),
            provider: PkgUser::new("com.example", 0),
            paths: Vec::new(),
        };
        assert!(a.add_path(path("a/b")));
        assert!(a.add_path(path("c")));
        assert!(!a.add_path(path("a/b/c")));
        assert!(a.add_path(path("a")));
        assert_eq!(a.paths, vec![path("c"), path("a")]);
        assert!(a.remove_path(&path("a")));
        assert!(!a.remove_path(&path("a")));
        assert_eq!(a.paths, vec![path("c")]);
    }

    /// The files as KXmlSerializer writes them, and read back.
    #[test]
    fn permission_files_are_written_as_the_original_writes_them() {
        let mut client = Client::new(PkgUser::new("com.client", 0));
        let (auth, _) = client.authority("com.example.slices", &PkgUser::new("com.example", 0));
        auth.add_path(vec!["wi fi".into(), "x".into()]);
        let mut x = Kxml::default();
        client.write(&mut x);
        assert_eq!(
            x.0,
            r#"<client pkg="com.client@0" fullAccess="0"><authority authority="com.example.slices" pkg="com.example@0"><path>wi%20fi/x</path></authority></client>"#
        );
        let read = Client::read(&aim_android_xml::read(x.0.as_bytes()).unwrap()).unwrap();
        assert_eq!(read, client);

        let mut provider = Provider::new(PkgUser::new("com.example", 0));
        assert!(provider.add("com.example.slices", &PkgUser::new("com.client", 0)));
        assert!(!provider.add("com.example.slices", &PkgUser::new("com.client", 0)));
        provider.add("com.example.empty", &PkgUser::new("b", 0));
        provider.auths[1].1.clear();
        let mut x = Kxml::default();
        provider.write(&mut x);
        let empty = java_hash("com.example.empty") < java_hash("com.example.slices");
        let slices =
            r#"<authority authority="com.example.slices"><pkg>com.client@0</pkg></authority>"#;
        let none = r#"<authority authority="com.example.empty" />"#;
        let (first, second) = if empty {
            (none, slices)
        } else {
            (slices, none)
        };
        assert_eq!(
            x.0,
            format!(r#"<provider pkg="com.example@0">{first}{second}</provider>"#)
        );
        let read = Provider::read(&aim_android_xml::read(x.0.as_bytes()).unwrap()).unwrap();
        assert_eq!(read.pkg, provider.pkg);
        assert_eq!(read.auths.len(), 2);
    }

    /// A grant persisted, read again by a new store, backed up and
    /// restored.
    #[test]
    fn grants_persist_and_round_trip_through_a_backup() {
        let dir = tmp().join("slice");
        // Callers grant a URI without its user: the client's authority is
        // the URI's as it is.
        let uri = Uri::parse("content://com.example.slices/wifi/details");
        let mut store = Store::new(dir.clone());
        store
            .grant("com.client", 0, Some("com.example"), 0, &uri)
            .unwrap();
        assert!(store.has_permission("com.client", 0, &uri));
        assert!(store.has_permission("com.client", 0, &with_user(&uri, 0)));
        assert!(!store.has_permission("com.other", 0, &uri));
        assert!(!store.has_permission(
            "com.client",
            0,
            &Uri::parse("content://0@com.example.slices/bluetooth")
        ));
        store.persist();
        assert!(dir.join("client_com.client@0").exists());
        assert!(dir.join("provider_com.example@0").exists());

        let mut again = Store::new(dir.clone());
        assert!(again.has_permission(
            "com.client",
            0,
            &Uri::parse("content://com.example.slices/wifi/details/more")
        ));
        let backup = again.backup().unwrap();
        let text = String::from_utf8(backup.clone()).unwrap();
        assert!(text.starts_with(r#"<slice-access-list version="2"><client pkg="com.client@0""#));

        let other = tmp().join("slice");
        let mut restored = Store::new(other.clone());
        restored.restore(&backup).unwrap();
        restored.persist();
        assert_eq!(
            std::fs::read(other.join("client_com.client@0")).unwrap(),
            std::fs::read(dir.join("client_com.client@0")).unwrap()
        );
        assert!(restored.has_permission("com.client", 0, &uri));

        let mut old = Store::new(tmp().join("slice"));
        old.restore(br#"<slice-access-list version="1"><client pkg="a@0" fullAccess="1" /></slice-access-list>"#)
            .unwrap();
        assert!(!old.has_full_access("a", 0));
    }

    /// `removePkg` of a provider whose data was cleared: its clients lose
    /// its authorities and its files go; of a client: its grants go.
    #[test]
    fn a_cleared_package_s_grants_go() {
        let dir = tmp().join("slice");
        let uri = Uri::parse("content://com.example.slices/wifi");
        let mut store = Store::new(dir.clone());
        store
            .grant("com.client", 0, Some("com.example"), 0, &uri)
            .unwrap();
        store.grant_full_access("com.full", 0);
        store.persist();
        store.remove_pkg("com.example", 0);
        assert!(!store.has_permission("com.client", 0, &uri));
        assert!(!dir.join("provider_com.example@0").exists());
        store.persist();
        assert!(!Store::new(dir.clone()).has_permission("com.client", 0, &uri));
        store.remove_pkg("com.full", 0);
        assert!(!dir.join("client_com.full@0").exists());
        assert!(!Store::new(dir).has_full_access("com.full", 0));
    }

    /// A grant to a provider with no package fails after the client's
    /// part, as the original's `PkgUser.hashCode` does.
    #[test]
    fn a_grant_of_no_provider_fails_after_the_client() {
        let mut store = Store::new(tmp().join("slice"));
        let uri = Uri::parse("content://com.example.slices/x");
        assert!(store.grant("com.client", 0, None, 0, &uri).is_err());
        assert!(store.has_permission("com.client", 0, &uri));
        assert_eq!(
            store.clients[&PkgUser::new("com.client", 0)].auths[0]
                .provider
                .to_string(),
            "null@0"
        );
    }
}
