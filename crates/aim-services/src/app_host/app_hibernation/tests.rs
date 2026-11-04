//! Source-derived: AppHibernationService, android-16.0.0_r1: reads before and
//! after the states are loaded, MANAGE_APP_HIBERNATION, full cross-user access,
//! disabled DeviceConfig, transitions, persistence, use and package events.
use super::super::permission_manager;
use super::*;
use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, EX_SECURITY, EX_UNSUPPORTED_OPERATION};
use aim_service_aidl::dev_aim_server_iartdeletion as art;
use std::sync::atomic::{AtomicI64, Ordering};

const READS: [u32; 2] = [
    ah::IS_HIBERNATING_FOR_USER,
    ah::GET_HIBERNATING_PACKAGES_FOR_USER,
];
fn error(p: &Parcel) -> i32 {
    p.reader().read_exception().unwrap().unwrap_err().code
}
fn success(p: &Parcel) {
    p.reader().read_exception().unwrap().unwrap();
}

fn call(s: &dyn Service, code: u32, uid: u32, user: i32, package: Option<&str>) -> Parcel {
    let mut p = Parcel::new();
    p.write_interface_token(ah::DESCRIPTOR);
    if code == ah::IS_HIBERNATING_FOR_USER {
        p.write_string16(package);
    }
    p.write_i32(user);
    s.transact(&mut Call {
        code,
        flags: 0,
        sender_pid: 71,
        sender_euid: uid,
        data: p.reader(),
    })
    .unwrap()
}

fn request(s: &dyn Service, code: u32, uid: u32, args: impl FnOnce(&mut Parcel)) -> Parcel {
    let mut p = Parcel::new();
    p.write_interface_token(ah::DESCRIPTOR);
    args(&mut p);
    s.transact(&mut Call {
        code,
        flags: 0,
        sender_pid: 71,
        sender_euid: uid,
        data: p.reader(),
    })
    .unwrap()
}

fn set_user(s: &dyn Service, uid: u32, user: i32, hibernating: bool) -> Parcel {
    request(s, ah::SET_HIBERNATING_FOR_USER, uid, |p| {
        p.write_string16(Some("p"));
        p.write_i32(user);
        p.write_bool(hibernating);
    })
}

fn set_global(s: &dyn Service, uid: u32, hibernating: bool) -> Parcel {
    request(s, ah::SET_HIBERNATING_GLOBALLY, uid, |p| {
        p.write_string16(Some("p"));
        p.write_bool(hibernating);
    })
}

fn bool_reply(p: &Parcel) -> bool {
    let mut r = p.reader();
    r.read_exception().unwrap().unwrap();
    r.read_bool().unwrap()
}

fn globally(s: &dyn Service) -> bool {
    bool_reply(&request(s, ah::IS_HIBERNATING_GLOBALLY, 1000, |p| {
        p.write_string16(Some("p"))
    }))
}

fn for_user(s: &dyn Service) -> bool {
    bool_reply(&call(s, ah::IS_HIBERNATING_FOR_USER, 1000, 0, Some("p")))
}

/// The system and two callers holding MANAGE, one with full cross-user access;
/// "p" is stopped, as a hibernating package is.
fn grant(host: &Arc<AppHost>) {
    let pm = permission_manager::PermissionManager::with_system(host.clone(), Default::default());
    assert!(host.permission_manager.set(Arc::new(pm)).is_ok());
    let system = host.package_system().unwrap();
    let base = host.package_capture().unwrap();
    let mut context = (**base.context()).clone();
    context.system.system_permissions = Some(
        [
            (2901, "android.permission.INTERACT_ACROSS_USERS"),
            (2902, "android.permission.INTERACT_ACROSS_USERS_FULL"),
        ]
        .into_iter()
        .map(|(uid, cross)| (uid, [MANAGE.into(), cross.into()].into()))
        .collect(),
    );
    context.scan_version = base.scan().version() + 1;
    system
        .publish_package_scan_with_queries(
            &system.package_bootstrap().unwrap(),
            Some(base.scan()),
            base.scan().owner().clone(),
            base.scan().usage().clone(),
            context,
        )
        .unwrap();
    system
        .internal_set_package_stopped_state(Some("p".into()), true, 0, 1000, 0)
        .unwrap();
}

