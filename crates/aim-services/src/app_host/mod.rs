//! The app host (docs/app-host.md): ActivityManager's side of an app's
//! process, native, for the app host image, which has no system_server.
//! It starts an app's process through zygote, as `ProcessList` does,
//! takes the process's `attachApplication` and binds it, as
//! `attachApplicationLocked` does.
//!
//! The process's start arguments are `ProcessList.startProcessLocked`'s
//! at `android-16.0.0_r1`, from the state PackageManager last persisted
//! (`packages.xml`, `packages.list`).

pub mod activity;
pub mod bind;
pub mod compat;
pub mod config;
pub mod display;
pub mod display_service;
pub mod package_service;
pub mod packages;
pub mod settings;
pub mod settings_provider;
pub mod zygote;

use std::collections::HashMap;
use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception,
    Parcel, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::ParcelValue;
use aim_service_aidl::android_app_iactivitymanager as am;
use aim_service_aidl::android_app_iapplicationthread as thread;

use crate::package;

/// The control service's binder name and descriptor.
pub const CONTROL: &str = "aim.app_host";
const CONTROL_DESCRIPTOR: &str = "dev.aim.IAppHost";
/// `int startActivity(String component)`: starts the activity of
/// `package/class` (`package/.Class` for one of the package) in a new
/// process, as `am start -n` does; the process's pid.
const START_ACTIVITY: u32 = 1;

/// `Process.FIRST_APPLICATION_UID`, `UserHandle.PER_USER_RANGE`.
const FIRST_APPLICATION_UID: u32 = 10000;
const PER_USER_RANGE: u32 = 100000;
/// `Process.SHARED_USER_GID`, `FIRST_SHARED_APPLICATION_GID`,
/// `FIRST_APPLICATION_CACHE_GID`.
const SHARED_USER_GID: u32 = 9997;
const FIRST_SHARED_APPLICATION_GID: u32 = 50000;
const FIRST_APPLICATION_CACHE_GID: u32 = 20000;
/// `ApplicationInfo.HIDDEN_API_ENFORCEMENT_*`, `Zygote.API_ENFORCEMENT_POLICY_SHIFT`.
const HIDDEN_API_ENFORCEMENT_DISABLED: i32 = 0;
const HIDDEN_API_ENFORCEMENT_ENABLED: i32 = 2;
const API_ENFORCEMENT_POLICY_SHIFT: i32 = 12;
/// `Zygote.GWP_ASAN_LEVEL_*`; `ApplicationInfo.FLAG_SYSTEM`.
const GWP_ASAN_LEVEL_LOTTERY: i32 = 1 << 21;
const GWP_ASAN_LEVEL_ALWAYS: i32 = 2 << 21;
const GWP_ASAN_LEVEL_DEFAULT: i32 = 3 << 21;
const NATIVE_HEAP_ZERO_INIT_ENABLED: i32 = 1 << 23;
const FLAG_SYSTEM: i32 = 1;
/// The compat changes `Zygote.getMemorySafetyRuntimeFlags` reads.
const GWP_ASAN: i64 = 135634846;
const NATIVE_HEAP_ZERO_INIT: i64 = 178038272;
/// `SELinuxUtil.COMPLETE_STR`, user 0's `seInfoUser`.
const SEINFO_COMPLETE: &str = ":complete";

/// Opens a new connection to the zygote relay's socket.
pub type Connect = Box<dyn Fn() -> io::Result<UnixStream> + Send + Sync>;

/// `ActivityManagerService.getCommonServicesLocked`'s services, in its
/// map's (`ArrayMap`'s) order: those the app host has are sent with
/// `bindApplication`.
const COMMON_SERVICES: &[&str] = &[
    "permissionmgr",
    "appops",
    "account",
    "graphicsstats",
    "package",
    "window",
    "accessibility",
    "user",
    "alarm",
    "input",
    "mount",
    "power",
    "notification",
    "content",
    "input_method",
    "jobscheduler",
    "platform_compat",
    "display",
    "connectivity",
    "network_management",
];
/// `PackageManager.GET_SHARED_LIBRARY_FILES | MATCH_DEBUG_TRIAGED_MISSING`,
/// `ActivityManagerService.STOCK_PM_FLAGS`.
const STOCK_PM_FLAGS: i64 = 0x400 | 0x1000_0000;
/// `ActivityManager.PROCESS_STATE_TOP`.
const PROCESS_STATE_TOP: i32 = 2;
/// `Build.VERSION_CODES.P`: from it, apps get `Build.UNKNOWN` as the serial.
const P: i32 = 28;

