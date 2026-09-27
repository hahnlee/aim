//! `android.hardware.audio.service-aidl.darwin`: the audio HAL of the
//! derived image (ADR 0012 decision 3, `docs/audio.md`). It runs from
//! `/vendor/bin/hw` like any vendor HAL and serves
//!
//! - `android.hardware.audio.core.IModule/default`, the primary module over
//!   the Mac's default output and input devices;
//! - `android.hardware.audio.core.IConfig/default`, with no surround
//!   formats and an empty engine configuration (the policy engine's
//!   defaults);
//! - `android.hardware.audio.effect.IFactory/default`, with no effects.
//!
//! Streams play and capture through the host-call module `audio`
//! (CoreAudio).

mod config;
mod effect;
mod host;
mod logger;
mod module;
mod stream;

use android_hardware_audio_core::aidl::android::hardware::audio::core::{
    IConfig::{BnConfig, IConfig},
    IModule::BnModule,
    SurroundSoundConfig::SurroundSoundConfig,
};
use android_hardware_audio_effect::aidl::android::hardware::audio::effect::IFactory::BnFactory;
use android_media_audio_common_types::aidl::android::media::audio::common::AudioHalEngineConfig::AudioHalEngineConfig;
use binder::{BinderFeatures, Interface};
use darwin_hostcall::{audio, guest, module as hostcall_module};

const TAG: &str = "android.hardware.audio.service-aidl.darwin";

struct Config;

impl Interface for Config {}

impl IConfig for Config {
    fn getSurroundSoundConfig(&self) -> binder::Result<SurroundSoundConfig> {
        Ok(SurroundSoundConfig::default())
    }

    /// Empty: the policy engine uses its default product strategies.
    fn getEngineConfig(&self) -> binder::Result<AudioHalEngineConfig> {
        Ok(AudioHalEngineConfig::default())
    }
}

fn register(name: &str, binder: binder::SpIBinder) {
    if let Err(e) = binder::add_service(name, binder) {
        log::error!("cannot register {name}: {e}");
        std::process::exit(1);
    }
    log::info!("registered {name}");
}

fn describe(kind: &str, d: &audio::Device) {
    let name = String::from_utf8_lossy(&d.name);
    log::info!(
        "{kind}: {} ({} Hz, {} ch, buffer {} frames, latency {} frames)",
        name.trim_end_matches('\0'),
        d.sample_rate,
        d.channels,
        d.buffer_frames,
        d.latency_frames
    );
}

fn main() {
    logger::init(TAG);
    match guest::version(hostcall_module::AUDIO) {
        Ok(v) if v >= audio::VERSION => {}
        other => {
            log::error!("host module `audio` unavailable or too old: {other:?}");
            std::process::exit(1);
        }
    }
    let devices = match guest::audio_devices() {
        Ok(d) => d,
        Err(e) => {
            log::error!("cannot query the host's audio devices: errno {}", e.0);
            std::process::exit(1);
        }
    };
    describe("output", &devices.output);
    describe("input", &devices.input);

    binder::ProcessState::set_thread_pool_max_thread_count(4);
    binder::ProcessState::start_thread_pool();
    register(
        "android.hardware.audio.core.IConfig/default",
        BnConfig::new_binder(Config, BinderFeatures::default()).as_binder(),
    );
    register(
        "android.hardware.audio.core.IModule/default",
        BnModule::new_binder(module::Module::new(devices), BinderFeatures::default()).as_binder(),
    );
    register(
        "android.hardware.audio.effect.IFactory/default",
        BnFactory::new_binder(effect::Factory, BinderFeatures::default()).as_binder(),
    );
    binder::ProcessState::join_thread_pool();
}
