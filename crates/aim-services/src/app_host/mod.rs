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
mod account_session;
pub mod accounts_db;
pub mod activity;
pub mod activity_stack;
pub mod activity_start;
pub mod alarm_service;
pub mod apex_manager;
mod app_op_infos;
mod app_standby;
pub mod app_widget;
pub mod appops_restrictions;
pub mod appops_sampling;
pub mod appops_service;
pub mod audio_focus;
pub mod audio_permissions;
pub mod audio_policy;
pub mod audio_service;
pub mod auth_service;
pub mod battery_service;
pub mod battery_stats;
pub mod bind;
pub mod bluetooth_manager;
mod broadcast_response;
pub mod broadcasts;
mod cache_quota;
pub mod companion_device;
pub mod compat;
pub mod config;
pub mod connectivity_service;
pub mod content_service;
pub mod cross_profile_apps;
pub mod cursor;
pub mod device_config;
pub mod device_identifiers;
pub mod device_policy;
pub mod display;
pub mod display_service;
pub mod domain_verification;
pub mod enhanced_confirmation;
pub mod exit_info;
pub mod fgs_types;
pub mod freeform;
pub mod game_service;
pub mod generations;
pub mod grammatical_inflection;
pub mod hint_manager;
pub mod input_manager;
pub mod input_method;
pub mod input_shell;
pub mod job_content;
pub mod job_info;
pub mod job_scheduler;
pub mod job_store;
pub mod kept;
pub mod legacy_permission;
pub mod locale_manager;
pub mod lock_settings;
mod loudness;
mod media_route_providers;
pub mod media_router;
mod media_router2;
pub mod media_session;
mod media_session_record;
pub mod memory_info;
pub mod netpolicy_service;
pub mod netstats;
pub mod network;
pub mod network_management;
mod network_native;
pub mod notification_channels;
pub mod notification_service;
pub mod overlay_service;
mod package_checksums;
pub mod package_installer;
pub mod package_service;
mod package_writes;
pub mod packages;
pub mod pending_intents;
pub mod permission_checker;
pub mod permission_controller;
pub mod permission_manager;
pub mod platform_compat;
pub mod playback;
pub mod power_service;
pub mod providers;
mod recovery_certificates;
pub mod restrictions_service;
pub mod role_service;
pub mod safety_center;
pub mod safety_center_config;
pub mod safety_center_data;
pub mod sensor_privacy;
pub mod sensor_service;
pub mod services;
pub mod settings;
pub mod settings_provider;
pub mod settings_service;
pub mod settings_validators;
pub mod shortcut_service;
pub mod stats_manager;
pub mod status_bar;
pub mod storage;
pub mod storage_stats;
mod strict_mode;
pub mod surface;
pub mod system_update;
pub mod task_info;
pub mod tasks;
pub mod telephony_registry;
pub mod trust_service;
pub mod uid_observers;
pub mod uri_grants;
mod usage_database;
mod usage_events;
pub mod usage_stats;
pub mod usb_service;
pub mod user_service;
pub mod volume;
pub mod vpn_manager;
pub mod webview_update;
mod webview_zygote;
pub mod wifi_config;
pub mod wifi_service;
pub mod window_layout;
pub mod window_manager;
pub mod zen_config;
pub mod zygote;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use aim_binder_host::local::{Call, LocalProcess, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_ILLEGAL_ARGUMENT, EX_ILLEGAL_STATE, EX_NULL_POINTER,
    EX_UNSUPPORTED_OPERATION, Exception, Parcel, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::android_app_iactivitymanager as am;
use aim_service_aidl::android_app_iapplicationthread as thread;
use aim_service_aidl::{ParcelValue, WriteParcelable};

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
/// Those of them an isolated process gets.
const ISOLATED_COMMON_SERVICES: &[&str] = &["package", "permissionmgr"];
/// `PackageManager.GET_SHARED_LIBRARY_FILES | MATCH_DEBUG_TRIAGED_MISSING`,
/// `ActivityManagerService.STOCK_PM_FLAGS`.
const STOCK_PM_FLAGS: i64 = 0x400 | 0x1000_0000;
/// `ActivityManagerService.PERSISTENT_MASK`: `ApplicationInfo.FLAG_SYSTEM`
/// and `FLAG_PERSISTENT`.
const PERSISTENT_MASK: i32 = 0x1 | 0x8;
/// `PackageManager.GET_URI_PERMISSION_PATTERNS`, `MATCH_DIRECT_BOOT_AUTO`.
const GET_URI_PERMISSION_PATTERNS: i64 = 0x800;
const MATCH_DIRECT_BOOT_AUTO: i64 = 0x1000_0000;
/// `ActivityManager.PROCESS_STATE_TOP`, `PROCESS_STATE_NONEXISTENT`.
const PROCESS_STATE_TOP: i32 = 2;
const PROCESS_STATE_NONEXISTENT: i32 = 20;
/// `AppOpsManager.OP_GET_USAGE_STATS`.
const OP_GET_USAGE_STATS: i32 = 43;
const PACKAGE_USAGE_STATS: &str = "android.permission.PACKAGE_USAGE_STATS";
/// `Build.VERSION_CODES.P`: from it, apps get `Build.UNKNOWN` as the serial.
const P: i32 = 28;

/// Reads one of the device's properties.
pub type Properties = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;
/// Sets a guest property as the system (`SystemProperties.set`).
pub type SetProperty = Box<dyn Fn(&str, &str) + Send + Sync>;

/// Where the app host finds its inputs, set once guest-init knows them.
struct Configured {
    root: PathBuf,
    data: PathBuf,
    bpf: PathBuf,
    properties: Properties,
    set_property: SetProperty,
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
    /// `ro.sf.lcd_density`, the processes' `DisplayMetrics.DENSITY_DEVICE`.
    device_density: i32,
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
    /// The package it was started for, and its process name.
    package: String,
    process: String,
    uid: u32,
    /// Its package's uid (`ProcessRecord.info.uid`): its own uid, or an
    /// isolated process's owner's.
    package_uid: u32,
    /// Whether it was started isolated (`ProcessRecord.isolated`), and the
    /// entry point it runs instead of an application, with its arguments
    /// (`getIsolatedEntryPoint`).
    isolated: bool,
    entry_point: Option<(String, Vec<String>)>,
    /// The application it is bound with when not its package's
    /// (`ProcessRecord.info`): an external service's, the defining
    /// package's under the client's name and uid.
    application_info: Option<Arc<package::info::ApplicationInfo>>,
    pid: i32,
    started: Instant,
    /// `SystemClock.elapsedRealtime` and `uptimeMillis` at the start.
    start_elapsed: i64,
    start_uptime: i64,
    thread: Option<Strong>,
    /// The activities the process starts for, launched once its
    /// application is created.
    activities: Vec<activity_start::StartRequest>,
    /// The global configuration it was bound with.
    config: Option<config::Configuration>,
    /// The providers it was handed (`ProcessRecord.mProviders`).
    providers: Vec<package::info::ProviderInfo>,
    /// Whether it crashed too often to keep its services
    /// (`ProcessRecord.setRemoved`, without `allowRestart`).
    removed: bool,
    /// Its start's first frame (`ApplicationStartInfo
    /// .TIMESTAMP_INITIAL_RENDERTHREAD_FRAME`,
    /// `TIMESTAMP_SURFACEFLINGER_COMPOSITION_COMPLETE`), uptime nanoseconds.
    first_frame: Option<(i64, i64)>,
    /// When an activity of it was last resumed, uptime milliseconds
    /// (`ProcessRecord.getLastActivityTime`).
    last_activity: i64,
    /// `ProcessRecord.isPersistent`: user 0's process of a persistent
    /// system app, named as the app.
    persistent: bool,
    /// The packages it loaded code of (`ProcessRecord.getPkgDeps`): a
    /// change of one kills it.
    package_deps: Vec<String>,
    /// Its last `Debug.MemoryInfo` and when it was taken, uptime
    /// milliseconds (`ProcessProfileRecord.mLastMemInfo`,
    /// `mLastMemInfoTime`).
    last_mem_info: Option<(i64, memory_info::ProcessMemoryInfo)>,
}

/// `getPackageProcessState` over the processes, each its package (its
/// package list), its package dependencies and the state set on it: the
/// lowest state of one whose package list or dependencies hold `package`.
fn package_process_state<'a>(
    processes: impl Iterator<Item = (&'a str, &'a [String], i32)>,
    package: Option<&str>,
) -> i32 {
    let Some(package) = package else {
        return PROCESS_STATE_NONEXISTENT;
    };
    processes
        .filter(|(p, deps, _)| *p == package || deps.iter().any(|d| d == package))
        .map(|(_, _, state)| state)
        .fold(PROCESS_STATE_NONEXISTENT, i32::min)
}

/// `Process.SYSTEM_UID`.
const SYSTEM_UID: u32 = 1000;
/// The system process (`ActivityManagerService.setSystemProcess`): the
/// `android` package's process.
const SYSTEM_PROCESS: &str = "system";
const SYSTEM_PACKAGE: &str = "android";
/// `UserHandle.USER_ALL`.
const USER_ALL: i32 = -1;
const INTERACT_ACROSS_USERS_FULL: &str = "android.permission.INTERACT_ACROSS_USERS_FULL";
/// `Settings.AUTHORITY`, the provider the app host serves itself.
const SETTINGS_AUTHORITY: &str = "settings";
/// `Process.FIRST_ISOLATED_UID`, `LAST_ISOLATED_UID`.
const FIRST_ISOLATED_UID: u32 = 99_000;
const LAST_ISOLATED_UID: u32 = 99_999;

/// `ProcessList.IsolatedUidRange` of the isolated uids of processes not
/// spawned from an application zygote: the uids in use, and the app id to
/// try next.
struct IsolatedUids {
    used: HashSet<u32>,
    next: u32,
}

impl IsolatedUids {
    /// `allocateIsolatedUidLocked`: the next unused uid of `user`.
    fn allocate(&mut self, user: u32) -> Option<u32> {
        for _ in FIRST_ISOLATED_UID..=LAST_ISOLATED_UID {
            if !(FIRST_ISOLATED_UID..=LAST_ISOLATED_UID).contains(&self.next) {
                self.next = FIRST_ISOLATED_UID;
            }
            let uid = user * PER_USER_RANGE + self.next;
            self.next += 1;
            if self.used.insert(uid) {
                return Some(uid);
            }
        }
        None
    }
}

