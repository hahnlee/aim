//! Apps' content providers in the app host (`ContentProviderHelper` at
//! `android-16.0.0_r1`, ADR 0013): the providers a process publishes
//! (`publishContentProviders`, of those its `bindApplication` or
//! `scheduleInstallProvider` handed it), and `getContentProviderImpl` for
//! another process: the caller's access checked
//! (`checkContentProviderPermission`), a provider it may run itself
//! returned as local (`canRunHere`), else the provider's process started
//! (or told to install it) and its publish waited for; the connections
//! counted (`refContentProvider`, `removeContentProvider`), so that a
//! provider's process's death kills its stable clients and tells its
//! unstable ones (`removeDyingProviderLocked`). The app host keeps every
//! process it started until it dies or its task is removed, so external
//! references (`getContentProviderExternal`) keep nothing alive and are not
//! counted. The app host is a provider client itself, as system_server's
//! `ContentResolver` is one: `getType` (through `getMimeTypeFilterAsync`
//! for an identity other than the system's) and `openInputStream`.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::fs::{FileExt, FileTypeExt};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_SECURITY, EX_UNSUPPORTED_OPERATION, Exception, Parcel,
    Reader, UNKNOWN_TRANSACTION,
};
use aim_binder_host::server::RetainedFd;
use aim_service_aidl::android_app_iapplicationthread as thread;
use aim_service_aidl::android_content_icontentprovider as cp;
use aim_service_aidl::android_os_iremotecallback as remote_callback;

use super::uri_grants::write_attribution_source;
use super::{
    AppHost, GET_URI_PERMISSION_PATTERNS, MATCH_DIRECT_BOOT_AUTO, PER_USER_RANGE, STOCK_PM_FLAGS,
    activity, exit_info, field,
};
use crate::bundle::{self, Value};
use crate::package;
use crate::package::info::ProviderInfo;
use crate::package::uri::Uri;

/// `ContentProviderHelper.CONTENT_PROVIDER_WAIT_TIMEOUT`.
const WAIT_TIMEOUT: Duration = Duration::from_secs(20);
/// `ContentResolver.CONTENT_PROVIDER_TIMEOUT_MILLIS`,
/// `REMOTE_CONTENT_PROVIDER_TIMEOUT_MILLIS` and `REMOTE_CALLBACK_RESULT`.
const CONTENT_PROVIDER_TIMEOUT: Duration = Duration::from_secs(3);
const REMOTE_CONTENT_PROVIDER_TIMEOUT: Duration = Duration::from_secs(23);
const REMOTE_CALLBACK_RESULT: &str = "result";
/// `Process.SYSTEM_UID` and system_server's package.
const SYSTEM_UID: i32 = 1000;
const SYSTEM_PACKAGE: &str = "android";
const GET_ANY_PROVIDER_TYPE: &str = "android.permission.GET_ANY_PROVIDER_TYPE";

/// `ContentProviderHolder`.
pub struct Holder {
    pub info: ProviderInfo,
    pub provider: Option<Binder>,
    pub connection: Option<Binder>,
    pub no_release_needed: bool,
    pub local: bool,
}

impl aim_service_aidl::WriteParcelable for Holder {
    fn write_to(&self, p: &mut Parcel) {
        self.info.write_to(p);
        p.write_binder(self.provider);
        p.write_binder(self.connection);
        p.write_i32(self.no_release_needed as i32);
        p.write_i32(self.local as i32);
    }
}

/// A provider a process published.
struct Published {
    start_seq: i64,
    info: ProviderInfo,
    provider: Strong,
}

/// `ContentProviderConnection`: a client process's references to a
/// published provider.
struct Connection {
    token: Binder,
    client: i64,
    /// The provider's process, its class and its binder.
    provider_seq: i64,
    class: String,
    provider: Binder,
    stable: i32,
    unstable: i32,
}

#[derive(Default)]
struct State {
    /// The published providers, by authority.
    published: HashMap<String, Published>,
    connections: Vec<Connection>,
}

#[derive(Default)]
pub struct Providers {
    state: Mutex<State>,
    /// Signalled when a provider is published or a process dies.
    changed: Condvar,
}

