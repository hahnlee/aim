//! The feed of the original PackageManager's state to the native model
//! (docs/m4-packagemanager.md, slice A). system_server's bridge
//! (`PackageFeed` in java/device-services) reads the original's state
//! through `PackageManagerLocal` and the system APIs and sends it here as
//! records ([`record`]) over `IPackageFeedHost`: at attach every record,
//! then in each batch those that changed. A batch is taken on each package
//! monitor callback, and when [`Feed::fresh`] finds the
//! `package_info_cache` nonce moved since the last one; it ends with the
//! SHA-256 of a fresh snapshot's records, which the feed checks against
//! the records it holds before it publishes the state the batch makes. A
//! digest that differs means the model drifted: the next batch is a full
//! one.
//!
//! The feed never calls system_server synchronously while serving it:
//! `IPackageFeed.sync` is one-way, and the records come on system_server's
//! own calls.

mod record;

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, Weak};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::{
    dev_aim_server_ibridge as bridge, dev_aim_server_ipackagefeed as feed,
    dev_aim_server_ipackagefeedhost as host,
};
use sha2::{Digest, Sha256};

use super::model::{PackageState, State};
use crate::SYSTEM_UID;
use crate::system::System;

/// The records' kinds, as `PackageFeed.java` numbers them.
const PACKAGE: i32 = 0;
const DISABLED_SYSTEM_PACKAGE: i32 = 1;
const PARSED: i32 = 2;
const DISABLED_SYSTEM_PARSED: i32 = 3;
const SHARED_USER: i32 = 4;
const USER: i32 = 5;
const SYSTEM: i32 = 6;

/// A record's kind and key, ordered as `PackageFeed.Key` orders them: by
/// kind, then as Java compares strings (by UTF-16 unit).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    kind: i32,
    name: String,
}

impl Ord for Key {
    fn cmp(&self, other: &Self) -> Ordering {
        self.kind
            .cmp(&other.kind)
            .then_with(|| self.name.encode_utf16().cmp(other.name.encode_utf16()))
    }
}

impl PartialOrd for Key {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The model's side of the feed: the records system_server sent and the
/// state they make.
pub struct Feed {
    this: Weak<Feed>,
    process: Arc<LocalProcess>,
    system: Weak<System>,
    /// This feed's `IPackageFeedHost` node.
    node: Binder,
    /// Where each state published is written ([`dump`]).
    dump: Option<PathBuf>,
    inner: Mutex<Inner>,
    ended: Condvar,
}

#[derive(Default)]
struct Inner {
    /// The attached system_server's `IPackageFeed`.
    feed: Option<Arc<Strong>>,
    /// Each record and its SHA-256.
    records: BTreeMap<Key, (Arc<[u8]>, [u8; 32])>,
    /// A record coming in chunks: its key, length and bytes so far.
    partial: Option<(Key, usize, Vec<u8>)>,
    state: Option<Arc<State>>,
    /// The tokens asked for and the nonce read before each.
    asked: Vec<(i64, Option<i64>)>,
    last_token: i64,
    /// The latest batch's token, and whether its digest matched.
    ended: Option<(i64, bool)>,
}

impl Feed {
    /// Feeds the model from each bridge `system` is handed from now on;
    /// with `dump`, writes each state it publishes there.
    pub fn start(system: &Arc<System>, dump: Option<PathBuf>) -> Arc<Feed> {
        let process = system.process();
        let feed = Arc::new_cyclic(|this: &Weak<Feed>| Feed {
            this: this.clone(),
            node: process.add_service(Arc::new(HostNode { feed: this.clone() })),
            process: process.clone(),
            system: Arc::downgrade(system),
            dump,
            inner: Mutex::new(Inner::default()),
            ended: Condvar::new(),
        });
        // The listener keeps the feed while the host lives.
        let fed = feed.clone();
        system.add_bridge_listener(Box::new(move |handle| fed.attach(handle)));
        feed
    }

    /// The last state published.
    pub fn state(&self) -> Option<Arc<State>> {
        self.inner.lock().unwrap().state.clone()
    }

