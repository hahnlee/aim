//! The service host's own binder, registered as `aim.service_host`: the
//! device's system service in system_server (java/device-services) hands
//! it the system_server bridge when system services are ready
//! (docs/system-services.md, "The system_server bridge"), and forwards
//! apps' requests for POST_NOTIFICATIONS to the Mac (#470).

use std::sync::{Arc, Weak};

use aim_binder_host::local::{Call, LocalProcess, Reply, Service};
use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::{
    dev_aim_server_ibridge as bridge, dev_aim_server_inotificationpermissioncallback as callback,
    dev_aim_server_iservicehost as host,
};

use crate::SYSTEM_UID;
use crate::notifications;
use crate::system::System;

pub struct ServiceHost {
    process: Arc<LocalProcess>,
    system: Weak<System>,
}

impl ServiceHost {
    pub fn new(process: Arc<LocalProcess>, system: &Arc<System>) -> Self {
        Self {
            process,
            system: Arc::downgrade(system),
        }
    }

    fn attach_package_bootstrap(&self, call: &mut Call<'_>) -> Reply {
        let args = host::AttachPackageBootstrapBridge::read(&mut call.data)?;
        let result = match (args.bridge, self.system.upgrade()) {
            (Some(Binder::Handle(handle)), Some(system)) => system.attach_package_bootstrap(handle),
            (None, _) | (Some(Binder::Local(_)), _) => Err(Exception::illegal_argument(
                "package bootstrap requires a remote bridge",
            )),
            (_, None) => Err(Exception::new(
                EX_ILLEGAL_STATE,
                "native system owner is unavailable",
            )),
        };
        let mut reply = Parcel::new();
        match result {
            Ok(()) => reply.write_no_exception(),
            Err(error) => reply.write_exception(&error),
        }
        Ok(reply)
    }

    fn capture_package_scan(&self, call: &mut Call<'_>) -> Reply {
        host::CapturePackageScan::read(&mut call.data)?;
        if call.data.remaining() != 0 {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        }
        let result = self
            .system
            .upgrade()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "native system owner is unavailable"))
            .and_then(|system| system.capture_package_scan());
        let mut reply = Parcel::new();
        match result {
            Ok(snapshot) => {
                let binder = self.process.add_service(Arc::new(
                    crate::package::scan_snapshot::endpoint::Endpoint::new(snapshot),
                ));
                host::write_capture_package_scan_reply(&mut reply, Some(binder));
            }
            Err(error) => reply.write_exception(&error),
        }
        Ok(reply)
    }

    fn reconcile_package_sdk_data(&self, call: &mut Call<'_>) -> Reply {
        let args = host::ReconcilePackageSdkData::read(&mut call.data)?;
        if call.data.remaining() != 0 {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        }
        let result = self
            .system
            .upgrade()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "native system owner is unavailable"))
            .and_then(|system| {
                system.reconcile_package_sdk_data(crate::package::owner::sdk_data::SdkData {
                    uuid: args.volume_uuid,
                    package_name: args.package_name,
                    sub_dir_names: args.sub_dir_names,
                    user_id: args.user_id,
                    app_id: args.app_id,
                    previous_app_id: args.previous_app_id,
                    se_info: args.se_info,
                    flags: args.flags,
                })
            });
        let mut reply = Parcel::new();
        match result {
            Ok(()) => host::write_reconcile_package_sdk_data_reply(&mut reply),
            Err(error) => reply.write_exception(&error),
        }
        Ok(reply)
    }

    fn mutate_package_signing(&self, call: &mut Call<'_>) -> Reply {
        let (old, new) = match call.code {
            host::ADD_PACKAGE_SIGNING_OVERRIDE => {
                let args = host::AddPackageSigningOverride::read(&mut call.data)?;
                (args.old_details, args.new_details)
            }
            host::REMOVE_PACKAGE_SIGNING_OVERRIDE => {
                let args = host::RemovePackageSigningOverride::read(&mut call.data)?;
                (args.old_details, None)
            }
            _ => {
                host::ClearPackageSigningOverrides::read(&mut call.data)?;
                (None, None)
            }
        };
        if call.data.remaining() != 0 {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        }
        let result = self
            .system
            .upgrade()
            .ok_or_else(|| Exception::new(EX_ILLEGAL_STATE, "native system owner is unavailable"))
            .and_then(|system| {
                system.mutate_package_signing(|owner| {
                    let read = |bytes: Option<&[u8]>| {
                        crate::package::sign::read_override_details(bytes.ok_or_else(|| {
                            Exception::illegal_argument("missing signing details")
                        })?)
                        .map_err(|status| {
                            Exception::illegal_argument(format!(
                                "invalid signing details: status {status}"
                            ))
                        })
                    };
                    let result = match call.code {
                        host::ADD_PACKAGE_SIGNING_OVERRIDE => {
                            owner.add(read(old.as_deref())?, read(new.as_deref())?)
                        }
                        host::REMOVE_PACKAGE_SIGNING_OVERRIDE => {
                            owner.remove(&read(old.as_deref())?)
                        }
                        _ => owner.clear(),
                    };
                    result.map_err(|message| Exception::new(EX_ILLEGAL_STATE, message))
                })
            });
        let mut reply = Parcel::new();
        match result {
            Ok(version) => match call.code {
                host::ADD_PACKAGE_SIGNING_OVERRIDE => {
                    host::write_add_package_signing_override_reply(&mut reply, version)
                }
                host::REMOVE_PACKAGE_SIGNING_OVERRIDE => {
                    host::write_remove_package_signing_override_reply(&mut reply, version)
                }
                _ => host::write_clear_package_signing_overrides_reply(&mut reply, version),
            },
            Err(error) => reply.write_exception(&error),
        }
        Ok(reply)
    }

    fn attach_bridge(&self, call: &mut Call<'_>) -> Reply {
        let bridge = host::AttachBridge::read(&mut call.data)?.bridge;
        let (Some(Binder::Handle(handle)), Some(system)) = (bridge, self.system.upgrade()) else {
            eprintln!("services: attachBridge without a bridge ({bridge:?})");
            return Ok(Parcel::new());
        };
        match system.attach_bridge(handle) {
            Ok(()) => eprintln!("services: system_server's bridge attached"),
            Err(e) => eprintln!("services: cannot use the bridge: {}", e.message),
        }
        // The Mac answers apps' requests for POST_NOTIFICATIONS while it
        // shows their notifications.
        if notifications::Bridge::mac().is_some() {
            let mut data = Parcel::new();
            bridge::InterceptNotificationPermissionRequests {}.write(&mut data);
            let reply = self.process.transact(
                handle,
                bridge::INTERCEPT_NOTIFICATION_PERMISSION_REQUESTS,
                &data,
                false,
            );
            match reply.map(|r| {
                bridge::read_intercept_notification_permission_requests_reply(&mut r.reader())
            }) {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(e))) => eprintln!(
                    "services: interceptNotificationPermissionRequests: {}",
                    e.message
                ),
                Ok(Err(s)) | Err(s) => {
                    eprintln!("services: interceptNotificationPermissionRequests: status {s}")
                }
            }
        }
        Ok(Parcel::new())
    }

    fn request_notification_permission(&self, call: &mut Call<'_>) -> Reply {
        let args = host::RequestNotificationPermission::read(&mut call.data)?;
        let Some(Binder::Handle(handle)) = args.callback else {
            return Ok(Parcel::new());
        };
        let target = self.process.strong(handle);
        let answer = Box::new(move |granted: bool| {
            let mut data = Parcel::new();
            callback::OnResult { granted }.write(&mut data);
            if let Err(s) = target.transact(callback::ON_RESULT, &data, true) {
                eprintln!("services: notification permission answer: status {s}");
            }
        });
        let package = args.package_name.unwrap_or_default();
        match notifications::Bridge::mac() {
            Some(mac) => mac.request_permission(package, args.user_id, answer),
            // Asked only while the Mac shows notifications (attachBridge).
            None => answer(false),
        }
        Ok(Parcel::new())
    }
}