/// `ComponentInfo.processName`: the component's process, else its
/// application's.
pub(super) fn process_name(info: &package::info::ComponentInfo) -> String {
    info.process_name
        .clone()
        .or_else(|| info.application_info.process_name.clone())
        .or_else(|| info.application_info.item.package_name.clone())
        .unwrap_or_default()
}

/// `ContentProviderRecord.noReleaseNeeded`: a provider of root or the
/// system.
fn no_release_needed(info: &ProviderInfo) -> bool {
    matches!(
        info.info
            .application_info
            .uid
            .rem_euclid(PER_USER_RANGE as i32),
        0 | 1000
    )
}

/// What a wait for a provider does next.
enum Next {
    Start(String),
    Install(i64),
    Wait,
}

/// Of the system process's providers, those `installSystemProviders`
/// installs (of system apps) that the app host leaves to it: not the
/// `settings` provider, which the app host serves itself, nor the
/// `android` package's, system_server's code (#1518).
fn system_process_provider(p: &ProviderInfo) -> bool {
    p.info.application_info.flags & crate::package::settings::FLAG_SYSTEM != 0
        && p.info.item.package_name.as_deref() != Some(super::SYSTEM_PACKAGE)
        && !p
            .authority
            .as_deref()
            .is_some_and(|a| a.split(';').any(|a| a == super::SETTINGS_AUTHORITY))
}

impl AppHost {
    /// `generateApplicationProvidersLocked`: the providers of the process
    /// `process` of `uid`.
    pub(super) fn process_providers(
        &self,
        process: &str,
        uid: u32,
    ) -> Result<Vec<ProviderInfo>, Exception> {
        let (model, resolution) = self.packages()?;
        let query = crate::package::query::Query {
            state: &model,
            filter: &resolution.apps_filter,
            calling_uid: 1000,
        };
        let flags = STOCK_PM_FLAGS | GET_URI_PERMISSION_PATTERNS | MATCH_DIRECT_BOOT_AUTO;
        match query.query_content_providers(Some(process), uid as i32, flags, None) {
            // The system process's are `installSystemProviders`' (its
            // system apps'): the app host serves `settings` itself, and the
            // `android` package's are system_server's services' code (#1518).
            Ok(Ok(providers)) if process == super::SYSTEM_PROCESS && uid == super::SYSTEM_UID => {
                Ok(providers
                    .into_iter()
                    .filter(system_process_provider)
                    .collect())
            }
            Ok(Ok(providers)) => Ok(providers),
            Ok(Err(e)) => Err(e),
            Err(crate::package::apps_filter::NotModelled(reason)) => Err(Exception::new(
                EX_UNSUPPORTED_OPERATION,
                format!("queryContentProviders: {reason}"),
            )),
        }
    }

    /// `publishContentProviders`: the providers the caller's process
    /// installed, of those it was handed.
    pub(super) fn publish_providers(&self, call: &mut Call<'_>) -> Result<(), Exception> {
        let bad = || Exception::illegal_argument("publishContentProviders");
        let (start_seq, ours) = self
            .apps
            .lock()
            .unwrap()
            .iter()
            .find(|(_, a)| a.pid == call.sender_pid && a.uid == call.sender_euid)
            .map(|(seq, a)| (*seq, a.providers.clone()))
            .ok_or_else(|| {
                Exception::security("publishContentProviders from an unknown process")
            })?;
        call.data
            .enforce_interface(super::am::DESCRIPTOR)
            .map_err(|_| bad())?;
        call.data.read_binder().map_err(|_| bad())?;
        let n = call.data.read_i32().map_err(|_| bad())?;
        let mut state = self.providers.state.lock().unwrap();
        for _ in 0..n.max(0) {
            // `ContentProviderHolder`: its `ProviderInfo`, its provider,
            // its connection, two flags.
            if call.data.read_i32().map_err(|_| bad())? == 0 {
                continue;
            }
            let info = crate::package::reply::provider_info(&mut call.data).map_err(|_| bad())?;
            let provider = call.data.read_binder().map_err(|_| bad())?;
            call.data.read_binder().map_err(|_| bad())?;
            call.data.read_i32().map_err(|_| bad())?;
            call.data.read_i32().map_err(|_| bad())?;
            let name = field(&info, "name");
            let (Some(name), Some(Binder::Handle(handle))) = (name, provider) else {
                continue;
            };
            let Some(ours) = ours
                .iter()
                .find(|p| p.info.item.name.as_deref() == Some(name))
            else {
                continue;
            };
            for authority in ours.authority.iter().flat_map(|a| a.split(';')) {
                let provider = self.process.strong(handle);
                state.published.insert(
                    authority.to_string(),
                    Published {
                        start_seq,
                        info: ours.clone(),
                        provider,
                    },
                );
            }
            eprintln!(
                "app host: {name} published ({})",
                ours.authority.as_deref().unwrap_or("")
            );
        }
        self.providers.changed.notify_all();
        Ok(())
    }

