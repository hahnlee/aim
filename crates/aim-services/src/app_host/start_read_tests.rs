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

/// Pinned WMS, PMS, AudioService and StorageManagerService cold reads.
#[test]
fn owned_boot_reads_keep_permissions_and_results() {
    use aim_service_aidl::{
        android_os_ipowermanager as pm, android_os_storage_istoragemanager as sm,
        android_view_iwindowmanager as wm,
    };
    crate::system::with_discovery_binders(|host, _, _, export| {
        permissions(host);
        let window = window_manager::WindowManager(host.clone());
        let power = power_service::PowerManager::new(host.clone());
        let audio = audio_service::AudioService::new(host.clone());
        let storage = storage::StorageManager(host.clone());
        let denied = |p: Parcel, message: &str| {
            let e = p.reader().read_exception().unwrap().unwrap_err();
            assert_eq!(e.code, EX_SECURITY);
            assert_eq!(e.message, message);
        };
        let mut data = Parcel::new();
        wm::IsKeyguardSecure { user_id: 1 }.write(&mut data);
        denied(
            call(&window, wm::IS_KEYGUARD_SECURE, &data, 10100),
            "Requires INTERACT_ACROSS_USERS permission",
        );
        let mut data = Parcel::new();
        pm::IsAmbientDisplayAvailable {}.write(&mut data);
        denied(
            call(&power, pm::IS_AMBIENT_DISPLAY_AVAILABLE, &data, 10100),
            "Neither user 10100 nor current process has android.permission.READ_DREAM_STATE.",
        );
        let mut data = Parcel::new();
        sm::GetCacheQuotaBytes {
            volume_uuid: None,
            uid: 10101,
        }
        .write(&mut data);
        denied(
            call(&storage, sm::GET_CACHE_QUOTA_BYTES, &data, 10100),
            "StorageManagerService: Neither user 10100 nor current process has android.permission.STORAGE_INTERNAL.",
        );
        assert!(host.storage_stats.get().is_none());
        for uid in [10100, 1000] {
            let mut data = Parcel::new();
            audio::GetSpatializerImmersiveAudioLevel {}.write(&mut data);
            assert_eq!(
                audio::read_get_spatializer_immersive_audio_level_reply(
                    &mut call(
                        &audio,
                        audio::GET_SPATIALIZER_IMMERSIVE_AUDIO_LEVEL,
                        &data,
                        uid
                    )
                    .reader()
                )
                .unwrap()
                .unwrap(),
                0
            );
            let mut data = Parcel::new();
            wm::IsKeyguardSecure { user_id: 0 }.write(&mut data);
            let secure = wm::read_is_keyguard_secure_reply(
                &mut call(&window, wm::IS_KEYGUARD_SECURE, &data, uid).reader(),
            )
            .unwrap();
            match aim_host_lock::secure() {
                Ok(expected) => assert_eq!(secure.unwrap(), expected),
                Err(code) => {
                    let error = secure.unwrap_err();
                    assert_eq!(error.code, aim_binder_host::parcel::EX_ILLEGAL_STATE);
                    assert_eq!(
                        error.message,
                        format!("lock_settings: the Mac's lock: LocalAuthentication error {code}")
                    );
                    eprintln!(
                        "not run: successful Mac lock read; LocalAuthentication error {code}; propagation checked"
                    );
                }
            }
        }
        // Both Objects.requireNonNull checks precede process/display lookup. No permission guard.
        for (thread, token) in [(None, None), (Some(Binder::Local(1)), None)] {
            let mut data = Parcel::new();
            wm::AttachWindowContextToDisplayContent {
                app_thread: thread,
                client_token: token,
                display_id: 4,
            }
            .write(&mut data);
            assert_eq!(
                call(
                    &window,
                    wm::ATTACH_WINDOW_CONTEXT_TO_DISPLAY_CONTENT,
                    &data,
                    10100
                )
                .reader()
                .read_exception()
                .unwrap()
                .unwrap_err()
                .code,
                EX_NULL_POINTER
            );
        }
        let original = aim_paths::original_image();
        if !original
            .join("system/framework/framework-res.apk")
            .is_file()
        {
            eprintln!("not run: original framework resources for boot-read results");
            return;
        }
        assert!(
            host.framework
                .set(resources::Framework::load(&original, 160, "en-US").unwrap())
                .is_ok()
        );
        host.display_metrics
            .set(Ok(display::Metrics {
                width: 1080,
                height: 1920,
                density_dpi: 160,
            }))
            .unwrap();
        let mut data = Parcel::new();
        pm::IsAmbientDisplayAvailable {}.write(&mut data);
        let expected = !host
            .framework()
            .unwrap()
            .framework_string("config_dozeComponent")
            .unwrap()
            .is_empty();
        assert_eq!(
            pm::read_is_ambient_display_available_reply(
                &mut call(&power, pm::IS_AMBIENT_DISPLAY_AVAILABLE, &data, 1000).reader()
            )
            .unwrap()
            .unwrap(),
            expected
        );
        for uid in [10100, 1000] {
            let mut data = Parcel::new();
            audio::GetRingerModeInternal {}.write(&mut data);
            assert_eq!(
                audio::read_get_ringer_mode_internal_reply(
                    &mut call(&audio, audio::GET_RINGER_MODE_INTERNAL, &data, uid).reader()
                )
                .unwrap()
                .unwrap(),
                2
            );
        }
        let thread = export(Arc::new(activity::Token));
        let client = export(Arc::new(activity::Token));
        let mut app = tests::host_with_start(7, Arc::new(|| {}))
            .apps
            .lock()
            .unwrap()
            .remove(&7)
            .unwrap();
        app.thread = Some(host.process.strong(thread.handle));
        host.apps.lock().unwrap().insert(900, app);
        let request = |display_id| {
            let mut data = Parcel::new();
            wm::AttachWindowContextToDisplayContent {
                app_thread: Some(thread.binder()),
                client_token: Some(client.binder()),
                display_id,
            }
            .write(&mut data);
            call(
                &window,
                wm::ATTACH_WINDOW_CONTEXT_TO_DISPLAY_CONTENT,
                &data,
                10100,
            )
        };
        let e = request(4).reader().read_exception().unwrap().unwrap_err();
        assert_eq!(
            e.message,
            "attachWindowContextToDisplayContent: trying to attach to a non-existing display:4"
        );
        let result = request(0);
        let mut expected = Parcel::new();
        expected.write_no_exception();
        expected.write_i32(1); // WindowContextInfo
        expected.write_i32(1); // Configuration
        host.global_configuration().unwrap().write_to(&mut expected);
        expected.write_i32(0);
        assert_eq!(result.data(), expected.data());
        assert_eq!(host.window_contexts.lock().unwrap().len(), 1);
        // This package fixture deliberately has no installed android/UID 1000.
        // Mount must preserve the stats owner's failure after clearing identity.
        let mut data = Parcel::new();
        sm::GetCacheQuotaBytes {
            volume_uuid: None,
            uid: 10100,
        }
        .write(&mut data);
        let error = sm::read_get_cache_quota_bytes_reply(
            &mut call(&storage, sm::GET_CACHE_QUOTA_BYTES, &data, 10100).reader(),
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(
            error.message,
            "android from uid 1000 not allowed to perform GET_USAGE_STATS"
        );
        assert_eq!(
            error.message,
            host.storage_stats_service()
                .cache_quota_bytes(1000, None, 10100, Some("android"))
                .unwrap_err()
                .message
        );
    });
}

/// AudioService.getDevicesForAttributes: permission precedes null, the owner
/// sees arbitrary attributes and the caller, and its typed list/errors survive.
#[test]
fn audio_devices_for_attributes_preserve_routing_and_caller() {
    struct Routes;
    impl Service for Routes {
        fn descriptor(&self) -> &str {
            aq::DESCRIPTOR
        }
        fn transact(&self, c: &mut Call<'_>) -> Reply {
            assert_eq!(c.sender_euid, 1000);
            let a = aq::GetDevicesForAttributes::<playback::Attributes>::read(&mut c.data)?;
            let attributes = a.attributes.unwrap();
            assert!(matches!(a.calling_uid, 1000 | 10100));
            let mut p = Parcel::new();
            match attributes.usage {
                99 => p.write_exception(&Exception::illegal_argument("routing failure")),
                98 => return Err(UNKNOWN_TRANSACTION),
                _ => {
                    assert_eq!(attributes.usage, 4);
                    let mut devices = Parcel::new();
                    devices.write_i32(2);
                    for (kind, address) in [(2, "speaker"), (8, "AA:BB:CC:DD:EE:FF")] {
                        devices.write_i32(1);
                        devices.write_i32(2);
                        devices.write_i32(kind);
                        devices.write_string16(Some(address));
                        devices.write_string16(Some("route"));
                        devices.write_i32(if kind == 2 { 2 } else { 128 });
                        devices.write_i32(0);
                        devices.write_i32(0);
                    }
                    aq::write_get_devices_for_attributes_reply(
                        &mut p,
                        &Some(devices.data().to_vec()),
                    );
                }
            }
            Ok(p)
        }
    }
    for permission in ["MODIFY_AUDIO_ROUTING", "QUERY_AUDIO_STATE"] {
        crate::system::with_discovery_binders(|host, _, _, export| {
            let name = format!("android.permission.{permission}");
            let system = format!(
                "<access><permissions><permission name='{name}' packageName='android' protectionLevel='2' type='0'/></permissions></access>"
            );
            let system = crate::package::permissions::AccessSystem::parse(
                &aim_android_xml::read(system.as_bytes()).unwrap(),
            )
            .unwrap();
            let path = host
                .data()
                .unwrap()
                .join("misc_de/0/apexdata/com.android.permission/access.abx");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("<access><app-id-permissions><app-id id='10100'><permission name='{name}' flags='4'/></app-id></app-id-permissions></access>")).unwrap();
            host.permission_manager
                .set(Arc::new(
                    permission_manager::PermissionManager::with_system(host.clone(), system),
                ))
                .ok()
                .unwrap();
            let service = audio_service::AudioService::new(host.clone());
            attach(host, export(Arc::new(Routes)).binder(), true);
            for (uid, usage, code) in [
                (10101, None, EX_SECURITY),
                (10100, None, EX_NULL_POINTER),
                (10100, Some(4), 0),
                (1000, None, EX_NULL_POINTER),
                (1000, Some(99), aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT),
                (1000, Some(98), aim_binder_host::parcel::EX_ILLEGAL_STATE),
                (1000, Some(4), 0),
            ] {
                let mut data = Parcel::new();
                audio::GetDevicesForAttributes {
                    attributes: usage.map(|u| playback::Attributes::built(u, 1, 0x10)),
                }
                .write(&mut data);
                let result = call(&service, audio::GET_DEVICES_FOR_ATTRIBUTES, &data, uid);
                if code != 0 {
                    let e = result.reader().read_exception().unwrap().unwrap_err();
                    assert_eq!(e.code, code);
                    if code == EX_SECURITY {
                        assert_eq!(
                            e.message,
                            "Missing MODIFY_AUDIO_ROUTING or QUERY_AUDIO_STATE permissions"
                        );
                    }
                } else {
                    let devices = audio::read_get_devices_for_attributes_reply::<
                        volume::DeviceAttributes,
                    >(&mut result.reader())
                    .unwrap()
                    .unwrap()
                    .unwrap();
                    assert_eq!(devices.len(), 2);
                    if !devices.is_empty() {
                        assert_eq!(devices[1].as_ref().unwrap().internal_type, 128);
                    }
                }
            }
        });
    }
}