/// Reads one of the device's properties.
pub type Properties = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Where the app host finds its inputs, set once guest-init knows them.
struct Configured {
    root: PathBuf,
    data: PathBuf,
    properties: Properties,
    connect: Connect,
}

/// What the app host reads, loaded when the first process starts.
struct Inputs {
    /// The guest's `/data`.
    data: PathBuf,
    compat: compat::CompatConfig,
    packages: packages::Packages,
    settings: settings::Settings,
    /// `persist.sys.locale`.
    locales: String,
    /// `ro.serialno`, the serial of apps targeting an SDK before P.
    serial: String,
    shared_memory: bind::ApplicationSharedMemory,
}

/// A process started and not yet attached, or attached.
struct App {
    package: String,
    uid: u32,
    pid: i32,
    started: Instant,
    /// `SystemClock.elapsedRealtime` and `uptimeMillis` at the start.
    start_elapsed: i64,
    start_uptime: i64,
    thread: Option<Strong>,
    /// The activity the process starts for, launched once its application
    /// is created.
    activity: Option<String>,
    /// The global configuration it was bound with.
    config: Option<config::Configuration>,
}

pub struct AppHost {
    process: Arc<LocalProcess>,
    configured: OnceLock<Configured>,
    inputs: OnceLock<Inputs>,
    loading: Mutex<()>,
    zygote: Mutex<Option<zygote::Session>>,
    next_seq: AtomicI64,
    apps: Mutex<HashMap<i64, App>>,
    /// The services of this host, by name, once registered.
    published: Mutex<Vec<(String, Binder)>>,
    /// The global configuration's sequence number.
    config_seq: AtomicI64,
    unimplemented: Mutex<std::collections::HashSet<(String, u32)>>,
    model: OnceLock<Arc<crate::package::model::State>>,
    resolver: crate::package::resolve::Resolver,
    activities: activity::Activities,
    display: OnceLock<Arc<(display::Display, display_service::Resources)>>,
    /// The settings provider and its connection token.
    settings_provider: OnceLock<(Binder, Binder)>,
}

impl AppHost {
    pub fn new(process: Arc<LocalProcess>) -> Arc<Self> {
        Arc::new(AppHost {
            process,
            configured: OnceLock::new(),
            inputs: OnceLock::new(),
            loading: Mutex::new(()),
            zygote: Mutex::new(None),
            next_seq: AtomicI64::new(1),
            apps: Mutex::new(HashMap::new()),
            published: Mutex::new(Vec::new()),
            config_seq: AtomicI64::new(1),
            unimplemented: Mutex::new(std::collections::HashSet::new()),
            model: OnceLock::new(),
            resolver: Default::default(),
            activities: Default::default(),
            display: OnceLock::new(),
            settings_provider: OnceLock::new(),
        })
    }

    /// The binders of the services published with it, which
    /// `bindApplication` hands apps.
    pub fn set_published(&self, services: &[(String, Binder)]) {
        *self.published.lock().unwrap() = services.to_vec();
    }

    /// The image below `root`, the guest's `/data` at `data`, the
    /// device's properties (read once the first process starts, when
    /// init has set them), and how the zygote relay is reached.
    pub fn configure(
        &self,
        root: &Path,
        data: &Path,
        properties: Properties,
        connect: Connect,
    ) -> Result<(), String> {
        self.configured
            .set(Configured {
                root: root.to_path_buf(),
                data: data.to_path_buf(),
                properties,
                connect,
            })
            .map_err(|_| "app host already configured".to_string())
    }

    /// The services it publishes, by binder name.
    pub fn services(self: &Arc<Self>) -> Vec<(&'static str, Arc<dyn Service>)> {
        let _ = self.settings_provider.set((
            self.process
                .add_service(Arc::new(settings_provider::SettingsProvider(self.clone()))),
            self.process.add_service(Arc::new(activity::Token)),
        ));
        vec![
            (
                "activity",
                Arc::new(Activity(self.clone())) as Arc<dyn Service>,
            ),
            (
                "package",
                Arc::new(package_service::PackageService::new(self.clone())),
            ),
            (
                "display",
                Arc::new(display_service::DisplayManager::new(self.clone())),
            ),
            (
                "activity_task",
                Arc::new(activity::ActivityTaskManager(
                    self.clone(),
                    self.process
                        .add_service(Arc::new(activity::ActivityClientController(self.clone()))),
                )),
            ),
            (CONTROL, Arc::new(Control(self.clone()))),
        ]
    }

