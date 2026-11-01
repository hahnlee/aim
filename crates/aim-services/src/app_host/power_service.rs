//! `power`: PowerManagerService for the app host (`PowerManagerService`
//! at `android-16.0.0_r1`, ADR 0013): apps' wake locks, with their
//! permissions, death and callbacks, and the suspend blockers they hold
//! in `system_suspend` as the original holds them
//! (`PowerManagerService.WakeLocks` while one keeps the CPU on,
//! `PowerManagerService.Display` while the display is on); the screen kept
//! on, by a screen wake lock or a visible keep-screen-on window
//! (WindowManagerService's hold screen), keeps the Mac's display awake
//! (`PreventUserIdleDisplaySleep`, docs/power.md); the device's state as
//! apps read it. Binder sleep/wake requests share this display state;
//! the Mac's display sleep/session notifications remain #670. There is no
//! device idle (doze); battery saver is the
//! persisted `low_power` setting. Full-policy background-start/firewall
//! decisions are read natively; remaining policies are #1489.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, DEAD_OBJECT, EX_ILLEGAL_STATE, EX_UNSUPPORTED_OPERATION, Exception, Parcel,
    Reader, Result as ParcelResult,
};
use aim_service_aidl::android_os_ipowermanager as pm;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::settings::Table;
use super::{AppHost, reply};

/// `PowerManager`'s wake lock levels and flags.
const PARTIAL_WAKE_LOCK: i32 = 0x1;
const SCREEN_DIM_WAKE_LOCK: i32 = 0x6;
const SCREEN_BRIGHT_WAKE_LOCK: i32 = 0xa;
const FULL_WAKE_LOCK: i32 = 0x1a;
const PROXIMITY_SCREEN_OFF_WAKE_LOCK: i32 = 0x20;
const DOZE_WAKE_LOCK: i32 = 0x40;
const DRAW_WAKE_LOCK: i32 = 0x80;
const SCREEN_TIMEOUT_OVERRIDE_WAKE_LOCK: i32 = 0x100;
const WAKE_LOCK_LEVEL_MASK: i32 = 0xffff;
const SYSTEM_WAKELOCK: i32 = 0x8000_0000_u32 as i32;
const GO_TO_SLEEP_FLAG_NO_DOZE: i32 = 1;
const GO_TO_SLEEP_FLAG_SOFT_SLEEP: i32 = 2;
/// `Display.DEFAULT_DISPLAY`, `INVALID_DISPLAY`.
const DEFAULT_DISPLAY: i32 = 0;
const INVALID_DISPLAY: i32 = -1;
/// `PowerManager.LOCATION_MODE_NO_CHANGE`,
/// `SOUND_TRIGGER_MODE_ALL_ENABLED`; `ServiceType.SCREEN_BRIGHTNESS`.
const LOCATION_MODE_NO_CHANGE: i32 = 0;
const SOUND_TRIGGER_MODE_ALL_ENABLED: i32 = 0;
const SCREEN_BRIGHTNESS: i32 = 7;
/// The flag of the screen timeout override wake lock.
const EARLY_SCREEN_TIMEOUT: &str =
    "com.android.server.power.feature.flags.enable_early_screen_timeout_detector";
/// `system_suspend`'s service and its wake locks (`WakeLockType.PARTIAL`).
const SYSTEM_SUSPEND: &str = "android.system.suspend.ISystemSuspend/default";
const SYSTEM_SUSPEND_DESCRIPTOR: &str = "android.system.suspend.ISystemSuspend";
const ACQUIRE_WAKE_LOCK: u32 = 1;
const WAKE_LOCK_PARTIAL: i32 = 0;
const WAKE_LOCK_DESCRIPTOR: &str = "android.system.suspend.IWakeLock";
const RELEASE: u32 = 1;
/// The name of the Mac's display assertion (`pmset -g assertions`).
const SCREEN_ASSERTION: &str = "Android keeps the screen on";
/// `IWakeLockCallback.onStateChanged`.
const CALLBACK_DESCRIPTOR: &str = "android.os.IWakeLockCallback";
const ON_STATE_CHANGED: u32 = 1;

/// `WorkSource` at android-16.0.0_r1. Keep the backing arrays too:
/// `WorkChain.equals` compares capacity slots, not just its active nodes.
#[derive(Clone, Debug, Default)]
pub(super) struct WorkSource {
    num: usize,
    uids: Vec<i32>,
    names: Option<Vec<Option<String>>>,
    chains: Option<Vec<WorkChain>>,
}

#[derive(Clone, Debug, PartialEq)]
struct WorkChain {
    size: usize,
    uids: Vec<i32>,
    tags: Vec<Option<String>>,
}

impl WorkSource {
    /// `isEmpty` counts chains even if every chain has zero nodes.
    pub(super) fn is_empty(&self) -> bool {
        self.num == 0 && self.chains.as_ref().is_none_or(Vec::is_empty)
    }
}

impl PartialEq for WorkSource {
    fn eq(&self, other: &Self) -> bool {
        self.num == other.num
            && self.uids[..self.num] == other.uids[..other.num]
            && match (&self.names, &other.names) {
                (Some(a), Some(b)) => a[..self.num] == b[..other.num],
                _ => true,
            }
            && match &self.chains {
                Some(c) if !c.is_empty() => other.chains.as_ref() == Some(c),
                _ => other.chains.as_ref().is_none_or(Vec::is_empty),
            }
    }
}