fn disable(host: &AppHost) {
    host.platform()
        .unwrap()
        .settings
        .set_config(
            "app_hibernation/",
            &mut [(
                "app_hibernation/app_hibernation_enabled".into(),
                Some("false".into()),
            )],
            Some("android"),
            true,
        )
        .unwrap();
}

#[test]
fn published_manager_reads_fresh_state_and_enforces_original_permissions() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        grant(host);
        let services = host.services();
        let s = services
            .iter()
            .find(|(name, _)| *name == "app_hibernation")
            .unwrap()
            .1
            .as_ref();
        let hibernation = AppHibernation(host.clone());
        // Before the states are read, reads answer as for an absent user.
        assert!(!for_user(s));
        hibernation.boot_completed().unwrap();
        hibernation.user_unlocking().unwrap();
        hibernation.restrictions_unmodelled(0).unwrap();
        for package in [Some("p"), Some("uninstalled.package"), None] {
            let p = call(s, ah::IS_HIBERNATING_FOR_USER, 2901, 0, package);
            let mut r = p.reader();
            r.read_exception().unwrap().unwrap();
            assert_eq!(r.read_i32().unwrap(), 0);
            assert_eq!(r.remaining(), 0);
        }
        for code in READS {
            for (uid, user, expected) in [
                (10100, 0, EX_SECURITY),
                (2901, 10, EX_SECURITY),
                (1000, -1, EX_ILLEGAL_ARGUMENT),
            ] {
                let p = call(s, code, uid, user, Some("p"));
                assert_eq!(error(&p), expected);
            }
            // Full cross-user permission reaches the original absent-user empty state.
            success(&call(s, code, 2902, 10, None));
        }
        for user in [0, -2, -3] {
            let p = call(s, ah::GET_HIBERNATING_PACKAGES_FOR_USER, 1000, user, None);
            let mut r = p.reader();
            r.read_exception().unwrap().unwrap();
            assert_eq!(r.read_i32().unwrap(), 0); // empty list, never null (-1)
            assert_eq!(r.remaining(), 0);
        }
        let path = host.data().unwrap().join("system_ce/0/hibernation/states");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, [0x0a, 5, 0x0a, 1, b'p', 0x10, 1]).unwrap();
        hibernation.user_unlocking().unwrap();
        assert_eq!(
            hibernation.restrictions_unmodelled(0).unwrap_err().code,
            EX_UNSUPPORTED_OPERATION
        );
        assert!(for_user(s));
        disable(host);
        // Disabled reads and writes precede permission, user validation and state.
        for code in READS {
            success(&call(s, code, 10100, -1, None));
        }
        success(&set_user(s, 10100, -1, false));
        success(&set_global(s, 10100, true));
        hibernation.restrictions_unmodelled(0).unwrap();
    });
}

// Pinned UserLevelHibernationProto persists no savedByte, whereas the global
// writer persists int64 field 3. Unknown fields are skipped by both readers.
#[test]
fn original_proto_fields_preserve_global_bytes_and_skip_unknown_fields() {
    let bytes = [
        0x0a, 11, 0x0a, 1, b'p', 0x10, 1, 0x18, 0x80, 0x80, 0x80, 0x80, 0x10,
    ];
    let global = decode(&bytes, true).unwrap();
    assert!(global["p"].hibernated);
    assert_eq!(global["p"].saved, 1_i64 << 32);
    assert_eq!(decode(&bytes, false).unwrap()["p"].saved, 0);
    assert!(decode(&[0x0a, 2, 0x10], true).is_err());
    assert!(decode(&[0x0a, 0], false).unwrap().is_empty());
}