    /// A state at least as new as the original's now: the last one if the
    /// `package_info_cache` nonce has not moved since its batch was asked
    /// for, else that of a batch asked for now, once its digest matched (a
    /// batch that drifted asks for a full one, which is waited for too).
    /// `None` without a bridge, or when no such batch ended within
    /// `timeout`. Never called while serving system_server.
    pub fn fresh(&self, timeout: Duration) -> Option<Arc<State>> {
        let deadline = Instant::now() + timeout;
        let nonce = self.system.upgrade()?.package_info_nonce();
        let mut inner = self.inner.lock().unwrap();
        if let Some(state) = &inner.state
            && nonce.is_some()
            && state.nonce == nonce
        {
            return Some(state.clone());
        }
        let mut token = self.ask(&mut inner, false, nonce)?;
        let mut drifts = 0;
        loop {
            inner.feed.as_ref()?;
            match inner.ended {
                Some((ended, true)) if ended >= token => return inner.state.clone(),
                // `end` asked for every record again, with the last token.
                Some((ended, false)) if ended >= token && drifts == 0 => {
                    drifts += 1;
                    token = inner.last_token;
                }
                Some((ended, false)) if ended >= token => return None,
                _ => {}
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            inner = self.ended.wait_timeout(inner, left).unwrap().0;
        }
    }

    /// Asks system_server for a batch with a new token (one-way); `nonce`
    /// was read before.
    fn ask(&self, inner: &mut Inner, reset: bool, nonce: Option<i64>) -> Option<i64> {
        let feed = inner.feed.clone()?;
        inner.last_token += 1;
        let token = inner.last_token;
        inner.asked.push((token, nonce));
        let mut data = Parcel::new();
        feed::Sync { reset, token }.write(&mut data);
        if let Err(s) = feed.transact(feed::SYNC, &data, true) {
            eprintln!("package feed: sync: status {s}");
            return None;
        }
        Some(token)
    }

    /// `IBridge.getPackageFeed`: system_server sends every record now.
    fn attach(&self, handle: u32) {
        let mut data = Parcel::new();
        bridge::GetPackageFeed {
            host: Some(self.node),
        }
        .write(&mut data);
        let reply = match self
            .process
            .transact(handle, bridge::GET_PACKAGE_FEED, &data, false)
        {
            Ok(reply) => reply,
            Err(s) => {
                eprintln!("package feed: getPackageFeed: status {s}");
                return;
            }
        };
        let Ok(Ok(Some(Binder::Handle(h)))) =
            bridge::read_get_package_feed_reply(&mut reply.reader())
        else {
            eprintln!("package feed: system_server has no package feed");
            return;
        };
        let strong = Arc::new(self.process.strong(h));
        drop(reply);
        let this = self.this.clone();
        self.process.link_to_death(
            &strong,
            Box::new(move || {
                if let Some(feed) = this.upgrade() {
                    feed.inner.lock().unwrap().feed = None;
                    feed.ended.notify_all();
                }
            }),
        );
        self.inner.lock().unwrap().feed = Some(strong);
    }

    /// The batch ended: publishes the state its records make if `digest`
    /// is theirs, else asks for every record again.
    fn end(&self, digest: &[u8], token: i64) {
        let mut inner = self.inner.lock().unwrap();
        match inner.end(digest, token) {
            Ok(state) => {
                if let Some(path) = &self.dump {
                    let partial = path.with_extension("partial");
                    if let Err(e) = std::fs::write(&partial, dump(&state))
                        .and_then(|()| std::fs::rename(&partial, path))
                    {
                        eprintln!("package feed: {}: {e}", path.display());
                    }
                }
            }
            Err(Failed::Drifted) => {
                eprintln!("package feed: batch {token} drifted from the original; asking for all");
                let _ = self.ask(&mut inner, true, None);
            }
            Err(Failed::Unreadable(e)) => eprintln!("package feed: batch {token}: {e}"),
        }
        self.ended.notify_all();
    }
}

impl Inner {
    fn begin(&mut self, reset: bool) {
        self.partial = None;
        if reset {
            self.records.clear();
        }
    }

