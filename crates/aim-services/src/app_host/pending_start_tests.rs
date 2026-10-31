//! ProcessList.startProcessLocked / AMS.attachApplicationLocked, android-16.0.0_r1.
use super::super::*;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;

struct Thread(mpsc::Sender<u32>, &'static str);
impl Service for Thread {
    fn descriptor(&self) -> &str {
        self.1
    }
    fn accepts_fds(&self) -> bool {
        true
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        let value = if self.1 == AGENT_DESCRIPTOR {
            assert_eq!(call.code, KILL_PROCESS);
            call.data.enforce_interface(AGENT_DESCRIPTOR)?;
            let pid = call.data.read_i32()?;
            assert!(pid > 0, "kill must name a process");
            p.write_no_exception();
            pid as u32
        } else {
            call.code
        };
        self.0.send(value).unwrap();
        Ok(p)
    }
}

#[test]
fn pending_starts_attach_fail_or_are_killed_without_signalling_zero() {
    for path in 0..3 {
        for outcome in 0..if path == 0 { 4 } else { 2 } {
            let fail = outcome == 1;
            let killed = outcome > 1;
            crate::system::with_discovery_binders_flags(
                &[("android.permission.flags.appop_mode_caching_enabled", true)],
                true,
                |host, _, _, export| {
                    let pm = permission_manager::PermissionManager::with_system(
                        host.clone(),
                        Default::default(),
                    );
                    assert!(host.permission_manager.set(Arc::new(pm)).is_ok());
                    let metrics = display::Metrics::of(&display::Display::phone_for_test());
                    host.display_metrics.set(Ok(metrics)).unwrap();
                    let root = aim_paths::original_image();
                    let framework = resources::Framework::load(&root, 160, "en-US").unwrap();
                    assert!(host.framework.set(framework).is_ok());
                    let backing = host.data().unwrap().join("attach-memory");
                    std::fs::write(&backing, []).unwrap();
                    let fd = std::fs::File::open(backing).unwrap().into();
                    let memory = bind::ApplicationSharedMemory::for_test(fd);
                    assert!(host.platform().unwrap().shared_memory.set(memory).is_ok());
                    let (socket, relay) = UnixStream::pair().unwrap();
                    relay
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    if path == 2 {
                        *host.webview_zygote.child.lock().unwrap() = Some(super::Child {
                            pid: 1,
                            session: zygote::Session::new(socket),
                        });
                    } else {
                        *host.zygote.lock().unwrap() = Some(zygote::Session::new(socket));
                    }
                    let (sent, received) = mpsc::channel();
                    let thread = export(Arc::new(Thread(sent.clone(), thread::DESCRIPTOR)));
                    host.attach_agent(export(Arc::new(Thread(sent, AGENT_DESCRIPTOR))));
                    let pid = std::process::id() as i32;
                    std::thread::scope(|scope| {
                        let start = scope.spawn(|| match path {
                            0 => host
                                .start_package_process("p", "p", None, false, false)
                                .map(|(_, pid)| pid),
                            1 => host.start_isolated_entry_point(
                                "E",
                                &[],
                                "relro",
                                "arm64-v8a",
                                1037,
                                Arc::new(|| {}),
                            ),
                            _ => host
                                .start_webview_service_process(
                                    Arc::new(host.launch_inputs("p").unwrap().application),
                                    "renderer",
                                )
                                .map(|(_, pid)| pid),
                        });
                        let mut relay = BufReader::new(relay);
                        let mut line = String::new();
                        relay.read_line(&mut line).unwrap();
                        let count: usize = line.trim().parse().unwrap();
                        let args: Vec<_> = (&mut relay)
                            .lines()
                            .take(count)
                            .map(Result::unwrap)
                            .collect();
                        let seq = args.last().unwrap().strip_prefix("seq=").unwrap();
                        let seq: i64 = seq.parse().unwrap();
                        let uid = host.apps.lock().unwrap()[&seq].uid;
                        assert_eq!(host.apps.lock().unwrap()[&seq].pid, 0);
                        host.process_died(seq, 0);
                        assert_eq!(host.start_of_pid(0), None);
                        assert_eq!(
                            host.isolated_processes(if path == 1 { 1000 } else { 10100 }),
                            (path != 0).then_some(vec![0])
                        );
                        let empty = Parcel::new();
                        let call = Call {
                            code: 0,
                            flags: 0,
                            sender_pid: 0,
                            sender_euid: 1000,
                            data: empty.reader(),
                        };
                        assert!(host.running_app_processes(&call).unwrap().is_none());
                        if killed {
                            if outcome == 2 {
                                host.kill_package_processes("p", uid);
                            } else {
                                host.force_stop_package("p", uid as i32, 0, "pending test");
                            }
                            assert!(host.running_process("p", uid).is_none());
                            assert!(host.exit_infos.get().is_none());
                            assert!(received.try_recv().is_err());
                            attach(host, thread.binder(), seq, uid, pid);
                            assert_eq!(recv(&received), thread::SCHEDULE_EXIT);
                            assert_eq!(host.apps.lock().unwrap()[&seq].pid, 0);
                        } else if !fail {
                            for (s, u) in [(seq + 1000, uid), (seq, uid + 1)] {
                                attach(host, thread.binder(), s, u, pid);
                                assert_eq!(recv(&received), thread::SCHEDULE_EXIT);
                            }
                            attach(host, thread.binder(), seq, uid, pid);
                            let expected = if path == 1 {
                                thread::RUN_ISOLATED_ENTRY_POINT
                            } else {
                                thread::BIND_APPLICATION
                            };
                            assert_eq!(recv(&received), expected);
                            let apps = host.apps.lock().unwrap();
                            assert_eq!(apps[&seq].pid, pid);
                            assert!(apps[&seq].thread.is_some());
                        }
                        let result_pid = if fail { -1i32 } else { pid };
                        relay
                            .get_mut()
                            .write_all(&[result_pid.to_be_bytes().as_slice(), &[0]].concat())
                            .unwrap();
                        let result = start.join().unwrap();
                        assert_eq!(result.is_err(), fail);
                        if fail {
                            assert!(!host.apps.lock().unwrap().contains_key(&seq));
                        } else {
                            assert_eq!(result.unwrap(), pid);
                            if killed {
                                assert_eq!(recv(&received), pid as u32);
                            }
                            assert_eq!(host.apps.lock().unwrap()[&seq].thread.is_some(), !killed);
                            host.process_died(seq, pid);
                            assert!(!host.apps.lock().unwrap().contains_key(&seq));
                        }
                    });
                },
            );
        }
    }
}

fn attach(host: &AppHost, binder: Binder, seq: i64, uid: u32, pid: i32) {
    let data = Parcel::new();
    let call = Call {
        code: am::ATTACH_APPLICATION,
        flags: 0,
        sender_pid: pid,
        sender_euid: uid,
        data: data.reader(),
    };
    host.attach(&call, Some(binder), seq).unwrap();
}

fn recv(receiver: &mpsc::Receiver<u32>) -> u32 {
    receiver.recv_timeout(Duration::from_secs(2)).unwrap()
}
