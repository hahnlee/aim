//! Android 16 ActivityClientController/KeyguardController/PhoneWindowManager;
//! AccessibilityTestActivity.turnOnScreen and KeyguardManager.requestDismissKeyguard.
use super::*;
use aim_binder_host::parcel::{EX_NULL_POINTER, EX_SECURITY};
use aim_service_aidl::com_android_internal_policy_ikeyguarddismisscallback as cb;

struct Callback(Arc<Mutex<Vec<u32>>>);
impl Service for Callback {
    fn descriptor(&self) -> &str {
        cb::DESCRIPTOR
    }
    fn transact(&self, c: &mut Call<'_>) -> Reply {
        assert_eq!(c.sender_euid, 1000);
        assert_ne!(c.flags & 1, 0);
        cb::OnDismissError::read(&mut c.data)?;
        self.0.lock().unwrap().push(c.code);
        Ok(Parcel::new())
    }
}
fn invoke(service: &impl Service, code: u32, p: &Parcel, uid: u32) -> Reply {
    service.transact(&mut Call {
        code,
        flags: 0,
        sender_pid: 71,
        sender_euid: uid,
        data: p.reader(),
    })
}
fn dismiss(
    host: &Arc<AppHost>,
    token: Option<Binder>,
    callback: Option<Binder>,
    message: Option<Option<&str>>,
    uid: u32,
) -> Result<(), Exception> {
    let mut p = Parcel::new();
    p.write_interface_token(acc::DESCRIPTOR);
    p.write_binder(token);
    p.write_binder(callback);
    p.write_i32(i32::from(message.is_some()));
    if let Some(text) = message {
        crate::clip::write_char_sequence(&mut p, text);
    }
    let service = ActivityClientController(host.clone());
    acc::read_dismiss_keyguard_reply(
        &mut invoke(&service, acc::DISMISS_KEYGUARD, &p, uid)
            .unwrap()
            .reader(),
    )
    .unwrap()
}

fn flags(host: &AppHost, t: Option<Binder>) -> Option<(bool, bool, bool)> {
    host.activities.with(t, |r| {
        (
            r.show_when_locked,
            r.inherit_show_when_locked,
            r.turn_screen_on,
        )
    })
}
fn activity(host: &Arc<AppHost>) -> u64 {
    use super::super::{activity_stack::Activity, activity_start::StartRequest};
    let mut info = host.activity_info_of("p", "p.Main").unwrap();
    info.flags |= 0x800000 | 0x1000000;
    info.private_flags |= 1;
    let request = StartRequest::launcher("p.Main");
    let t = host.new_activity(1, 10100, "p", info, Default::default(), 7, request);
    host.activities
        .stack
        .lock()
        .unwrap()
        .activities
        .push(Activity::new(t, 7, 1, "p/p.Main", 0, 0, None));
    t
}

#[test]
fn unlocked_dismissal_checks_capability_visibility_and_message_permission() {
    crate::system::with_discovery_binders(|host, _, _, _| {
        let permissions = super::super::permission_manager::PermissionManager::with_system(
            host.clone(),
            Default::default(),
        );
        assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
        let t = activity(host);
        let token = Some(Binder::Local(t));
        assert_eq!(flags(host, token), Some((true, true, true)));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let callback = Some(host.process.add_service(Arc::new(Callback(calls.clone()))));
        // No ownership restriction: the token is the capability, even for an isolated UID.
        for uid in [10100, 10101, 99000, 1000] {
            dismiss(host, token, callback, None, uid).unwrap();
        }
        assert_eq!(*calls.lock().unwrap(), vec![cb::ON_DISMISS_ERROR; 4]);
        // Null and unknown/remote tokens fail through the callback, never succeed/cancel.
        for bad in [
            None,
            Some(Binder::Local(u64::MAX)),
            Some(Binder::Handle(99)),
        ] {
            dismiss(host, bad, callback, None, 10100).unwrap();
            assert_eq!(
                dismiss(host, bad, None, None, 10100).unwrap_err().code,
                EX_NULL_POINTER
            );
        }
        dismiss(host, token, None, None, 10100).unwrap();
        let before = calls.lock().unwrap().len();
        for text in ["", "Unlock"] {
            for t in [token, None] {
                let error = dismiss(host, t, callback, Some(Some(text)), 10100).unwrap_err();
                assert_eq!(error.code, EX_SECURITY);
            }
        }
        assert_eq!(calls.lock().unwrap().len(), before);
        dismiss(host, token, callback, Some(Some("Unlock")), 1000).unwrap();
        // TextUtils can decode a null value inside a present typed object.
        dismiss(host, token, callback, Some(None), 10100).unwrap();
        {
            let mut stack = host.activities.stack.lock().unwrap();
            stack.get_mut(t).unwrap().finishing = true;
        }
        dismiss(host, token, callback, None, 10100).unwrap();
        assert_eq!(
            dismiss(host, token, None, None, 10100).unwrap_err().code,
            EX_NULL_POINTER
        );
        let observed = calls.lock().unwrap().clone();
        assert_eq!(observed.len(), 10);
        assert!(observed.iter().all(|&code| code == cb::ON_DISMISS_ERROR));
        // Binder delivery failure is logged as the original catches RemoteException.
        dismiss(host, token, Some(Binder::Handle(u32::MAX)), None, 10100).unwrap();
        let service = ActivityClientController(host.clone());
        for enabled in [true, false] {
            for token in [Some(Binder::Local(t)), None, Some(Binder::Local(u64::MAX))] {
                for code in [
                    acc::SET_SHOW_WHEN_LOCKED,
                    acc::SET_INHERIT_SHOW_WHEN_LOCKED,
                    acc::SET_TURN_SCREEN_ON,
                ] {
                    let mut p = Parcel::new();
                    p.write_interface_token(acc::DESCRIPTOR);
                    p.write_binder(token);
                    p.write_bool(enabled);
                    let result = invoke(&service, code, &p, 10101).unwrap();
                    if code == acc::SET_TURN_SCREEN_ON {
                        result.reader().read_exception().unwrap().unwrap();
                    } else {
                        assert!(result.data().is_empty());
                    }
                }
            }
            assert_eq!(
                flags(host, Some(Binder::Local(t))),
                Some((enabled, enabled, enabled))
            );
        }
        use aim_service_aidl::android_view_iwindowmanager as wm;
        let mut p = Parcel::new();
        wm::IsKeyguardLocked {}.write(&mut p);
        let wm = super::super::window_manager::WindowManager(host.clone());
        let reply = invoke(&wm, wm::IS_KEYGUARD_LOCKED, &p, 10100).unwrap();
        assert!(
            !wm::read_is_keyguard_locked_reply(&mut reply.reader())
                .unwrap()
                .unwrap()
        );
    });
}