    fn put(&mut self, key: Key, length: usize, chunk: &[u8]) -> Result<(), String> {
        let mut bytes = match self.partial.take() {
            Some((k, l, bytes)) if k == key && l == length => bytes,
            Some((k, ..)) => return Err(format!("{k:?} ended early")),
            None => Vec::with_capacity(length),
        };
        bytes.extend_from_slice(chunk);
        match bytes.len().cmp(&length) {
            Ordering::Less => self.partial = Some((key, length, bytes)),
            Ordering::Equal => {
                let hash = Sha256::digest(&bytes).into();
                self.records.insert(key, (bytes.into(), hash));
            }
            Ordering::Greater => return Err(format!("{key:?} is longer than {length} bytes")),
        }
        Ok(())
    }

    /// The batch with `token` ended with the original's `digest`: the
    /// state its records make, now published, or why there is none. The
    /// state's nonce is the one read before `token` was asked for: the
    /// batch's snapshot was taken after that.
    fn end(&mut self, digest: &[u8], token: i64) -> Result<Arc<State>, Failed> {
        self.ended = Some((token, false));
        self.asked.retain(|(t, _)| *t >= token);
        if digest != records_digest(&self.records) {
            return Err(Failed::Drifted);
        }
        let nonce = self
            .asked
            .iter()
            .find(|(t, _)| *t == token)
            .and_then(|(_, n)| *n);
        let generation = self.state.as_ref().map_or(1, |s| s.generation + 1);
        let state = Arc::new(build(&self.records, generation, nonce).map_err(Failed::Unreadable)?);
        self.state = Some(state.clone());
        self.ended = Some((token, true));
        Ok(state)
    }
}

/// Why a batch made no state.
#[derive(Debug, PartialEq)]
enum Failed {
    /// The records differ from the original's: some batch was lost.
    Drifted,
    /// A record does not read.
    Unreadable(String),
}

/// The SHA-256 of `records` as `PackageFeed.digest` computes it: per
/// record in key order, its kind and its key's length (big-endian), the
/// key's UTF-8 bytes and the record's own SHA-256.
fn records_digest(records: &BTreeMap<Key, (Arc<[u8]>, [u8; 32])>) -> [u8; 32] {
    let mut d = Sha256::new();
    for (key, (_, hash)) in records {
        d.update(key.kind.to_be_bytes());
        d.update((key.name.len() as i32).to_be_bytes());
        d.update(key.name.as_bytes());
        d.update(hash);
    }
    d.finalize().into()
}

/// The state `records` make.
fn build(
    records: &BTreeMap<Key, (Arc<[u8]>, [u8; 32])>,
    generation: u64,
    nonce: Option<i64>,
) -> Result<State, String> {
    let mut state = State {
        generation,
        nonce,
        ..State::default()
    };
    let mut shared_user_ids = Vec::new();
    for (key, (bytes, _)) in records {
        let failed = |s| format!("{key:?}: status {s}");
        match key.kind {
            PACKAGE | DISABLED_SYSTEM_PACKAGE => {
                let (package, shared_user) = record::package(bytes).map_err(failed)?;
                shared_user_ids.push((key.kind, key.name.clone(), shared_user));
                packages(&mut state, key.kind == PACKAGE).insert(key.name.clone(), package);
            }
            // After their packages, in key order.
            PARSED | DISABLED_SYSTEM_PARSED => {
                if let Some(p) = packages(&mut state, key.kind == PARSED).get_mut(&key.name) {
                    p.parcel = Some(bytes.clone());
                }
            }
            SHARED_USER => {
                let user = record::shared_user(bytes).map_err(failed)?;
                state.shared_users.insert(key.name.clone(), user);
            }
            USER => {
                let user = record::user(bytes).map_err(failed)?;
                state.users.insert(user.id, user);
            }
            SYSTEM => {
                let (all, packages, platform) = record::system(bytes).map_err(failed)?;
                state.system.force_system_packages_queryable = all;
                state.system.force_queryable_packages = packages;
                state.platform = platform;
            }
            _ => return Err(format!("{key:?}: no such kind")),
        }
    }
    for (kind, name, app_id) in shared_user_ids {
        let shared_user = app_id.and_then(|id| {
            let mut users = state.shared_users.values();
            users.find(|u| u.app_id == id).map(|u| u.name.clone())
        });
        if let Some(p) = packages(&mut state, kind == PACKAGE).get_mut(&name) {
            p.shared_user = shared_user;
        }
    }
    Ok(state)
}

fn packages(state: &mut State, installed: bool) -> &mut BTreeMap<String, PackageState> {
    if installed {
        &mut state.packages
    } else {
        &mut state.disabled_system_packages
    }
}

/// `state` in `dumpsys package`'s words where it has them (its
/// "Packages:", "Hidden system packages:" and "Shared users:" sections),
/// for comparing the two; times in ms.
pub fn dump(state: &State) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "generation={} nonce={:?}", state.generation, state.nonce);
    for (title, packages) in [
        ("Packages:", &state.packages),
        ("Hidden system packages:", &state.disabled_system_packages),
    ] {
        let _ = writeln!(s, "{title}");
        for p in packages.values() {
            dump_package(&mut s, p);
        }
    }
    let _ = writeln!(s, "Shared users:");
    for u in state.shared_users.values() {
        let _ = writeln!(s, "  SharedUser [{}]:", u.name);
        let _ = writeln!(s, "    appId={}", u.app_id);
        let _ = writeln!(s, "    packages={}", u.packages.join(","));
        let _ = writeln!(s, "    signatures={}", signatures(u.signatures.as_ref()));
    }
    let _ = writeln!(s, "Users:");
    for u in state.users.values() {
        let preferred = u.preferred_activities.as_ref().map_or(0, Vec::len);
        let _ = writeln!(s, "  User {}: preferredActivities={preferred} bytes", u.id);
    }
    s
}