    /// `getContentProviderImpl` of `name` in `user` for `uid`, from the
    /// process of `client` (none for an external reference): the provider,
    /// its process started and its publish waited for; none when no
    /// provider has the authority, or when it was not published in time.
    pub(super) fn provider(
        &self,
        client: Option<i64>,
        uid: i32,
        name: &str,
        user: i32,
        stable: bool,
    ) -> Result<Option<Holder>, Exception> {
        let flags = STOCK_PM_FLAGS | GET_URI_PERMISSION_PATTERNS;
        let Some(cpi) = self.resolve_provider(name, flags, user)? else {
            return Ok(None);
        };
        if let Some(denial) = self.provider_access(name, uid, user)? {
            return Err(Exception::security(denial));
        }
        let owner = cpi.info.application_info.uid as u32;
        let process = process_name(&cpi.info);
        let class = cpi.info.item.name.clone().unwrap_or_default();
        let release = no_release_needed(&cpi);
        // `canRunHere`: the client instantiates it itself.
        let client_app = client.and_then(|seq| {
            self.apps
                .lock()
                .unwrap()
                .get(&seq)
                .map(|a| (a.process.clone(), a.uid))
        });
        if let Some((client_process, client_uid)) = client_app
            && (cpi.multiprocess || client_process == process)
            && client_uid == owner
        {
            return Ok(Some(Holder {
                info: cpi,
                provider: None,
                connection: None,
                no_release_needed: release,
                local: true,
            }));
        }
        // The provider's app may now see its client (`grantImplicitAccess`).
        self.grant_implicit_access(owner as i32, uid)?;
        let deadline = Instant::now() + WAIT_TIMEOUT;
        let mut launched = false;
        loop {
            let next = {
                let mut state = self.providers.state.lock().unwrap();
                if let Some(p) = state.published.get(name) {
                    let (info, provider, provider_seq) =
                        (p.info.clone(), p.provider.binder(), p.start_seq);
                    let connection = client.map(|client| {
                        // `incProviderCountLocked`.
                        let found = state.connections.iter_mut().find(|c| {
                            c.client == client && c.provider_seq == provider_seq && c.class == class
                        });
                        let c = match found {
                            Some(c) => c,
                            None => {
                                let token = self
                                    .process
                                    .add_service(std::sync::Arc::new(activity::Token));
                                state.connections.push(Connection {
                                    token,
                                    client,
                                    provider_seq,
                                    class: class.clone(),
                                    provider,
                                    stable: 0,
                                    unstable: 0,
                                });
                                state.connections.last_mut().unwrap()
                            }
                        };
                        if stable {
                            c.stable += 1
                        } else {
                            c.unstable += 1
                        }
                        c.token
                    });
                    return Ok(Some(Holder {
                        info,
                        provider: Some(provider),
                        connection,
                        no_release_needed: release,
                        local: false,
                    }));
                }
                match self.running_process(&process, owner) {
                    None if launched => {
                        eprintln!(
                            "app host: unable to launch {process} for provider {name}: process is gone"
                        );
                        return Ok(None);
                    }
                    None => Next::Start(
                        cpi.info
                            .application_info
                            .item
                            .package_name
                            .clone()
                            .unwrap_or_default(),
                    ),
                    Some((seq, true)) if !self.handed(seq, &class) => Next::Install(seq),
                    Some(_) => {
                        let wait = deadline.saturating_duration_since(Instant::now());
                        if wait.is_zero() {
                            eprintln!("app host: timeout waiting for provider {name} of {process}");
                            return Ok(None);
                        }
                        drop(self.providers.changed.wait_timeout(state, wait).unwrap());
                        Next::Wait
                    }
                }
            };
            match next {
                Next::Start(package) => {
                    self.start_process_for(&package, &process, None, false)?;
                    launched = true;
                }
                Next::Install(seq) => {
                    // `scheduleInstallProvider` to a running process not
                    // handed it.
                    if let Some(app) = self.apps.lock().unwrap().get_mut(&seq) {
                        app.providers.push(cpi.clone());
                    }
                    let mut p = Parcel::new();
                    thread::ScheduleInstallProvider {
                        provider: Some(cpi.clone()),
                    }
                    .write(&mut p);
                    self.call_thread(seq, thread::SCHEDULE_INSTALL_PROVIDER, &p)
                        .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
                    launched = true;
                }
                Next::Wait => {}
            }
        }
    }

