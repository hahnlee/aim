//! Android 16 start-time reads: real native permission checks, retained Java boundaries.
use super::*;
use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{EX_NULL_POINTER, EX_SECURITY, UNKNOWN_TRANSACTION};
use aim_service_aidl::{
    android_content_icontentservice as cs, android_media_iaudioservice as audio,
    dev_aim_server_iaudiopolicyqueries as aq, dev_aim_server_isyncadapterqueries as sq,
    dev_aim_server_isystemprocess as sp,
};

fn call(service: &impl Service, code: u32, data: &Parcel, uid: u32) -> Parcel {
    service
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 71,
            sender_euid: uid,
            data: data.reader(),
        })
        .unwrap()
}

struct Original {
    audio: bool,
    payload: Vec<u8>,
    calls: Arc<Mutex<Vec<(i32, i32)>>>,
}
impl Service for Original {
    fn descriptor(&self) -> &str {
        if self.audio {
            aq::DESCRIPTOR
        } else {
            sq::DESCRIPTOR
        }
    }
    fn transact(&self, c: &mut Call<'_>) -> Reply {
        assert_eq!(c.sender_euid, 1000);
        let mut p = Parcel::new();
        if self.audio {
            assert_eq!(c.code, aq::GET_AUDIO_PRODUCT_STRATEGIES);
            aq::GetAudioProductStrategies::read(&mut c.data)?;
            self.calls.lock().unwrap().push((1000, 0));
            aq::write_get_audio_product_strategies_reply(&mut p, &Some(self.payload.clone()));
        } else {
            assert_eq!(c.code, sq::GET_SYNC_ADAPTER_TYPES);
            let a = sq::GetSyncAdapterTypes::read(&mut c.data)?;
            self.calls.lock().unwrap().push((a.calling_uid, a.user_id));
            match a.user_id {
                11 => p.write_exception(&Exception::illegal_argument("original failure")),
                12 => return Err(UNKNOWN_TRANSACTION),
                _ => sq::write_get_sync_adapter_types_reply(&mut p, &Some(self.payload.clone())),
            }
        }
        Ok(p)
    }
}

fn attach(host: &Arc<AppHost>, binder: Binder, audio: bool) {
    let system = system_process::SystemProcess::new(host.clone());
    let mut data = Parcel::new();
    let code = if audio {
        sp::AttachAudioPolicyQueries {
            queries: Some(binder),
        }
        .write(&mut data);
        sp::ATTACH_AUDIO_POLICY_QUERIES
    } else {
        sp::AttachSyncAdapterQueries {
            queries: Some(binder),
        }
        .write(&mut data);
        sp::ATTACH_SYNC_ADAPTER_QUERIES
    };
    assert_eq!(
        call(&system, code, &data, 10100)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap_err()
            .code,
        EX_SECURITY
    );
    call(&system, code, &data, 1000)
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
}

fn payload(reply: &Parcel) -> &[u8] {
    let mut r = reply.reader();
    r.read_exception().unwrap().unwrap();
    &reply.data()[r.position()..]
}

fn permissions(host: &Arc<AppHost>) {
    let manager =
        permission_manager::PermissionManager::with_system(host.clone(), Default::default());
    assert!(host.permission_manager.set(Arc::new(manager)).is_ok());
}

#[test]
fn spatializer_device_read_checks_effect_permission_then_null_and_reports_no_modes() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        permissions(host);
        let service = audio_service::AudioService::new(host.clone());
        for present in [false, true] {
            for role in [1, 2] {
                let mut data = Parcel::new();
                data.write_interface_token(audio::DESCRIPTOR);
                data.write_i32(present as i32);
                if present {
                    data.write_i32(role);
                    data.write_i32(2); // built-in speaker
                    data.write_string16(Some(""));
                    data.write_string16(Some("speaker"));
                    data.write_i32(2);
                    data.write_i32(1); // one AudioProfile
                    data.write_string16(Some("android.media.AudioProfile"));
                    data.write_i32(1); // format
                    for array in [&[48000][..], &[3][..], &[][..]] {
                        data.write_i32(array.len() as i32);
                        for v in array {
                            data.write_i32(*v);
                        }
                    }
                    data.write_i32(0); // encapsulation
                    data.write_i32(1); // one AudioDescriptor
                    data.write_string16(Some("android.media.AudioDescriptor"));
                    data.write_i32(1);
                    data.write_i32(0);
                    data.write_i32(3);
                    data.write_raw(&[1, 2, 3, 0], &[]);
                }
                let response = call(
                    &service,
                    audio::IS_SPATIALIZER_AVAILABLE_FOR_DEVICE,
                    &data,
                    10100,
                );
                assert_eq!(
                    response
                        .reader()
                        .read_exception()
                        .unwrap()
                        .unwrap_err()
                        .code,
                    EX_SECURITY
                );
                let response = call(
                    &service,
                    audio::IS_SPATIALIZER_AVAILABLE_FOR_DEVICE,
                    &data,
                    1000,
                );
                if present {
                    assert!(
                        !audio::read_is_spatializer_available_for_device_reply(
                            &mut response.reader()
                        )
                        .unwrap()
                        .unwrap()
                    );
                } else {
                    assert_eq!(
                        response
                            .reader()
                            .read_exception()
                            .unwrap()
                            .unwrap_err()
                            .code,
                        EX_NULL_POINTER
                    );
                }
            }
        }
    });
}

