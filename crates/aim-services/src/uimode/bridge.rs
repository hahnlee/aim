//! The uimode service's side of the system_server bridge (`IUiModeHost`,
//! java/device-services): what the original learns inside system_server
//! comes here (docking, charging, power save, twilight, the screen, the
//! clock, restored settings, shutdown, VR, users, a dock broadcast's
//! result), and what it does inside system_server goes through
//! `IUiModeBridge` (the configuration, an app's night mode, the client
//! caches, broadcasts, the car mode's notification, dock apps and dreams,
//! the wake lock, content observers).

use std::sync::Weak;

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    Binder, Parcel, Reader, Result as ParcelResult, UNKNOWN_TRANSACTION,
};
use aim_service_aidl::{
    ReadParcelable, android_database_icontentobserver as observer,
    dev_aim_server_iuimodebridge as iub, dev_aim_server_iuimodehost as iuh,
};

use super::UiModeManagerService;

/// `IUiModeBridge.getConfig`, by index.
pub struct Config {
    pub default_ui_mode_type: i32,
    pub car_mode_keeps_screen_on: bool,
    pub desk_mode_keeps_screen_on: bool,
    pub start_dream_immediately_on_dock: bool,
    pub dreams_disabled_by_ambient_mode_suppression: bool,
    pub enable_car_dock_launch: bool,
    pub ui_mode_locked: bool,
    pub television: bool,
    pub car: bool,
    pub watch: bool,
    pub force_invert_color: bool,
}

/// What an observed setting's change affects.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Setting {
    /// `Secure.UI_NIGHT_MODE` (`mDarkThemeObserver`).
    NightMode,
    /// `ACCESSIBILITY_FORCE_INVERT_COLOR_ENABLED`.
    ForceInvert,
    /// `CONTRAST_LEVEL`.
    Contrast,
    /// `USER_SETUP_COMPLETE` (`mSetupWizardObserver`).
    SetupComplete,
}

/// The bridge, once system_server handed it over.
pub struct Bridge {
    pub strong: Strong,
}

