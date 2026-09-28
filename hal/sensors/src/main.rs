//! `android.hardware.sensors-service.aim`: the `ISensors/default` vendor
//! HAL of the derived image (ADR 0012 decision 3). It offers the Mac's
//! ambient light sensor and lid angle, read through the host-call module
//! `sensors`, when the Mac has them.

mod fmq;
mod logger;
mod service;

use aim_hostcall::{guest, module, sensors};
use android_hardware_sensors::aidl::android::hardware::sensors::ISensors::BnSensors;
use binder::BinderFeatures;

const TAG: &str = "android.hardware.sensors-service.aim";
const INSTANCE: &str = "android.hardware.sensors.ISensors/default";

fn main() {
    logger::init(TAG);
    match guest::version(module::SENSORS) {
        Ok(v) if v >= sensors::VERSION => {}
        other => {
            log::error!("host module `sensors` unavailable or too old: {other:?}");
            std::process::exit(1);
        }
    }
    let present = match guest::sensors() {
        Ok(r) => {
            log::info!(
                "host sensors {:#x}: light {} lux, lid {} degrees",
                r.present,
                r.light_lux,
                r.hinge_degrees
            );
            r.present
        }
        Err(e) => {
            log::warn!("sensor read failed: {e:?}; offering no sensors");
            0
        }
    };

    binder::ProcessState::set_thread_pool_max_thread_count(0);
    let hal = service::Sensors::new(present);
    let binder = BnSensors::new_binder(hal.clone(), BinderFeatures::default());
    if let Err(e) = binder::add_service(INSTANCE, binder.as_binder()) {
        log::error!("cannot register {INSTANCE}: {e}");
        std::process::exit(1);
    }
    log::info!("registered {INSTANCE}");
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(hal.poll());
        }
    });
    binder::ProcessState::join_thread_pool();
}
