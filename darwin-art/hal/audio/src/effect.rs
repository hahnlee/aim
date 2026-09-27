//! `android.hardware.audio.effect.IFactory/default` with no effects.
//!
//! audioserver loads the effect HAL of the same kind (AIDL) and major
//! version as the core HAL, and waits for the HIDL service manager's
//! answer when there is none; an empty factory is a valid one. The
//! original effect libraries can be served here later.

use android_hardware_audio_effect::aidl::android::hardware::audio::effect::{
    Descriptor::Descriptor, IEffect::IEffect, IFactory::IFactory, Processing::Processing,
    Processing::Type::Type as ProcessingType,
};
use android_media_audio_common_types::aidl::android::media::audio::common::AudioUuid::AudioUuid;
use binder::{ExceptionCode, Interface, Strong};

use crate::stream::exception;

pub struct Factory;

impl Interface for Factory {}

impl IFactory for Factory {
    fn queryEffects(
        &self,
        _type: Option<&AudioUuid>,
        _implementation: Option<&AudioUuid>,
        _proxy: Option<&AudioUuid>,
    ) -> binder::Result<Vec<Descriptor>> {
        Ok(Vec::new())
    }

    fn queryProcessing(&self, _type: Option<&ProcessingType>) -> binder::Result<Vec<Processing>> {
        Ok(Vec::new())
    }

    fn createEffect(&self, _uuid: &AudioUuid) -> binder::Result<Strong<dyn IEffect>> {
        exception(ExceptionCode::ILLEGAL_ARGUMENT)
    }

    fn destroyEffect(&self, _effect: &Strong<dyn IEffect>) -> binder::Result<()> {
        exception(ExceptionCode::ILLEGAL_ARGUMENT)
    }
}
