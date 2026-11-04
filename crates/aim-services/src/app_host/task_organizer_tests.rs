//! Android 16 registration, permission and retained-leash contracts over real Binder.
use super::*;
use aim_binder_host::parcel::{EX_NULL_POINTER, EX_SECURITY, EX_UNSUPPORTED_OPERATION};
use std::sync::mpsc;
use std::time::{Duration, Instant};
struct Callback(mpsc::Sender<(u32, Parcel)>);
impl Service for Callback {
    fn descriptor(&self) -> &str {
        callback::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert!(call.is_oneway());
        call.data.enforce_interface(callback::DESCRIPTOR)?;
        let start = call.data.position();
        call.data.skip(call.data.remaining())?;
        let (bytes, objects) = call.data.since(start);
        let mut payload = Parcel::new();
        payload.write_raw(bytes, &objects);
        self.0.send((call.code, payload)).unwrap();
        Ok(Parcel::new())
    }
}
fn invoke(service: &dyn Service, code: u32, data: &Parcel, uid: u32) -> Parcel {
    service
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 42,
            sender_euid: uid,
            data: data.reader(),
        })
        .unwrap()
}
pub(in crate::app_host) fn apply_wct(host: &Arc<AppHost>, payload: &Parcel, uid: u32) -> Parcel {
    let mut request = Parcel::new();
    request.write_interface_token(woc::DESCRIPTOR);
    request.write_i32(1);
    request.write_raw(payload.data(), payload.objects());
    invoke(
        &Controller(host.clone(), false),
        woc::APPLY_TRANSACTION,
        &request,
        uid,
    )
}
fn register(host: &Arc<AppHost>, who: &Strong) -> Result<Parcel, Exception> {
    host.register_task_organizer(Some(who.binder()))
}
fn unregister(host: &Arc<AppHost>, who: &Strong) {
    host.unregister_task_organizer(Some(who.binder()), None)
        .unwrap();
}
fn state(host: &AppHost) -> std::sync::MutexGuard<'_, State> {
    host.organizers.state.lock().unwrap()
}
fn denied(p: Parcel, code: i32) {
    assert_eq!(p.reader().read_exception().unwrap().unwrap_err().code, code);
}
fn permissions(host: &Arc<AppHost>) {
    let pm = super::super::permission_manager::PermissionManager::with_system(
        host.clone(),
        Default::default(),
    );
    assert!(host.permission_manager.set(Arc::new(pm)).is_ok());
}
fn empty(p: Parcel) {
    let mut r = p.reader();
    r.read_exception().unwrap().unwrap();
    assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap()], [1, 0]);
    assert_eq!(r.remaining(), 0);
}
#[test]
fn controller_getters_are_stable_and_task_permission_is_enforced() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        permissions(host);
        let window_controller = host.window_organizer_controller();
        assert_eq!(window_controller, host.window_organizer_controller());
        let controller = Controller(host.clone(), false);
        let mut request = Parcel::new();
        woc::GetTaskOrganizerController {}.write(&mut request);
        let ask = |uid| {
            invoke(
                &controller,
                woc::GET_TASK_ORGANIZER_CONTROLLER,
                &request,
                uid,
            )
        };
        denied(ask(10100), EX_SECURITY);
        assert!(host.organizers.tasks.get().is_none());
        let get = || {
            woc::read_get_task_organizer_controller_reply(&mut ask(1000).reader())
                .unwrap()
                .unwrap()
                .unwrap()
        };
        assert_eq!(get(), get());
        denied(
            invoke(
                &controller,
                woc::APPLY_SYNC_TRANSACTION,
                &Parcel::new(),
                1000,
            ),
            EX_UNSUPPORTED_OPERATION,
        );
        denied(
            invoke(
                &controller,
                woc::APPLY_SYNC_TRANSACTION,
                &Parcel::new(),
                10100,
            ),
            EX_SECURITY,
        );
        let mut transaction = Parcel::new();
        transaction.write_i32(0);
        transaction.write_i32(0);
        transaction.write_binder(None);
        transaction.write_binder(None);
        denied(
            apply_wct(host, &transaction, 1000),
            EX_UNSUPPORTED_OPERATION,
        );
        let task = Controller(host.clone(), true);
        let mut request = Parcel::new();
        toc::RegisterTaskOrganizer { organizer: None }.write(&mut request);
        denied(
            invoke(&task, toc::REGISTER_TASK_ORGANIZER, &request, 10100),
            EX_SECURITY,
        );
        denied(
            invoke(&task, toc::GET_IME_TARGET, &Parcel::new(), 1000),
            EX_UNSUPPORTED_OPERATION,
        );
        for code in [toc::REGISTER_TASK_ORGANIZER, toc::UNREGISTER_TASK_ORGANIZER] {
            denied(invoke(&task, code, &request, 1000), EX_NULL_POINTER);
        }
        assert!(state(host).entries.is_empty());
        for (uid, code) in [(10100, EX_SECURITY), (1000, EX_NULL_POINTER)] {
            assert_eq!(
                host.clear_organizer_launch_params(uid, None)
                    .unwrap_err()
                    .code,
                code
            );
        }
    });
}
#[test]
fn empty_registration_is_nonnull_idempotent_and_unregistration_releases_watch() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let (tx, events) = mpsc::channel();
        let organizer = export(Arc::new(Callback(tx)));
        for _ in 0..2 {
            empty(register(host, &organizer).unwrap());
            assert_eq!(state(host).entries.len(), 1);
        }
        let stale = state(host).entries[0].held.clone();
        unregister(host, &organizer);
        register(host, &organizer).unwrap();
        host.unregister_task_organizer(Some(organizer.binder()), Some(&stale))
            .unwrap();
        assert_eq!(state(host).entries.len(), 1); // queued old death cannot retire the new registration
        for _ in 0..2 {
            unregister(host, &organizer);
        }
        assert!(state(host).entries.is_empty());
        assert!(events.try_recv().is_err());
    });
}
#[test]
fn organizer_binder_death_retires_registration() {
    crate::system::with_discovery_binders_lifecycle(&[], true, |host, _, _, export, die| {
        let (tx, _) = mpsc::channel();
        let organizer = export(Arc::new(Callback(tx)));
        register(host, &organizer).unwrap();
        die();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !state(host).entries.is_empty() {
            assert!(Instant::now() < deadline, "dead organizer retained");
            std::thread::yield_now();
        }
        assert!(state(host).reports.is_empty());
    });
}
#[test]
fn native_task_loss_during_registration_does_not_publish_a_partial_registration() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let (tx, _) = mpsc::channel();
        let organizer = export(Arc::new(Callback(tx)));
        let surface = export(Arc::new(super::super::activity::Token));
        assert!(
            host.composer
                .set(super::super::surface::Composer::for_test(
                    &host.process,
                    surface.handle
                ))
                .is_ok()
        );
        host.tasks.animation_test_task(7, Default::default());
        // Activity owner removal precedes native layer removal. No Entry/watch may escape this failure.
        host.tasks.organizer_test_layer(
            7,
            Layer::animation_test_layer(host.process.strong(surface.handle), 71),
        );
        assert!(register(host, &organizer).is_err());
        assert!(state(host).entries.is_empty());
        host.remove_task(7);
        empty(register(host, &organizer).unwrap());
        host.remove_task(7); // native removal after registration is idempotent too
        unregister(host, &organizer);
    });
}
#[test]
fn callback_delivery_preserves_initial_list_and_appeared_changed_vanished_order() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let (tx, events) = mpsc::channel();
        let organizer = export(Arc::new(Callback(tx)));
        register(host, &organizer).unwrap();
        let surface = export(Arc::new(super::super::activity::Token));
        let layer = Layer::animation_test_layer(host.process.strong(surface.handle), 71);
        let leash = Leash::new(host, surface.binder(), &layer);
        let mut info = super::super::task_info::tests::organizer_fixture();
        info.token = surface.binder();
        let snapshot = |info: &super::super::task_info::RunningTaskInfo| {
            let mut p = Parcel::new();
            info.write_to(&mut p);
            Appeared {
                info: p,
                leash: leash.clone(),
            }
        };
        let before = snapshot(&info);
        assert_eq!(
            state(host)
                .update(&[(9, true, false, before.clone())], true)
                .len(),
            1
        );
        state(host).update(&[(9, true, false, before.clone())], false);
        host.dispatch_organizer_reports();
        assert!(events.try_recv().is_err()); // no duplicate initial or unchanged callback
        info.is_focused = false;
        let after = snapshot(&info);
        for current in [
            vec![(9, true, false, after.clone())],
            vec![],
            vec![(9, false, false, before.clone())],
            vec![(9, true, false, before.clone())],
        ] {
            state(host).update(&current, false);
        }
        host.dispatch_organizer_reports();
        for (code, expected) in [
            (callback::ON_TASK_INFO_CHANGED, &after),
            (callback::ON_TASK_VANISHED, &after),
            (callback::ON_TASK_APPEARED, &before),
        ] {
            let (actual, payload) = events.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(actual, code);
            let mut bytes = payload.data()[4..4 + expected.info.data().len()].to_vec();
            for at in expected.info.objects().iter().map(|at| *at as usize) {
                bytes[at..at + 24].copy_from_slice(&expected.info.data()[at..at + 24]);
            }
            assert_eq!(bytes, expected.info.data());
            let mut r = payload.reader();
            r.skip(4 + bytes.len()).unwrap();
            if code == callback::ON_TASK_APPEARED {
                assert_eq!(r.read_i32().unwrap(), 1);
                assert_eq!(
                    r.read_string8().unwrap().as_deref(),
                    Some(layer.name.as_str())
                );
                assert_eq!([r.read_i32().unwrap(), r.read_i32().unwrap()], [0, 0]);
                assert!(r.read_bool().unwrap());
                assert!(r.read_binder().unwrap().is_some());
                assert!(r.read_binder().unwrap().is_some());
                assert_eq!(r.read_i32().unwrap(), 71);
            } else {
                assert_eq!(r.remaining(), 0);
            }
        }
        let other = export(Arc::new(Callback(mpsc::channel().0)));
        // Register the successor against an empty native owner, then retain real task history.
        register(host, &other).unwrap();
        state(host).update(&[(9, true, false, before)], true);
        empty(register(host, &organizer).unwrap());
        unregister(host, &organizer);
        assert_eq!(state(host).entries.last().unwrap().tasks.len(), 1);
        unregister(host, &other);
    });
}

