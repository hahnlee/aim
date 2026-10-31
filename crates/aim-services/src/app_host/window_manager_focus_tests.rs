//! WindowManagerService.reportFocusChanged / WindowState's observer list
//! at android-16.0.0_r1, over real host Binder processes, without a guest.
use super::super::activity_stack::{Activity, State, Task};
use super::super::activity_start::LaunchIntent;
use super::*;
use aim_binder_driver::{Credentials, Device, Driver, File, GuestProcess, errno, uapi::*};
use aim_binder_host::local::{LocalProcess, Strong};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

const BARRIER: u32 = 99;

struct Recorder {
    tokens: Arc<Mutex<HashMap<Binder, u32>>>,
    events: Sender<(u32, u32)>,
    process: Arc<LocalProcess>,
}

impl Service for Recorder {
    fn descriptor(&self) -> &str {
        observer::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert!(call.is_oneway());
        let window = match call.code {
            observer::FOCUS_GAINED => {
                observer::FocusGained::read(&mut call.data)
                    .unwrap()
                    .input_token
            }
            observer::FOCUS_LOST => {
                observer::FocusLost::read(&mut call.data)
                    .unwrap()
                    .input_token
            }
            BARRIER => {
                self.events.send((BARRIER, 0)).unwrap();
                return Ok(Parcel::new());
            }
            _ => panic!("unexpected focus callback"),
        }
        .unwrap();
        assert_eq!(call.data.remaining(), 0);
        let Binder::Handle(handle) = window else {
            unreachable!()
        };
        let mut p = Parcel::new();
        wid::IsFocused {}.write(&mut p);
        let reply = self
            .process
            .transact(handle, wid::IS_FOCUSED, &p, false)
            .unwrap();
        wid::read_is_focused_reply(&mut reply.reader())
            .unwrap()
            .unwrap();
        self.events
            .send((call.code, self.tokens.lock().unwrap()[&window]))
            .unwrap();
        Ok(Parcel::new())
    }
}

struct Hub {
    tokens: Arc<Mutex<HashMap<Binder, u32>>>,
    process: Arc<LocalProcess>,
    held: Mutex<Vec<Strong>>,
}
impl Service for Hub {
    fn descriptor(&self) -> &str {
        "test.focus.Hub"
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        if call.code == 1 {
            p.write_binder(Some(Binder::Local(call.data.read_i64().unwrap() as u64)));
        } else {
            let id = call.data.read_i32().unwrap() as u32;
            let token = call.data.read_binder().unwrap().unwrap();
            let Binder::Handle(handle) = token else {
                unreachable!()
            };
            self.held.lock().unwrap().push(self.process.strong(handle));
            self.tokens.lock().unwrap().insert(token, id);
        }
        Ok(p)
    }
}

struct NoMemory;
impl GuestProcess for NoMemory {
    fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), i32> {
        Err(errno::EFAULT)
    }
    fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), i32> {
        Err(errno::EFAULT)
    }
    fn get_file(&mut self, _: u32) -> Result<File, i32> {
        Err(errno::EBADF)
    }
    fn install_file(&mut self, _: File) -> Result<u32, i32> {
        Err(errno::EBADF)
    }
    fn close_fd(&mut self, _: u32) {
        panic!("no fixture files")
    }
}

pub(super) struct Fixture {
    pub(super) host: Arc<AppHost>,
    pub(super) remote: Arc<LocalProcess>,
    pub(super) observer: Strong,
    events: Receiver<(u32, u32)>,
}

impl Fixture {
    pub(super) fn new() -> Self {
        let driver = Driver::new();
        let open = |pid| {
            LocalProcess::open(
                &driver,
                Device::Binder,
                Credentials {
                    pid,
                    euid: 1000,
                    security_context: None,
                },
            )
        };
        let remote = open(98201);
        let host = AppHost::new(open(98202));
        let tokens = Arc::new(Mutex::new(HashMap::new()));
        let Binder::Local(ptr) = remote.add_service(Arc::new(Hub {
            tokens: tokens.clone(),
            process: remote.clone(),
            held: Mutex::default(),
        })) else {
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
                remote.proc_handle(),
                98201,
                BINDER_SET_CONTEXT_MGR_EXT,
                &mut object,
                &mut NoMemory,
            )
            .unwrap();
        remote.start();
        host.process.start();
        let (send, events) = mpsc::channel();
        let Binder::Local(ptr) = remote.add_service(Arc::new(Recorder {
            tokens,
            events: send,
            process: remote.clone(),
        })) else {
            unreachable!()
        };
        let mut p = Parcel::new();
        p.write_i64(ptr as i64);
        let received = host.process.transact(0, 1, &p, false).unwrap();
        let observer = received
            .retain_remote_binder(received.reader().read_binder().unwrap().unwrap())
            .unwrap();
        Self {
            host,
            remote,
            observer,
            events,
        }
    }

