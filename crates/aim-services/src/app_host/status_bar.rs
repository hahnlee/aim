//! `statusbar`: StatusBarManagerService (`android-16.0.0_r1`) for the app
//! host, of what its status bar and the Mac's notifications need: the bar
//! registers (`registerStatusBar`, the lightweight shell's `IStatusBar`
//! in guest-init, `crate::statusbar`) and hears the default display's
//! state, no icons and nothing disabled; `onNotificationClick` tells the
//! notification service a notification shown on the Mac was clicked,
//! which cancels it if it auto-cancels. The rest is not implemented.

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service, Strong};
use aim_binder_host::parcel::{
    BAD_VALUE, Binder, Exception, Parcel, Reader, Result as ParcelResult,
};
use aim_service_aidl::com_android_internal_statusbar_istatusbarservice as sb;
use aim_service_aidl::{ReadParcelable, WriteParcelable};

use super::{AppHost, reply};

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
    bar: Arc<Mutex<Option<Strong>>>,
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
}

impl Service for StatusBar {
    fn descriptor(&self) -> &str {
        sb::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
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
                    *self.bar.lock().unwrap() = Some(bar);
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
            code => reply(Err(self.host.unimplemented("statusbar", code))),
        }
    }
}
