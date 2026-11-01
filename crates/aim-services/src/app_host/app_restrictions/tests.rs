use super::*;

#[test]
fn defaults_follow_bucket_appops_hibernation_and_flag_precedence() {
    for (b, r, h, api, expected) in [
        (5, true, true, false, 60),
        (5, true, false, false, 20),
        (50, false, false, false, 50),
        (50, false, false, true, 30),
        (45, false, false, false, 40),
        (45, true, false, true, 50),
        (10, false, false, false, 30),
        (20, false, false, true, 30),
        (30, false, false, false, 30),
        (40, false, false, false, 30),
    ] {
        assert_eq!(level(b, r, h, api), expected);
    }
}

fn settings() -> Settings {
    let mut s = Settings::default();
    for (uid, p, l) in [(10100, "a", 30), (10100, "b", 40), (10101, "c", 50)] {
        let mut r = Row::new(uid, p, 10, false);
        r.update(l, USER);
        s.rows.insert((uid, p.into()), r);
    }
    s
}

#[test]
fn restore_is_stricter_only_and_preserves_metadata_and_uid_aggregation() {
    let mut s = settings();
    s.restore(br#"<?xml version='1.0'?><ignored><settings uid="10100" package="a" curlevel="50" levelts="1234" reason="1027" last_batt_noti_ts="77" vendor="kept"/><settings uid="10100" package="b" curlevel="20" last_long_fgs_noti_ts="88"/><settings uid="10101" package="stale" curlevel="60"/><settings uid="99999" package="a" curlevel="60"/></ignored>"#).unwrap();
    assert_eq!(s.get(10100), 40);
    assert_eq!(s.get(10101), 50);
    assert_eq!(s.get(99999), 0);
    let a = &s.rows[&(10100, "a".into())];
    assert_eq!((a.last, a.int("curlevel"), a.int("reason")), (30, 50, USER));
    assert_eq!(a.xml.string("levelts").as_deref(), Some("1234"));
    assert_eq!(a.xml.string("vendor").as_deref(), Some("kept"));
    assert_eq!(a.xml.string("last_batt_noti_ts").as_deref(), Some("77"));
    assert_eq!(
        s.rows[&(10100, "b".into())]
            .xml
            .string("last_long_fgs_noti_ts")
            .as_deref(),
        Some("88")
    );
    assert_eq!(s.rows.len(), 3);
    let bytes = aim_android_xml::abx::write_sequence(
        &s.rows.values().map(|r| r.xml.clone()).collect::<Vec<_>>(),
    )
    .unwrap();
    let mut restored = settings();
    restored.restore(&bytes).unwrap();
    assert_eq!(restored.get(10100), 40);
    assert_eq!(restored.rows[&(10100, "a".into())].xml, a.xml);
    assert!(restored.restore(b"ABX\0\x3f").is_err());
}

fn fixture(f: impl FnOnce(&Arc<AppHost>)) {
    fixture_flags(false, f);
}
fn fixture_flags(api: bool, f: impl FnOnce(&Arc<AppHost>)) {
    let _image = aim_storage::system::ImageLease::read(&aim_paths::system_image()).unwrap();
    crate::system::with_discovery_binders_flags(
        &[
            (API, api),
            ("android.permission.flags.appop_mode_caching_enabled", true),
        ],
        true,
        |host, _, _, _| {
            let pm = super::super::permission_manager::PermissionManager::with_system(
                host.clone(),
                Default::default(),
            );
            assert!(host.permission_manager.set(Arc::new(pm)).is_ok());
            let framework = super::super::resources::Framework::load(
                &aim_paths::original_image(),
                160,
                "en-US",
            )
            .unwrap();
            assert!(host.framework.set(framework).is_ok());
            assert!(host.app_idle_enabled().unwrap());
            host.with_standby_checks_paused(|| f(host));
        },
    );
}
fn standby(host: &Arc<AppHost>, bucket: i32, reason: i32) {
    // Exercise the same post-lock notification as native usage/idle updates.
    host.set_restriction_test_bucket("p", bucket, reason)
        .unwrap();
    host.restrictions_changed(None).unwrap();
}
fn appop(host: &Arc<AppHost>, mode: i32) {
    let uid = host
        .installed(0x200, 0)
        .unwrap()
        .into_iter()
        .find(|(p, _, _)| p == "p")
        .unwrap()
        .1;
    host.permission_manager
        .get()
        .unwrap()
        .set_app_op_mode(uid, Some("p"), RUN_ANY, mode)
        .unwrap();
}
fn uid(host: &Arc<AppHost>) -> i32 {
    host.installed(0x200, 0)
        .unwrap()
        .into_iter()
        .find(|(p, _, _)| p == "p")
        .unwrap()
        .1
}

#[test]
fn native_owner_restores_once_then_tracks_appops_removal_and_standby() {
    fixture(|h| {
        let u = uid(h);
        standby(h, 10, DEFAULT);
        let path = h
            .data()
            .unwrap()
            .join("system_de/0/apprestriction/settings.xml");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("<settings uid=\"{u}\" package=\"p\" curlevel=\"50\" levelts=\"99\" reason=\"1027\" last_batt_noti_ts=\"7\"/>" )).unwrap();
        let other = h
            .data()
            .unwrap()
            .join("system_de/10/apprestriction/settings.xml");
        std::fs::create_dir_all(other.parent().unwrap()).unwrap();
        std::fs::write(&other, b"other user's state").unwrap();
        assert_eq!(h.restriction_level(u).unwrap(), 50);
        assert_eq!(h.restriction_level(99999).unwrap(), 0);
        assert!(
            std::fs::read(&path)
                .unwrap()
                .starts_with(aim_android_xml::abx::MAGIC)
        );
        assert_eq!(std::fs::read(&other).unwrap(), b"other user's state");
        appop(h, appops_service::MODE_IGNORED);
        assert_eq!(h.restriction_level(u).unwrap(), 50);
        appop(h, appops_service::MODE_ALLOWED);
        assert_eq!(h.restriction_level(u).unwrap(), 30);
        standby(h, 45, 0x200);
        assert_eq!(h.restriction_level(u).unwrap(), 40);
        appop(h, appops_service::MODE_IGNORED);
        appop(h, appops_service::MODE_ALLOWED);
        assert_eq!(h.restriction_level(u).unwrap(), 40); // previous restricted bucket survives
        standby(h, 5, DEFAULT);
        assert_eq!(h.restriction_level(u).unwrap(), 20);
        appop(h, appops_service::MODE_IGNORED);
        assert_eq!(h.restriction_level(u).unwrap(), 50); // explicit transition overrides exemption
        appop(h, appops_service::MODE_ALLOWED);
        assert_eq!(h.restriction_level(u).unwrap(), 20);
        let rows = aim_android_xml::read_sequence(&std::fs::read(path).unwrap()).unwrap();
        let p = rows
            .iter()
            .find(|r| r.string("package").as_deref() == Some("p"))
            .unwrap();
        assert_eq!(p.string("last_batt_noti_ts").as_deref(), Some("7"));
    });
}

