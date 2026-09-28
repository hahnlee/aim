//! `android.hardware.camera.provider-service.darwin`: the
//! `ICameraProvider/internal/0` vendor HAL of the derived image (ADR 0012
//! decision 3). One LIMITED camera device per Mac camera, streaming
//! through the host-call module `camera` (AVFoundation). See
//! docs/camera.md.

mod buffer;
mod characteristics;
mod device;
mod logger;
mod metadata;
mod session;

use android_hardware_camera_provider::aidl::android::hardware::camera::provider::ICameraProvider::BnCameraProvider;
use binder::BinderFeatures;
use darwin_hostcall::{camera, guest, module};

const TAG: &str = "android.hardware.camera.provider-service.darwin";
const INSTANCE: &str = "android.hardware.camera.provider.ICameraProvider/internal/0";

fn text(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

fn main() {
    logger::init(TAG);
    match guest::version(module::CAMERA) {
        Ok(v) if v >= camera::VERSION => {}
        other => {
            log::error!("host module `camera` unavailable or too old: {other:?}");
            std::process::exit(1);
        }
    }
    let devices = guest::camera_devices().unwrap_or_default();
    let access = match devices.access {
        camera::access::AUTHORIZED => "granted",
        camera::access::NOT_DETERMINED => "not asked yet",
        _ => "denied",
    };
    let mut cameras = Vec::new();
    for (i, d) in devices.devices[..devices.count as usize].iter().enumerate() {
        let sizes = &d.sizes[..d.size_count as usize];
        let Some(c) = characteristics::Camera::new(d.facing, sizes) else {
            log::warn!("camera {:?}: no size at 30 fps", text(&d.name));
            continue;
        };
        log::info!(
            "camera {i}: {:?} ({}), sizes {:?}; camera access {access}",
            text(&d.name),
            ["front", "back", "external"][d.facing.min(2) as usize],
            c.sizes
        );
        cameras.push(device::Entry::new(i as u32, c));
    }

    binder::ProcessState::set_thread_pool_max_thread_count(4);
    binder::ProcessState::start_thread_pool();
    let provider = device::Provider::new(cameras);
    let binder = BnCameraProvider::new_binder(provider, BinderFeatures::default());
    if let Err(e) = binder::add_service(INSTANCE, binder.as_binder()) {
        log::error!("cannot register {INSTANCE}: {e}");
        std::process::exit(1);
    }
    log::info!("registered {INSTANCE}");
    binder::ProcessState::join_thread_pool();
}