#[derive(Default)]
struct AppErrors {
    crash_times: HashMap<(String, u32), Instant>,
    crash_counts: HashMap<(String, u32), (Instant, u32)>,
    /// `mBadProcesses`: processes that crashed too often, which a start
    /// from the background does not start again.
    bad: HashSet<(String, u32)>,
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
    /// The package model's state, replaced as PackageManager's writes
    /// change it.
    model: Mutex<Option<Arc<crate::package::model::State>>>,
    /// Held by each change of the package model from reading it to
    /// storing the changed model, so that no change is lost to another.
    model_writer: Mutex<()>,
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
    providers: providers::Providers,
    uri_grants: uri_grants::UriGrants,
    /// `permissionmgr`, which the app host's own permission checks ask.
    permission_manager: OnceLock<Arc<permission_manager::PermissionManager>>,
    /// Processes' exit history (`AppExitInfoTracker`), loaded at its first use.
    exit_infos: OnceLock<Arc<exit_info::Tracker>>,
    uid_observers: uid_observers::UidObservers,
    /// The image's tzdata, read once.
    tzdata: OnceLock<Vec<u8>>,
    /// Processes' last crash and recent crash count, by process name and
    /// uid (`AppErrors.mProcessCrashTimes`, `mProcessCrashCounts`).
    app_errors: Mutex<AppErrors>,
    /// `ProcessList.mGlobalIsolatedUids`.
    isolated_uids: Mutex<IsolatedUids>,
    app_standby: app_standby::AppStandby,
    broadcast_responses: broadcast_response::BroadcastResponses,
    webview_zygote: webview_zygote::WebViewZygote,
    storage: storage::Volumes,
    text_input: input_method::TextInput,
    content_service: OnceLock<Arc<content_service::ContentService>>,
    role_service: OnceLock<Arc<role_service::RoleManager>>,
    /// The window contexts attached, by their client token's handle.
    window_contexts: Arc<Mutex<HashMap<u32, window_manager::WindowContext>>>,
    notification_service: OnceLock<Arc<notification_service::NotificationManager>>,
    power_service: OnceLock<Arc<power_service::PowerManager>>,
    alarm_service: OnceLock<Arc<alarm_service::AlarmManager>>,
    battery_service: OnceLock<Arc<battery_service::BatteryService>>,
    battery_stats: OnceLock<Arc<battery_stats::BatteryStats>>,
    /// `mStrictModeCallbacks`.
    strict_mode_callbacks: strict_mode::Callbacks,
    /// `mStrictModeBalCallbacks`.
    bal_strict_mode_callbacks: strict_mode::BalCallbacks,
    connectivity_service: OnceLock<Arc<connectivity_service::ConnectivityService>>,
    job_scheduler: OnceLock<Arc<job_scheduler::JobScheduler>>,
    hint_manager: OnceLock<Arc<hint_manager::HintManager>>,
    stats: OnceLock<Arc<stats_manager::Stats>>,
    bluetooth: OnceLock<Arc<bluetooth_manager::BluetoothManager>>,
    /// What DevicePolicyManagerService persisted, read once.
    device_policies: OnceLock<device_policy::Policies>,
    services: services::Services,
    pending_intents: pending_intents::PendingIntents,
    broadcasts: broadcasts::Broadcasts,
    /// User 0's restrictions.
    user_restrictions: OnceLock<std::collections::BTreeSet<String>>,
    /// Whether the system user's first boot records were made
    /// (`user_service::ensure_system_user`).
    system_user: OnceLock<Result<(), String>>,
    /// What SensorPrivacyService persisted, read once.
    sensor_privacy: OnceLock<sensor_privacy::Persisted>,
    /// audioserver, once it registered, and the stream volumes.
    audio_policy: OnceLock<audio_policy::AudioPolicy>,
    volumes: Mutex<Option<volume::Volumes>>,
    /// The players and recorders apps made.
    playback: Mutex<playback::Playback>,
    /// The audio focus stack, locked before the players.
    focus: Mutex<audio_focus::Focus<Strong, (Strong, u64)>>,
    /// AppOps' started ops, their watchers and the async-noted ops.
    app_ops: appops_service::State,
    /// UsageStatsService's current intervals of user 0, made at the first
    /// event or query.
    usage: Mutex<Option<usage_events::UserStats>>,
    /// The attribution sources whose data delivery started
    /// (`PermissionCheckerService.sRunningAttributionSources`).
    running_attributions: Mutex<permission_checker::Running>,
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
            model: Mutex::new(None),
            model_writer: Mutex::new(()),
            resolver: Default::default(),
            activities: Default::default(),
            display: OnceLock::new(),
            settings_provider: OnceLock::new(),
            windows: Default::default(),
            composer: OnceLock::new(),
            agent: Mutex::new(None),
            tasks: Default::default(),
            pending_launches: Mutex::new(HashMap::new()),
            providers: Default::default(),
            uri_grants: Default::default(),
            permission_manager: OnceLock::new(),
            exit_infos: OnceLock::new(),
            uid_observers: Default::default(),
            tzdata: OnceLock::new(),
            app_errors: Default::default(),
            isolated_uids: Mutex::new(IsolatedUids {
                used: HashSet::new(),
                next: FIRST_ISOLATED_UID,
            }),
            app_standby: Default::default(),
            broadcast_responses: Default::default(),
            webview_zygote: Default::default(),
            storage: Default::default(),
            text_input: Default::default(),
            content_service: OnceLock::new(),
            role_service: OnceLock::new(),
            window_contexts: Arc::new(Mutex::new(HashMap::new())),
            notification_service: OnceLock::new(),
            power_service: OnceLock::new(),
            alarm_service: OnceLock::new(),
            battery_service: OnceLock::new(),
            battery_stats: OnceLock::new(),
            strict_mode_callbacks: Default::default(),
            bal_strict_mode_callbacks: Default::default(),
            connectivity_service: OnceLock::new(),
            job_scheduler: OnceLock::new(),
            hint_manager: OnceLock::new(),
            stats: OnceLock::new(),
            bluetooth: OnceLock::new(),
            device_policies: OnceLock::new(),
            services: Default::default(),
            pending_intents: Default::default(),
            broadcasts: Default::default(),
            user_restrictions: OnceLock::new(),
            system_user: OnceLock::new(),
            sensor_privacy: OnceLock::new(),
            audio_policy: OnceLock::new(),
            volumes: Mutex::new(None),
            playback: Mutex::default(),
            focus: Mutex::default(),
            app_ops: Default::default(),
            usage: Mutex::new(None),
            running_attributions: Mutex::default(),
        })
    }

    /// The binders of the services published with it, which
    /// `bindApplication` hands apps.
    pub fn set_published(&self, services: &[(String, Binder)]) {
        *self.published.lock().unwrap() = services.to_vec();
    }

    /// The image below `root`, the guest's `/data` at `data` and its bpffs
    /// at `bpf`, the device's properties (read once the first process
    /// starts, when init has set them), and how the zygote relay is
    /// reached.
    pub fn configure(
        &self,
        root: &Path,
        data: &Path,
        bpf: &Path,
        properties: Properties,
        set_property: SetProperty,
        connect: Connect,
    ) -> Result<(), String> {
        self.configured
            .set(Configured {
                root: root.to_path_buf(),
                data: data.to_path_buf(),
                bpf: bpf.to_path_buf(),
                properties,
                set_property,
                connect,
            })
            .map_err(|_| "app host already configured".to_string())
    }

    /// The services it publishes, by binder name.
    pub fn services(self: &Arc<Self>) -> Vec<(&'static str, Arc<dyn Service>)> {
        let settings = Arc::new(settings_provider::SettingsProvider::new(self.clone()));
        let _ = self.settings_provider.set((
            self.process.add_service(settings.clone()),
            self.process.add_service(Arc::new(activity::Token)),
        ));
        let permission_manager = self
            .permission_manager
            .get_or_init(|| Arc::new(permission_manager::PermissionManager::new(self.clone())))
            .clone();
        // Published together, as `StatsCompanion.Lifecycle` publishes them.
        let stats = self
            .stats
            .get_or_init(|| stats_manager::Stats::new(self.clone()))
            .clone();
        let bluetooth = self
            .bluetooth
            .get_or_init(|| {
                bluetooth_manager::BluetoothManager::new(self.clone(), settings.clone())
            })
            .clone();
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
                "window",
                Arc::new(window_manager::WindowManager(self.clone())),
            ),
            (
                "user",
                Arc::new(user_service::UserManager::new(self.clone())),
            ),
            (
                "appops",
                Arc::new(appops_service::AppOps::new(
                    self.clone(),
                    permission_manager.clone(),
                )),
            ),
            (
                "trust",
                Arc::new(trust_service::TrustManager::new(self.clone())),
            ),
            (
                "auth",
                Arc::new(auth_service::AuthService::new(self.clone())),
            ),
            (
                "permission",
                Arc::new(permission_controller::PermissionController(self.clone())),
            ),
            ("permissionmgr", permission_manager.clone()),
            (
                "permission_checker",
                Arc::new(permission_checker::PermissionChecker::new(
                    self.clone(),
                    permission_manager,
                )),
            ),
            (
                "input_method",
                Arc::new(input_method::InputMethodManager::new(self.clone())),
            ),
            (
                "input",
                Arc::new(input_manager::InputManager::new(self.clone())),
            ),
            (
                "audio",
                Arc::new(audio_service::AudioService::new(self.clone())),
            ),
            ("mount", Arc::new(storage::StorageManager(self.clone()))),
            (
                "uri_grants",
                Arc::new(uri_grants::UriGrantsManager(self.clone())),
            ),
            (
                "platform_compat",
                Arc::new(platform_compat::PlatformCompat(self.clone())),
            ),
            (
                "sensor_privacy",
                Arc::new(sensor_privacy::SensorPrivacy::new(self.clone())),
            ),
            (
                "crossprofileapps",
                Arc::new(cross_profile_apps::CrossProfileApps::new(self.clone())),
            ),
            (
                "overlay",
                Arc::new(overlay_service::OverlayManager::new(self.clone())),
            ),
            (
                "vpn_management",
                Arc::new(vpn_manager::VpnManager::new(self.clone())),
            ),
            (
                "media_router",
                Arc::new(media_router::MediaRouter::new(self.clone())),
            ),
            (
                "shortcut",
                Arc::new(shortcut_service::ShortcutManager::new(self.clone())),
            ),
            (
                "netstats",
                Arc::new(netstats::NetworkStats::new(self.clone())),
            ),
            (
                "game",
                Arc::new(game_service::GameManager::new(self.clone())),
            ),
            (
                "performance_hint",
                self.hint_manager
                    .get_or_init(|| hint_manager::HintManager::new(self.clone()))
                    .clone(),
            ),
            ("sensorservice", Arc::new(sensor_service::SensorService)),
            (
                "wifi",
                Arc::new(wifi_service::WifiService::new(self.clone())),
            ),
            (
                "usagestats",
                Arc::new(usage_stats::UsageStats::new(self.clone())),
            ),
            (
                "webviewupdate",
                Arc::new(webview_update::WebViewUpdate::new(self.clone())),
            ),
            (
                "locale",
                Arc::new(locale_manager::LocaleManager::new(self.clone())),
            ),
            (
                "grammatical_inflection",
                Arc::new(grammatical_inflection::GrammaticalInflection::new(
                    self.clone(),
                )),
            ),
            (
                "domain_verification",
                Arc::new(domain_verification::DomainVerification::new(self.clone())),
            ),
            ("usb", Arc::new(usb_service::UsbService::new(self.clone()))),
            (
                "storagestats",
                Arc::new(storage_stats::StorageStatsService::new(self.clone())),
            ),
            (
                "system_update",
                Arc::new(system_update::SystemUpdate::new(self.clone())),
            ),
            (
                "lock_settings",
                Arc::new(lock_settings::LockSettings::new(self.clone())),
            ),
            (
                "media_session",
                media_session::MediaSession::new(self.clone(), settings.clone()),
            ),
            (
                "safety_center",
                Arc::new(safety_center::SafetyCenter::new(self.clone())),
            ),
            (
                "appwidget",
                Arc::new(app_widget::AppWidgetService::new(self.clone())),
            ),
            (
                "restrictions",
                Arc::new(restrictions_service::RestrictionsManager::new(self.clone())),
            ),
            (
                "legacy_permission",
                Arc::new(legacy_permission::LegacyPermissionManager::new(
                    self.clone(),
                )),
            ),
            (
                "device_identifiers",
                Arc::new(device_identifiers::DeviceIdentifiers::new(self.clone())),
            ),
            (
                "netpolicy",
                Arc::new(netpolicy_service::NetworkPolicy::new(self.clone())),
            ),
            (
                "telephony.registry",
                Arc::new(telephony_registry::TelephonyRegistry::new(self.clone())),
            ),
            (
                "network_management",
                Arc::new(network_management::NetworkManagement::new(self.clone())),
            ),
            (
                "role",
                self.role_service
                    .get_or_init(|| Arc::new(role_service::RoleManager::new(self.clone())))
                    .clone(),
            ),
            (
                "settings",
                Arc::new(settings_service::SettingsService::new(
                    self.clone(),
                    settings,
                )),
            ),
            (
                "content",
                self.content_service
                    .get_or_init(|| Arc::new(content_service::ContentService::new(self.clone())))
                    .clone(),
            ),
            (
                "notification",
                self.notification_service
                    .get_or_init(|| {
                        Arc::new(notification_service::NotificationManager::new(self.clone()))
                    })
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
            (
                "batteryproperties",
                Arc::new(battery_service::BatteryProperties(
                    self.battery_service
                        .get_or_init(|| {
                            Arc::new(battery_service::BatteryService::new(self.clone()))
                        })
                        .clone(),
                )),
            ),
            (
                "batterystats",
                self.battery_stats
                    .get_or_init(|| Arc::new(battery_stats::BatteryStats::new(self)))
                    .clone(),
            ),
            (
                "jobscheduler",
                self.job_scheduler
                    .get_or_init(|| job_scheduler::JobScheduler::new(self.clone()))
                    .clone(),
            ),
            (
                "account",
                account_service::AccountManager::new(self.clone()),
            ),
            (
                "device_policy",
                Arc::new(device_policy::DevicePolicy::new(self.clone())),
            ),
            (
                "connectivity",
                self.connectivity_service
                    .get_or_init(|| {
                        Arc::new(connectivity_service::ConnectivityService::new(self.clone()))
                    })
                    .clone(),
            ),
            (
                "statusbar",
                Arc::new(status_bar::StatusBar::new(self.clone())),
            ),
            (
                "statscompanion",
                Arc::new(stats_manager::StatsCompanionService(stats.clone())),
            ),
            (
                "statsmanager",
                Arc::new(stats_manager::StatsManagerService(stats)),
            ),
            (
                "bluetooth_manager",
                Arc::new(bluetooth_manager::BluetoothManagerService(bluetooth)),
            ),
            (
                "companiondevice",
                Arc::new(companion_device::CompanionDevice::new(self.clone())),
            ),
            // SystemServer starts it with
            // `enhanced_confirmation_mode_apis_enabled`, on in the image.
            (
                "ecm_enhanced_confirmation",
                Arc::new(enhanced_confirmation::EnhancedConfirmation::new(
                    self.clone(),
                )),
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
            &configured.data,
            number("ro.build.version.sdk")?,
            &property("ro.build.version.codename")?,
        )
        .map_err(state)?;
        let device_density = number("ro.sf.lcd_density")?;
        let packages = packages::Packages::load(
            &configured.root,
            &configured.data,
            device_density,
            &property("persist.sys.locale")?,
            &|name| (configured.properties)(name),
            // `SharedLibrariesImpl.ENFORCE_NATIVE_SHARED_LIBRARY_DEPENDENCIES`.
            &|name, target_sdk| compat.is_enabled(142_191_088, Some(name), target_sdk),
            &apex_manager::inventory(&self.process).map_err(state)?,
        )
        .map_err(state)?;
        let settings =
            settings::Settings::load(&configured.data, &configured.root).map_err(state)?;
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
            device_density,
            shared_memory,
        };
        Ok(self.inputs.get_or_init(|| inputs))
    }

    /// The launcher's start of package `name`'s activity `class`, user
    /// 0's: in its process if that runs, else in the one started for it,
    /// whose pid it answers.
    fn start_process(&self, name: &str, class: &str) -> Result<i32, Exception> {
        let info = self.activity_info_of(name, class)?;
        let (process, uid) = (
            providers::process_name(&info.info),
            info.info.application_info.uid as u32,
        );
        let me = self
            .me
            .upgrade()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host gone"))?;
        me.start_launcher(name, class, info)?;
        Ok(self
            .apps
            .lock()
            .unwrap()
            .values()
            .find(|a| a.process == process && a.uid == uid)
            .map_or(0, |a| a.pid))
    }

    /// User 0's activity `class` of package `name`, as the system sees it.
    fn activity_info_of(
        &self,
        name: &str,
        class: &str,
    ) -> Result<package::info::ActivityInfo, Exception> {
        let (state, resolution) = self.packages()?;
        let query = crate::package::query::Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: 1000,
        };
        let component = crate::package::intent::ComponentName {
            package: name.to_string(),
            class: class.to_string(),
        };
        match query.activity_info(&component, STOCK_PM_FLAGS, 1000, 0) {
            Ok(Ok(Some(info))) => Ok(info),
            Ok(Ok(None)) => Err(Exception::illegal_argument(format!(
                "no activity {name}/{class}"
            ))),
            Ok(Err(e)) => Err(e),
            Err(crate::package::apps_filter::NotModelled(reason)) => Err(Exception::new(
                EX_UNSUPPORTED_OPERATION,
                format!("activity info: {reason}"),
            )),
        }
    }

    /// The starts of the processes of `package` for `user`.
    fn starts_of_package(&self, package: &str, user: u32) -> Vec<i64> {
        self.apps
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, a)| a.package == package && a.uid / PER_USER_RANGE == user)
            .map(|(seq, _)| *seq)
            .collect()
    }

    /// The start of the process `pid`.
    fn start_of_pid(&self, pid: i32) -> Option<i64> {
        self.apps
            .lock()
            .unwrap()
            .iter()
            .find(|(_, a)| a.pid == pid)
            .map(|(seq, _)| *seq)
    }

    /// The running process `process` of `uid`: its start, and whether it
    /// attached (`ProcessList.getProcessRecordLocked`).
    fn running_process(&self, process: &str, uid: u32) -> Option<(i64, bool)> {
        self.apps
            .lock()
            .unwrap()
            .iter()
            .find(|(_, a)| a.process == process && a.uid == uid)
            .map(|(seq, a)| (*seq, a.thread.is_some()))
    }

    /// Starts the process of package `name` for `request`, or for a
    /// service, a provider or a broadcast (no activity). As
    /// `ProcessList.startProcessLocked` decides by `FLAG_FROM_BACKGROUND`,
    /// a start `from_background` (a broadcast's) fails for a bad process,
    /// and any other start makes the process good again and resets its
    /// crash counts.
    fn start_process_for(
        &self,
        name: &str,
        process: &str,
        request: Option<activity_start::StartRequest>,
        from_background: bool,
    ) -> Result<i32, Exception> {
        self.start_app_process(name, process, request, from_background, false)
            .map(|(_, pid)| pid)
    }

    /// A new isolated process `process` of package `name`, for a service
    /// (`startProcessLocked` with `isolated`): its start and pid. Its uid
    /// is a new isolated uid, which the package manager knows to be the
    /// package's (`addIsolatedUid`) until the process dies.
    fn start_isolated_process(&self, name: &str, process: &str) -> Result<(i64, i32), Exception> {
        self.start_app_process(name, process, None, false, true)
    }

    /// `ActivityManagerInternal.startIsolatedProcess`: a process `process`
    /// of the system (`android`) with `uid`, isolated, that runs `entry`'s
    /// `main` with `args` once it attaches (`runIsolatedEntryPoint`)
    /// instead of an application, for `abi`. The package manager knows
    /// `uid` to be the system's until it dies.
    pub(super) fn start_isolated_entry_point(
        &self,
        entry: &str,
        args: &[String],
        process: &str,
        abi: &str,
        uid: u32,
    ) -> Result<i32, Exception> {
        let inputs = self.inputs()?;
        let configured = self
            .configured
            .get()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host not configured"))?;
        let sdk = (configured.properties)("ro.build.version.sdk")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no ro.build.version.sdk"))?;
        let start_seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let start = zygote::Start {
            uid,
            gids: Vec::new(),
            runtime_flags: hidden_api_policy(
                0,
                0,
                0,
                inputs
                    .settings
                    .get(settings::Table::Global, "hidden_api_policy")
                    .as_deref(),
            ) << API_ENFORCEMENT_POLICY_SHIFT
                | memory_safety(&inputs.compat, "android", 0, sdk),
            mount_external: zygote::MountExternal::None,
            target_sdk_version: sdk,
            nice_name: process.to_string(),
            // `info.seInfo` (none) and `seInfoUser`.
            seinfo: format!("null{SEINFO_COMPLETE}"),
            instruction_set: Some(instruction_set(abi)),
            app_data_dir: None,
            start_child_zygote: false,
            package_name: Some("android".into()),
            is_top_app: true,
            pkg_data_info: Vec::new(),
            allowlisted_data_info: Vec::new(),
            bind_mount_app_storage_dirs: false,
            bind_mount_app_data_dirs: false,
            disabled_compat_changes: inputs.compat.disabled_changes("android", sdk),
            process_class: zygote::ACTIVITY_THREAD.into(),
            extra_args: vec![zygote::start_seq_arg(start_seq)],
        };
        let started = Instant::now();
        let pid = {
            let mut zygote = self.zygote.lock().unwrap();
            if zygote.is_none() {
                let stream = (configured.connect)()
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
            "app host: Start proc {pid}:{process}/{uid} for {entry} in {:?}",
            started.elapsed()
        );
        self.isolated_owners_changed(|owners| owners.push((uid as i32, SYSTEM_UID as i32)))?;
        self.apps.lock().unwrap().insert(
            start_seq,
            App {
                package: "android".into(),
                process: process.to_string(),
                uid,
                package_uid: SYSTEM_UID,
                isolated: true,
                entry_point: Some((entry.to_string(), args.to_vec())),
                application_info: None,
                pid,
                started,
                start_elapsed: clock_ms(libc::CLOCK_MONOTONIC_RAW),
                start_uptime: clock_ms(libc::CLOCK_UPTIME_RAW),
                thread: None,
                activities: Vec::new(),
                config: None,
                providers: Vec::new(),
                removed: false,
                first_frame: None,
                last_activity: 0,
                persistent: false,
                package_deps: Vec::new(),
                last_mem_info: None,
            },
        );
        Ok(pid)
    }

    /// A new isolated process `process` for a service of `info`'s,
    /// started by the WebView zygote (`ProcessList.startProcess` with a
    /// hosting record `byWebviewZygote`: `Process.startWebView`), bound
    /// with `info`: its start and pid.
    pub(super) fn start_webview_service_process(
        &self,
        info: Arc<package::info::ApplicationInfo>,
        process: &str,
    ) -> Result<(i64, i32), Exception> {
        let inputs = self.inputs()?;
        let owner = info.uid as u32;
        let package = info.item.package_name.clone().unwrap_or_default();
        let uid = self
            .isolated_uids
            .lock()
            .unwrap()
            .allocate(owner / PER_USER_RANGE)
            .ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("no isolated uid left for {process}"),
                )
            })?;
        let start_seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let start = zygote::Start {
            uid,
            gids: Vec::new(),
            runtime_flags: hidden_api_policy(
                info.flags,
                info.private_flags,
                info.private_flags_ext,
                inputs
                    .settings
                    .get(settings::Table::Global, "hidden_api_policy")
                    .as_deref(),
            ) << API_ENFORCEMENT_POLICY_SHIFT
                | memory_safety(
                    &inputs.compat,
                    &package,
                    info.flags,
                    info.target_sdk_version,
                ),
            mount_external: zygote::MountExternal::None,
            target_sdk_version: info.target_sdk_version,
            nice_name: process.to_string(),
            seinfo: format!(
                "{}{}",
                info.se_info.as_deref().unwrap_or("null"),
                info.se_info_user.as_deref().unwrap_or("")
            ),
            instruction_set: info.primary_cpu_abi.as_deref().map(instruction_set),
            app_data_dir: info.data_dir.clone(),
            start_child_zygote: false,
            package_name: Some(package.clone()),
            is_top_app: false,
            pkg_data_info: Vec::new(),
            allowlisted_data_info: Vec::new(),
            bind_mount_app_storage_dirs: false,
            bind_mount_app_data_dirs: false,
            disabled_compat_changes: inputs
                .compat
                .disabled_changes(&package, info.target_sdk_version),
            process_class: zygote::ACTIVITY_THREAD.into(),
            extra_args: vec![zygote::start_seq_arg(start_seq)],
        };
        let started = Instant::now();
        let pid = match self.start_webview(&start) {
            Ok(pid) => pid,
            Err(e) => {
                self.isolated_uids.lock().unwrap().used.remove(&uid);
                return Err(e);
            }
        };
        eprintln!(
            "app host: Start proc {pid}:{process}/{uid} by the WebView zygote in {:?}",
            started.elapsed()
        );
        self.isolated_owners_changed(|owners| owners.push((uid as i32, owner as i32)))?;
        self.apps.lock().unwrap().insert(
            start_seq,
            App {
                package,
                process: process.to_string(),
                uid,
                package_uid: owner,
                isolated: true,
                entry_point: None,
                application_info: Some(info),
                pid,
                started,
                start_elapsed: clock_ms(libc::CLOCK_MONOTONIC_RAW),
                start_uptime: clock_ms(libc::CLOCK_UPTIME_RAW),
                thread: None,
                activities: Vec::new(),
                config: None,
                providers: Vec::new(),
                removed: false,
                first_frame: None,
                last_activity: 0,
                persistent: false,
                package_deps: Vec::new(),
                last_mem_info: None,
            },
        );
        Ok((start_seq, pid))
    }

    fn start_app_process(
        &self,
        name: &str,
        process: &str,
        request: Option<activity_start::StartRequest>,
        from_background: bool,
        isolated: bool,
    ) -> Result<(i64, i32), Exception> {
        let inputs = self.inputs()?;
        // The system process (`setSystemProcess`) is the `android`
        // package's, `system` of the system uid: a component of another
        // system package declared in it runs there. The `android`
        // package's own components are system_server's code, which needs
        // its class path (`SYSTEMSERVERCLASSPATH`, the system class loader
        // `LoadedApk` gives `android`); a process zygote starts as an app
        // does not have it (#1518).
        if process == SYSTEM_PROCESS && !isolated {
            let uid = inputs
                .packages
                .application_info(name, STOCK_PM_FLAGS)
                .map(|ai| ai.uid as u32);
            if uid == Some(SYSTEM_UID) {
                if name == SYSTEM_PACKAGE {
                    return Err(self.unimplemented_named(
                        "activity",
                        "a component of system_server's own code (#1518)",
                    ));
                }
                return self.start_package_process(
                    SYSTEM_PACKAGE,
                    process,
                    request,
                    from_background,
                    false,
                );
            }
        }
        self.start_package_process(name, process, request, from_background, isolated)
    }

    /// `startProcessLocked` of the process `process` of package `name`.
    fn start_package_process(
        &self,
        name: &str,
        process: &str,
        request: Option<activity_start::StartRequest>,
        from_background: bool,
        isolated: bool,
    ) -> Result<(i64, i32), Exception> {
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
        let owner = listed.uid;
        let uid = if isolated {
            self.isolated_uids
                .lock()
                .unwrap()
                .allocate(owner / PER_USER_RANGE)
                .ok_or_else(|| {
                    Exception::new(
                        EX_ILLEGAL_STATE,
                        format!("no isolated uid left for {process}"),
                    )
                })?
        } else {
            owner
        };
        let started = self.spawn(
            name,
            process,
            request,
            from_background,
            isolated,
            uid,
            owner,
            setting,
            listed,
        );
        if isolated {
            match &started {
                Ok(_) => {
                    self.isolated_owners_changed(|owners| owners.push((uid as i32, owner as i32)))?
                }
                Err(_) => self.free_isolated_uid(uid)?,
            }
        }
        started
    }

    /// The rest of `startProcessLocked` for the process `process` of
    /// package `name` (of `package_uid`) with `uid`.
    #[allow(clippy::too_many_arguments)]
    fn spawn(
        &self,
        name: &str,
        process: &str,
        request: Option<activity_start::StartRequest>,
        from_background: bool,
        isolated: bool,
        uid: u32,
        package_uid: u32,
        setting: &package::settings::Package,
        listed: &package::list::Entry,
    ) -> Result<(i64, i32), Exception> {
        let inputs = self.inputs()?;
        if !isolated {
            let key = (process.to_string(), uid);
            let mut errors = self.app_errors.lock().unwrap();
            if from_background {
                if errors.bad.contains(&key) {
                    return Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        format!("bad process {uid}/{process}"),
                    ));
                }
            } else {
                errors.crash_times.remove(&key);
                errors.crash_counts.remove(&key);
                errors.bad.remove(&key);
            }
        }
        // An isolated process has no gids and no external storage.
        let (mount_external, gids) = if isolated {
            (zygote::MountExternal::None, Vec::new())
        } else {
            let mount_external = self.mount_mode(uid as i32, name)?;
            let external_storage_access = self.has_external_storage_access(uid as i32, name)?;
            (
                mount_external,
                process_gids(
                    listed.gids.clone(),
                    uid,
                    mount_external,
                    external_storage_access,
                ),
            )
        };
        let target_sdk = setting.target_sdk_version;
        let app_info = inputs
            .packages
            .application_info(name, STOCK_PM_FLAGS)
            .ok_or_else(|| {
                Exception::new(EX_ILLEGAL_STATE, format!("no application info of {name}"))
            })?;
        let start_seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let mut isolation = self.data_isolation(name, uid, target_sdk, mount_external)?;
        // An isolated process does not even mount its own app's data.
        if isolated {
            isolation.packages.clear();
            isolation.allowlisted.clear();
        }
        let start = zygote::Start {
            uid,
            gids,
            runtime_flags: hidden_api_policy(
                app_info.flags,
                app_info.private_flags,
                app_info.private_flags_ext,
                inputs
                    .settings
                    .get(settings::Table::Global, "hidden_api_policy")
                    .as_deref(),
            ) << API_ENFORCEMENT_POLICY_SHIFT
                | memory_safety(&inputs.compat, name, setting.flags, target_sdk),
            mount_external,
            target_sdk_version: target_sdk,
            nice_name: process.to_string(),
            seinfo: format!("{}{SEINFO_COMPLETE}", listed.seinfo),
            instruction_set: setting.primary_cpu_abi.as_deref().map(instruction_set),
            app_data_dir: Some(listed.data_dir.clone()),
            start_child_zygote: false,
            package_name: Some(name.to_string()),
            is_top_app: true,
            pkg_data_info: isolation.packages,
            allowlisted_data_info: isolation.allowlisted,
            bind_mount_app_storage_dirs: isolation.storage_dirs,
            bind_mount_app_data_dirs: isolation.data_dirs,
            disabled_compat_changes: inputs.compat.disabled_changes(name, target_sdk),
            process_class: zygote::ACTIVITY_THREAD.into(),
            extra_args: vec![zygote::start_seq_arg(start_seq)],
        };
        let started = Instant::now();
        // Starts are serialized: a process of this name and uid that runs
        // or is starting is kept, and what this start was for joins it
        // (`ProcessList.startProcessLocked` with a pending `ProcessRecord`).
        let mut zygote = self.zygote.lock().unwrap();
        let mut request = request;
        let joined = {
            let mut apps = self.apps.lock().unwrap();
            apps.iter_mut()
                .find(|(_, a)| a.process == process && a.uid == uid)
                .map(|(seq, a)| {
                    if a.thread.is_none() {
                        a.activities.extend(request.take());
                    }
                    (*seq, a.pid, a.package.clone())
                })
        };
        if let Some((seq, pid, package)) = joined {
            drop(zygote);
            if let Some(request) = request {
                self.launch_in_attached(seq, uid, &package, request)?;
            }
            return Ok((seq, pid));
        }
        let pid = {
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
            "app host: Start proc {pid}:{process}/{uid} in {:?}",
            started.elapsed()
        );
        self.apps.lock().unwrap().insert(
            start_seq,
            App {
                package: name.to_string(),
                process: process.to_string(),
                uid,
                package_uid,
                isolated,
                entry_point: None,
                application_info: None,
                pid,
                started,
                start_elapsed: clock_ms(libc::CLOCK_MONOTONIC_RAW),
                start_uptime: clock_ms(libc::CLOCK_UPTIME_RAW),
                thread: None,
                activities: request.into_iter().collect(),
                config: None,
                providers: Vec::new(),
                removed: false,
                first_frame: None,
                last_activity: 0,
                persistent: !isolated
                    && uid / PER_USER_RANGE == 0
                    && app_info.flags & PERSISTENT_MASK == PERSISTENT_MASK
                    && app_info.process_name.as_deref() == Some(process),
                package_deps: Vec::new(),
                last_mem_info: None,
            },
        );
        Ok((start_seq, pid))
    }

    /// `ProcessList.startProcess`'s app data isolation for `package`'s
    /// process of `uid`: its packages' data (its shared user's, else its
    /// own) and the allowlisted apps', which zygote binds alone into the
    /// process's view of `/data/user` (`--bind-mount-data-dirs`); the
    /// storage directories' isolation is vold's
    /// (`persist.sys.vold_app_data_isolation_enabled`).
    fn data_isolation(
        &self,
        package: &str,
        uid: u32,
        target_sdk: i32,
        mount_external: zygote::MountExternal,
    ) -> Result<Isolation, Exception> {
        let inputs = self.inputs()?;
        let configured = self
            .configured
            .get()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host not configured"))?;
        let flag = |name: &str, default: bool| {
            system_boolean((configured.properties)(name).as_deref(), default)
        };
        let (state, _) = self.packages()?;
        let user = (uid / PER_USER_RANGE) as i32;
        let app_id = uid % PER_USER_RANGE;
        let mut data_dirs = flag("persist.zygote.app_data_isolation", true)
            && ((FIRST_APPLICATION_UID..=19_999).contains(&app_id) || is_isolated(uid as i32))
            && inputs
                .compat
                .is_enabled(APP_DATA_DIRECTORY_ISOLATION, Some(package), target_sdk);
        let ps = state
            .packages
            .get(package)
            .ok_or_else(|| Exception::illegal_argument(format!("unknown package {package}")))?;
        let targets: Vec<&str> = match &ps.shared_user {
            Some(shared) => state
                .packages
                .values()
                .filter(|p| p.shared_user.as_ref() == Some(shared))
                .map(|p| p.name.as_str())
                .collect(),
            None => vec![package],
        };
        // `getPackageAppDataInfoMap`: none when a package has no CE data
        // inode yet; unknown packages are left out.
        let infos = |names: &mut dyn Iterator<Item = &str>| -> Option<Vec<zygote::DataInfo>> {
            let mut infos = Vec::new();
            for name in names {
                let Some(ps) = state.packages.get(name) else {
                    continue;
                };
                let inode = package::info::user_state(ps, user).ce_data_inode;
                if inode <= 0 {
                    return None;
                }
                infos.push(zygote::DataInfo {
                    package: name.to_string(),
                    volume_uuid: ps.volume_uuid.clone(),
                    ce_data_inode: inode,
                });
            }
            infos.sort_by_key(|i| package::info::java_hash(&i.package));
            Some(infos)
        };
        let packages = infos(&mut targets.iter().copied());
        let allowlist = &state.system.app_data_isolation_allowlist;
        let allowlisted = infos(
            &mut allowlist
                .iter()
                .map(String::as_str)
                .filter(|p| !targets.contains(p)),
        );
        data_dirs &= packages.is_some() && allowlisted.is_some();
        // `hasAppStorage`.
        let no_storage = ps
            .pkg
            .as_ref()
            .and_then(|p| p.properties.as_ref())
            .is_some_and(|props| {
                props.iter().any(|(n, p)| {
                    n == "android.internal.PROPERTY_NO_APP_DATA_STORAGE"
                        && p.value == package::pkg::PropertyValue::Bool(true)
                })
            });
        if no_storage {
            return Ok(Isolation::default());
        }
        // `needsStorageDataIsolation`, with the emulated volume mounted:
        // not for the media store's uid (`isExternalStorageService`, whose
        // mode is pass-through) or a mode that does not use `/mnt/user`.
        let storage_dirs = flag("persist.sys.vold_app_data_isolation_enabled", false)
            && packages.is_some()
            && self.storage_mounted()
            && mount_external == zygote::MountExternal::Default;
        Ok(Isolation {
            packages: packages.unwrap_or_default(),
            allowlisted: allowlisted.unwrap_or_default(),
            storage_dirs,
            data_dirs,
        })
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
        let uid = app.uid;
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
        if let Some((entry, args)) = &app.entry_point {
            // An isolated process that runs an entry point instead of
            // being bound to an application.
            let mut p = Parcel::new();
            thread::RunIsolatedEntryPoint {
                entry_point: Some(entry.clone()),
                entry_point_args: Some(args.iter().cloned().map(Some).collect()),
            }
            .write(&mut p);
            strong
                .transact(thread::RUN_ISOLATED_ENTRY_POINT, &p, true)
                .map_err(|s| {
                    Exception::new(EX_ILLEGAL_STATE, format!("runIsolatedEntryPoint: {s}"))
                })?;
            app.thread = Some(strong);
            return Ok(());
        }
        let providers = self.process_providers(&app.process, app.uid)?;
        let (bind, config) = self.bind_application(app, providers.clone())?;
        app.config = Some(config);
        app.providers = providers;
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
        // Its uid's first process.
        let first = !apps
            .iter()
            .any(|(s, a)| *s != seq && a.uid == uid && a.thread.is_some());
        drop(apps);
        if first {
            self.uid_changed(uid as i32, uid_observers::Change::Active);
        }
        // `onApplicationAttachedLocked`: the broadcasts waiting for it.
        self.broadcasts.process_attached(self, seq);
        Ok(())
    }

    /// `ProcessList.getIsolatedProcessesLocked`: the pids of the isolated
    /// processes started for `uid`'s package, none when there are none.
    pub(super) fn isolated_processes(&self, uid: i32) -> Option<Vec<i32>> {
        let pids: Vec<i32> = self
            .apps
            .lock()
            .unwrap()
            .values()
            .filter(|a| a.isolated && a.package_uid as i32 == uid)
            .map(|a| a.pid)
            .collect();
        (!pids.is_empty()).then_some(pids)
    }

    /// Whether `uid` has a process (each of which is top).
    fn uid_running(&self, uid: i32) -> bool {
        self.apps
            .lock()
            .unwrap()
            .values()
            .any(|a| a.uid as i32 == uid)
    }

    /// The process of `start_seq`, `pid`, died: what it had goes, and its
    /// exit is noted (`noteProcessDiedLocked`).
    fn process_died(&self, start_seq: i64, pid: i32) {
        let (removed, gone, exit, isolated) = {
            let mut apps = self.apps.lock().unwrap();
            let app = apps.remove(&start_seq);
            let isolated = app.as_ref().is_some_and(|a| a.isolated);
            let gone = app.as_ref().filter(|a| {
                a.thread.is_some() && !apps.values().any(|o| o.uid == a.uid && o.thread.is_some())
            });
            let exit = app.as_ref().map(|a| {
                exit_info::ExitInfo::raw(
                    &a.package,
                    &a.process,
                    a.uid,
                    a.package_uid,
                    a.pid,
                    a.thread.is_some(),
                )
            });
            (
                app.as_ref().is_some_and(|a| a.removed),
                gone.map(|a| a.uid),
                exit,
                isolated,
            )
        };
        if let Some(exit) = exit {
            if isolated && let Err(e) = self.free_isolated_uid(exit.real_uid as u32) {
                eprintln!("app host: isolated uid {}: {}", exit.real_uid, e.message);
            }
            self.note_exit(&exit, false);
        }
        if let Some(uid) = gone {
            self.uid_changed(uid as i32, uid_observers::Change::Gone);
        }
        self.providers_process_died(start_seq);
        let (gone, commands) = self.activities.remove_start(start_seq);
        for token in gone {
            self.release_uri_grants(uri_grants::Owner::Activity(token));
        }
        self.windows_died(pid);
        self.broadcasts.process_died(self, start_seq);
        if let Some(me) = self.me.upgrade() {
            me.run(commands);
            me.services_process_died(start_seq, !removed);
        }
    }

    /// `AppErrors.crashApplicationInner` for the process of `pid` and
    /// `uid`: its exit is noted as a crash (`native` for a native one), and
    /// as `handleAppCrashLSPB` does, its services' crash counts go up, and
    /// a process that crashed again within `MIN_CRASH_INTERVAL`, or too
    /// often in the last hours, is removed: its services are not started
    /// again, unless a foreground one may be (`removeProcessLocked`'s
    /// `allowRestart`), and it is marked bad, so broadcasts no longer start
    /// it until something else does (`markBadProcess`).
    fn app_crashed(&self, pid: i32, uid: u32, native: bool) {
        const PROCESS_CRASH_COUNT_RESET_INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);
        const PROCESS_CRASH_COUNT_LIMIT: u32 = 12;
        let found = self
            .apps
            .lock()
            .unwrap()
            .iter()
            .find(|(_, a)| a.pid == pid && a.uid == uid)
            .map(|(seq, a)| {
                (
                    *seq,
                    a.process.clone(),
                    exit_info::ExitInfo::raw(
                        &a.package,
                        &a.process,
                        a.uid,
                        a.package_uid,
                        a.pid,
                        a.thread.is_some(),
                    ),
                )
            });
        let Some((start_seq, process, mut exit)) = found else {
            return;
        };
        exit.reason = if native {
            exit_info::REASON_CRASH_NATIVE
        } else {
            exit_info::REASON_CRASH
        };
        exit.description = Some("crash".into());
        self.note_exit(&exit, true);
        let try_again = self.services_crashed(start_seq);
        let now = Instant::now();
        let key = (process, uid);
        let mut errors = self.app_errors.lock().unwrap();
        let quick = errors
            .crash_times
            .get(&key)
            .is_some_and(|t| now < *t + services::MIN_CRASH_INTERVAL);
        let over = errors.crash_counts.get(&key).is_some_and(|(first, n)| {
            now < *first + PROCESS_CRASH_COUNT_RESET_INTERVAL && *n >= PROCESS_CRASH_COUNT_LIMIT
        });
        if quick || over {
            let reason = if quick {
                "crashed quickly"
            } else {
                "over process crash limit"
            };
            eprintln!(
                "app host: process {} has crashed too many times, killing! Reason: {reason}",
                key.0
            );
            errors.crash_times.remove(&key);
            errors.crash_counts.remove(&key);
            errors.bad.insert(key);
            if let Some(app) = self.apps.lock().unwrap().get_mut(&start_seq) {
                app.removed = !try_again;
            }
            return;
        }
        errors.crash_times.insert(key.clone(), now);
        let count = match errors.crash_counts.get(&key) {
            Some((first, n)) if *first + PROCESS_CRASH_COUNT_RESET_INTERVAL >= now => {
                (*first, n + 1)
            }
            _ => (now, 1),
        };
        errors.crash_counts.insert(key, count);
    }

    /// `UserController.handleIncomingUser` for `uid`: the user it acts as.
    fn handle_incoming_user(
        &self,
        uid: i32,
        user: i32,
        allow_all: bool,
        require_full: bool,
        name: Option<&str>,
    ) -> Result<i32, Exception> {
        let target = self.incoming_user(uid, user, !require_full)?;
        if !allow_all && target < 0 {
            return Err(Exception::illegal_argument(format!(
                "Call does not support special user #{target}{}",
                name.map(|n| format!(" ({n})")).unwrap_or_default()
            )));
        }
        Ok(target)
    }

    /// A guest property.
    pub(super) fn property(&self, name: &str) -> Option<String> {
        (self.configured.get()?.properties)(name)
    }

    /// Sets a guest property, as the system.
    pub(super) fn set_property(&self, name: &str, value: &str) {
        if let Some(c) = self.configured.get() {
            (c.set_property)(name, value);
        }
    }

    /// `SystemTimeZone.isValidTimeZoneId`: whether the image's tzdata has
    /// `zone`.
    pub(super) fn valid_time_zone(&self, zone: &str) -> bool {
        let tzdata = self.tzdata.get_or_init(|| {
            let root = self
                .configured
                .get()
                .map(|c| c.root.clone())
                .unwrap_or_default();
            std::fs::read(root.join(crate::tzdata::PATH.trim_start_matches('/')))
                .unwrap_or_default()
        });
        crate::tzdata::has(tzdata, zone)
    }

    /// The running processes re-read the time zone (`updateTimeZone`, on
    /// `ACTION_TIMEZONE_CHANGED`).
    pub(super) fn update_time_zone(&self) {
        let seqs: Vec<i64> = self
            .apps
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, a)| a.thread.is_some())
            .map(|(s, _)| *s)
            .collect();
        let mut p = Parcel::new();
        thread::UpdateTimeZone {}.write(&mut p);
        for seq in seqs {
            if let Err(e) = self.call_thread(seq, thread::UPDATE_TIME_ZONE, &p) {
                eprintln!("app host: updateTimeZone: {e}");
            }
        }
    }

    /// The system user's records, made once if this is the data's first
    /// boot, before anything reads them, as UserManagerService's
    /// construction makes them before its first call.
    fn ensure_system_user(&self) -> Result<(), String> {
        self.system_user
            .get_or_init(|| user_service::ensure_system_user(self))
            .clone()
    }

    /// User 0's file (`/data/system/users/0.xml`) and its path.
    pub(super) fn user_file(&self) -> Result<(PathBuf, Vec<u8>), Exception> {
        let path = self.data()?.join("system/users/0.xml");
        let state =
            |e: String| Exception::new(EX_ILLEGAL_STATE, format!("{}: {e}", path.display()));
        self.ensure_system_user().map_err(state)?;
        let bytes = std::fs::read(&path).map_err(|e| state(e.to_string()))?;
        Ok((path, bytes))
    }

    /// User 0's `UserInfo` attribute `name` (`/data/system/users/0.xml`).
    fn user_attribute(&self, name: &str) -> Result<i32, Exception> {
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let (path, bytes) = self.user_file()?;
        let root =
            aim_android_xml::read(&bytes).map_err(|e| state(format!("{}: {e}", path.display())))?;
        root.int(name)
            .map_err(state)?
            .ok_or_else(|| state(format!("{}: no {name}", path.display())))
    }

    /// User 0's persisted properties: `users/0.xml`'s `<userProperties>`
    /// attributes, as `UserProperties.updateFromXml` reads them (booleans
    /// 0 or 1).
    fn user_properties(&self) -> Result<Vec<(String, i32)>, Exception> {
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let (path, bytes) = self.user_file()?;
        let root =
            aim_android_xml::read(&bytes).map_err(|e| state(format!("{}: {e}", path.display())))?;
        let mut out = Vec::new();
        for props in root.children().filter(|e| e.name == "userProperties") {
            for (name, _) in &props.attrs {
                let value = match props.bool(name) {
                    Ok(Some(b)) => i32::from(b),
                    _ => props.int(name).map_err(state)?.unwrap_or(0),
                };
                out.push((name.clone(), value));
            }
        }
        Ok(out)
    }

    /// User 0's serial number (`UserInfo.serialNumber`).
    fn user_serial(&self) -> Result<i32, Exception> {
        self.user_attribute("serialNumber")
    }

    /// User 0's `UserInfo.flags`.
    fn user_flags(&self) -> Result<i32, Exception> {
        self.user_attribute("flags")
    }

    /// `addPackageDependency`: the calling process loaded `package`'s
    /// code, unless the caller may not see it.
    fn add_package_dependency(
        &self,
        call: &Call<'_>,
        package: Option<String>,
    ) -> Result<(), Exception> {
        let Some(package) = package else {
            return Ok(());
        };
        let calling_uid = call.sender_euid as i32;
        let (state, resolution) = self.packages()?;
        // `PackageManagerInternal.filterAppAccess`.
        let ps = state.packages.get(&package);
        let filtered = ps.is_none()
            || crate::package::apps_filter::should_filter_application(
                &state,
                &resolution.apps_filter,
                ps,
                calling_uid,
                calling_uid / PER_USER_RANGE as i32,
                true,
                true,
            )
            .map_err(|e| {
                Exception::new(
                    EX_UNSUPPORTED_OPERATION,
                    format!("addPackageDependency: {}", e.0),
                )
            })?;
        if filtered {
            eprintln!(
                "app host: Failed trying to add dependency on non-existing package: {package}"
            );
            return Ok(());
        }
        if let Some(app) = self
            .apps
            .lock()
            .unwrap()
            .values_mut()
            .find(|a| a.pid == call.sender_pid)
        {
            if !app.package_deps.contains(&package) {
                app.package_deps.push(package);
            }
        }
        Ok(())
    }

    /// `getPackageProcessState`: the lowest state a process running
    /// `package` or depending on it was set to (`getSetProcState`), for a
    /// caller allowed usage stats or holding PACKAGE_USAGE_STATS. The app
    /// host sets each process it binds to PROCESS_STATE_TOP; a process not
    /// yet bound, or one running an isolated entry point, was set none.
    fn package_process_state(
        &self,
        call: &Call<'_>,
        package: Option<&str>,
        calling_package: Option<&str>,
    ) -> Result<i32, Exception> {
        let uid = call.sender_euid as i32;
        if !self.has_usage_stats_permission(uid, calling_package)?
            && self.check_permission(Some(PACKAGE_USAGE_STATS), uid)? != 0
        {
            return Err(Exception::security(format!(
                "Permission Denial: getPackageProcessState from pid={}, uid={uid} requires {PACKAGE_USAGE_STATS}",
                call.sender_pid
            )));
        }
        let apps = self.apps.lock().unwrap();
        let processes = apps.values().map(|a| {
            let set = if a.thread.is_some() && a.entry_point.is_none() {
                PROCESS_STATE_TOP
            } else {
                PROCESS_STATE_NONEXISTENT
            };
            (a.package.as_str(), a.package_deps.as_slice(), set)
        });
        Ok(package_process_state(processes, package))
    }

    /// `hasUsageStatsPermission`: OP_GET_USAGE_STATS noted for the caller's
    /// package, by default PACKAGE_USAGE_STATS.
    fn has_usage_stats_permission(
        &self,
        uid: i32,
        package: Option<&str>,
    ) -> Result<bool, Exception> {
        match appops_service::note(self, OP_GET_USAGE_STATS, uid, package)? {
            appops_service::MODE_DEFAULT => {
                Ok(self.check_permission(Some(PACKAGE_USAGE_STATS), uid)? == 0)
            }
            mode => Ok(mode == appops_service::MODE_ALLOWED),
        }
    }

    /// `isBackgroundRestricted` of the caller's own `package`: whether its
    /// RUN_ANY_IN_BACKGROUND op is not allowed.
    fn background_restricted(
        &self,
        call: &Call<'_>,
        package: Option<String>,
    ) -> Result<bool, Exception> {
        /// `PackageManager.MATCH_DEBUG_TRIAGED_MISSING`;
        /// `AppOpsManager.OP_RUN_ANY_IN_BACKGROUND`.
        const MATCH_DEBUG_TRIAGED_MISSING: i64 = 0x1000_0000;
        const OP_RUN_ANY_IN_BACKGROUND: i32 = 70;
        let calling_uid = call.sender_euid as i32;
        let (state, resolution) = self.packages()?;
        let query = crate::package::query::Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: 1000,
        };
        let package_uid = match &package {
            Some(package) => match query.package_uid(
                package,
                MATCH_DEBUG_TRIAGED_MISSING,
                calling_uid / PER_USER_RANGE as i32,
            ) {
                Ok(uid) => uid?,
                Err(crate::package::apps_filter::NotModelled(reason)) => {
                    return Err(Exception::new(
                        EX_UNSUPPORTED_OPERATION,
                        format!("activity isBackgroundRestricted: {reason}"),
                    ));
                }
            },
            None => -1,
        };
        if package_uid != calling_uid {
            return Err(Exception::illegal_argument(format!(
                "Uid {calling_uid} cannot query restriction state for package {}",
                package.as_deref().unwrap_or("null")
            )));
        }
        Ok(appops_service::mode(
            self,
            OP_RUN_ANY_IN_BACKGROUND,
            calling_uid,
            package.as_deref(),
        )? != appops_service::MODE_ALLOWED)
    }

    /// `UserController.isUserRunning`: user 0 runs, unlocked; no other user
    /// is started.
    fn user_running(&self, call: &Call<'_>, user: i32, flags: i32) -> Result<bool, Exception> {
        const FLAG_OR_STOPPED: i32 = 1 << 0;
        const FLAG_AND_LOCKED: i32 = 1 << 1;
        let uid = call.sender_euid as i32;
        if user != uid / PER_USER_RANGE as i32
            && self.check_permission(Some("android.permission.INTERACT_ACROSS_USERS"), uid)? != 0
        {
            return Err(Exception::security(format!(
                "Permission Denial: isUserRunning() from pid={}, uid={uid} requires android.permission.INTERACT_ACROSS_USERS",
                call.sender_pid
            )));
        }
        if user != 0 {
            return Ok(false);
        }
        Ok(flags & FLAG_OR_STOPPED != 0 || flags & FLAG_AND_LOCKED == 0)
    }

    /// User 0's base restrictions, as UserManagerService last persisted
    /// them (`/data/system/users/0.xml`'s `<restrictions>`).
    fn user_restrictions(&self) -> Result<&std::collections::BTreeSet<String>, Exception> {
        if let Some(r) = self.user_restrictions.get() {
            return Ok(r);
        }
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let (path, bytes) = self.user_file()?;
        let root =
            aim_android_xml::read(&bytes).map_err(|e| state(format!("{}: {e}", path.display())))?;
        let restrictions = root
            .children()
            .find(|e| e.name == "restrictions")
            .map(|r| {
                r.attrs
                    .iter()
                    .filter(|(_, v)| matches!(v, aim_android_xml::Value::Bool(true)))
                    .map(|(k, _)| k.clone())
                    .collect()
            })
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
    fn fill_in_proc_mem_info(
        &self,
        start_seq: i64,
        app: &App,
        info: &mut RunningAppProcessInfo,
        front: Option<i64>,
    ) {
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
    fn running_app_processes(
        &self,
        call: &Call<'_>,
    ) -> Result<Option<Vec<Option<RunningAppProcessInfo>>>, Exception> {
        let uid = call.sender_euid as i32;
        if is_isolated(uid) {
            return Err(Exception::security(
                "Isolated process not allowed to call getRunningAppProcesses",
            ));
        }
        let all_users =
            self.check_permission(Some("android.permission.INTERACT_ACROSS_USERS_FULL"), uid)? == 0;
        let all_uids = self.get_tasks_allowed(uid)?;
        let front = self.activities.front_start();
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
                    process_name: Some(a.process.clone()),
                    pkg_list: Some(vec![a.package.clone()]),
                    ..Default::default()
                };
                self.fill_in_proc_mem_info(*seq, a, &mut info, front);
                Some(info)
            })
            .collect();
        Ok((!list.is_empty()).then_some(list))
    }

    /// `ActivityTaskManagerService.isGetTasksAllowed`: whether `uid` may
    /// see other uids' tasks and processes: the recents component's, one
    /// with REAL_GET_TASKS, or a privileged one with the old GET_TASKS.
    fn get_tasks_allowed(&self, uid: i32) -> Result<bool, Exception> {
        if self.recents_uid()?.is_some_and(|r| {
            r.rem_euclid(PER_USER_RANGE as i32) == uid.rem_euclid(PER_USER_RANGE as i32)
        }) {
            return Ok(true);
        }
        if self.check_permission(Some("android.permission.REAL_GET_TASKS"), uid)? == 0 {
            return Ok(true);
        }
        if self.check_permission(Some("android.permission.GET_TASKS"), uid)? != 0 {
            return Ok(false);
        }
        let (state, resolution) = self.packages()?;
        let query = crate::package::query::Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: uid,
        };
        query
            .uid_privileged(uid)
            .map_err(|crate::package::apps_filter::NotModelled(e)| {
                Exception::new(EX_UNSUPPORTED_OPERATION, format!("isUidPrivileged: {e}"))
            })
    }

    /// `RecentTasks.loadRecentsComponent`: the uid of the package of
    /// `config_recentsComponentName`, if installed.
    fn recents_uid(&self) -> Result<Option<i32>, Exception> {
        const MATCH_DISABLED_COMPONENTS: i64 = 0x200;
        const MATCH_UNINSTALLED_PACKAGES: i64 = 0x2000;
        let packages = &self.inputs()?.packages;
        let component = packages
            .framework_string("config_recentsComponentName")
            .unwrap_or_default();
        let Some((package, _)) = component.split_once('/') else {
            return Ok(None);
        };
        Ok(packages
            .application_info(
                package,
                MATCH_UNINSTALLED_PACKAGES | MATCH_DISABLED_COMPONENTS,
            )
            .map(|ai| ai.uid))
    }

    /// `getMyMemoryState`: the calling process's state.
    fn my_memory_state(&self, call: &Call<'_>) -> Result<RunningAppProcessInfo, Exception> {
        if is_isolated(call.sender_euid as i32) {
            return Err(Exception::security(
                "Isolated process not allowed to call getMyMemoryState",
            ));
        }
        let mut info = RunningAppProcessInfo::default();
        let front = self.activities.front_start();
        let apps = self.apps.lock().unwrap();
        if let Some((seq, app)) = apps
            .iter()
            .find(|(_, a)| a.pid == call.sender_pid && a.thread.is_some())
        {
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
        let thread = apps
            .get(&start_seq)
            .and_then(|a| a.thread.as_ref())
            .ok_or_else(|| format!("no process of start {start_seq}"))?;
        thread
            .transact(code, data, true)
            .map(|_| ())
            .map_err(|s| format!("{code}: {s}"))
    }

    /// `UserController.handleIncomingUser`: `USER_CURRENT` as user 0,
    /// another user than the caller's, `USER_ALL` among them, only with
    /// `INTERACT_ACROSS_USERS_FULL` (or, if `allow_non_full`,
    /// `INTERACT_ACROSS_USERS`).
    fn incoming_user(&self, uid: i32, user: i32, allow_non_full: bool) -> Result<i32, Exception> {
        const USER_ALL: i32 = -1;
        const USER_CURRENT: i32 = -2;
        let user = if user == USER_CURRENT { 0 } else { user };
        let has = |p: &str| self.check_permission(Some(p), uid).map(|r| r == 0);
        if user < 0 && user != USER_ALL {
            return Err(Exception::new(
                EX_ILLEGAL_ARGUMENT,
                format!("Invalid user: {user}"),
            ));
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
        self.call_thread(
            start_seq,
            thread::SCHEDULE_TRANSACTION,
            &activity::transaction(items),
        )
    }

    /// Kills the process of `start_seq` (`ProcessRecord.killLocked`) for
    /// `reason`, `sub_reason` and `description`, noting its exit first.
    fn kill_process(
        &self,
        start_seq: i64,
        reason: i32,
        sub_reason: i32,
        description: &str,
    ) -> Result<(), String> {
        let exit = self
            .apps
            .lock()
            .unwrap()
            .get(&start_seq)
            .map(|a| {
                exit_info::ExitInfo::raw(
                    &a.package,
                    &a.process,
                    a.uid,
                    a.package_uid,
                    a.pid,
                    a.thread.is_some(),
                )
            })
            .ok_or_else(|| format!("no process of start {start_seq}"))?;
        let pid = exit.pid;
        self.note_exit(
            &exit_info::ExitInfo {
                reason,
                sub_reason,
                description: Some(description.into()),
                ..exit
            },
            true,
        );
        self.call_agent(KILL_PROCESS, "killProcess", |data| data.write_i32(pid))
            .map(|_| ())
            .map_err(|e| format!("{e:?}"))
    }

    /// `ProcessList.killPackageProcessesLSP` of `package` in the user and
    /// app id of `uid`, for a change of its state (`REASON_PACKAGE_STATE_CHANGE`):
    /// its processes are killed.
    fn kill_package_processes(&self, package: &str, uid: u32) {
        let starts: Vec<i64> = self
            .apps
            .lock()
            .unwrap()
            .iter()
            // `killPackageProcessesLSP`: the package's processes, and those
            // of its user depending on it.
            .filter(|(_, a)| {
                a.package == package && a.uid == uid
                    || a.uid / PER_USER_RANGE == uid / PER_USER_RANGE
                        && a.package_deps.iter().any(|d| d == package)
            })
            .map(|(seq, _)| *seq)
            .collect();
        for seq in starts {
            let description = format!("change {package}");
            if let Err(e) = self.kill_process(
                seq,
                exit_info::REASON_PACKAGE_STATE_CHANGE,
                exit_info::SUBREASON_UNKNOWN,
                &description,
            ) {
                eprintln!("app host: killing {package}'s process of start {seq}: {e}");
            }
        }
    }

    /// `killUidForPermissionChange`: the processes of `uid`
    /// (`killPackageProcessesLSP` of its app id and user), for a change of
    /// its permissions.
    fn kill_uid_for_permission_change(&self, uid: u32, reason: &str) {
        let starts: Vec<i64> = self
            .apps
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, a)| a.uid == uid)
            .map(|(seq, _)| *seq)
            .collect();
        for seq in starts {
            if let Err(e) = self.kill_process(
                seq,
                exit_info::REASON_PERMISSION_CHANGE,
                exit_info::SUBREASON_UNKNOWN,
                reason,
            ) {
                eprintln!("app host: killing uid {uid}'s process of start {seq}: {e}");
            }
        }
    }

    /// `ActivityManagerService.checkPermission`: no permission is denied;
    /// else `ActivityManager.checkComponentPermission` of an exported
    /// component nobody owns, which ends in PermissionManagerService's
    /// `checkUidPermission` (`permissionmgr`'s), so that `Context` and
    /// `PackageManager` checks agree.
    pub(super) fn check_permission(
        &self,
        permission: Option<&str>,
        uid: i32,
    ) -> Result<i32, Exception> {
        const GRANTED: i32 = 0;
        const DENIED: i32 = -1;
        let Some(permission) = permission else {
            return Ok(DENIED);
        };
        let permissions = self
            .permission_manager
            .get()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "no permissionmgr"))?;
        Ok(if permissions.holds(uid, permission)? {
            GRANTED
        } else {
            DENIED
        })
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
        let requests = std::mem::take(&mut app.activities);
        if requests.is_empty() {
            return Ok(());
        }
        let (uid, package, started) = (app.uid, app.package.clone(), app.started);
        drop(apps);
        for request in requests {
            self.launch_in_attached(start_seq, uid, &package, request)?;
        }
        eprintln!(
            "app host: {package} launch sent {:?} after its start",
            started.elapsed()
        );
        Ok(())
    }

    /// `realStartActivityLocked` of `request` in the attached process of
    /// `start_seq`, on top of the task the start placed it in, else of a
    /// new one.
    fn launch_in_attached(
        &self,
        start_seq: i64,
        uid: u32,
        package: &str,
        request: activity_start::StartRequest,
    ) -> Result<(), Exception> {
        let info = self.activity_info_of(package, &request.class)?;
        let decision = activity_stack::Decision {
            outcome: activity_stack::Outcome::New(request.task),
            ..Default::default()
        };
        if let Some(me) = self.me.upgrade() {
            me.start_in_process(start_seq, uid, package, info, request, &decision)?;
        }
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
        Ok(&self
            .configured
            .get()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host not configured"))?
            .root)
    }

    /// The guest's `/data`.
    fn data(&self) -> Result<&Path, Exception> {
        Ok(&self
            .configured
            .get()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host not configured"))?
            .data)
    }

    /// The guest's `/sys/fs/bpf`.
    fn bpf(&self) -> Result<&Path, Exception> {
        Ok(&self
            .configured
            .get()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host not configured"))?
            .bpf)
    }

    /// `ActivityTaskManagerService.updateActivityUsageStats`: the usage
    /// event of activity `token` for app standby.
    fn activity_usage(self: &Arc<Self>, token: u64, event: app_standby::Event) {
        let app = self
            .activities
            .records
            .lock()
            .unwrap()
            .get(&token)
            .map(|r| (r.package.clone(), r.uid));
        if let Some((package, uid)) = app {
            self.report_usage_event(&package, (uid / PER_USER_RANGE) as i32, event);
        }
        self.activity_usage_stats(
            token,
            match event {
                app_standby::Event::ActivityResumed => usage_events::ACTIVITY_RESUMED,
                app_standby::Event::ActivityPaused => usage_events::ACTIVITY_PAUSED,
            },
        );
    }

    /// `ActivityTaskManagerService.updateActivityUsageStats`: the event of
    /// the activity of `token`, with its task's root activity, to
    /// UsageStatsService (`reportEvent`), its instance the token's.
    fn activity_usage_stats(&self, token: u64, event_type: i32) {
        let Some((package, class, uid, task)) = self
            .activities
            .records
            .lock()
            .unwrap()
            .get(&token)
            .map(|r| (r.package.clone(), r.class.clone(), r.uid, r.task))
        else {
            return;
        };
        let root = {
            let stack = self.activities.stack.lock().unwrap();
            stack
                .root(task)
                .or_else(|| stack.activities.iter().find(|a| a.task == task))
                .map(|a| a.token)
        };
        let root = root.and_then(|t| {
            self.activities
                .records
                .lock()
                .unwrap()
                .get(&t)
                .map(|r| (r.package.clone(), r.class.clone()))
        });
        let user = (uid / PER_USER_RANGE) as i32;
        let instant = self.packages().is_ok_and(|(state, _)| {
            state
                .packages
                .get(&package)
                .and_then(|ps| ps.users.get(&user))
                .is_some_and(|u| u.instant_app)
        });
        let event = usage_events::Event {
            package,
            class: Some(class),
            instance_id: token as i32,
            task_root_package: root.as_ref().map(|r| r.0.clone()),
            task_root_class: root.map(|r| r.1),
            event_type,
            time_stamp: usage_stats::now_millis(),
            flags: if instant {
                usage_events::FLAG_IS_PACKAGE_INSTANT_APP
            } else {
                0
            },
            extra: usage_events::Extra::None,
        };
        self.report_usage_stats_event(event);
    }

    /// What DevicePolicyManagerService persisted, read once.
    fn device_policies(&self) -> Result<&device_policy::Policies, Exception> {
        if let Some(policies) = self.device_policies.get() {
            return Ok(policies);
        }
        let policies = device_policy::Policies::load(self.data()?)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        Ok(self.device_policies.get_or_init(|| policies))
    }

    /// The app host's connection to SurfaceFlinger, made once.
    fn composer(&self) -> Result<&surface::Composer, Exception> {
        if let Some(composer) = self.composer.get() {
            return Ok(composer);
        }
        let composer = surface::Composer::connect(&self.process)
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e))?;
        Ok(self.composer.get_or_init(|| composer))
    }

    /// The global configuration `uid`'s process was bound with.
    fn global_config(&self, uid: u32) -> Option<config::Configuration> {
        self.apps
            .lock()
            .unwrap()
            .values()
            .find(|a| a.uid == uid && a.thread.is_some())
            .and_then(|a| a.config.clone())
    }

    /// The activity `ApplicationPackageManager.getLaunchIntentForPackage`
    /// launches `package` with: the first of its `MAIN` activities in the
    /// `INFO` category, else in the `LAUNCHER` category.
    fn launch_activity_of(&self, package: &str, calling_uid: i32) -> Result<String, Exception> {
        let (_, resolution) = self.packages()?;
        for category in [
            "android.intent.category.INFO",
            "android.intent.category.LAUNCHER",
        ] {
            let intent = crate::package::intent::Intent {
                action: Some("android.intent.action.MAIN".into()),
                categories: Some(vec![category.into()]),
                package: Some(package.into()),
                ..Default::default()
            };
            let found = resolution
                .query_intent_activities(&intent, None, 0, 0, calling_uid)
                .map_err(|e| Exception::new(EX_ILLEGAL_STATE, format!("{package}: {e:?}")))?;
            if let Some(crate::package::component_resolver::Info::Activity(activity)) =
                found.into_iter().next().map(|r| r.info)
            {
                return activity
                    .info
                    .item
                    .name
                    .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "an activity without a name"));
            }
        }
        Err(Exception::illegal_argument(format!(
            "{package}: no launch activity"
        )))
    }

    /// A window input channel from the agent, for a window of `frame` on
    /// a display of `display` pixels: its index, and the reply, whose
    /// client socket starts at [`AGENT_CHANNEL_FD`].
    fn open_input_channel(
        &self,
        name: &str,
        display: (i32, i32),
        input: &window_manager::Input,
    ) -> Result<(i32, aim_binder_host::local::Received), Exception> {
        let reply = self.call_agent(OPEN_INPUT_CHANNEL, "openInputChannel", |data| {
            data.write_string16(Some(name));
            [display.0, display.1]
                .iter()
                .for_each(|v| data.write_i32(*v));
            write_input(data, input);
        })?;
        let mut r = reply.reader();
        let index = r.read_exception().ok().and_then(|_| r.read_i32().ok());
        let index =
            index.ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "openInputChannel reply"))?;
        Ok((index, reply))
    }

    /// The input channel `index` takes the focus, its window in front.
    fn focus_input_channel(&self, index: i32) -> Result<(), Exception> {
        self.call_agent(FOCUS_INPUT_CHANNEL, "focusInputChannel", |data| {
            data.write_i32(index)
        })
        .map(|_| ())
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
    fn call_agent(
        &self,
        code: u32,
        name: &str,
        write: impl FnOnce(&mut Parcel),
    ) -> Result<aim_binder_host::local::Received, Exception> {
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let agent = self.agent.lock().unwrap();
        let agent = agent
            .as_ref()
            .ok_or_else(|| state("the app host agent has not attached".into()))?;
        let mut data = Parcel::new();
        data.write_interface_token(AGENT_DESCRIPTOR);
        write(&mut data);
        let reply = agent
            .transact(code, &data, false)
            .map_err(|s| state(format!("{name}: {s}")))?;
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
            .lock()
            .unwrap()
            .get_or_insert_with(|| Arc::new(inputs.packages.state().clone()))
            .clone();
        let resolution = self
            .resolver
            .resolution(&state)
            .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "package resolution: MIME groups"))?;
        Ok((state, resolution))
    }

    /// `PackageManagerInternal.addIsolatedUid`/`removeIsolatedUid`: the
    /// package model's isolated uids and their owners, changed by `change`.
    fn isolated_owners_changed(
        &self,
        change: impl FnOnce(&mut Vec<(i32, i32)>),
    ) -> Result<(), Exception> {
        let _writer = self.model_writer.lock().unwrap();
        let (before, _) = self.packages()?;
        let mut state = (*before).clone();
        change(&mut state.system.isolated_owners);
        self.user_states_changed(&before, Arc::new(state))
    }

    /// `PackageManagerInternal.grantImplicitAccess(user, null,
    /// appId(recipient), visible, true)`: the package of `visible` made
    /// visible to `recipient`'s.
    pub(super) fn grant_implicit_access(
        &self,
        recipient: i32,
        visible: i32,
    ) -> Result<(), Exception> {
        let _writer = self.model_writer.lock().unwrap();
        let (before, resolution) = self.packages()?;
        let query = crate::package::query::Query {
            state: &before,
            filter: &resolution.apps_filter,
            calling_uid: recipient,
        };
        let grants = crate::package::visibility_mutation::MakeUidVisible { recipient, visible }
            .grant(&query)
            .map_err(|e| {
                self.unimplemented_named("package", &format!("grantImplicitAccess: {}", e.0))
            })??;
        let Some(grants) = grants else { return Ok(()) };
        let mut state = (*before).clone();
        state.system.implicit_access = grants;
        self.user_states_changed(&before, Arc::new(state))
    }

    /// The isolated uid `uid` is free again (`freeIsolatedUidLocked`), and
    /// no longer has an owner.
    fn free_isolated_uid(&self, uid: u32) -> Result<(), Exception> {
        self.isolated_uids.lock().unwrap().used.remove(&uid);
        self.isolated_owners_changed(|owners| {
            owners.retain(|(isolated, _)| *isolated != uid as i32)
        })
    }

    /// Replaces the package model's state `before` with `state`, which
    /// differs from it in packages' user states only.
    fn user_states_changed(
        &self,
        before: &Arc<crate::package::model::State>,
        state: Arc<crate::package::model::State>,
    ) -> Result<(), Exception> {
        let mut model = self.model.lock().unwrap();
        self.resolver
            .user_states_changed(before, state.clone())
            .map_err(|_| Exception::new(EX_ILLEGAL_STATE, "package resolution: MIME groups"))?;
        *model = Some(state);
        Ok(())
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

    /// `getContentProviderExternal`, the shell's `content`: the provider
    /// with no client process.
    fn content_provider_external(
        &self,
        call: &Call<'_>,
        name: Option<&str>,
        user: i32,
    ) -> Result<Option<providers::Holder>, Exception> {
        self.external_provider_access(call)?;
        let uid = call.sender_euid as i32;
        let user = self.incoming_user(uid, user, false)?;
        match name {
            Some("settings") => self.settings_holder(),
            Some(name) => self.provider(None, uid, name, user, true),
            None => Ok(None),
        }
    }

    /// `ACCESS_CONTENT_PROVIDERS_EXTERNALLY`, which external references
    /// need.
    fn external_provider_access(&self, call: &Call<'_>) -> Result<(), Exception> {
        let permission = "android.permission.ACCESS_CONTENT_PROVIDERS_EXTERNALLY";
        if self.check_permission(Some(permission), call.sender_euid as i32)? != 0 {
            return Err(Exception::security(format!(
                "Permission Denial: getContentProviderExternal() from pid={}, uid={} requires {permission}",
                call.sender_pid, call.sender_euid
            )));
        }
        Ok(())
    }

    /// `ActivityManagerService.getContentProvider`: the `settings`
    /// provider the app host serves, else apps' providers.
    fn content_provider(
        &self,
        call: &Call<'_>,
        name: Option<&str>,
        user: i32,
        stable: bool,
    ) -> Result<Option<providers::Holder>, Exception> {
        let uid = call.sender_euid as i32;
        let Some(name) = name else {
            return Ok(None);
        };
        if name != SETTINGS_AUTHORITY {
            let user = self.incoming_user(uid, user, false)?;
            let client = self
                .apps
                .lock()
                .unwrap()
                .iter()
                .find(|(_, a)| a.pid == call.sender_pid && a.uid == call.sender_euid)
                .map(|(seq, _)| *seq)
                .ok_or_else(|| Exception::security(format!("Unable to find app for caller (pid={}) when getting content provider {name}", call.sender_pid)))?;
            return self.provider(Some(client), uid, name, user, stable);
        }
        self.settings_holder()
    }

    /// The `settings` provider's holder: the app host's, a system provider.
    fn settings_holder(&self) -> Result<Option<providers::Holder>, Exception> {
        let (state, resolution) = self.packages()?;
        let state_error = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let registered = resolution
            .components
            .provider_by_authority(SETTINGS_AUTHORITY)
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
        Ok(Some(providers::Holder {
            info,
            provider: Some(provider),
            connection: Some(connection),
            no_release_needed: true,
            local: false,
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
    /// `user` for `uid`: `None` when it may use it (by the provider's
    /// permissions, or a URI grant of it), else why not.
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
        // All users: the caller's own, with INTERACT_ACROSS_USERS_FULL.
        let user = if user == USER_ALL {
            if self.check_permission(Some(INTERACT_ACROSS_USERS_FULL), uid)? != 0 {
                return Err(Exception::security(format!(
                    "ContentProviderHelper: Neither user {uid} nor current process has {INTERACT_ACROSS_USERS_FULL}."
                )));
            }
            uid / PER_USER_RANGE as i32
        } else {
            user
        };
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
            for permission in [&pp.read_permission, &pp.write_permission]
                .into_iter()
                .flatten()
            {
                if check(Some(permission))? {
                    return Ok(None);
                }
            }
        }
        if self.authority_granted(uid, &cpi, user)? {
            return Ok(None);
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
        self.resolve_provider_as(authority, flags, user, 1000)
    }

    /// `resolveContentProvider` of `authority` with `flags`, as `uid` may
    /// see it.
    pub(super) fn resolve_provider_as(
        &self,
        authority: &str,
        flags: i64,
        user: i32,
        uid: i32,
    ) -> Result<Option<package::info::ProviderInfo>, Exception> {
        let (_, resolution) = self.packages()?;
        match resolution.resolve_content_provider(authority, flags, user, uid) {
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

    /// `CompatChanges.isChangeEnabled(change, uid)`: enabled for every
    /// package of the uid.
    pub(super) fn change_enabled(&self, change: i64, uid: i32) -> Result<bool, Exception> {
        let inputs = self.inputs()?;
        let (state, resolution) = self.packages()?;
        let query = crate::package::query::Query {
            state: &state,
            filter: &resolution.apps_filter,
            calling_uid: 1000,
        };
        let names = query.packages_for_uid(uid).map_err(|e| {
            Exception::new(
                EX_UNSUPPORTED_OPERATION,
                format!("getPackagesForUid: {}", e.0),
            )
        })?;
        let mut names = names.into_iter().flatten().flatten().peekable();
        if names.peek().is_none() {
            return Ok(inputs.compat.default_value(change));
        }
        Ok(
            names.all(|name| match inputs.packages.application_info(&name, 0) {
                Some(ai) => inputs
                    .compat
                    .is_enabled(change, Some(&name), ai.target_sdk_version),
                None => inputs.compat.default_value(change),
            }),
        )
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

    /// The global configuration (`ActivityTaskManagerService
    /// .getGlobalConfiguration`): the display's, the locales and fonts the
    /// settings hold, night as the Mac's appearance, at the current
    /// sequence number.
    fn global_configuration(&self) -> Result<config::Configuration, Exception> {
        let inputs = self.inputs()?;
        let display = self.display()?.0.clone();
        Ok(config::Configuration::of(
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
        ))
    }

    /// `ActivityTaskManagerService.updateGlobalConfigurationLocked` once
    /// what the global configuration is made of changed (the Mac's
    /// appearance): unless nothing changed, a new sequence number, each
    /// process told (`ConfigurationChangeItem`), then each activity
    /// (`RootWindowContainer.onConfigurationChanged`), and the
    /// broadcasts `broadcastGlobalConfigurationChanged` sends.
    fn update_global_configuration(&self) -> Result<(), Exception> {
        const FLAG_RECEIVER_REGISTERED_ONLY: i32 = 0x4000_0000;
        const FLAG_RECEIVER_REPLACE_PENDING: i32 = 0x2000_0000;
        const FLAG_RECEIVER_FOREGROUND: i32 = 0x1000_0000;
        const FLAG_RECEIVER_INCLUDE_BACKGROUND: i32 = 0x0100_0000;
        const FLAG_RECEIVER_VISIBLE_TO_INSTANT_APPS: i32 = 0x0020_0000;
        const FLAG_RECEIVER_OFFLOAD_FOREGROUND: i32 = 0x0000_0800;
        let Some(old) = self
            .apps
            .lock()
            .unwrap()
            .values()
            .find_map(|a| a.config.clone())
        else {
            // No process was bound with one: the next is bound with the new.
            return Ok(());
        };
        let changes = old.diff(&self.global_configuration()?);
        if changes == 0 {
            return Ok(());
        }
        self.config_seq.fetch_add(1, Ordering::Relaxed);
        let global = self.global_configuration()?;
        eprintln!("app host: Config changes={changes:x}");
        // `UsageStatsManagerInternal.reportConfigurationChange`.
        self.report_usage_stats_event(usage_events::Event {
            package: "android".into(),
            class: None,
            instance_id: 0,
            task_root_package: None,
            task_root_class: None,
            event_type: usage_events::CONFIGURATION_CHANGE,
            time_stamp: usage_stats::now_millis(),
            flags: 0,
            extra: usage_events::Extra::Configuration(global.to_proto()),
        });
        let bound: Vec<i64> = {
            let mut apps = self.apps.lock().unwrap();
            apps.iter_mut()
                .filter(|(_, a)| a.thread.is_some())
                .map(|(seq, a)| {
                    a.config = Some(global.clone());
                    *seq
                })
                .collect()
        };
        for start_seq in bound {
            let item: Box<dyn activity::Item> = Box::new(activity::ConfigurationChangeItem {
                config: global.clone(),
            });
            if let Err(e) = self.schedule(start_seq, vec![item]) {
                eprintln!("app host: configuration of start {start_seq}: {e}");
            }
        }
        let mut by_start: BTreeMap<i64, Vec<Box<dyn activity::Item>>> = BTreeMap::new();
        for (start_seq, item) in self.reconfigure_window_contexts(&global) {
            by_start.entry(start_seq).or_default().push(item);
        }
        for (start_seq, item) in
            self.reconfigure_all_activities(&global, |task| self.tasks.freeform_bounds(task))
        {
            by_start.entry(start_seq).or_default().push(item);
        }
        for (start_seq, items) in by_start {
            if let Err(e) = self.schedule(start_seq, items) {
                eprintln!("app host: activities of start {start_seq}: {e}");
            }
        }
        let flags = FLAG_RECEIVER_REGISTERED_ONLY
            | FLAG_RECEIVER_REPLACE_PENDING
            | FLAG_RECEIVER_FOREGROUND
            | FLAG_RECEIVER_VISIBLE_TO_INSTANT_APPS;
        self.broadcasts.send_from_system(
            self,
            kept::RawIntent::new("android.intent.action.CONFIGURATION_CHANGED", flags, &[]),
        );
        if changes & config::CONFIG_LOCALE != 0 {
            let flags = FLAG_RECEIVER_OFFLOAD_FOREGROUND
                | FLAG_RECEIVER_INCLUDE_BACKGROUND
                | FLAG_RECEIVER_VISIBLE_TO_INSTANT_APPS;
            self.broadcasts.send_from_system(
                self,
                kept::RawIntent::new("android.intent.action.LOCALE_CHANGED", flags, &[]),
            );
        }
        Ok(())
    }

    /// `bindApplication`'s call for `app`'s process, with its `providers`.
    fn bind_application(
        &self,
        app: &App,
        providers: Vec<package::info::ProviderInfo>,
    ) -> Result<(Parcel, config::Configuration), Exception> {
        let inputs = self.inputs()?;
        let state = |e: String| Exception::new(EX_ILLEGAL_STATE, e);
        let info = match &app.application_info {
            Some(info) => (**info).clone(),
            None => inputs
                .packages
                .application_info(&app.package, STOCK_PM_FLAGS)
                .ok_or_else(|| state(format!("no application info of {}", app.package)))?,
        };
        let config = self.global_configuration()?;
        let compat_info = config::CompatibilityInfo::of(&info, &config, inputs.device_density);
        let analog_clock_fps = inputs
            .packages
            .framework_integer("config_defaultAnalogClockSecondsHandFps")
            .ok_or_else(|| state("no config_defaultAnalogClockSecondsHandFps".into()))?;
        let core_settings =
            bind::core_settings(&inputs.settings, analog_clock_fps).map_err(state)?;
        let published = self.published.lock().unwrap();
        // An isolated process gets only those it may use (`getCommonServicesLocked`).
        let isolated = is_isolated(app.uid as i32);
        let services: Vec<(ParcelValue, ParcelValue)> = COMMON_SERVICES
            .iter()
            .filter(|name| !isolated || ISOLATED_COMMON_SERVICES.contains(name))
            .filter_map(|name| published.iter().find(|(n, _)| n == name))
            .map(|(name, binder)| {
                (
                    ParcelValue::String(Some(name.clone())),
                    ParcelValue::Binder(Some(*binder)),
                )
            })
            .collect();
        let target_sdk = info.target_sdk_version;
        let app_package = info.item.package_name.clone().unwrap_or_default();
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
            // `bindApplication`'s first argument is the process's name.
            package_name: Some(app.process.clone()),
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
            disabled_compat_changes: Some(inputs.compat.disabled_changes(&app_package, target_sdk)),
            loggable_compat_changes: Some(inputs.compat.loggable_changes(&app_package, target_sdk)),
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
pub(super) fn system_boolean(value: Option<&str>, default: bool) -> bool {
    match value {
        Some("1" | "y" | "yes" | "on" | "true") => true,
        Some("0" | "n" | "no" | "off" | "false") => false,
        _ => default,
    }
}

/// The string field `name` of a decoded parcelable, at any depth.
fn field<'v>(value: &'v crate::shadow::Value, name: &str) -> Option<&'v str> {
    let crate::shadow::Value::Fields(fields) = value else {
        return None;
    };
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
        [r.left, r.top, r.right, r.bottom]
            .iter()
            .for_each(|v| data.write_i32(*v));
    }
    data.write_bool(input.watch_outside);
    data.write_i32(input.owner as i32);
    data.write_i32(input.owner_pid);
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
fn hidden_api_policy(
    flags: i32,
    private_flags: i32,
    private_flags_ext: i32,
    setting: Option<&str>,
) -> i32 {
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
    let setting = setting
        .and_then(|v| v.parse::<i32>().ok())
        .filter(|p| (-1..=HIDDEN_API_ENFORCEMENT_ENABLED).contains(p));
    match setting {
        _ if allowed => HIDDEN_API_ENFORCEMENT_DISABLED,
        Some(policy) if policy >= 0 => policy,
        _ => HIDDEN_API_ENFORCEMENT_ENABLED,
    }
}

/// `ProcessList.computeGidsForProcess`: the package's permission gids,
/// its shared and cache gids, its user's, and those of its storage mode
/// and access.
fn process_gids(
    mut gids: Vec<u32>,
    uid: u32,
    mount_external: zygote::MountExternal,
    external_storage_access: bool,
) -> Vec<u32> {
    /// `Process.SDCARD_RW_GID`, `EXT_DATA_RW_GID`, `EXT_OBB_RW_GID`,
    /// `MEDIA_RW_GID`, `EXTERNAL_STORAGE_GID`.
    const SDCARD_RW_GID: u32 = 1015;
    const EXT_DATA_RW_GID: u32 = 1078;
    const EXT_OBB_RW_GID: u32 = 1079;
    const MEDIA_RW_GID: u32 = 1023;
    const EXTERNAL_STORAGE_GID: u32 = 1077;
    let (user, app_id) = (uid / PER_USER_RANGE, uid % PER_USER_RANGE);
    if app_id >= FIRST_APPLICATION_UID {
        gids.push(app_id - FIRST_APPLICATION_UID + FIRST_SHARED_APPLICATION_GID);
        gids.push(app_id - FIRST_APPLICATION_UID + FIRST_APPLICATION_CACHE_GID);
    }
    gids.push(user * PER_USER_RANGE + SHARED_USER_GID);
    if matches!(
        mount_external,
        zygote::MountExternal::AndroidWritable | zygote::MountExternal::PassThrough
    ) {
        gids.extend([
            user * PER_USER_RANGE + SDCARD_RW_GID,
            EXT_DATA_RW_GID,
            EXT_OBB_RW_GID,
        ]);
    }
    if mount_external == zygote::MountExternal::Installer {
        gids.push(EXT_OBB_RW_GID);
    }
    if mount_external == zygote::MountExternal::PassThrough {
        gids.push(MEDIA_RW_GID);
    }
    if external_storage_access {
        gids.push(EXTERNAL_STORAGE_GID);
    }
    gids
}

/// `Zygote.getMemorySafetyRuntimeFlags` for an app with no manifest
/// overrides, on a kernel without tagged pointers (the layer answers
/// `PR_GET_TAGGED_ADDR_CTRL` with `EINVAL`), which disables tagging.
fn memory_safety(compat: &compat::CompatConfig, package: &str, flags: i32, target_sdk: i32) -> i32 {
    let gwp_asan = if compat.is_enabled(GWP_ASAN, Some(package), target_sdk) {
        GWP_ASAN_LEVEL_ALWAYS
    } else if flags & FLAG_SYSTEM != 0 {
        GWP_ASAN_LEVEL_LOTTERY
    } else {
        GWP_ASAN_LEVEL_DEFAULT
    };
    let zero_init = if compat.is_enabled(NATIVE_HEAP_ZERO_INIT, Some(package), target_sdk) {
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
                let a = am::StartService::<kept::RawIntent>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let Some(intent) = a.service else {
                    return reply(Err(Exception::illegal_argument("service intent is null")));
                };
                let uid = call.sender_euid as i32;
                let started = self.0.start_service(
                    a.caller,
                    uid,
                    Some(call.sender_pid),
                    intent,
                    a.resolved_type,
                    a.require_foreground,
                    a.calling_package.as_deref(),
                    a.user_id,
                );
                reply(started.map(|name| {
                    let mut p = Parcel::new();
                    am::write_start_service_reply(
                        &mut p,
                        name.as_ref()
                            .map(|(p, c)| activity_start::Component(p, c))
                            .as_ref(),
                    );
                    p
                }))
            }
            am::STOP_SERVICE => {
                let a = am::StopService::<kept::RawIntent>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let Some(intent) = a.service else {
                    return reply(Err(Exception::illegal_argument("service intent is null")));
                };
                reply(
                    self.0
                        .stop_service(
                            call.sender_euid as i32,
                            call.sender_pid,
                            intent,
                            a.resolved_type,
                            a.user_id,
                        )
                        .map(|r| {
                            let mut p = Parcel::new();
                            am::write_stop_service_reply(&mut p, r);
                            p
                        }),
                )
            }
            am::STOP_SERVICE_TOKEN => {
                let a = am::StopServiceToken::<services::Name>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let stopped = self.0.stop_service_token(a.class_name, a.token, a.start_id);
                let mut p = Parcel::new();
                am::write_stop_service_token_reply(&mut p, stopped);
                Ok(p)
            }
            // Read as written, its notification held as NotificationManager
            // holds one.
            am::SET_SERVICE_FOREGROUND => {
                let r = &mut call.data;
                r.enforce_interface(am::DESCRIPTOR).map_err(|_| BAD_VALUE)?;
                let name =
                    aim_service_aidl::read_typed::<services::Name>(r).map_err(|_| BAD_VALUE)?;
                let token = r.read_binder().map_err(|_| BAD_VALUE)?;
                let id = r.read_i32().map_err(|_| BAD_VALUE)?;
                let notification = match r.read_i32().map_err(|_| BAD_VALUE)? {
                    0 => None,
                    _ => match notification_service::Incoming::read(r, &self.0.process) {
                        Ok(n) => Some(n),
                        Err(e) => return reply(Err(e)),
                    },
                };
                let flags = r.read_i32().map_err(|_| BAD_VALUE)?;
                let service_type = r.read_i32().map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .set_service_foreground(
                            call.sender_euid as i32,
                            name,
                            token,
                            id,
                            notification,
                            flags,
                            service_type,
                        )
                        .map(|()| {
                            let mut p = Parcel::new();
                            am::write_set_service_foreground_reply(&mut p);
                            p
                        }),
                )
            }
            am::GET_FOREGROUND_SERVICE_TYPE => {
                let a = am::GetForegroundServiceType::<services::Name>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                am::write_get_foreground_service_type_reply(
                    &mut p,
                    self.0
                        .foreground_service_type(call.sender_euid as i32, a.class_name, a.token),
                );
                Ok(p)
            }
            am::BIND_SERVICE | am::BIND_SERVICE_INSTANCE => {
                let a = if call.code == am::BIND_SERVICE {
                    let a = am::BindService::<kept::RawIntent>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                    am::BindServiceInstance {
                        caller: a.caller,
                        token: a.token,
                        service: a.service,
                        resolved_type: a.resolved_type,
                        connection: a.connection,
                        flags: a.flags,
                        instance_name: None,
                        calling_package: a.calling_package,
                        user_id: a.user_id,
                    }
                } else {
                    am::BindServiceInstance::<kept::RawIntent>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?
                };
                let Some(intent) = a.service else {
                    return reply(Err(Exception::illegal_argument("service intent is null")));
                };
                reply(
                    self.0
                        .bind_service(
                            a.caller,
                            call.sender_euid as i32,
                            call.sender_pid,
                            a.token,
                            intent,
                            a.resolved_type,
                            a.connection,
                            a.flags,
                            a.instance_name,
                            a.calling_package,
                            a.user_id,
                        )
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
            am::UPDATE_SERVICE_GROUP => {
                let a = am::UpdateServiceGroup::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.update_service_group(a.connection).map(|()| {
                    let mut p = Parcel::new();
                    am::write_update_service_group_reply(&mut p);
                    p
                }))
            }
            am::PUBLISH_SERVICE => {
                let a = am::PublishService::<kept::RawIntent>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.0.publish_service(a.token, a.intent, a.service);
                let mut p = Parcel::new();
                am::write_publish_service_reply(&mut p);
                Ok(p)
            }
            am::UNBIND_FINISHED => {
                let a = am::UnbindFinished::<kept::RawIntent>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.0.unbind_finished(a.token, a.service);
                let mut p = Parcel::new();
                am::write_unbind_finished_reply(&mut p);
                Ok(p)
            }
            am::GET_INTENT_SENDER | am::GET_INTENT_SENDER_WITH_FEATURE => {
                let req = if call.code == am::GET_INTENT_SENDER {
                    let a = am::GetIntentSender::<kept::RawIntent, kept::RawBundle>::read(
                        &mut call.data,
                    )
                    .map_err(|_| BAD_VALUE)?;
                    pending_intents::Request {
                        kind: a.r#type,
                        package: a.package_name,
                        feature: None,
                        token: a.token,
                        result_who: a.result_who,
                        request_code: a.request_code,
                        intents: a.intents,
                        resolved_types: a.resolved_types,
                        flags: a.flags,
                        options: a.options,
                        user: a.user_id,
                    }
                } else {
                    let a =
                        am::GetIntentSenderWithFeature::<kept::RawIntent, kept::RawBundle>::read(
                            &mut call.data,
                        )
                        .map_err(|_| BAD_VALUE)?;
                    pending_intents::Request {
                        kind: a.r#type,
                        package: a.package_name,
                        feature: a.feature_id,
                        token: a.token,
                        result_who: a.result_who,
                        request_code: a.request_code,
                        intents: a.intents,
                        resolved_types: a.resolved_types,
                        flags: a.flags,
                        options: a.options,
                        user: a.user_id,
                    }
                };
                reply(
                    self.0
                        .get_intent_sender(call.sender_euid as i32, req)
                        .map(|sender| {
                            let mut p = Parcel::new();
                            am::write_get_intent_sender_with_feature_reply(&mut p, sender);
                            p
                        }),
                )
            }
            am::CANCEL_INTENT_SENDER => {
                let a = am::CancelIntentSender::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .cancel_intent_sender(call.sender_euid as i32, a.sender)
                        .map(|()| {
                            let mut p = Parcel::new();
                            am::write_cancel_intent_sender_reply(&mut p);
                            p
                        }),
                )
            }
            am::GET_INFO_FOR_INTENT_SENDER => {
                let a = am::GetInfoForIntentSender::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                am::write_get_info_for_intent_sender_reply(
                    &mut p,
                    Some(&self.0.intent_sender_info(a.sender)),
                );
                Ok(p)
            }
            am::REGISTER_INTENT_SENDER_CANCEL_LISTENER_EX => {
                let a = am::RegisterIntentSenderCancelListenerEx::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                am::write_register_intent_sender_cancel_listener_ex_reply(
                    &mut p,
                    self.0.listen_for_cancel(a.sender, a.receiver),
                );
                Ok(p)
            }
            am::UNREGISTER_INTENT_SENDER_CANCEL_LISTENER => {
                let a = am::UnregisterIntentSenderCancelListener::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                self.0.unlisten_for_cancel(a.sender, a.receiver);
                let mut p = Parcel::new();
                am::write_unregister_intent_sender_cancel_listener_reply(&mut p);
                Ok(p)
            }
            am::GET_INTENT_FOR_INTENT_SENDER => {
                let a =
                    am::GetIntentForIntentSender::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .intent_for_intent_sender(call.sender_euid as i32, a.sender)
                        .map(|intent| {
                            let mut p = Parcel::new();
                            am::write_get_intent_for_intent_sender_reply(&mut p, intent.as_ref());
                            p
                        }),
                )
            }
            am::SEND_INTENT_SENDER => {
                let a =
                    am::SendIntentSender::<kept::RawIntent, kept::RawBundle>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .send_intent_sender(
                            call.sender_euid as i32,
                            a.caller,
                            a.target,
                            a.code,
                            a.intent,
                            a.resolved_type,
                            a.finished_receiver,
                            a.required_permission,
                            a.options,
                        )
                        .map(|r| {
                            let mut p = Parcel::new();
                            am::write_send_intent_sender_reply(&mut p, r);
                            p
                        }),
                )
            }
            // One-way.
            am::SERVICE_DONE_EXECUTING => {
                let a = am::ServiceDoneExecuting::<kept::RawIntent>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                if let Err(e) = self
                    .0
                    .service_done_executing(a.token, a.r#type, a.start_id, a.res)
                {
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
                reply(
                    self.0
                        .content_provider(call, args.name.as_deref(), args.user_id, args.stable)
                        .map(|holder| {
                            let mut p = Parcel::new();
                            am::write_get_content_provider_reply(&mut p, holder.as_ref());
                            p
                        }),
                )
            }
            am::GET_CONTENT_PROVIDER_EXTERNAL => {
                let args =
                    am::GetContentProviderExternal::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .content_provider_external(call, args.name.as_deref(), args.user_id)
                        .map(|holder| {
                            let mut p = Parcel::new();
                            am::write_get_content_provider_external_reply(&mut p, holder.as_ref());
                            p
                        }),
                )
            }
            am::REMOVE_CONTENT_PROVIDER_EXTERNAL_AS_USER => {
                am::RemoveContentProviderExternalAsUser::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                reply(self.0.external_provider_access(call).map(|()| {
                    let mut p = Parcel::new();
                    am::write_remove_content_provider_external_as_user_reply(&mut p);
                    p
                }))
            }
            am::CHECK_URI_PERMISSION => {
                let args =
                    am::CheckUriPermission::<content_service::ParcelUri>::read(&mut call.data)
                        .map_err(|_| BAD_VALUE)?;
                let Some(content_service::ParcelUri(uri)) = args.uri else {
                    return reply(Err(Exception::new(EX_NULL_POINTER, "uri")));
                };
                reply(
                    self.0
                        .check_uri_permission(call, uri, args.uid, args.mode, args.user_id)
                        .map(|result| {
                            let mut p = Parcel::new();
                            am::write_check_uri_permission_reply(&mut p, result);
                            p
                        }),
                )
            }
            am::GRANT_URI_PERMISSION => {
                let a = am::GrantUriPermission::<content_service::ParcelUri>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let uri = a.uri.map(|content_service::ParcelUri(uri)| uri);
                reply(
                    self.0
                        .grant_uri_permission(call, a.caller, a.target_pkg, uri, a.mode, a.user_id)
                        .map(|()| {
                            let mut p = Parcel::new();
                            am::write_grant_uri_permission_reply(&mut p);
                            p
                        }),
                )
            }
            am::REVOKE_URI_PERMISSION => {
                let a = am::RevokeUriPermission::<content_service::ParcelUri>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let uri = a.uri.map(|content_service::ParcelUri(uri)| uri);
                reply(
                    self.0
                        .revoke_uri_permission(call, a.caller, a.target_pkg, uri, a.mode, a.user_id)
                        .map(|()| {
                            let mut p = Parcel::new();
                            am::write_revoke_uri_permission_reply(&mut p);
                            p
                        }),
                )
            }
            // `CachedAppOptimizer.binderError`: logged; the app host freezes
            // no process (one-way).
            am::FROZEN_BINDER_TRANSACTION_DETECTED => {
                let a = am::FrozenBinderTransactionDetected::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let name = self
                    .0
                    .apps
                    .lock()
                    .unwrap()
                    .values()
                    .find(|app| app.pid == a.debug_pid)
                    .map(|app| app.process.clone());
                eprintln!(
                    "app host: pid {} {} sent binder code {} with flags {} to frozen apps and got error {}",
                    a.debug_pid,
                    name.as_deref().unwrap_or("null"),
                    a.code,
                    a.flags,
                    a.err
                );
                Ok(Parcel::new())
            }
            // No monkey runs (`mUserIsMonkey`, a monkey controller).
            am::IS_USER_A_MONKEY => {
                am::IsUserAMonkey::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                am::write_is_user_a_monkey_reply(&mut p, false);
                Ok(p)
            }
            am::GET_PACKAGE_PROCESS_STATE => {
                let a = am::GetPackageProcessState::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .package_process_state(
                            call,
                            a.package_name.as_deref(),
                            a.calling_package.as_deref(),
                        )
                        .map(|state| {
                            let mut p = Parcel::new();
                            am::write_get_package_process_state_reply(&mut p, state);
                            p
                        }),
                )
            }
            am::ADD_PACKAGE_DEPENDENCY => {
                let a = am::AddPackageDependency::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .add_package_dependency(call, a.package_name)
                        .map(|()| {
                            let mut p = Parcel::new();
                            am::write_add_package_dependency_reply(&mut p);
                            p
                        }),
                )
            }
            am::IS_BACKGROUND_RESTRICTED => {
                let a = am::IsBackgroundRestricted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .background_restricted(call, a.package_name)
                        .map(|restricted| {
                            let mut p = Parcel::new();
                            am::write_is_background_restricted_reply(&mut p, restricted);
                            p
                        }),
                )
            }
            am::IS_USER_RUNNING => {
                let a = am::IsUserRunning::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.user_running(call, a.userid, a.flags).map(|running| {
                    let mut p = Parcel::new();
                    am::write_is_user_running_reply(&mut p, running);
                    p
                }))
            }
            // `UserController.getCurrentUserIdChecked`: user 0, with
            // INTERACT_ACROSS_USERS or its FULL form.
            am::GET_CURRENT_USER_ID => {
                am::GetCurrentUserId::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let uid = call.sender_euid as i32;
                let held = |p: &str| self.0.check_permission(Some(p), uid).map(|r| r == 0);
                reply(held("android.permission.INTERACT_ACROSS_USERS").and_then(|a| Ok(a || held("android.permission.INTERACT_ACROSS_USERS_FULL")?)).and_then(|held| {
                    if !held {
                        return Err(Exception::security(format!(
                            "Permission Denial: getCurrentUser() from pid={}, uid={uid} requires android.permission.INTERACT_ACROSS_USERS",
                            call.sender_pid
                        )));
                    }
                    let mut p = Parcel::new();
                    am::write_get_current_user_id_reply(&mut p, 0);
                    Ok(p)
                }))
            }
            am::HANDLE_INCOMING_USER => {
                let a = am::HandleIncomingUser::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .handle_incoming_user(
                            a.calling_uid,
                            a.user_id,
                            a.allow_all,
                            a.require_full,
                            a.name.as_deref(),
                        )
                        .map(|user| {
                            let mut p = Parcel::new();
                            am::write_handle_incoming_user_reply(&mut p, user);
                            p
                        }),
                )
            }
            // `handleApplicationWtf`: logged; fatal for an app when
            // `wtf_is_fatal` is set (the image is not `eng`).
            am::HANDLE_APPLICATION_WTF => {
                let a = am::HandleApplicationWtf::<CrashInfo>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let Some(crash) = a.crash_info else {
                    return reply(Err(Exception::new(EX_NULL_POINTER, "crashInfo")));
                };
                let app = self.0.app_of_thread(a.app);
                let process = match (&a.app, &app) {
                    (None, _) => "system_server".to_string(),
                    (Some(_), None) => "unknown".to_string(),
                    (Some(_), Some((seq, ..))) => self
                        .0
                        .apps
                        .lock()
                        .unwrap()
                        .get(seq)
                        .map_or("unknown".into(), |p| p.process.clone()),
                };
                eprintln!(
                    "app host: WTF in {process} (pid {}): {}: {}",
                    call.sender_pid,
                    a.tag.as_deref().unwrap_or("null"),
                    crash.exception_message.as_deref().unwrap_or("unknown")
                );
                let fatal = self.0.inputs().is_ok_and(|i| {
                    i.settings
                        .get(settings::Table::Global, "wtf_is_fatal")
                        .is_some_and(|v| v != "0")
                });
                let killed = !a.system && app.is_some() && fatal;
                let mut p = Parcel::new();
                am::write_handle_application_wtf_reply(&mut p, killed);
                Ok(p)
            }
            // `logFgsApiBegin`, `logFgsApiEnd`: statsd's, which the app host
            // does not run (one-way).
            am::LOG_FGS_API_BEGIN | am::LOG_FGS_API_END => Ok(Parcel::new()),
            am::GET_MEMORY_INFO => {
                am::GetMemoryInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.memory_info().map(|info| {
                    let mut p = Parcel::new();
                    am::write_get_memory_info_reply(
                        &mut p,
                        &am::GetMemoryInfoReply {
                            out_info: Some(info),
                        },
                    );
                    p
                }))
            }
            am::REGISTER_UID_OBSERVER => {
                let args = am::RegisterUidObserver::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .register_uid_observer(
                            call,
                            args.observer,
                            args.which,
                            args.cutpoint,
                            args.calling_package.as_deref(),
                        )
                        .map(|()| {
                            let mut p = Parcel::new();
                            am::write_register_uid_observer_reply(&mut p);
                            p
                        }),
                )
            }
            am::REGISTER_UID_OBSERVER_FOR_UIDS => {
                let args =
                    am::RegisterUidObserverForUids::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .register_uid_observer_for_uids(
                            call,
                            args.observer,
                            args.which,
                            args.cutpoint,
                            args.calling_package.as_deref(),
                            args.uids,
                        )
                        .map(|token| {
                            let mut p = Parcel::new();
                            am::write_register_uid_observer_for_uids_reply(&mut p, Some(token));
                            p
                        }),
                )
            }
            am::ADD_UID_TO_OBSERVER => {
                let args = am::AddUidToObserver::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .change_observer_uids(
                            call,
                            args.observer_token,
                            args.calling_package.as_deref(),
                            args.uid,
                            true,
                        )
                        .map(|()| {
                            let mut p = Parcel::new();
                            am::write_add_uid_to_observer_reply(&mut p);
                            p
                        }),
                )
            }
            am::REMOVE_UID_FROM_OBSERVER => {
                let args =
                    am::RemoveUidFromObserver::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .change_observer_uids(
                            call,
                            args.observer_token,
                            args.calling_package.as_deref(),
                            args.uid,
                            false,
                        )
                        .map(|()| {
                            let mut p = Parcel::new();
                            am::write_remove_uid_from_observer_reply(&mut p);
                            p
                        }),
                )
            }
            am::UNREGISTER_UID_OBSERVER => {
                let args =
                    am::UnregisterUidObserver::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.0.unregister_uid_observer(args.observer);
                let mut p = Parcel::new();
                am::write_unregister_uid_observer_reply(&mut p);
                Ok(p)
            }
            am::IS_UID_ACTIVE => {
                let args = am::IsUidActive::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .is_uid_active(call, args.uid, args.calling_package.as_deref())
                        .map(|active| {
                            let mut p = Parcel::new();
                            am::write_is_uid_active_reply(&mut p, active);
                            p
                        }),
                )
            }
            am::GET_UID_PROCESS_STATE => {
                let args = am::GetUidProcessState::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .uid_process_state(call, args.uid, args.calling_package.as_deref())
                        .map(|state| {
                            let mut p = Parcel::new();
                            am::write_get_uid_process_state_reply(&mut p, state);
                            p
                        }),
                )
            }
            am::REF_CONTENT_PROVIDER => {
                let args = am::RefContentProvider::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .ref_provider(args.connection, args.stable_delta, args.unstable_delta)
                        .map(|alive| {
                            let mut p = Parcel::new();
                            am::write_ref_content_provider_reply(&mut p, alive);
                            p
                        }),
                )
            }
            am::REMOVE_CONTENT_PROVIDER => {
                let args =
                    am::RemoveContentProvider::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.0.remove_provider(args.connection, args.stable) {
                    eprintln!("app host: removeContentProvider: {}", e.message);
                }
                Ok(Parcel::new())
            }
            // The provider's process's death, which its thread's death
            // notice reports, removes it; a provider still alive stays.
            am::UNSTABLE_PROVIDER_DIED => {
                am::UnstableProviderDied::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let mut p = Parcel::new();
                am::write_unstable_provider_died_reply(&mut p);
                Ok(p)
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
            am::GET_SERVICES => {
                let a = am::GetServices::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .running_services(call.sender_euid as i32, a.max_num)
                        .map(|list| {
                            let mut p = Parcel::new();
                            am::write_get_services_reply(&mut p, Some(&list));
                            p
                        }),
                )
            }
            am::GET_PROCESS_MEMORY_INFO => {
                let a = am::GetProcessMemoryInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .process_memory_info(call.sender_euid as i32, a.pids)
                        .map(|infos| {
                            let mut p = Parcel::new();
                            am::write_get_process_memory_info_reply(&mut p, Some(&infos));
                            p
                        }),
                )
            }
            am::GET_MY_MEMORY_STATE => {
                am::GetMyMemoryState::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.my_memory_state(call).map(|info| {
                    let mut p = Parcel::new();
                    am::write_get_my_memory_state_reply(
                        &mut p,
                        &am::GetMyMemoryStateReply {
                            out_info: Some(info),
                        },
                    );
                    p
                }))
            }
            am::REGISTER_RECEIVER => {
                let a = am::RegisterReceiver::<broadcasts::ParcelFilter>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let args = broadcasts::Registration {
                    caller: a.caller,
                    caller_package: a.caller_package,
                    receiver: a.receiver,
                    filter: a.filter,
                    permission: a.required_permission,
                    user: a.user_id,
                    flags: a.flags,
                };
                reply(
                    self.0
                        .broadcasts
                        .register(&self.0, call, args)
                        .map(|sticky| {
                            let mut p = Parcel::new();
                            am::write_register_receiver_reply(&mut p, sticky.as_ref());
                            p
                        }),
                )
            }
            am::REGISTER_RECEIVER_WITH_FEATURE => {
                let a = am::RegisterReceiverWithFeature::<broadcasts::ParcelFilter>::read(
                    &mut call.data,
                )
                .map_err(|_| BAD_VALUE)?;
                let args = broadcasts::Registration {
                    caller: a.caller,
                    caller_package: a.caller_package,
                    receiver: a.receiver,
                    filter: a.filter,
                    permission: a.required_permission,
                    user: a.user_id,
                    flags: a.flags,
                };
                reply(
                    self.0
                        .broadcasts
                        .register(&self.0, call, args)
                        .map(|sticky| {
                            let mut p = Parcel::new();
                            am::write_register_receiver_with_feature_reply(&mut p, sticky.as_ref());
                            p
                        }),
                )
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
                let a = am::BroadcastIntentWithFeature::<kept::RawIntent, kept::RawBundle>::read(
                    &mut call.data,
                )
                .map_err(|_| BAD_VALUE)?;
                reply(self.0.broadcasts.broadcast(&self.0, call, a).map(|result| {
                    let mut p = Parcel::new();
                    am::write_broadcast_intent_with_feature_reply(&mut p, result);
                    p
                }))
            }
            am::UNBROADCAST_INTENT => {
                let a = am::UnbroadcastIntent::<kept::RawIntent>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                reply(
                    self.0
                        .broadcasts
                        .unbroadcast(&self.0, call, a.intent, a.user_id)
                        .map(|()| {
                            let mut p = Parcel::new();
                            am::write_unbroadcast_intent_reply(&mut p);
                            p
                        }),
                )
            }
            // One-way.
            am::FINISH_RECEIVER => {
                let a = am::FinishReceiver::<kept::RawBundle>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                if let Err(e) = self.0.broadcasts.finish(&self.0, a) {
                    eprintln!("app host: finishReceiver: {e:?}");
                }
                Ok(Parcel::new())
            }
            // `handleApplicationCrash`: the crash is the process's last
            // act (`RuntimeInit` kills it after the call); it is logged.
            am::HANDLE_APPLICATION_CRASH => {
                let a = am::HandleApplicationCrash::<CrashInfo>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let Some(crash) = a.crash_info else {
                    return reply(Err(Exception::new(EX_NULL_POINTER, "crashInfo")));
                };
                eprintln!(
                    "app host: process {} (uid {}) crashed",
                    call.sender_pid, call.sender_euid
                );
                let native = crash.exception_class_name.as_deref() == Some("Native crash");
                self.0
                    .app_crashed(call.sender_pid, call.sender_euid, native);
                let mut p = Parcel::new();
                am::write_handle_application_crash_reply(&mut p);
                Ok(p)
            }
            am::GET_HISTORICAL_PROCESS_EXIT_REASONS => {
                let a = am::GetHistoricalProcessExitReasons::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                let exits = self.0.historical_process_exit_reasons(
                    call,
                    a.package_name.as_deref(),
                    a.pid,
                    a.max_num,
                    a.user_id,
                );
                reply(exits.map(|exits| {
                    let slice = crate::package::list_slice::Slice::new(
                        self.0.process.clone(),
                        "android.app.ApplicationExitInfo",
                        exits,
                    );
                    let mut p = Parcel::new();
                    am::write_get_historical_process_exit_reasons_reply(&mut p, Some(&slice));
                    p
                }))
            }
            am::SET_PROCESS_STATE_SUMMARY => {
                let a = am::SetProcessStateSummary::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(self.0.set_process_state_summary(call, a.state).map(|()| {
                    let mut p = Parcel::new();
                    am::write_set_process_state_summary_reply(&mut p);
                    p
                }))
            }
            am::REGISTER_STRICT_MODE_CALLBACK => {
                let a =
                    am::RegisterStrictModeCallback::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.0
                    .register_strict_mode_callback(call.sender_pid, a.binder);
                let mut p = Parcel::new();
                am::write_register_strict_mode_callback_reply(&mut p);
                Ok(p)
            }
            code => reply(Err(self.0.unimplemented("activity", code))),
        }
    }
}

