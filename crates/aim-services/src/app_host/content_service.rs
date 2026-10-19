//! `content`: ContentService for the app host (`ContentService` at
//! `android-16.0.0_r1`): content observers by URI, the notification of
//! their changes, and the content cache. Who may observe or notify a URI
//! is ActivityManager's `checkContentProviderAccess`. Sync adapters
//! (SyncManager) are not run (#1460): of SyncStorageEngine there are the
//! master sync settings it persisted (`/data/system/sync/accounts.xml`)
//! and its status listeners, which no sync's progress notifies.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, Exception, Parcel, Reader, Result as ParcelResult,
};
use aim_service_aidl::android_content_icontentservice as cs;
use aim_service_aidl::ReadParcelable;

use super::kept::{By, KeptBundle, RawBundle};
use super::{AppHost, PER_USER_RANGE, reply};
use crate::package::intent_filter::Plain;
use crate::package::uri::Uri;

/// `IContentObserver.onChangeEtc`; its descriptor.
const ON_CHANGE_ETC: u32 = 2;
const OBSERVER_DESCRIPTOR: &str = "android.database.IContentObserver";
/// `UserHandle.USER_ALL`; `Build.VERSION_CODES.O`.
const USER_ALL: i32 = -1;
const O: i32 = 26;
/// `ContentResolver.NOTIFY_SYNC_TO_NETWORK`,
/// `NOTIFY_SKIP_NOTIFY_FOR_DESCENDANTS`.
const NOTIFY_SYNC_TO_NETWORK: i32 = 1;
const NOTIFY_SKIP_NOTIFY_FOR_DESCENDANTS: i32 = 1 << 1;

/// `readAccountInfoLocked`'s master sync setting of `user` in the
/// accounts document `root`: the root's `listen-for-tickles` for user 0,
/// then each `listenForTickles` entry's; none under another root.
fn master_sync_of(root: &aim_android_xml::Element, user: i32) -> Option<bool> {
    if root.name != "accounts" {
        return None;
    }
    let mut listen = (user == 0).then(|| root.bool("listen-for-tickles").ok().flatten().unwrap_or(true));
    for e in root.children().filter(|e| e.name == "listenForTickles") {
        if e.int("user").ok().flatten().unwrap_or(0) == user {
            listen = Some(e.bool("enabled").ok().flatten().unwrap_or(true));
        }
    }
    listen
}

/// A parcelled `Uri`.
pub(super) struct ParcelUri(pub(super) Uri);

impl ReadParcelable for ParcelUri {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Uri::read(r, &mut Plain)?.map(ParcelUri).ok_or(BAD_VALUE)
    }
}

/// `ObserverNode.ObserverEntry`: the observer by its handle.
struct Entry {
    binder: u32,
    notify_for_descendants: bool,
    user: i32,
}

/// `ContentService.ObserverNode`: a URI segment's observers and children.
#[derive(Default)]
struct Node {
    name: String,
    children: Vec<Node>,
    observers: Vec<Entry>,
}

/// `ObserverNode.getUriSegment`: the authority, then the path's segments.
fn segments(uri: &Uri) -> Option<Vec<String>> {
    let mut segments = vec![uri.authority()?];
    segments.extend(uri.path_segments());
    Some(segments)
}

impl Node {
    fn add(&mut self, segments: &[String], entry: Entry) {
        let Some((first, rest)) = segments.split_first() else {
            self.observers.push(entry);
            return;
        };
        match self.children.iter_mut().find(|c| c.name == *first) {
            Some(child) => child.add(rest, entry),
            None => {
                let mut child = Node {
                    name: first.clone(),
                    ..Default::default()
                };
                child.add(rest, entry);
                self.children.push(child);
            }
        }
    }

    /// Whether `binder` is registered at this node or below.
    fn has(&self, binder: u32) -> bool {
        self.observers.iter().any(|e| e.binder == binder)
            || self.children.iter().any(|c| c.has(binder))
    }

    /// Removes one of `binder`'s registrations at each node: whether the
    /// node is empty.
    fn remove(&mut self, binder: u32) -> bool {
        self.children.retain_mut(|c| !c.remove(binder));
        if let Some(at) = self.observers.iter().position(|e| e.binder == binder) {
            self.observers.remove(at);
        }
        self.children.is_empty() && self.observers.is_empty()
    }

