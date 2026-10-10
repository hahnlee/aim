//! GentleUpdateHelper pending checks, android-16.0.0_r1 (AOSP Apache-2.0).
use aim_binder_host::parcel::{Exception, Reader, Result as ParcelResult};
use aim_service_aidl::ReadParcelable;
use std::{
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const MAX_TIMEOUT_MILLIS: i64 = 604_800_000;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Constraints(pub u8);
impl ReadParcelable for Constraints {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        Ok(Self(r.read_i32()? as u8))
    }
}
impl Constraints {
    pub fn satisfied(self, app_state: u8, device_idle: bool) -> bool {
        (self.0 & 1 == 0 || device_idle)
            && (self.0 & 2 == 0 || app_state & 1 == 0)
            && (self.0 & 4 == 0 || app_state & 2 == 0)
            && (self.0 & 8 == 0 || app_state & 4 == 0)
            && (self.0 & 16 == 0 || app_state & 8 == 0)
    }
}
pub type AppState =
    Arc<dyn Fn(&[String], Constraints, bool) -> Result<u8, Exception> + Send + Sync>;
pub type IdleJob = Arc<dyn Fn() -> Result<(), Exception> + Send + Sync>;
pub type Dependencies = Arc<dyn Fn(&[String]) -> Result<Vec<String>, Exception> + Send + Sync>;
pub type Callback = Box<dyn FnOnce(Result<bool, Exception>) + Send>;
struct Pending {
    names: Vec<String>,
    constraints: Constraints,
    finish: Instant,
    idle_probe: Option<Instant>,
    timeout: Duration,
    started: bool,
    evaluated: bool,
    callback: Callback,
}
#[derive(Default)]
struct State {
    pending: Vec<Pending>,
    stop: bool,
    wake: bool,
    idle: bool,
    packages: Vec<String>,
    errors: Vec<String>,
}
pub struct Owner {
    state: Mutex<State>,
    changed: Condvar,
    app_state: AppState,
    idle_job: IdleJob,
    dependencies: Dependencies,
}
pub struct Worker {
    owner: Arc<Owner>,
    thread: Option<JoinHandle<()>>,
}
impl Owner {
    pub fn start(
        app_state: AppState,
        idle_job: IdleJob,
        dependencies: Dependencies,
    ) -> (Arc<Self>, Worker) {
        let owner = Arc::new(Self {
            state: Mutex::new(State::default()),
            changed: Condvar::new(),
            app_state,
            idle_job,
            dependencies,
        });
        let cloned = owner.clone();
        let worker = thread::spawn(move || cloned.run());
        (
            owner.clone(),
            Worker {
                owner,
                thread: Some(worker),
            },
        )
    }
    /// Caller/name/install-source access checks are performed by NativeOwners
    /// before enqueue; callback capabilities are retained until completion.
    pub fn submit(
        &self,
        names: Vec<String>,
        constraints: Constraints,
        timeout_millis: i64,
        callback: Callback,
    ) -> Result<(), Exception> {
        if !(0..=MAX_TIMEOUT_MILLIS).contains(&timeout_millis) {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT,
                format!("Invalid timeoutMillis={timeout_millis}"),
            ));
        }
        let now = Instant::now();
        let mut state = self.state.lock().unwrap();
        if state.stop {
            return Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "constraint owner stopped",
            ));
        }
        state.pending.push(Pending {
            names,
            constraints,
            finish: now + Duration::from_millis(timeout_millis as u64),
            idle_probe: None,
            timeout: Duration::from_millis(timeout_millis as u64),
            started: false,
            evaluated: false,
            callback,
        });
        state.wake = true;
        self.changed.notify_one();
        Ok(())
    }
    pub fn notify_app_state(&self) {
        let mut state = self.state.lock().unwrap();
        state.wake = true;
        self.changed.notify_one();
    }
    pub fn notify_package_state(&self, package: String) {
        let mut state = self.state.lock().unwrap();
        state.packages.push(package);
        state.wake = true;
        self.changed.notify_one();
    }
    /// Called by the actual requires-device-idle JobScheduler job, not Doze.
    pub fn notify_idle(&self) {
        let mut state = self.state.lock().unwrap();
        state.idle = true;
        state.wake = true;
        self.changed.notify_one();
    }
    pub fn stop(&self) {
        let pending = {
            let mut state = self.state.lock().unwrap();
            state.stop = true;
            std::mem::take(&mut state.pending)
        };
        self.changed.notify_one();
        drop(pending);
    }
    pub fn errors(&self) -> Vec<String> {
        self.state.lock().unwrap().errors.clone()
    }
    fn run(&self) {
        loop {
            let (mut pending, idle, changed_packages) = {
                let mut state = self.state.lock().unwrap();
                while !state.stop && !state.wake {
                    let deadline = state
                        .pending
                        .iter()
                        .map(|p| p.idle_probe.unwrap_or(p.finish))
                        .min();
                    state = match deadline {
                        Some(at) => {
                            self.changed
                                .wait_timeout(state, at.saturating_duration_since(Instant::now()))
                                .unwrap()
                                .0
                        }
                        None => self.changed.wait(state).unwrap(),
                    };
                    if state
                        .pending
                        .iter()
                        .any(|p| p.idle_probe.unwrap_or(p.finish) <= Instant::now())
                    {
                        state.wake = true;
                    }
                }
                if state.stop {
                    return;
                }
                state.wake = false;
                let idle = std::mem::take(&mut state.idle);
                (
                    std::mem::take(&mut state.pending),
                    idle,
                    std::mem::take(&mut state.packages),
                )
            };
            for check in &mut pending {
                if !check.started {
                    let now = Instant::now();
                    check.started = true;
                    check.finish = now + check.timeout;
                    check.idle_probe =
                        (check.constraints.0 & 1 != 0).then_some(now + Duration::from_secs(10));
                }
            }
            let mut retained = Vec::new();
            if pending.iter().any(|p| p.idle_probe.is_some())
                && let Err(error) = (self.idle_job)()
            {
                self.state
                    .lock()
                    .unwrap()
                    .errors
                    .push(format!("constraint idle scheduling failed: {error:?}"));
            }
            for mut pending in pending {
                let now = Instant::now();
                if !idle && pending.idle_probe.is_some_and(|at| now < at) {
                    retained.push(pending);
                    continue;
                }
                pending.idle_probe = None;
                if !idle
                    && pending.evaluated
                    && now < pending.finish
                    && !changed_packages.is_empty()
                {
                    match (self.dependencies)(&pending.names) {
                        Ok(dependencies)
                            if !changed_packages
                                .iter()
                                .any(|package| dependencies.contains(package)) =>
                        {
                            retained.push(pending);
                            continue;
                        }
                        Ok(_) => {}
                        Err(failure) => {
                            self.state
                                .lock()
                                .unwrap()
                                .errors
                                .push(format!("constraint dependency query failed: {failure:?}"));
                            (pending.callback)(Err(failure));
                            continue;
                        }
                    }
                }
                pending.evaluated = true;
                match (self.app_state)(&pending.names, pending.constraints, idle) {
                    Ok(app_state) => {
                        let satisfied = pending.constraints.satisfied(app_state, idle);
                        if satisfied || now >= pending.finish {
                            (pending.callback)(Ok(satisfied));
                        } else {
                            retained.push(pending);
                        }
                    }
                    Err(failure) => {
                        self.state
                            .lock()
                            .unwrap()
                            .errors
                            .push(format!("constraint state query failed: {failure:?}"));
                        (pending.callback)(Err(failure));
                    }
                }
            }
            let needs_idle = retained.iter().any(|p| p.constraints.0 & 1 != 0);
            let mut state = self.state.lock().unwrap();
            if state.stop {
                drop(state);
                drop(retained);
                return;
            }
            state.pending.extend(retained);
            drop(state);
            if needs_idle && let Err(error) = (self.idle_job)() {
                self.state
                    .lock()
                    .unwrap()
                    .errors
                    .push(format!("constraint idle scheduling failed: {error:?}"));
            }
        }
    }
}
impl Worker {
    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().is_none_or(JoinHandle::is_finished)
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.owner.stop();
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

/// PackageInstallerService.checkInstallConstraintsInternal access policy.
/// INSTALL_PACKAGES alone does not grant arbitrary package access here.
pub fn authorize(
    query: &crate::package::query::Query<'_>,
    installer: Option<&str>,
    names: &[String],
) -> Result<(), Exception> {
    use aim_service_aidl::android_content_pm_ipackagemanager as pm;
    let mut request = aim_binder_host::parcel::Parcel::new();
    pm::GetNameForUid {
        uid: query.calling_uid,
    }
    .write(&mut request);
    let reply = query
        .answer(
            pm::DESCRIPTOR,
            pm::GET_NAME_FOR_UID,
            &mut aim_binder_host::parcel::Reader::new(request.data(), request.objects()),
        )
        .map_err(|reason| {
            Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                format!("installer UID name owner: {reason:?}"),
            )
        })?;
    let calling_name = pm::read_get_name_for_uid_reply(&mut aim_binder_host::parcel::Reader::new(
        reply.data(),
        reply.objects(),
    ))
    .map_err(|code| {
        Exception::new(
            aim_binder_host::parcel::EX_ILLEGAL_STATE,
            format!("installer UID name parcel: {code}"),
        )
    })??;
    if calling_name.as_deref() != installer {
        return Err(Exception::security(
            "The installerPackageName set by the caller doesn't match the caller's own package name.",
        ));
    }
    if crate::package::apps_filter::is_system_or_root_or_shell(query.calling_uid) {
        return Ok(());
    }
    for name in names {
        let resolved = query.resolve_internal_package_name(name, -1);
        let Some(package) = query.state.packages.get(&resolved) else {
            return Err(Exception::security(format!(
                "Caller has no access to package {name}"
            )));
        };
        let self_update = query
            .uid_has_permission(query.calling_uid, "android.permission.INSTALL_SELF_UPDATES")
            .map_err(|reason| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    format!("self-update permission owner: {reason:?}"),
                )
            })?
            && Some(name.as_str()) == installer;
        if package.install_source.installer.as_deref() != installer
            && package.install_source.update_owner.as_deref() != installer
            && !self_update
        {
            return Err(Exception::security(format!(
                "Caller has no access to package {name}"
            )));
        }
    }
    Ok(())
}