    /// What starts read, loaded by the first.
    fn inputs(&self) -> Result<&Inputs, Exception> {
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let configured = self
            .configured
            .get()
            .ok_or_else(|| state("app host not configured".into()))?;
        if let Some(inputs) = self.inputs.get() {
            return Ok(inputs);
        }
        let _loading = self.loading.lock().unwrap();
        if let Some(inputs) = self.inputs.get() {
            return Ok(inputs);
        }
        let started = Instant::now();
        let property = |name: &str| {
            (configured.properties)(name).ok_or_else(|| state(format!("no property {name}")))
        };
        let number = |name: &str| {
            property(name)?
                .parse::<i32>()
                .map_err(|_| state(format!("{name} is not a number")))
        };
        let compat = compat::CompatConfig::load(
            &configured.root,
            number("ro.build.version.sdk")?,
            &property("ro.build.version.codename")?,
        )
        .map_err(state)?;
        let packages = packages::Packages::load(
            &configured.root,
            &configured.data,
            number("ro.sf.lcd_density")?,
            &property("persist.sys.locale")?,
            &|name| (configured.properties)(name),
        )
        .map_err(state)?;
        let settings = settings::Settings::load(&configured.data).map_err(state)?;
        let shared_memory = bind::ApplicationSharedMemory::new()
            .map_err(|e| state(format!("application shared memory: {e}")))?;
        eprintln!(
            "app host: {} packages loaded in {:?}",
            packages.state().packages.len(),
            started.elapsed()
        );
        let inputs = Inputs {
            data: configured.data.clone(),
            compat,
            packages,
            settings,
            locales: property("persist.sys.locale")?,
            serial: property("ro.serialno").unwrap_or_else(|_| "unknown".into()),
            shared_memory,
        };
        Ok(self.inputs.get_or_init(|| inputs))
    }

