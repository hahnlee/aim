use super::super::super::activity_stack::{Activity, Task};
use super::*;
use aim_binder_host::local::DUMP_TRANSACTION;
use std::fs;
use std::io::Seek;
use std::os::fd::AsFd;

#[test]
fn multi_task_window_dump_decodes_with_pinned_schema() {
    let host = super::super::super::tests::host_with_start(1, Arc::new(|| {}));
    host.tasks.windows.set(true).unwrap();
    host.activities.stack.lock().unwrap().freeform = true;
    let global = Configuration::of_metrics(
        &super::super::super::display::Metrics {
            width: 1920,
            height: 1080,
            density_dpi: 160,
        },
        &super::super::super::config::Device {
            locales: "en-US",
            right_to_left: false,
            night: false,
            font_scale: 1.,
            font_weight_adjustment: 0,
            grammatical_gender: 0,
            seq: 1,
        },
    );
    for (id, left) in [(7, 20), (8, 600)] {
        let bounds = Rect {
            left,
            top: 50,
            right: left + 400,
            bottom: 850,
        };
        host.tasks.animation_test_task(id, bounds);
        let mut stack = host.activities.stack.lock().unwrap();
        stack.tasks.push(Task {
            id,
            affinity: Some("p".into()),
            real_activity: format!("p/p.A{id}"),
            intent: Default::default(),
            base: super::super::super::activity_start::LaunchIntent::Component,
            uid: 10100,
            user: 0,
            last_active: 1,
        });
        let mut activity = Activity::new(id as u64, id, 1, &format!("p/p.A{id}"), 0, 0, None);
        activity.state = State::Resumed;
        stack.activities.push(activity);
        drop(stack);
        host.animation_test_window(host.process.strong(0), id as u64, id, id as u32);
        let mut windows = host.windows.windows.lock().unwrap();
        let w = windows.get_mut(&(id as u32)).unwrap();
        w.name = format!("p/p.A{id}");
        w.layer = w.container.take();
        w.frames.display = global.window.bounds;
    }
    // Real parent/child window relation, not another activity or task.
    host.animation_test_window(host.process.strong(0), 8, 8, 9);
    host.animation_test_window(host.process.strong(0), 8, 8, 10);
    {
        let mut windows = host.windows.windows.lock().unwrap();
        windows.get_mut(&10).unwrap().activity = None;
        let popup = windows.get_mut(&9).unwrap();
        popup.parent = Some(8);
        popup.name = "popup".into();
    }
    host.update_window_focus();
    let dump = host.window_dump(&global);
    for value in [
        "root_window_container",
        "DefaultTaskDisplayArea",
        "RESUMED",
        "p/.A7",
        "p/.A8",
        "focused_window",
    ] {
        assert!(dump.text.contains(value), "{value}");
    }
    let dir = super::super::super::test_temp_dir("window-dump");
    fs::write(dir.join("dump.pb"), &dump.proto.0).unwrap();
    let result = std::process::Command::new("python3")
        .arg(aim_paths::root().join("crates/aim-services/tests/window_dump_decode.py"))
        .arg(aim_paths::aosp())
        .arg(aim_paths::root().join("crates/aim-services/sources.lock"))
        .arg(dir.join("dump.pb"))
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    fs::remove_dir_all(dir).unwrap();
    host.process.shutdown().unwrap();
}

#[test]
fn dump_arguments_follow_priority_dump_and_wms() {
    for args in [
        vec!["--proto"],
        vec!["-a", "--proto"],
        vec!["--proto", "-a"],
        vec!["windows", "--proto"],
    ] {
        assert_eq!(
            options(&args.into_iter().map(|s| Some(s.into())).collect::<Vec<_>>()),
            (true, false, String::new())
        );
    }
    assert_eq!(options(&[Some("-a".into())]), (false, false, String::new()));
    assert!(options(&[Some("-h".into()), Some("--proto".into())]).1);
    assert_eq!(options(&[None, Some("-bad".into())]).2, "");
    assert!(
        options(&[Some("-bad".into())])
            .2
            .starts_with("Unknown argument: -bad;")
    );
}