impl ReadParcelable for WorkSource {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let num = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
        let uids = aim_service_aidl::read_int_array(r)?.unwrap_or_default();
        let names = aim_service_aidl::read_string_list(r)?;
        if num > uids.len() || names.as_ref().is_some_and(|n| num > n.len()) {
            return Err(BAD_VALUE);
        }
        let chains = if r.read_i32()? >= 0 {
            // The first count supplies ArrayList's initial capacity. The
            // second belongs to writeParcelableList and controls its elements.
            let n = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
            if n > r.remaining() / 4 {
                return Err(BAD_VALUE);
            }
            let mut chains = Vec::new();
            for _ in 0..n {
                if r.read_string16()?.as_deref() != Some("android.os.WorkSource$WorkChain") {
                    // Null entries and other parcelable classes are unsupported.
                    return Err(BAD_VALUE);
                }
                let size = usize::try_from(r.read_i32()?).map_err(|_| BAD_VALUE)?;
                let uids = aim_service_aidl::read_int_array(r)?.ok_or(BAD_VALUE)?;
                let tags = aim_service_aidl::read_string_list(r)?.ok_or(BAD_VALUE)?;
                if size > uids.len() || size > tags.len() {
                    return Err(BAD_VALUE);
                }
                chains.push(WorkChain { size, uids, tags });
            }
            Some(chains)
        } else {
            None
        };
        Ok(WorkSource {
            num,
            uids,
            names,
            chains,
        })
    }
}

/// Immutable owner identity and Notifier's attributed properties.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Attribution {
    pub uid: i32,
    pub pid: i32,
    pub package: String,
    pub tag: String,
    pub history: Option<String>,
    pub source: Option<WorkSource>,
    pub kind: Option<usize>,
}

impl Attribution {
    pub(super) fn entries(&self, other: Option<&Self>) -> Vec<i32> {
        let Some(ws) = &self.source else {
            return vec![self.uid];
        };
        let other = other
            .and_then(|a| a.source.as_ref())
            .and_then(|w| w.chains.as_ref());
        ws.uids[..ws.num]
            .iter()
            .copied()
            .chain(
                ws.chains
                    .iter()
                    .flatten()
                    .filter(|c| other.is_none_or(|o| !o.contains(c)))
                    .map(|c| if c.size == 0 { -1 } else { c.uids[0] }),
            )
            .collect()
    }
}

fn monitor_type(flags: i32) -> Option<usize> {
    match flags & WAKE_LOCK_LEVEL_MASK {
        PARTIAL_WAKE_LOCK | PROXIMITY_SCREEN_OFF_WAKE_LOCK => Some(0),
        FULL_WAKE_LOCK | SCREEN_DIM_WAKE_LOCK | SCREEN_BRIGHT_WAKE_LOCK => Some(1),
        DRAW_WAKE_LOCK => Some(2),
        _ => None,
    }
}

/// `PowerManagerService.WakeLock`: state, immutable owner and full attribution.
struct WakeLock {
    lock: Strong,
    death: u64,
    flags: i32,
    attribution: Attribution,
    callback: Option<Strong>,
}

impl WakeLock {
    /// Whether it keeps the CPU on (`WAKE_LOCK_CPU` of the summary while
    /// awake): a partial lock, or one of the screen.
    fn keeps_cpu(&self) -> bool {
        matches!(
            self.flags & WAKE_LOCK_LEVEL_MASK,
            PARTIAL_WAKE_LOCK | FULL_WAKE_LOCK | SCREEN_BRIGHT_WAKE_LOCK | SCREEN_DIM_WAKE_LOCK
        )
    }

    fn keeps_screen(&self) -> bool {
        keeps_screen(self.flags)
    }
}

/// Whether a wake lock of `flags` keeps the screen on
/// (`WAKE_LOCK_SCREEN_BRIGHT` or `WAKE_LOCK_SCREEN_DIM` of the summary
/// while awake).
fn keeps_screen(flags: i32) -> bool {
    matches!(
        flags & WAKE_LOCK_LEVEL_MASK,
        FULL_WAKE_LOCK | SCREEN_BRIGHT_WAKE_LOCK | SCREEN_DIM_WAKE_LOCK
    )
}

/// A suspend blocker held in `system_suspend`.
struct Blocker(Strong);

impl Drop for Blocker {
    fn drop(&mut self) {
        let mut data = Parcel::new();
        data.write_interface_token(WAKE_LOCK_DESCRIPTOR);
        // A dead system_suspend released its wake locks with it.
        match self.0.transact(RELEASE, &data, true) {
            Ok(_) | Err(DEAD_OBJECT) => {}
            Err(s) => eprintln!("app host: power: releasing a suspend blocker: status {s}"),
        }
    }
}

#[derive(Default)]
struct State {
    asleep: bool,
    last_wake: Option<pm::WakeUp>,
    last_sleep: (i64, i32),
    locks: Vec<WakeLock>,
    device_idle_temp: Vec<i32>,
    /// `PowerManagerService.WakeLocks`, `.Display`.
    wake_locks: Option<Blocker>,
    display: Option<Blocker>,
    /// Whether a visible window keeps the screen on (WindowManagerService's
    /// `mHoldingScreenWakeLock`, a screen-bright wake lock).
    window_holds_screen: bool,
    /// The Mac's display kept awake while the screen is kept on.
    screen_assertion: crate::power_assertion::AttributedAssertion,
}

pub struct PowerManager {
    host: Arc<AppHost>,
    state: Mutex<State>,
    transition: Mutex<()>,
}

/// `IWakeLockCallback.onStateChanged`: the lock is enabled or not.
fn notify(callback: &Strong, enabled: bool) {
    let mut data = Parcel::new();
    data.write_interface_token(CALLBACK_DESCRIPTOR);
    data.write_bool(enabled);
    if let Err(s) = callback.transact(ON_STATE_CHANGED, &data, true) {
        eprintln!("app host: power: wake lock callback: status {s}");
    }
}

