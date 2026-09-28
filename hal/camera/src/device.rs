//! `ICameraDevice` (one per Mac camera) and `ICameraProvider`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use android_hardware_camera_common::aidl::android::hardware::camera::common::{
    CameraResourceCost::CameraResourceCost, Status::Status, VendorTagSection::VendorTagSection,
};
use android_hardware_camera_device::aidl::android::hardware::camera::device::ICameraDevice::BnCameraDevice;
use android_hardware_camera_device::aidl::android::hardware::camera::device::{
    CameraMetadata::CameraMetadata, ICameraDevice::ICameraDevice,
    ICameraDeviceCallback::ICameraDeviceCallback, ICameraDeviceSession::BnCameraDeviceSession,
    ICameraDeviceSession::ICameraDeviceSession, ICameraInjectionSession::ICameraInjectionSession,
    StreamConfiguration::StreamConfiguration,
};
use android_hardware_camera_provider::aidl::android::hardware::camera::provider::{
    CameraIdAndStreamCombination::CameraIdAndStreamCombination,
    ConcurrentCameraIdCombination::ConcurrentCameraIdCombination, ICameraProvider::ICameraProvider,
    ICameraProviderCallback::ICameraProviderCallback,
};
use binder::{BinderFeatures, Interface, Strong};

use crate::characteristics::Camera;
use crate::session::{Session, check_config, status};

/// A camera: its host index, static description and whether a session is
/// open (one at a time).
#[derive(Clone)]
pub struct Entry {
    pub index: u32,
    pub name: String,
    pub camera: Camera,
    in_use: Arc<AtomicBool>,
}

impl Entry {
    pub fn new(index: u32, camera: Camera) -> Entry {
        Entry {
            index,
            // The AIDL provider's device name: device@<major>.<minor>/<type>/<id>.
            name: format!("device@1.1/internal/{index}"),
            camera,
            in_use: Arc::new(AtomicBool::new(false)),
        }
    }
}

pub struct Device(Entry);

impl Interface for Device {}

#[allow(non_snake_case)]
impl ICameraDevice for Device {
    fn getCameraCharacteristics(&self) -> binder::Result<CameraMetadata> {
        Ok(CameraMetadata {
            metadata: self.0.camera.characteristics().to_bytes(),
        })
    }

    fn getPhysicalCameraCharacteristics(&self, _id: &str) -> binder::Result<CameraMetadata> {
        Err(status(Status::ILLEGAL_ARGUMENT))
    }

    fn getResourceCost(&self) -> binder::Result<CameraResourceCost> {
        Ok(CameraResourceCost {
            resourceCost: 100,
            conflictingDevices: Vec::new(),
        })
    }

    fn isStreamCombinationSupported(&self, c: &StreamConfiguration) -> binder::Result<bool> {
        Ok(check_config(&self.0.camera, c).is_ok())
    }

    fn open(
        &self,
        callback: &Strong<dyn ICameraDeviceCallback>,
    ) -> binder::Result<Strong<dyn ICameraDeviceSession>> {
        let e = &self.0;
        if e.in_use.swap(true, Ordering::AcqRel) {
            return Err(status(Status::CAMERA_IN_USE));
        }
        log::info!("camera {}: session opened", e.index);
        let session = Session::new(
            e.index,
            e.camera.clone(),
            callback.clone(),
            e.in_use.clone(),
        );
        Ok(BnCameraDeviceSession::new_binder(
            session,
            BinderFeatures::default(),
        ))
    }

    fn openInjectionSession(
        &self,
        _callback: &Strong<dyn ICameraDeviceCallback>,
    ) -> binder::Result<Strong<dyn ICameraInjectionSession>> {
        Err(status(Status::OPERATION_NOT_SUPPORTED))
    }

    fn setTorchMode(&self, _on: bool) -> binder::Result<()> {
        Err(status(Status::OPERATION_NOT_SUPPORTED))
    }

    fn turnOnTorchWithStrengthLevel(&self, _level: i32) -> binder::Result<()> {
        Err(status(Status::OPERATION_NOT_SUPPORTED))
    }

    fn getTorchStrengthLevel(&self) -> binder::Result<i32> {
        Err(status(Status::OPERATION_NOT_SUPPORTED))
    }
}

pub struct Provider {
    cameras: Vec<Entry>,
}

impl Provider {
    pub fn new(cameras: Vec<Entry>) -> Provider {
        Provider { cameras }
    }
}

impl Interface for Provider {}

#[allow(non_snake_case)]
impl ICameraProvider for Provider {
    fn setCallback(&self, _cb: &Strong<dyn ICameraProviderCallback>) -> binder::Result<()> {
        // The cameras are fixed at start-up: no status changes to report.
        Ok(())
    }

    fn getVendorTags(&self) -> binder::Result<Vec<VendorTagSection>> {
        Ok(Vec::new())
    }

    fn getCameraIdList(&self) -> binder::Result<Vec<String>> {
        Ok(self.cameras.iter().map(|c| c.name.clone()).collect())
    }

    fn getCameraDeviceInterface(&self, name: &str) -> binder::Result<Strong<dyn ICameraDevice>> {
        let e = self
            .cameras
            .iter()
            .find(|c| c.name == name)
            .ok_or_else(|| status(Status::ILLEGAL_ARGUMENT))?;
        Ok(BnCameraDevice::new_binder(
            Device(e.clone()),
            BinderFeatures::default(),
        ))
    }

    fn notifyDeviceStateChange(&self, _state: i64) -> binder::Result<()> {
        Ok(())
    }

    fn getConcurrentCameraIds(&self) -> binder::Result<Vec<ConcurrentCameraIdCombination>> {
        Ok(Vec::new())
    }

    fn isConcurrentStreamCombinationSupported(
        &self,
        _configs: &[CameraIdAndStreamCombination],
    ) -> binder::Result<bool> {
        Ok(false)
    }
}
