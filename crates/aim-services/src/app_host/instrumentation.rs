//! AMS / ActiveInstrumentation / InstrumentationReporter, android-16.0.0_r1.
use super::*;
use crate::package::intent::ComponentName;
use aim_binder_host::parcel::EX_NULL_POINTER;
use aim_service_aidl::android_app_iinstrumentationwatcher as watcher;
use aim_service_aidl::android_app_iuiautomationconnection as ui;
use kept::{KeptBundle, RawBundle};
use std::sync::mpsc::{self, Sender};
type Result<T> = std::result::Result<T, Exception>;
type Request = am::StartInstrumentation<ComponentName, RawBundle>;
#[derive(Default)]
pub(super) struct State {
    admission: Mutex<()>,
    active: Mutex<HashMap<i32, Arc<Session>>>,
    reporter: OnceLock<Sender<Report>>,
    delegate: Mutex<Option<Arc<Delegate>>>,
}
/// AccessCheckDelegateImpl, android-16.0.0_r1 (AOSP, Apache-2.0).
pub(super) struct Delegate {
    pub uid: i32,
    pub package: String,
    pub permissions: Option<Vec<Option<String>>>,
}
impl Delegate {
    pub fn permission(&self, name: &str) -> bool {
        self.permissions
            .as_ref()
            .is_none_or(|p| p.iter().any(|p| p.as_deref() == Some(name)))
    }
}
pub(super) struct Session {
    pub class: ComponentName,
    pub target: package::info::ApplicationInfo,
    processes: Vec<String>,
    pub arguments: Option<KeptBundle>,
    pub watcher: Option<Arc<Strong>>,
    pub connection: Option<Arc<Strong>>,
    pub profile: Option<Profile>,
    abi: Option<String>,
    hidden: bool,
    test_api: bool,
    no_restart: bool,
    storage: bool,
    source_uid: i32,
    background: [bool; 2],
    results: Mutex<(bool, Option<KeptBundle>)>,
}

impl Session {
    pub fn live(&self) -> bool {
        !self.results.lock().unwrap().0
    }
    fn matches(&self, name: &str, process: &str) -> bool {
        if self.processes.is_empty() {
            self.target.item.package_name.as_deref() == Some(name)
        } else {
            self.processes.iter().any(|p| p == process)
        }
    }
    pub fn configure(&self, start: &mut zygote::Start) {
        if self.hidden {
            start.runtime_flags &= !(3 << API_ENFORCEMENT_POLICY_SHIFT);
        }
        if self.test_api {
            start.runtime_flags |= 1 << 18; // Zygote.DISABLE_TEST_API_ENFORCEMENT_POLICY.
        }
        if let Some(abi) = &self.abi {
            start.instruction_set = Some(instruction_set(abi));
        }
    }
}