    /// `ProcessList.startProcessLocked` for an activity of `name`'s
    /// `ProcessList.startProcessLocked` for the activity `class` of
    /// `name`'s default process, user 0.
    fn start_process(&self, name: &str, class: &str) -> Result<i32, Exception> {
        let inputs = self.inputs()?;
        let state = package::State::read(&inputs.data, &[0])
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("package state: {e}")))?
            .ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    "no package state: Android never booted on this data",
                )
            })?;
        let setting = state
            .settings
            .packages
            .iter()
            .find(|p| p.name == name)
            .ok_or_else(|| Exception::illegal_argument(format!("unknown package {name}")))?;
        let listed = state.list.iter().find(|e| e.name == name).ok_or_else(|| {
            Exception::new(EX_ILLEGAL_STATE, format!("{name} not in packages.list"))
        })?;
        let uid = listed.uid;
        let app_id = uid % PER_USER_RANGE;
        let mut gids = listed.gids.clone();
        if app_id >= FIRST_APPLICATION_UID {
            gids.push(app_id - FIRST_APPLICATION_UID + FIRST_SHARED_APPLICATION_GID);
            gids.push(app_id - FIRST_APPLICATION_UID + FIRST_APPLICATION_CACHE_GID);
        }
        gids.push(SHARED_USER_GID);
        let target_sdk = setting.target_sdk_version;
        let start_seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let start = zygote::Start {
            uid,
            gids,
            runtime_flags: hidden_api_policy(&listed.seinfo, setting.flags)?
                << API_ENFORCEMENT_POLICY_SHIFT
                | memory_safety(&inputs.compat, setting.flags, target_sdk),
            // External storage has no owner in the app host yet (#1423).
            mount_external: zygote::MountExternal::None,
            target_sdk_version: target_sdk,
            nice_name: name.to_string(),
            seinfo: format!("{}{SEINFO_COMPLETE}", listed.seinfo),
            instruction_set: setting.primary_cpu_abi.as_deref().map(instruction_set),
            app_data_dir: Some(listed.data_dir.clone()),
            package_name: name.to_string(),
            is_top_app: true,
            disabled_compat_changes: inputs.compat.disabled_changes(target_sdk),
            start_seq,
        };
        let started = Instant::now();
        let pid = {
            let mut zygote = self.zygote.lock().unwrap();
            if zygote.is_none() {
                let stream = (self.configured.get().unwrap().connect)()
                    .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("zygote relay: {e}")))?;
                *zygote = Some(zygote::Session::new(stream));
            }
            match zygote.as_mut().unwrap().start(&start) {
                Ok(pid) => pid,
                Err(e) => {
                    *zygote = None;
                    return Err(Exception::new(EX_ILLEGAL_STATE, format!("zygote: {e}")));
                }
            }
        };
        eprintln!(
            "app host: Start proc {pid}:{name}/{uid} in {:?}",
            started.elapsed()
        );
        self.apps.lock().unwrap().insert(
            start_seq,
            App {
                package: name.to_string(),
                uid,
                pid,
                started,
                start_elapsed: clock_ms(libc::CLOCK_MONOTONIC_RAW),
                start_uptime: clock_ms(libc::CLOCK_UPTIME_RAW),
                thread: None,
                activity: Some(class.to_string()),
                config: None,
            },
        );
        Ok(pid)
    }

    /// `attachApplicationLocked`, up to binding: the start the caller's
    /// uid and sequence number name, linked to the thread's death.
    fn attach(
        &self,
        call: &Call<'_>,
        thread: Option<Binder>,
        start_seq: i64,
    ) -> Result<(), Exception> {
        let Some(Binder::Handle(handle)) = thread else {
            return Err(Exception::illegal_argument(
                "attachApplication without a thread",
            ));
        };
        let mut apps = self.apps.lock().unwrap();
        let strong = self.process.strong(handle);
        let Some(app) = apps
            .get_mut(&start_seq)
            .filter(|a| a.uid == call.sender_euid && a.thread.is_none())
        else {
            // As the original drops a process it has no pending start of
            // (its pid is the guest's, so it is told to exit).
            eprintln!(
                "app host: no pending start {start_seq} for pid {}; dropping process",
                call.sender_pid
            );
            let mut exit = Parcel::new();
            aim_service_aidl::android_app_iapplicationthread::ScheduleExit {}.write(&mut exit);
            let _ = strong.transact(
                aim_service_aidl::android_app_iapplicationthread::SCHEDULE_EXIT,
                &exit,
                true,
            );
            return Ok(());
        };
        let seq = start_seq;
        self.process.link_to_death(
            &strong,
            Box::new(move || {
                eprintln!("app host: process of start {seq} died");
            }),
        );
        eprintln!(
            "app host: {} (pid {}, sent {}) attached {:?} after its start",
            app.package,
            app.pid,
            call.sender_pid,
            app.started.elapsed()
        );
        let (bind, config) = self.bind_application(app)?;
        app.config = Some(config);
        strong
            .transact(thread::BIND_APPLICATION, &bind, true)
            .map_err(|s| Exception::new(EX_ILLEGAL_STATE, format!("bindApplication: {s}")))?;
        let mut state = Parcel::new();
        thread::SetProcessState {
            state: PROCESS_STATE_TOP,
        }
        .write(&mut state);
        strong
            .transact(thread::SET_PROCESS_STATE, &state, true)
            .map_err(|s| Exception::new(EX_ILLEGAL_STATE, format!("setProcessState: {s}")))?;
        eprintln!(
            "app host: {} bound {:?} after its start",
            app.package,
            app.started.elapsed()
        );
        app.thread = Some(strong);
        Ok(())
    }

    /// `ActivityManager.checkComponentPermission` of an exported
    /// component nobody owns: root and system hold every permission,
    /// isolated processes none, other uids what PackageManager granted
    /// them (`checkUidPermission`).
    fn check_permission(&self, permission: Option<&str>, uid: i32) -> Result<i32, Exception> {
        const GRANTED: i32 = 0;
        const DENIED: i32 = -1;
        let app_id = uid.rem_euclid(PER_USER_RANGE as i32);
        if app_id == 0 || app_id == 1000 {
            return Ok(GRANTED);
        }
        // `Process.isIsolated`: FIRST_ISOLATED_UID..LAST_ISOLATED_UID.
        if (99000..=99999).contains(&app_id) || (90000..=98999).contains(&app_id) {
            return Ok(DENIED);
        }
        let Some(permission) = permission else {
            return Ok(DENIED);
        };
        let (state, resolution) = self.packages()?;
        let query = crate::package::query::Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        match query.uid_has_permission(uid, permission) {
            Ok(true) => Ok(GRANTED),
            Ok(false) => Ok(DENIED),
            Err(crate::package::apps_filter::NotModelled(reason)) => Err(Exception::new(
                EX_UNSUPPORTED_OPERATION,
                format!("checkPermission: {reason}"),
            )),
        }
    }

    /// `finishAttachApplication`: the application's `onCreate` returned,
    /// so the activity its process started for is launched
    /// (`ActivityTaskManagerInternal.attachApplication`).
    fn finish_attach(&self, call: &Call<'_>, start_seq: i64) -> Result<(), Exception> {
        let mut apps = self.apps.lock().unwrap();
        let app = apps
            .get_mut(&start_seq)
            .filter(|a| a.uid == call.sender_euid && a.thread.is_some())
            .ok_or_else(|| Exception::illegal_argument(format!("no attached start {start_seq}")))?;
        eprintln!(
            "app host: {} created its application {:?} after its start",
            app.package,
            app.started.elapsed()
        );
        let Some(class) = app.activity.take() else {
            return Ok(());
        };
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let (model, resolution) = self.packages()?;
        let query = crate::package::query::Query {
            state: &model,
            filter: &resolution.apps_filter,
            calling_uid: 1000,
        };
        let component = crate::package::intent::ComponentName {
            package: app.package.clone(),
            class: class.clone(),
        };
        let info = match query.activity_info(&component, STOCK_PM_FLAGS, 1000, 0) {
            Ok(Ok(Some(info))) => info,
            Ok(Ok(None)) => {
                return Err(Exception::illegal_argument(format!(
                    "no activity {}/{class}",
                    app.package
                )));
            }
            Ok(Err(e)) => return Err(e),
            Err(crate::package::apps_filter::NotModelled(reason)) => {
                return Err(state(format!("activity info: {reason}")));
            }
        };
        let record = activity::ActivityRecord {
            package: app.package.clone(),
            class,
            uid: app.uid,
            start_seq,
            started: app.started,
            resumed: false,
            requested_orientation: -1,
        };
        let global = app
            .config
            .clone()
            .ok_or_else(|| state("launch before bind".into()))?;
        let (launch, top) = self.launch_activity(
            record,
            activity::Launch {
                info,
                global: &global,
            },
        );
        let thread = app.thread.as_ref().unwrap();
        for (what, transaction) in [("launch", launch), ("top resumed", top)] {
            thread
                .transact(thread::SCHEDULE_TRANSACTION, &transaction, true)
                .map_err(|s| state(format!("scheduleTransaction ({what}): {s}")))?;
        }
        eprintln!(
            "app host: {} launch sent {:?} after its start",
            app.package,
            app.started.elapsed()
        );
        Ok(())
    }

    /// The default display, from SurfaceFlinger, and the framework's
    /// resources describing it, read once.
    fn display(&self) -> Result<Arc<(display::Display, display_service::Resources)>, Exception> {
        if let Some(display) = self.display.get() {
            return Ok(display.clone());
        }
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let inputs = self.inputs()?;
        let display = display::Display::query(&self.process).map_err(state)?;
        let packages = &inputs.packages;
        let integer = |name: &str| {
            packages
                .framework_integer(name)
                .ok_or_else(|| state(format!("no {name}")))
        };
        let resources = display_service::Resources {
            name: packages
                .framework_string("display_manager_built_in_display_name")
                .ok_or_else(|| state("no display_manager_built_in_display_name".into()))?,
            density_dpi: display.density_dpi(),
            brightness: (
                integer("config_screenBrightnessSettingMinimum")?,
                integer("config_screenBrightnessSettingMaximum")?,
                integer("config_screenBrightnessSettingDefault")?,
            ),
            brightness_dim: packages
                .framework_float("config_screenBrightnessDimFloat")
                .ok_or_else(|| state("no config_screenBrightnessDimFloat".into()))?,
        };
        Ok(self
            .display
            .get_or_init(|| Arc::new((display, resources)))
            .clone())
    }

    /// The package model's state and its resolution, made once.
    fn packages(
        &self,
    ) -> Result<
        (
            Arc<crate::package::model::State>,
            Arc<crate::package::resolve::Resolution>,
        ),
        Exception,
    > {
        let inputs = self.inputs()?;
        let state = self
            .model
            .get_or_init(|| Arc::new(inputs.packages.state().clone()))
            .clone();
        let resolution = self
            .resolver
            .resolution(&state)
            .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "package resolution: MIME groups"))?;
        Ok((state, resolution))
    }

    /// The exception of a method `name` of `service` it does not
    /// implement, logged once.
    fn unimplemented_named(&self, service: &str, name: &str) -> Exception {
        if self
            .unimplemented
            .lock()
            .unwrap()
            .insert((format!("{service}:{name}"), 0))
        {
            eprintln!("app host: {service} {name} not implemented");
        }
        Exception::new(
            EX_UNSUPPORTED_OPERATION,
            format!("{service} {name} is not implemented by the app host"),
        )
    }

    /// `ActivityManagerService.getContentProvider`: the providers the app
    /// host serves (`settings`), else none.
    fn content_provider(
        &self,
        name: Option<&str>,
    ) -> Result<Option<settings_provider::ContentProviderHolder>, Exception> {
        const GET_URI_PERMISSION_PATTERNS: i64 = 0x800;
        let Some("settings") = name else {
            return Ok(None);
        };
        let (state, resolution) = self.packages()?;
        let state_error = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let registered = resolution
            .components
            .provider_by_authority("settings")
            .ok_or_else(|| state_error("no settings provider".into()))?;
        let class = state
            .packages
            .get(&registered.package)
            .and_then(|ps| ps.pkg.as_ref())
            .and_then(|pkg| pkg.providers.get(registered.index))
            .map(|provider| provider.main.component.name.clone())
            .ok_or_else(|| state_error("settings provider not parsed".into()))?;
        let query = crate::package::query::Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: 1000,
        };
        let component = crate::package::intent::ComponentName {
            package: registered.package.clone(),
            class,
        };
        let info = match query.provider_info(
            &component,
            STOCK_PM_FLAGS | GET_URI_PERMISSION_PATTERNS,
            0,
        ) {
            Ok(Ok(Some(info))) => info,
            Ok(Ok(None)) => return Err(state_error("settings provider info".into())),
            Ok(Err(e)) => return Err(e),
            Err(crate::package::apps_filter::NotModelled(reason)) => {
                return Err(state_error(format!("provider info: {reason}")));
            }
        };
        let (provider, connection) = *self
            .settings_provider
            .get()
            .ok_or_else(|| state_error("no settings provider binder".into()))?;
        Ok(Some(settings_provider::ContentProviderHolder {
            info,
            provider,
            connection,
        }))
    }

    /// The exception of a method of `service` it does not implement,
    /// logged once.
    fn unimplemented(&self, service: &str, code: u32) -> Exception {
        if self
            .unimplemented
            .lock()
            .unwrap()
            .insert((service.to_string(), code))
        {
            eprintln!("app host: {service}#{code} not implemented");
        }
        Exception::new(
            EX_UNSUPPORTED_OPERATION,
            format!("{service}#{code} is not implemented by the app host"),
        )
    }

    /// `bindApplication`'s call for `app`'s process.
    fn bind_application(&self, app: &App) -> Result<(Parcel, config::Configuration), Exception> {
        let inputs = self.inputs()?;
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let info = inputs
            .packages
            .application_info(&app.package, STOCK_PM_FLAGS)
            .ok_or_else(|| state(format!("no application info of {}", app.package)))?;
        let display = self.display()?.0.clone();
        let config = config::Configuration::of(
            &display,
            &config::Device {
                locales: &inputs.locales,
                right_to_left: false,
                night: crate::uimode::mac::dark(),
                font_scale: inputs
                    .settings
                    .get(settings::Table::System, "font_scale")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(1.0),
                font_weight_adjustment: inputs
                    .settings
                    .get(settings::Table::Secure, "font_weight_adjustment")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
                seq: self.config_seq.load(Ordering::Relaxed) as i32,
            },
        );
        let compat_info = config::CompatibilityInfo::of(&info, &config);
        let analog_clock_fps = inputs
            .packages
            .framework_integer("config_defaultAnalogClockSecondsHandFps")
            .ok_or_else(|| state("no config_defaultAnalogClockSecondsHandFps".into()))?;
        let core_settings =
            bind::core_settings(&inputs.settings, analog_clock_fps).map_err(state)?;
        let published = self.published.lock().unwrap();
        let services: Vec<(ParcelValue, ParcelValue)> = COMMON_SERVICES
            .iter()
            .filter_map(|name| published.iter().find(|(n, _)| n == name))
            .map(|(name, binder)| {
                (
                    ParcelValue::String(Some(name.clone())),
                    ParcelValue::Binder(Some(*binder)),
                )
            })
            .collect();
        let target_sdk = info.target_sdk_version;
        let mut p = Parcel::new();
        thread::BindApplication::<
            package::info::ApplicationInfo,
            bind::ProviderInfoList,
            bind::Absent,
            bind::Absent,
            bind::Bundle,
            config::Configuration,
            config::CompatibilityInfo,
            bind::Absent,
            bind::Absent,
            bind::Absent,
            bind::RawFileDescriptor,
        > {
            package_name: Some(app.package.clone()),
            info: Some(info),
            sdk_sandbox_client_app_volume_uuid: None,
            sdk_sandbox_client_app_package: None,
            is_sdk_in_sandbox: false,
            // The app host does not publish providers yet (#1424).
            provider_list: Some(bind::ProviderInfoList(Vec::new())),
            test_name: None,
            profiler_info: None,
            test_arguments: None,
            test_watcher: None,
            ui_automation_connection: None,
            debug_mode: 0,
            enable_binder_tracking: false,
            track_allocation: false,
            restricted_backup_mode: false,
            persistent: false,
            config: Some(config.clone()),
            compat_info: Some(compat_info),
            services: Some(services),
            core_settings: Some(bind::Bundle(core_settings)),
            build_serial: Some(if target_sdk < P {
                inputs.serial.clone()
            } else {
                "unknown".into()
            }),
            autofill_options: None,
            content_capture_options: None,
            disabled_compat_changes: Some(inputs.compat.disabled_changes(target_sdk)),
            loggable_compat_changes: Some(inputs.compat.loggable_changes(target_sdk)),
            serialized_system_font_map: None,
            application_shared_memory_fd: bind::RawFileDescriptor(
                inputs
                    .shared_memory
                    .file()
                    .map_err(|e| state(e.to_string()))?,
            ),
            start_requested_elapsed_time: app.start_elapsed,
            start_requested_uptime: app.start_uptime,
        }
        .write(&mut p);
        Ok((p, config))
    }
}

