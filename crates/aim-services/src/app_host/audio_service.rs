//! `audio`: AudioService for the app host, what a view's click asks of it
//! (`AudioService` at `android-16.0.0_r1`): whether navigation repeats
//! sound, and its sound effects, which play only while
//! `Settings.System.SOUND_EFFECTS_ENABLED` is set. The app host plays none
//! yet (#1433).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Parcel};
use aim_service_aidl::android_media_iaudioservice as audio;

use super::settings::Table;
use super::{AppHost, reply};

/// `Settings.System.SOUND_EFFECTS_ENABLED`.
const SOUND_EFFECTS_ENABLED: &str = "sound_effects_enabled";

pub struct AudioService {
    host: Arc<AppHost>,
    navigation_repeat: AtomicBool,
}

impl AudioService {
    pub fn new(host: Arc<AppHost>) -> Self {
        AudioService { host, navigation_repeat: AtomicBool::new(false) }
    }
}

impl Service for AudioService {
    fn descriptor(&self) -> &str {
        audio::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match call.code {
            audio::ARE_NAVIGATION_REPEAT_SOUND_EFFECTS_ENABLED => {
                audio::AreNavigationRepeatSoundEffectsEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                audio::write_are_navigation_repeat_sound_effects_enabled_reply(&mut p, self.navigation_repeat.load(Ordering::Relaxed));
            }
            audio::SET_NAVIGATION_REPEAT_SOUND_EFFECTS_ENABLED => {
                let args = audio::SetNavigationRepeatSoundEffectsEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.navigation_repeat.store(args.enabled, Ordering::Relaxed);
            }
            audio::PLAY_SOUND_EFFECT => {
                audio::PlaySoundEffect::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let enabled = match self.host.inputs() {
                    Ok(inputs) => inputs.settings.get(Table::System, SOUND_EFFECTS_ENABLED).and_then(|v| v.parse::<i32>().ok()).unwrap_or(0) != 0,
                    Err(e) => return reply(Err(e)),
                };
                if enabled {
                    return reply(Err(self.host.unimplemented_named("audio", "playSoundEffect with sound effects enabled")));
                }
            }
            code => return reply(Err(self.host.unimplemented("audio", code))),
        }
        Ok(p)
    }
}