/// `ApplicationErrorReport.ParcelableCrashInfo`, of which the app host
/// reads the exception's class and message.
struct CrashInfo {
    exception_class_name: Option<String>,
    exception_message: Option<String>,
}

impl aim_service_aidl::ReadParcelable for CrashInfo {
    fn read_from(
        r: &mut aim_binder_host::parcel::Reader<'_>,
    ) -> aim_binder_host::parcel::Result<Self> {
        // The handler's and the exception's class, the message, where it
        // was thrown, the stack and the tag.
        r.read_string16()?;
        let exception_class_name = r.read_string16()?;
        let exception_message = r.read_string16()?;
        for _ in 0..3 {
            r.read_string16()?;
        }
        r.read_i32()?;
        r.read_string16()?;
        r.read_string16()?;
        Ok(CrashInfo {
            exception_class_name,
            exception_message,
        })
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
            call.data
                .enforce_interface(CONTROL_DESCRIPTOR)
                .map_err(|_| BAD_VALUE)?;
            let Some(Binder::Handle(agent)) = call.data.read_binder().map_err(|_| BAD_VALUE)?
            else {
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
                // UserManagerService's construction: the system user
                // recorded at first boot.
                if let Err(e) = host.ensure_system_user() {
                    eprintln!("app host: user: the system user: {e}");
                }
                if let Err(e) = input_manager::watch_key_repeat(&host) {
                    eprintln!("app host: input: key repeat settings: {}", e.message);
                }
                // The Mac's appearance is the configuration's night mode, as
                // UiModeManagerService's is.
                let weak = Arc::downgrade(&host);
                crate::uimode::mac::watch(Box::new(move |_| {
                    if let Some(host) = weak.upgrade()
                        && let Err(e) = host.update_global_configuration()
                    {
                        eprintln!("app host: the global configuration: {e:?}");
                    }
                }));
                // audioserver starts with its HAL, which asks the Mac for
                // its devices: apart from the storage's start.
                let audio = host.clone();
                std::thread::spawn(move || {
                    if let Err(e) = audio.start_audio() {
                        eprintln!("app host: audio: {e}");
                    }
                });
                // The battery service waits for the health HAL, which init
                // starts with the HALs.
                if let Some(battery) = host.battery_service.get().cloned() {
                    std::thread::spawn(move || {
                        if let Err(e) = battery.start() {
                            eprintln!("app host: battery: {e}");
                        }
                    });
                }
                if let Err(e) = host.start_storage() {
                    eprintln!("app host: external storage: {e}");
                }
                // Apps' storage is mounted: the persisted jobs are read and
                // run, as at JobSchedulerService's
                // `PHASE_THIRD_PARTY_APPS_CAN_START`.
                if let Some(jobs) = host.job_scheduler.get() {
                    jobs.start();
                }
                // StatsCompanionService's `PHASE_THIRD_PARTY_APPS_CAN_START`
                // and `PHASE_BOOT_COMPLETED`: the app host's boot completes
                // with its start.
                if let Some(stats) = host.stats.get() {
                    stats.system_ready();
                    stats.boot_completed();
                }
                // BluetoothService's `onUserStarting` and `onUserUnlocking`
                // of the system user, once apps' storage is mounted.
                if let Some(bluetooth) = host.bluetooth.get() {
                    bluetooth.on_user_starting();
                    bluetooth.on_user_unlocking();
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
            Some((package, class)) if class.starts_with('.') => {
                (package, format!("{package}{class}"))
            }
            Some((package, class)) => (package, class.to_string()),
            None => match self
                .0
                .launch_activity_of(&component, call.sender_euid as i32)
            {
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

    /// As `IsolatedUidRange.allocateIsolatedUidLocked`: in turn through
    /// the range, per user, skipping uids in use, none when all are.
    #[test]
    fn isolated_uids_are_allocated_in_turn() {
        let mut uids = IsolatedUids {
            used: HashSet::new(),
            next: FIRST_ISOLATED_UID,
        };
        assert_eq!(uids.allocate(0), Some(99_000));
        assert_eq!(uids.allocate(10), Some(1_099_001));
        uids.used.insert(99_002);
        assert_eq!(uids.allocate(0), Some(99_003));
        uids.next = LAST_ISOLATED_UID;
        assert_eq!(uids.allocate(0), Some(99_999));
        assert_eq!(uids.allocate(0), Some(99_001));
        uids.used.extend(99_000..=99_999);
        assert_eq!(uids.allocate(0), None);
        assert_eq!(uids.allocate(10), Some(1_099_002));
    }

    #[test]
    fn a_package_s_state_is_its_lowest_process_s() {
        let deps = vec!["com.lib".to_string()];
        let processes = [
            ("com.a", &[][..], PROCESS_STATE_NONEXISTENT),
            ("com.b", &deps[..], PROCESS_STATE_TOP),
            ("com.c", &[][..], 6),
        ];
        let state = |package| package_process_state(processes.iter().copied(), package);
        assert_eq!(state(Some("com.lib")), PROCESS_STATE_TOP);
        assert_eq!(state(Some("com.c")), 6);
        assert_eq!(state(Some("com.a")), PROCESS_STATE_NONEXISTENT);
        assert_eq!(state(Some("com.none")), PROCESS_STATE_NONEXISTENT);
        assert_eq!(state(None), PROCESS_STATE_NONEXISTENT);
    }

    #[test]
    fn the_fuse_daemon_has_the_lower_filesystem_gids() {
        let gids = process_gids(
            vec![3007],
            10_208,
            zygote::MountExternal::PassThrough,
            false,
        );
        assert_eq!(gids, [3007, 50_208, 20_208, 9997, 1015, 1078, 1079, 1023]);
        assert_eq!(
            process_gids(vec![], 10_213, zygote::MountExternal::Default, true),
            [50_213, 20_213, 9997, 1077]
        );
    }

    #[test]
    fn system_booleans_default_when_unset_or_empty() {
        assert!(
            system_boolean(None, true)
                && system_boolean(Some(""), true)
                && system_boolean(Some("on"), false)
        );
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
            memory_safety(&compat, "a", 0, 35),
            GWP_ASAN_LEVEL_ALWAYS | NATIVE_HEAP_ZERO_INIT_ENABLED
        );
        assert_eq!(instruction_set("arm64-v8a"), "arm64");
    }
}
