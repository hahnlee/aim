//! `android.hardware.gnss-service.aim`: the `IGnss/default` vendor HAL of
//! the derived image (ADR 0012 decision 3). It reports the Mac's
//! CoreLocation fixes, read through the host-call module `location`.

mod logger;
mod service;

use aim_hostcall::{guest, location, module};
use android_hardware_gnss::aidl::android::hardware::gnss::IGnss::BnGnss;
use binder::BinderFeatures;

const TAG: &str = "android.hardware.gnss-service.aim";
const INSTANCE: &str = "android.hardware.gnss.IGnss/default";

fn main() {
    logger::init(TAG);
    match guest::version(module::LOCATION) {
        Ok(v) if v >= location::VERSION => {}
        other => {
            log::error!("host module `location` unavailable or too old: {other:?}");
            std::process::exit(1);
        }
    }

    binder::ProcessState::set_thread_pool_max_thread_count(0);
    let gnss = service::Gnss::default();
    let binder = BnGnss::new_binder(gnss.clone(), BinderFeatures::default());
    if let Err(e) = binder::add_service(INSTANCE, binder.as_binder()) {
        log::error!("cannot register {INSTANCE}: {e}");
        std::process::exit(1);
    }
    log::info!("registered {INSTANCE}");
    std::thread::spawn(move || gnss.run());
    binder::ProcessState::join_thread_pool();
}