    fn activity(&self, token: u64, task: i32, state: State) {
        let mut stack = self.host.activities.stack.lock().unwrap();
        stack.freeform = true;
        if !stack.tasks.iter().any(|t| t.id == task) {
            stack.tasks.push(Task {
                id: task,
                affinity: None,
                real_activity: "p/A".into(),
                intent: Default::default(),
                base: LaunchIntent::Component,
                uid: 10100 + task as u32,
                user: 0,
                last_active: 0,
            });
        }
        let mut a = Activity::new(token, task, 1, "p/A", 0, 0, None);
        a.state = state;
        stack.activities.push(a);
    }

    fn state(&self, token: u64, state: State) {
        self.host
            .activities
            .stack
            .lock()
            .unwrap()
            .activities
            .iter_mut()
            .find(|a| a.token == token)
            .unwrap()
            .state = state;
        self.host.run(Vec::new());
    }

    pub(super) fn window(&self, handle: u32, activity: Option<u64>, registered: bool) -> Binder {
        let (task, uid) = {
            let stack = self.host.activities.stack.lock().unwrap();
            let task = activity.and_then(|t| stack.get(t)).map(|a| a.task);
            (
                task,
                task.and_then(|t| stack.task(t)).map_or(10100, |t| t.uid),
            )
        };
        let attrs = LayoutParams::default();
        let frames = window_layout::compute_frames(&attrs, Rect::default(), None, true, (-1, -1));
        self.host.windows.windows.lock().unwrap().insert(
            handle,
            Window {
                _client: self.host.process.strong(self.observer.handle),
                id: handle,
                name: "p/A".into(),
                uid,
                pid: 71,
                task,
                activity,
                parent: None,
                attrs,
                requested: (-1, -1),
                frames,
                channel: 0,
                container: None,
                layer: None,
                frames_seq: FIRST_SEQ,
                insets_seq: FIRST_SEQ,
                requested_visible_types: 0,
                window_id: None,
                accessibility_id: -1,
                focus_callbacks: Arc::default(),
            },
        );
        let session = Session {
            host: self.host.clone(),
            uid: 10100,
            pid: 71,
        };
        let id = session.window_id(handle).unwrap();
        assert_eq!(session.window_id(handle).unwrap(), id);
        let mut p = Parcel::new();
        p.write_i32(handle as i32);
        p.write_binder(Some(id));
        self.host.process.transact(0, 2, &p, false).unwrap();
        // Finish setup before publishing callbacks that can reenter windows.
        self.host.update_window_focus();
        if registered {
            self.register(id, true);
        }
        id
    }

    fn invoke(&self, id: Binder, code: u32, p: &Parcel) -> aim_binder_host::local::Received {
        let Binder::Local(ptr) = id else {
            unreachable!()
        };
        self.host
            .process
            .local_service(ptr)
            .unwrap()
            .transact(code, p, false)
            .unwrap()
    }

    fn register(&self, id: Binder, on: bool) {
        let mut p = Parcel::new();
        let code = if on {
            wid::RegisterFocusObserver {
                observer: Some(self.observer.binder()),
            }
            .write(&mut p);
            wid::REGISTER_FOCUS_OBSERVER
        } else {
            wid::UnregisterFocusObserver {
                observer: Some(self.observer.binder()),
            }
            .write(&mut p);
            wid::UNREGISTER_FOCUS_OBSERVER
        };
        self.invoke(id, code, &p)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap();
    }

    fn focused(&self, id: Binder) -> bool {
        let mut p = Parcel::new();
        wid::IsFocused {}.write(&mut p);
        wid::read_is_focused_reply(&mut self.invoke(id, wid::IS_FOCUSED, &p).reader())
            .unwrap()
            .unwrap()
    }

    fn expect(&self, expected: &[(u32, u32)]) {
        self.observer
            .transact(BARRIER, &Parcel::new(), true)
            .unwrap();
        for event in expected.iter().copied().chain([(BARRIER, 0)]) {
            assert_eq!(
                self.events.recv_timeout(Duration::from_secs(5)).unwrap(),
                event
            );
        }
        assert!(self.events.try_recv().is_err());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let remote = self.remote.shutdown();
        let host = self.host.process.shutdown();
        for result in [remote, host] {
            if let Err(error) = result {
                if std::thread::panicking() {
                    eprintln!("window fixture cleanup: {error}");
                } else {
                    panic!("window fixture cleanup: {error}");
                }
            }
        }
    }
}

#[test]
fn fixture_cleanup_reports_looper_failure_and_preserves_unwinding() {
    struct Panics(Sender<()>);
    impl Service for Panics {
        fn descriptor(&self) -> &str {
            "test.focus.Panics"
        }
        fn transact(&self, _: &mut Call<'_>) -> Reply {
            self.0.send(()).unwrap();
            panic!("original callback failure");
        }
    }
    for unwinding in [false, true] {
        let f = Fixture::new();
        let remote = f.remote.clone();
        let host = f.host.process.clone();
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (send, entered) = mpsc::channel();
            let Binder::Local(ptr) = remote.add_service(Arc::new(Panics(send))) else {
                unreachable!()
            };
            let mut p = Parcel::new();
            p.write_i64(ptr as i64);
            let received = host.transact(0, 1, &p, false).unwrap();
            let callback = received
                .retain_remote_binder(received.reader().read_binder().unwrap().unwrap())
                .unwrap();
            callback.transact(1, &Parcel::new(), true).unwrap();
            entered.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(remote.shutdown().is_err());
            if unwinding {
                panic!("original fixture failure");
            }
            drop(f);
        }))
        .unwrap_err();
        let message = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .unwrap();
        if unwinding {
            assert_eq!(message, "original fixture failure");
        } else {
            assert!(message.contains("native Binder looper panicked"));
        }
        assert!(remote.shutdown().is_err());
        assert!(host.shutdown().is_ok());
    }
}