/// `PowerManager.validateWakeLockParameters`.
fn validate(flags: i32, tag: Option<&str>) -> Result<(), Exception> {
    match flags & WAKE_LOCK_LEVEL_MASK {
        PARTIAL_WAKE_LOCK
        | SCREEN_DIM_WAKE_LOCK
        | SCREEN_BRIGHT_WAKE_LOCK
        | FULL_WAKE_LOCK
        | PROXIMITY_SCREEN_OFF_WAKE_LOCK
        | DOZE_WAKE_LOCK
        | DRAW_WAKE_LOCK
        | SCREEN_TIMEOUT_OVERRIDE_WAKE_LOCK => {}
        _ => {
            return Err(Exception::illegal_argument(
                "Must specify a valid wake lock level.",
            ));
        }
    }
    if tag.is_none() {
        return Err(Exception::illegal_argument("The tag must not be null."));
    }
    Ok(())
}

fn bool_reply(r: Result<bool, Exception>) -> Reply {
    reply(r.map(|v| {
        let mut p = Parcel::new();
        p.write_no_exception();
        p.write_bool(v);
        p
    }))
}

fn empty(r: Result<(), Exception>) -> Reply {
    reply(r.map(|()| {
        let mut p = Parcel::new();
        p.write_no_exception();
        p
    }))
}

/// `PowerSaveState` with battery saver off (`BatterySaverPolicy.OFF_POLICY`).
struct PowerSaveState {
    brightness_factor: f32,
}

impl WriteParcelable for PowerSaveState {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(0); // battery saver enabled
        p.write_i32(0); // globally
        p.write_i32(LOCATION_MODE_NO_CHANGE);
        p.write_i32(SOUND_TRIGGER_MODE_ALL_ENABLED);
        p.write_f32(self.brightness_factor);
    }
}

impl AppHost {
    /// BatterySaverPolicy's selected full-policy boolean. Other power policies
    /// remain unsupported; no light/deep idle state is inferred from this.
    pub(super) fn power_restriction(&self, key: &str, resource: &str) -> Result<bool, Exception> {
        let power = self
            .power_service
            .get_or_init(|| Arc::new(PowerManager::new(self.me.upgrade().unwrap())));
        if !power.power_save_mode()? {
            return Ok(false);
        }
        let settings = &self.platform()?.settings;
        let constants = settings
            .get(Table::Global, "battery_saver_constants")
            .unwrap_or_default();
        let pairs: Option<Vec<_>> = constants
            .split(',')
            .filter(|v| !v.trim().is_empty())
            .map(|v| v.trim().split_once('=').map(|(k, v)| (k.trim(), v.trim())))
            .collect();
        let value = pairs
            .as_ref()
            .and_then(|p| {
                p.iter()
                    .rev()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| (*v).to_owned())
            })
            .or_else(|| settings.device_config("battery_saver", key));
        if let Some(value) = value {
            return Ok(value.eq_ignore_ascii_case("true"));
        }
        self.framework()?
            .framework_bool(&format!("config_batterySaver_full_{resource}"))
            .ok_or_else(|| {
                Exception::new(
                    EX_ILLEGAL_STATE,
                    format!("power: missing {resource} policy"),
                )
            })
    }
}

impl PowerManager {
    /// PowerManagerInternal.setDeviceIdleTempWhitelist. Neither light nor deep
    /// idle is implemented: no wake lock is disabled or enabled by this update.
    pub(super) fn set_device_idle_temp_allowlist(&self, ids: Vec<i32>) {
        self.state.lock().unwrap().device_idle_temp = ids;
    }

    #[cfg(test)]
    pub(super) fn temp_ids_for_test(&self) -> Vec<i32> {
        self.state.lock().unwrap().device_idle_temp.clone()
    }

    pub fn new(host: Arc<AppHost>) -> PowerManager {
        PowerManager {
            host,
            state: Mutex::default(),
            transition: Mutex::default(),
        }
    }

    pub(super) fn interactive(&self) -> bool {
        !self.state.lock().unwrap().asleep
    }

    /// PMS BinderService / PowerGroup timestamp guards and Notifier's
    /// single-display interactive transition (android-16.0.0_r1).
    #[allow(clippy::too_many_arguments)]
    fn set_interactive(
        &self,
        call: &Call<'_>,
        time: i64,
        awake: bool,
        reason: i32,
        details: Option<String>,
        package: Option<String>,
        flags: i32,
    ) -> Result<(), Exception> {
        if time > aim_hostcall::clock::monotonic_ns() / 1_000_000 {
            return Err(Exception::illegal_argument(
                "event time must not be in the future",
            ));
        }
        self.enforce("android.permission.DEVICE_POWER", call.sender_euid as i32)?;
        let _transition = self.transition.lock().unwrap();
        {
            let mut state = self.state.lock().unwrap();
            let last = if awake {
                state.last_sleep.0
            } else {
                state.last_wake.as_ref().map_or(0, |a| a.time)
            };
            if time < last || awake == !state.asleep {
                return Ok(());
            }
            // SOFT_SLEEP, without fold grace: a screen wake lock vetoes sleep.
            if !awake
                && flags & GO_TO_SLEEP_FLAG_SOFT_SLEEP != 0
                && (state.window_holds_screen || state.locks.iter().any(WakeLock::keeps_screen))
            {
                return Ok(());
            }
            self.battery().wake_screen(awake);
            state.asleep = !awake;
            if awake {
                eprintln!(
                    "app host: power: waking uid={} reason={reason} details={details:?} package={package:?}",
                    call.sender_euid
                );
                state.last_wake = Some(pm::WakeUp {
                    time,
                    reason,
                    details,
                    op_package_name: package,
                });
            } else {
                state.last_sleep = (
                    time,
                    if flags & GO_TO_SLEEP_FLAG_NO_DOZE != 0 {
                        reason
                    } else {
                        reason.clamp(0, 10)
                    },
                );
            }
            self.update_blockers(&mut state);
        }
        if let Some(display) = self.host.display_manager.get() {
            display.power_changed();
        }
        // Notifier's interactive broadcast: nonsticky, registered receivers,
        // foreground and visible to instant apps, for every user.
        let action = if awake {
            "android.intent.action.SCREEN_ON"
        } else {
            "android.intent.action.SCREEN_OFF"
        };
        self.host.broadcasts.send_from_system(
            &self.host,
            super::kept::RawIntent::new(action, 0x5000_0000 | 0x0020_0000, &[]),
        );
        Ok(())
    }