#[test]
fn app_task_removal_reports_vanished_once() {
    use aim_service_aidl::android_app_iapptask as iat;
    crate::system::with_discovery_binders(|host, _, _, export| {
        let (tx, events) = mpsc::channel();
        let organizer = export(Arc::new(Callback(tx)));
        register(host, &organizer).unwrap();
        let surface = export(Arc::new(super::super::activity::Token));
        let layer = Layer::animation_test_layer(host.process.strong(surface.handle), 71);
        let leash = Leash::new(host, surface.binder(), &layer);
        let mut info = super::super::task_info::tests::organizer_fixture();
        info.task_id = 7;
        let mut p = Parcel::new();
        info.write_to(&mut p);
        // Seed the original appeared snapshot without a platform/resource boot.
        state(host).update(&[(7, true, false, Appeared { info: p, leash })], true);
        super::super::task_info::tests::add_app_task(host, 7);
        host.tasks.animation_test_task(7, Default::default());
        let mut request = Parcel::new();
        iat::FinishAndRemoveTask {}.write(&mut request);
        let result = super::super::task_info::tests::invoke_app_task(
            host,
            7,
            10100,
            iat::FINISH_AND_REMOVE_TASK,
            &request,
        );
        result.reader().read_exception().unwrap().unwrap();
        assert!(host.tasks.bounds(7).is_none());
        let (code, payload) = events.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(code, callback::ON_TASK_VANISHED);
        let mut r = payload.reader();
        assert_eq!(r.read_i32().unwrap(), 1); // typed TaskInfo
        assert_eq!(
            (r.read_i32().unwrap(), r.read_i32().unwrap()),
            (7, info.user_id)
        );
        let result = super::super::task_info::tests::invoke_app_task(
            host,
            7,
            10100,
            iat::FINISH_AND_REMOVE_TASK,
            &request,
        );
        assert_eq!(
            result.reader().read_exception().unwrap().unwrap_err().code,
            aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT
        );
        assert!(state(host).entries[0].tasks.is_empty());
        assert!(events.try_recv().is_err());
    });
}