/// `clock`'s milliseconds, as the guest's `SystemClock` reads them: its
/// MONOTONIC (uptime) is Darwin's `mach_absolute_time`, its BOOTTIME
/// (elapsed) `mach_continuous_time` (`aim_linux_abi`'s vDSO).
fn clock_ms(clock: libc::clockid_t) -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(clock, &mut ts) };
    ts.tv_sec * 1000 + ts.tv_nsec / 1_000_000
}

/// `ApplicationInfo.getHiddenApiEnforcementPolicy` with the device's
/// default policy: disabled for apps signed with the platform key, which
/// `isAllowedToUseHiddenApis`; a system app's allowlist is not modelled.
fn hidden_api_policy(seinfo: &str, flags: i32) -> Result<i32, Exception> {
    if seinfo == "platform" || seinfo.starts_with("platform:") {
        return Ok(HIDDEN_API_ENFORCEMENT_DISABLED);
    }
    if flags & FLAG_SYSTEM != 0 {
        return Err(Exception::new(
            EX_UNSUPPORTED_OPERATION,
            "hidden API allowlist of system apps",
        ));
    }
    Ok(HIDDEN_API_ENFORCEMENT_ENABLED)
}

/// `Zygote.getMemorySafetyRuntimeFlags` for an app with no manifest
/// overrides, on a kernel without tagged pointers (the layer answers
/// `PR_GET_TAGGED_ADDR_CTRL` with `EINVAL`), which disables tagging.
fn memory_safety(compat: &compat::CompatConfig, flags: i32, target_sdk: i32) -> i32 {
    let gwp_asan = if compat.is_enabled(GWP_ASAN, target_sdk) {
        GWP_ASAN_LEVEL_ALWAYS
    } else if flags & FLAG_SYSTEM != 0 {
        GWP_ASAN_LEVEL_LOTTERY
    } else {
        GWP_ASAN_LEVEL_DEFAULT
    };
    let zero_init = if compat.is_enabled(NATIVE_HEAP_ZERO_INIT, target_sdk) {
        NATIVE_HEAP_ZERO_INIT_ENABLED
    } else {
        0
    };
    gwp_asan | zero_init
}

