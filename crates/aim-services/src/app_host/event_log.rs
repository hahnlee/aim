//! Binary EventLog payloads, per liblog's `log_event_list.cpp` and
//! `include/private/android_logger.h` at android-16.0.0_r1:
//! https://android.googlesource.com/platform/system/logging/+/android-16.0.0_r1/liblog/
//! The guest agent adds liblog's logd header and tag and writes
//! LOG_ID_EVENTS as the agent's system uid.

use super::AppHost;
use aim_binder_host::parcel::Exception;

/// `IAppHostAgent.writeEvent(int tag, byte[] payload)`.
pub(super) const WRITE_EVENT: u32 = 17;
/// `LOGGER_ENTRY_MAX_PAYLOAD`, less `android_event_header_t`'s tag.
const MAX_PAYLOAD: usize = 4068 - 4;

/// An EventLog value. Lists carry a one-byte element count; strings are UTF-8.
pub enum Value<'a> {
    Int(i32),
    Long(i64),
    String(&'a str),
    List(&'a [Value<'a>]),
}

impl Value<'_> {
    fn encode(&self, out: &mut Vec<u8>, depth: usize) -> Result<(), Exception> {
        let invalid = || Exception::illegal_argument("EventLog payload exceeds liblog limits");
        match self {
            Self::Int(value) => {
                out.push(0);
                out.extend(value.to_le_bytes());
            }
            Self::Long(value) => {
                out.push(1);
                out.extend(value.to_le_bytes());
            }
            Self::String(value) => {
                if value.len() > MAX_PAYLOAD {
                    return Err(invalid());
                }
                out.push(2);
                out.extend((value.len() as i32).to_le_bytes());
                out.extend(value.as_bytes());
            }
            Self::List(values) => {
                // `ANDROID_MAX_LIST_NEST_DEPTH` and the uint8_t count.
                if depth >= 8 || values.len() > u8::MAX as usize {
                    return Err(invalid());
                }
                out.extend([3, values.len() as u8]);
                for value in *values {
                    value.encode(out, depth + 1)?;
                }
            }
        }
        if out.len() > MAX_PAYLOAD {
            return Err(invalid());
        }
        Ok(())
    }
}