    /// Whether the process of `start_seq` was handed the provider `class`.
    fn handed(&self, start_seq: i64, class: &str) -> bool {
        self.apps.lock().unwrap().get(&start_seq).is_some_and(|a| {
            a.providers
                .iter()
                .any(|p| p.info.item.name.as_deref() == Some(class))
        })
    }

    /// `refContentProvider`: the connection's counts adjusted; whether it
    /// is still alive.
    pub(super) fn ref_provider(
        &self,
        connection: Option<Binder>,
        stable: i32,
        unstable: i32,
    ) -> Result<bool, Exception> {
        let token = Self::connection(connection, "refContentProvider")?;
        let mut state = self.providers.state.lock().unwrap();
        let Some(c) = state.connections.iter_mut().find(|c| c.token == token) else {
            // Its provider died.
            return Ok(false);
        };
        if c.stable + stable < 0 || c.unstable + unstable < 0 {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                format!(
                    "stableCount < 0: {} or unstableCount < 0: {}",
                    c.stable + stable,
                    c.unstable + unstable
                ),
            ));
        }
        c.stable += stable;
        c.unstable += unstable;
        Ok(true)
    }

    /// `removeContentProvider` (`decProviderCountLocked`): one reference
    /// less, the connection gone with its last.
    pub(super) fn remove_provider(
        &self,
        connection: Option<Binder>,
        stable: bool,
    ) -> Result<(), Exception> {
        let token = Self::connection(connection, "removeContentProvider")?;
        let mut state = self.providers.state.lock().unwrap();
        if let Some(c) = state.connections.iter_mut().find(|c| c.token == token) {
            if stable {
                c.stable -= 1
            } else {
                c.unstable -= 1
            }
            if c.stable <= 0 && c.unstable <= 0 {
                state.connections.retain(|c| c.token != token);
            }
        }
        Ok(())
    }

    /// A connection the app host handed out (one whose provider died is
    /// still one).
    fn connection(connection: Option<Binder>, method: &str) -> Result<Binder, Exception> {
        match connection {
            Some(b @ Binder::Local(_)) => Ok(b),
            Some(_) => Err(Exception::illegal_argument(format!(
                "{method}: not a ContentProviderConnection"
            ))),
            None => Err(Exception::new(
                aim_binder_host::parcel::EX_NULL_POINTER,
                "connection is null",
            )),
        }
    }

    /// The process of `start_seq` died (`removeDyingProviderLocked` for
    /// its providers, its own connections removed): its stable clients are
    /// killed, its unstable ones told; waits for a provider look again.
    pub(super) fn providers_process_died(&self, start_seq: i64) {
        // Its providers are held until their clients are told of them.
        let (gone, dying) = {
            let mut state = self.providers.state.lock().unwrap();
            let gone: Vec<Published> = state
                .published
                .extract_if(|_, p| p.start_seq == start_seq)
                .map(|(_, p)| p)
                .collect();
            state.connections.retain(|c| c.client != start_seq);
            let (dying, kept) = std::mem::take(&mut state.connections)
                .into_iter()
                .partition::<Vec<_>, _>(|c| c.provider_seq == start_seq);
            state.connections = kept;
            (gone, dying)
        };
        self.providers.changed.notify_all();
        for c in dying {
            if c.stable > 0 {
                eprintln!(
                    "app host: killing start {} as it depends on provider {} in dying start {start_seq}",
                    c.client, c.class
                );
                // `killLocked`'s description: the provider's
                // `flattenToShortString` and its process, whose adj is the
                // top's (`FOREGROUND_APP_ADJ`).
                let dying = gone
                    .iter()
                    .find(|p| p.info.info.item.name.as_deref() == Some(c.class.as_str()));
                let name = dying
                    .and_then(|p| p.info.info.item.package_name.clone())
                    .map_or(c.class.clone(), |package| {
                        let short = c
                            .class
                            .strip_prefix(package.as_str())
                            .filter(|r| r.starts_with('.'))
                            .unwrap_or(&c.class);
                        format!("{package}/{short}")
                    });
                let process = dying.map_or("??".to_string(), |p| process_name(&p.info.info));
                let description =
                    format!("depends on provider {name} in dying proc {process} (adj 0)");
                if let Err(e) = self.kill_process(
                    c.client,
                    exit_info::REASON_DEPENDENCY_DIED,
                    exit_info::SUBREASON_UNKNOWN,
                    &description,
                ) {
                    eprintln!("app host: kill start {}: {e}", c.client);
                }
            } else {
                let mut p = Parcel::new();
                thread::UnstableProviderDied {
                    provider: Some(c.provider),
                }
                .write(&mut p);
                if let Err(e) = self.call_thread(c.client, thread::UNSTABLE_PROVIDER_DIED, &p) {
                    eprintln!("app host: unstableProviderDied to start {}: {e}", c.client);
                }
            }
        }
    }
}