    /// `collectObserversLocked`: the observers of `segments[index..]`'s
    /// path, and of its ancestors that watch descendants.
    fn collect(
        &self,
        segments: &[String],
        index: usize,
        change: &Change,
        out: &mut Vec<(u32, bool)>,
    ) {
        let leaf = index >= segments.len();
        for e in &self.observers {
            let self_change = change.observer == Some(e.binder);
            if self_change && !change.wants_self {
                continue;
            }
            if !(change.user == USER_ALL || e.user == USER_ALL || change.user == e.user) {
                continue;
            }
            if leaf {
                if change.flags & NOTIFY_SKIP_NOTIFY_FOR_DESCENDANTS != 0
                    && e.notify_for_descendants
                {
                    continue;
                }
            } else if !e.notify_for_descendants {
                continue;
            }
            out.push((e.binder, self_change));
        }
        let segment = segments.get(index);
        for child in &self.children {
            if segment.is_none_or(|s| child.name == *s) {
                child.collect(segments, index + 1, change, out);
                if segment.is_some() {
                    break;
                }
            }
        }
    }
}

/// What changed, as `notifyChange` collects for it.
struct Change {
    observer: Option<u32>,
    wants_self: bool,
    flags: i32,
    user: i32,
}

/// A notification to send: observer, `selfChange`, flags, user (the
/// collector's key), and its URIs.
type Notification = (u32, bool, i32, i32, Vec<Uri>);

/// A cache entry's place: user and provider package; caller package and key.
type CacheProvider = (i32, Option<String>);
type CacheKey = (String, String);

pub struct ContentService {
    host: Arc<AppHost>,
    root: Mutex<Node>,
    /// The observers' binders and death links, held while registered, by
    /// handle.
    observers: Mutex<HashMap<u32, (Strong, u64)>>,
    /// `mCache`.
    cache: Mutex<HashMap<CacheProvider, Vec<(CacheKey, KeptBundle)>>>,
    /// SyncStorageEngine's `mChangeListeners`: by handle, the binder and
    /// its death link, and the cookie (the caller's uid and the mask).
    status_listeners: Mutex<HashMap<u32, (Strong, u64, (i32, i32))>>,
}

impl ContentService {
    pub fn new(host: Arc<AppHost>) -> Self {
        ContentService {
            host,
            root: Mutex::new(Node::default()),
            observers: Mutex::new(HashMap::new()),
            cache: Mutex::new(HashMap::new()),
            status_listeners: Mutex::new(HashMap::new()),
        }
    }

    /// `enforceCallingOrSelfPermission(permission, message)`.
    fn enforce(&self, uid: i32, permission: &str, message: &str) -> Result<(), Exception> {
        if self.host.check_permission(Some(permission), uid)? == 0 {
            return Ok(());
        }
        Err(Exception::security(format!("{message}: Neither user {uid} nor current process has {permission}.")))
    }

    /// `getCurrentSyncsAsUser`: `enforceCrossUserPermission` and
    /// READ_SYNC_STATS, then SyncStorageEngine's current syncs, of which
    /// there are none: no sync runs (#1460).
    fn current_syncs(&self, uid: i32, user: i32) -> Result<(), Exception> {
        if uid / PER_USER_RANGE as i32 != user {
            self.enforce(uid, "android.permission.INTERACT_ACROSS_USERS_FULL", "no permission to read the syncs of other users")?;
        }
        self.enforce(uid, "android.permission.READ_SYNC_STATS", "no permission to read the sync stats")
    }