fn create_root(host: &Arc<AppHost>, display_id: i32, mode: i32, cookie: Option<Binder>) -> Parcel {
    let mut request = Parcel::new();
    toc::CreateRootTask {
        display_id,
        windowing_mode: mode,
        launch_cookie: cookie,
        remove_with_task_organizer: false,
    }
    .write(&mut request);
    invoke(
        &Controller(host.clone(), true),
        toc::CREATE_ROOT_TASK,
        &request,
        1000,
    )
}
fn delete_root(host: &Arc<AppHost>, token: Binder) -> Result<bool, i32> {
    let mut request = Parcel::new();
    request.write_interface_token(toc::DESCRIPTOR);
    request.write_i32(1);
    request.write_binder(Some(token));
    let reply = invoke(
        &Controller(host.clone(), true),
        toc::DELETE_ROOT_TASK,
        &request,
        1000,
    );
    let mut r = reply.reader();
    match r.read_exception().unwrap() {
        Ok(()) => Ok(r.read_bool().unwrap()),
        Err(e) => Err(e.code),
    }
}
/// `getRootTasks`: how many, and the first one's id.
fn root_tasks(
    host: &Arc<AppHost>,
    display_id: i32,
    types: Option<Vec<i32>>,
) -> Result<(i32, i32), i32> {
    let mut request = Parcel::new();
    toc::GetRootTasks {
        display_id,
        activity_types: types,
    }
    .write(&mut request);
    let reply = invoke(
        &Controller(host.clone(), true),
        toc::GET_ROOT_TASKS,
        &request,
        1000,
    );
    let mut r = reply.reader();
    r.read_exception().unwrap().map_err(|e| e.code)?;
    let count = r.read_i32().unwrap();
    assert_eq!(r.read_i32().unwrap(), 1);
    Ok((count, r.read_i32().unwrap()))
}
fn next_event(events: &mpsc::Receiver<(u32, Parcel)>) -> (u32, i32) {
    let (code, payload) = events.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut r = payload.reader();
    assert_eq!(r.read_i32().unwrap(), 1);
    (code, r.read_i32().unwrap())
}
#[test]
fn organizer_root_tasks_are_created_listed_deleted_and_go_with_their_organizer() {
    use aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT;
    crate::system::with_discovery_binders(|host, _, _, export| {
        let old = super::super::config::Rect {
            left: 100,
            top: 100,
            right: 900,
            bottom: 1500,
        };
        let (_thread, _activities, _bridge, tree) =
            super::super::tasks::tests::wct_tasks(host, export, old);
        for task in &mut host.activities.stack.lock().unwrap().tasks {
            task.intent.component = Some(crate::package::intent::ComponentName {
                package: "p".into(),
                class: "p.Main".into(),
            });
        }
        let (tx, events) = mpsc::channel();
        let organizer = export(Arc::new(Callback(tx)));
        let cookies: Vec<_> = (0..2)
            .map(|_| export(Arc::new(super::super::activity::Token)))
            .collect();
        // Task.Builder's windowing mode validation, before anything is made.
        denied(create_root(host, 0, 1, None), EX_UNSUPPORTED_OPERATION);
        denied(create_root(host, 0, 3, None), EX_ILLEGAL_ARGUMENT);
        // An unknown display is logged and creates nothing.
        create_root(host, 1, 6, None)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        assert_eq!(host.tasks.root_task_ids(), [8, 7]);
        create_root(host, 0, 6, Some(cookies[0].binder()))
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        let first = *host.tasks.root_task_ids().last().unwrap();
        // Its real container layer is the display's child.
        assert_eq!(tree.lock().unwrap().parents[&1], Some(0));
        let info = host.organizer_task_info(first).unwrap();
        let display = host.display().unwrap();
        assert_eq!(info.launch_cookie, Some(cookies[0].binder()));
        assert_eq!(info.configuration.window.windowing_mode, 6);
        assert_eq!(
            (
                info.configuration.window.bounds.right,
                info.configuration.window.bounds.bottom
            ),
            (display.0.width, display.0.height)
        );
        assert_eq!(
            (
                info.is_running,
                info.num_activities,
                info.activity_type,
                info.parent_task_id
            ),
            (false, 0, 0, -1)
        );
        // A root task appears by callback, not in the registration's list.
        empty(register(host, &organizer).unwrap());
        assert_eq!(next_event(&events), (callback::ON_TASK_APPEARED, first));
        create_root(host, 0, 6, Some(cookies[1].binder()))
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        let second = *host.tasks.root_task_ids().last().unwrap();
        assert_eq!(next_event(&events), (callback::ON_TASK_APPEARED, second));
        // Top to bottom: the app tasks, then the roots, each made at the bottom.
        assert_eq!(host.tasks.root_task_ids(), [8, 7, first, second]);
        assert_eq!(root_tasks(host, 0, None), Ok((4, 8)));
        assert_eq!(root_tasks(host, 0, Some(vec![0])), Ok((2, first)));
        assert_eq!(root_tasks(host, 1, None), Err(EX_ILLEGAL_ARGUMENT));
        // A root task stays in its windowing mode.
        let mut fullscreen =
            super::super::window_container_transaction::tests::fixture(host.task_token(first));
        fullscreen.set_i32_at(352, 1);
        denied(apply_wct(host, &fullscreen, 1000), EX_UNSUPPORTED_OPERATION);
        assert_eq!(
            delete_root(host, host.task_token(7)),
            Err(EX_ILLEGAL_ARGUMENT)
        );
        assert_eq!(delete_root(host, cookies[0].binder()), Ok(false));
        assert_eq!(delete_root(host, host.task_token(first)), Ok(true));
        assert_eq!(next_event(&events), (callback::ON_TASK_VANISHED, first));
        assert!(host.tasks.root(first).is_none());
        assert_eq!(tree.lock().unwrap().removed, [1]);
        // TaskOrganizerState.dispose removes the organizer's root tasks without
        // callbacks: migrated off the departing leash (removeTask), then removed.
        unregister(host, &organizer);
        assert!(host.tasks.root(second).is_none());
        assert_eq!(tree.lock().unwrap().removed, [1, 2, 3]);
        assert_eq!(host.tasks.root_task_ids(), [8, 7]);
        assert!(events.try_recv().is_err());
    });
}

