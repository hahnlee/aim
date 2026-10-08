//! Native clearApplicationUserData lifecycle, AOSP android-16.0.0_r1.
use super::{apps_filter::{AppsFilter, Config}, model::State, query::Query, scan_snapshot::query_state::Capture, write::{Enabled, mutation::{Plan, Change}}};
use aim_binder_host::{local::{LocalProcess, LocalService, Strong}, parcel::{Binder, Exception, Parcel, Reader, EX_ILLEGAL_STATE, BAD_VALUE}};
use aim_service_aidl::{dev_aim_server_iapplicationdatabridge as api, android_content_pm_ipackagedataobserver as observer_api};
use std::{sync::{Arc, Mutex, mpsc}, thread::{self, JoinHandle}};
pub type Current = Arc<dyn Fn() -> Result<Arc<Capture>, Exception> + Send + Sync>;
pub type Commit = Arc<dyn Fn(Arc<Capture>, Vec<Plan>, i32) -> Result<(), Exception> + Send + Sync>;
enum Observer { Remote(Strong), Local(LocalService) }
impl Observer {
    fn send(&self, name: Option<String>, succeeded: bool) -> Result<(), Exception> {
        let mut data = Parcel::new(); observer_api::OnRemoveCompleted { package_name: name, succeeded }.write(&mut data);
        let result = match self { Self::Remote(owner) => owner.transact(observer_api::ON_REMOVE_COMPLETED, &data, true), Self::Local(owner) => owner.transact(observer_api::ON_REMOVE_COMPLETED, &data, true) };
        result.map_err(transport)?; Ok(())
    }
}
fn transport(code: i32) -> Exception { Exception::new(EX_ILLEGAL_STATE, format!("application data owner transport: {code}")) }
struct Task { package: Option<String>, user: i32, uid: i32, observer: Option<Observer>, execute: bool }
pub struct Owner {
    bridge: Strong, process: Arc<LocalProcess>, current: Current, commit: Commit,
    install_lock: Arc<Mutex<()>>, queue: mpsc::Sender<Option<Task>>, errors: Mutex<Vec<String>>,
}
pub struct Worker { owner: Arc<Owner>, thread: Option<JoinHandle<()>> }
impl Owner {
    pub fn stop(&self) { let _ = self.queue.send(None); }
    pub fn start(bridge: Strong, process: Arc<LocalProcess>, current: Current, commit: Commit,
            install_lock: Arc<Mutex<()>>) -> (Arc<Self>, Worker) {
        let (send, receive) = mpsc::channel();
        let owner = Arc::new(Self { bridge, process, current, commit, install_lock, queue: send, errors: Mutex::new(vec![]) });
        let run = owner.clone();
        let thread = thread::spawn(move || { while let Ok(Some(task)) = receive.recv() {
            let result = if task.execute { run.execute(task.package.as_deref(), task.user, task.uid) } else { Ok(false) };
            let succeeded = match result { Ok(value) => value, Err(error) => { run.errors.lock().unwrap().push(format!("clear user data: {error:?}")); false } };
            if let Some(observer) = task.observer { if let Err(error) = observer.send(task.package, succeeded) { run.errors.lock().unwrap().push(format!("clear user data observer: {error:?}")); } }
        }});
        (owner.clone(), Worker { owner, thread: Some(thread) })
    }
    fn call<T>(&self, code: u32, fill: impl FnOnce(&mut Parcel), decode: impl FnOnce(&mut Reader<'_>) -> Result<T, i32>) -> Result<T, Exception> {
        let mut data = Parcel::new(); data.write_interface_token(api::DESCRIPTOR); fill(&mut data);
        let reply = self.bridge.transact(code, &data, false).map_err(transport)?;
        let mut reader = reply.reader(); reader.read_exception().map_err(transport)??;
        let value = decode(&mut reader).map_err(transport)?;
        if reader.remaining() != 0 { return Err(transport(BAD_VALUE)); } Ok(value)
    }
    pub fn clear(&self, package: Option<String>, user: i32, pid: i32, uid: i32, callback: Option<Binder>, effects: &super::effects::Owner) -> Result<(), Exception> {
        self.call(api::ENFORCE_PERMISSION, |p| { p.write_string16(Some("android.permission.CLEAR_APP_USER_DATA")); p.write_i32(pid); p.write_i32(uid); }, |_| Ok(()))?;
        let capture = (self.current)()?; let state = capture.state();
        let filter = AppsFilter::new(state, &Config { force_system_packages_queryable: state.system.force_system_packages_queryable, force_queryable_packages: state.system.force_queryable_packages.clone() }).map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.message()))?;
        let query = Query { state, filter: &filter, calling_uid: uid };
        if user < 0 { return Err(Exception::illegal_argument(format!("Invalid userId {user}"))); }
        if user != super::apps_filter::user_id(uid) && uid != 0 && uid != 1000
            && !query.uid_has_permission(uid, "android.permission.INTERACT_ACROSS_USERS_FULL").map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.0))? {
            return Err(Exception::security("clear application data requires INTERACT_ACROSS_USERS_FULL"));
        }
        let setting = package.as_deref().map(|name| query.resolve_internal_package_name(name, -1)).and_then(|name| state.packages.get(&name));
        let execute = setting.is_some_and(|setting| super::info::user_state(setting, user).installed)
            && !query.filtered(setting, uid, user).map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.0))?;
        if execute && effects.data_protected(package.as_deref().unwrap(), user)? { return Err(Exception::security(format!("Cannot clear data for a protected package: {}", package.as_deref().unwrap()))); }
        let observer = callback.map(|binder| match binder {
            Binder::Handle(handle) => Ok(Observer::Remote(self.process.strong(handle))),
            Binder::Local(ptr) => self.process.local_service(ptr).map(Observer::Local).ok_or_else(|| transport(BAD_VALUE)),
        }).transpose()?;
        self.queue.send(Some(Task { package, user, uid, observer, execute })).map_err(|_| Exception::new(EX_ILLEGAL_STATE, "application data worker stopped"))
    }
    fn execute(&self, package: Option<&str>, user: i32, caller: i32) -> Result<bool, Exception> {
        let Some(name) = package else { return Ok(false); };
        let capture = (self.current)()?;
        let state = capture.state();
        let lifecycle = state.system.lifecycle.as_ref().ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "native freezer owner unavailable"))?;
        let freeze = lifecycle.freeze(name.to_owned())
            .map_err(|message| Exception::new(EX_ILLEGAL_STATE, message))?;
        if let Some(setting) = state.packages.get(name) {
            self.call(api::KILL_AND_WAIT, |p| { p.write_string16(Some(name)); p.write_i32(setting.app_id); }, |_| Ok(()))?;
        }
        let succeeded = {
            let _install = self.install_lock.lock().unwrap();
            self.call(api::CLEAR_DATA, |p| { p.write_string16(Some(name)); p.write_i32(user); }, |r| r.read_bool())?
        };
        let current = (self.current)()?;
        let registry = current.state().system.instant_registry.as_ref().ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "instant metadata owner unavailable"))?;
        registry.delete_metadata(user, name).map_err(|message| Exception::new(EX_ILLEGAL_STATE, message))?;
        if succeeded {
            let plans = reset_components(current.state(), name, user);
            if !plans.is_empty() { (self.commit)(current.clone(), plans, caller)?; }
        }
        freeze.close().map_err(|message| Exception::new(EX_ILLEGAL_STATE, message))?;
        if succeeded {
            self.call(api::CHECK_MEMORY, |_| {}, |_| Ok(()))?;
            let current = (self.current)()?;
            let filter = AppsFilter::new(current.state(), &Config { force_system_packages_queryable: current.state().system.force_system_packages_queryable, force_queryable_packages: current.state().system.force_queryable_packages.clone() }).map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.message()))?;
            let query = Query { state: current.state(), filter: &filter, calling_uid: 1000 };
            if let Some(setting) = current.state().packages.get(name) {
                if query.uid_has_permission(super::info::uid(user, setting.app_id), "android.permission.SUSPEND_APPS").map_err(|error| Exception::new(EX_ILLEGAL_STATE, error.0))? {
                    (self.commit)(current.clone(), remove_suspender(current.state(), name, user), caller)?;
                }
            }
        }
        Ok(succeeded)
    }
    pub fn delete_preloads(&self, pid: i32, uid: i32) -> Result<(), Exception> {
        self.call(api::ENFORCE_PERMISSION, |p| { p.write_string16(Some("android.permission.CLEAR_APP_CACHE")); p.write_i32(pid); p.write_i32(uid); }, |_| Ok(()))?;
        self.call(api::DELETE_PRELOADS, |_| {}, |_| Ok(()))
    }
    pub fn customization_ready(&self, pid: i32, uid: i32) -> Result<(), Exception> {
        self.call(api::ENFORCE_PERMISSION, |p| { p.write_string16(Some("android.permission.SEND_DEVICE_CUSTOMIZATION_READY")); p.write_i32(pid); p.write_i32(uid); }, |_| Ok(()))?;
        self.call(api::SEND_DEVICE_CUSTOMIZATION_READY, |_| {}, |_| Ok(()))
    }
    pub fn errors(&self) -> Vec<String> { self.errors.lock().unwrap().clone() }
}
fn reset_components(state: &State, name: &str, user: i32) -> Vec<Plan> {
    let Some(setting) = state.packages.get(name) else { return vec![]; };
    let Some(code) = setting.pkg.as_deref().filter(|code| code.booleans & super::pkg::booleans::RESET_ENABLED_SETTINGS_ON_APP_DATA_CLEARED != 0) else { return vec![]; };
    let user_state = super::info::user_state(setting, user);
    let mut enabled = Enabled { enabled: user_state.enabled, last_disable_app_caller: user_state.last_disable_app_caller, enabled_components: user_state.enabled_components.into_iter().collect(), disabled_components: user_state.disabled_components.into_iter().collect() };
    let mut changed = false;
    for class in code.activities.iter().map(|c| &c.main.component.name)
        .chain(code.receivers.iter().map(|c| &c.main.component.name))
        .chain(code.services.iter().map(|c| &c.main.component.name))
        .chain(code.providers.iter().map(|c| &c.main.component.name)) {
        changed |= enabled.enabled_components.remove(class) | enabled.disabled_components.remove(class);
    }
    if changed { vec![Plan { package: name.into(), user: Some(user), change: Change::Enabled(enabled) }] } else { vec![] }
}
fn remove_suspender(state: &State, name: &str, suspender_user: i32) -> Vec<Plan> {
    let mut plans = vec![];
    for (user, _) in &state.users {
        for setting in state.packages.values() {
            let stored = super::info::user_state(setting, *user);
            if let Some(suspensions) = &stored.suspensions {
                let filtered = suspensions.iter().filter(|entry| !(entry.package == name && entry.user == super::restrictions::SuspendingUser::Resolved(suspender_user))).cloned().collect::<Vec<_>>();
                if filtered.len() != suspensions.len() { plans.push(Plan { package: setting.name.clone(), user: Some(*user), change: Change::Suspensions((!filtered.is_empty()).then_some(filtered)) }); }
            }
            if *user == suspender_user && stored.distraction_flags != 0 { plans.push(Plan { package: setting.name.clone(), user: Some(*user), change: Change::Distraction(0) }); }
        }
    }
    plans
}
impl Drop for Worker { fn drop(&mut self) { let _ = self.owner.queue.send(None); if let Some(thread) = self.thread.take() { thread.join().unwrap(); } } }