#[test]
fn persisted_states_load_atomic_backup_and_correct_stale_rows() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        grant(host);
        let s = AppHibernation(host.clone());
        let data = host.data().unwrap();
        let user = data.join("system_ce/0/hibernation/states");
        let global = data.join("system/hibernation/states");
        for path in [&user, &global] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        }
        let awake = [0x0a, 3, 0x0a, 1, b'p'];
        let asleep = [0x0a, 5, 0x0a, 1, b'p', 0x10, 1];
        std::fs::write(&user, awake).unwrap();
        std::fs::write(user.with_extension("bak"), asleep).unwrap();
        // A removed package's record is dropped.
        let mut recorded = vec![0x0a, 7, 0x0a, 1, b'p', 0x10, 1, 0x18, 99];
        recorded.extend([0x0a, 7, 0x0a, 3, b'x', b'.', b'y', 0x10, 1]);
        std::fs::write(&global, &recorded).unwrap();
        s.boot_completed().unwrap();
        s.user_unlocking().unwrap();
        assert!(for_user(&s));
        assert!(!user.with_extension("bak").exists());
        assert_eq!(std::fs::read(&user).unwrap(), asleep);
        assert!(globally(&s));
        let p = request(&s, ah::GET_HIBERNATION_STATS_FOR_USER, 1000, |p| {
            aim_service_aidl::write_string_list(
                p,
                Some(&[Some("p".into()), Some("missing".into())]),
            );
            p.write_i32(0);
        });
        assert_eq!(stats(&p), [("p".to_string(), 99)]);
        assert_eq!(
            error(&request(&s, ah::IS_HIBERNATING_GLOBALLY, 10100, |p| p
                .write_string16(Some("p")))),
            EX_SECURITY
        );
        // An unstopped package's recorded user hibernation is stale; the
        // unlocking user then leaves global hibernation, which is written.
        host.package_system()
            .unwrap()
            .internal_set_package_stopped_state(Some("p".into()), false, 0, 1000, 0)
            .unwrap();
        s.user_unlocking().unwrap();
        assert!(!for_user(&s));
        assert!(!globally(&s));
        assert_eq!(
            std::fs::read(&global).unwrap(),
            [0x0a, 3, 0x0a, 1, b'p', 0x0a, 3, 0x0a, 1, b'q']
        );
        disable(host);
        assert!(!s.hibernating(10100, Some("p"), -1, true).unwrap());
        // This resource query always checks MANAGE, even while DeviceConfig is off.
        assert_eq!(
            error(&request(
                &s,
                ah::IS_OAT_ARTIFACT_DELETION_ENABLED,
                10100,
                |_| {}
            )),
            EX_SECURITY
        );
    });
}

fn stats(p: &Parcel) -> Vec<(String, i64)> {
    let mut r = p.reader();
    r.read_exception().unwrap().unwrap();
    let entries = aim_service_aidl::read_value_map(&mut r).unwrap().unwrap();
    assert_eq!(r.remaining(), 0);
    entries
        .into_iter()
        .map(|(name, value)| {
            let aim_service_aidl::ParcelValue::String(Some(name)) = name else {
                panic!("no name")
            };
            let aim_service_aidl::ParcelValue::Custom { kind: 4, bytes } = value else {
                panic!("no stats")
            };
            let mut stats = aim_binder_host::parcel::Reader::new(&bytes, &[]);
            assert_eq!(
                stats.read_string16().unwrap().as_deref(),
                Some("android.apphibernation.HibernationStats")
            );
            let saved = stats.read_i64().unwrap();
            assert_eq!(stats.remaining(), 0);
            (name, saved)
        })
        .collect()
}

/// The original ART owner's deletion as the system's own call.
struct Art(Arc<AtomicI64>, Arc<Mutex<Vec<(Option<String>, i32, i32)>>>);
impl Service for Art {
    fn descriptor(&self) -> &str {
        art::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, art::DELETE_DEXOPT_ARTIFACTS);
        let a = art::DeleteDexoptArtifacts::read(&mut call.data)?;
        self.1
            .lock()
            .unwrap()
            .push((a.package_name, a.calling_uid, a.user_id));
        let mut p = Parcel::new();
        art::write_delete_dexopt_artifacts_reply(&mut p, self.0.load(Ordering::SeqCst));
        Ok(p)
    }
}