#[test]
fn dump_fd_and_permission_contract() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let permissions = super::super::super::permission_manager::PermissionManager::with_system(
            host.clone(),
            Default::default(),
        );
        assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
        struct Output(aim_binder_driver::File);
        impl Service for Output {
            fn descriptor(&self) -> &str {
                ""
            }
            fn transact(&self, _: &mut Call<'_>) -> Reply {
                let mut p = Parcel::new();
                p.write_file(self.0.clone());
                Ok(p)
            }
        }
        let service = WindowManager(host.clone());
        let invoke = |p: &Parcel, uid| {
            service.transact(&mut Call {
                code: DUMP_TRANSACTION,
                flags: 0,
                sender_pid: 42,
                sender_euid: uid,
                data: p.reader(),
            })
        };
        assert_eq!(
            invoke(&Parcel::new(), 2000).err(),
            Some(aim_binder_host::parcel::NOT_ENOUGH_DATA)
        );
        let dir = super::super::super::test_temp_dir("window-dump-fd");
        let path = dir.join("output");
        let mut file = fs::File::create(&path).unwrap();
        let endpoint = export(Arc::new(Output(
            aim_binder_host::server::file_from_fd(file.as_fd()).unwrap(),
        )));
        let received = endpoint.transact(1, &Parcel::new(), false).unwrap();
        let base = received.into_parcel();
        let mut request = base.clone();
        aim_service_aidl::write_string_list(&mut request, Some(&[Some("--proto".into())]));
        invoke(&request, 10100)
            .unwrap()
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        let denial = fs::read_to_string(&path).unwrap();
        assert!(
            denial.contains("pid=42, uid=10100")
                && denial.contains("missing android.permission.DUMP")
        );
        request.write_i32(7);
        assert_eq!(invoke(&request, 1000).err(), Some(BAD_VALUE));
        assert_eq!(
            invoke(&base, 2000).err(),
            Some(aim_binder_host::parcel::NOT_ENOUGH_DATA)
        );
        // Authorized -h is handled before state collection and emits text even with --proto.
        file.set_len(0).unwrap();
        file.rewind().unwrap();
        let mut request = base.clone();
        aim_service_aidl::write_string_list(
            &mut request,
            Some(&[Some("-h".into()), Some("--proto".into())]),
        );
        invoke(&request, 1000)
            .unwrap()
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        assert!(
            fs::read_to_string(&path)
                .unwrap()
                .contains("Window manager dump options:")
        );
        host.display_metrics
            .set(Ok(super::super::super::display::Metrics {
                width: 1920,
                height: 1080,
                density_dpi: 160,
            }))
            .unwrap();
        file.set_len(0).unwrap();
        file.rewind().unwrap();
        let mut request = base.clone();
        aim_service_aidl::write_string_list(
            &mut request,
            Some(&[Some("-a".into()), Some("--proto".into())]),
        );
        invoke(&request, 1000)
            .unwrap()
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        assert!(!fs::read(&path).unwrap().is_empty());
        let readonly = fs::File::open(&path).unwrap();
        let endpoint = export(Arc::new(Output(
            aim_binder_host::server::file_from_fd(readonly.as_fd()).unwrap(),
        )));
        let mut request = endpoint
            .transact(1, &Parcel::new(), false)
            .unwrap()
            .into_parcel();
        aim_service_aidl::write_string_list(&mut request, Some(&[Some("-h".into())]));
        assert_eq!(invoke(&request, 1000).err(), Some(FAILED_TRANSACTION));
        assert!(file.metadata().is_ok()); // The supplied fd remains owned by its caller.
        drop(file);
        fs::remove_dir_all(dir).unwrap();
    });
}