impl AppHost {
    /// Write as system_server would: the guest agent's uid is system, even
    /// when this service is handling a Binder call from an app. Reject an
    /// oversized payload rather than letting logd receive a truncated value.
    pub fn write_event(&self, tag: i32, payload: Value<'_>) -> Result<(), Exception> {
        let mut bytes = Vec::new();
        payload.encode(&mut bytes, 0)?;
        self.call_agent(WRITE_EVENT, "writeEvent", |data| {
            data.write_i32(tag);
            aim_service_aidl::write_byte_array(data, Some(&bytes));
        })
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_driver::{Credentials, Device, Driver, File, GuestProcess, errno, uapi::*};
    use aim_binder_host::local::{Call, LocalProcess, Reply, Service};
    use aim_binder_host::parcel::{Binder, EX_ILLEGAL_STATE, Parcel};
    use std::sync::{Arc, mpsc};

    struct Agent(mpsc::Sender<(i32, Vec<u8>)>);
    impl Service for Agent {
        fn descriptor(&self) -> &str {
            super::super::AGENT_DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            assert_eq!(call.code, WRITE_EVENT);
            assert_eq!(call.sender_euid, 1000);
            assert!(!call.is_oneway());
            call.data.enforce_interface(self.descriptor())?;
            let tag = call.data.read_i32()?;
            let payload = aim_service_aidl::read_byte_array(&mut call.data)?.unwrap();
            assert_eq!(call.data.remaining(), 0);
            self.0.send((tag, payload)).unwrap();
            super::super::reply(if tag == -1 {
                Err(Exception::new(EX_ILLEGAL_STATE, "logd unavailable"))
            } else {
                let mut reply = Parcel::new();
                reply.write_no_exception();
                Ok(reply)
            })
        }
    }

    struct NoMemory;
    impl GuestProcess for NoMemory {
        fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), i32> {
            Err(errno::EFAULT)
        }
        fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), i32> {
            Err(errno::EFAULT)
        }
        fn get_file(&mut self, _: u32) -> Result<File, i32> {
            Err(errno::EBADF)
        }
        fn install_file(&mut self, _: File) -> Result<u32, i32> {
            Err(errno::EBADF)
        }
        fn close_fd(&mut self, _: u32) {
            panic!("no fixture files")
        }
    }

    #[test]
    fn host_and_package_leaves_send_events_to_the_agent() {
        let driver = Driver::new();
        let open = |pid| {
            LocalProcess::open(
                &driver,
                Device::Binder,
                Credentials {
                    pid,
                    euid: 1000,
                    security_context: None,
                },
            )
        };
        let remote = open(97501);
        let host = AppHost::new(open(97502));
        assert_eq!(
            host.write_event(42, Value::Int(7)).unwrap_err().code,
            EX_ILLEGAL_STATE
        );
        let (send, receive) = mpsc::channel();
        let Binder::Local(ptr) = remote.add_service(Arc::new(Agent(send))) else {
            unreachable!()
        };
        let mut object = FlatBinderObject {
            kind: BINDER_TYPE_BINDER,
            flags: 0,
            binder: ptr,
            cookie: ptr,
        }
        .encode();
        driver
            .ioctl(
                remote.proc_handle(),
                97501,
                BINDER_SET_CONTEXT_MGR_EXT,
                &mut object,
                &mut NoMemory,
            )
            .unwrap();
        remote.start();
        *host.agent.lock().unwrap() = Some(host.process.strong(0));
        host.write_event(42, Value::Long(-2)).unwrap();
        assert_eq!(
            receive.recv().unwrap(),
            (42, b"\x01\xfe\xff\xff\xff\xff\xff\xff\xff".to_vec())
        );

        use aim_service_aidl::{
            dev_aim_server_ipackagemutationbridge as effects,
            dev_aim_server_ipackagepolicybridge as policy,
        };
        let mut request = Parcel::new();
        policy::ReportAdminPermissionDenied {}.write(&mut request);
        let result = super::super::package_policy::PackagePolicy(host.clone())
            .transact(&mut Call {
                code: policy::REPORT_ADMIN_PERMISSION_DENIED,
                flags: 0,
                sender_pid: 77,
                sender_euid: 1000,
                data: request.reader(),
            })
            .unwrap();
        result.reader().read_exception().unwrap().unwrap();
        let (tag, payload) = receive.recv().unwrap();
        assert_eq!(tag, 0x534e4554);
        assert_eq!(
            payload,
            b"\x03\x03\x02\x09\x00\x00\x00128599183\x00\xff\xff\xff\xff\x02\x00\x00\x00\x00"
        );

        // The event precedes package/compat lookup, even if that owner is absent.
        let mut request = Parcel::new();
        effects::RequireInstallerPermission { calling_uid: 10123 }.write(&mut request);
        let result = super::super::package_effects::PackageEffects::new(host.clone())
            .transact(&mut Call {
                code: effects::REQUIRE_INSTALLER_PERMISSION,
                flags: 0,
                sender_pid: 77,
                sender_euid: 1000,
                data: request.reader(),
            })
            .unwrap();
        assert!(result.reader().read_exception().unwrap().is_err());
        assert_eq!(
            receive.recv().unwrap(),
            (
                0x534e4554,
                b"\x03\x03\x02\x09\x00\x00\x00150857253\x00\x8b\x27\x00\x00\x02\x00\x00\x00\x00"
                    .to_vec()
            )
        );
        let failure = host.write_event(-1, Value::Int(0)).unwrap_err();
        assert_eq!(failure.code, EX_ILLEGAL_STATE);
        assert_eq!(failure.message, "logd unavailable");
        assert_eq!(receive.recv().unwrap().0, -1);
        *host.agent.lock().unwrap() = None;
        host.process.shutdown().unwrap();
        remote.shutdown().unwrap();
    }

    #[test]
    fn safetynet_matches_liblog_binary_layout() {
        let mut bytes = 0x534e4554i32.to_le_bytes().to_vec();
        Value::List(&[
            Value::String("128599183"),
            Value::Int(-1),
            Value::String(""),
        ])
        .encode(&mut bytes, 0)
        .unwrap();
        assert_eq!(bytes, b"\x54\x45\x4e\x53\x03\x03\x02\x09\x00\x00\x00128599183\x00\xff\xff\xff\xff\x02\x00\x00\x00\x00");
    }

    #[test]
    fn scalars_nested_lists_and_limits() {
        let mut bytes = Vec::new();
        Value::List(&[Value::Long(-2), Value::List(&[Value::String("한")])])
            .encode(&mut bytes, 0)
            .unwrap();
        assert_eq!(
            bytes,
            b"\x03\x02\x01\xfe\xff\xff\xff\xff\xff\xff\xff\x03\x01\x02\x03\x00\x00\x00\xed\x95\x9c"
        );
        assert!(
            Value::String(&"x".repeat(MAX_PAYLOAD - 5))
                .encode(&mut Vec::new(), 0)
                .is_ok()
        );
        assert!(
            Value::String(&"x".repeat(MAX_PAYLOAD - 4))
                .encode(&mut Vec::new(), 0)
                .is_err()
        );
        let many: Vec<_> = (0..256).map(Value::Int).collect();
        assert!(Value::List(&many).encode(&mut Vec::new(), 0).is_err());
        assert!(Value::List(&[]).encode(&mut Vec::new(), 8).is_err());
    }
}