/// A `RemoteCallback` the app host passes (its `IRemoteCallback`): the
/// bundle it is sent.
struct ResultCallback(Mutex<mpsc::Sender<HashMap<String, Value>>>);

impl Service for ResultCallback {
    fn descriptor(&self) -> &str {
        remote_callback::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != remote_callback::SEND_RESULT {
            return Err(UNKNOWN_TRANSACTION);
        }
        let r = &mut call.data;
        r.enforce_interface(remote_callback::DESCRIPTOR)
            .map_err(|_| BAD_VALUE)?;
        let result = if r.read_i32().map_err(|_| BAD_VALUE)? != 0 {
            bundle::read(r).map_err(|_| BAD_VALUE)?
        } else {
            None
        };
        let _ = self.0.lock().unwrap().send(result.unwrap_or_default());
        Ok(Parcel::new())
    }
}

/// `AssetFileDescriptor.createInputStream` of a provider's file: from its
/// start for its length when it has one (`SeekableAutoCloseInputStream`,
/// or skipped to by `NonSeekableAutoCloseInputStream`), else to its end.
pub struct ProviderInput {
    fd: RetainedFd,
    offset: u64,
    remaining: Option<u64>,
    seekable: bool,
}

impl Read for ProviderInput {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let Some(remaining) = self.remaining else {
            return self.fd.read(buffer);
        };
        let len = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        if len == 0 {
            return Ok(0);
        }
        let n = if self.seekable {
            self.fd.read_at(&mut buffer[..len], self.offset)?
        } else {
            self.fd.read(&mut buffer[..len])?
        };
        self.offset += n as u64;
        self.remaining = Some(remaining - n as u64);
        Ok(n)
    }
}

