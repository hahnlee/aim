//! The app host (docs/app-host.md): ActivityManager's side of an app's
//! process, native, for the app host image, which has no system_server.
//! It starts an app's process through zygote, as `ProcessList` does, and
//! takes the process's `attachApplication`.
//!
//! The process's start arguments are `ProcessList.startProcessLocked`'s
//! at `android-16.0.0_r1`, from the state PackageManager last persisted
//! (`packages.xml`, `packages.list`).

pub mod compat;
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
use aim_service_aidl::android_app_iactivitymanager as am;

use crate::package;

/// The control service's binder name and descriptor.
pub const CONTROL: &str = "aim.app_host";
const CONTROL_DESCRIPTOR: &str = "dev.aim.IAppHost";
/// `int startProcess(String packageName)`: the started process's pid.
const START_PROCESS: u32 = 1;

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

/// What the app host reads, set once guest-init knows it.
struct Inputs {
    /// The guest's `/data`.
    data: PathBuf,
    compat: compat::CompatConfig,
    connect: Connect,
}

/// A process started and not yet attached, or attached.
struct App {
    package: String,
    uid: u32,
    pid: i32,
    started: Instant,
    thread: Option<Strong>,
}

pub struct AppHost {
    process: Arc<LocalProcess>,
    inputs: OnceLock<Inputs>,
    zygote: Mutex<Option<zygote::Session>>,
    next_seq: AtomicI64,
    apps: Mutex<HashMap<i64, App>>,
}

impl AppHost {
    pub fn new(process: Arc<LocalProcess>) -> Arc<Self> {
        Arc::new(AppHost {
            process,
            inputs: OnceLock::new(),
            zygote: Mutex::new(None),
            next_seq: AtomicI64::new(1),
            apps: Mutex::new(HashMap::new()),
        })
    }

    /// The image below `root`, the guest's `/data` at `data`, and how the
    /// zygote relay is reached.
    pub fn configure(
        &self,
        root: &Path,
        data: &Path,
        sdk: i32,
        codename: &str,
        connect: Connect,
    ) -> Result<(), String> {
        let compat = compat::CompatConfig::load(root, sdk, codename)?;
        self.inputs
            .set(Inputs {
                data: data.to_path_buf(),
                compat,
                connect,
            })
            .map_err(|_| "app host already configured".to_string())
    }

    /// The services it publishes, by binder name.
    pub fn services(self: &Arc<Self>) -> Vec<(&'static str, Arc<dyn Service>)> {
        vec![
            (
                "activity",
                Arc::new(Activity(self.clone())) as Arc<dyn Service>,
            ),
            (CONTROL, Arc::new(Control(self.clone()))),
        ]
    }

    fn inputs(&self) -> Result<&Inputs, Exception> {
        self.inputs
            .get()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "app host not configured"))
    }

    /// `ProcessList.startProcessLocked` for an activity of `name`'s
    /// default process, user 0.
    fn start_process(&self, name: &str) -> Result<i32, Exception> {
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
            // External storage has no owner in the app host yet
            // (docs/app-host.md, "Not yet").
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
                let stream = (inputs.connect)()
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
                thread: None,
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
        app.thread = Some(strong);
        Ok(())
    }
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
            _ => Err(UNKNOWN_TRANSACTION),
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
        if call.code != START_PROCESS {
            return Err(UNKNOWN_TRANSACTION);
        }
        // Root and the shell, as `am start` is theirs.
        if !matches!(call.sender_euid, 0 | 2000) {
            return reply(Err(Exception::security("startProcess is the shell's")));
        }
        let name = call
            .data
            .enforce_interface(CONTROL_DESCRIPTOR)
            .and_then(|()| call.data.read_string16())
            .map_err(|_| BAD_VALUE)?;
        let Some(name) = name else {
            return reply(Err(Exception::new(EX_NULL_POINTER, "packageName")));
        };
        reply(self.0.start_process(&name).map(|pid| {
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
