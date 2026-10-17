//! The app host (docs/app-host.md): ActivityManager's side of an app's
//! process, native, for the app host image, which has no system_server.
//! It starts an app's process through zygote, as `ProcessList` does,
//! takes the process's `attachApplication` and binds it, as
//! `attachApplicationLocked` does.
//!
//! The process's start arguments are `ProcessList.startProcessLocked`'s
//! at `android-16.0.0_r1`, from the state PackageManager last persisted
//! (`packages.xml`, `packages.list`).

pub mod account_authenticators;
pub mod account_service;
pub mod accounts_db;
pub mod activity;
pub mod activity_stack;
pub mod activity_start;
pub mod alarm_service;
pub mod appops_service;
pub mod audio_policy;
pub mod audio_service;
pub mod bind;
pub mod broadcasts;
pub mod compat;
pub mod config;
pub mod content_service;
pub mod display;
pub mod display_service;
pub mod freeform;
pub mod input_manager;
pub mod input_method;
pub mod keychars;
pub mod kept;
pub mod notification_channels;
pub mod notification_service;
pub mod package_service;
pub mod packages;
pub mod pending_intents;
pub mod permission_controller;
pub mod playback;
pub mod power_service;
pub mod sensor_privacy;
pub mod services;
pub mod settings;
pub mod settings_provider;
pub mod status_bar;
pub mod storage;
pub mod surface;
pub mod tasks;
pub mod trust_service;
pub mod user_service;
pub mod volume;
pub mod window_layout;
pub mod window_manager;
pub mod zygote;

use std::collections::HashMap;
use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Instant;

use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception,
    Parcel, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::{ParcelValue, WriteParcelable};
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
/// `void attachAgent(IBinder agent)`: the app host agent (`aim-app-host-agent`).
const ATTACH_AGENT: u32 = 2;
/// The agent's `openInputChannel(String name, int displayWidth, int
/// displayHeight, int left, int top, int right, int bottom)`, and where
/// the client's socket (a `ParcelFileDescriptor`) starts in its reply:
/// after the status and the channel's index; its `focusInputChannel(int
/// index)`, `setInputChannelFrame(int index, int left, int top, int
/// right, int bottom)` and `killProcess(int pid)`.
const AGENT_DESCRIPTOR: &str = "dev.aim.IAppHostAgent";
const OPEN_INPUT_CHANNEL: u32 = 1;
const AGENT_CHANNEL_FD: usize = 8;
const FOCUS_INPUT_CHANNEL: u32 = 2;
const SET_INPUT_CHANNEL_FRAME: u32 = 3;
const KILL_PROCESS: u32 = 4;

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
/// `ProcessList.APP_DATA_DIRECTORY_ISOLATION`.
const APP_DATA_DIRECTORY_ISOLATION: i64 = 143937733;
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
/// `PackageManager.GET_URI_PERMISSION_PATTERNS`, `MATCH_DIRECT_BOOT_AUTO`.
const GET_URI_PERMISSION_PATTERNS: i64 = 0x800;
const MATCH_DIRECT_BOOT_AUTO: i64 = 0x1000_0000;
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

/// A start's app data isolation (`zygote::Start`'s).
#[derive(Default)]
struct Isolation {
    packages: Vec<zygote::DataInfo>,
    allowlisted: Vec<zygote::DataInfo>,
    storage_dirs: bool,
    data_dirs: bool,
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
    activity: Option<activity_start::StartRequest>,
    /// The global configuration it was bound with.
    config: Option<config::Configuration>,
    /// Its start's first frame (`ApplicationStartInfo
    /// .TIMESTAMP_INITIAL_RENDERTHREAD_FRAME`,
    /// `TIMESTAMP_SURFACEFLINGER_COMPOSITION_COMPLETE`), uptime nanoseconds.
    first_frame: Option<(i64, i64)>,
    /// When an activity of it was last resumed, uptime milliseconds
    /// (`ProcessRecord.getLastActivityTime`).
    last_activity: i64,
}

pub struct AppHost {
    me: Weak<AppHost>,
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
    windows: window_manager::Windows,
    composer: OnceLock<surface::Composer>,
    agent: Mutex<Option<Strong>>,
    tasks: tasks::Tasks,
    /// The server's launches, by package, until the package's task is made.
    pending_launches: Mutex<HashMap<String, String>>,
    /// The content providers apps published, by authority.
    providers: Mutex<HashMap<String, Published>>,
    storage: storage::Volumes,
    text_input: input_method::TextInput,
    content_service: OnceLock<Arc<content_service::ContentService>>,
    notification_service: OnceLock<Arc<notification_service::NotificationManager>>,
    power_service: OnceLock<Arc<power_service::PowerManager>>,
    alarm_service: OnceLock<Arc<alarm_service::AlarmManager>>,
    services: services::Services,
    pending_intents: pending_intents::PendingIntents,
    broadcasts: broadcasts::Broadcasts,
    /// User 0's restrictions.
    user_restrictions: OnceLock<std::collections::BTreeSet<String>>,
    /// audioserver, once it registered, and the stream volumes.
    audio_policy: OnceLock<audio_policy::AudioPolicy>,
    volumes: Mutex<Option<volume::Volumes>>,
    /// The players and recorders apps made.
    playback: Mutex<playback::Playback>,
}

/// A content provider an app's process published
/// (`publishContentProviders`).
struct Published {
    start_seq: i64,
    uid: u32,
    info: package::info::ProviderInfo,
    provider: Strong,
}

