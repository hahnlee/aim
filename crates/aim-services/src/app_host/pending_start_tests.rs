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
                    // Neither an unpublished service nor its failure holds up AMS attach.
                    if path == 0 && outcome == 0 {
                        host.accessibility_ready.failed("accessibility unavailable");
                    }
                    assert!(host.accessibility_ready.service().is_none());
                    let (socket, relay) = start_inputs(host);
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
                        let args = next_start(&mut relay);
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

/// The inputs a start needs, and the relay of a zygote session for it.
fn start_inputs(host: &Arc<AppHost>) -> (UnixStream, UnixStream) {
    let pm = permission_manager::PermissionManager::with_system(host.clone(), Default::default());
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
    (socket, relay)
}

/// The arguments of the next start the zygote relay reads.
fn next_start(relay: &mut BufReader<UnixStream>) -> Vec<String> {
    let mut line = String::new();
    relay.read_line(&mut line).unwrap();
    let count: usize = line.trim().parse().unwrap();
    relay.lines().take(count).map(Result::unwrap).collect()
}

/// The nice name and start of the next start, which then runs as this
/// process.
fn run_next_start(relay: &mut BufReader<UnixStream>) -> (String, i64) {
    let args = next_start(relay);
    let name = args.iter().find_map(|a| a.strip_prefix("--nice-name="));
    let seq = args.last().unwrap().strip_prefix("seq=").unwrap();
    let pid = std::process::id() as i32;
    let started = [pid.to_be_bytes().as_slice(), &[0]].concat();
    relay.get_mut().write_all(&started).unwrap();
    (name.unwrap().to_string(), seq.parse().unwrap())
}

/// Waits for the process-start handler to run what was posted to it.
fn drain_process_starts(host: &Arc<AppHost>) {
    let (done, drained) = mpsc::channel();
    host.post_process_start(move |_| done.send(()).unwrap());
    drained.recv_timeout(Duration::from_secs(5)).unwrap();
}

/// `mProcessesOnHold`: a start before `mProcessesReady` returns at once and
/// is posted at `finishBooting`, in order, which does not wait for the
/// zygote; one after readiness runs at once.
#[test]
fn starts_before_processes_ready_are_held_until_finish_booting() {
    crate::system::with_discovery_binders_flags(
        &[("android.permission.flags.appop_mode_caching_enabled", true)],
        false,
        |host, _, _, _| {
            let (socket, relay) = start_inputs(host);
            *host.zygote.lock().unwrap() = Some(zygote::Session::new(socket));
            let mut relay = BufReader::new(relay);
            host.start_process_for("p", "p:one", None, false).unwrap();
            host.start_process_for("p", "p:two", None, false).unwrap();
            assert!(host.apps.lock().unwrap().is_empty());
            host.processes_ready.set(Ok(())).unwrap();
            host.start_held_while_booting();
            assert_eq!(host.processes_on_hold.lock().unwrap().len(), 2);
            std::thread::scope(|scope| {
                let late = scope.spawn(|| host.start_process_for("p", "p:late", None, false));
                let mut started = vec![run_next_start(&mut relay)];
                late.join().unwrap().unwrap();
                host.start_processes_on_hold();
                started.push(run_next_start(&mut relay));
                started.push(run_next_start(&mut relay));
                drain_process_starts(host);
                let names: Vec<_> = started.iter().map(|(name, _)| name.as_str()).collect();
                assert_eq!(names, ["p:late", "p:one", "p:two"]);
                for (_, seq) in started {
                    host.process_died(seq, std::process::id() as i32);
                }
            });
            assert!(host.processes_on_hold.lock().unwrap().is_empty());
        },
    );
}

/// `isAllowedWhileBooting`: a persistent app's start is not held for
/// `finishBooting`; it is posted once forks may (`startPersistentApps`),
/// and readiness does not wait for the zygote. A boot that stops fails the
/// held starts.
#[test]
fn starts_allowed_while_booting_are_not_held_for_finish_booting() {
    crate::system::with_discovery_binders_flags(
        &[("android.permission.flags.appop_mode_caching_enabled", true)],
        false,
        |host, publish, _, _| {
            publish(&mut |package| {
                if package.package_name == "p" {
                    package.booleans |= crate::package::pkg::booleans::SYSTEM
                        | crate::package::pkg::booleans::PERSISTENT;
                }
            });
            let (socket, relay) = start_inputs(host);
            *host.zygote.lock().unwrap() = Some(zygote::Session::new(socket));
            let mut relay = BufReader::new(relay);
            host.start_process_for("p", "p", None, false).unwrap();
            host.processes_ready.set(Ok(())).unwrap();
            host.start_held_while_booting();
            let (name, seq) = run_next_start(&mut relay);
            drain_process_starts(host);
            assert_eq!(name, "p");
            host.process_died(seq, std::process::id() as i32);
            assert!(host.processes_on_hold.lock().unwrap().is_empty());
        },
    );
    crate::system::with_boot_discovery_binders(|host, _, _, _| {
        host.start_process_for("p", "p", None, false).unwrap();
        drop(super::super::boot_ready::BootReady(host));
        assert!(host.processes_on_hold.lock().unwrap().is_empty());
        assert!(host.start_process_for("p", "p", None, false).is_err());
    });
}

pub(in crate::app_host) fn attach(host: &AppHost, binder: Binder, seq: i64, uid: u32, pid: i32) {
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
