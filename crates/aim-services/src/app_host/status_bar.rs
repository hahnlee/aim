//! `statusbar`: StatusBarManagerService (`android-16.0.0_r1`) for the app
//! host, of what its status bar and the Mac's notifications need: the bar
//! registers (`registerStatusBar`, the lightweight shell's `IStatusBar`
//! in guest-init, `crate::statusbar`) and hears the default display's
//! state, no icons and nothing disabled; `onNotificationClick` tells the
//! notification service a notification shown on the Mac was clicked,
//! which cancels it if it auto-cancels; a tile's listening request goes to
//! the bar. The rest is not implemented.

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, EX_NULL_POINTER, EX_UNSUPPORTED_OPERATION, Exception, Parcel, Reader,
    Result as ParcelResult,
};
use aim_service_aidl::com_android_internal_statusbar_istatusbar as bar_aidl;
use aim_service_aidl::com_android_internal_statusbar_istatusbarservice as sb;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::activity_user;
use super::{AppHost, PER_USER_RANGE, reply};
use crate::package::intent::ComponentName;
use crate::shell::{SHELL_COMMAND_TRANSACTION, ShellCommand};

/// `StatusBarManagerService`'s compat changes of tile listening requests.
const REQUEST_LISTENING_MUST_MATCH_PACKAGE: i64 = 172251878;
const REQUEST_LISTENING_OTHER_USER_NOOP: i64 = 242194868;
/// The current user (`getCurrentUserId`): the app host's one user.
const CURRENT_USER: i32 = 0;

/// A `NotificationVisibility`, read past.
struct Visibility;

impl ReadParcelable for Visibility {
    fn read_from(r: &mut Reader<'_>) -> ParcelResult<Self> {
        // key, rank, count, visible, location
        r.read_string16()?;
        for _ in 0..3 {
            r.read_i32()?;
        }
        r.read_string16()?;
        Ok(Visibility)
    }
}

/// `RegisterStatusBarResult` of the default display's `UiState` as it
/// starts: no icons, nothing disabled, the default appearance, the
/// default visible insets (all but the IME's), no package.
struct Registered;

impl WriteParcelable for Registered {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(0); // icons
        p.write_i32(0); // disabled flags 1
        p.write_i32(0); // appearance
        p.write_i32(0); // appearance regions
        p.write_i32(0); // IME window visibility
        p.write_i32(0); // BACK_DISPOSITION_DEFAULT
        p.write_bool(false); // no IME switcher
        p.write_i32(0); // disabled flags 2
        p.write_bool(false); // navigation bar color not the IME's
        p.write_i32(0); // behavior
        p.write_i32(!(1 << 3)); // WindowInsets.Type.defaultVisible()
        p.write_string16(Some("none"));
        p.write_i32(0); // transient bar types
        p.write_i32(0); // letterbox details
    }
}

pub struct StatusBar {
    host: Arc<AppHost>,
    /// The registered `IStatusBar`, while it lives.
    bar: Arc<Mutex<Option<Arc<Strong>>>>,
}

impl StatusBar {
    pub fn new(host: Arc<AppHost>) -> StatusBar {
        StatusBar {
            host,
            bar: Arc::default(),
        }
    }

    /// `enforceStatusBarService`.
    fn enforce(&self, call: &Call<'_>) -> Result<(), Exception> {
        if self.host.check_permission(
            Some("android.permission.STATUS_BAR_SERVICE"),
            call.sender_euid as i32,
        )? != 0
        {
            return Err(Exception::security(
                "StatusBarManagerService requires android.permission.STATUS_BAR_SERVICE",
            ));
        }
        Ok(())
    }

    /// StatusBarShellCommand / StatusBarManagerService, android-16.0.0_r1
    /// (AOSP Apache-2.0), services/core/java/com/android/server/statusbar/.
    fn shell(&self, call: &Call<'_>, c: &mut ShellCommand) -> Result<i32, Exception> {
        let Some(command) = c.command() else {
            c.print(HELP);
            return Ok(1);
        };
        let uid = call.sender_euid as i32;
        let enforce = |permission: &str| {
            if self.host.check_permission(Some(permission), uid)? == 0 {
                Ok(())
            } else {
                Err(Exception::security(format!(
                    "StatusBarManagerService requires {permission}"
                )))
            }
        };
        let mut p = Parcel::new();
        let code = match command {
            "help" | "-h" => {
                c.print(HELP);
                return Ok(0);
            }
            "expand-notifications" => {
                enforce("android.permission.EXPAND_STATUS_BAR")?;
                bar_aidl::AnimateExpandNotificationsPanel {}.write(&mut p);
                bar_aidl::ANIMATE_EXPAND_NOTIFICATIONS_PANEL
            }
            "expand-settings" => {
                enforce("android.permission.EXPAND_STATUS_BAR")?;
                bar_aidl::AnimateExpandSettingsPanel { sub_panel: None }.write(&mut p);
                bar_aidl::ANIMATE_EXPAND_SETTINGS_PANEL
            }
            "collapse" => {
                let permission = "android.permission.STATUS_BAR";
                // checkCanCollapseStatusBar: legacy callers without STATUS_BAR
                // also need the ATM canCloseSystemDialogs policy owner.
                if self.host.check_permission(Some(permission), uid)? != 0
                    && !self.host.change_enabled(173_031_413, uid)?
                {
                    enforce("android.permission.EXPAND_STATUS_BAR")?;
                    return Err(Exception::new(
                        EX_UNSUPPORTED_OPERATION,
                        "Legacy collapse requires unimplemented canCloseSystemDialogs policy (#2561)",
                    ));
                }
                enforce(permission)?;
                bar_aidl::AnimateCollapsePanels {}.write(&mut p);
                bar_aidl::ANIMATE_COLLAPSE_PANELS
            }
            _ => {
                c.eprintln(&format!(
                    "Error: unsupported statusbar command '{command}' (#2561)"
                ));
                return Ok(-1);
            }
        };
        let bar = self.bar.lock().unwrap().clone().ok_or_else(|| {
            Exception::new(
                EX_UNSUPPORTED_OPERATION,
                "No registered IStatusBar for panel commands",
            )
        })?;
        bar.transact(code, &p, true).map_err(|status| {
            Exception::new(
                EX_UNSUPPORTED_OPERATION,
                format!("IStatusBar panel callback failed: {status}"),
            )
        })?;
        Ok(0)
    }
}