    /// `getMasterSyncAutomaticallyAsUser`: `enforceCrossUserPermission`,
    /// READ_SYNC_SETTINGS, then SyncStorageEngine's setting of the user.
    fn master_sync(&self, uid: i32, user: i32) -> Result<bool, Exception> {
        if uid / PER_USER_RANGE as i32 != user {
            self.enforce(
                uid,
                "android.permission.INTERACT_ACROSS_USERS_FULL",
                &format!("no permission to read the sync settings for user {user}"),
            )?;
        }
        self.enforce(uid, "android.permission.READ_SYNC_SETTINGS", "no permission to read the sync settings")?;
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let path = self.host.data()?.join("system/sync/accounts.xml");
        let bytes = super::device_policy::read_atomic_bytes(&path).map_err(state)?;
        let root = bytes.and_then(|b| {
            aim_android_xml::read(&b).inspect_err(|e| eprintln!("app host: Error reading accounts: {e}")).ok()
        });
        if let Some(listen) = root.as_ref().and_then(|r| master_sync_of(r, user)) {
            return Ok(listen);
        }
        let packages = &self.host.inputs()?.packages;
        packages
            .framework_bool("config_syncstorageengine_masterSyncAutomatically")
            .ok_or_else(|| state("no config_syncstorageengine_masterSyncAutomatically".into()))
    }