/// AppProfiler's instrumentation profile: ActivityThread opens the file.
pub(super) struct Profile(String);
impl WriteParcelable for Profile {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(Some(&self.0));
        p.write_i32(0); // no profile fd
        p.write_i32(0); // sampling interval
        p.write_bool(false); // auto stop
        p.write_bool(false); // streaming
        p.write_string16(None); // agent
        p.write_bool(false); // attach agent during bind
        p.write_i32(0); // clock type
        p.write_i32(1); // ProfilerInfo.OUTPUT_VERSION_DEFAULT
    }
}
struct Report {
    watcher: Option<Arc<Strong>>,
    class: ComponentName,
    code: i32,
    results: Option<KeptBundle>,
    finished: bool,
    shutdown: Option<Arc<Strong>>,
}
impl State {
    pub fn delegate(&self) -> Option<Arc<Delegate>> {
        self.delegate.lock().unwrap().clone()
    }
    fn report(&self, report: Report) {
        let sender = self.reporter.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Report>();
            std::thread::spawn(move || {
                for report in rx {
                    if let Some(w) = report.watcher {
                        let mut p = Parcel::new();
                        // The pinned watcher methods share their argument layout.
                        watcher::InstrumentationStatus {
                            name: Some(report.class),
                            result_code: report.code,
                            results: report.results.as_ref().map(|r| kept::By(&r.bundle)),
                        }
                        .write(&mut p);
                        let code = if report.finished {
                            watcher::INSTRUMENTATION_FINISHED
                        } else {
                            watcher::INSTRUMENTATION_STATUS
                        };
                        let result = w
                            .transact(code, &p, false)
                            .and_then(|p| p.reader().read_exception());
                        if !matches!(result, Ok(Ok(()))) {
                            eprintln!("app host: instrumentation watcher: {result:?}");
                        }
                    }
                    if let Some(connection) = report.shutdown {
                        let mut p = Parcel::new();
                        ui::Shutdown {}.write(&mut p);
                        if let Err(e) = connection.transact(ui::SHUTDOWN, &p, true) {
                            eprintln!("app host: instrumentation shutdown: {e}");
                        }
                    }
                }
            });
            tx
        });
        sender.send(report).expect("reporter stopped");
    }
    pub fn matching(&self, name: &str, process: &str, uid: u32) -> Option<Arc<Session>> {
        self.active
            .lock()
            .unwrap()
            .get(&(uid as i32))
            .filter(|s| s.live() && s.matches(name, process))
            .cloned()
    }
}
pub(super) fn transact(host: &AppHost, call: &mut Call<'_>) -> Reply {
    let mut p = Parcel::new();
    let result = match call.code {
        am::START_INSTRUMENTATION => {
            let a = am::StartInstrumentation::<ComponentName, RawBundle>::read(&mut call.data)?;
            host.start_instrumentation(call, a)
                .map(|v| am::write_start_instrumentation_reply(&mut p, v))
        }
        am::ADD_INSTRUMENTATION_RESULTS => {
            let a = am::AddInstrumentationResults::<RawBundle>::read(&mut call.data)?;
            host.instrumentation_results(a.target, None, a.results)
                .map(|()| am::write_add_instrumentation_results_reply(&mut p))
        }
        am::FINISH_INSTRUMENTATION => {
            let a = am::FinishInstrumentation::<RawBundle>::read(&mut call.data)?;
            host.instrumentation_results(a.target, Some(a.result_code), a.results)
                .map(|()| am::write_finish_instrumentation_reply(&mut p))
        }
        am::START_DELEGATE_SHELL_PERMISSION_IDENTITY => {
            let a = am::StartDelegateShellPermissionIdentity::read(&mut call.data)?;
            host.delegate_shell(call.sender_euid as i32, a.uid, a.permissions)
                .map(|()| am::write_start_delegate_shell_permission_identity_reply(&mut p))
        }
        am::STOP_DELEGATE_SHELL_PERMISSION_IDENTITY => {
            am::StopDelegateShellPermissionIdentity::read(&mut call.data)?;
            shell_caller(call.sender_euid as i32).and_then(|()| {
                let mut delegate = host.instrumentation.delegate.lock().unwrap();
                *delegate = None;
                host.notify_permission_cache_changed()?;
                am::write_stop_delegate_shell_permission_identity_reply(&mut p);
                Ok(())
            })
        }
        am::GET_DELEGATED_SHELL_PERMISSIONS => {
            am::GetDelegatedShellPermissions::read(&mut call.data)?;
            shell_caller(call.sender_euid as i32).map(|()| {
                let delegate = host.instrumentation.delegate();
                let names = delegate
                    .as_ref()
                    .map_or(Some(Vec::new()), |d| d.permissions.clone());
                am::write_get_delegated_shell_permissions_reply(&mut p, &names);
            })
        }
        _ => unreachable!(),
    };
    reply(result.map(|()| p))
}
fn stop_starts(apps: &mut HashMap<i64, App>, session: &Session) -> Vec<(i64, i32)> {
    let package = session.target.item.package_name.as_ref().unwrap();
    apps.iter_mut()
        .filter(|(_, a)| {
            a.uid == session.target.uid as u32
                && (a.instrumentation.is_some()
                    || a.packages.contains(package)
                    || a.package_deps.contains(package))
        })
        .map(|(seq, a)| {
            a.removed = true;
            (*seq, a.pid)
        })
        .collect()
}
fn state_error(error: impl std::fmt::Display) -> Exception {
    Exception::new(EX_ILLEGAL_STATE, format!("instrumentation: {error}"))
}
fn check_bundle(bundle: &Option<RawBundle>, kind: &str) -> Result<()> {
    if bundle.as_ref().is_some_and(RawBundle::has_fds) {
        return Err(Exception::illegal_argument(format!(
            "File descriptors passed in {kind}"
        )));
    }
    Ok(())
}
fn strings(values: &[(&str, &str)]) -> RawBundle {
    let mut p = Parcel::new();
    let values: Vec<_> = values
        .iter()
        .map(|(k, v)| (*k, crate::bundle::Value::String(Some((*v).into()))))
        .collect();
    crate::bundle::write(&mut p, &values);
    RawBundle::read_from(&mut p.reader()).unwrap()
}
fn shell_caller(uid: i32) -> Result<()> {
    if matches!(uid % PER_USER_RANGE as i32, 0 | 2000) {
        Ok(())
    } else {
        Err(Exception::security(
            "Only the shell can delegate its permissions",
        ))
    }
}
impl AppHost {
    /// AccessCheckDelegateImpl invalidates PackageManager's permission-cache
    /// nonce on adoption/removal. Apps' native shared memory has no such nonce,
    /// but aim-system owns a mutable store, so invalidate through its owner.
    fn notify_permission_cache_changed(&self) -> Result<()> {
        use aim_service_aidl::dev_aim_server_ipackagebootstrapbridge as bridge;
        let owner = self.package_bootstrap_java.lock().unwrap().clone();
        // Without aim-system there is no mutable store or cached answer to retire.
        let Some(owner) = owner else { return Ok(()) };
        let mut p = Parcel::new();
        bridge::InvalidatePackageInfoCache {}.write(&mut p);
        let p = owner
            .transact(bridge::INVALIDATE_PACKAGE_INFO_CACHE, &p, false)
            .map_err(state_error)?;
        bridge::read_invalidate_package_info_cache_reply(&mut p.reader()).map_err(state_error)?
    }

