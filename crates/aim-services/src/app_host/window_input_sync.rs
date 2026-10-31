//! WMS.syncInputTransactions, android-16.0.0_r1. The app host commits
//! placement/input eagerly; its agent, rather than SF, owns input windows.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::*;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

// WMS's ANIMATION_COMPLETED_TIMEOUT_MS and SYNC_INPUT_TRANSACTIONS_TIMEOUT_MS.
const TIMEOUT: Duration = Duration::from_secs(5);

impl AppHost {
    pub(super) fn sync_input_transactions(
        self: &Arc<Self>,
        wait_for_animations: bool,
    ) -> Result<Parcel, Exception> {
        if wait_for_animations {
            self.wait_for_window_animations(TIMEOUT);
        }
        self.wait_for_input_commit(TIMEOUT)?;
        let mut p = Parcel::new();
        p.write_no_exception();
        Ok(p)
    }

    fn wait_for_input_commit(self: &Arc<Self>, timeout: Duration) -> Result<(), Exception> {
        let (send, receive) = mpsc::sync_channel(1);
        let (start, started) = mpsc::sync_channel(1);
        let host = self.clone();
        std::thread::spawn(move || {
            // add/relayout/resize/remove finish their placement and synchronous
            // input RPCs under this lock. There is no deferred placement queue.
            let windows = host.windows.windows.lock().unwrap();
            let _ = start.send(());
            let result = host
                .call_agent(super::super::SYNC_INPUT_WINDOWS, "syncInputWindows", |p| {
                    p.write_i32(windows.len() as i32);
                    for w in windows.values() {
                        p.write_i32(w.channel);
                    }
                })
                .map(|_| ());
            let _ = send.send(result);
        });
        // As in WMS, placement precedes the bounded reported-ack wait.
        let result = started
            .recv()
            .map_err(|_| RecvTimeoutError::Disconnected)
            .and_then(|_| receive.recv_timeout(timeout));
        match result {
            Ok(result) => result,
            // AOSP ignores CountDownLatch.await's false result: no timeout log.
            Err(RecvTimeoutError::Timeout) => Ok(()),
            Err(e) => {
                eprintln!(
                    "app host: Exception thrown while waiting for window infos to be reported: {e}"
                );
                Err(Exception::new(EX_ILLEGAL_STATE, e.to_string()))
            }
        }
    }
}

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../../daemons/app-host-agent/src/input.rs"]
pub(crate) mod dispatcher;

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_host::local::Strong;
    use aim_input_transport::{Motion, Region};
    use std::os::unix::net::UnixDatagram;

    // Compile the real dispatcher; device discovery must
    // never request Android properties in a host-only test.
    #[unsafe(no_mangle)]
    unsafe extern "C" fn __system_property_get(
        _: *const libc::c_char,
        _: *mut libc::c_char,
    ) -> libc::c_int {
        panic!("unexpected Android property read in host dispatcher test")
    }

    struct Agent(
        Arc<dispatcher::Dispatcher>,
        mpsc::Sender<()>,
        Mutex<mpsc::Receiver<()>>,
        std::sync::atomic::AtomicBool,
    );
    impl Service for Agent {
        fn descriptor(&self) -> &str {
            super::super::super::AGENT_DESCRIPTOR
        }
        fn transact(&self, call: &mut Call<'_>) -> Reply {
            call.data.enforce_interface(self.descriptor())?;
            match call.code {
                super::super::super::SET_INPUT_CHANNEL_FRAME => {
                    let index = call.data.read_i32()?;
                    let mut rect = || -> Result<[i32; 4], i32> {
                        Ok([
                            call.data.read_i32()?,
                            call.data.read_i32()?,
                            call.data.read_i32()?,
                            call.data.read_i32()?,
                        ])
                    };
                    let frame = rect()?;
                    let touchable = rect()?;
                    let region = Region {
                        frame,
                        touchable,
                        watch_outside: call.data.read_bool()?,
                        owner: call.data.read_i32()?,
                        owner_pid: call.data.read_i32()?,
                    };
                    self.1.send(()).unwrap();
                    self.2.lock().unwrap().recv().unwrap();
                    assert!(self.0.set_region(index, region));
                }
                super::super::super::SYNC_INPUT_WINDOWS => {
                    let live = (0..call.data.read_i32()?)
                        .map(|_| call.data.read_i32())
                        .collect::<Result<Vec<_>, _>>()?;
                    self.0.sync_windows(&live);
                    if self.3.swap(false, std::sync::atomic::Ordering::SeqCst) {
                        self.2.lock().unwrap().recv().unwrap();
                    }
                    self.1.send(()).unwrap();
                }
                _ => return Err(BAD_VALUE),
            }
            let mut p = Parcel::new();
            p.write_no_exception();
            Ok(p)
        }
    }

    fn sync(host: &Arc<AppHost>, animations: bool) {
        let mut p = Parcel::new();
        wm::SyncInputTransactions {
            wait_for_animations: animations,
        }
        .write(&mut p);
        let result = WindowManager(host.clone())
            .transact(&mut Call {
                code: wm::SYNC_INPUT_TRANSACTIONS,
                flags: 0,
                sender_pid: 99,
                sender_euid: 2000,
                data: p.reader(),
            })
            .unwrap();
        result.reader().read_exception().unwrap().unwrap();
    }

    #[test]
    fn sync_waits_for_window_commit_and_dispatch_uses_the_new_frame() {
        crate::system::with_discovery_binders(|host, _, _, export| {
            let dispatcher = dispatcher::Dispatcher::new();
            let (server, client) = UnixDatagram::pair().unwrap();
            client.set_read_timeout(Some(TIMEOUT)).unwrap();
            dispatcher.add_window(
                0,
                server.into(),
                (500, 500),
                Region {
                    frame: [0, 0, 100, 100],
                    touchable: [0, 0, 100, 100],
                    watch_outside: false,
                    owner: 10001,
                    owner_pid: 42,
                },
            );
            let mut bytes = [0; 512];
            client.recv(&mut bytes).unwrap(); // focus
            let (entered, entering) = mpsc::channel();
            let (release, released) = mpsc::channel();
            let endpoint = Arc::new(Agent(
                dispatcher.clone(),
                entered,
                Mutex::new(released),
                false.into(),
            ));
            let agent: Strong = export(endpoint.clone());
            host.tasks.animation_test_task(7, Rect::default());
            host.animation_test_window(host.process.strong(agent.handle), 1, 7, 1);
            host.attach_agent(agent);
            let changing = host.clone();
            let change = std::thread::spawn(move || {
                let _windows = changing.windows.windows.lock().unwrap();
                let frame = Rect {
                    left: 200,
                    top: 200,
                    right: 300,
                    bottom: 300,
                };
                changing
                    .set_input_frame(
                        0,
                        &Input {
                            frame,
                            touchable: frame,
                            watch_outside: false,
                            owner: 10001,
                            owner_pid: 42,
                        },
                    )
                    .unwrap();
            });
            entering.recv_timeout(TIMEOUT).unwrap();
            let (done, complete) = mpsc::channel();
            let waiting = host.clone();
            let waiter = std::thread::spawn(move || {
                sync(&waiting, false);
                done.send(()).unwrap();
            });
            assert!(matches!(
                complete.recv_timeout(Duration::from_millis(30)),
                Err(RecvTimeoutError::Timeout)
            ));
            release.send(()).unwrap();
            change.join().unwrap();
            complete.recv_timeout(TIMEOUT).unwrap();
            waiter.join().unwrap();
            entering.recv_timeout(TIMEOUT).unwrap();
            endpoint.3.store(true, std::sync::atomic::Ordering::SeqCst);
            host.wait_for_input_commit(Duration::from_millis(20))
                .unwrap();
            release.send(()).unwrap(); // A timeout does not cancel the pending commit.
            entering.recv_timeout(TIMEOUT).unwrap();
            let motion = Motion::injected(0, 1, 1, 250., 260., 1.).unwrap();
            dispatcher.inject(&motion);
            let n = client.recv(&mut bytes).unwrap();
            assert_eq!(&bytes[..n], motion.message(2, (1 << 30) | 2, (200., 200.)));
            // The old region no longer receives gestures after the barrier.
            dispatcher.inject(&Motion::injected(0, 2, 2, 50., 60., 1.).unwrap());
            client.set_nonblocking(true).unwrap();
            assert_eq!(
                client.recv(&mut bytes).unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            host.windows.windows.lock().unwrap().clear();
            sync(host, true);
            dispatcher.inject(&motion);
            // The publisher closes asynchronously after removal.
            client.set_nonblocking(false).unwrap();
            client.set_read_timeout(Some(TIMEOUT)).unwrap();
            assert_eq!(
                client.recv(&mut bytes).unwrap_err().kind(),
                std::io::ErrorKind::ConnectionReset // removal closes the channel publisher
            );
            // Transport/owner errors remain errors, rather than successful syncs.
            *host.agent.lock().unwrap() = None;
            assert!(host.sync_input_transactions(false).is_err());
        });
    }
}
