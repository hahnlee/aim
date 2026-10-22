//! `IInstallerRecoveryPresentation` for the app host: the decoder of the
//! installer sessions' persisted icons, which system_server's
//! `InstallerRecoveryPresentation` decodes with `BitmapFactory` into a
//! `Bitmap`'s parcel (`android-16.0.0_r1`). The app host does not own that
//! decode (#2155).

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_NULL_POINTER, Exception, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_iinstallerrecoverypresentation as presentation;

use super::{AppHost, reply};

const SYSTEM_UID: u32 = 1000;

pub struct InstallerRecovery {
    host: Arc<AppHost>,
}

impl InstallerRecovery {
    pub fn new(host: Arc<AppHost>) -> Self {
        InstallerRecovery { host }
    }
}

impl Service for InstallerRecovery {
    fn descriptor(&self) -> &str {
        presentation::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        match call.code {
            presentation::DECODE_ICON => {
                let a = presentation::DecodeIcon::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                reply(Err(if call.sender_euid != SYSTEM_UID {
                    Exception::security("native package leaf requires system UID")
                } else if a.png.is_none() {
                    Exception::new(EX_NULL_POINTER, "")
                } else {
                    self.host
                        .unimplemented_named("installer recovery", "decodeIcon (#2155)")
                }))
            }
            _ => Err(UNKNOWN_TRANSACTION),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::parcel::{EX_UNSUPPORTED_OPERATION, Parcel};

    /// A PNG is not decoded but names its issue; none is a null pointer.
    #[test]
    fn an_icon_is_not_decoded() {
        let process = aim_binder_host::local::LocalProcess::open(
            &aim_binder_driver::Driver::new(),
            aim_binder_driver::Device::Binder,
            aim_binder_driver::Credentials {
                pid: std::process::id() as i32,
                euid: SYSTEM_UID,
                security_context: None,
            },
        );
        let host = AppHost::new(process.clone());
        let aim_binder_host::parcel::Binder::Local(ptr) =
            process.add_service(Arc::new(InstallerRecovery::new(host)))
        else {
            unreachable!()
        };
        let node = process.local_service(ptr).unwrap();
        let decode = |png: Option<Vec<u8>>| {
            let mut p = Parcel::new();
            presentation::DecodeIcon { png }.write(&mut p);
            let reply = node.transact(presentation::DECODE_ICON, &p, false).unwrap();
            presentation::read_decode_icon_reply(&mut reply.reader())
                .unwrap()
                .unwrap_err()
        };
        let e = decode(Some(b"\x89PNG".to_vec()));
        assert_eq!(e.code, EX_UNSUPPORTED_OPERATION);
        assert!(e.message.contains("#2155"), "{}", e.message);
        assert_eq!(decode(None).code, EX_NULL_POINTER);
    }
}