    fn enforce(&self, permission: &str, uid: i32) -> Result<(), Exception> {
        if self.host.check_permission(Some(permission), uid)? == 0 {
            return Ok(());
        }
        Err(Exception::security(format!(
            "uid {uid} does not have {permission}."
        )))
    }

    /// A suspend blocker of `name`, from `system_suspend`.
    fn blocker(&self, name: &str) -> Result<Blocker, String> {
        let suspend = super::display::check_service(&self.host.process, SYSTEM_SUSPEND)?;
        let mut data = Parcel::new();
        data.write_interface_token(SYSTEM_SUSPEND_DESCRIPTOR);
        data.write_i32(WAKE_LOCK_PARTIAL);
        data.write_string16(Some(name));
        let reply = suspend
            .transact(ACQUIRE_WAKE_LOCK, &data, false)
            .map_err(|s| format!("acquireWakeLock: status {s}"))?;
        let mut r = reply.reader();
        r.read_exception()
            .map_err(|s| format!("acquireWakeLock: status {s}"))?
            .map_err(|e| format!("acquireWakeLock: {}", e.message))?;
        match r.read_binder() {
            Ok(Some(Binder::Handle(h))) => Ok(Blocker(self.host.process.strong(h))),
            _ => Err("acquireWakeLock: no wake lock".into()),
        }
    }

    fn battery(&self) -> &Arc<super::battery_stats::BatteryStats> {
        self.host
            .battery_stats
            .get_or_init(|| Arc::new(super::battery_stats::BatteryStats::new(&self.host)))
    }

    /// `updateSuspendBlockerLocked`: `.WakeLocks` while a lock keeps the
    /// CPU on, `.Display` while the display is on.
    fn update_blockers(&self, state: &mut State) {
        let need = state.locks.iter().any(|l| {
            l.flags & WAKE_LOCK_LEVEL_MASK == PARTIAL_WAKE_LOCK || !state.asleep && l.keeps_cpu()
        });
        let take = |slot: &mut Option<Blocker>, name: &str| {
            if slot.is_none() {
                match self.blocker(name) {
                    Ok(b) => *slot = Some(b),
                    Err(e) => eprintln!("app host: power: {name}: {e}"),
                }
            }
        };
        if need {
            take(&mut state.wake_locks, "PowerManagerService.WakeLocks");
        }
        if state.asleep {
            state.display = None;
        } else {
            take(&mut state.display, "PowerManagerService.Display");
        }
        if !need {
            state.wake_locks = None;
        }
        // The screen kept on is the Mac's display kept awake.
        let screen = !state.asleep
            && (state.window_holds_screen || state.locks.iter().any(WakeLock::keeps_screen));
        let screen_name = screen.then(|| {
            let mut names: Vec<_> = state
                .locks
                .iter()
                .filter(|l| l.keeps_screen())
                .map(|l| format!("{:?}", l.attribution))
                .collect();
            if state.window_holds_screen {
                names.push("Android display".into());
            }
            format!("{SCREEN_ASSERTION}: {}", names.join("; "))
        });
        if let Err(e) = state.screen_assertion.update(screen_name.as_deref()) {
            eprintln!("app host: power: display assertion: IOKit error {e:#x}");
        }
    }

