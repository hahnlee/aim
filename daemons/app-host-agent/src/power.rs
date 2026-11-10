//! PhoneWindowManager's sleep and wake keys (`interceptKeyBeforeQueueing`,
//! android-16.0.0_r1), which the app host's window manager policy has no
//! other part of (#1434): `KEYCODE_SLEEP`'s release goes to sleep
//! (`sleepRelease`: `goToSleep(GO_TO_SLEEP_REASON_SLEEP_BUTTON)`),
//! `KEYCODE_WAKEUP`'s press wakes (`WindowWakeUpPolicy.wakeUpFromKey`:
//! `wakeUp(WAKE_REASON_WAKE_KEY, "android.policy:KEY")`), and neither
//! reaches a window. The Mac's screens press them (docs/power.md).
use super::*;
use aim_framework_aidl::android_os_ipowermanager as api;
use aim_input_transport::keyboard::{ACTION_DOWN, ACTION_UP, KeyEvent};

/// `KeyEvent.KEYCODE_SLEEP`, `KEYCODE_WAKEUP`.
const KEYCODE_SLEEP: i32 = 223;
const KEYCODE_WAKEUP: i32 = 224;
/// `PowerManager.GO_TO_SLEEP_REASON_SLEEP_BUTTON`, `WAKE_REASON_WAKE_KEY`.
const GO_TO_SLEEP_REASON_SLEEP_BUTTON: i32 = 6;
const WAKE_REASON_WAKE_KEY: i32 = 6;

trait Power: Interface {}
declare_binder_interface! {
    Power[api::DESCRIPTOR] {
        native: BnPower(unused),
        proxy: BpPower,
    }
}
impl Power for BpPower {}
impl Power for binder::binder_impl::Binder<BnPower> {}
fn unused(
    _: &dyn Power,
    _: u32,
    _: &BorrowedParcel<'_>,
    _: &mut BorrowedParcel<'_>,
) -> Result<(), StatusCode> {
    Err(StatusCode::UNKNOWN_TRANSACTION)
}

/// Whether `key` is a sleep or wake key, applied to the power service.
pub(crate) fn key(key: &KeyEvent) -> bool {
    // `KeyEvent.getEventTime`: uptime milliseconds.
    let time = key.event_time / 1_000_000;
    let result = match (key.key_code, key.action) {
        (KEYCODE_SLEEP, ACTION_UP) => call(api::GO_TO_SLEEP, |p| {
            p.write(&time)?;
            p.write(&GO_TO_SLEEP_REASON_SLEEP_BUTTON)?;
            p.write(&0i32)
        }),
        (KEYCODE_WAKEUP, ACTION_DOWN) => call(api::WAKE_UP, |p| {
            p.write(&time)?;
            p.write(&WAKE_REASON_WAKE_KEY)?;
            p.write("android.policy:KEY")?;
            p.write("android")
        }),
        (KEYCODE_SLEEP | KEYCODE_WAKEUP, _) => Ok(()),
        _ => return false,
    };
    if let Err(error) = result {
        log::error!("power key {}: {error}", key.key_code);
    }
    true
}

fn call(
    code: u32,
    write: impl FnOnce(&mut binder::binder_impl::Parcel) -> Result<(), StatusCode>,
) -> Result<(), Status> {
    let Some(mut power) = binder::check_service("power") else {
        return Err(StatusCode::NAME_NOT_FOUND.into());
    };
    if !power.associate_class(BnPower::get_class()) {
        return Err(StatusCode::BAD_TYPE.into());
    }
    let mut data = power.prepare_transact()?;
    write(&mut data)?;
    let status: Status = power.submit_transact(code, data, 0)?.read()?;
    if status.is_ok() { Ok(()) } else { Err(status) }
}