impl AppHost {
    pub fn new(process: Arc<LocalProcess>) -> Arc<Self> {
        Arc::new_cyclic(|me| AppHost {
            me: me.clone(),
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
            windows: Default::default(),
            composer: OnceLock::new(),
            agent: Mutex::new(None),
            tasks: Default::default(),
            pending_launches: Mutex::new(HashMap::new()),
            providers: Mutex::new(HashMap::new()),
            storage: Default::default(),
            text_input: Default::default(),
            content_service: OnceLock::new(),
            notification_service: OnceLock::new(),
            power_service: OnceLock::new(),
            alarm_service: OnceLock::new(),
            services: Default::default(),
            pending_intents: Default::default(),
            broadcasts: Default::default(),
            user_restrictions: OnceLock::new(),
            audio_policy: OnceLock::new(),
            volumes: Mutex::new(None),
            playback: Mutex::default(),
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
            ("window", Arc::new(window_manager::WindowManager(self.clone()))),
            ("user", Arc::new(user_service::UserManager::new(self.clone()))),
            ("appops", Arc::new(appops_service::AppOps::new(self.clone()))),
            ("trust", Arc::new(trust_service::TrustManager::new(self.clone()))),
            ("permission", Arc::new(permission_controller::PermissionController(self.clone()))),
            ("input_method", Arc::new(input_method::InputMethodManager::new(self.clone()))),
            ("input", Arc::new(input_manager::InputManager::new(self.clone()))),
            ("audio", Arc::new(audio_service::AudioService::new(self.clone()))),
            ("mount", Arc::new(storage::StorageManager(self.clone()))),
            ("sensor_privacy", Arc::new(sensor_privacy::SensorPrivacy::new(self.clone()))),
            (
                "content",
                self.content_service
                    .get_or_init(|| Arc::new(content_service::ContentService::new(self.clone())))
                    .clone(),
            ),
            (
                "notification",
                self.notification_service
                    .get_or_init(|| Arc::new(notification_service::NotificationManager::new(self.clone())))
                    .clone(),
            ),
            (
                "power",
                self.power_service
                    .get_or_init(|| Arc::new(power_service::PowerManager::new(self.clone())))
                    .clone(),
            ),
            (
                "alarm",
                self.alarm_service
                    .get_or_init(|| Arc::new(alarm_service::AlarmManager::new(self.clone())))
                    .clone(),
            ),
            ("account", Arc::new(account_service::AccountManager::new(self.clone()))),
            ("statusbar", Arc::new(status_bar::StatusBar::new(self.clone()))),
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
    /// Starts the process of package `name` for the launcher's start of
    /// its activity `class`.
    fn start_process(&self, name: &str, class: &str) -> Result<i32, Exception> {
        self.start_process_for(name, Some(activity_start::StartRequest::launcher(class)))
    }

    /// Starts the process of package `name` for `request`, or for a
    /// service, a provider or a broadcast (no activity).
    fn start_process_for(&self, name: &str, request: Option<activity_start::StartRequest>) -> Result<i32, Exception> {
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
        let app_info = inputs
            .packages
            .application_info(name, STOCK_PM_FLAGS)
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, format!("no application info of {name}")))?;
        let start_seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let isolation = self.data_isolation(name, uid, target_sdk)?;
        let start = zygote::Start {
            uid,
            gids,
            runtime_flags: hidden_api_policy(
                app_info.flags,
                app_info.private_flags,
                app_info.private_flags_ext,
                inputs.settings.get(settings::Table::Global, "hidden_api_policy"),
            )
                << API_ENFORCEMENT_POLICY_SHIFT
                | memory_safety(&inputs.compat, setting.flags, target_sdk),
            // `getExternalStorageMountMode`: every app's scoped view.
            mount_external: zygote::MountExternal::Default,
            target_sdk_version: target_sdk,
            nice_name: name.to_string(),
            seinfo: format!("{}{SEINFO_COMPLETE}", listed.seinfo),
            instruction_set: setting.primary_cpu_abi.as_deref().map(instruction_set),
            app_data_dir: Some(listed.data_dir.clone()),
            package_name: name.to_string(),
            is_top_app: true,
            pkg_data_info: isolation.packages,
            allowlisted_data_info: isolation.allowlisted,
            bind_mount_app_storage_dirs: isolation.storage_dirs,
            bind_mount_app_data_dirs: isolation.data_dirs,
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
                activity: request,
                config: None,
                first_frame: None,
                last_activity: 0,
            },
        );
        Ok(pid)
    }

