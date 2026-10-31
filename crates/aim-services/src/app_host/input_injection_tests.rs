//! CTS InputEventTest/InputManagerTest contracts, pinned IMS/InputDispatcher.
use super::*;
use crate::app_host::window_manager::input_sync::dispatcher::Dispatcher;
use std::os::unix::net::UnixDatagram;
use std::time::Duration;

fn motion() -> Event {
    let mut p = Parcel::new();
    for v in [1, 1, 2, 123, 7, 0x1002, -1, 32] {
        p.write_i32(v);
    }
    p.write_raw(&[9; 32], &[]);
    for v in [0, 0, 0, 4, 8, 16, 2] {
        p.write_i32(v);
    }
    for v in [
        2.0,
        0.0,
        100.0,
        0.0,
        3.0,
        200.0,
        0.5,
        0.25,
        f32::NAN,
        f32::NAN,
        1.0,
        0.0,
        0.0,
        0.0,
        1.0,
        0.0,
    ] {
        p.write_f32(v);
    }
    p.write_i64(10);
    p.write_i32(3);
    p.write_i32(2);
    for time in [10, 20] {
        p.write_i64(time);
        p.write_i64(0xc000_0000_0000_0001u64 as i64);
        for v in [5.0, 6.0, 42.0] {
            p.write_f32(v);
        }
        p.write_bool(true);
    }
    let (e, used) = Event::decode(p.data()).unwrap();
    assert_eq!(used, p.data().len());
    assert_eq!(e.parcel, p.data());
    for n in 0..p.data().len() {
        assert!(Event::decode(&p.data()[..n]).is_none());
    }
    e
}
struct Agent(Arc<Dispatcher>);
impl Service for Agent {
    fn descriptor(&self) -> &str {
        crate::app_host::AGENT_DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, 19);
        call.data.enforce_interface(self.descriptor())?;
        let bytes = aim_service_aidl::read_byte_array(&mut call.data)?.unwrap();
        let e = Event::decode(&bytes).unwrap().0;
        let (mode, uid) = (call.data.read_i32()?, call.data.read_i32()?);
        let mut p = Parcel::new();
        p.write_no_exception();
        p.write_i32(
            self.0
                .inject_event(e, mode, uid, Duration::from_millis(100)),
        );
        Ok(p)
    }
}
#[test]
fn input_injection_permission_target_modes_transform_and_completion() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let permissions = super::super::permission_manager::PermissionManager::with_system(
            host.clone(),
            Default::default(),
        );
        assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
        let d = Dispatcher::new();
        host.attach_agent(export(Arc::new(Agent(d.clone()))));
        let (server, client) = UnixDatagram::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        use std::os::fd::AsRawFd;
        let size: libc::c_int = 1024;
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    server.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&size as *const libc::c_int).cast(),
                    4,
                )
            },
            0
        );
        d.add_window(
            0,
            server.into(),
            (500, 500),
            aim_input_transport::Region {
                frame: [100, 200, 300, 400],
                touchable: [100, 200, 300, 400],
                watch_outside: false,
                owner: 10100,
                owner_pid: 42,
            },
        );
        let mut buf = [0; 512];
        client.recv(&mut buf).unwrap();
        let manager = InputManager::new(host.clone());
        let call = |caller, mode, uid, event: Option<Event>| {
            let mut p = Parcel::new();
            im::InjectInputEventToTarget {
                ev: event.map(InputEvent),
                mode,
                target_uid: uid,
            }
            .write(&mut p);
            let p = manager
                .transact(&mut Call {
                    code: im::INJECT_INPUT_EVENT_TO_TARGET,
                    flags: 0,
                    sender_pid: 99,
                    sender_euid: caller,
                    data: p.reader(),
                })
                .unwrap();
            im::read_inject_input_event_to_target_reply(&mut p.reader()).unwrap()
        };
        let e = motion();
        assert_eq!(call(10100, 99, -1, None).unwrap_err().code, -1); // permission before null/mode
        assert_eq!(call(0, 1, -1, None).unwrap_err().code, -4);
        assert_eq!(call(0, 99, -1, Some(e.clone())).unwrap_err().code, -3);
        assert_eq!(call(0, 1, 10099, Some(e.clone())).unwrap_err().code, -3);
        assert!(call(0, 0, 10099, Some(e.clone())).unwrap()); // ASYNC acknowledges enqueue
        assert!(call(0, 1, 10100, Some(e.clone())).unwrap());
        let receive = || {
            let mut b = [0; 512];
            let n = client.recv(&mut b).unwrap();
            b[..n].to_vec()
        };
        let a = receive();
        let b = receive();
        let int = aim_input_transport::injection::int;
        let float = |m: &[u8], at| f32::from_bits(int(m, at) as u32);
        assert_eq!(
            (int(&a, 24), int(&a, 32), int(&a, 168), int(&a, 172)),
            (-1, 0, 3, 2)
        );
        assert_eq!(
            (
                float(&a, 120),
                float(&a, 124),
                float(&a, 184),
                float(&a, 188),
                float(&a, 192)
            ),
            (-100.0, -200.0, 110.0, 218.0, 42.0)
        );
        assert_eq!(a[304], 1);
        assert_eq!(int(&b, 16), 20);
        assert!(!call(0, 2, 10100, Some(e.clone())).unwrap()); // FINISHED timeout, not send success
        receive();
        receive();
        std::thread::scope(|scope| {
            let waiting = scope.spawn(|| call(0, 2, 10100, Some(e.clone())));
            let a = receive();
            let b = receive();
            let ack = |seq: u32| {
                let mut m = vec![0; 24];
                m[..4].copy_from_slice(&2u32.to_ne_bytes());
                m[4..8].copy_from_slice(&seq.to_ne_bytes());
                client.send(&m).unwrap();
            };
            ack(999999u32);
            ack(int(&a, 4) as u32);
            ack(int(&a, 4) as u32);
            assert!(!waiting.is_finished()); // wrong and duplicate sequences cannot finish history
            ack(int(&b, 4) as u32);
            assert!(waiting.join().unwrap().unwrap());
        });
        // Fill the real channel without reading: selection succeeds while
        // publication retains its head and FINISHED still waits for the consumer.
        for _ in 0..100 {
            assert_eq!(
                d.inject_event(e.clone(), 1, 10100, Duration::from_secs(1)),
                0
            );
        }
        std::thread::scope(|scope| {
            let waiting =
                scope.spawn(|| d.inject_event(e.clone(), 2, 10100, Duration::from_secs(2)));
            let mut previous = 0;
            for n in 0..202 {
                let m = receive();
                let seq = int(&m, 4) as u32;
                assert!(seq > previous);
                previous = seq;
                let mut ack = vec![0; 24];
                ack[..4].copy_from_slice(&2u32.to_le_bytes());
                ack[4..8].copy_from_slice(&seq.to_le_bytes());
                if n >= 200 {
                    client.send(&ack).unwrap();
                }
            }
            assert_eq!(waiting.join().unwrap(), 0);
        });
        let mut unknown = e.clone();
        unknown.messages[0][32..36].copy_from_slice(&7i32.to_ne_bytes());
        assert_eq!(
            d.inject_event(unknown, 1, -1, Duration::from_millis(100)),
            2
        );
        let mut invalid = e.clone();
        invalid.valid = false;
        assert_eq!(
            d.inject_event(invalid, 0, -1, Duration::from_millis(100)),
            2
        );
        drop(client);
    });
}

pub(crate) fn permission_probe(host: &Arc<AppHost>, uid: i32) -> i32 {
    InputManager::new(host.clone())
        .inject(
            (99, uid),
            im::InjectInputEventToTarget {
                ev: None,
                mode: 0,
                target_uid: -1,
            },
        )
        .unwrap_err()
        .code
}