fn dump_package(s: &mut String, p: &PackageState) {
    let some = |v: &Option<String>| v.clone().unwrap_or_else(|| "null".into());
    let i = &p.install_source;
    let _ = writeln!(s, "  Package [{}]:", p.name);
    let mut fields = vec![("appId", p.app_id.to_string())];
    if let Some(u) = &p.shared_user {
        fields.push(("sharedUser", u.clone()));
    }
    fields.extend([
        ("codePath", p.path.clone()),
        ("primaryCpuAbi", some(&p.primary_cpu_abi)),
        ("secondaryCpuAbi", some(&p.secondary_cpu_abi)),
        ("cpuAbiOverride", some(&p.cpu_abi_override)),
        (
            "versionCode",
            format!("{} targetSdk={}", p.version_code, p.target_sdk_version),
        ),
        (
            "hiddenApiEnforcementPolicy",
            p.hidden_api_enforcement_policy.to_string(),
        ),
        ("timeStamp", p.last_modified_time.to_string()),
        ("lastUpdateTime", p.last_update_time.to_string()),
        ("installerPackageName", some(&i.installer)),
        ("initiatingPackageName", some(&i.initiating_package)),
        ("originatingPackageName", some(&i.originating_package)),
        ("packageSource", i.package_source.to_string()),
        ("signatures", signatures(p.signatures.as_ref())),
        (
            "installPermissionsFixed",
            p.is.install_permissions_fixed.to_string(),
        ),
        (
            "scannedAsStoppedSystemApp",
            p.is.scanned_as_stopped_system_app.to_string(),
        ),
    ]);
    fields.push(("apexModuleName", some(&p.apex_module_name)));
    if p.is.apex {
        // dumpsys lists APEX packages apart, not in this section.
        fields.push(("isApex", "true".into()));
    }
    let parcel = p.parcel.as_ref().map_or(0, |b| b.len());
    fields.push(("parcel", parcel.to_string()));
    for (key, value) in fields {
        let _ = writeln!(s, "    {key}={value}");
    }
    list(s, "    usesLibraryFiles:", &p.uses_library_files);
    for (id, u) in &p.users {
        let _ = writeln!(
            s,
            "    User {id}: ceDataInode={} deDataInode={} installed={} hidden={} suspended={} \
             distractionFlags={} stopped={} notLaunched={} enabled={} instant={} virtual={} \
             quarantined={}",
            u.ce_data_inode,
            u.de_data_inode,
            u.installed,
            u.hidden,
            !u.suspended_by.is_empty(),
            u.distraction_flags,
            u.stopped,
            u.not_launched,
            u.enabled,
            u.instant_app,
            u.virtual_preload,
            u.quarantined
        );
        let _ = writeln!(s, "      installReason={}", u.install_reason);
        let _ = writeln!(s, "      firstInstallTime={}", u.first_install_time);
        let _ = writeln!(s, "      uninstallReason={}", u.uninstall_reason);
        if let Some(caller) = &u.last_disable_app_caller {
            let _ = writeln!(s, "      lastDisabledCaller: {caller}");
        }
        if let Some(o) = &u.overlay_paths {
            list(s, "      overlay paths:", &o.overlay_paths);
        }
        list(s, "      disabledComponents:", &u.disabled_components);
        list(s, "      enabledComponents:", &u.enabled_components);
    }
}

