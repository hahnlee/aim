//! `android.hardware.thermal-service.darwin`: the `IThermal/default` vendor
//! HAL of the derived image (ADR 0012 decision 3). It reports the Mac's
//! thermal state and temperatures, read through the host-call module
//! `thermal`.

mod logger;
mod service;

use std::time::Duration;

use android_hardware_thermal::aidl::android::hardware::thermal::IThermal::BnThermal;
use binder::BinderFeatures;
use darwin_hostcall::{guest, module, thermal};

const TAG: &str = "android.hardware.thermal-service.darwin";
const INSTANCE: &str = "android.hardware.thermal.IThermal/default";
/// How often the host's thermal state is checked for changes. macOS moves
/// between states over tens of seconds.
const PERIOD: Duration = Duration::from_secs(5);

fn main() {
    logger::init(TAG);
    match guest::version(module::THERMAL) {
        Ok(v) if v >= thermal::VERSION => {}
        other => {
            log::error!("host module `thermal` unavailable or too old: {other:?}");
            std::process::exit(1);
        }
    }
    match guest::thermal() {
        Ok(t) => log::info!(
            "thermal state {} cpu {:.1}C battery {:.1}C",
            t.state,
            t.cpu_celsius,
            t.battery_celsius
        ),
        Err(e) => log::warn!("thermal read failed: {e:?}"),
    }

    binder::ProcessState::set_thread_pool_max_thread_count(0);
    let thermal = service::Thermal::default();
    let binder = BnThermal::new_binder(thermal.clone(), BinderFeatures::default());
    if let Err(e) = binder::add_service(INSTANCE, binder.as_binder()) {
        log::error!("cannot register {INSTANCE}: {e}");
        std::process::exit(1);
    }
    log::info!("registered {INSTANCE}");
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(PERIOD);
            thermal.poll();
        }
    });
    binder::ProcessState::join_thread_pool();
}