#[test]
fn resume_pause_and_latest_window_of_the_top_activity() {
    let f = Fixture::new();
    f.activity(1, 1, State::Paused);
    let first = f.window(1, Some(1), true);
    assert!(!f.focused(first));
    f.expect(&[]); // registration is not an initial focus report
    f.state(1, State::Resumed);
    assert!(f.focused(first));
    f.expect(&[(observer::FOCUS_GAINED, 1)]);
    let latest = f.window(2, Some(1), true);
    assert!(!f.focused(first));
    assert!(f.focused(latest));
    f.expect(&[(observer::FOCUS_LOST, 1)]); // new window has not registered yet
    f.state(1, State::Pausing);
    f.expect(&[(observer::FOCUS_LOST, 2)]);
    f.state(1, State::Paused);
    f.expect(&[]);
    f.state(1, State::Resumed);
    f.expect(&[(observer::FOCUS_GAINED, 2)]);
    // The next activity is not resumed yet: none of its app's windows
    // can take focus, including the old activity's registered windows.
    f.activity(2, 1, State::Paused);
    let other = f.window(3, Some(2), true);
    assert!(!f.focused(other));
    assert!(!f.focused(latest));
    f.expect(&[(observer::FOCUS_LOST, 2)]);
    f.state(2, State::Resumed);
    assert!(f.focused(other));
    f.expect(&[(observer::FOCUS_GAINED, 3)]);
    // Same UID, but an older activity's newer window cannot take focus.
    let old_activity = f.window(4, Some(1), true);
    assert!(!f.focused(old_activity));
    let tokenless = f.window(5, None, true);
    assert!(!f.focused(tokenless));
    f.expect(&[]);
}

#[test]
fn task_switch_delivers_gain_before_loss_and_removal_restores_the_previous_window() {
    let f = Fixture::new();
    f.activity(1, 1, State::Resumed);
    let first = f.window(1, Some(1), true);
    let latest = f.window(2, Some(1), true);
    f.expect(&[(observer::FOCUS_LOST, 1)]);
    f.activity(2, 2, State::Resumed);
    let second = f.window(3, Some(2), true);
    f.expect(&[(observer::FOCUS_LOST, 2)]);
    // Mac FOCUS and moveTaskToFront share this stack transition and run.
    f.host.activities.stack.lock().unwrap().bring_to_front(1);
    f.host.run(Vec::new());
    assert!(f.focused(latest));
    assert!(!f.focused(second));
    f.expect(&[(observer::FOCUS_GAINED, 2), (observer::FOCUS_LOST, 3)]);
    f.host.run(Vec::new());
    f.expect(&[]);
    f.host.remove_window(2);
    assert!(!f.focused(latest));
    assert!(f.focused(first));
    f.expect(&[(observer::FOCUS_GAINED, 1), (observer::FOCUS_LOST, 2)]);
    f.host.windows_died(71);
    assert!(!f.focused(first));
    f.expect(&[(observer::FOCUS_LOST, 1)]);
}

#[test]
fn unregister_and_observer_death_remove_callbacks() {
    let f = Fixture::new();
    f.activity(1, 1, State::Paused);
    let id = f.window(1, Some(1), true);
    f.register(id, true); // RemoteCallbackList registers each binder once
    let callbacks = f.host.windows.windows.lock().unwrap()[&1]
        .focus_callbacks
        .clone();
    assert_eq!(callbacks.observers.lock().unwrap().len(), 1);
    f.state(1, State::Resumed);
    f.expect(&[(observer::FOCUS_GAINED, 1)]);
    f.register(id, false);
    assert!(callbacks.observers.lock().unwrap().is_empty());
    f.state(1, State::Pausing);
    f.expect(&[]);
    f.register(id, true);
    f.remote.shutdown().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !callbacks.observers.lock().unwrap().is_empty() {
        assert!(
            Instant::now() < deadline,
            "observer death was not delivered"
        );
        std::thread::yield_now();
    }
    f.state(1, State::Resumed);
    assert!(f.focused(id));
    assert!(f.events.try_recv().is_err());
}

#[test]
fn serialized_reports_allow_observer_reentry_during_rapid_transitions() {
    let f = Fixture::new();
    f.activity(1, 1, State::Paused);
    let id = f.window(1, Some(1), true);
    let mut expected = Vec::new();
    for _ in 0..64 {
        f.state(1, State::Resumed);
        f.state(1, State::Pausing);
        expected.extend([(observer::FOCUS_GAINED, 1), (observer::FOCUS_LOST, 1)]);
    }
    assert!(!f.focused(id));
    f.expect(&expected);
}