/// `VMRuntime.getInstructionSet`.
fn instruction_set(abi: &str) -> String {
    match abi {
        "arm64-v8a" => "arm64",
        "armeabi" | "armeabi-v7a" => "arm",
        "x86_64" => "x86_64",
        "x86" => "x86",
        other => other,
    }
    .to_string()
}

fn reply(result: Result<Parcel, Exception>) -> Reply {
    Ok(result.unwrap_or_else(|exception| {
        let mut reply = Parcel::new();
        reply.write_exception(&exception);
        reply
    }))
}

/// `activity`: the `IActivityManager` calls of an app's start.
struct Activity(Arc<AppHost>);

impl Service for Activity {
    fn descriptor(&self) -> &str {
        am::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            am::ATTACH_APPLICATION => {
                let args = am::AttachApplication::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.attach(call, args.app, args.start_seq).map(|()| {
                    let mut p = Parcel::new();
                    am::write_attach_application_reply(&mut p);
                    p
                }))
            }
            am::CHECK_PERMISSION => {
                let args = am::CheckPermission::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .check_permission(args.permission.as_deref(), args.uid)
                        .map(|result| {
                            let mut p = Parcel::new();
                            am::write_check_permission_reply(&mut p, result);
                            p
                        }),
                )
            }
            am::CHECK_PERMISSION_FOR_DEVICE => {
                let args =
                    am::CheckPermissionForDevice::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                // Device-specific grants are the default device's on a
                // device without virtual devices.
                reply(
                    self.0
                        .check_permission(args.permission.as_deref(), args.uid)
                        .map(|result| {
                            let mut p = Parcel::new();
                            am::write_check_permission_for_device_reply(&mut p, result);
                            p
                        }),
                )
            }
            am::GET_CONTENT_PROVIDER => {
                let args = am::GetContentProvider::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.content_provider(args.name.as_deref()).map(|holder| {
                    let mut p = Parcel::new();
                    am::write_get_content_provider_reply(&mut p, holder.as_ref());
                    p
                }))
            }
            am::FINISH_ATTACH_APPLICATION => {
                let args =
                    am::FinishAttachApplication::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.finish_attach(call, args.start_seq).map(|()| {
                    let mut p = Parcel::new();
                    am::write_finish_attach_application_reply(&mut p);
                    p
                }))
            }
            // `handleApplicationCrash`: the crash is the process's last
            // act (`RuntimeInit` kills it after the call); it is logged.
            am::HANDLE_APPLICATION_CRASH => {
                eprintln!(
                    "app host: process {} (uid {}) crashed",
                    call.sender_pid, call.sender_euid
                );
                let mut p = Parcel::new();
                p.write_no_exception();
                Ok(p)
            }
            // A Java proxy reads an empty reply as success with zero
            // results (a granted permission), so a method the app host
            // does not implement throws.
            code => reply(Err(self.0.unimplemented("activity", code))),
        }
    }
}

