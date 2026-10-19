//! `usb`: UsbService for the app host (`android-16.0.0_r1`), as the
//! original runs on the app host's guest: with `FEATURE_USB_HOST` it has a
//! host manager, which finds no device (the guest has no `/dev/bus/usb`);
//! without `/sys/class/android_usb` it has no device manager, so no
//! accessory and no gadget functions; its port manager has no USB HAL, so
//! no port. Permissions, defaults, the writes and the Mac's own USB
//! devices are not implemented (#1783).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, EX_NULL_POINTER, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_hardware_usb_iusbmanager as usb;

use super::{AppHost, reply, system_boolean};

const MANAGE_USB: &str = "android.permission.MANAGE_USB";
/// `UsbManager.USB_HAL_NOT_SUPPORTED`.
const USB_HAL_NOT_SUPPORTED: i32 = -1;

/// The host manager's devices (`UsbHostManager.getDeviceList`): none.
struct NoDevices;

impl WriteParcelable for NoDevices {
    fn write_to(&self, p: &mut Parcel) {
        crate::bundle::write(p, &[]);
    }
}

/// No accessory, port or port status, of which none is written.
enum None_ {}

impl WriteParcelable for None_ {
    fn write_to(&self, _: &mut Parcel) {
        match *self {}
    }
}

pub struct UsbService {
    host: Arc<AppHost>,
}

impl UsbService {
    pub fn new(host: Arc<AppHost>) -> Self {
        UsbService { host }
    }

    /// `@EnforcePermission(MANAGE_USB)`.
    fn enforce_manage(&self, uid: i32) -> Result<(), Exception> {
        if self.host.check_permission(Some(MANAGE_USB), uid)? != 0 {
            return Err(Exception::security(format!("Access denied, requires: {MANAGE_USB}")));
        }
        Ok(())
    }

    /// `Preconditions.checkState(mDeviceManager != null)`: there is none.
    fn no_device_manager() -> Exception {
        Exception::new(EX_ILLEGAL_STATE, "")
    }

    /// `Preconditions.checkNotNull(mDeviceManager, ..)`.
    fn device_manager_null() -> Exception {
        Exception::new(EX_NULL_POINTER, "DeviceManager must not be null")
    }
}

impl Service for UsbService {
    fn descriptor(&self) -> &str {
        usb::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let uid = call.sender_euid as i32;
        let mut p = Parcel::new();
        let result = match call.code {
            usb::GET_DEVICE_LIST => {
                usb::GetDeviceList::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                usb::write_get_device_list_reply(&mut p, &usb::GetDeviceListReply { devices: Some(NoDevices) });
                Ok(())
            }
            usb::GET_CURRENT_ACCESSORY => {
                usb::GetCurrentAccessory::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                usb::write_get_current_accessory_reply::<None_>(&mut p, None);
                Ok(())
            }
            usb::IS_UVC_GADGET_SUPPORT_ENABLED => {
                usb::IsUvcGadgetSupportEnabled::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let enabled = system_boolean(self.host.property("ro.usb.uvc.enabled").as_deref(), false);
                usb::write_is_uvc_gadget_support_enabled_reply(&mut p, enabled);
                Ok(())
            }
            // `isFunctionEnabled` asks `getCurrentFunctions` of the same
            // caller.
            usb::IS_FUNCTION_ENABLED | usb::GET_CURRENT_FUNCTIONS | usb::GET_SCREEN_UNLOCKED_FUNCTIONS => {
                self.enforce_manage(uid).and(Err(Self::no_device_manager()))
            }
            usb::GET_GADGET_HAL_VERSION | usb::GET_CURRENT_USB_SPEED => {
                self.enforce_manage(uid).and(Err(Self::device_manager_null()))
            }
            usb::GET_USB_HAL_VERSION => {
                usb::GetUsbHalVersion::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce_manage(uid).map(|()| usb::write_get_usb_hal_version_reply(&mut p, USB_HAL_NOT_SUPPORTED))
            }
            usb::GET_PORTS => {
                usb::GetPorts::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                self.enforce_manage(uid).map(|()| usb::write_get_ports_reply::<None_>(&mut p, Some(&[])))
            }
            usb::GET_PORT_STATUS => {
                let a = usb::GetPortStatus::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                if a.port_id.is_none() {
                    return reply(Err(Exception::new(EX_NULL_POINTER, "portId must not be null")));
                }
                self.enforce_manage(uid).map(|()| usb::write_get_port_status_reply::<None_>(&mut p, None))
            }
            code => {
                let name = usb::METHODS.iter().find(|(c, _)| *c == code).map_or("an unknown call", |(_, n)| n);
                Err(self.host.unimplemented_named("usb", &format!("{name} (#1783)")))
            }
        };
        reply(result.map(|()| p))
    }
}
