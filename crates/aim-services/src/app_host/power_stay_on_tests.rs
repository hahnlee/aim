//! Pinned android-16.0.0_r1 caller, settings-observer and stay-on contracts.
use super::super::{content_service::LocalObserver, permission_manager::PermissionManager};
use super::*;
use crate::package::uri::Uri;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Observer(AtomicUsize);
impl LocalObserver for Observer {
    fn on_change(&self, _: bool, uris: &[Uri], _: i32, _: i32) {
        assert_eq!(
            uris[0].as_str(),
            format!("content://settings/global/{STAY_ON}")
        );
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
fn call(power: &PowerManager, uid: u32, code: u32, p: &Parcel) -> Result<(), Exception> {
    let reply = power
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 71,
            sender_euid: uid,
            data: p.reader(),
        })
        .unwrap();
    reply.reader().read_exception().unwrap()
}

fn set(power: &PowerManager, uid: u32, value: i32) -> Result<(), Exception> {
    let mut p = Parcel::new();
    pm::SetStayOnSetting { val: value }.write(&mut p);
    call(power, uid, pm::SET_STAY_ON_SETTING, &p)
}

#[test]
fn stay_on_permissions_values_observers_without_assertions_or_guest_sleep() {
    for granted in ["WRITE_SETTINGS", "WRITE_SECURE_SETTINGS", "DEVICE_POWER"] {
        crate::system::with_discovery_binders(|host, publish, _, _| {
            publish(&mut |p| {
                if p.package_name == "p" {
                    let mut provider = crate::package::pkg::Provider {
                        main: p.activities[0].main.clone(),
                        authority: Some("settings".into()),
                        ..Default::default()
                    };
                    provider.main.direct_boot_aware = true;
                    p.providers.push(provider);
                }
            });
            let xml = format!(
                "<access><permissions><permission name='android.permission.{granted}' packageName='android' protectionLevel='2' type='0'/></permissions></access>"
            );
            let system = crate::package::permissions::AccessSystem::parse(
                &aim_android_xml::read(xml.as_bytes()).unwrap(),
            )
            .unwrap();
            let path = host
                .data()
                .unwrap()
                .join("misc_de/0/apexdata/com.android.permission/access.abx");
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, format!("<access><app-id-permissions><app-id id='10100'><permission name='android.permission.{granted}' flags='4'/></app-id></app-id-permissions></access>")).unwrap();
            let permissions = Arc::new(PermissionManager::with_system(host.clone(), system));
            assert!(host.permission_manager.set(permissions.clone()).is_ok());
            host.services();
            let power = host.power_service.get().unwrap();
            let settings = &host.platform().unwrap().settings;
            settings
                .insert(
                    Table::System,
                    "screen_off_timeout",
                    &super::super::settings::Change {
                        value: Some("0"),
                        tag: None,
                        make_default: false,
                        package: Some("android"),
                        caller_system: true,
                        overrideable_by_restore: false,
                    },
                    false,
                )
                .unwrap();
            let last_sleep = power.state.lock().unwrap().last_sleep;
            let before = settings.get(Table::Global, STAY_ON);
            let result = set(power, 10100, 2);
            if granted != "WRITE_SETTINGS" {
                assert_eq!(
                    result.unwrap_err().code,
                    aim_binder_host::parcel::EX_SECURITY
                );
                assert_eq!(settings.get(Table::Global, STAY_ON), before);
            } else {
                result.unwrap();
            }
            let observer = Arc::new(Observer(AtomicUsize::new(0)));
            let content = host.content_service.get().unwrap();
            let id = content
                .register_local(
                    &Uri::parse(&format!("content://settings/global/{STAY_ON}")),
                    false,
                    0,
                    observer.clone(),
                )
                .unwrap();
            for value in [15, 0, 2, 1, 4, 8] {
                let count = observer.0.load(Ordering::SeqCst);
                set(power, 0, value).unwrap(); // root bypasses WRITE_SETTINGS
                assert_eq!(
                    settings.get(Table::Global, STAY_ON),
                    Some(value.to_string())
                );
                assert_eq!(observer.0.load(Ordering::SeqCst), count + 1);
                let state = power.state.lock().unwrap();
                assert!(!state.asleep);
                assert_eq!(state.last_sleep, last_sleep);
                assert!(state.screen_assertion.name().is_none() && state.wake_locks.is_none());
            }
            content.unregister_local(id);
            let ops = super::super::appops_service::AppOps::new(host.clone(), permissions);
            ops.set_mode(1000, 23, 10100, Some(Some("p")), 1).unwrap();
            assert!(set(power, 10100, 1).is_err());
            assert_eq!(settings.get(Table::Global, STAY_ON).as_deref(), Some("8"));
            ops.set_mode(1000, 23, 10100, Some(Some("p")), 0).unwrap();
            set(power, 10100, 4).unwrap(); // app-op grant without static WRITE_SETTINGS
            if granted == "WRITE_SETTINGS" {
                // Past the removed worker's 10-second minimum. Even a zero
                // Android timeout must not schedule sleep: the Mac owns it (#670).
                std::thread::sleep(std::time::Duration::from_millis(10_100));
                let state = power.state.lock().unwrap();
                assert!(!state.asleep);
                assert_eq!(state.last_sleep, last_sleep);
                assert!(state.screen_assertion.name().is_none() && state.wake_locks.is_none());
            }
        });
    }
}
