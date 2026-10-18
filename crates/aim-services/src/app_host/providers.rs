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
//! counted.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::HashMap;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, Strong};
use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, EX_UNSUPPORTED_OPERATION, Exception, Parcel};
use aim_service_aidl::android_app_iapplicationthread as thread;

use super::{AppHost, GET_URI_PERMISSION_PATTERNS, MATCH_DIRECT_BOOT_AUTO, PER_USER_RANGE, STOCK_PM_FLAGS, activity, field};
use crate::package;
use crate::package::info::ProviderInfo;

/// `ContentProviderHelper.CONTENT_PROVIDER_WAIT_TIMEOUT`.
const WAIT_TIMEOUT: Duration = Duration::from_secs(20);

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
    matches!(info.info.application_info.uid.rem_euclid(PER_USER_RANGE as i32), 0 | 1000)
}

/// What a wait for a provider does next.
enum Next {
    Start(String),
    Install(i64),
    Wait,
}

impl AppHost {
    /// `generateApplicationProvidersLocked`: the providers of the process
    /// `process` of `uid`.
    pub(super) fn process_providers(&self, process: &str, uid: u32) -> Result<Vec<ProviderInfo>, Exception> {
        let (model, resolution) = self.packages()?;
        let query = crate::package::query::Query { state: &model, filter: &resolution.apps_filter, calling_uid: 1000 };
        let flags = STOCK_PM_FLAGS | GET_URI_PERMISSION_PATTERNS | MATCH_DIRECT_BOOT_AUTO;
        match query.query_content_providers(Some(process), uid as i32, flags, None) {
            Ok(Ok(providers)) => Ok(providers),
            Ok(Err(e)) => Err(e),
            Err(crate::package::apps_filter::NotModelled(reason)) => {
                Err(Exception::new(EX_UNSUPPORTED_OPERATION, format!("queryContentProviders: {reason}")))
            }
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
            .ok_or_else(|| Exception::security("publishContentProviders from an unknown process"))?;
        call.data.enforce_interface(super::am::DESCRIPTOR).map_err(|_| bad())?;
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
            let (Some(name), Some(Binder::Handle(handle))) = (name, provider) else { continue };
            let Some(ours) = ours.iter().find(|p| p.info.item.name.as_deref() == Some(name)) else { continue };
            for authority in ours.authority.iter().flat_map(|a| a.split(';')) {
                let provider = self.process.strong(handle);
                state.published.insert(authority.to_string(), Published { start_seq, info: ours.clone(), provider });
            }
            eprintln!("app host: {name} published ({})", ours.authority.as_deref().unwrap_or(""));
        }
        self.providers.changed.notify_all();
        Ok(())
    }

    /// `getContentProviderImpl` of `name` in `user` for `uid`, from the
    /// process of `client` (none for an external reference): the provider,
    /// its process started and its publish waited for; none when no
    /// provider has the authority, or when it was not published in time.
    pub(super) fn provider(&self, client: Option<i64>, uid: i32, name: &str, user: i32, stable: bool) -> Result<Option<Holder>, Exception> {
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
        let client_app = client.and_then(|seq| self.apps.lock().unwrap().get(&seq).map(|a| (a.process.clone(), a.uid)));
        if let Some((client_process, client_uid)) = client_app
            && (cpi.multiprocess || client_process == process)
            && client_uid == owner
        {
            return Ok(Some(Holder { info: cpi, provider: None, connection: None, no_release_needed: release, local: true }));
        }
        let deadline = Instant::now() + WAIT_TIMEOUT;
        let mut launched = false;
        loop {
            let next = {
                let mut state = self.providers.state.lock().unwrap();
                if let Some(p) = state.published.get(name) {
                    let (info, provider, provider_seq) = (p.info.clone(), p.provider.binder(), p.start_seq);
                    let connection = client.map(|client| {
                        // `incProviderCountLocked`.
                        let found = state.connections.iter_mut().find(|c| c.client == client && c.provider_seq == provider_seq && c.class == class);
                        let c = match found {
                            Some(c) => c,
                            None => {
                                let token = self.process.add_service(std::sync::Arc::new(activity::Token));
                                state.connections.push(Connection { token, client, provider_seq, class: class.clone(), provider, stable: 0, unstable: 0 });
                                state.connections.last_mut().unwrap()
                            }
                        };
                        if stable { c.stable += 1 } else { c.unstable += 1 }
                        c.token
                    });
                    return Ok(Some(Holder { info, provider: Some(provider), connection, no_release_needed: release, local: false }));
                }
                match self.running_process(&process, owner) {
                    None if launched => {
                        eprintln!("app host: unable to launch {process} for provider {name}: process is gone");
                        return Ok(None);
                    }
                    None => Next::Start(cpi.info.application_info.item.package_name.clone().unwrap_or_default()),
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
                    self.start_process_for(&package, &process, None)?;
                    launched = true;
                }
                Next::Install(seq) => {
                    // `scheduleInstallProvider` to a running process not
                    // handed it.
                    if let Some(app) = self.apps.lock().unwrap().get_mut(&seq) {
                        app.providers.push(cpi.clone());
                    }
                    let mut p = Parcel::new();
                    thread::ScheduleInstallProvider { provider: Some(cpi.clone()) }.write(&mut p);
                    self.call_thread(seq, thread::SCHEDULE_INSTALL_PROVIDER, &p).map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
                    launched = true;
                }
                Next::Wait => {}
            }
        }
    }

