//! Isolated driver contract tests. The retained owner is a boundary double;
//! actual original permission/listener execution is the 6f boot gate.
use super::*;
use aim_binder_driver::{Credentials, Device, Driver, Errno, File, GuestProcess, errno, uapi::*};
use aim_binder_host::{local::Service, parcel::*};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
    mpsc,
};
use std::time::Duration;

struct NoMemory;
impl GuestProcess for NoMemory {
    fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> std::result::Result<(), Errno> {
        Err(errno::EFAULT)
    }
    fn copy_to_user(&mut self, _: u64, _: &[u8]) -> std::result::Result<(), Errno> {
        Err(errno::EFAULT)
    }
    fn get_file(&mut self, _: u32) -> std::result::Result<File, Errno> {
        Err(errno::EBADF)
    }
    fn install_file(&mut self, _: File) -> std::result::Result<u32, Errno> {
        Err(errno::EBADF)
    }
    fn close_fd(&mut self, _: u32) {
        panic!("no fixture files")
    }
}
type Handler = dyn Fn(&mut Call<'_>) -> Reply + Send + Sync;
struct Endpoint(Box<Handler>);
impl Service for Endpoint {
    fn descriptor(&self) -> &str {
        cs::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        self.0(call)
    }
}
const INSTALL: u32 = 0xffff_fffe;
const LOOKUP: u32 = 0xffff_fffd;
struct Gateway {
    process: Arc<LocalProcess>,
    owner: Mutex<Option<Strong>>,
    forge: AtomicU32,
}
impl Service for Gateway {
    fn descriptor(&self) -> &str {
        cs::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        if call.code == INSTALL {
            assert_eq!(call.sender_euid, 1000);
            let Some(Binder::Handle(h)) = call.data.read_binder()? else {
                return Err(BAD_VALUE);
            };
            *self.owner.lock().unwrap() = Some(self.process.strong(h));
            return Ok(Parcel::new());
        }
        if call.code == LOOKUP {
            assert_eq!((call.sender_pid, call.sender_euid), (702, 1000));
            let mut p = Parcel::new();
            p.write_string16(Some("visible.adapter"));
            return Ok(p);
        }
        let mut forged = Call {
            code: call.code,
            flags: call.flags,
            sender_pid: call.sender_pid,
            sender_euid: call.sender_euid,
            data: call.data.clone(),
        };
        let call = &mut forged;
        match self.forge.load(Ordering::SeqCst) {
            1 => call.sender_euid = 1000,
            2 => call.sender_pid += 1,
            3 => call.flags |= TF_ONE_WAY,
            4 => call.data.set_position(4),
            _ => {}
        }
        let owner = self
            .process
            .strong(self.owner.lock().unwrap().as_ref().unwrap().handle);
        forward(&self.process, &owner, call)
    }
}
struct Fixture {
    gateway: Arc<Gateway>,
    native: Arc<LocalProcess>,
    original: Arc<LocalProcess>,
    apps: Vec<Arc<LocalProcess>>,
}
impl Fixture {
    fn new(owner: impl FnOnce(Arc<LocalProcess>) -> Arc<dyn Service>) -> Self {
        let driver = Driver::new();
        let open = |pid, euid| {
            LocalProcess::open(
                &driver,
                Device::Binder,
                Credentials {
                    pid,
                    euid,
                    security_context: None,
                },
            )
        };
        let native = open(701, 1000);
        let original = open(702, 1000);
        let apps = vec![
            open(703, 10100),
            open(704, 10101),
            open(705, 10102),
            open(706, 2000),
        ];
        let gateway = Arc::new(Gateway {
            process: native.clone(),
            owner: Mutex::new(None),
            forge: AtomicU32::new(0),
        });
        let Binder::Local(ptr) = native.add_service(gateway.clone()) else {
            unreachable!()
        };
        let mut object = FlatBinderObject {
            kind: BINDER_TYPE_BINDER,
            flags: 0,
            binder: ptr,
            cookie: ptr,
        }
        .encode();
        driver
            .ioctl(
                native.proc_handle(),
                700,
                BINDER_SET_CONTEXT_MGR_EXT,
                &mut object,
                &mut NoMemory,
            )
            .unwrap();
        native.start();
        original.start();
        for app in &apps {
            app.start();
        }
        let mut p = Parcel::new();
        p.write_binder(Some(original.add_service(owner(original.clone()))));
        original.transact(0, INSTALL, &p, false).unwrap();
        Self {
            gateway,
            native,
            original,
            apps,
        }
    }
    fn call(&self, app: usize, code: u32, p: &Parcel) -> Reply {
        self.apps[app]
            .transact(0, code, p, false)
            .map(|r| r.into_parcel())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for p in self.apps.iter().chain([&self.original, &self.native]) {
            p.shutdown().unwrap();
        }
    }
}
fn wire(reader: &Reader<'_>) -> Parcel {
    let mut r = reader.clone();
    r.set_position(r.position() + r.remaining());
    let (bytes, objects) = r.since(0);
    let mut p = Parcel::new();
    p.write_raw(bytes, &objects);
    p
}
fn endpoint(f: impl Fn(&mut Call<'_>) -> Reply + Send + Sync + 'static) -> Arc<dyn Service> {
    Arc::new(Endpoint(Box::new(f)))
}

#[test]
fn every_generated_sync_code_preserves_wire_objects_and_app_identity() {
    let (tx, rx) = mpsc::channel();
    let f = Fixture::new(|process| {
        endpoint(move |call| {
            tx.send((
                call.code,
                call.sender_pid,
                call.sender_euid,
                wire(&call.data).objects().to_vec(),
            ))
            .unwrap();
            let mut r = call.data.clone();
            r.enforce_interface(cs::DESCRIPTOR)?;
            r.read_i32()?;
            let Some(Binder::Handle(h)) = r.read_binder()? else {
                return Err(BAD_VALUE);
            };
            let mut p = wire(&call.data);
            p.keep_alive(Arc::new(process.strong(h)));
            Ok(p)
        })
    });
    let excluded = [
        cs::UNREGISTER_CONTENT_OBSERVER,
        cs::REGISTER_CONTENT_OBSERVER,
        cs::NOTIFY_CHANGE,
        cs::PUT_CACHE,
        cs::GET_CACHE,
        cs::ON_DB_CORRUPTION,
    ];
    assert_eq!(SYNC_CODES.len(), 35);
    let expected: Vec<_> = cs::METHODS
        .iter()
        .map(|m| m.0)
        .filter(|c| !excluded.contains(c))
        .collect();
    assert_eq!(SYNC_CODES, expected);
    let binder = f.apps[0].add_service(endpoint(|_| Ok(Parcel::new())));
    for &(code, _) in cs::METHODS {
        let mut p = Parcel::new();
        p.write_interface_token(cs::DESCRIPTOR);
        p.write_i32(0x1234);
        p.write_binder(Some(binder));
        p.write_string16(Some("wire tail"));
        if excluded.contains(&code) {
            assert_eq!(f.call(0, code, &p).unwrap_err(), UNKNOWN_TRANSACTION);
        } else {
            let answer = f.call(0, code, &p).unwrap();
            assert_eq!(answer.data(), p.data(), "code {code}");
            assert_eq!(answer.objects(), p.objects());
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                (code, 703, 10100, p.objects().to_vec())
            );
        }
    }
    assert!(rx.try_recv().is_err());
}

#[test]
fn synthetic_forged_oneway_and_consumed_calls_are_rejected() {
    let f = Fixture::new(|_| endpoint(|_| panic!("must not reach owner")));
    let mut p = Parcel::new();
    cs::GetCurrentSyncs {}.write(&mut p);
    for uid in [1000, 10100] {
        let owner = f.gateway.owner.lock().unwrap();
        assert_eq!(
            forward(
                &f.native,
                owner.as_ref().unwrap(),
                &Call {
                    code: cs::GET_CURRENT_SYNCS,
                    flags: 0,
                    sender_pid: 703,
                    sender_euid: uid,
                    data: p.reader()
                }
            )
            .unwrap_err(),
            PERMISSION_DENIED
        );
    }
    for (mode, error) in [
        (1, PERMISSION_DENIED),
        (2, PERMISSION_DENIED),
        (3, BAD_VALUE),
        (4, BAD_VALUE),
    ] {
        f.gateway.forge.store(mode, Ordering::SeqCst);
        assert_eq!(f.call(0, cs::GET_CURRENT_SYNCS, &p).unwrap_err(), error);
    }
    assert_eq!(f.call(0, 0xffff, &p).unwrap_err(), UNKNOWN_TRANSACTION);
}

#[test]
fn malformed_null_asuser_exceptions_and_transport_errors_belong_to_owner() {
    let f = Fixture::new(|_| {
        endpoint(|call| {
            let a =
                cs::SyncAsUser::<super::super::content_service::SyncRequest>::read(&mut call.data)?;
            assert_eq!(a.calling_package.as_deref(), Some("app.package"));
            let mut p = Parcel::new();
            p.write_exception(&Exception::new(
                if a.user_id != 0 {
                    EX_SECURITY
                } else {
                    EX_NULL_POINTER
                },
                format!("owner user={} null={}", a.user_id, a.request.is_none()),
            ));
            Ok(p)
        })
    });
    for user in [0, 10, -1, -2, -10000] {
        let mut p = Parcel::new();
        p.write_interface_token(cs::DESCRIPTOR);
        p.write_i32(0);
        p.write_i32(user);
        p.write_string16(Some("app.package"));
        let answer = f.call(0, cs::SYNC_AS_USER, &p).unwrap();
        let e = answer.reader().read_exception().unwrap().unwrap_err();
        assert_eq!(
            e.code,
            if user == 0 {
                EX_NULL_POINTER
            } else {
                EX_SECURITY
            }
        );
        assert_eq!(e.message, format!("owner user={user} null=true"));
        for len in [0, 4, p.data().len() - 4] {
            let mut bad = Parcel::new();
            bad.write_raw(&p.data()[..len], &[]);
            assert!(f.call(0, cs::SYNC_AS_USER, &bad).is_err());
        }
    }
    f.original.shutdown().unwrap();
    assert_eq!(
        f.call(0, cs::SYNC_AS_USER, &Parcel::new()).unwrap_err(),
        DEAD_OBJECT
    );
}

#[test]
fn owner_permissions_redaction_and_nested_package_lookup_keep_identity() {
    let (tx, rx) = mpsc::channel();
    let f = Fixture::new(|process| {
        endpoint(move |call| {
            if call.code == cs::RESET_TODAY_STATS {
                call.data.enforce_interface(cs::DESCRIPTOR)?;
                let mut p = Parcel::new();
                if matches!(call.sender_euid, 0 | 2000) {
                    p.write_no_exception();
                } else {
                    p.write_exception(&Exception::security("Non-shell resetTodayStats"));
                }
                return Ok(p);
            }
            if call.code == cs::GET_SYNC_ADAPTER_PACKAGES_FOR_AUTHORITY_AS_USER {
                assert_eq!((call.sender_pid, call.sender_euid), (702, 1000));
                let a = cs::GetSyncAdapterPackagesForAuthorityAsUser::read(&mut call.data)?;
                assert_eq!(a.authority.as_deref(), Some("visible"));
                assert_eq!(a.user_id, 0);
                return process
                    .transact(0, LOOKUP, &Parcel::new(), false)
                    .map(|r| r.into_parcel());
            }
            cs::GetCurrentSyncsAsUser::read(&mut call.data)?;
            let uid = call.sender_euid;
            let mut p = Parcel::new();
            if uid == 10102 {
                p.write_exception(&Exception::security("READ_SYNC_STATS"));
            } else {
                let mut nested = Parcel::new();
                cs::GetSyncAdapterPackagesForAuthorityAsUser {
                    authority: Some("visible".into()),
                    user_id: 0,
                }
                .write(&mut nested);
                let answer = process.transact(
                    0,
                    cs::GET_SYNC_ADAPTER_PACKAGES_FOR_AUTHORITY_AS_USER,
                    &nested,
                    false,
                )?;
                assert_eq!(
                    answer.reader().read_string16()?.as_deref(),
                    Some("visible.adapter")
                );
                // The cleared nested identity must not replace the outer app identity.
                assert_eq!(
                    process.authenticated_inbound_identity(),
                    Some((call.sender_pid, uid))
                );
                tx.send(uid).unwrap();
                p.write_no_exception();
                p.write_i32(1); // The owner's visible-authority result only.
                p.write_string16((uid == 10101).then_some("account.name"));
                p.write_string16(Some("visible"));
            }
            Ok(p)
        })
    });
    let mut p = Parcel::new();
    cs::GetCurrentSyncsAsUser { user_id: 0 }.write(&mut p);
    for app in 0..3 {
        let answer = f.call(app, cs::GET_CURRENT_SYNCS_AS_USER, &p).unwrap();
        let mut r = answer.reader();
        if app == 2 {
            assert_eq!(
                r.read_exception().unwrap().unwrap_err(),
                Exception::security("READ_SYNC_STATS")
            );
        } else {
            r.read_exception().unwrap().unwrap();
            assert_eq!(r.read_i32().unwrap(), 1);
            assert_eq!(
                r.read_string16().unwrap().as_deref(),
                (app == 1).then_some("account.name")
            );
            assert_eq!(r.read_string16().unwrap().as_deref(), Some("visible"));
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                10100 + app as u32
            );
        }
        assert_eq!(r.remaining(), 0);
    }
    let mut p = Parcel::new();
    p.write_interface_token(cs::DESCRIPTOR);
    for app in [0, 3] {
        let answer = f.call(app, cs::RESET_TODAY_STATS, &p).unwrap();
        assert_eq!(answer.reader().read_exception().unwrap().is_ok(), app == 3);
    }
    let answer = f
        .original
        .transact(0, cs::RESET_TODAY_STATS, &p, false)
        .unwrap();
    assert_eq!(
        answer.reader().read_exception().unwrap().unwrap_err().code,
        EX_SECURITY
    );
}