    /// `addStatusChangeListener`: a listener kept, as a
    /// `RemoteCallbackList` keeps it, until removed or dead.
    fn add_status_listener(&self, uid: i32, mask: i32, listener: u32) {
        let strong = self.host.process.strong(listener);
        let me = Arc::downgrade(&self.host);
        let cookie = self.host.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(cs) = me.upgrade().and_then(|h| h.content_service.get().cloned()) {
                    cs.remove_status_listener(listener);
                }
            }),
        );
        let old = self.status_listeners.lock().unwrap().insert(listener, (strong, cookie, (uid, mask)));
        if let Some((strong, cookie, _)) = old {
            self.host.process.clear_death(&strong, cookie);
        }
    }

    fn remove_status_listener(&self, listener: u32) {
        if let Some((strong, cookie, _)) = self.status_listeners.lock().unwrap().remove(&listener) {
            self.host.process.clear_death(&strong, cookie);
        }
    }

    /// The access check of `registerContentObserver` and `notifyChange`:
    /// none for apps targeting before O that name a provider that does not
    /// exist; `false` when the call is to be ignored.
    fn check_access(
        &self,
        uri: &Uri,
        uid: i32,
        user: i32,
        target_sdk: i32,
    ) -> Result<bool, Exception> {
        let authority = uri.authority().unwrap_or_default();
        match self.host.provider_access(&authority, uid, user)? {
            None => Ok(true),
            Some(msg) if target_sdk >= O => Err(Exception::security(msg)),
            Some(msg) if msg.starts_with("Failed to find provider") => Ok(true),
            Some(_) => Ok(false),
        }
    }

    fn register(&self, call: &mut Call<'_>) -> Result<(), Exception> {
        let args = cs::RegisterContentObserver::<ParcelUri>::read(&mut call.data)
            .map_err(|_| Exception::illegal_argument("registerContentObserver"))?;
        let (Some(ParcelUri(uri)), Some(Binder::Handle(observer))) = (args.uri, args.observer)
        else {
            return Err(Exception::illegal_argument(
                "You must pass a valid uri and observer",
            ));
        };
        let uid = call.sender_euid as i32;
        let user = self.host.incoming_user(uid, args.user_handle, true)?;
        if !self.check_access(&uri, uid, user, args.target_sdk_version)? {
            return Ok(());
        }
        let segments = segments(&uri).ok_or_else(|| {
            Exception::illegal_argument(format!("Invalid Uri ({}) used for observer", uri.as_str()))
        })?;
        self.observers
            .lock()
            .unwrap()
            .entry(observer)
            .or_insert_with(|| {
                let strong = self.host.process.strong(observer);
                // `ObserverEntry` links to the observer's death.
                let me = Arc::downgrade(&self.host);
                let cookie = self.host.process.link_to_death(
                    &strong,
                    Box::new(move || {
                        if let Some(cs) =
                            me.upgrade().and_then(|h| h.content_service.get().cloned())
                        {
                            cs.remove(observer);
                        }
                    }),
                );
                (strong, cookie)
            });
        self.root.lock().unwrap().add(
            &segments,
            Entry {
                binder: observer,
                notify_for_descendants: args.notify_for_descendants,
                user,
            },
        );
        Ok(())
    }

    /// An observer's registrations go (`removeObserverLocked`, which
    /// removes one at each node).
    fn remove(&self, observer: u32) {
        let mut root = self.root.lock().unwrap();
        root.remove(observer);
        if !root.has(observer)
            && let Some((strong, cookie)) = self.observers.lock().unwrap().remove(&observer)
        {
            self.host.process.clear_death(&strong, cookie);
        }
    }

    fn notify(&self, call: &mut Call<'_>) -> Result<(), Exception> {
        let args = cs::NotifyChange::<ParcelUri>::read(&mut call.data)
            .map_err(|_| Exception::illegal_argument("notifyChange"))?;
        let uid = call.sender_euid as i32;
        let observer = match args.observer {
            Some(Binder::Handle(h)) => Some(h),
            _ => None,
        };
        let user = self.host.incoming_user(uid, args.user_handle, true)?;
        let mut uris = Vec::new();
        for ParcelUri(uri) in args.uris.unwrap_or_default().into_iter().flatten() {
            if self.check_access(&uri, uid, user, args.target_sdk_version)? {
                uris.push(uri);
            }
        }
        self.change(
            uris,
            &Change {
                observer,
                wants_self: args.observer_wants_self_notifications,
                flags: args.flags,
                user,
            },
        )
    }

    /// The system's `notifyChange` of `uri` for `user`, as
    /// `ContentResolver.notifyChange(uri, null, true, user)` makes it
    /// (no observer, `NOTIFY_SYNC_TO_NETWORK`).
    pub(super) fn notify_from_system(&self, uri: Uri, user: i32) -> Result<(), Exception> {
        let change = Change { observer: None, wants_self: false, flags: NOTIFY_SYNC_TO_NETWORK, user };
        self.change(vec![uri], &change)
    }

    /// `notifyChange` of the URIs that passed its access checks: their
    /// observers told, the content cache of their providers invalidated.
    fn change(&self, uris: Vec<Uri>, change: &Change) -> Result<(), Exception> {
        let mut collected: Vec<Notification> = Vec::new();
        let mut touched: Vec<(i32, Option<String>, Uri)> = Vec::new();
        let (flags, user) = (change.flags, change.user);
        for uri in uris {
            let Some(segments) = segments(&uri) else {
                continue;
            };
            let mut found = Vec::new();
            self.root
                .lock()
                .unwrap()
                .collect(&segments, 0, change, &mut found);
            // `ObserverCollector`: one call per observer, uid, selfChange,
            // flags and user, with its URIs.
            for (binder, self_change) in found {
                match collected
                    .iter_mut()
                    .find(|n| n.0 == binder && n.1 == self_change && n.2 == flags && n.3 == user)
                {
                    Some(n) => n.4.push(uri.clone()),
                    None => collected.push((binder, self_change, flags, user, vec![uri.clone()])),
                }
            }
            touched.push((
                user,
                self.host
                    .provider_package(&uri.authority().unwrap_or_default(), user)?,
                uri,
            ));
        }
        // Foreground observers get theirs at once; the app host's processes
        // are all top.
        let observers = self.observers.lock().unwrap();
        for (binder, self_change, flags, user, uris) in collected {
            let Some((observer, _)) = observers.get(&binder) else {
                continue;
            };
            let mut p = Parcel::new();
            p.write_interface_token(OBSERVER_DESCRIPTOR);
            p.write_bool(self_change);
            p.write_i32(uris.len() as i32);
            for uri in &uris {
                p.write_i32(1);
                uri.write(&mut p);
            }
            p.write_i32(flags);
            p.write_i32(user);
            let _ = observer.transact(ON_CHANGE_ETC, &p, true);
        }
        drop(observers);
        // `invalidateCacheLocked` of the providers touched.
        let mut cache = self.cache.lock().unwrap();
        for (user, package, uri) in touched {
            if let Some(entries) = cache.get_mut(&(user, package)) {
                entries.retain(|((_, key), _)| !key.starts_with(uri.as_str()));
            }
        }
        Ok(())
    }

    /// `putCache` and `getCache`'s checks: the caller's user, `CACHE_CONTENT`,
    /// its package; the key's provider package.
    fn cache_key(
        &self,
        uid: i32,
        package: Option<&str>,
        key: Option<&Uri>,
        user: i32,
    ) -> Result<(CacheProvider, CacheKey), Exception> {
        if user != uid / 100_000
            && self
                .host
                .check_permission(Some("android.permission.INTERACT_ACROSS_USERS_FULL"), uid)?
                != 0
        {
            return Err(Exception::security("INTERACT_ACROSS_USERS_FULL"));
        }
        if self
            .host
            .check_permission(Some("android.permission.CACHE_CONTENT"), uid)?
            != 0
        {
            return Err(Exception::security(
                "Neither user nor current process has android.permission.CACHE_CONTENT.",
            ));
        }
        let package = package.ok_or_else(|| Exception::illegal_argument("no package"))?;
        if !self.host.owns_package(uid, package)? {
            return Err(Exception::security(format!(
                "Package {package} does not belong to {uid}"
            )));
        }
        let key = key.ok_or_else(|| Exception::illegal_argument("no key"))?;
        let provider = self
            .host
            .provider_package(&key.authority().unwrap_or_default(), user)?;
        Ok((
            (user, provider),
            (package.to_string(), key.as_str().to_string()),
        ))
    }
}