    /// Whether the process of `start_seq` was handed the provider `class`.
    fn handed(&self, start_seq: i64, class: &str) -> bool {
        self.apps.lock().unwrap().get(&start_seq).is_some_and(|a| a.providers.iter().any(|p| p.info.item.name.as_deref() == Some(class)))
    }

    /// `refContentProvider`: the connection's counts adjusted; whether it
    /// is still alive.
    pub(super) fn ref_provider(&self, connection: Option<Binder>, stable: i32, unstable: i32) -> Result<bool, Exception> {
        let token = Self::connection(connection, "refContentProvider")?;
        let mut state = self.providers.state.lock().unwrap();
        let Some(c) = state.connections.iter_mut().find(|c| c.token == token) else {
            // Its provider died.
            return Ok(false);
        };
        if c.stable + stable < 0 || c.unstable + unstable < 0 {
            return Err(Exception::new(
                EX_ILLEGAL_STATE,
                format!("stableCount < 0: {} or unstableCount < 0: {}", c.stable + stable, c.unstable + unstable),
            ));
        }
        c.stable += stable;
        c.unstable += unstable;
        Ok(true)
    }

    /// `removeContentProvider` (`decProviderCountLocked`): one reference
    /// less, the connection gone with its last.
    pub(super) fn remove_provider(&self, connection: Option<Binder>, stable: bool) -> Result<(), Exception> {
        let token = Self::connection(connection, "removeContentProvider")?;
        let mut state = self.providers.state.lock().unwrap();
        if let Some(c) = state.connections.iter_mut().find(|c| c.token == token) {
            if stable { c.stable -= 1 } else { c.unstable -= 1 }
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
            Some(_) => Err(Exception::illegal_argument(format!("{method}: not a ContentProviderConnection"))),
            None => Err(Exception::new(aim_binder_host::parcel::EX_NULL_POINTER, "connection is null")),
        }
    }

    /// The process of `start_seq` died (`removeDyingProviderLocked` for
    /// its providers, its own connections removed): its stable clients are
    /// killed, its unstable ones told; waits for a provider look again.
    pub(super) fn providers_process_died(&self, start_seq: i64) {
        let dying = {
            let mut state = self.providers.state.lock().unwrap();
            state.published.retain(|_, p| p.start_seq != start_seq);
            state.connections.retain(|c| c.client != start_seq);
            let (dying, kept) = std::mem::take(&mut state.connections).into_iter().partition::<Vec<_>, _>(|c| c.provider_seq == start_seq);
            state.connections = kept;
            dying
        };
        self.providers.changed.notify_all();
        for c in dying {
            if c.stable > 0 {
                eprintln!("app host: killing start {} as it depends on provider {} in dying start {start_seq}", c.client, c.class);
                if let Err(e) = self.kill_process(c.client) {
                    eprintln!("app host: kill start {}: {e}", c.client);
                }
            } else {
                let mut p = Parcel::new();
                thread::UnstableProviderDied { provider: Some(c.provider) }.write(&mut p);
                if let Err(e) = self.call_thread(c.client, thread::UNSTABLE_PROVIDER_DIED, &p) {
                    eprintln!("app host: unstableProviderDied to start {}: {e}", c.client);
                }
            }
        }
    }
}