#[test]
fn audio_product_strategies_require_routing_permission_and_keep_original_nonempty_reply() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        permissions(host);
        let service = audio_service::AudioService::new(host.clone());
        let mut data = Parcel::new();
        audio::GetAudioProductStrategies {}.write(&mut data);
        assert!(
            call(&service, audio::GET_AUDIO_PRODUCT_STRATEGIES, &data, 1000)
                .reader()
                .read_exception()
                .unwrap()
                .is_err()
        );
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut strategies = Parcel::new();
        strategies.write_i32(1); // typed list
        strategies.write_i32(1);
        strategies.write_string16(Some("STRATEGY_MEDIA"));
        strategies.write_i32(3);
        strategies.write_i32(0); // attributes groups
        let original = export(Arc::new(Original {
            audio: true,
            payload: strategies.data().to_vec(),
            calls: calls.clone(),
        }));
        attach(host, original.binder(), true);
        let denied = call(&service, audio::GET_AUDIO_PRODUCT_STRATEGIES, &data, 10100);
        assert_eq!(
            denied.reader().read_exception().unwrap().unwrap_err().code,
            EX_SECURITY
        );
        assert!(calls.lock().unwrap().is_empty());
        assert_eq!(
            payload(&call(
                &service,
                audio::GET_AUDIO_PRODUCT_STRATEGIES,
                &data,
                1000
            )),
            strategies.data()
        );
        assert_eq!(*calls.lock().unwrap(), [(1000, 0)]);
    });
}

#[test]
fn sync_adapter_types_allow_same_user_without_read_sync_settings_and_require_full_cross_user() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        permissions(host);
        let service = content_service::ContentService::new(host.clone());
        let calls = Arc::new(Mutex::new(Vec::new()));
        let mut types = Parcel::new();
        types.write_i32(1);
        types.write_i32(1);
        types.write_string16(Some("contacts"));
        types.write_string16(Some("fixture.account"));
        for b in [1, 1, 0, 0] {
            types.write_i32(b);
        }
        types.write_string16(None);
        types.write_string16(Some("p"));
        let original = export(Arc::new(Original {
            audio: false,
            payload: types.data().to_vec(),
            calls: calls.clone(),
        }));
        attach(host, original.binder(), false);
        let system = system_process::SystemProcess::new(host.clone());
        for (name, hidden) in [("p", false), ("missing", true)] {
            let mut data = Parcel::new();
            sp::FilterSyncAdapterPackageAccess {
                package_name: Some(name.into()),
                uid: 10100,
                user_id: 0,
            }
            .write(&mut data);
            assert_eq!(
                call(
                    &system,
                    sp::FILTER_SYNC_ADAPTER_PACKAGE_ACCESS,
                    &data,
                    10100
                )
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
                EX_SECURITY
            );
            assert_eq!(
                sp::read_filter_sync_adapter_package_access_reply(
                    &mut call(&system, sp::FILTER_SYNC_ADAPTER_PACKAGE_ACCESS, &data, 1000)
                        .reader()
                )
                .unwrap()
                .unwrap(),
                hidden
            );
        }
        for (uid, user) in [(10100, 0), (10100, 10), (1000, 10)] {
            let mut data = Parcel::new();
            cs::GetSyncAdapterTypesAsUser { user_id: user }.write(&mut data);
            let response = call(&service, cs::GET_SYNC_ADAPTER_TYPES_AS_USER, &data, uid);
            if uid == 10100 && user == 10 {
                assert_eq!(
                    response
                        .reader()
                        .read_exception()
                        .unwrap()
                        .unwrap_err()
                        .code,
                    EX_SECURITY
                );
            } else {
                assert_eq!(payload(&response), types.data());
            }
        }
        assert_eq!(*calls.lock().unwrap(), [(10100, 0), (1000, 10)]);
        let mut data = Parcel::new();
        cs::GetSyncAdapterTypes {}.write(&mut data);
        assert_eq!(
            payload(&call(&service, cs::GET_SYNC_ADAPTER_TYPES, &data, 10100)),
            types.data()
        );
        for user in [11, 12] {
            let mut data = Parcel::new();
            cs::GetSyncAdapterTypesAsUser { user_id: user }.write(&mut data);
            let response = call(&service, cs::GET_SYNC_ADAPTER_TYPES_AS_USER, &data, 1000);
            assert_eq!(
                response
                    .reader()
                    .read_exception()
                    .unwrap()
                    .unwrap_err()
                    .code,
                if user == 11 {
                    aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT
                } else {
                    EX_ILLEGAL_STATE
                }
            );
        }
    });
}