#[test]
fn transitions_persist_and_leave_hibernation_on_use_unstop_and_package_events() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        grant(host);
        let s = AppHibernation(host.clone());
        let data = host.data().unwrap();
        let user = data.join("system_ce/0/hibernation/states");
        let global = data.join("system/hibernation/states");
        // Before PHASE_BOOT_COMPLETED there is no global state to change.
        success(&set_global(&s, 1000, true));
        assert!(!globally(&s) && !global.exists());
        s.boot_completed().unwrap();
        s.user_unlocking().unwrap();
        assert!(s.suppresses_dexopt("p").is_ok_and(|v| !v));
        let deletes = host
            .framework()
            .unwrap()
            .framework_bool("config_hibernationDeletesOatArtifactsEnabled")
            .unwrap();
        assert!(deletes, "the image's original deletion configuration");
        let bytes = Arc::new(AtomicI64::new(-1));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let owner = export(Arc::new(Art(bytes.clone(), calls.clone())));
        super::super::sync_owner::tests::install(
            host,
            export(Arc::new(Art(bytes.clone(), calls.clone()))),
        );
        let system = host.package_system().unwrap();
        let epoch = Arc::new(crate::package::boot_captures::Captures::new(
            system,
            system.package_bootstrap().unwrap().clone(),
        ));
        host.attach_art_owner(42, 100, epoch, Some(owner.binder()))
            .unwrap();
        assert_eq!(error(&set_global(&s, 10100, true)), EX_SECURITY);
        // ART's -1 is clamped to no saved bytes.
        success(&set_global(&s, 1000, true));
        assert!(globally(&s) && s.suppresses_dexopt("p").unwrap());
        assert_eq!(*calls.lock().unwrap(), [(Some("p".into()), 1000, 0)]);
        assert_eq!(
            std::fs::read(&global).unwrap(),
            [0x0a, 5, 0x0a, 1, b'p', 0x10, 1, 0x0a, 3, 0x0a, 1, b'q']
        );
        success(&set_global(&s, 1000, false));
        bytes.store(4096, Ordering::SeqCst);
        success(&set_global(&s, 1000, true));
        // A repeated transition has no effect.
        success(&set_global(&s, 1000, true));
        assert_eq!(calls.lock().unwrap().len(), 2);
        let saved = [
            0x0a, 8, 0x0a, 1, b'p', 0x10, 1, 0x18, 0x80, 0x20, 0x0a, 3, 0x0a, 1, b'q',
        ];
        assert_eq!(std::fs::read(&global).unwrap(), saved);
        let all = request(&s, ah::GET_HIBERNATION_STATS_FOR_USER, 1000, |p| {
            aim_service_aidl::write_string_list(p, None);
            p.write_i32(0);
        });
        assert!(stats(&all).contains(&("p".to_string(), 4096)));
        // Entering a user's hibernation needs its force stop and cache effects.
        assert_eq!(
            error(&set_user(&s, 1000, 0, true)),
            EX_UNSUPPORTED_OPERATION
        );
        assert!(!for_user(&s));
        // Another event type is not use; a resumed activity is.
        s.used("p", 0, 2).unwrap();
        assert!(globally(&s));
        s.used("p", 0, usage_events::ACTIVITY_RESUMED).unwrap();
        assert!(!globally(&s));
        assert_eq!(
            std::fs::read(&global).unwrap(),
            [0x0a, 3, 0x0a, 1, b'p', 0x0a, 3, 0x0a, 1, b'q']
        );
        // An unstopped package hibernating for the user leaves both states.
        std::fs::create_dir_all(user.parent().unwrap()).unwrap();
        std::fs::write(&user, [0x0a, 5, 0x0a, 1, b'p', 0x10, 1]).unwrap();
        s.user_unlocking().unwrap();
        success(&set_global(&s, 1000, true));
        assert!(for_user(&s) && globally(&s));
        s.unhibernate("p", 0).unwrap();
        assert!(!for_user(&s) && !globally(&s));
        assert_eq!(
            std::fs::read(&user).unwrap(),
            [0x0a, 3, 0x0a, 1, b'p', 0x0a, 3, 0x0a, 1, b'q']
        );
        // A removal for the user drops its state; the remaining code keeps the
        // global state. A new addition starts awake.
        success(&set_global(&s, 1000, true));
        s.package_removed("p", 0).unwrap();
        success(&set_user(&s, 1000, 0, true));
        assert!(globally(&s));
        s.package_added("p", 0);
        assert!(!for_user(&s) && globally(&s));
    });
}