/// `aim.app_host`: starts processes for the shell.
struct Control(Arc<AppHost>);

impl Service for Control {
    fn descriptor(&self) -> &str {
        CONTROL_DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != START_ACTIVITY {
            return Err(UNKNOWN_TRANSACTION);
        }
        // Root and the shell, as `am start` is theirs.
        if !matches!(call.sender_euid, 0 | 2000) {
            return reply(Err(Exception::security("startActivity is the shell's")));
        }
        let name = call
            .data
            .enforce_interface(CONTROL_DESCRIPTOR)
            .and_then(|()| call.data.read_string16())
            .map_err(|_| BAD_VALUE)?;
        let Some(component) = name else {
            return reply(Err(Exception::new(EX_NULL_POINTER, "component")));
        };
        let Some((package, class)) = component.split_once('/') else {
            return reply(Err(Exception::illegal_argument(format!(
                "{component}: not package/class"
            ))));
        };
        // `ComponentName.unflattenFromString`.
        let class = if class.starts_with('.') {
            format!("{package}{class}")
        } else {
            class.to_string()
        };
        reply(self.0.start_process(package, &class).map(|pid| {
            let mut p = Parcel::new();
            p.write_no_exception();
            p.write_i32(pid);
            p
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_api_policy_by_signer() {
        assert_eq!(
            hidden_api_policy("platform:targetSdkVersion=36", FLAG_SYSTEM).unwrap(),
            0
        );
        assert_eq!(
            hidden_api_policy("default:targetSdkVersion=35", 0).unwrap(),
            2
        );
        assert!(hidden_api_policy("default:privapp:targetSdkVersion=35", FLAG_SYSTEM).is_err());
    }

    #[test]
    fn memory_safety_of_a_user_app() {
        let compat = compat::CompatConfig::default();
        // Undeclared changes are enabled.
        assert_eq!(
            memory_safety(&compat, 0, 35),
            GWP_ASAN_LEVEL_ALWAYS | NATIVE_HEAP_ZERO_INIT_ENABLED
        );
        assert_eq!(instruction_set("arm64-v8a"), "arm64");
    }
}