const HELP: &str = "Status bar commands:
  help
    Print this help text.
  expand-notifications
    Open the notifications panel.
  expand-settings
    Open the notifications panel and expand quick settings if present.
  collapse
    Collapse the notifications and settings panel.
  Other commands (tiles, disable flags, tracing, GC and SystemUI passthrough)
    are unsupported by the native status bar and fail explicitly (#2561).
";

impl StatusBar {
    /// `requestTileServiceListeningState`: with
    /// REQUEST_LISTENING_MUST_MATCH_PACKAGE, the caller must be the
    /// tile's package acting for the current user (another user's request
    /// does nothing with REQUEST_LISTENING_OTHER_USER_NOOP); the bar is
    /// asked to bind the tile.
    fn request_tile_listening(
        &self,
        call: &Call<'_>,
        component: Option<ComponentName>,
        user: i32,
    ) -> Result<(), Exception> {
        let component =
            component.ok_or_else(|| Exception::new(EX_NULL_POINTER, "componentName"))?;
        let uid = call.sender_euid as i32;
        if self
            .host
            .change_enabled(REQUEST_LISTENING_MUST_MATCH_PACKAGE, uid)?
        {
            let user = self.host.handle_incoming_user(
                uid,
                user,
                false,
                activity_user::AllowMode::NonFull,
                Some(&component.package),
                None,
            )?;
            // `checkCallingUidPackage`.
            let package_uid = self.host.package_uid(&component.package, 0, user)?;
            if uid.rem_euclid(PER_USER_RANGE as i32)
                != package_uid.rem_euclid(PER_USER_RANGE as i32)
            {
                return Err(Exception::security(format!(
                    "Package {} does not belong to the calling uid {uid}",
                    component.package
                )));
            }
            if user != CURRENT_USER {
                if self
                    .host
                    .change_enabled(REQUEST_LISTENING_OTHER_USER_NOOP, uid)?
                {
                    return Ok(());
                }
                return Err(Exception::illegal_argument(format!(
                    "User {user} is not the current user."
                )));
            }
        }
        if let Some(bar) = self.bar.lock().unwrap().as_ref() {
            let mut p = Parcel::new();
            bar_aidl::RequestTileServiceListeningState {
                component_name: Some(component),
            }
            .write(&mut p);
            let _ = bar.transact(bar_aidl::REQUEST_TILE_SERVICE_LISTENING_STATE, &p, true);
        }
        Ok(())
    }
}

impl Service for StatusBar {
    fn descriptor(&self) -> &str {
        sb::DESCRIPTOR
    }

    fn accepts_fds(&self) -> bool {
        true
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            SHELL_COMMAND_TRANSACTION => {
                let mut c = ShellCommand::read(&self.host.process, &mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                if c.has_output() {
                    let status = match self.shell(call, &mut c) {
                        Ok(status) => status,
                        Err(error) => {
                            c.exception(&super::settings_service::java_exception(&error));
                            -1
                        }
                    };
                    c.finish(status);
                }
                let mut p = Parcel::new();
                p.write_no_exception();
                Ok(p)
            }
            sb::REGISTER_STATUS_BAR => {
                let a = sb::RegisterStatusBar::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply((|| {
                    self.enforce(call)?;
                    let Some(Binder::Handle(h)) = a.callbacks else {
                        return Err(Exception::illegal_argument("bar is null"));
                    };
                    let bar = self.host.process.strong(h);
                    let slot = Arc::downgrade(&self.bar);
                    self.host.process.link_to_death(
                        &bar,
                        Box::new(move || {
                            if let Some(slot) = slot.upgrade() {
                                slot.lock().unwrap().take_if(|b| b.handle == h);
                            }
                        }),
                    );
                    *self.bar.lock().unwrap() = Some(Arc::new(bar));
                    let mut p = Parcel::new();
                    sb::write_register_status_bar_reply(&mut p, Some(&Registered));
                    Ok(p)
                })())
            }
            sb::ON_NOTIFICATION_CLICK => {
                let a = sb::OnNotificationClick::<Visibility>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                reply((|| {
                    self.enforce(call)?;
                    if let (Some(key), Some(nms)) = (a.key, self.host.notification_service.get()) {
                        nms.clicked(&key)?;
                    }
                    let mut p = Parcel::new();
                    sb::write_on_notification_click_reply(&mut p);
                    Ok(p)
                })())
            }
            sb::REQUEST_TILE_SERVICE_LISTENING_STATE => {
                let a = sb::RequestTileServiceListeningState::<ComponentName>::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                reply(
                    self.request_tile_listening(call, a.component_name, a.user_id)
                        .map(|()| {
                            let mut p = Parcel::new();
                            sb::write_request_tile_service_listening_state_reply(&mut p);
                            p
                        }),
                )
            }
            code => reply(Err(self.host.unimplemented(
                "statusbar",
                self.descriptor(),
                code,
            ))),
        }
    }
}