#[test]
fn startup_raw_foreground_and_runtime_evaluation_are_distinct() {
    fixture(|h| {
        standby(h, 10, DEFAULT);
        appop(h, appops_service::MODE_FOREGROUND);
        assert_eq!(h.restriction_level(uid(h)).unwrap(), 50);
        // Ordinary bucket changes still consult AppStateTracker's cached restriction.
        standby(h, 20, 0x300);
        assert_eq!(h.restriction_level(uid(h)).unwrap(), 50);
        appop(h, appops_service::MODE_ALLOWED);
        assert_eq!(h.restriction_level(uid(h)).unwrap(), 30);
    });
}

#[test]
fn shutdown_drains_access_writes_and_rejects_late_restriction_callbacks() {
    fixture(|h| {
        standby(h, 10, DEFAULT);
        appop(h, appops_service::MODE_IGNORED);
        assert_eq!(h.restriction_level(uid(h)).unwrap(), 50);
        let access = h
            .data()
            .unwrap()
            .join("misc_de/0/apexdata/com.android.permission/access.abx");
        let saving = h.app_restrictions.0.lock().unwrap();
        let (started, start) = std::sync::mpsc::channel();
        let (finished, finish) = std::sync::mpsc::channel();
        let host = h.clone();
        let shutdown = std::thread::spawn(move || {
            started.send(()).unwrap();
            host.shutdown_persistence().unwrap();
            finished.send(()).unwrap();
        });
        start.recv().unwrap();
        assert!(
            finish.try_recv().is_err(),
            "shutdown must wait for the active save"
        );
        drop(saving);
        finish.recv().unwrap();
        shutdown.join().unwrap();
        assert!(
            access.is_file(),
            "shutdown must flush the queued access snapshot"
        );
        std::fs::remove_dir_all(h.data().unwrap()).unwrap();
        assert!(h.restrictions_changed(None).is_err());
        assert!(h.start_app_restrictions().is_err());
        h.shutdown_persistence().unwrap();
        assert!(
            !h.data().unwrap().exists(),
            "late callbacks must not recreate data"
        );
    });
}