struct EventBundle {
    package: Option<String>,
}
impl ReadParcelable for EventBundle {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let size = r.read_i32()?;
        if size == 0 {
            return Ok(Self { package: None });
        }
        if size < 0 || r.read_i32()? != 0x4c444e42 {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        }
        let end = r
            .position()
            .checked_add(size as usize)
            .ok_or(aim_binder_host::parcel::BAD_VALUE)?;
        if r.read_i32()? != 1 || r.read_string16()?.as_deref() != Some("package") {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        }
        let package = match r.read_i32()? {
            -1 => None,
            0 => r.read_string16()?,
            _ => return Err(aim_binder_host::parcel::BAD_VALUE),
        };
        if r.position() != end {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        }
        Ok(Self { package })
    }
}
struct Events {
    owner: std::sync::Weak<Owner>,
    idle: bool,
}
impl aim_binder_host::local::Service for Events {
    fn descriptor(&self) -> &str {
        aim_service_aidl::android_os_iremotecallback::DESCRIPTOR
    }
    fn transact(
        &self,
        call: &mut aim_binder_host::local::Call<'_>,
    ) -> aim_binder_host::local::Reply {
        use aim_service_aidl::android_os_iremotecallback as aidl;
        if call.code != aidl::SEND_RESULT {
            return Err(aim_binder_host::parcel::UNKNOWN_TRANSACTION);
        }
        if call.sender_euid != 1000 && call.sender_euid != 0 {
            return Err(aim_binder_host::parcel::PERMISSION_DENIED);
        }
        let event = aidl::SendResult::<EventBundle>::read(&mut call.data)?;
        if let Some(owner) = self.owner.upgrade() {
            if self.idle {
                owner.notify_idle();
            } else if let Some(package) = event.data.and_then(|data| data.package) {
                owner.notify_package_state(package);
            }
        }
        Ok(aim_binder_host::parcel::Parcel::new())
    }
}
/// Publish real event capabilities and retain their original-service listeners.
/// Call after the bootstrap attachment reply, outside native/SystemServer locks.
pub fn configure(
    bridge: Arc<super::preapproval::BridgeOwner>,
    publisher: &super::native::Publisher,
) -> Result<(Arc<Owner>, Worker), Exception> {
    let callback = Arc::new(Mutex::new(None));
    let idle_callback = callback.clone();
    let idle_bridge = bridge.clone();
    let app_bridge = bridge.clone();
    let dependencies = bridge.clone();
    let (owner, worker) = Owner::start(
        Arc::new(move |names, flags, idle| app_bridge.app_state(names, flags, idle)),
        Arc::new(move || {
            let callback = idle_callback.lock().unwrap().ok_or_else(|| {
                Exception::new(
                    aim_binder_host::parcel::EX_ILLEGAL_STATE,
                    "constraint idle event capability unavailable",
                )
            })?;
            idle_bridge.request_idle_job(callback)
        }),
        Arc::new(move |names| dependencies.dependencies(names)),
    );
    let idle = publisher(Arc::new(Events {
        owner: Arc::downgrade(&owner),
        idle: true,
    }))?;
    let changes = publisher(Arc::new(Events {
        owner: Arc::downgrade(&owner),
        idle: false,
    }))?;
    *callback.lock().unwrap() = Some(idle);
    bridge.watch_app_state(changes)?;
    Ok((owner, worker))
}
