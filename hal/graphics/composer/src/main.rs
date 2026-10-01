//! `android.hardware.graphics.composer3-service.aim`: the
//! `IComposer/default` V4 vendor HAL of the derived image
//! (`docs/composer.md`).
//!
//! It serves one display, shown by the display server (`aim-display`),
//! which it reaches through the host-call module `display`. In device mode
//! every layer is composed by SurfaceFlinger's RenderEngine into the
//! client target, which the server shows without copying it; in window
//! mode the server composes each Mac window from its task's layers
//! (`docs/layers.md`).

mod client;
mod cursor;
mod display;
mod host;
mod logger;

use std::sync::Arc;

use aim_hostcall::{display as abi, guest, module};
use android_hardware_graphics_composer3::aidl::android::hardware::graphics::composer3::{
    Capability::Capability,
    IComposer::{BnComposer, IComposer},
    IComposerClient::{BnComposerClient, IComposerClient},
};
use binder::{BinderFeatures, Strong};

use client::{Client, Shared};
use host::Host;

const TAG: &str = "android.hardware.graphics.composer3-service.aim";
const INSTANCE: &str = "android.hardware.graphics.composer3.IComposer/default";

struct Composer(Arc<Shared>);

impl binder::Interface for Composer {}

#[allow(non_snake_case)]
impl IComposer for Composer {
    fn createClient(&self) -> binder::Result<Strong<dyn IComposerClient>> {
        // One client at a time: a new one (a restarted SurfaceFlinger)
        // starts from a clean display.
        self.0.display.lock().unwrap().reset(&self.0.host);
        *self.0.callback.lock().unwrap() = None;
        Ok(BnComposerClient::new_binder(
            Client(self.0.clone()),
            BinderFeatures::default(),
        ))
    }

    fn getCapabilities(&self) -> binder::Result<Vec<Capability>> {
        // Present fences signal when the frame was shown (docs/composer.md),
        // so SurfaceFlinger predicts vsync from them and turns vsync off.
        Ok(Vec::new())
    }
}

fn main() {
    logger::init(TAG);
    match guest::version(module::DISPLAY) {
        Ok(v) if v >= abi::VERSION => {}
        other => {
            log::error!("host module `display` unavailable or too old: {other:?}");
            std::process::exit(1);
        }
    }
    let host = match Host::connect() {
        Ok(host) => host,
        Err(e) => {
            log::error!("no display server (linux-run --display): {e:?}");
            std::process::exit(1);
        }
    };
    let i = host.info;
    log::info!(
        "display {}x{} pixels, {:.1}x{:.1} dpi, vsync period {} ns",
        i.width,
        i.height,
        i.dpi_x_milli as f64 / 1000.0,
        i.dpi_y_milli as f64 / 1000.0,
        i.vsync_period_ns
    );
    let shared = Shared::new(host);

    binder::ProcessState::set_thread_pool_max_thread_count(4);
    binder::ProcessState::start_thread_pool();
    let composer = BnComposer::new_binder(Composer(shared.clone()), BinderFeatures::default());
    if let Err(e) = binder::add_service(INSTANCE, composer.as_binder()) {
        log::error!("cannot register {INSTANCE}: {e}");
        std::process::exit(1);
    }
    log::info!("registered {INSTANCE}");
    shared.run_vsync();
    log::error!("the display server went away");
    std::process::exit(1);
}
