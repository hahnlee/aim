//! ShortcutService.hasShortcutHostPermission and UsageStatsService.queryEventsHelper,
//! android-16.0.0_r1: shortcut visibility must not discard the rest of a query.
use super::*;

#[test]
fn shortcut_invocations_follow_the_callers_permission_and_home_role() {
    const ACCESS: &str = "android.permission.ACCESS_SHORTCUTS";
    crate::system::with_discovery_session_flags(&[(USE_PARCELED_LIST, true)], |host| {
        let dir = host
            .data()
            .unwrap()
            .join("misc_de/0/apexdata/com.android.permission");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("access.abx"),
            aim_android_xml::abx::write(
                &aim_android_xml::read(format!(
                    "<access><app-id-permissions><app-id id='10100'><permission name='{ACCESS}' flags='1'/></app-id></app-id-permissions></access>"
                ).as_bytes()).unwrap(),
            ).unwrap(),
        ).unwrap();
        let permissions = super::super::permission_manager::PermissionManager::with_system(
            host.clone(),
            crate::package::permissions::AccessSystem {
                permissions: vec![crate::package::permissions::Permission {
                    name: ACCESS.into(),
                    package: "android".into(),
                    protection_level: 2,
                    dynamic: None,
                }],
                ..Default::default()
            },
        );
        assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
        let allowed = |uid, package| {
            super::super::shortcut_service::has_host_permission(host, uid, package, 0)
        };
        // ACCESS_SHORTCUTS does not require a loaded or unlocked user.
        assert!(allowed(10100, "reader").unwrap());
        let locked = allowed(10101, "home").unwrap_err();
        assert_eq!(locked.code, EX_ILLEGAL_STATE);
        assert_eq!(
            locked.message,
            "User (with userId=0) is locked or not running"
        );
        host.set_user_unlocked_for_package_test(0, true);
        std::fs::write(
            dir.join("roles.xml"),
            "<roles version='1'><role name='android.app.role.HOME'><holder name='home'/></role></roles>",
        ).unwrap();
        assert!(
            host.role_service
                .set(Arc::new(super::super::role_service::RoleManager::new(
                    host.clone()
                )))
                .is_ok()
        );
        assert!(allowed(10101, "home").unwrap());
        assert!(!allowed(10101, "reader").unwrap());

        let (elapsed, now) = clocks();
        let dir = host.data().unwrap().join("system_ce/0/usagestats");
        let (mut stats, _, _) = UserStats::init(
            &dir,
            (elapsed, now),
            "test",
            DEFAULT_SELECTION_LOG_RETENTION_LEN,
            &HashMap::new(),
        )
        .unwrap();
        let daily = &mut stats.current[INTERVAL_DAILY];
        daily.events = vec![Event {
            package: "publisher".into(),
            class: None,
            instance_id: 0,
            task_root_package: None,
            task_root_class: None,
            event_type: usage_events::SHORTCUT_INVOCATION,
            time_stamp: now,
            flags: 0,
            extra: Extra::ShortcutId("conversation".into()),
        }];
        daily.end_time = now + 1;
        *host.usage.lock().unwrap() = Some(stats);
        let service = UsageStats::new(host.clone());
        let count = |uid, package| {
            service
                .query_events(uid, package, now - 1, now + 1000)
                .unwrap()
                .map(|written| written.0.reader().read_i32().unwrap())
        };
        assert_eq!(count(10101, "reader"), None);
        assert_eq!(count(10101, "home"), Some(1));
        assert_eq!(count(10100, "reader"), Some(1));

        let mut ordinary =
            host.usage.lock().unwrap().as_ref().unwrap().current[INTERVAL_DAILY].events[0].clone();
        ordinary.event_type = usage_events::USER_INTERACTION;
        ordinary.extra = Extra::None;
        host.usage.lock().unwrap().as_mut().unwrap().current[INTERVAL_DAILY]
            .events
            .push(ordinary);
        assert_eq!(count(10101, "reader"), Some(1));
        assert_eq!(count(10101, "home"), Some(2));
        // Package-scoped queries keep the publisher's own shortcut events.
        assert!(
            service
                .query_events_for_package(now - 1, now + 1000, "publisher", false)
                .unwrap()
                .is_some()
        );
    });
}