    /// WindowManagerService's `setHoldScreenLocked`: a visible window keeps
    /// the screen on, or none does.
    pub(super) fn set_window_holds_screen(&self, hold: bool) {
        let mut state = self.state.lock().unwrap();
        if state.window_holds_screen != hold {
            state.window_holds_screen = hold;
            self.update_blockers(&mut state);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn acquire(
        &self,
        call: &Call<'_>,
        lock: Option<Binder>,
        flags: i32,
        tag: Option<String>,
        package: Option<String>,
        ws: Option<WorkSource>,
        history: Option<String>,
        display_id: i32,
        callback: Option<Binder>,
    ) -> Result<(), Exception> {
        let Some(Binder::Handle(handle)) = lock else {
            return Err(Exception::illegal_argument("lock must not be null"));
        };
        if package.is_none() {
            return Err(Exception::illegal_argument("packageName must not be null"));
        }
        validate(flags, tag.as_deref())?;
        let calling_uid = call.sender_euid as i32;
        self.enforce("android.permission.WAKE_LOCK", calling_uid)?;
        if flags & DOZE_WAKE_LOCK != 0 {
            self.enforce("android.permission.DEVICE_POWER", calling_uid)?;
        }
        if flags & SCREEN_TIMEOUT_OVERRIDE_WAKE_LOCK != 0 {
            let (state, _) = self.host.packages()?;
            let enabled = state
                .system
                .flags
                .iter()
                .any(|(n, on)| n == EARLY_SCREEN_TIMEOUT && *on);
            if !enabled {
                return Err(Exception::illegal_argument(format!(
                    "Acquiring an unsupported wake lock: flags={flags}, tag={}",
                    tag.as_deref().unwrap_or_default()
                )));
            }
            self.enforce("android.permission.SCREEN_TIMEOUT_OVERRIDE", calling_uid)?;
        }
        let mut ws = ws.filter(|w| !w.is_empty());
        if ws.is_some() {
            self.enforce("android.permission.UPDATE_DEVICE_STATS", calling_uid)?;
        }
        let (mut uid, mut pid) = (calling_uid, call.sender_pid as i32);
        if flags & SYSTEM_WAKELOCK != 0 {
            self.enforce("android.permission.DEVICE_POWER", calling_uid)?;
            let mut source = ws.unwrap_or_default();
            if source.num > 0 && source.names.is_none() {
                return Err(Exception::illegal_argument(
                    "Adding unnamed WorkSource to named WorkSource",
                ));
            }
            let mut entries: Vec<_> = source.uids[..source.num]
                .iter()
                .copied()
                .zip(
                    source
                        .names
                        .as_ref()
                        .map_or(&[][..], |n| &n[..source.num])
                        .iter()
                        .cloned(),
                )
                .collect();
            if let Some(chains) = &mut source.chains {
                let mut unique = Vec::new();
                for c in chains.drain(..) {
                    if !unique.contains(&c) {
                        unique.push(c);
                    }
                }
                *chains = unique;
            }
            entries.push((calling_uid, package.clone()));
            entries.sort_by_key(|(uid, name)| {
                (
                    *uid,
                    name.as_ref().map(|n| n.encode_utf16().collect::<Vec<_>>()),
                )
            });
            entries.dedup();
            source.num = entries.len();
            (source.uids, source.names) = (
                entries.iter().map(|e| e.0).collect(),
                Some(entries.into_iter().map(|e| e.1).collect()),
            );
            ws = Some(source);
            uid = 1000;
            pid = self.host.process.pid() as i32;
        }
        let attribution = Attribution {
            uid,
            pid,
            package: package.unwrap(),
            tag: tag.unwrap(),
            history,
            source: ws,
            kind: if flags & WAKE_LOCK_LEVEL_MASK == PROXIMITY_SCREEN_OFF_WAKE_LOCK
                && self
                    .host
                    .framework()?
                    .framework_bool("config_suspendWhenScreenOffDueToProximity")
                    .ok_or_else(|| {
                        Exception::new(EX_ILLEGAL_STATE, "Missing proximity suspend configuration")
                    })?
            {
                None
            } else {
                monitor_type(flags)
            },
        };
        if display_id != INVALID_DISPLAY && display_id != DEFAULT_DISPLAY {
            // getDisplayInfo of an unknown display: the original logs a wtf.
            eprintln!("app host: power: a wake lock for invalid display {display_id}");
            return Ok(());
        }
        let mut state = self.state.lock().unwrap();
        if let Some(l) = state.locks.iter_mut().find(|l| l.lock.handle == handle) {
            if l.flags != flags
                || l.attribution.tag != attribution.tag
                || l.attribution.source != attribution.source
                || l.attribution.uid != uid
                || l.attribution.pid != pid
            {
                if (
                    l.attribution.package.as_str(),
                    l.attribution.uid,
                    l.attribution.pid,
                ) != (attribution.package.as_str(), uid, pid)
                {
                    return Err(Exception::new(
                        EX_ILLEGAL_STATE,
                        "Existing wake lock owner changed",
                    ));
                }
                self.battery()
                    .change_wake(Some(&l.attribution), Some(&attribution))?;
                l.flags = flags;
                l.attribution = attribution;
                l.callback = callback.and_then(|c| match c {
                    Binder::Handle(h) => Some(self.host.process.strong(h)),
                    Binder::Local(_) => None,
                });
            }
        } else {
            self.battery().change_wake(None, Some(&attribution))?;
            let lock = self.host.process.strong(handle);
            let me = Arc::downgrade(&self.host);
            let death = self.host.process.link_to_death(
                &lock,
                Box::new(move || {
                    if let Some(power) = me.upgrade().and_then(|h| h.power_service.get().cloned()) {
                        power.remove(handle);
                    }
                }),
            );
            let callback = callback.and_then(|c| match c {
                Binder::Handle(h) => Some(self.host.process.strong(h)),
                Binder::Local(_) => None,
            });
            // Notifier.onWakeLockAcquired: enabled (none is disabled: the
            // app host's processes are all top, the device never idles).
            if let Some(c) = &callback {
                notify(c, true);
            }
            state.locks.push(WakeLock {
                lock,
                death,
                flags,
                attribution,
                callback,
            });
        }
        self.update_blockers(&mut state);
        Ok(())
    }

    /// `releaseWakeLockInternal` (or the lock's death): the lock goes.
    fn remove(&self, handle: u32) {
        let mut state = self.state.lock().unwrap();
        let Some(at) = state.locks.iter().position(|l| l.lock.handle == handle) else {
            return;
        };
        let l = state.locks.remove(at);
        self.battery()
            .change_wake(Some(&l.attribution), None)
            .unwrap();
        self.host.process.clear_death(&l.lock, l.death);
        if let Some(c) = &l.callback {
            notify(c, false);
        }
        self.update_blockers(&mut state);
    }

    fn release(&self, call: &Call<'_>, lock: Option<Binder>) -> Result<(), Exception> {
        let Some(lock) = lock else {
            return Err(Exception::illegal_argument("lock must not be null"));
        };
        self.enforce("android.permission.WAKE_LOCK", call.sender_euid as i32)?;
        if let Binder::Handle(h) = lock {
            self.remove(h);
        }
        Ok(())
    }

    /// `updateWakeLockWorkSource` and `updateWakeLockUids`.
    fn update_work_source(
        &self,
        call: &Call<'_>,
        lock: Option<Binder>,
        ws: Option<WorkSource>,
        history: Option<String>,
    ) -> Result<(), Exception> {
        let Some(lock) = lock else {
            return Err(Exception::illegal_argument("lock must not be null"));
        };
        let uid = call.sender_euid as i32;
        self.enforce("android.permission.WAKE_LOCK", uid)?;
        let ws = ws.filter(|w| !w.is_empty());
        if ws.is_some() {
            self.enforce("android.permission.UPDATE_DEVICE_STATS", uid)?;
        }
        let mut state = self.state.lock().unwrap();
        let l = state
            .locks
            .iter_mut()
            .find(|l| Binder::Handle(l.lock.handle) == lock)
            .ok_or_else(|| Exception::illegal_argument("Wake lock not active"))?;
        if l.attribution.source != ws {
            let mut next = l.attribution.clone();
            next.source = ws;
            next.history = history;
            self.battery()
                .change_wake(Some(&l.attribution), Some(&next))?;
            l.attribution = next;
            self.update_blockers(&mut state);
        }
        Ok(())
    }

    /// `PowerSaveState` of `service_type`, battery saver off.
    fn power_save_state(&self, service_type: i32) -> Result<PowerSaveState, Exception> {
        if self.power_save_mode()? {
            return Err(Exception::new(
                EX_UNSUPPORTED_OPERATION,
                "power: battery saver's policies are not run (#1489)",
            ));
        }
        Ok(PowerSaveState {
            brightness_factor: if service_type == SCREEN_BRIGHTNESS {
                1.0
            } else {
                0.5
            },
        })
    }

    /// Battery saver: supported (`config_batterySaverSupported`) and on
    /// by the persisted `low_power` setting.
    pub(super) fn power_save_mode(&self) -> Result<bool, Exception> {
        let inputs = self.host.platform()?;
        let supported = self
            .host
            .framework()?
            .framework_bool("config_batterySaverSupported")
            .unwrap_or(true);
        Ok(supported && inputs.settings.get(Table::Global, "low_power").as_deref() == Some("1"))
    }
}

impl PowerManager {
    /// `LowPowerStandbyController.isEnabled`: supported
    /// (`config_lowPowerStandbySupported`) and on by the
    /// `low_power_standby_enabled` setting, else by
    /// `config_lowPowerStandbyEnabledByDefault`. The default is read, not
    /// written into the setting as `initSettingsLocked` does.
    fn low_power_standby_enabled(&self) -> Result<bool, Exception> {
        let inputs = self.host.platform()?;
        let framework = self.host.framework()?;
        let config = |name| framework.framework_bool(name).unwrap_or(false);
        let by_default = config("config_lowPowerStandbyEnabledByDefault");
        let enabled = inputs
            .settings
            .get(Table::Global, "low_power_standby_enabled")
            .and_then(|v| v.parse::<i32>().ok())
            .map_or(by_default, |v| v != 0);
        Ok(config("config_lowPowerStandbySupported") && enabled)
    }
}

impl Service for PowerManager {
    fn descriptor(&self) -> &str {
        pm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let bad = |_| BAD_VALUE;
        match call.code {
            pm::ACQUIRE_WAKE_LOCK => {
                let a = pm::AcquireWakeLock::<WorkSource>::read(&mut call.data).map_err(bad)?;
                empty(self.acquire(
                    call,
                    a.lock,
                    a.flags,
                    a.tag,
                    a.package_name,
                    a.ws,
                    a.history_tag,
                    a.display_id,
                    a.callback,
                ))
            }
            pm::ACQUIRE_WAKE_LOCK_ASYNC => {
                let a =
                    pm::AcquireWakeLockAsync::<WorkSource>::read(&mut call.data).map_err(bad)?;
                if let Err(e) = self.acquire(
                    call,
                    a.lock,
                    a.flags,
                    a.tag,
                    a.package_name,
                    a.ws,
                    a.history_tag,
                    INVALID_DISPLAY,
                    None,
                ) {
                    eprintln!("app host: power: acquireWakeLockAsync: {}", e.message);
                }
                Ok(Parcel::new())
            }
            pm::RELEASE_WAKE_LOCK => {
                let a = pm::ReleaseWakeLock::read(&mut call.data).map_err(bad)?;
                empty(self.release(call, a.lock))
            }
            pm::RELEASE_WAKE_LOCK_ASYNC => {
                let a = pm::ReleaseWakeLockAsync::read(&mut call.data).map_err(bad)?;
                if let Err(e) = self.release(call, a.lock) {
                    eprintln!("app host: power: releaseWakeLockAsync: {}", e.message);
                }
                Ok(Parcel::new())
            }
            pm::UPDATE_WAKE_LOCK_WORK_SOURCE => {
                let a = pm::UpdateWakeLockWorkSource::<WorkSource>::read(&mut call.data)
                    .map_err(bad)?;
                empty(self.update_work_source(call, a.lock, a.ws, a.history_tag))
            }
            pm::UPDATE_WAKE_LOCK_UIDS | pm::UPDATE_WAKE_LOCK_UIDS_ASYNC => {
                let oneway = call.code == pm::UPDATE_WAKE_LOCK_UIDS_ASYNC;
                let a = pm::UpdateWakeLockUids::read(&mut call.data).map_err(bad)?;
                // A work source of the uids.
                let chained = self.host.flag(
                    "com.android.server.power.feature.flags.wakelock_attribution_via_workchain",
                );
                let r = chained.and_then(|chained| {
                    let ws = a.uids.map(|mut uids| {
                        if chained {
                            WorkSource {
                                chains: Some(
                                    uids.into_iter()
                                        .map(|uid| WorkChain {
                                            size: 2,
                                            uids: vec![uid, call.sender_euid as i32, 0, 0],
                                            tags: vec![None; 4],
                                        })
                                        .collect(),
                                ),
                                ..WorkSource::default()
                            }
                        } else {
                            uids.sort();
                            uids.dedup();
                            WorkSource {
                                num: uids.len(),
                                uids,
                                ..WorkSource::default()
                            }
                        }
                    });
                    self.update_work_source(call, a.lock, ws, None)
                });
                if oneway {
                    if let Err(e) = r {
                        eprintln!("app host: power: updateWakeLockUidsAsync: {}", e.message);
                    }
                    return Ok(Parcel::new());
                }
                empty(r)
            }
            pm::UPDATE_WAKE_LOCK_CALLBACK => {
                let a = pm::UpdateWakeLockCallback::read(&mut call.data).map_err(bad)?;
                empty((|| {
                    let Some(lock) = a.lock else {
                        return Err(Exception::illegal_argument("lock must not be null"));
                    };
                    self.enforce("android.permission.WAKE_LOCK", call.sender_euid as i32)?;
                    let mut state = self.state.lock().unwrap();
                    let l = state
                        .locks
                        .iter_mut()
                        .find(|l| Binder::Handle(l.lock.handle) == lock)
                        .ok_or_else(|| Exception::illegal_argument("Wake lock not active"))?;
                    l.callback = match a.callback {
                        Some(Binder::Handle(h)) => Some(self.host.process.strong(h)),
                        _ => None,
                    };
                    Ok(())
                })())
            }
            pm::IS_WAKE_LOCK_LEVEL_SUPPORTED | pm::IS_WAKE_LOCK_LEVEL_SUPPORTED_WITH_DISPLAY_ID => {
                let level = if call.code == pm::IS_WAKE_LOCK_LEVEL_SUPPORTED {
                    pm::IsWakeLockLevelSupported::read(&mut call.data)
                        .map_err(bad)?
                        .level
                } else {
                    pm::IsWakeLockLevelSupportedWithDisplayId::read(&mut call.data)
                        .map_err(bad)?
                        .level
                };
                // No proximity sensor, no screen timeout override policy.
                bool_reply(Ok(matches!(
                    level,
                    PARTIAL_WAKE_LOCK
                        | SCREEN_DIM_WAKE_LOCK
                        | SCREEN_BRIGHT_WAKE_LOCK
                        | FULL_WAKE_LOCK
                        | DOZE_WAKE_LOCK
                        | DRAW_WAKE_LOCK
                )))
            }
            pm::IS_AMBIENT_DISPLAY_AVAILABLE => {
                pm::IsAmbientDisplayAvailable::read(&mut call.data).map_err(bad)?;
                bool_reply((|| {
                    let permission = "android.permission.READ_DREAM_STATE";
                    let uid = call.sender_euid as i32;
                    if self.host.check_permission(Some(permission), uid)? != 0 {
                        return Err(Exception::security(format!(
                            "Neither user {uid} nor current process has {permission}."
                        )));
                    }
                    let component = self
                        .host
                        .framework()?
                        .framework_string("config_dozeComponent")
                        .ok_or_else(|| {
                            Exception::new(
                                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                                "missing config_dozeComponent",
                            )
                        })?;
                    Ok(!component.is_empty())
                })())
            }
            pm::WAKE_UP => {
                let a = pm::WakeUp::read(&mut call.data).map_err(bad)?;
                empty(self.set_interactive(
                    call,
                    a.time,
                    true,
                    a.reason,
                    a.details,
                    a.op_package_name,
                    0,
                ))
            }
            pm::GO_TO_SLEEP => {
                let a = pm::GoToSleep::read(&mut call.data).map_err(bad)?;
                empty(self.set_interactive(call, a.time, false, a.reason, None, None, a.flags))
            }
            pm::IS_INTERACTIVE => {
                pm::IsInteractive::read(&mut call.data).map_err(bad)?;
                bool_reply(Ok(self.interactive()))
            }
            pm::IS_DISPLAY_INTERACTIVE => {
                let a = pm::IsDisplayInteractive::read(&mut call.data).map_err(bad)?;
                bool_reply(Ok(a.display_id == DEFAULT_DISPLAY && self.interactive()))
            }
            pm::IS_POWER_SAVE_MODE => {
                pm::IsPowerSaveMode::read(&mut call.data).map_err(bad)?;
                bool_reply(self.power_save_mode())
            }
            pm::IS_BATTERY_SAVER_SUPPORTED => {
                pm::IsBatterySaverSupported::read(&mut call.data).map_err(bad)?;
                bool_reply(self.host.framework().map(|framework| {
                    framework
                        .framework_bool("config_batterySaverSupported")
                        .unwrap_or(true)
                }))
            }
            pm::GET_POWER_SAVE_STATE => {
                let a = pm::GetPowerSaveState::read(&mut call.data).map_err(bad)?;
                reply(self.power_save_state(a.service_type).map(|s| {
                    let mut p = Parcel::new();
                    pm::write_get_power_save_state_reply(&mut p, Some(&s));
                    p
                }))
            }
            pm::IS_LOW_POWER_STANDBY_ENABLED => {
                pm::IsLowPowerStandbyEnabled::read(&mut call.data).map_err(bad)?;
                bool_reply(self.low_power_standby_enabled())
            }
            pm::IS_DEVICE_IDLE_MODE => {
                pm::IsDeviceIdleMode::read(&mut call.data).map_err(bad)?;
                bool_reply(Ok(false))
            }
            pm::IS_LIGHT_DEVICE_IDLE_MODE => {
                pm::IsLightDeviceIdleMode::read(&mut call.data).map_err(bad)?;
                bool_reply(Ok(false))
            }
            code => reply(Err(self.host.unimplemented(
                "power",
                self.descriptor(),
                code,
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The screen levels keep the screen (and so the Mac's display) on;
    /// a partial lock, proximity, doze and draw locks do not.
    #[test]
    fn screen_levels_keep_the_screen_on() {
        for level in [
            FULL_WAKE_LOCK,
            SCREEN_BRIGHT_WAKE_LOCK,
            SCREEN_DIM_WAKE_LOCK,
        ] {
            assert!(keeps_screen(level | SYSTEM_WAKELOCK), "{level:#x}");
        }
        for level in [
            PARTIAL_WAKE_LOCK,
            PROXIMITY_SCREEN_OFF_WAKE_LOCK,
            DOZE_WAKE_LOCK,
            DRAW_WAKE_LOCK,
        ] {
            assert!(!keeps_screen(level), "{level:#x}");
        }
    }

    #[test]
    fn wake_lock_parameters_are_validated() {
        assert!(validate(PARTIAL_WAKE_LOCK | SYSTEM_WAKELOCK, Some("t")).is_ok());
        assert!(validate(0x2, Some("t")).is_err());
        assert!(validate(PARTIAL_WAKE_LOCK, None).is_err());
    }

    /// A work source of two uids and a chain, as `WorkSource.writeToParcel`
    /// writes it, read to its end.
    #[test]
    fn work_sources_read_as_written() {
        let mut p = Parcel::new();
        p.write_i32(2);
        aim_service_aidl::write_int_array(&mut p, Some(&[10_001, 10_002]));
        p.write_i32(-1); // no names
        p.write_i32(1);
        p.write_i32(1);
        p.write_string16(Some("android.os.WorkSource$WorkChain"));
        p.write_i32(1);
        aim_service_aidl::write_int_array(&mut p, Some(&[10_003]));
        p.write_i32(1);
        p.write_string16(Some("tag"));
        let mut r = p.reader();
        let ws = WorkSource::read_from(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        assert_eq!(ws.num, 2);
        assert_eq!(ws.uids, [10_001, 10_002]);
        let chains = ws.chains.as_ref().unwrap();
        assert_eq!(chains[0].uids, [10_003]);
        assert_eq!(chains[0].tags, [Some("tag".into())]);
        assert!(!ws.is_empty());
        assert!(WorkSource::default().is_empty());
    }

    pub(super) fn source_with_chain(class: Option<&str>, size: i32) -> Parcel {
        let mut p = Parcel::new();
        p.write_i32(1);
        aim_service_aidl::write_int_array(&mut p, Some(&[10001, 0, 0]));
        aim_service_aidl::write_string_list(&mut p, Some(&[Some("owner".into()), None, None]));
        p.write_i32(8); // ArrayList initial capacity, independent of list count
        p.write_i32(1);
        p.write_string16(class);
        if class.is_some() {
            p.write_i32(size);
            aim_service_aidl::write_int_array(&mut p, Some(&[10002, 1000, 0, 0]));
            aim_service_aidl::write_string_list(
                &mut p,
                Some(&[Some("adapter".into()), None, None, None]),
            );
        }
        p
    }

    #[test]
    fn work_source_preserves_names_chain_nodes_capacity_and_parcel_boundary() {
        let mut p = source_with_chain(Some("android.os.WorkSource$WorkChain"), 2);
        p.write_i32(0x12345678);
        let mut r = p.reader();
        let ws = WorkSource::read_from(&mut r).unwrap();
        assert_eq!(ws.num, 1);
        assert_eq!(ws.uids, [10001, 0, 0]);
        assert_eq!(ws.names.as_ref().unwrap()[0].as_deref(), Some("owner"));
        let chain = &ws.chains.as_ref().unwrap()[0];
        assert_eq!(chain.size, 2);
        assert_eq!(chain.uids, [10002, 1000, 0, 0]);
        assert_eq!(chain.tags, [Some("adapter".into()), None, None, None]);
        assert_eq!(r.read_i32().unwrap(), 0x12345678);
        assert_eq!(r.remaining(), 0);
        let mut changed = ws.clone();
        changed.chains.as_mut().unwrap()[0].uids[3] = 42;
        assert_ne!(ws, changed); // WorkChain.equals includes inactive capacity
        let mut changed = ws.clone();
        changed.uids[2] = 42;
        assert_eq!(ws, changed); // WorkSource.diff ignores inactive capacity
        changed.names = None;
        assert_eq!(ws, changed); // pinned diff only compares two named sources
    }

    #[test]
    fn work_source_rejects_unrepresentable_counts_classes_and_truncation() {
        let valid = source_with_chain(Some("android.os.WorkSource$WorkChain"), 2);
        let size = valid.reader().remaining();
        let mut r = valid.reader();
        r.skip(size).unwrap();
        let bytes = r.since(0).0.to_vec();
        for len in 0..size {
            // Build each truncation without treating payload bytes as a parcelable.
            let mut r = Reader::new(&bytes[..len], &[]);
            assert!(WorkSource::read_from(&mut r).is_err(), "{len}");
        }
        for class in [None, Some("other.Parcelable")] {
            assert!(WorkSource::read_from(&mut source_with_chain(class, 2).reader()).is_err());
        }
        for size in [-1, 5] {
            assert!(
                WorkSource::read_from(
                    &mut source_with_chain(Some("android.os.WorkSource$WorkChain"), size).reader()
                )
                .is_err()
            );
        }
        let mut p = Parcel::new();
        p.write_i32(2);
        aim_service_aidl::write_int_array(&mut p, Some(&[10001]));
        p.write_i32(-1);
        p.write_i32(-1);
        assert!(WorkSource::read_from(&mut p.reader()).is_err());
    }
}

#[cfg(test)]
#[path = "power_wake_tests.rs"]
mod wake_tests;

#[cfg(test)]
#[path = "sync_cleanup_tests.rs"]
mod sync_cleanup_tests;