    fn delegate_shell(
        &self,
        caller: i32,
        uid: i32,
        permissions: Option<Vec<Option<String>>>,
    ) -> Result<()> {
        shell_caller(caller)?;
        let active = self.instrumentation.active.lock().unwrap();
        let mut delegate = self.instrumentation.delegate.lock().unwrap();
        if delegate.as_ref().is_some_and(|d| d.uid != uid) {
            return Err(Exception::security(
                "Shell can delegate permissions only to one instrumentation at a time",
            ));
        }
        let Some(session) = active.get(&uid).filter(|s| s.live()) else {
            return Ok(()); // AccessCheckDelegateHelper: no matching active instrumentation.
        };
        if session.connection.is_none() {
            return Err(Exception::security(
                "Shell can delegate its permissions only to an instrumentation started from the shell",
            ));
        }
        *delegate = Some(Arc::new(Delegate {
            uid,
            package: session.target.item.package_name.clone().unwrap(),
            permissions,
        }));
        self.notify_permission_cache_changed()
    }
    fn start_instrumentation(&self, call: &Call<'_>, a: Request) -> Result<bool> {
        let instrumentation = &self.instrumentation;
        let active = &instrumentation.active;
        let uid = call.sender_euid as i32;
        let granted = |p| self.check_permission(Some(p), uid).map(|r| r == 0);
        if is_isolated(uid) {
            return Err(Exception::security(
                "Isolated process not allowed to call startInstrumentation",
            ));
        }
        let user = self.handle_incoming_user(
            uid,
            a.user_id,
            false,
            activity_user::AllowMode::FullOnly,
            Some("startInstrumentation"),
            None,
        )?;
        check_bundle(&a.arguments, "Bundle")?;
        if a.flags & ((1 << 5) | (1 << 6)) != 0 || user != 0 {
            return Err(self.unimplemented_named(
                "activity",
                "SDK sandbox or nonresident-user instrumentation (#2511)",
            ));
        }
        let null_class = || Exception::new(EX_NULL_POINTER, "null instrumentation component");
        let class = a.class_name.ok_or_else(null_class)?;
        let keep = |b: Option<Binder>| {
            b.map(|b| match b {
                Binder::Handle(h) => Ok(Arc::new(self.process.strong(h))),
                _ => Err(Exception::illegal_argument("Expected a remote Binder")),
            })
            .transpose()
        };
        let watch = keep(a.watcher)?;
        let failure = |message: String| -> Result<bool> {
            eprintln!("app host: {message}");
            let results = strings(&[("id", "ActivityManagerService"), ("Error", &message)]);
            let results = KeptBundle::new(results, &self.process).map_err(state_error)?;
            instrumentation.report(Report {
                watcher: watch.clone(),
                class: class.clone(),
                code: -1,
                results: Some(results),
                finished: false,
                shutdown: None,
            });
            Ok(false)
        };
        let _admission = instrumentation.admission.lock().unwrap();
        let (state, resolution) = self.packages()?;
        let query = crate::package::query::Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: SYSTEM_UID as i32,
        };
        let query_error =
            |e: package::apps_filter::NotModelled| Exception::new(EX_ILLEGAL_STATE, e.0);
        let Some(ii) = query
            .instrumentation_info(Some(&class), STOCK_PM_FLAGS, user)
            .map_err(query_error)??
        else {
            return failure(format!(
                "Unable to find instrumentation info for: {}/{}",
                class.package, class.class
            ));
        };
        let target = ii.0.target_package.as_deref().unwrap_or_default();
        let Some(ai) = self.application_info(target, STOCK_PM_FLAGS, Some(&state))? else {
            return failure(format!(
                "Unable to find instrumentation target package: {target}"
            ));
        };
        let no_restart = a.flags & (1 << 3) != 0;
        if target == SYSTEM_PACKAGE && !no_restart {
            return failure("Cannot instrument system server without 'no-restart'".into());
        }
        if target != SYSTEM_PACKAGE && ai.flags & 4 == 0 {
            return failure(format!("Instrumentation target has no code: {target}"));
        }
        let signed = query
            .check_signatures(target, &class.package, user)
            .map_err(query_error)??;
        let debug = self.property("ro.debuggable").as_deref() == Some("1");
        let denied = if signed < 0
            && signed != -1
            && !(debug && uid == 0 && a.flags & (1 << 4) == 0)
        {
            Some(format!(
                "Permission Denial: starting instrumentation {}/{} from pid={}, uid={uid} not allowed because package {} does not have a signature matching the target {target}",
                class.package, class.class, call.sender_pid, class.package
            ))
        } else if !debug
            && !matches!(uid, 0 | 1000 | 2000)
            && !self.apps.lock().unwrap().values().any(|p| {
                p.pid == call.sender_pid
                    && p.uid == call.sender_euid
                    && p.instrumentation.as_ref().is_some_and(|s| s.live())
            })
        {
            Some(format!(
                "Permission Denial: instrumentation test {}/{} from pid={}, uid={uid} not allowed because it's not started from SHELL",
                class.package, class.class, call.sender_pid
            ))
        } else {
            None
        };
        if let Some(message) = denied {
            failure(message.clone())?;
            return Err(Exception::security(message));
        }
        let hidden = ai.private_flags & (1 << 22) != 0 || a.flags & 1 != 0;
        let test_api = hidden || a.flags & (1 << 2) != 0;
        if test_api && !granted("android.permission.DISABLE_HIDDEN_API_CHECKS")? {
            return Err(Exception::security(
                "disable hidden API checks requires android.permission.DISABLE_HIDDEN_API_CHECKS",
            ));
        }
        let default = ai.process_name.clone().unwrap_or_else(|| target.into());
        let processes = match ii.0.target_processes.as_deref() {
            None => vec![default.clone()],
            Some("*") => Vec::new(),
            Some(list) => list.split(',').map(String::from).collect(),
        };
        let process = if no_restart {
            default
        } else {
            processes.first().cloned().unwrap_or(default)
        };
        let session = Arc::new(Session {
            class,
            target: ai,
            processes,
            arguments: a
                .arguments
                .map(|b| KeptBundle::new(b, &self.process))
                .transpose()
                .map_err(state_error)?,
            watcher: watch,
            connection: keep(a.connection)?,
            profile: a.profile_file.map(Profile),
            abi: a.abi_override,
            hidden,
            test_api,
            no_restart,
            storage: a.flags & 2 != 0,
            source_uid: uid,
            background: [
                granted("android.permission.START_ACTIVITIES_FROM_BACKGROUND")?,
                granted("android.permission.START_FOREGROUND_SERVICES_FROM_BACKGROUND")?,
            ],
            results: Mutex::new((false, None)),
        });
        let starts = {
            let mut apps = self.apps.lock().unwrap();
            let mut active = active.lock().unwrap();
            if active.contains_key(&session.target.uid) {
                return Err(state_error("instrumentation already active for target UID"));
            }
            active.insert(session.target.uid, session.clone());
            if no_restart {
                Vec::new()
            } else {
                stop_starts(&mut apps, &session)
            }
        };
        let started = (|| {
            self.stop_instrumentation_starts(starts)?;
            if session.storage {
                self.instrumentation_storage(&session, 0)?;
            }
            if no_restart {
                let strong = {
                    let mut apps = self.apps.lock().unwrap();
                    let app = apps
                        .values_mut()
                        .find(|p| {
                            !p.removed
                                && p.uid == session.target.uid as u32
                                && p.process == process
                                && p.thread.is_some()
                        })
                        .ok_or_else(|| state_error("no running instrumentation target"))?;
                    app.instrumentation = Some(session.clone());
                    self.process.strong(app.thread.as_ref().unwrap().handle)
                };
                let mut p = Parcel::new();
                thread::InstrumentWithoutRestart {
                    instrumentation_name: Some(kept::By(&session.class)),
                    instrumentation_args: session.arguments.as_ref().map(|b| kept::By(&b.bundle)),
                    instrumentation_watcher: session.watcher.as_ref().map(|s| s.binder()),
                    instrumentation_ui_connection: session.connection.as_ref().map(|s| s.binder()),
                    target_info: Some(kept::By(&session.target)),
                }
                .write(&mut p);
                strong
                    .transact(thread::INSTRUMENT_WITHOUT_RESTART, &p, true)
                    .map_err(state_error)?;
            } else {
                // AMS.addAppLocked unstops the target before starting instrumentation.
                self.package_system()?.internal_set_package_stopped_state(
                    Some(target.into()),
                    false,
                    user,
                    1000,
                    0,
                )?;
                self.start_package_process(target, &process, None, false, false)?;
            }
            Ok(())
        })();
        eprintln!(
            "app host: instrumentation from uid {}: {target}/{process}",
            session.source_uid
        );
        if let Err(error) = started {
            self.retire_instrumentation(&session, "failed to attach");
            return Err(error);
        }
        Ok(true)
    }
    fn instrumentation_storage(&self, session: &Session, mode: i32) -> Result<()> {
        let permissions = self
            .permission_manager
            .get()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no permissionmgr"))?;
        let ops = appops_service::AppOps::new(self.me.upgrade().unwrap(), permissions.clone());
        ops.set_mode(
            SYSTEM_UID as i32,
            99,
            session.target.uid,
            Some(Some(&session.class.package)),
            mode,
        )
    }
    fn stop_instrumentation_starts(&self, starts: Vec<(i64, i32)>) -> Result<()> {
        let mut error = None;
        for (seq, pid) in starts {
            if let Err(e) = self.kill_process(
                seq,
                exit_info::REASON_OTHER,
                exit_info::SUBREASON_UNKNOWN,
                "instrumentation",
            ) {
                error.get_or_insert(Exception::new(EX_ILLEGAL_STATE, e));
            }
            if pid > 0 {
                self.process_died(seq, pid);
            }
        }
        error.map_or(Ok(()), Err)
    }
    fn instrumentation_results(
        &self,
        target: Option<Binder>,
        code: Option<i32>,
        bundle: Option<RawBundle>,
    ) -> Result<()> {
        check_bundle(&bundle, "Intent")?;
        let session = self
            .apps
            .lock()
            .unwrap()
            .values()
            .find(|a| a.thread.as_ref().map(|s| s.binder()) == target && target.is_some())
            .and_then(|a| a.instrumentation.clone());
        let Some(session) = session else {
            return Ok(());
        };
        if let Some(code) = code {
            return self.complete_instrumentation(&session, code, bundle);
        }
        let mut results = session.results.lock().unwrap();
        if !results.0 {
            self.merge_results(&mut results.1, bundle)?;
        }
        Ok(())
    }
    fn merge_results(&self, old: &mut Option<KeptBundle>, new: Option<RawBundle>) -> Result<()> {
        if let Some(bundle) = new {
            let merged = match old.as_ref() {
                Some(old) => old.bundle.put_all(&bundle).map_err(state_error)?,
                None => bundle,
            };
            *old = Some(KeptBundle::new(merged, &self.process).map_err(state_error)?);
        }
        Ok(())
    }
    fn complete_instrumentation(
        &self,
        session: &Arc<Session>,
        code: i32,
        bundle: Option<RawBundle>,
    ) -> Result<()> {
        let mut apps = self.apps.lock().unwrap();
        let mut results = session.results.lock().unwrap();
        if results.0 {
            return Ok(());
        }
        self.merge_results(&mut results.1, bundle)?;
        results.0 = true;
        let final_results = results.1.take();
        drop(results);
        let starts = if session.no_restart {
            Vec::new()
        } else {
            stop_starts(&mut apps, session)
        };
        for app in apps.values_mut() {
            if app.uid == session.target.uid as u32 {
                app.instrumentation = None;
            }
        }
        drop(apps);
        let reset = if session.storage {
            self.instrumentation_storage(session, 2)
        } else {
            Ok(())
        };
        let stopped = self.stop_instrumentation_starts(starts);
        let active = &self.instrumentation.active;
        let mut active = active.lock().unwrap();
        let mut delegate = self.instrumentation.delegate.lock().unwrap();
        let invalidated = if delegate
            .as_ref()
            .is_some_and(|d| d.uid == session.target.uid)
        {
            *delegate = None;
            self.notify_permission_cache_changed()
        } else {
            Ok(())
        };
        active.remove(&session.target.uid);
        drop(delegate);
        drop(active);
        self.instrumentation.report(Report {
            watcher: session.watcher.clone(),
            class: session.class.clone(),
            code,
            results: final_results,
            finished: true,
            shutdown: session.connection.clone(),
        });
        reset.and(stopped).and(invalidated)
    }
    pub(super) fn retire_instrumentation(&self, session: &Arc<Session>, message: &str) {
        if let Err(e) =
            self.complete_instrumentation(session, 0, Some(strings(&[("shortMsg", message)])))
        {
            eprintln!("app host: instrumentation retirement: {}", e.message);
        }
    }
    pub(super) fn instrumentation_failed(&self, seq: i64, message: &str) {
        let session = self
            .apps
            .lock()
            .unwrap()
            .get(&seq)
            .and_then(|a| a.instrumentation.clone());
        if let Some(session) = session {
            self.retire_instrumentation(&session, message);
        }
    }
    pub(super) fn instrumentation_background(&self, uid: i32, foreground_service: bool) -> bool {
        self.apps.lock().unwrap().values().any(|a| {
            a.uid as i32 == uid
                && a.instrumentation
                    .as_ref()
                    .is_some_and(|s| s.live() && s.background[foreground_service as usize])
        })
    }
}
