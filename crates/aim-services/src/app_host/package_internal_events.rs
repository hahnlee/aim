//! `PackageInternalEventsBridge` (`android-16.0.0_r1`): the package
//! broadcast owner's Intents and callback Bundles. Delivery to the
//! activity manager's receivers and native service receivers is #2167.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::Arc;

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_NULL_POINTER, Exception, Parcel, UNKNOWN_TRANSACTION};
use aim_service_aidl::dev_aim_server_ipackageinternaleventsbridge as events;

use super::kept::RawIntent;
use super::{AppHost, package_writes, reply};

pub struct InternalEvents {
    host: Arc<AppHost>,
}

impl InternalEvents {
    pub fn new(host: Arc<AppHost>) -> Self {
        InternalEvents { host }
    }
}

/// `record`: asynchronous delivery, the Intent, then its callback Bundle.
fn record(intent: &RawIntent, asynchronous: bool) -> Vec<u8> {
    let mut p = Parcel::new();
    p.write_bool(asynchronous);
    aim_service_aidl::write_byte_array(&mut p, Some(intent.parcel().data()));
    aim_service_aidl::write_byte_array(&mut p, Some(&package_writes::callback_bundle(intent)));
    p.data().to_vec()
}

impl Service for InternalEvents {
    fn descriptor(&self) -> &str {
        events::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.sender_euid != 1000 {
            return reply(Err(Exception::security(
                "native internal package event owner required",
            )));
        }
        let mut p = Parcel::new();
        let result = match call.code {
            events::PREPARE_RESTARTED => {
                let a = events::PrepareRestarted::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                a.package_name
                    .ok_or_else(|| Exception::new(EX_NULL_POINTER, "ssp"))
                    .and_then(|name| {
                        let stay_stopped = self
                            .host
                            .framework()?
                            .0
                            .flags
                            .get("android.content.pm.stay_stopped")
                            == Some(&true);
                        let intent =
                            package_writes::restarted_intent(&name, a.uid, a.flags, stay_stopped);
                        events::write_prepare_restarted_reply(
                            &mut p,
                            &Some(record(&intent, stay_stopped)),
                        );
                        Ok(())
                    })
            }
            events::PREPARE_DATA_CLEARED => {
                let a = events::PrepareDataCleared::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                a.package_name
                    .ok_or_else(|| Exception::new(EX_NULL_POINTER, "ssp"))
                    .map(|name| {
                        let intent = package_writes::data_cleared_intent(
                            &name, a.uid, a.user_id, a.restore, a.instant,
                        );
                        events::write_prepare_data_cleared_reply(
                            &mut p,
                            &Some(record(&intent, false)),
                        );
                    })
            }
            events::BROADCAST => {
                events::Broadcast::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                Err(self.host.unimplemented_named(
                    "package internal events",
                    "broadcast to registered and native service receivers (#2167)",
                ))
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        };
        reply(result.map(|()| p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::{self, Value};
    use crate::package::{intent_filter::Plain, uri::Uri};
    use aim_binder_host::parcel::{Binder, EX_SECURITY, EX_UNSUPPORTED_OPERATION, Reader};

    fn callback_intent(bytes: &[u8]) -> Vec<u8> {
        let values = bundle::read(&mut Reader::new(bytes, &[])).unwrap().unwrap();
        let Value::Lazy {
            start,
            len,
            kind: 4,
        } = values["android.content.pm.extra.EXTRA_PACKAGE_MONITOR_CALLBACK_RESULT"]
        else {
            panic!("not an Intent")
        };
        let mut r = Reader::new(&bytes[start..start + len], &[]);
        assert_eq!(
            r.read_string16().unwrap().as_deref(),
            Some("android.content.Intent")
        );
        let at = r.position();
        use aim_service_aidl::ReadParcelable;
        RawIntent::read_from(&mut r).unwrap();
        assert_eq!(r.remaining(), 0);
        bytes[start + at..start + len].to_vec()
    }

    /// These generated Intents have no component, selector or clip. Read
    /// their original fields and extras independently of their producers.
    fn extras(
        bytes: &[u8],
        action: Option<&str>,
        data: Option<&str>,
        flags: i32,
    ) -> std::collections::HashMap<String, Value> {
        let mut r = Reader::new(bytes, &[]);
        assert_eq!(r.read_string8().unwrap().as_deref(), action);
        assert_eq!(
            Uri::read(&mut r, &mut Plain)
                .unwrap()
                .as_ref()
                .map(Uri::as_str),
            data
        );
        assert_eq!(r.read_string8().unwrap(), None);
        assert_eq!(r.read_string8().unwrap(), None);
        assert_eq!(r.read_i32().unwrap(), flags);
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.read_string8().unwrap(), None);
        assert_eq!(r.read_string16().unwrap(), None);
        for _ in 0..4 {
            assert_eq!(r.read_i32().unwrap(), 0);
        }
        assert_eq!(r.read_i32().unwrap(), -2);
        let values = bundle::read(&mut r).unwrap().unwrap();
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.read_i32().unwrap(), 0);
        assert_eq!(r.remaining(), 0);
        values
    }

    #[test]
    fn monitor_payload_translates_only_nonnegative_uids() {
        for (uid, user, expected) in [
            (10123, 10, 1010123),
            (-1, 10, -1),
            (1010123, 10, 1010123),
            (
                10123,
                i32::MAX,
                i32::MAX.wrapping_mul(100000).wrapping_add(10123),
            ),
        ] {
            let result = package_writes::monitor_result(
                Some("android.intent.action.PACKAGE_ADDED"),
                Some("a.b"),
                user,
                uid,
                true,
            );
            let intent = callback_intent(&result);
            let values = extras(
                &intent,
                Some("android.intent.action.PACKAGE_ADDED"),
                Some("package:a.b"),
                0,
            );
            assert_eq!(values.len(), 3);
            assert_eq!(values["android.intent.extra.UID"], Value::Int(expected));
            assert_eq!(values["android.intent.extra.user_handle"], Value::Int(user));
            assert_eq!(values["android.intent.extra.REPLACING"], Value::Bool(true));
        }
        let result = package_writes::monitor_result(None, None, 0, -1, false);
        extras(&callback_intent(&result), None, None, 0);
    }

    #[test]
    fn internal_event_records_match_the_java_bridge() {
        let Some(root) = aim_paths::original_image_with("system/framework/framework-res.apk")
        else {
            return;
        };
        let driver = aim_binder_driver::Driver::new();
        for stay_stopped in [false, true] {
            let process = aim_binder_host::local::LocalProcess::open(
                &driver,
                aim_binder_driver::Device::Binder,
                aim_binder_driver::Credentials {
                    pid: std::process::id() as i32,
                    euid: 1000,
                    security_context: None,
                },
            );
            let host = AppHost::new(process.clone());
            let mut framework =
                super::super::packages::Framework::load(&root, 320, "en-US").unwrap();
            framework
                .0
                .flags
                .insert("android.content.pm.stay_stopped".into(), stay_stopped);
            assert!(host.framework.set(framework).is_ok());
            let Binder::Local(ptr) =
                process.add_service(Arc::new(InternalEvents::new(host.clone())))
            else {
                unreachable!()
            };
            let node = process.local_service(ptr).unwrap();
            let mut request = Parcel::new();
            events::PrepareRestarted {
                package_name: Some("a.b".into()),
                uid: 1010123,
                flags: 0x20,
            }
            .write(&mut request);
            let reply = node
                .transact(events::PREPARE_RESTARTED, &request, false)
                .unwrap();
            let bytes = events::read_prepare_restarted_reply(&mut reply.reader())
                .unwrap()
                .unwrap()
                .unwrap();
            let (intent, asynchronous) = read_record(&bytes);
            assert_eq!(asynchronous, stay_stopped);
            let values = extras(
                &intent,
                Some("android.intent.action.PACKAGE_RESTARTED"),
                Some("package:a.b"),
                0x04000020,
            );
            assert_eq!(values["android.intent.extra.UID"], Value::Int(1010123));
            assert_eq!(values["android.intent.extra.user_handle"], Value::Int(10));
            assert_eq!(
                values.contains_key("android.intent.extra.TIME"),
                stay_stopped
            );
            if stay_stopped {
                assert!(
                    matches!(values["android.intent.extra.TIME"], Value::Long(t) if t >= 0 && t <= super::super::alarm_service::elapsed_ms())
                );
            }
            for restore in [false, true] {
                for instant in [false, true] {
                    let mut request = Parcel::new();
                    events::PrepareDataCleared {
                        package_name: Some("a.b".into()),
                        uid: -1,
                        user_id: 10,
                        restore,
                        instant,
                    }
                    .write(&mut request);
                    let reply = node
                        .transact(events::PREPARE_DATA_CLEARED, &request, false)
                        .unwrap();
                    let bytes = events::read_prepare_data_cleared_reply(&mut reply.reader())
                        .unwrap()
                        .unwrap()
                        .unwrap();
                    let (intent, asynchronous) = read_record(&bytes);
                    assert!(!asynchronous);
                    let values = extras(
                        &intent,
                        Some("android.intent.action.PACKAGE_DATA_CLEARED"),
                        Some("package:a.b"),
                        0x05000000,
                    );
                    assert_eq!(values["android.intent.extra.UID"], Value::Int(-1));
                    assert_eq!(values["android.intent.extra.user_handle"], Value::Int(10));
                    assert_eq!(
                        values.get("android.intent.extra.IS_RESTORE"),
                        restore.then_some(&Value::Bool(true))
                    );
                    assert_eq!(
                        values.contains_key("android.intent.extra.PACKAGE_NAME"),
                        instant
                    );
                }
            }
            for code in [events::PREPARE_RESTARTED, events::PREPARE_DATA_CLEARED] {
                let mut request = Parcel::new();
                if code == events::PREPARE_RESTARTED {
                    events::PrepareRestarted {
                        package_name: None,
                        uid: 10123,
                        flags: 0,
                    }
                    .write(&mut request);
                } else {
                    events::PrepareDataCleared {
                        package_name: None,
                        uid: 10123,
                        user_id: 0,
                        restore: false,
                        instant: false,
                    }
                    .write(&mut request);
                }
                let reply = node.transact(code, &request, false).unwrap();
                let error = reply.reader().read_exception().unwrap().unwrap_err();
                assert_eq!(error.code, EX_NULL_POINTER);
                assert_eq!(error.message, "ssp");
            }
            let mut request = Parcel::new();
            events::Broadcast {
                intent: Some(Vec::new()),
                user_id: 0,
                instant: false,
                allow_list: None,
            }
            .write(&mut request);
            let reply = node.transact(events::BROADCAST, &request, false).unwrap();
            let error = reply.reader().read_exception().unwrap().unwrap_err();
            assert_eq!(error.code, EX_UNSUPPORTED_OPERATION);
            assert!(error.message.contains("#2167"));
            let mut call = Call {
                code: events::BROADCAST,
                flags: 0,
                sender_pid: 42,
                sender_euid: 10123,
                data: request.reader(),
            };
            assert_eq!(
                InternalEvents::new(host)
                    .transact(&mut call)
                    .unwrap()
                    .reader()
                    .read_exception()
                    .unwrap()
                    .unwrap_err()
                    .code,
                EX_SECURITY
            );
        }
    }

    fn read_record(bytes: &[u8]) -> (Vec<u8>, bool) {
        let mut r = Reader::new(bytes, &[]);
        let asynchronous = r.read_bool().unwrap();
        let intent = aim_service_aidl::read_byte_array(&mut r).unwrap().unwrap();
        let callback = aim_service_aidl::read_byte_array(&mut r).unwrap().unwrap();
        assert_eq!(callback_intent(&callback), intent);
        assert_eq!(r.remaining(), 0);
        (intent, asynchronous)
    }
}
