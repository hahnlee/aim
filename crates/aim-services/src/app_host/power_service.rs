//! `power`: PowerManagerService for the app host (`PowerManagerService`
//! at `android-16.0.0_r1`, ADR 0013): apps' wake locks, with their
//! permissions, death and callbacks, and the suspend blockers they hold
//! in `system_suspend` as the original holds them
//! (`PowerManagerService.WakeLocks` while one keeps the CPU on,
//! `PowerManagerService.Display` while the display is on); the device's
//! state as apps read it. The app host's display is always on, so the
//! device is awake and interactive (the Mac's display sleep is #669's
//! follow-up); there is no device idle (doze); battery saver is the
//! persisted `low_power` setting, its policies not run (#1489).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, DEAD_OBJECT, EX_UNSUPPORTED_OPERATION, Exception, Parcel, Reader,
    Result as ParcelResult,
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
/// `IWakeLockCallback.onStateChanged`.
const CALLBACK_DESCRIPTOR: &str = "android.os.IWakeLockCallback";
const ON_STATE_CHANGED: u32 = 1;

/// `android.os.WorkSource`, as far as whether it is empty: its uids'
/// count and its chains'. Battery stats, which it attributes power to,
/// are not kept.
#[derive(Default)]
pub(super) struct WorkSource {
    num: i32,
    chains: Option<usize>,
}

impl WorkSource {
    /// `isEmpty`.
    pub(super) fn is_empty(&self) -> bool {
        self.num == 0 && self.chains.is_none_or(|n| n == 0)
    }
}

/// `createStringArray`, read past.
fn string_array(r: &mut Reader<'_>) -> ParcelResult<()> {
    for _ in 0..r.read_i32()?.max(0) {
        r.read_string16()?;
    }
    Ok(())
}

impl ReadParcelable for WorkSource {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        let num = r.read_i32()?;
        aim_service_aidl::read_int_array(r)?;
        string_array(r)?;
        let chains = if r.read_i32()? >= 0 {
            // writeParcelableList: its count, then each chain by class:
            // its size, uids and tags.
            let n = r.read_i32()?.max(0) as usize;
            for _ in 0..n {
                if r.read_string16()?.is_none() {
                    return Err(BAD_VALUE);
                }
                r.read_i32()?;
                aim_service_aidl::read_int_array(r)?;
                string_array(r)?;
            }
            Some(n)
        } else {
            None
        };
        Ok(WorkSource { num, chains })
    }
}

/// `PowerManagerService.WakeLock`, of what decides the device's state:
/// its level and its callback.
struct WakeLock {
    lock: Strong,
    death: u64,
    flags: i32,
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
    locks: Vec<WakeLock>,
    /// `PowerManagerService.WakeLocks`, `.Display`.
    wake_locks: Option<Blocker>,
    display: Option<Blocker>,
}

pub struct PowerManager {
    host: Arc<AppHost>,
    state: Mutex<State>,
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

impl PowerManager {
    pub fn new(host: Arc<AppHost>) -> PowerManager {
        PowerManager {
            host,
            state: Mutex::default(),
        }
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

    /// `updateSuspendBlockerLocked`: `.WakeLocks` while a lock keeps the
    /// CPU on, `.Display` while the display is on (always).
    fn update_blockers(&self, state: &mut State) {
        let need = state.locks.iter().any(WakeLock::keeps_cpu);
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
        take(&mut state.display, "PowerManagerService.Display");
        if !need {
            state.wake_locks = None;
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
        if ws.is_some_and(|w| !w.is_empty()) {
            self.enforce("android.permission.UPDATE_DEVICE_STATS", calling_uid)?;
        }
        if flags & SYSTEM_WAKELOCK != 0 {
            self.enforce("android.permission.DEVICE_POWER", calling_uid)?;
        }
        if display_id != INVALID_DISPLAY && display_id != DEFAULT_DISPLAY {
            // getDisplayInfo of an unknown display: the original logs a wtf.
            eprintln!("app host: power: a wake lock for invalid display {display_id}");
            return Ok(());
        }
        let mut state = self.state.lock().unwrap();
        if let Some(l) = state.locks.iter_mut().find(|l| l.lock.handle == handle) {
            l.flags = flags;
            l.callback = callback.and_then(|c| match c {
                Binder::Handle(h) => Some(self.host.process.strong(h)),
                Binder::Local(_) => None,
            });
        } else {
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
    ) -> Result<(), Exception> {
        let Some(lock) = lock else {
            return Err(Exception::illegal_argument("lock must not be null"));
        };
        let uid = call.sender_euid as i32;
        self.enforce("android.permission.WAKE_LOCK", uid)?;
        if ws.is_some_and(|w| !w.is_empty()) {
            self.enforce("android.permission.UPDATE_DEVICE_STATS", uid)?;
        }
        let state = self.state.lock().unwrap();
        if !state
            .locks
            .iter()
            .any(|l| Binder::Handle(l.lock.handle) == lock)
        {
            return Err(Exception::illegal_argument("Wake lock not active"));
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
        let inputs = self.host.inputs()?;
        let supported = inputs
            .packages
            .framework_bool("config_batterySaverSupported")
            .unwrap_or(true);
        Ok(supported && inputs.settings.get(Table::Global, "low_power") == Some("1"))
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
                empty(self.update_work_source(call, a.lock, a.ws))
            }
            pm::UPDATE_WAKE_LOCK_UIDS | pm::UPDATE_WAKE_LOCK_UIDS_ASYNC => {
                let oneway = call.code == pm::UPDATE_WAKE_LOCK_UIDS_ASYNC;
                let a = pm::UpdateWakeLockUids::read(&mut call.data).map_err(bad)?;
                // A work source of the uids.
                let ws = a.uids.map(|uids| WorkSource {
                    num: uids.len() as i32,
                    chains: None,
                });
                let r = self.update_work_source(call, a.lock, ws);
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
            pm::IS_INTERACTIVE => {
                pm::IsInteractive::read(&mut call.data).map_err(bad)?;
                bool_reply(Ok(true))
            }
            pm::IS_DISPLAY_INTERACTIVE => {
                let a = pm::IsDisplayInteractive::read(&mut call.data).map_err(bad)?;
                bool_reply(Ok(a.display_id == DEFAULT_DISPLAY))
            }
            pm::IS_POWER_SAVE_MODE => {
                pm::IsPowerSaveMode::read(&mut call.data).map_err(bad)?;
                bool_reply(self.power_save_mode())
            }
            pm::IS_BATTERY_SAVER_SUPPORTED => {
                pm::IsBatterySaverSupported::read(&mut call.data).map_err(bad)?;
                bool_reply(self.host.inputs().map(|i| {
                    i.packages
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
            pm::IS_DEVICE_IDLE_MODE => {
                pm::IsDeviceIdleMode::read(&mut call.data).map_err(bad)?;
                bool_reply(Ok(false))
            }
            pm::IS_LIGHT_DEVICE_IDLE_MODE => {
                pm::IsLightDeviceIdleMode::read(&mut call.data).map_err(bad)?;
                bool_reply(Ok(false))
            }
            code => reply(Err(self.host.unimplemented("power", code))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!((ws.num, ws.chains), (2, Some(1)));
        assert!(!ws.is_empty());
        assert!(WorkSource::default().is_empty());
    }
}
