//! `android.hardware.health-service.aim`: the `IHealth/default` vendor
//! HAL of the derived image (ADR 0012 decision 3). It runs from
//! `/vendor/bin/hw` like any vendor HAL and reads the Mac's battery through
//! the host-call module `health`.

mod logger;
mod service;

use std::time::Duration;

use aim_hostcall::{guest, health, module};
use android_hardware_health::aidl::android::hardware::health::IHealth::BnHealth;
use binder::BinderFeatures;

const TAG: &str = "android.hardware.health-service.aim";
const INSTANCE: &str = "android.hardware.health.IHealth/default";
/// AOSP health's fast periodic chore: callbacks hear from us at least this
/// often (there are no power-supply uevents to wake us sooner).
const PERIOD: Duration = Duration::from_secs(60);

fn main() {
    logger::init(TAG);
    match guest::version(module::HEALTH) {
        Ok(v) if v >= health::VERSION => {}
        other => {
            log::error!("host module `health` unavailable or too old: {other:?}");
            std::process::exit(1);
        }
    }
    match service::health_info() {
        Ok(info) => log::info!(
            "battery present={} level={}% status={:?} ac={}",
            info.batteryPresent,
            info.batteryLevel,
            info.batteryStatus,
            info.chargerAcOnline
        ),
        Err(e) => log::warn!("battery read failed: {e}"),
    }

    binder::ProcessState::set_thread_pool_max_thread_count(0);
    let health = service::Health::default();
    let binder = BnHealth::new_binder(health.clone(), BinderFeatures::default());
    if let Err(e) = binder::add_service(INSTANCE, binder.as_binder()) {
        log::error!("cannot register {INSTANCE}: {e}");
        std::process::exit(1);
    }
    log::info!("registered {INSTANCE}");
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(PERIOD);
            health.notify();
        }
    });
    binder::ProcessState::join_thread_pool();
}