    /// `ProcessList.startProcess`'s app data isolation for `package`'s
    /// process of `uid`: its packages' data (its shared user's, else its
    /// own) and the allowlisted apps', which zygote binds alone into the
    /// process's view of `/data/user` (`--bind-mount-data-dirs`); the
    /// storage directories' isolation is vold's
    /// (`persist.sys.vold_app_data_isolation_enabled`).
    fn data_isolation(&self, package: &str, uid: u32, target_sdk: i32) -> Result<Isolation, Exception> {
        let inputs = self.inputs()?;
        let configured = self.configured.get().ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host not configured"))?;
        let flag = |name: &str, default: bool| system_boolean((configured.properties)(name).as_deref(), default);
        let (state, _) = self.packages()?;
        let user = (uid / PER_USER_RANGE) as i32;
        let app_id = uid % PER_USER_RANGE;
        let mut data_dirs = flag("persist.zygote.app_data_isolation", true)
            && (FIRST_APPLICATION_UID..=19_999).contains(&app_id)
            && inputs.compat.is_enabled(APP_DATA_DIRECTORY_ISOLATION, target_sdk);
        let ps = state.packages.get(package).ok_or_else(|| Exception::illegal_argument(format!("unknown package {package}")))?;
        let targets: Vec<&str> = match &ps.shared_user {
            Some(shared) => state.packages.values().filter(|p| p.shared_user.as_ref() == Some(shared)).map(|p| p.name.as_str()).collect(),
            None => vec![package],
        };
        // `getPackageAppDataInfoMap`: none when a package has no CE data
        // inode yet; unknown packages are left out.
        let infos = |names: &mut dyn Iterator<Item = &str>| -> Option<Vec<zygote::DataInfo>> {
            let mut infos = Vec::new();
            for name in names {
                let Some(ps) = state.packages.get(name) else { continue };
                let inode = package::info::user_state(ps, user).ce_data_inode;
                if inode <= 0 {
                    return None;
                }
                infos.push(zygote::DataInfo { package: name.to_string(), volume_uuid: ps.volume_uuid.clone(), ce_data_inode: inode });
            }
            infos.sort_by_key(|i| package::info::java_hash(&i.package));
            Some(infos)
        };
        let packages = infos(&mut targets.iter().copied());
        let allowlist = &state.system.app_data_isolation_allowlist;
        let allowlisted = infos(&mut allowlist.iter().map(String::as_str).filter(|p| !targets.contains(p)));
        data_dirs &= packages.is_some() && allowlisted.is_some();
        // `hasAppStorage`.
        let no_storage = ps.pkg.as_ref().and_then(|p| p.properties.as_ref()).is_some_and(|props| {
            props.iter().any(|(n, p)| n == "android.internal.PROPERTY_NO_APP_DATA_STORAGE" && p.value == package::pkg::PropertyValue::Bool(true))
        });
        if no_storage {
            return Ok(Isolation::default());
        }
        // `needsStorageDataIsolation`, with the emulated volume mounted.
        let storage_dirs = flag("persist.sys.vold_app_data_isolation_enabled", false) && packages.is_some() && self.storage_mounted();
        Ok(Isolation { packages: packages.unwrap_or_default(), allowlisted: allowlisted.unwrap_or_default(), storage_dirs, data_dirs })
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
        let pid = app.pid;
        let host = self.me.clone();
        self.process.link_to_death(
            &strong,
            Box::new(move || {
                eprintln!("app host: process of start {seq} died");
                if let Some(host) = host.upgrade() {
                    host.process_died(seq, pid);
                }
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

    /// The process of `start_seq`, `pid`, died: what it had goes.
    fn process_died(&self, start_seq: i64, pid: i32) {
        self.apps.lock().unwrap().remove(&start_seq);
        self.providers.lock().unwrap().retain(|_, p| p.start_seq != start_seq);
        self.activities.remove_start(start_seq);
        self.windows_died(pid);
        self.tasks_died(start_seq);
        self.broadcasts.process_died(self, start_seq);
        if let Some(me) = self.me.upgrade() {
            me.services_process_died(start_seq);
        }
    }

    /// User 0's base restrictions, as UserManagerService last persisted
    /// them (`/data/system/users/0.xml`'s `<restrictions>`).
    fn user_restrictions(&self) -> Result<&std::collections::BTreeSet<String>, Exception> {
        if let Some(r) = self.user_restrictions.get() {
            return Ok(r);
        }
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let path = self.data()?.join("system/users/0.xml");
        let bytes = std::fs::read(&path).map_err(|e| state(format!("{}: {e}", path.display())))?;
        let root = aim_android_xml::read(&bytes).map_err(|e| state(format!("{}: {e}", path.display())))?;
        let restrictions = root
            .children()
            .find(|e| e.name == "restrictions")
            .map(|r| r.attrs.iter().filter(|(_, v)| matches!(v, aim_android_xml::Value::Bool(true))).map(|(k, _)| k.clone()).collect())
            .unwrap_or_default();
        Ok(self.user_restrictions.get_or_init(|| restrictions))
    }

    /// An activity of the process of `start_seq` was resumed.
    fn activity_changed(&self, start_seq: i64) {
        if let Some(app) = self.apps.lock().unwrap().get_mut(&start_seq) {
            app.last_activity = clock_ms(libc::CLOCK_UPTIME_RAW);
        }
    }

    /// `ProcessList.fillInProcMemInfoLOSP` of `app`, the process of
    /// `start_seq`: each of the app host's processes is top.
    fn fill_in_proc_mem_info(&self, start_seq: i64, app: &App, info: &mut RunningAppProcessInfo, front: Option<i64>) {
        info.pid = app.pid;
        info.uid = app.uid as i32;
        if self.activities.has_start(start_seq) {
            info.flags |= FLAG_HAS_ACTIVITIES;
        }
        info.importance = IMPORTANCE_FOREGROUND;
        info.process_state = PROCESS_STATE_TOP;
        info.is_focused = front == Some(start_seq);
        info.last_activity_time = app.last_activity;
    }

    /// `getRunningAppProcesses`: the caller's uid's processes, or its
    /// user's or all with the permissions, most recently used first.
    fn running_app_processes(&self, call: &Call<'_>) -> Result<Option<Vec<Option<RunningAppProcessInfo>>>, Exception> {
        let uid = call.sender_euid as i32;
        if is_isolated(uid) {
            return Err(Exception::security("Isolated process not allowed to call getRunningAppProcesses"));
        }
        let all_users = self.check_permission(Some("android.permission.INTERACT_ACROSS_USERS_FULL"), uid)? == 0;
        // `isGetTasksAllowed`.
        let all_uids = self.check_permission(Some("android.permission.REAL_GET_TASKS"), uid)? == 0;
        let front = self.tasks.front_start();
        let apps = self.apps.lock().unwrap();
        let mut running: Vec<(&i64, &App)> = apps
            .iter()
            .filter(|(_, a)| a.thread.is_some())
            .filter(|(_, a)| all_users || a.uid / PER_USER_RANGE == uid as u32 / PER_USER_RANGE)
            .filter(|(_, a)| all_uids || a.uid == uid as u32)
            .collect();
        running.sort_by_key(|(seq, a)| std::cmp::Reverse((a.last_activity, **seq)));
        let list: Vec<_> = running
            .into_iter()
            .map(|(seq, a)| {
                let mut info = RunningAppProcessInfo {
                    process_name: Some(a.package.clone()),
                    pkg_list: Some(vec![a.package.clone()]),
                    ..Default::default()
                };
                self.fill_in_proc_mem_info(*seq, a, &mut info, front);
                Some(info)
            })
            .collect();
        Ok((!list.is_empty()).then_some(list))
    }

    /// `getMyMemoryState`: the calling process's state.
    fn my_memory_state(&self, call: &Call<'_>) -> Result<RunningAppProcessInfo, Exception> {
        if is_isolated(call.sender_euid as i32) {
            return Err(Exception::security("Isolated process not allowed to call getMyMemoryState"));
        }
        let mut info = RunningAppProcessInfo::default();
        let front = self.tasks.front_start();
        let apps = self.apps.lock().unwrap();
        if let Some((seq, app)) = apps.iter().find(|(_, a)| a.pid == call.sender_pid && a.thread.is_some()) {
            self.fill_in_proc_mem_info(*seq, app, &mut info, front);
        }
        Ok(info)
    }

    /// `getRecordForAppLOSP`: the attached process whose thread `thread`
    /// is: its start, package, uid and pid.
    fn app_of_thread(&self, thread: Option<Binder>) -> Option<(i64, String, i32, i32)> {
        let Some(Binder::Handle(handle)) = thread else {
            return None;
        };
        self.apps
            .lock()
            .unwrap()
            .iter()
            .find(|(_, a)| a.thread.as_ref().is_some_and(|t| t.handle == handle))
            .map(|(seq, a)| (*seq, a.package.clone(), a.uid as i32, a.pid))
    }

    /// Sends the process of `start_seq` the one-way call `code` on its
    /// thread.
    fn call_thread(&self, start_seq: i64, code: u32, data: &Parcel) -> Result<(), String> {
        let apps = self.apps.lock().unwrap();
        let thread = apps.get(&start_seq).and_then(|a| a.thread.as_ref()).ok_or_else(|| format!("no process of start {start_seq}"))?;
        thread.transact(code, data, true).map(|_| ()).map_err(|s| format!("{code}: {s}"))
    }

    /// `UserController.handleIncomingUser`: `USER_CURRENT` as user 0,
    /// another user than the caller's only with `INTERACT_ACROSS_USERS_FULL`
    /// (or, if `allow_non_full`, `INTERACT_ACROSS_USERS`).
    fn incoming_user(&self, uid: i32, user: i32, allow_non_full: bool) -> Result<i32, Exception> {
        const USER_ALL: i32 = -1;
        const USER_CURRENT: i32 = -2;
        let user = if user == USER_CURRENT { 0 } else { user };
        let has = |p: &str| self.check_permission(Some(p), uid).map(|r| r == 0);
        if user == USER_ALL {
            if !has("android.permission.INTERACT_ACROSS_USERS_FULL")? {
                return Err(Exception::security("No access: INTERACT_ACROSS_USERS_FULL"));
            }
        } else if user < 0 {
            return Err(Exception::new(EX_ILLEGAL_ARGUMENT, format!("Invalid user: {user}")));
        } else if user != uid / PER_USER_RANGE as i32
            && !has("android.permission.INTERACT_ACROSS_USERS_FULL")?
            && !(allow_non_full && has("android.permission.INTERACT_ACROSS_USERS")?)
        {
            return Err(Exception::security(format!(
                "Permission Denial: user {uid} is asking for user {user} but does not have INTERACT_ACROSS_USERS_FULL"
            )));
        }
        Ok(user)
    }

    /// Sends the process of `start_seq` the transaction of `items`
    /// (`ClientLifecycleManager.scheduleTransactionItems`).
    fn schedule(&self, start_seq: i64, items: Vec<Box<dyn activity::Item>>) -> Result<(), String> {
        self.call_thread(start_seq, thread::SCHEDULE_TRANSACTION, &activity::transaction(items))
    }

    /// Kills the process of `start_seq` (`ProcessRecord.killLocked`).
    fn kill_process(&self, start_seq: i64) -> Result<(), String> {
        let pid = self.apps.lock().unwrap().get(&start_seq).map(|a| a.pid).ok_or_else(|| format!("no process of start {start_seq}"))?;
        self.call_agent(KILL_PROCESS, "killProcess", |data| data.write_i32(pid)).map(|_| ()).map_err(|e| format!("{e:?}"))
    }

    /// `ActivityManager.checkComponentPermission` of an exported
    /// component nobody owns: root and system hold every permission,
    /// isolated processes none, other uids what PackageManager granted
    /// them (`checkUidPermission`).
    pub(super) fn check_permission(
        &self,
        permission: Option<&str>,
        uid: i32,
    ) -> Result<i32, Exception> {
        const GRANTED: i32 = 0;
        const DENIED: i32 = -1;
        let app_id = uid.rem_euclid(PER_USER_RANGE as i32);
        if app_id == 0 || app_id == 1000 {
            return Ok(GRANTED);
        }
        // `Process.isIsolated`: FIRST_ISOLATED_UID..LAST_ISOLATED_UID.
        // `Process.isIsolated`: isolated and app zygote processes.
        if (90000..=99999).contains(&app_id) {
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

    /// `reportStartInfoViewTimestamps`: the first frame of the caller's
    /// latest start, kept once (`AppStartInfoTracker.addTimestampToStart`).
    fn report_view_timestamps(&self, call: &Call<'_>, draw_start: i64, presented: i64) {
        let mut apps = self.apps.lock().unwrap();
        let Some(app) = apps
            .values_mut()
            .filter(|a| a.pid == call.sender_pid && a.uid == call.sender_euid)
            .max_by_key(|a| a.started)
        else {
            return;
        };
        if app.first_frame.is_none() {
            app.first_frame = Some((draw_start, presented));
            eprintln!(
                "app host: {} presented its first frame {} ms after its start",
                app.package,
                presented / 1_000_000 - app.start_uptime
            );
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
        let Some(request) = app.activity.take() else {
            return Ok(());
        };
        let class = request.class.clone();
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
        let (uid, package, started) = (app.uid, app.package.clone(), app.started);
        drop(apps);
        // `realStartActivityLocked` of the activity on top of a new task.
        self.new_task(start_seq, uid, &package, &class, freeform::Activity::from(&info));
        let task = self.tasks.of_uid(uid).map_or(-(start_seq as i32), |(id, _)| id);
        let result_to = request.result_to;
        let token = self.new_activity(start_seq, uid, &package, info, task, request);
        let commands = self.activities.stack.lock().unwrap().start(token, task, start_seq, result_to);
        if let Some(me) = self.me.upgrade() {
            me.run(commands);
        }
        eprintln!("app host: {package} launch sent {:?} after its start", started.elapsed());
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

    /// `WindowManagerInternal.isUidFocused`.
    pub fn uid_focused(&self, uid: i32) -> bool {
        u32::try_from(uid).is_ok_and(|uid| self.activities.uid_focused(uid))
    }

    /// The image's root.
    fn root(&self) -> Result<&Path, Exception> {
        Ok(&self.configured.get().ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host not configured"))?.root)
    }

    /// The guest's `/data`.
    fn data(&self) -> Result<&Path, Exception> {
        Ok(&self.configured.get().ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host not configured"))?.data)
    }

    /// The app host's connection to SurfaceFlinger, made once.
    fn composer(&self) -> Result<&surface::Composer, Exception> {
        if let Some(composer) = self.composer.get() {
            return Ok(composer);
        }
        let composer = surface::Composer::connect(&self.process).map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        Ok(self.composer.get_or_init(|| composer))
    }

    /// The global configuration `uid`'s process was bound with.
    fn global_config(&self, uid: u32) -> Option<config::Configuration> {
        self.apps.lock().unwrap().values().find(|a| a.uid == uid && a.thread.is_some()).and_then(|a| a.config.clone())
    }

    /// The activity `ApplicationPackageManager.getLaunchIntentForPackage`
    /// launches `package` with: the first of its `MAIN` activities in the
    /// `INFO` category, else in the `LAUNCHER` category.
    fn launch_activity_of(&self, package: &str, calling_uid: i32) -> Result<String, Exception> {
        let (_, resolution) = self.packages()?;
        for category in ["android.intent.category.INFO", "android.intent.category.LAUNCHER"] {
            let intent = crate::package::intent::Intent {
                action: Some("android.intent.action.MAIN".into()),
                categories: Some(vec![category.into()]),
                package: Some(package.into()),
                ..Default::default()
            };
            let found = resolution
                .query_intent_activities(&intent, None, 0, 0, calling_uid)
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("{package}: {e:?}")))?;
            if let Some(crate::package::component_resolver::Info::Activity(activity)) = found.into_iter().next().map(|r| r.info) {
                return activity.info.item.name.ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "an activity without a name"));
            }
        }
        Err(Exception::illegal_argument(format!("{package}: no launch activity")))
    }

    /// A window input channel from the agent, for a window of `frame` on
    /// a display of `display` pixels: its index, and the reply, whose
    /// client socket starts at [`AGENT_CHANNEL_FD`].
    fn open_input_channel(&self, name: &str, display: (i32, i32), input: &window_manager::Input) -> Result<(i32, aim_binder_host::local::Received), Exception> {
        let reply = self.call_agent(OPEN_INPUT_CHANNEL, "openInputChannel", |data| {
            data.write_string16(Some(name));
            [display.0, display.1].iter().for_each(|v| data.write_i32(*v));
            write_input(data, input);
        })?;
        let mut r = reply.reader();
        let index = r.read_exception().ok().and_then(|_| r.read_i32().ok());
        let index = index.ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "openInputChannel reply"))?;
        Ok((index, reply))
    }

    /// The input channel `index` takes the focus, its window in front.
    fn focus_input_channel(&self, index: i32) -> Result<(), Exception> {
        self.call_agent(FOCUS_INPUT_CHANNEL, "focusInputChannel", |data| data.write_i32(index)).map(|_| ())
    }

    /// The window of input channel `index` moved to `input`'s frame and
    /// touchable region.
    fn set_input_frame(&self, index: i32, input: &window_manager::Input) -> Result<(), Exception> {
        self.call_agent(SET_INPUT_CHANNEL_FRAME, "setInputChannelFrame", |data| {
            data.write_i32(index);
            write_input(data, input);
        })
        .map(|_| ())
    }

    /// The agent's call `code` (`name`) of the arguments `write` writes:
    /// its reply, after a status without exception.
    fn call_agent(&self, code: u32, name: &str, write: impl FnOnce(&mut Parcel)) -> Result<aim_binder_host::local::Received, Exception> {
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let agent = self.agent.lock().unwrap();
        let agent = agent.as_ref().ok_or_else(|| state("the app host agent has not attached".into()))?;
        let mut data = Parcel::new();
        data.write_interface_token(AGENT_DESCRIPTOR);
        write(&mut data);
        let reply = agent.transact(code, &data, false).map_err(|s| state(format!("{name}: {s}")))?;
        let status = reply.reader().read_exception();
        match status {
            Ok(Ok(())) => Ok(reply),
            Ok(Err(e)) => Err(e),
            Err(s) => Err(state(format!("{name} reply: {s}"))),
        }
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
        calling_uid: u32,
    ) -> Result<Option<settings_provider::ContentProviderHolder>, Exception> {
        if let Some(name) = name.filter(|n| *n != "settings") {
            return self.published_provider(name, calling_uid);
        }
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

    /// `ActivityManager.checkComponentPermission`: root and system, and the
    /// owner's app, hold it; isolated processes do not; of a component
    /// not exported, nobody else; else the permission, if any.
    pub(super) fn check_component_permission(
        &self,
        permission: Option<&str>,
        uid: i32,
        owner: i32,
        exported: bool,
    ) -> Result<bool, Exception> {
        let app_id = uid.rem_euclid(PER_USER_RANGE as i32);
        if app_id == 0 || app_id == 1000 {
            return Ok(true);
        }
        // `Process.isIsolated`: isolated and app zygote processes.
        if (90000..=99999).contains(&app_id) {
            return Ok(false);
        }
        if owner >= 0 && app_id == owner.rem_euclid(PER_USER_RANGE as i32) {
            return Ok(true);
        }
        if !exported {
            return Ok(false);
        }
        match permission {
            None => Ok(true),
            Some(_) => Ok(self.check_permission(permission, uid)? == 0),
        }
    }

    /// `ContentProviderHelper.checkContentProviderAccess` of `authority` in
    /// `user` for `uid`: `None` when it may use it, else why not. The app
    /// host keeps no URI grants (`UriGrantsManager`).
    pub(super) fn provider_access(
        &self,
        authority: &str,
        uid: i32,
        user: i32,
    ) -> Result<Option<String>, Exception> {
        const MATCH_DISABLED_COMPONENTS: i64 = 0x200;
        const MATCH_DIRECT_BOOT_AWARE: i64 = 0x8_0000;
        const MATCH_DIRECT_BOOT_UNAWARE: i64 = 0x4_0000;
        let flags = STOCK_PM_FLAGS
            | GET_URI_PERMISSION_PATTERNS
            | MATCH_DISABLED_COMPONENTS
            | MATCH_DIRECT_BOOT_AWARE
            | MATCH_DIRECT_BOOT_UNAWARE;
        let Some(cpi) = self.resolve_provider(authority, flags, user)? else {
            return Ok(Some(format!(
                "Failed to find provider {authority} for user {user}; expected to find a valid ContentProvider for this authority"
            )));
        };
        let owner = cpi.info.application_info.uid;
        let exported = cpi.info.exported;
        let check = |permission: Option<&str>| {
            self.check_component_permission(permission, uid, owner, exported)
        };
        if check(cpi.read_permission.as_deref())? || check(cpi.write_permission.as_deref())? {
            return Ok(None);
        }
        // A path permission grants the provider when it names one the
        // caller holds.
        for pp in cpi.path_permissions.iter().flatten() {
            for permission in [&pp.read_permission, &pp.write_permission].into_iter().flatten() {
                if check(Some(permission))? {
                    return Ok(None);
                }
            }
        }
        let name = cpi.info.item.name.clone().unwrap_or_default();
        let suffix = if !exported {
            format!(" that is not exported from UID {owner}")
        } else if cpi.read_permission.as_deref() == Some("android.permission.MANAGE_DOCUMENTS") {
            " requires that you obtain access using ACTION_OPEN_DOCUMENT or related APIs"
                .to_string()
        } else {
            format!(
                " requires {} or {}",
                cpi.read_permission.as_deref().unwrap_or("null"),
                cpi.write_permission.as_deref().unwrap_or("null")
            )
        };
        Ok(Some(format!(
            "Permission Denial: opening provider {name} (uid={uid}){suffix}"
        )))
    }

    /// `resolveContentProvider` of `authority` with `flags`, as the system
    /// asks it.
    pub(super) fn resolve_provider(
        &self,
        authority: &str,
        flags: i64,
        user: i32,
    ) -> Result<Option<package::info::ProviderInfo>, Exception> {
        let (_, resolution) = self.packages()?;
        match resolution.resolve_content_provider(authority, flags, user, 1000) {
            Ok(found) => Ok(found),
            Err(crate::package::resolve::ResolutionError::Original(e)) => Err(e),
            Err(e) => Err(Exception::new(
                EX_UNSUPPORTED_OPERATION,
                format!("resolveContentProvider: {e:?}"),
            )),
        }
    }

    /// `ContentService.getProviderPackageName`.
    pub(super) fn provider_package(
        &self,
        authority: &str,
        user: i32,
    ) -> Result<Option<String>, Exception> {
        Ok(self
            .resolve_provider(authority, 0, user)?
            .and_then(|p| p.info.application_info.item.package_name.clone()))
    }

    /// `AppOpsManager.checkPackage`: whether `package` is `uid`'s.
    pub(super) fn owns_package(&self, uid: i32, package: &str) -> Result<bool, Exception> {
        let (state, _) = self.packages()?;
        Ok(state
            .packages
            .get(package)
            .is_some_and(|ps| ps.app_id == uid.rem_euclid(PER_USER_RANGE as i32)))
    }

    /// The provider an app published under `authority`, for an app of the
    /// same uid: its other processes reach it as the original's
    /// `getContentProviderImpl` gives a running provider. Another app's
    /// provider, or one whose process is not running, is not served yet
    /// (#1450).
    fn published_provider(&self, authority: &str, calling_uid: u32) -> Result<Option<settings_provider::ContentProviderHolder>, Exception> {
        let providers = self.providers.lock().unwrap();
        match providers.get(authority) {
            Some(p) if p.uid == calling_uid => Ok(Some(settings_provider::ContentProviderHolder {
                info: p.info.clone(),
                provider: p.provider.binder(),
                connection: self.process.add_service(Arc::new(activity::Token)),
            })),
            _ => Err(self.unimplemented_named("activity", &format!("getContentProvider of {authority}"))),
        }
    }

    /// `publishContentProviders`: the providers the caller's process
    /// installed, of those its `bindApplication` named.
    fn publish_providers(&self, call: &mut Call<'_>) -> Result<(), Exception> {
        let bad = || Exception::illegal_argument("publishContentProviders");
        let (start_seq, uid) = self
            .apps
            .lock()
            .unwrap()
            .iter()
            .find(|(_, a)| a.pid == call.sender_pid && a.uid == call.sender_euid)
            .map(|(seq, a)| (*seq, a.uid))
            .ok_or_else(|| Exception::security("publishContentProviders from an unknown process"))?;
        call.data.enforce_interface(am::DESCRIPTOR).map_err(|_| bad())?;
        call.data.read_binder().map_err(|_| bad())?;
        let n = call.data.read_i32().map_err(|_| bad())?;
        let (model, resolution) = self.packages()?;
        let query = crate::package::query::Query { state: &model, filter: &resolution.apps_filter, calling_uid: 1000 };
        let ours = self.app_providers(&query, uid)?;
        let mut providers = self.providers.lock().unwrap();
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
                providers.insert(authority.to_string(), Published { start_seq, uid, info: ours.clone(), provider: self.process.strong(handle) });
            }
            eprintln!("app host: {name} published ({})", ours.authority.as_deref().unwrap_or(""));
        }
        Ok(())
    }

    /// `generateApplicationProvidersLocked`: the providers of `uid`'s
    /// default process (user 0's, an installed app's).
    fn app_providers(&self, query: &crate::package::query::Query<'_>, uid: u32) -> Result<Vec<package::info::ProviderInfo>, Exception> {
        let name = self
            .apps
            .lock()
            .unwrap()
            .values()
            .find(|a| a.uid == uid)
            .map(|a| a.package.clone())
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no process of the uid"))?;
        let flags = STOCK_PM_FLAGS | GET_URI_PERMISSION_PATTERNS | MATCH_DIRECT_BOOT_AUTO;
        match query.query_content_providers(Some(&name), uid as i32, flags, None) {
            Ok(Ok(providers)) => Ok(providers),
            Ok(Err(e)) => Err(e),
            Err(crate::package::apps_filter::NotModelled(reason)) => Err(Exception::new(EX_UNSUPPORTED_OPERATION, format!("queryContentProviders: {reason}"))),
        }
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
        let providers = {
            let (model, resolution) = self.packages()?;
            let query = crate::package::query::Query { state: &model, filter: &resolution.apps_filter, calling_uid: 1000 };
            let flags = STOCK_PM_FLAGS | GET_URI_PERMISSION_PATTERNS | MATCH_DIRECT_BOOT_AUTO;
            match query.query_content_providers(Some(&app.package), app.uid as i32, flags, None) {
                Ok(Ok(providers)) => providers,
                Ok(Err(e)) => return Err(e),
                Err(crate::package::apps_filter::NotModelled(reason)) => {
                    return Err(Exception::new(EX_UNSUPPORTED_OPERATION, format!("queryContentProviders: {reason}")));
                }
            }
        };
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
            provider_list: Some(bind::ProviderInfoList(providers)),
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

/// `SystemProperties.getBoolean` of `value`: `default` when unset, empty
/// or neither true nor false.
fn system_boolean(value: Option<&str>, default: bool) -> bool {
    match value {
        Some("1" | "y" | "yes" | "on" | "true") => true,
        Some("0" | "n" | "no" | "off" | "false") => false,
        _ => default,
    }
}

/// The string field `name` of a decoded parcelable, at any depth.
fn field<'v>(value: &'v crate::shadow::Value, name: &str) -> Option<&'v str> {
    let crate::shadow::Value::Fields(fields) = value else { return None };
    fields.iter().find_map(|(n, v)| match v {
        crate::shadow::Value::Str(s) if n == name => Some(s.as_str()),
        crate::shadow::Value::Fields(_) => field(v, name),
        _ => None,
    })
}

/// A window's input for the agent: its frame, its touchable region,
/// whether it watches touches outside it and its app's uid.
fn write_input(data: &mut Parcel, input: &window_manager::Input) {
    for r in [input.frame, input.touchable] {
        [r.left, r.top, r.right, r.bottom].iter().for_each(|v| data.write_i32(*v));
    }
    data.write_bool(input.watch_outside);
    data.write_i32(input.owner as i32);
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

/// `ApplicationInfo.getHiddenApiEnforcementPolicy` of an app of `flags`,
/// `private_flags` and `private_flags_ext`, after `ProcessList` applied
/// `Settings.Global.HIDDEN_API_POLICY` (`setting`): disabled for an app
/// allowed hidden APIs (`isAllowedToUseHiddenApis`: signed with the
/// platform key, or a system app using non-SDK APIs or allowlisted by
/// `SystemConfig`) or by the setting, else the setting's, else enabled.
fn hidden_api_policy(flags: i32, private_flags: i32, private_flags_ext: i32, setting: Option<&str>) -> i32 {
    const FLAG_UPDATED_SYSTEM_APP: i32 = 1 << 7;
    const PRIVATE_FLAG_SIGNED_WITH_PLATFORM_KEY: i32 = 1 << 20;
    const PRIVATE_FLAG_USES_NON_SDK_API: i32 = 1 << 22;
    const PRIVATE_FLAG_EXT_ALLOWLISTED_FOR_HIDDEN_APIS: i32 = 1 << 4;
    let system = flags & (FLAG_SYSTEM | FLAG_UPDATED_SYSTEM_APP) != 0;
    let allowed = private_flags & PRIVATE_FLAG_SIGNED_WITH_PLATFORM_KEY != 0
        || system
            && (private_flags & PRIVATE_FLAG_USES_NON_SDK_API != 0
                || private_flags_ext & PRIVATE_FLAG_EXT_ALLOWLISTED_FOR_HIDDEN_APIS != 0);
    // `HiddenApiSettings.getValidEnforcementPolicy`: DEFAULT (-1) to ENABLED.
    let setting = setting.and_then(|v| v.parse::<i32>().ok()).filter(|p| (-1..=HIDDEN_API_ENFORCEMENT_ENABLED).contains(p));
    match setting {
        _ if allowed => HIDDEN_API_ENFORCEMENT_DISABLED,
        Some(policy) if policy >= 0 => policy,
        _ => HIDDEN_API_ENFORCEMENT_ENABLED,
    }
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

/// `Process.isIsolated`: an isolated or app zygote process.
fn is_isolated(uid: i32) -> bool {
    (90_000..=99_999).contains(&uid.rem_euclid(PER_USER_RANGE as i32))
}

/// `RunningAppProcessInfo.FLAG_HAS_ACTIVITIES`, `IMPORTANCE_FOREGROUND`;
/// `ActivityManager.PROCESS_STATE_IMPORTANT_FOREGROUND`.
const FLAG_HAS_ACTIVITIES: i32 = 1 << 2;
const IMPORTANCE_FOREGROUND: i32 = 100;
const PROCESS_STATE_IMPORTANT_FOREGROUND: i32 = 6;

/// `ActivityManager.RunningAppProcessInfo`, as its `writeToParcel` writes
/// it; what the app host does not track (trim level, LRU position, the
/// reason for its importance, package dependencies) is its default.
struct RunningAppProcessInfo {
    process_name: Option<String>,
    pid: i32,
    uid: i32,
    pkg_list: Option<Vec<String>>,
    flags: i32,
    importance: i32,
    process_state: i32,
    is_focused: bool,
    last_activity_time: i64,
}

/// `RunningAppProcessInfo()`.
impl Default for RunningAppProcessInfo {
    fn default() -> Self {
        RunningAppProcessInfo {
            process_name: None,
            pid: 0,
            uid: 0,
            pkg_list: None,
            flags: 0,
            importance: IMPORTANCE_FOREGROUND,
            process_state: PROCESS_STATE_IMPORTANT_FOREGROUND,
            is_focused: false,
            last_activity_time: 0,
        }
    }
}

impl WriteParcelable for RunningAppProcessInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_string16(self.process_name.as_deref());
        p.write_i32(self.pid);
        p.write_i32(self.uid);
        match &self.pkg_list {
            Some(list) => {
                p.write_i32(list.len() as i32);
                for name in list {
                    p.write_string16(Some(name));
                }
            }
            None => p.write_i32(-1),
        }
        p.write_i32(-1); // no package dependencies
        p.write_i32(self.flags);
        p.write_i32(0); // last trim level
        p.write_i32(self.importance);
        p.write_i32(0); // lru
        p.write_i32(0); // importance reason: unknown
        p.write_i32(0); // its pid
        p.write_string16(None); // its component (`ComponentName.writeToParcel(null)`)
        p.write_i32(0); // its importance
        p.write_i32(self.process_state);
        p.write_i32(self.is_focused as i32);
        p.write_i64(self.last_activity_time);
    }
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
                    // The services waiting for the process
                    // (`ActiveServices.attachApplicationLocked`).
                    self.0.services_attached(args.start_seq);
                    let mut p = Parcel::new();
                    am::write_attach_application_reply(&mut p);
                    p
                }))
            }
            am::START_SERVICE => {
                let a = am::StartService::<kept::RawIntent>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let Some(intent) = a.service else {
                    return reply(Err(Exception::illegal_argument("service intent is null")));
                };
                reply(self.0.start_service(a.caller, call.sender_euid as i32, intent, a.resolved_type, a.user_id).map(|name| {
                    let mut p = Parcel::new();
                    am::write_start_service_reply(&mut p, name.as_ref().map(|(p, c)| activity_start::Component(p, c)).as_ref());
                    p
                }))
            }
            am::STOP_SERVICE => {
                let a = am::StopService::<kept::RawIntent>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let Some(intent) = a.service else {
                    return reply(Err(Exception::illegal_argument("service intent is null")));
                };
                reply(self.0.stop_service(call.sender_euid as i32, intent, a.resolved_type, a.user_id).map(|r| {
                    let mut p = Parcel::new();
                    am::write_stop_service_reply(&mut p, r);
                    p
                }))
            }
            am::STOP_SERVICE_TOKEN => {
                let a = am::StopServiceToken::<services::Name>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let stopped = self.0.stop_service_token(a.class_name, a.token, a.start_id);
                let mut p = Parcel::new();
                am::write_stop_service_token_reply(&mut p, stopped);
                Ok(p)
            }
            am::BIND_SERVICE | am::BIND_SERVICE_INSTANCE => {
                let a = if call.code == am::BIND_SERVICE {
                    let a = am::BindService::<kept::RawIntent>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                    am::BindServiceInstance { caller: a.caller, token: a.token, service: a.service, resolved_type: a.resolved_type, connection: a.connection, flags: a.flags, instance_name: None, calling_package: a.calling_package, user_id: a.user_id }
                } else {
                    am::BindServiceInstance::<kept::RawIntent>::read(&mut call.data).map_err(|_| BAD_VALUE)?
                };
                let Some(intent) = a.service else {
                    return reply(Err(Exception::illegal_argument("service intent is null")));
                };
                reply(
                    self.0
                        .bind_service(a.caller, call.sender_euid as i32, a.token, intent, a.resolved_type, a.connection, a.flags, a.instance_name, a.user_id)
                        .map(|r| {
                            let mut p = Parcel::new();
                            am::write_bind_service_instance_reply(&mut p, r);
                            p
                        }),
                )
            }
            am::UNBIND_SERVICE => {
                let a = am::UnbindService::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let unbound = self.0.unbind_service(a.connection);
                let mut p = Parcel::new();
                am::write_unbind_service_reply(&mut p, unbound);
                Ok(p)
            }
            am::PUBLISH_SERVICE => {
                let a = am::PublishService::<kept::RawIntent>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.0.publish_service(a.token, a.intent, a.service);
                let mut p = Parcel::new();
                am::write_publish_service_reply(&mut p);
                Ok(p)
            }
            am::UNBIND_FINISHED => {
                let a = am::UnbindFinished::<kept::RawIntent>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.0.unbind_finished(a.token, a.service);
                let mut p = Parcel::new();
                am::write_unbind_finished_reply(&mut p);
                Ok(p)
            }
            am::GET_INTENT_SENDER | am::GET_INTENT_SENDER_WITH_FEATURE => {
                let req = if call.code == am::GET_INTENT_SENDER {
                    let a = am::GetIntentSender::<kept::RawIntent, kept::RawBundle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                    pending_intents::Request { kind: a.r#type, package: a.package_name, feature: None, token: a.token, result_who: a.result_who, request_code: a.request_code, intents: a.intents, resolved_types: a.resolved_types, flags: a.flags, options: a.options, user: a.user_id }
                } else {
                    let a = am::GetIntentSenderWithFeature::<kept::RawIntent, kept::RawBundle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                    pending_intents::Request { kind: a.r#type, package: a.package_name, feature: a.feature_id, token: a.token, result_who: a.result_who, request_code: a.request_code, intents: a.intents, resolved_types: a.resolved_types, flags: a.flags, options: a.options, user: a.user_id }
                };
                reply(self.0.get_intent_sender(call.sender_euid as i32, req).map(|sender| {
                    let mut p = Parcel::new();
                    am::write_get_intent_sender_with_feature_reply(&mut p, sender);
                    p
                }))
            }
            am::CANCEL_INTENT_SENDER => {
                let a = am::CancelIntentSender::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.cancel_intent_sender(call.sender_euid as i32, a.sender).map(|()| {
                    let mut p = Parcel::new();
                    am::write_cancel_intent_sender_reply(&mut p);
                    p
                }))
            }
            am::GET_INFO_FOR_INTENT_SENDER => {
                let a = am::GetInfoForIntentSender::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                am::write_get_info_for_intent_sender_reply(&mut p, Some(&self.0.intent_sender_info(a.sender)));
                Ok(p)
            }
            am::REGISTER_INTENT_SENDER_CANCEL_LISTENER_EX => {
                let a = am::RegisterIntentSenderCancelListenerEx::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                am::write_register_intent_sender_cancel_listener_ex_reply(&mut p, self.0.listen_for_cancel(a.sender, a.receiver));
                Ok(p)
            }
            am::UNREGISTER_INTENT_SENDER_CANCEL_LISTENER => {
                let a = am::UnregisterIntentSenderCancelListener::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.0.unlisten_for_cancel(a.sender, a.receiver);
                let mut p = Parcel::new();
                am::write_unregister_intent_sender_cancel_listener_reply(&mut p);
                Ok(p)
            }
            am::GET_INTENT_FOR_INTENT_SENDER => {
                let a = am::GetIntentForIntentSender::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.intent_for_intent_sender(call.sender_euid as i32, a.sender).map(|intent| {
                    let mut p = Parcel::new();
                    am::write_get_intent_for_intent_sender_reply(&mut p, intent.as_ref());
                    p
                }))
            }
            am::SEND_INTENT_SENDER => {
                let a = am::SendIntentSender::<kept::RawIntent, kept::RawBundle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .send_intent_sender(call.sender_euid as i32, a.caller, a.target, a.code, a.intent, a.resolved_type, a.finished_receiver, a.required_permission, a.options)
                        .map(|r| {
                            let mut p = Parcel::new();
                            am::write_send_intent_sender_reply(&mut p, r);
                            p
                        }),
                )
            }
            // One-way.
            am::SERVICE_DONE_EXECUTING => {
                let a = am::ServiceDoneExecuting::<kept::RawIntent>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.0.service_done_executing(a.token, a.r#type, a.start_id, a.res) {
                    eprintln!("app host: serviceDoneExecuting: {}", e.message);
                }
                Ok(Parcel::new())
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
                reply(self.0.content_provider(args.name.as_deref(), call.sender_euid).map(|holder| {
                    let mut p = Parcel::new();
                    am::write_get_content_provider_reply(&mut p, holder.as_ref());
                    p
                }))
            }
            am::PUBLISH_CONTENT_PROVIDERS => reply(self.0.publish_providers(call).map(|()| {
                let mut p = Parcel::new();
                p.write_no_exception();
                p
            })),
            am::REPORT_START_INFO_VIEW_TIMESTAMPS => {
                let args = am::ReportStartInfoViewTimestamps::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.0.report_view_timestamps(
                    call,
                    args.render_thread_draw_start_time_ns,
                    args.frame_presented_time_ns,
                );
                Ok(Parcel::new())
            }
            // The render thread's priority is the process's; the app host
            // does not boost it.
            am::SET_RENDER_THREAD => {
                am::SetRenderThread::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                am::write_set_render_thread_reply(&mut p);
                Ok(p)
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
            am::GET_RUNNING_APP_PROCESSES => {
                am::GetRunningAppProcesses::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.running_app_processes(call).map(|list| {
                    let mut p = Parcel::new();
                    am::write_get_running_app_processes_reply(&mut p, list.as_deref());
                    p
                }))
            }
            am::GET_MY_MEMORY_STATE => {
                am::GetMyMemoryState::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.my_memory_state(call).map(|info| {
                    let mut p = Parcel::new();
                    am::write_get_my_memory_state_reply(&mut p, &am::GetMyMemoryStateReply { out_info: Some(info) });
                    p
                }))
            }
            am::REGISTER_RECEIVER => {
                let a = am::RegisterReceiver::<broadcasts::ParcelFilter>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let args = broadcasts::Registration {
                    caller: a.caller,
                    caller_package: a.caller_package,
                    receiver: a.receiver,
                    filter: a.filter,
                    permission: a.required_permission,
                    user: a.user_id,
                    flags: a.flags,
                };
                reply(self.0.broadcasts.register(&self.0, call, args).map(|sticky| {
                    let mut p = Parcel::new();
                    am::write_register_receiver_reply(&mut p, sticky.as_ref());
                    p
                }))
            }
            am::REGISTER_RECEIVER_WITH_FEATURE => {
                let a = am::RegisterReceiverWithFeature::<broadcasts::ParcelFilter>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let args = broadcasts::Registration {
                    caller: a.caller,
                    caller_package: a.caller_package,
                    receiver: a.receiver,
                    filter: a.filter,
                    permission: a.required_permission,
                    user: a.user_id,
                    flags: a.flags,
                };
                reply(self.0.broadcasts.register(&self.0, call, args).map(|sticky| {
                    let mut p = Parcel::new();
                    am::write_register_receiver_with_feature_reply(&mut p, sticky.as_ref());
                    p
                }))
            }
            am::UNREGISTER_RECEIVER => {
                let a = am::UnregisterReceiver::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.broadcasts.unregister(&self.0, a.receiver).map(|()| {
                    let mut p = Parcel::new();
                    am::write_unregister_receiver_reply(&mut p);
                    p
                }))
            }
            am::BROADCAST_INTENT_WITH_FEATURE => {
                let a = am::BroadcastIntentWithFeature::<kept::RawIntent, kept::RawBundle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.broadcasts.broadcast(&self.0, call, a).map(|result| {
                    let mut p = Parcel::new();
                    am::write_broadcast_intent_with_feature_reply(&mut p, result);
                    p
                }))
            }
            am::UNBROADCAST_INTENT => {
                let a = am::UnbroadcastIntent::<kept::RawIntent>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.broadcasts.unbroadcast(&self.0, call, a.intent, a.user_id).map(|()| {
                    let mut p = Parcel::new();
                    am::write_unbroadcast_intent_reply(&mut p);
                    p
                }))
            }
            // One-way.
            am::FINISH_RECEIVER => {
                let a = am::FinishReceiver::<kept::RawBundle>::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.0.broadcasts.finish(&self.0, a) {
                    eprintln!("app host: finishReceiver: {e:?}");
                }
                Ok(Parcel::new())
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
        if call.code == ATTACH_AGENT {
            // The agent runs as the system uid.
            if call.sender_euid != 1000 {
                return reply(Err(Exception::security("attachAgent is the agent's")));
            }
            call.data.enforce_interface(CONTROL_DESCRIPTOR).map_err(|_| BAD_VALUE)?;
            let Some(Binder::Handle(agent)) = call.data.read_binder().map_err(|_| BAD_VALUE)? else {
                return Err(BAD_VALUE);
            };
            *self.0.agent.lock().unwrap() = Some(self.0.process.strong(agent));
            eprintln!("app host: agent attached (pid {})", call.sender_pid);
            // The guest's properties are set by now: what starts read is
            // loaded while the boot goes on, as PackageManager scans at its
            // start, and not by the first start.
            let host = self.0.clone();
            std::thread::spawn(move || {
                if let Err(e) = host.inputs().and_then(|_| host.packages()) {
                    eprintln!("app host: loading its inputs: {e:?}");
                }
                // audioserver starts with its HAL, which asks the Mac for
                // its devices: apart from the storage's start.
                let audio = host.clone();
                std::thread::spawn(move || {
                    if let Err(e) = audio.start_audio() {
                        eprintln!("app host: audio: {e}");
                    }
                });
                if let Err(e) = host.start_storage() {
                    eprintln!("app host: external storage: {e}");
                }
            });
            let mut p = Parcel::new();
            p.write_no_exception();
            return Ok(p);
        }
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
        // A component (`ComponentName.unflattenFromString`), or a package,
        // whose launch activity it is.
        let (package, class) = match component.split_once('/') {
            Some((package, class)) if class.starts_with('.') => (package, format!("{package}{class}")),
            Some((package, class)) => (package, class.to_string()),
            None => match self.0.launch_activity_of(&component, call.sender_euid as i32) {
                Ok(class) => (component.as_str(), class),
                Err(e) => return reply(Err(e)),
            },
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
    fn system_booleans_default_when_unset_or_empty() {
        assert!(system_boolean(None, true) && system_boolean(Some(""), true) && system_boolean(Some("on"), false));
        assert!(!system_boolean(Some("false"), true) && !system_boolean(Some("maybe"), false));
    }

    #[test]
    fn hidden_api_policy_by_signer_and_allowlist() {
        // Signed with the platform key.
        assert_eq!(hidden_api_policy(FLAG_SYSTEM, 1 << 20, 0, None), 0);
        // A user app; one the setting relaxes.
        assert_eq!(hidden_api_policy(0, 0, 0, None), 2);
        assert_eq!(hidden_api_policy(0, 0, 0, Some("1")), 1);
        // A system app: allowlisted, using non-SDK APIs, or neither.
        assert_eq!(hidden_api_policy(FLAG_SYSTEM, 0, 1 << 4, None), 0);
        assert_eq!(hidden_api_policy(FLAG_SYSTEM, 1 << 22, 0, None), 0);
        assert_eq!(hidden_api_policy(FLAG_SYSTEM, 0, 0, Some("bad")), 2);
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
