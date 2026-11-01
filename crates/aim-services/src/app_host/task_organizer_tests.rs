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
fn unregister(host: &AppHost, who: &Strong) {
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
            invoke(&task, toc::CREATE_ROOT_TASK, &Parcel::new(), 1000),
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
            state(host).update(&[(9, true, before.clone())], true).len(),
            1
        );
        state(host).update(&[(9, true, before.clone())], false);
        host.dispatch_organizer_reports();
        assert!(events.try_recv().is_err()); // no duplicate initial or unchanged callback
        info.is_focused = false;
        let after = snapshot(&info);
        for current in [
            vec![(9, true, after.clone())],
            vec![],
            vec![(9, false, before.clone())],
            vec![(9, true, before.clone())],
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
        state(host).update(&[(9, true, before)], true);
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
        state(host).update(&[(7, true, Appeared { info: p, leash })], true);
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