impl Service for ServiceHost {
    fn descriptor(&self) -> &str {
        host::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code != host::ATTACH_BRIDGE
            && call.code != host::REQUEST_NOTIFICATION_PERMISSION
            && call.code != host::ATTACH_PACKAGE_BOOTSTRAP_BRIDGE
            && call.code != host::CAPTURE_PACKAGE_SCAN
            && call.code != host::RECONCILE_PACKAGE_SDK_DATA
            && call.code != host::ADD_PACKAGE_SIGNING_OVERRIDE
            && call.code != host::REMOVE_PACKAGE_SIGNING_OVERRIDE
            && call.code != host::CLEAR_PACKAGE_SIGNING_OVERRIDES
        {
            return Err(UNKNOWN_TRANSACTION);
        }
        // Only system_server's side calls it; the system uid is the one it
        // runs as.
        if call.sender_euid != SYSTEM_UID {
            if call.code == host::ATTACH_PACKAGE_BOOTSTRAP_BRIDGE
                || call.code == host::CAPTURE_PACKAGE_SCAN
                || call.code == host::RECONCILE_PACKAGE_SDK_DATA
                || call.code == host::ADD_PACKAGE_SIGNING_OVERRIDE
                || call.code == host::REMOVE_PACKAGE_SIGNING_OVERRIDE
                || call.code == host::CLEAR_PACKAGE_SIGNING_OVERRIDES
            {
                let mut reply = Parcel::new();
                reply.write_exception(&Exception::security(
                    "package bootstrap serves the system uid only",
                ));
                return Ok(reply);
            }
            eprintln!(
                "services: call {} from uid {} refused",
                call.code, call.sender_euid
            );
            return Ok(Parcel::new());
        }
        if matches!(
            call.code,
            host::ADD_PACKAGE_SIGNING_OVERRIDE
                | host::REMOVE_PACKAGE_SIGNING_OVERRIDE
                | host::CLEAR_PACKAGE_SIGNING_OVERRIDES
        ) {
            self.mutate_package_signing(call)
        } else if call.code == host::RECONCILE_PACKAGE_SDK_DATA {
            self.reconcile_package_sdk_data(call)
        } else if call.code == host::CAPTURE_PACKAGE_SCAN {
            self.capture_package_scan(call)
        } else if call.code == host::ATTACH_PACKAGE_BOOTSTRAP_BRIDGE {
            self.attach_package_bootstrap(call)
        } else if call.code == host::ATTACH_BRIDGE {
            self.attach_bridge(call)
        } else {
            self.request_notification_permission(call)
        }
    }
}

#[cfg(test)]
mod tests;