impl Bridge {
    fn call<T>(
        &self,
        code: u32,
        write: impl FnOnce(&mut Parcel),
        read: impl FnOnce(&mut Reader<'_>) -> ParcelResult<aim_service_aidl::Returned<T>>,
    ) -> Option<T> {
        let mut data = Parcel::new();
        write(&mut data);
        let reply = self.strong.transact(code, &data, false).ok()?;
        match read(&mut reply.reader()) {
            Ok(Ok(v)) => Some(v),
            Ok(Err(e)) => {
                eprintln!("uimode: bridge call {code}: {}", e.message);
                None
            }
            Err(s) => {
                eprintln!("uimode: bridge call {code}: status {s}");
                None
            }
        }
    }

    pub fn config(&self) -> Option<Config> {
        let v = self
            .call(
                iub::GET_CONFIG,
                |p| iub::GetConfig {}.write(p),
                iub::read_get_config_reply,
            )
            .flatten()?;
        let at = |i: usize| v.get(i).copied().unwrap_or(0);
        Some(Config {
            default_ui_mode_type: at(0),
            car_mode_keeps_screen_on: at(1) == 1,
            desk_mode_keeps_screen_on: at(2) == 1,
            start_dream_immediately_on_dock: at(3) != 0,
            dreams_disabled_by_ambient_mode_suppression: at(4) != 0,
            enable_car_dock_launch: at(5) != 0,
            ui_mode_locked: at(6) != 0,
            television: at(7) != 0,
            car: at(8) != 0,
            watch: at(9) != 0,
            force_invert_color: at(10) != 0,
        })
    }

    pub fn register_content_observer(&self, uri: &str, observer: Binder, user_id: i32) {
        let args = iub::RegisterContentObserver {
            uri: Some(uri.into()),
            notify_for_descendants: false,
            observer: Some(observer),
            user_id,
        };
        self.call(
            iub::REGISTER_CONTENT_OBSERVER,
            |p| args.write(p),
            iub::read_register_content_observer_reply,
        );
    }

    pub fn update_configuration(&self, ui_mode: i32) {
        self.call(
            iub::UPDATE_CONFIGURATION,
            |p| iub::UpdateConfiguration { ui_mode }.write(p),
            iub::read_update_configuration_reply,
        );
    }

    pub fn set_application_night_mode(&self, pid: i32, uid: i32, config_night_mode: i32) {
        let args = iub::SetApplicationNightMode {
            pid,
            uid,
            config_night_mode,
        };
        self.call(
            iub::SET_APPLICATION_NIGHT_MODE,
            |p| args.write(p),
            iub::read_set_application_night_mode_reply,
        );
    }

    pub fn invalidate_night_mode_cache(&self) {
        self.call(
            iub::INVALIDATE_NIGHT_MODE_CACHE,
            |p| iub::InvalidateNightModeCache {}.write(p),
            iub::read_invalidate_night_mode_cache_reply,
        );
    }

    pub fn invalidate_current_mode_type_cache(&self) {
        self.call(
            iub::INVALIDATE_CURRENT_MODE_TYPE_CACHE,
            |p| iub::InvalidateCurrentModeTypeCache {}.write(p),
            iub::read_invalidate_current_mode_type_cache_reply,
        );
    }

    pub fn set_device_theme(&self, theme: &str) {
        let args = iub::SetDeviceTheme {
            theme: Some(theme.into()),
        };
        self.call(
            iub::SET_DEVICE_THEME,
            |p| args.write(p),
            iub::read_set_device_theme_reply,
        );
    }

    /// Interactive and not dreaming; active when it cannot be asked.
    pub fn is_device_active(&self) -> bool {
        self.call(
            iub::IS_DEVICE_ACTIVE,
            |p| iub::IsDeviceActive {}.write(p),
            iub::read_is_device_active_reply,
        )
        .unwrap_or(true)
    }

    pub fn set_twilight_listening(&self, listening: bool) {
        self.call(
            iub::SET_TWILIGHT_LISTENING,
            |p| iub::SetTwilightListening { listening }.write(p),
            iub::read_set_twilight_listening_reply,
        );
    }

    /// The last twilight state: at night, by day, or none.
    pub fn twilight(&self) -> Option<bool> {
        let state = self.call(
            iub::GET_TWILIGHT_STATE,
            |p| iub::GetTwilightState {}.write(p),
            iub::read_get_twilight_state_reply,
        )?;
        (state >= 0).then_some(state == 1)
    }

    pub fn dump_twilight(&self) -> Option<String> {
        self.call(
            iub::DUMP_TWILIGHT_STATE,
            |p| iub::DumpTwilightState {}.write(p),
            iub::read_dump_twilight_state_reply,
        )
        .flatten()
    }

    pub fn send_car_mode_broadcast(&self, enabled: bool, priority: i32, package: Option<String>) {
        let args = iub::SendCarModeBroadcast {
            enabled,
            priority,
            package_name: package,
        };
        self.call(
            iub::SEND_CAR_MODE_BROADCAST,
            |p| args.write(p),
            iub::read_send_car_mode_broadcast_reply,
        );
    }

    pub fn send_foreground_broadcast(&self, action: &str) {
        let args = iub::SendForegroundBroadcastToAllUsers {
            action: Some(action.into()),
        };
        self.call(
            iub::SEND_FOREGROUND_BROADCAST_TO_ALL_USERS,
            |p| args.write(p),
            iub::read_send_foreground_broadcast_to_all_users_reply,
        );
    }

    pub fn send_dock_broadcast(&self, action: &str, enable_flags: i32, disable_flags: i32) {
        let args = iub::SendDockBroadcast {
            action: Some(action.into()),
            enable_flags,
            disable_flags,
        };
        self.call(
            iub::SEND_DOCK_BROADCAST,
            |p| args.write(p),
            iub::read_send_dock_broadcast_reply,
        );
    }

    pub fn set_car_mode_status_bar(&self, car_mode: bool) {
        self.call(
            iub::SET_CAR_MODE_STATUS_BAR,
            |p| iub::SetCarModeStatusBar { car_mode }.write(p),
            iub::read_set_car_mode_status_bar_reply,
        );
    }

    pub fn start_dock_app(&self, category: &str, ui_mode: i32) -> bool {
        let args = iub::StartDockApp {
            category: Some(category.into()),
            ui_mode,
        };
        self.call(
            iub::START_DOCK_APP,
            |p| args.write(p),
            iub::read_start_dock_app_reply,
        )
        .unwrap_or(false)
    }

    pub fn start_dream_if_docked(&self, start_immediately: bool, disabled: bool) {
        let args = iub::StartDreamIfDocked {
            start_immediately,
            disabled_by_ambient_mode_suppression: disabled,
        };
        self.call(
            iub::START_DREAM_IF_DOCKED,
            |p| args.write(p),
            iub::read_start_dream_if_docked_reply,
        );
    }

    pub fn set_keep_screen_on(&self, on: bool) {
        self.call(
            iub::SET_KEEP_SCREEN_ON,
            |p| iub::SetKeepScreenOn { on }.write(p),
            iub::read_set_keep_screen_on_reply,
        );
    }

    pub fn is_system_ui_in_dark_theme(&self) -> bool {
        self.call(
            iub::IS_SYSTEM_UI_IN_DARK_THEME,
            |p| iub::IsSystemUiInDarkTheme {}.write(p),
            iub::read_is_system_ui_in_dark_theme_reply,
        )
        .unwrap_or(false)
    }
}

/// A setting's `IContentObserver`.
pub struct Observer {
    pub service: Weak<UiModeManagerService>,
    pub setting: Setting,
}

impl Service for Observer {
    fn descriptor(&self) -> &str {
        observer::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let r = &mut call.data;
        let self_change = match call.code {
            observer::ON_CHANGE => observer::OnChange::<Uri>::read(r)?.self_update,
            observer::ON_CHANGE_ETC => {
                r.enforce_interface(observer::DESCRIPTOR)?;
                r.read_bool()?
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        if let Some(service) = self.service.upgrade() {
            service.on_setting_changed(self.setting, self_change);
        }
        Ok(Parcel::new())
    }
}

/// A `Uri`, read past.
pub struct Uri;

impl ReadParcelable for Uri {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        crate::clip::uri(r).map(|_| Uri)
    }
}

/// The service host's `IUiModeHost`.
pub struct Host {
    pub service: Weak<UiModeManagerService>,
}

impl Service for Host {
    fn descriptor(&self) -> &str {
        iuh::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        // Only system_server's side of the bridge calls.
        if call.sender_euid != crate::SYSTEM_UID {
            return Err(aim_binder_host::parcel::PERMISSION_DENIED);
        }
        let Some(s) = self.service.upgrade() else {
            return Ok(Parcel::new());
        };
        let r = &mut call.data;
        match call.code {
            iuh::ON_DOCK_EVENT => s.on_dock_event(iuh::OnDockEvent::read(r)?.state),
            iuh::ON_BATTERY_CHANGED => {
                s.on_battery_changed(iuh::OnBatteryChanged::read(r)?.charging)
            }
            iuh::ON_POWER_SAVE_CHANGED => {
                s.on_power_save_changed(iuh::OnPowerSaveChanged::read(r)?.battery_saver_enabled)
            }
            iuh::ON_TWILIGHT_STATE_CHANGED => {
                iuh::OnTwilightStateChanged::read(r)?;
                s.on_twilight_state_changed();
            }
            iuh::ON_DEVICE_INACTIVE => {
                iuh::OnDeviceInactive::read(r)?;
                s.on_device_inactive();
            }
            iuh::ON_TIME_CHANGED => {
                iuh::OnTimeChanged::read(r)?;
                s.on_time_changed();
            }
            iuh::ON_SETTING_RESTORED => {
                let name = iuh::OnSettingRestored::read(r)?.name;
                s.on_setting_restored(name.as_deref());
            }
            iuh::ON_SHUTDOWN => {
                iuh::OnShutdown::read(r)?;
                s.on_shutdown();
            }
            iuh::ON_VR_STATE_CHANGED => {
                s.on_vr_state_changed(iuh::OnVrStateChanged::read(r)?.enabled)
            }
            iuh::ON_USER_SWITCHING => {
                let a = iuh::OnUserSwitching::read(r)?;
                s.on_user_switching(a.from_user_id, a.to_user_id);
            }
            iuh::ON_DOCK_BROADCAST_RESULT => {
                let a = iuh::OnDockBroadcastResult::read(r)?;
                s.on_dock_broadcast_result(
                    a.action.as_deref().unwrap_or_default(),
                    a.enable_flags,
                    a.disable_flags,
                    a.result_code,
                );
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        }
        Ok(Parcel::new())
    }
}
