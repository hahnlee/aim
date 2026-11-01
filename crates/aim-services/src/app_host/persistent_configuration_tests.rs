//! Pinned AMS permission/attribution and Configuration.updateFrom contracts.
use super::*;
use aim_binder_host::parcel::{EX_SECURITY, EX_UNSUPPORTED_OPERATION};
use aim_service_aidl::{ReadParcelable, com_android_internal_app_iappopsservice as ao};

fn call(s: &impl Service, uid: u32, code: u32, p: &Parcel) -> Parcel {
    s.transact(&mut Call {
        code,
        flags: 0,
        sender_pid: 71,
        sender_euid: uid,
        data: p.reader(),
    })
    .unwrap()
}

#[test]
fn persistent_configuration_decode_merge_permissions_attribution_and_persistence() {
    crate::system::with_discovery_binders(|host, publish, _, export| {
        struct Selection;
        impl Service for Selection {
            fn descriptor(&self) -> &str {
                aim_service_aidl::dev_aim_server_isettingsvalues::DESCRIPTOR
            }
            fn transact(&self, c: &mut Call<'_>) -> Reply {
                use aim_service_aidl::dev_aim_server_isettingsvalues as sv;
                assert_eq!(c.sender_euid, SYSTEM_UID);
                let a = sv::BestPersistentLocale::read(&mut c.data)?;
                assert_eq!(a.tags.as_deref(), Some("ar-EG,en-US"));
                let mut p = Parcel::new();
                sv::write_best_persistent_locale_reply(&mut p, &Some("ar-EG".into()));
                Ok(p)
            }
        }
        let selection = export(Arc::new(Selection));
        publish(&mut |p| {
            p.attributions.push(crate::package::pkg::Attribution {
                tag: Some("configuration-test".into()),
                label: 0,
                inherit_from: None,
            })
        });
        let xml = "<access><permissions><permission name='android.permission.CHANGE_CONFIGURATION' packageName='android' protectionLevel='2' type='0'/><permission name='android.permission.WRITE_SETTINGS' packageName='android' protectionLevel='2' type='0'/></permissions></access>";
        let system = crate::package::permissions::AccessSystem::parse(
            &aim_android_xml::read(xml.as_bytes()).unwrap(),
        )
        .unwrap();
        let path = host
            .data()
            .unwrap()
            .join("misc_de/0/apexdata/com.android.permission/access.abx");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "<access><app-id-permissions><app-id id='10100'><permission name='android.permission.CHANGE_CONFIGURATION' flags='4'/><permission name='android.permission.WRITE_SETTINGS' flags='4'/></app-id></app-id-permissions></access>").unwrap();
        let permissions = Arc::new(permission_manager::PermissionManager::with_system(
            host.clone(),
            system,
        ));
        host.permission_manager
            .set(permissions.clone())
            .ok()
            .unwrap();
        host.services();
        host.settings_provider()
            .unwrap()
            .attach_values(Some(selection.binder()))
            .unwrap();
        host.display_metrics
            .set(Ok(display::Metrics {
                width: 1080,
                height: 1920,
                density_dpi: 160,
            }))
            .unwrap();
        let service = Activity(host.clone());
        let old = host.global_configuration().unwrap();
        let mut c = config::Configuration::from_proto(&[]).unwrap();
        c.locales = "ar-EG,en-US".into();
        c.font_scale = 1.25;
        c.user_set_locale = true;
        c.window.bounds.right = 999; // ATMS clears the window part.
        let mut p = Parcel::new();
        c.write_to(&mut p);
        assert_eq!(
            config::Configuration::read_from(&mut p.reader()).unwrap(),
            c
        );
        for n in 0..p.data().len() {
            let mut r = aim_binder_host::parcel::Reader::new(&p.data()[..n], &[]);
            assert!(config::Configuration::read_from(&mut r).is_err());
        }
        let request = |package: Option<&str>| {
            let mut p = Parcel::new();
            am::UpdatePersistentConfigurationWithAttribution {
                values: Some(c.clone()),
                calling_package_name: package.map(Into::into),
                calling_attribution_tag: Some("configuration-test".into()),
            }
            .write(&mut p);
            p
        };
        for (uid, package, permission) in [
            (10101, Some("q"), "CHANGE_CONFIGURATION"),
            (10100, None, "WRITE_SETTINGS"),
        ] {
            let e = call(
                &service,
                uid,
                am::UPDATE_PERSISTENT_CONFIGURATION_WITH_ATTRIBUTION,
                &request(package),
            )
            .reader()
            .read_exception()
            .unwrap()
            .unwrap_err();
            assert_eq!(e.code, EX_SECURITY);
            assert!(e.message.contains(permission));
        }
        call(
            &service,
            10100,
            am::UPDATE_PERSISTENT_CONFIGURATION_WITH_ATTRIBUTION,
            &request(Some("p")),
        )
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
        let mut p = Parcel::new();
        am::GetConfiguration {}.write(&mut p);
        let got = am::read_get_configuration_reply::<config::Configuration>(
            &mut call(&service, 10100, am::GET_CONFIGURATION, &p).reader(),
        )
        .unwrap()
        .unwrap()
        .unwrap();
        assert_eq!(
            (&got.locales, got.font_scale, got.user_set_locale, got.seq),
            (&c.locales, 1.25, false, old.seq + 1)
        );
        assert!(host.global_configuration().unwrap().user_set_locale);
        assert_eq!(got.screen_layout & 0xc0, 0x80);
        assert_eq!(got.window, old.window);
        let inputs = host.platform().unwrap();
        assert_eq!(
            inputs
                .settings
                .get(settings::Table::System, "system_locales"),
            Some(c.locales.clone())
        );
        wait_persisted(host, "font_scale", "1.25");
        let ops = appops_service::AppOps::new(host.clone(), permissions);
        let mut p = Parcel::new();
        ao::GetOpsForPackage {
            uid: 10100,
            package_name: Some("p".into()),
            ops: Some(vec![23]),
        }
        .write(&mut p);
        let notes = call(&ops, 1000, ao::GET_OPS_FOR_PACKAGE, &p);
        notes.reader().read_exception().unwrap().unwrap();
        let tag: Vec<u8> = "configuration-test"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert!(notes.data().windows(tag.len()).any(|b| b == tag));
        let mut unsupported = c.clone();
        unsupported.density_dpi = old.density_dpi + 1;
        assert_eq!(
            got.merge_persistent(&unsupported).unwrap_err().code,
            EX_UNSUPPORTED_OPERATION
        );
        assert_eq!(
            got.merge_persistent(&config::Configuration::from_proto(&[]).unwrap())
                .unwrap(),
            got
        );
        host.apply_persistent_configuration(&c).unwrap();
        assert_eq!(host.global_configuration().unwrap().seq, got.seq); // idempotent
    });
}