#[test]
fn typed_backend_answers_and_unavailable_inputs_remain_errors() {
    use aim_binder_host::{
        local::{Call, Service},
        parcel::Parcel,
    };
    use aim_service_aidl::dev_aim_server_iactivitystatshost as api;
    fixture(|h| {
        standby(h, 10, DEFAULT);
        let backend = super::super::activity_stats::Backend(h.clone());
        let mut request = Parcel::new();
        api::GetRestrictionLevel { uid: uid(h) }.write(&mut request);
        let p = backend
            .transact(&mut Call {
                code: api::GET_RESTRICTION_LEVEL,
                flags: 0,
                sender_pid: 1,
                sender_euid: 1000,
                data: request.reader(),
            })
            .unwrap();
        assert_eq!(
            api::read_get_restriction_level_reply(&mut p.reader())
                .unwrap()
                .unwrap(),
            30
        );
        let path = h.data().unwrap().join("system_ce/0/hibernation/states");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, [0x0a, 0xff]).unwrap();
        assert!(h.restrictions_changed(None).is_err());
        assert!(h.restriction_level(uid(h)).is_err());
    });
}

#[test]
fn auto_bucket_effects_defer_for_active_uids_and_unrestrict_by_reason() {
    fixture(|h| {
        let u = uid(h);
        standby(h, 10, DEFAULT);
        h.platform()
            .unwrap()
            .settings
            .set_config(
                "activity_manager/",
                &mut [(format!("activity_manager/{AUTO}"), Some("true".into()))],
                Some("android"),
                true,
            )
            .unwrap();
        assert_eq!(h.restriction_level(u).unwrap(), 30);
        let donor = super::super::tests::host_with_start(7, Arc::new(|| {}));
        let mut app = donor.apps.lock().unwrap().remove(&7).unwrap();
        app.uid = u as u32;
        h.apps.lock().unwrap().insert(7, app);
        h.restrictions_changed(None).unwrap();
        appop(h, appops_service::MODE_IGNORED);
        assert_eq!(h.restriction_level(u).unwrap(), 50);
        assert_eq!(h.restriction_standby("p", 0).unwrap().0, 10);
        h.apps.lock().unwrap().remove(&7);
        h.restrictions_changed(None).unwrap();
        assert_eq!(h.restriction_standby("p", 0).unwrap(), (45, USER));
        appop(h, appops_service::MODE_ALLOWED);
        assert_eq!(h.restriction_standby("p", 0).unwrap(), (40, USAGE));
        assert_eq!(h.restriction_level(u).unwrap(), 30);
        appop(h, appops_service::MODE_IGNORED);
        h.platform()
            .unwrap()
            .settings
            .set_config(
                "activity_manager/",
                &mut [(format!("activity_manager/{AUTO}"), Some("false".into()))],
                Some("android"),
                true,
            )
            .unwrap();
        h.restrictions_changed(None).unwrap();
        assert_eq!(h.restriction_standby("p", 0).unwrap().0, 40);
        assert_eq!(h.restriction_level(u).unwrap(), 50); // the AppOp still restricts
    });
}

#[test]
fn startup_store_errors_and_absent_access_owner_never_become_defaults() {
    crate::system::with_discovery_binders(|h, _, _, _| {
        assert!(h.restriction_level(10100).is_err());
    });
    fixture(|h| {
        let dir = h.data().unwrap().join("system_de/0/apprestriction");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.xml"), b"ABX\0\x3f").unwrap();
        assert!(h.restriction_level(uid(h)).is_err());
        assert_eq!(
            std::fs::read(dir.join("settings.xml")).unwrap(),
            b"ABX\0\x3f"
        );
    });
}

#[test]
fn standby_unrestriction_retains_other_restriction_reasons() {
    fixture(|h| {
        standby(h, 45, 0x605);
        h.restriction_standby_effect("p", 0, Some(0x601), USAGE)
            .unwrap();
        assert_eq!(h.restriction_standby("p", 0).unwrap(), (45, 0x604));
        h.restriction_standby_effect("p", 0, Some(0x403), USAGE)
            .unwrap();
        assert_eq!(h.restriction_standby("p", 0).unwrap(), (45, 0x604));
        h.restriction_standby_effect("p", 0, Some(0x604), USAGE)
            .unwrap();
        assert_eq!(h.restriction_standby("p", 0).unwrap(), (40, USAGE));
    });
}

#[test]
fn restrictions_api_ignores_bucket_callbacks_until_locked_boot_completed() {
    fixture_flags(true, |h| {
        standby(h, 50, DEFAULT);
        assert_eq!(h.restriction_level(uid(h)).unwrap(), 30);
        standby(h, 45, 0x200);
        assert_eq!(h.restriction_level(uid(h)).unwrap(), 30);
        h.restriction_locked_boot_completed();
        standby(h, 40, 0x200);
        standby(h, 45, 0x200);
        assert_eq!(h.restriction_level(uid(h)).unwrap(), 40);
    });
}