impl Service for ContentService {
    fn descriptor(&self) -> &str {
        cs::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let empty = |r: Result<(), Exception>| {
            reply(r.map(|()| {
                let mut p = Parcel::new();
                p.write_no_exception();
                p
            }))
        };
        match call.code {
            cs::REGISTER_CONTENT_OBSERVER => empty(self.register(call)),
            cs::UNREGISTER_CONTENT_OBSERVER => {
                let args =
                    cs::UnregisterContentObserver::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let Some(Binder::Handle(observer)) = args.observer else {
                    return empty(Err(Exception::illegal_argument(
                        "You must pass a valid observer",
                    )));
                };
                self.remove(observer);
                empty(Ok(()))
            }
            cs::NOTIFY_CHANGE => empty(self.notify(call)),
            cs::PUT_CACHE => {
                let args = cs::PutCache::<ParcelUri, RawBundle>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let key = args.key.map(|k| k.0);
                let value = args.value.map(|v| KeptBundle::new(v, &self.host.process)).transpose()?;
                empty(
                    self.cache_key(
                        call.sender_euid as i32,
                        args.package_name.as_deref(),
                        key.as_ref(),
                        args.user_id,
                    )
                    .map(|(provider, key)| {
                        let mut cache = self.cache.lock().unwrap();
                        let entries = cache.entry(provider).or_default();
                        entries.retain(|(k, _)| *k != key);
                        if let Some(value) = value {
                            entries.push((key, value));
                        }
                    }),
                )
            }
            cs::GET_CACHE => {
                let args =
                    cs::GetCache::<ParcelUri>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let key = args.key.map(|k| k.0);
                reply(
                    self.cache_key(
                        call.sender_euid as i32,
                        args.package_name.as_deref(),
                        key.as_ref(),
                        args.user_id,
                    )
                    .map(|(provider, key)| {
                        let cache = self.cache.lock().unwrap();
                        let value = cache
                            .get(&provider)
                            .and_then(|e| e.iter().find(|(k, _)| *k == key))
                            .map(|(_, v)| v.clone());
                        let mut p = Parcel::new();
                        cs::write_get_cache_reply(&mut p, value.as_ref().map(|v| By(&v.bundle)).as_ref());
                        p
                    }),
                )
            }
            cs::GET_CURRENT_SYNCS | cs::GET_CURRENT_SYNCS_AS_USER => {
                let uid = call.sender_euid as i32;
                let user = if call.code == cs::GET_CURRENT_SYNCS {
                    cs::GetCurrentSyncs::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                    uid / PER_USER_RANGE as i32
                } else {
                    cs::GetCurrentSyncsAsUser::read(&mut call.data).map_err(|_| BAD_VALUE)?.user_id
                };
                reply(self.current_syncs(uid, user).map(|()| {
                    // `writeTypedList` of no `SyncInfo`.
                    let mut p = Parcel::new();
                    p.write_no_exception();
                    p.write_i32(0);
                    p
                }))
            }
            cs::GET_MASTER_SYNC_AUTOMATICALLY => {
                cs::GetMasterSyncAutomatically::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let uid = call.sender_euid as i32;
                reply(self.master_sync(uid, uid / PER_USER_RANGE as i32).map(|listen| {
                    let mut p = Parcel::new();
                    cs::write_get_master_sync_automatically_reply(&mut p, listen);
                    p
                }))
            }
            cs::GET_MASTER_SYNC_AUTOMATICALLY_AS_USER => {
                let a = cs::GetMasterSyncAutomaticallyAsUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.master_sync(call.sender_euid as i32, a.user_id).map(|listen| {
                    let mut p = Parcel::new();
                    cs::write_get_master_sync_automatically_as_user_reply(&mut p, listen);
                    p
                }))
            }
            cs::ADD_STATUS_CHANGE_LISTENER => {
                let a = cs::AddStatusChangeListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(Binder::Handle(listener)) = a.callback {
                    self.add_status_listener(call.sender_euid as i32, a.mask, listener);
                }
                empty(Ok(()))
            }
            cs::REMOVE_STATUS_CHANGE_LISTENER => {
                let a = cs::RemoveStatusChangeListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Some(Binder::Handle(listener)) = a.callback {
                    self.remove_status_listener(listener);
                }
                empty(Ok(()))
            }
            code => reply(Err(self.host.unimplemented("content", code))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The master sync setting as `readAccountInfoLocked` reads it: the
    /// root's attribute for user 0, then each user's entry.
    #[test]
    fn master_sync_is_read_as_the_storage_engine_reads_it() {
        let xml = br#"<accounts version="3" listen-for-tickles="false"><listenForTickles user="10" enabled="true" /><listenForTickles user="11" /></accounts>"#;
        let root = aim_android_xml::read(xml).unwrap();
        assert_eq!(master_sync_of(&root, 0), Some(false));
        assert_eq!(master_sync_of(&root, 10), Some(true));
        assert_eq!(master_sync_of(&root, 11), Some(true));
        assert_eq!(master_sync_of(&root, 12), None);
        let other = aim_android_xml::read(b"<other />").unwrap();
        assert_eq!(master_sync_of(&other, 0), None);
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn entry(binder: u32, descendants: bool) -> Entry {
        Entry {
            binder,
            notify_for_descendants: descendants,
            user: 0,
        }
    }

    fn change(observer: Option<u32>) -> Change {
        Change {
            observer,
            wants_self: false,
            flags: 0,
            user: 0,
        }
    }

    /// `ObserverNodeTest`'s cases: an observer of a URI hears changes to it
    /// and to its ancestors, and to its descendants only if it asked for
    /// them.
    #[test]
    fn observers_hear_their_uri_and_maybe_its_descendants() {
        let mut root = Node::default();
        root.add(&s(&["a", "b"]), entry(1, false));
        root.add(&s(&["a"]), entry(2, true));
        root.add(&s(&["a"]), entry(3, false));
        fn heard(root: &Node, path: &[&str], c: &Change) -> Vec<u32> {
            let mut out = Vec::new();
            root.collect(&s(path), 0, c, &mut out);
            out.iter().map(|o| o.0).collect()
        }
        assert_eq!(heard(&root, &["a", "b"], &change(None)), [2, 1]);
        assert_eq!(heard(&root, &["a"], &change(None)), [2, 3, 1]);
        assert_eq!(heard(&root, &["a", "c"], &change(None)), [2]);
        // The notifier itself, unless it wants its own changes.
        assert_eq!(heard(&root, &["a", "b"], &change(Some(1))), [2]);
        // Skipping the observers of descendants at the leaf.
        let skip = Change {
            flags: NOTIFY_SKIP_NOTIFY_FOR_DESCENDANTS,
            ..change(None)
        };
        assert_eq!(heard(&root, &["a"], &skip), [3, 1]);
        // Another user's observers do not hear it.
        assert_eq!(
            heard(
                &root,
                &["a"],
                &Change {
                    user: 10,
                    ..change(None)
                }
            ),
            Vec::<u32>::new()
        );
        assert!(!root.remove(2) && !root.has(2));
        assert_eq!(heard(&root, &["a", "b"], &change(None)), [1]);
    }

    #[test]
    fn segments_are_the_authority_then_the_path() {
        let uri = Uri::parse("content://settings/system/font_scale");
        assert_eq!(
            segments(&uri),
            Some(s(&["settings", "system", "font_scale"]))
        );
        assert_eq!(segments(&Uri::parse("mailto:a@b")), None);
    }
}