#[test]
fn persistent_configuration_publishes_process_before_configuration_and_locale_broadcasts() {
    use std::sync::mpsc;
    use std::time::Duration;
    struct Thread(mpsc::Sender<String>);
    impl Service for Thread {
        fn descriptor(&self) -> &str {
            thread::DESCRIPTOR
        }
        fn transact(&self, c: &mut Call<'_>) -> Reply {
            let event = if c.code == thread::SCHEDULE_TRANSACTION {
                c.data.enforce_interface(thread::DESCRIPTOR)?;
                assert_eq!(c.data.read_i32()?, 1); // typed ClientTransaction
                assert_eq!(c.data.read_i32()?, 1); // one item
                c.data.read_string16()?.unwrap()
            } else {
                let a =
                    thread::ScheduleRegisteredReceiver::<kept::RawIntent, kept::RawBundle>::read(
                        &mut c.data,
                    )?;
                a.intent.unwrap().intent.action.unwrap()
            };
            self.0.send(event).unwrap();
            Ok(Parcel::new())
        }
    }
    crate::system::with_discovery_binders(|host, _, _, export| {
        let permissions =
            permission_manager::PermissionManager::with_system(host.clone(), Default::default());
        host.permission_manager
            .set(Arc::new(permissions))
            .ok()
            .unwrap();
        host.display_metrics
            .set(Ok(display::Metrics {
                width: 1080,
                height: 1920,
                density_dpi: 160,
            }))
            .unwrap();
        let (send, recv) = mpsc::channel();
        let thread = export(Arc::new(Thread(send)));
        let binder = thread.binder();
        let mut app = super::super::tests::host_with_start(7, Arc::new(|| {}))
            .apps
            .lock()
            .unwrap()
            .remove(&7)
            .unwrap();
        app.uid = 10100;
        app.package = "p".into();
        app.packages = vec!["p".into()];
        app.thread = Some(thread);
        app.config = Some(host.global_configuration().unwrap());
        host.apps.lock().unwrap().insert(7, app);
        let token = host.new_activity(
            7,
            10100,
            "p",
            host.activity_info_of("p", "p.Main").unwrap(),
            resources::WindowStyle::default(),
            1,
            activity_start::StartRequest::launcher("A"),
        );
        {
            let mut records = host.activities.records.lock().unwrap();
            let r = records.get_mut(&token).unwrap();
            r.config_changes = config::CONFIG_LOCALE | config::CONFIG_LAYOUT_DIRECTION;
            r.reported = Some(host.global_configuration().unwrap());
        }
        let mut filter = crate::package::intent_filter::IntentFilter::default();
        for a in ["CONFIGURATION_CHANGED", "LOCALE_CHANGED"] {
            filter.add_action(&format!("android.intent.action.{a}"));
        }
        let data = Parcel::new();
        host.broadcasts
            .register(
                host,
                &Call {
                    code: 0,
                    flags: 0,
                    sender_pid: 71,
                    sender_euid: 10100,
                    data: data.reader(),
                },
                broadcasts::Registration {
                    caller: Some(binder),
                    caller_package: Some("p".into()),
                    feature_id: None,
                    receiver_id: None,
                    receiver: Some(binder),
                    filter: Some({
                        let mut p = Parcel::new();
                        filter.write(&mut p);
                        broadcasts::ParcelFilter::read_from(&mut p.reader()).unwrap()
                    }),
                    permission: None,
                    user: 0,
                    flags: 2,
                },
            )
            .unwrap();
        host.apply_persistent_configuration(&config::Configuration {
            locales: "ja-JP".into(),
            user_set_locale: true,
            ..config::Configuration::from_proto(&[]).unwrap()
        })
        .unwrap();
        for expected in [
            "android.app.servertransaction.ConfigurationChangeItem",
            "android.app.servertransaction.ActivityConfigurationChangeItem",
            "android.intent.action.CONFIGURATION_CHANGED",
            "android.intent.action.LOCALE_CHANGED",
        ] {
            assert_eq!(recv.recv_timeout(Duration::from_secs(2)).unwrap(), expected);
        }
    });
}