/// A provider's reply status: a `DatabaseUtils.writeExceptionToParcel`
/// exception as `readExceptionFromParcel` throws it, or the binder's.
fn provider_status(r: &mut Reader<'_>) -> Result<Result<(), String>, i32> {
    let start = r.position();
    let code = r.read_i32()?;
    if code > 0 {
        let class = match code {
            1 => "java.io.FileNotFoundException",
            2 => "java.lang.IllegalArgumentException",
            3 => "java.lang.UnsupportedOperationException",
            11 => "android.os.OperationCanceledException",
            _ => "android.database.sqlite.SQLiteException",
        };
        return Ok(Err(format!(
            "{class}: {}",
            r.read_string16()?.unwrap_or_default()
        )));
    }
    r.set_position(start);
    Ok(r.read_exception()?.map_err(|e| e.message))
}

/// `ContentProvider.getAuthorityWithoutUserId` and the user of a URI's
/// authority (`resolveUserIdFromAuthority`, user 0 for system_server).
fn authority_and_user(uri: &Uri) -> Option<(String, i32)> {
    let authority = uri.authority()?;
    Some(match authority.rsplit_once('@') {
        Some((user, name)) => (name.to_string(), user.parse().unwrap_or(-10_000)),
        None => (authority, 0),
    })
}