#[test]
fn status_masks_visibility_removal_and_callback_death_stay_with_owner() {
    use aim_service_aidl::android_content_isyncstatusobserver as observer;
    type Listeners = Arc<Mutex<Vec<(i32, u32, Strong)>>>;
    let listeners: Listeners = Arc::default();
    let (died, deaths) = mpsc::channel();
    let retained = listeners.clone();
    let f = Fixture::new(|process| {
        endpoint(move |call| {
            let mut p = Parcel::new();
            match call.code {
                cs::ADD_STATUS_CHANGE_LISTENER => {
                    let a = cs::AddStatusChangeListener::read(&mut call.data)?;
                    if let Some(Binder::Handle(h)) = a.callback {
                        let strong = process.strong(h);
                        let list = retained.clone();
                        let died = died.clone();
                        process.link_to_death(
                            &strong,
                            Box::new(move || {
                                list.lock().unwrap().retain(|v| v.2.handle != h);
                                died.send(()).unwrap();
                            }),
                        );
                        retained
                            .lock()
                            .unwrap()
                            .push((a.mask, call.sender_euid, strong));
                    }
                }
                cs::REMOVE_STATUS_CHANGE_LISTENER => {
                    let a = cs::RemoveStatusChangeListener::read(&mut call.data)?;
                    if let Some(Binder::Handle(h)) = a.callback {
                        retained.lock().unwrap().retain(|v| v.2.handle != h);
                    }
                }
                cs::SET_MASTER_SYNC_AUTOMATICALLY => {
                    cs::SetMasterSyncAutomatically::read(&mut call.data)?;
                    for (mask, uid, callback) in retained.lock().unwrap().iter() {
                        // Boundary policy double: SETTINGS event, one hidden UID.
                        if mask & 1 != 0 && *uid != 10102 {
                            let mut event = Parcel::new();
                            observer::OnStatusChanged { which: 1 }.write(&mut event);
                            callback.transact(observer::ON_STATUS_CHANGED, &event, true)?;
                        }
                    }
                }
                _ => return Err(UNKNOWN_TRANSACTION),
            }
            p.write_no_exception();
            Ok(p)
        })
    });
    let (tx, rx) = mpsc::channel();
    let mut callbacks = Vec::new();
    for (app, mask) in [1, 2, 1].into_iter().enumerate() {
        let tx = tx.clone();
        let binder = f.apps[app].add_service(endpoint(move |call| {
            assert!(call.is_oneway());
            assert_eq!((call.sender_pid, call.sender_euid), (0, 1000));
            let a = observer::OnStatusChanged::read(&mut call.data)?;
            tx.send((app, a.which)).unwrap();
            Ok(Parcel::new())
        }));
        callbacks.push(binder);
        let mut p = Parcel::new();
        cs::AddStatusChangeListener {
            mask,
            callback: Some(binder),
        }
        .write(&mut p);
        f.call(app, cs::ADD_STATUS_CHANGE_LISTENER, &p)
            .unwrap()
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
    }
    let fire = || {
        let mut p = Parcel::new();
        cs::SetMasterSyncAutomatically { flag: true }.write(&mut p);
        f.call(3, cs::SET_MASTER_SYNC_AUTOMATICALLY, &p).unwrap();
    };
    fire();
    assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), (0, 1));
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
    let mut p = Parcel::new();
    cs::RemoveStatusChangeListener {
        callback: Some(callbacks[0]),
    }
    .write(&mut p);
    f.call(0, cs::REMOVE_STATUS_CHANGE_LISTENER, &p).unwrap();
    assert_eq!(listeners.lock().unwrap().len(), 2);
    for code in [
        cs::ADD_STATUS_CHANGE_LISTENER,
        cs::REMOVE_STATUS_CHANGE_LISTENER,
    ] {
        let mut p = Parcel::new();
        if code == cs::ADD_STATUS_CHANGE_LISTENER {
            cs::AddStatusChangeListener {
                mask: -1,
                callback: None,
            }
            .write(&mut p);
        } else {
            cs::RemoveStatusChangeListener { callback: None }.write(&mut p);
        }
        f.call(0, code, &p).unwrap();
    }
    fire();
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
    f.apps[2].shutdown().unwrap();
    deaths.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(listeners.lock().unwrap().len(), 1);
}