fn list(s: &mut String, title: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    let _ = writeln!(s, "{title}");
    let indent = " ".repeat(title.len() - title.trim_start().len() + 2);
    for item in items {
        let _ = writeln!(s, "{indent}{item}");
    }
}

/// As `PackageSignatures.toString` names them: the scheme, then each
/// signature's `hashCode` in hex.
fn signatures(s: Option<&super::settings::Signatures>) -> String {
    let Some(s) = s else {
        return "null".into();
    };
    let hex = |der: &[u8]| format!("{:x}", java_hash(der));
    let past: Vec<String> = s
        .past_signatures
        .iter()
        .flatten()
        .map(|(der, flags)| format!("{} flags: {flags:x}", hex(der)))
        .collect();
    format!(
        "version:{}, signatures:[{}], past signatures:[{}]",
        s.scheme_version,
        s.signatures
            .iter()
            .map(|d| hex(d))
            .collect::<Vec<_>>()
            .join(", "),
        past.join(", ")
    )
}

/// `Arrays.hashCode(byte[])`, `Signature.hashCode`.
fn java_hash(bytes: &[u8]) -> u32 {
    bytes.iter().fold(1i32, |h, &b| {
        h.wrapping_mul(31).wrapping_add(b as i8 as i32)
    }) as u32
}

/// The feed's `IPackageFeedHost` node.
struct HostNode {
    feed: Weak<Feed>,
}

impl Service for HostNode {
    fn descriptor(&self) -> &str {
        host::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // Only system_server's bridge feeds it.
        if call.sender_euid != SYSTEM_UID {
            return Err(UNKNOWN_TRANSACTION);
        }
        let Some(feed) = self.feed.upgrade() else {
            return Err(UNKNOWN_TRANSACTION);
        };
        let mut reply = Parcel::new();
        match call.code {
            host::BEGIN => {
                let reset = host::Begin::read(&mut call.data)?.reset;
                feed.inner.lock().unwrap().begin(reset);
                host::write_begin_reply(&mut reply);
            }
            host::PUT => {
                let a = host::Put::read(&mut call.data)?;
                let (Some(name), Ok(length), Some(chunk)) =
                    (a.key, usize::try_from(a.length), a.chunk)
                else {
                    return Err(BAD_VALUE);
                };
                let key = Key { kind: a.kind, name };
                if let Err(e) = feed.inner.lock().unwrap().put(key, length, &chunk) {
                    eprintln!("package feed: {e}");
                    return Err(BAD_VALUE);
                }
                host::write_put_reply(&mut reply);
            }
            host::REMOVE => {
                let a = host::Remove::read(&mut call.data)?;
                let key = Key {
                    kind: a.kind,
                    name: a.key.ok_or(BAD_VALUE)?,
                };
                feed.inner.lock().unwrap().records.remove(&key);
                host::write_remove_reply(&mut reply);
            }
            host::END => {
                let a = host::End::read(&mut call.data)?;
                feed.end(&a.digest.unwrap_or_default(), a.token);
                host::write_end_reply(&mut reply);
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests;