impl AppHost {
    /// `ContentResolver.getType` of `uri` from system_server, run as `uid`.
    /// As the system it asks the provider it acquires (`getTypeAsync`); an
    /// app's identity (one SettingsProvider did not clear) does not own
    /// system_server's package, so acquiring fails and AMS's
    /// `getMimeTypeFilterAsync` answers: none for a provider hidden from
    /// the app, `getTypeAnonymous` without GET_ANY_PROVIDER_TYPE. A failure
    /// is a type of none, as the resolver logs it.
    pub(super) fn provider_type(&self, uri: &Uri, uid: i32) -> Result<Option<String>, Exception> {
        let Some((name, user)) =
            authority_and_user(uri).filter(|_| uri.scheme() == Some("content"))
        else {
            return Ok(None);
        };
        let system = uid.rem_euclid(PER_USER_RANGE as i32) == SYSTEM_UID;
        let holder = match self.provider(None, SYSTEM_UID, &name, user, true) {
            Ok(holder) => holder,
            Err(e) if e.code == EX_SECURITY => {
                eprintln!(
                    "app host: Failed to get type for: {} ({})",
                    uri.as_str(),
                    e.message
                );
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let Some((Some(Binder::Handle(handle)), info)) = holder.map(|h| (h.provider, h.info))
        else {
            return Ok(None);
        };
        // `isHolderVisibleToCaller`.
        let package = info.info.item.package_name.unwrap_or_default();
        if !system && self.filter_package_access(&package, uid, user)? {
            return Ok(None);
        }
        let (sender, results) = mpsc::channel();
        let callback = self
            .process
            .add_service(Arc::new(ResultCallback(Mutex::new(sender))));
        let mut p = Parcel::new();
        p.write_interface_token(cp::DESCRIPTOR);
        let code = if system || self.check_permission(Some(GET_ANY_PROVIDER_TYPE), uid)? == 0 {
            write_attribution_source(&mut p, uid, system.then_some(SYSTEM_PACKAGE));
            cp::GET_TYPE_ASYNC_TRANSACTION
        } else {
            cp::GET_TYPE_ANONYMOUS_ASYNC_TRANSACTION
        };
        // AMS asks of the URI without its user (`getUriWithoutUserId`).
        if system {
            uri.clone()
        } else {
            uri.with_encoded_authority(Some(&name))
        }
        .write(&mut p);
        p.write_binder(Some(callback));
        if let Err(status) = self.process.transact(handle, code, &p, true) {
            eprintln!(
                "app host: Failed to get type for: {} (binder status {status})",
                uri.as_str()
            );
            return Ok(None);
        }
        let timeout = if system {
            CONTENT_PROVIDER_TIMEOUT
        } else {
            REMOTE_CONTENT_PROVIDER_TIMEOUT
        };
        let Ok(result) = results.recv_timeout(timeout) else {
            return Ok(None);
        };
        Ok(match result.get(REMOTE_CALLBACK_RESULT) {
            Some(Value::String(mime_type)) => mime_type.clone(),
            _ => None,
        })
    }

    /// `ContentResolver.openInputStream` of a `content` URI from
    /// system_server as the system: the provider's `openTypedAssetFile` of
    /// any type, read as its `AssetFileDescriptor` says; else the
    /// exception thrown, as it prints.
    pub(super) fn open_provider_input(&self, uri: &Uri) -> Result<ProviderInput, String> {
        let not_found = || {
            format!(
                "java.io.FileNotFoundException: No content provider: {}",
                uri.as_str()
            )
        };
        let (name, user) = authority_and_user(uri)
            .filter(|_| uri.scheme() == Some("content"))
            .ok_or_else(not_found)?;
        let holder = self
            .provider(None, SYSTEM_UID, &name, user, false)
            .map_err(|e| e.message)?;
        let Some(Binder::Handle(handle)) = holder.and_then(|h| h.provider) else {
            return Err(not_found());
        };
        let mut p = Parcel::new();
        p.write_interface_token(cp::DESCRIPTOR);
        write_attribution_source(&mut p, SYSTEM_UID, Some(SYSTEM_PACKAGE));
        uri.write(&mut p);
        p.write_string16(Some("*/*"));
        p.write_i32(-1); // no options
        p.write_binder(None); // no cancellation signal
        let reply = self
            .process
            .transact(handle, cp::OPEN_TYPED_ASSET_FILE_TRANSACTION, &p, false)
            .map_err(|status| format!("android.os.DeadObjectException: binder status {status}"))?;
        let mut r = reply.reader();
        let bad = |status: i32| format!("java.io.IOException: bad reply: {status}");
        provider_status(&mut r).map_err(bad)??;
        if r.read_i32().map_err(bad)? == 0 {
            return Err(format!("java.io.FileNotFoundException: {}", uri.as_str()));
        }
        // `ParcelFileDescriptor` (whether it has a comm fd, its fd and
        // that one), then the start and the length.
        let comm = r.read_i32().map_err(bad)? != 0;
        let fd = r.read_fd().map_err(bad)?;
        if comm {
            r.read_fd().map_err(bad)?;
        }
        let (start, length) = (r.read_i64().map_err(bad)?, r.read_i64().map_err(bad)?);
        let fd = self
            .process
            .file(fd)
            .and_then(|f| aim_binder_host::server::file_fd(&f))
            .ok_or_else(|| "java.io.IOException: no descriptor".to_string())?;
        let io = |e: std::io::Error| format!("java.io.IOException: {e}");
        let file_type = fd.metadata().map_err(io)?.file_type();
        let mut input = ProviderInput {
            fd,
            offset: start.max(0) as u64,
            remaining: u64::try_from(length).ok(),
            seekable: !(file_type.is_fifo() || file_type.is_socket()),
        };
        if input.remaining.is_some() && !input.seekable {
            std::io::copy(
                &mut (&mut input.fd).take(input.offset),
                &mut std::io::sink(),
            )
            .map_err(io)?;
        }
        Ok(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider's reply status: a `DatabaseUtils` exception (its code
    /// and message), the binder's, or none.
    #[test]
    fn provider_statuses() {
        let mut p = Parcel::new();
        p.write_i32(1);
        p.write_string16(Some("No such file"));
        assert_eq!(
            provider_status(&mut Reader::new(p.data(), &[])),
            Ok(Err("java.io.FileNotFoundException: No such file".into()))
        );
        let mut p = Parcel::new();
        p.write_exception(&Exception::security("denied"));
        assert_eq!(
            provider_status(&mut Reader::new(p.data(), &[])),
            Ok(Err("denied".into()))
        );
        let mut p = Parcel::new();
        p.write_no_exception();
        p.write_i32(7);
        let mut r = Reader::new(p.data(), &[]);
        assert_eq!(provider_status(&mut r), Ok(Ok(())));
        assert_eq!(r.read_i32(), Ok(7));
    }

    #[test]
    fn users_of_authorities() {
        assert_eq!(
            authority_and_user(&Uri::parse("content://media/internal/audio/media/1")),
            Some(("media".into(), 0))
        );
        assert_eq!(
            authority_and_user(&Uri::parse("content://10@media/x")),
            Some(("media".into(), 10))
        );
    }
}