#[test]
fn get_root_tasks_reports_the_root_home_task_by_its_home_type() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let old = super::super::config::Rect {
            left: 100,
            top: 100,
            right: 900,
            bottom: 1500,
        };
        let (_thread, _activities, _bridge, _tree) =
            super::super::tasks::tests::wct_tasks(host, export, old);
        let (home, _) = super::super::tasks::tests::home_task(host);
        for task in &mut host.activities.stack.lock().unwrap().tasks {
            task.intent.component = Some(crate::package::intent::ComponentName {
                package: "p".into(),
                class: "p.Main".into(),
            });
        }
        create_root(host, 0, 6, None)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        let root = *host.tasks.root_task_ids().last().unwrap();
        // Top to bottom: the app tasks, the root home task, the organizer's.
        assert_eq!(host.tasks.root_task_ids(), [8, 7, home, root]);
        assert_eq!(root_tasks(host, 0, None), Ok((4, 8)));
        assert_eq!(root_tasks(host, 0, Some(vec![2])), Ok((1, home)));
        assert_eq!(root_tasks(host, 0, Some(vec![0])), Ok((1, root)));
    });
}

/// `getChildTasks`: how many (-1 for null), and the first one's id.
fn child_tasks(host: &Arc<AppHost>, parent: Binder, types: Option<Vec<i32>>) -> (i32, i32) {
    let mut request = Parcel::new();
    request.write_interface_token(toc::DESCRIPTOR);
    request.write_i32(1);
    request.write_binder(Some(parent));
    match types {
        Some(t) => {
            request.write_i32(t.len() as i32);
            t.iter().for_each(|&v| request.write_i32(v));
        }
        None => request.write_i32(-1),
    }
    let reply = invoke(
        &Controller(host.clone(), true),
        toc::GET_CHILD_TASKS,
        &request,
        1000,
    );
    let mut r = reply.reader();
    r.read_exception().unwrap().unwrap();
    let count = r.read_i32().unwrap();
    if count <= 0 {
        return (count, -1);
    }
    assert_eq!(r.read_i32().unwrap(), 1);
    (count, r.read_i32().unwrap())
}
#[test]
fn organizer_root_tasks_hold_reparented_tasks_in_their_bounds() {
    use super::super::config::Rect;
    use super::super::window_container_transaction::tests::{Op, reparent, transaction};
    use aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT;
    crate::system::with_discovery_binders(|host, _, _, export| {
        let old = Rect {
            left: 100,
            top: 100,
            right: 900,
            bottom: 1500,
        };
        let (_thread, activities, _bridge, tree) =
            super::super::tasks::tests::wct_tasks(host, export, old);
        // Task 8's activity is resizeable (RESIZE_MODE_RESIZEABLE).
        host.activities
            .records
            .lock()
            .unwrap()
            .get_mut(&activities[1])
            .unwrap()
            .info
            .resize_mode = 2;
        for task in &mut host.activities.stack.lock().unwrap().tasks {
            task.intent.component = Some(crate::package::intent::ComponentName {
                package: "p".into(),
                class: "p.Main".into(),
            });
        }
        // Task 8's layer is surface 1, under the display's (0).
        let layer = host.task_layer(8, |l| Ok(l.id)).unwrap();
        let (tx, events) = mpsc::channel();
        let organizer = export(Arc::new(Callback(tx)));
        register(host, &organizer).unwrap();
        create_root(host, 0, 6, None)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        let root = *host.tasks.root_task_ids().last().unwrap();
        let (task, token) = (host.task_token(8), host.task_token(root));
        let mode = |id| host.organizer_task_info(id).unwrap().configuration.window;
        let half = Rect {
            left: 0,
            top: 0,
            right: 540,
            bottom: 1920,
        };
        let bounds = |b: Rect| [(244, b.left), (248, b.top), (252, b.right), (256, b.bottom)];
        let inherit = [(244, 0), (248, 0), (252, 0), (256, 0), (352, 0)];
        // TestTaskOrganizer.putTaskInSplitPrimary. Task 7's unresizeable
        // activity has no multi-window (`supportsMultiWindowInDisplayArea`).
        let split = transaction(
            &[(token, &bounds(half)), (task, &inherit)],
            &[
                reparent(host.task_token(7), Some(token), true),
                reparent(task, Some(token), true),
                Op {
                    kind: 1,
                    reparent: Some(token),
                    ..reparent(token, None, true)
                },
            ],
        );
        apply_wct(host, &split, 1000)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        let info = host.organizer_task_info(8).unwrap();
        assert_eq!(info.parent_task_id, root);
        assert_eq!((mode(8).windowing_mode, mode(8).bounds), (6, half));
        assert_eq!(mode(root).bounds, half);
        assert_eq!(host.organizer_task_info(7).unwrap().parent_task_id, -1);
        assert_eq!(host.tasks.root_task_ids(), [root, 7]);
        assert_eq!(child_tasks(host, token, None), (1, 8));
        assert_eq!(child_tasks(host, token, Some(vec![2])), (0, -1));
        assert_eq!(child_tasks(host, task, None), (-1, -1));
        let surface = tree.lock().unwrap().parents.clone();
        assert_eq!(surface[&layer], surface.keys().last().copied());
        // The task follows its root task's bounds, at its place in it.
        let right = Rect {
            left: 540,
            top: 0,
            right: 1080,
            bottom: 1920,
        };
        apply_wct(host, &transaction(&[(token, &bounds(right))], &[]), 1000)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        assert_eq!(host.tasks.bounds(8), Some(right));
        assert_eq!(
            host.organizer_task_info(8).unwrap().position_in_parent,
            (0, 0)
        );
        // Nesting in an app task, nesting root tasks, multi-window on the
        // display area and reparentTasks of no parent are refused unapplied.
        for (op, change, code) in [
            (
                reparent(task, Some(host.task_token(7)), true),
                None,
                EX_UNSUPPORTED_OPERATION,
            ),
            (reparent(token, None, true), None, EX_UNSUPPORTED_OPERATION),
            (
                reparent(task, None, true),
                Some((host.task_token(7), [(340, 0), (344, 0), (352, 6)])),
                EX_UNSUPPORTED_OPERATION,
            ),
            (
                Op {
                    kind: 2,
                    container: None,
                    ..reparent(task, None, true)
                },
                None,
                EX_ILLEGAL_ARGUMENT,
            ),
        ] {
            let changes: Vec<_> = change.iter().map(|(t, e)| (*t, e.as_slice())).collect();
            denied(apply_wct(host, &transaction(&changes, &[op]), 1000), code);
            assert_eq!(host.tasks.parent(8), Some(root));
        }
        // dismissSplitScreen: reparentTasks of its modes and types onto the
        // display area, where the task takes its last freeform bounds back.
        let dismiss = Op {
            kind: 2,
            container: Some(token),
            containers: None,
            reparent: None,
            to_top: true,
            top_only: false,
            modes: Some(vec![1, 6, 0]),
            types: Some(vec![1, 2, 3, 0]),
        };
        apply_wct(host, &transaction(&[], &[dismiss]), 1000)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
        assert_eq!(host.tasks.parent(8), None);
        assert_eq!((mode(8).windowing_mode, mode(8).bounds), (5, old));
        assert_eq!(tree.lock().unwrap().parents[&layer], Some(0));
        assert_eq!(child_tasks(host, token, None), (0, -1));
        // Deleting a root task takes its tasks' activities with it.
        apply_wct(
            host,
            &transaction(&[], &[reparent(task, Some(token), false)]),
            1000,
        )
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();
        assert_eq!(host.tasks.parent(8), Some(root));
        assert_eq!(delete_root(host, token), Ok(true));
        assert_eq!(host.tasks.parent(8), None);
        assert!(
            host.activities
                .stack
                .lock()
                .unwrap()
                .activities
                .iter()
                .filter(|a| a.task == 8)
                .all(|a| a.finishing)
        );
        unregister(host, &organizer);
        drop(events);
    });
}