#[test]
fn persistent_locale_property_is_written_only_for_user_locale_changes() {
    crate::system::with_discovery_binders(|source, _, _, _| {
        let host = AppHost::new(source.process.clone());
        let c = source.configured.get().unwrap();
        let writes = Arc::new(Mutex::new(Vec::new()));
        let record = writes.clone();
        host.configure(
            &c.root,
            &c.data,
            &c.bpf,
            &c.net,
            Box::new(|name| match name {
                "ro.build.version.sdk" => Some("36".into()),
                "ro.build.version.codename" => Some("REL".into()),
                "persist.sys.locale" => Some("en-US".into()),
                "ro.sf.lcd_density" => Some("160".into()),
                _ => None,
            }),
            Box::new(|_, _, _| Ok(true)),
            Box::new(move |k, v| record.lock().unwrap().push((k.to_string(), v.to_string()))),
            Box::new(|| Err(std::io::Error::other("no zygote"))),
        )
        .unwrap();
        host.display_metrics
            .set(Ok(display::Metrics {
                width: 1080,
                height: 1920,
                density_dpi: 160,
            }))
            .unwrap();
        let mut delta = config::Configuration::from_proto(&[]).unwrap();
        delta.locales = "ja-JP".into();
        delta.screen_layout = 0x40; // Configuration.setLocales on an otherwise unset delta.
        host.apply_persistent_configuration(&delta).unwrap();
        assert!(writes.lock().unwrap().is_empty());
        assert_eq!(
            host.global_configuration().unwrap().screen_layout & 0x1000_0000,
            0
        );
        delta.locales = "en-US".into();
        delta.user_set_locale = true;
        host.apply_persistent_configuration(&delta).unwrap();
        host.apply_persistent_configuration(&delta).unwrap();
        assert_eq!(
            *writes.lock().unwrap(),
            vec![("persist.sys.locale".into(), "en-US".into())]
        );
        wait_persisted(&host, "system_locales", "en-US");
        let loaded = settings::Settings::load(&c.data, &c.root).unwrap();
        assert_eq!(
            loaded
                .get(settings::Table::System, "system_locales")
                .as_deref(),
            Some("en-US")
        );
        delta.locales = "ko-KR".into();
        host.apply_configuration(&delta, false).unwrap();
        assert_eq!(
            writes.lock().unwrap().last(),
            Some(&("persist.sys.locale".into(), "ko-KR".into()))
        );
        assert_eq!(
            host.platform()
                .unwrap()
                .settings
                .get(settings::Table::System, "system_locales")
                .as_deref(),
            Some("en-US")
        );
    });
}

fn wait_persisted(host: &AppHost, name: &str, expected: &str) {
    let c = host.configured.get().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        if settings::Settings::load(&c.data, &c.root)
            .unwrap()
            .get(settings::Table::System, name)
            .as_deref()
            == Some(expected)
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "setting {name} was not persisted"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
