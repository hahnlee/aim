//! InputDispatcher injection results, modes and foreground completion, pinned
//! android-16.0.0_r1 (AOSP Apache-2.0). Real and injected input share Windows.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::*;
use aim_input_transport::injection::Event;
use std::sync::{Weak, mpsc};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Injection {
    state: Mutex<(Option<i32>, usize)>,
    changed: Condvar,
}
type Job = (Event, i32, Arc<Injection>);
pub(super) fn queue() -> (mpsc::Sender<Job>, mpsc::Receiver<Job>) {
    mpsc::channel()
}

impl Dispatcher {
    pub(super) fn inject_queue(self: Arc<Self>, jobs: mpsc::Receiver<Job>) {
        for (event, uid, injection) in jobs {
            let mut windows = self.windows.lock().unwrap();
            let result = self.inject_dispatch(&mut windows, &event, uid, &injection);
            injection.state.lock().unwrap().0 = Some(result);
            injection.changed.notify_all();
        }
    }
    pub fn inject_event(&self, event: Event, mode: i32, uid: i32, timeout: Duration) -> i32 {
        if !event.valid || !(0..=2).contains(&mode) {
            return 2;
        }
        let deadline = Instant::now() + timeout;
        let injection = Arc::new(Injection::default());
        if self
            .injections
            .send((event, uid, injection.clone()))
            .is_err()
        {
            return 2;
        }
        if mode == 0 {
            return 0;
        } // NONE acknowledges validated enqueue, not targeting.
        let mut state = injection.state.lock().unwrap();
        loop {
            if let Some(result) = state.0 {
                if result != 0 || mode == 1 || state.1 == 0 {
                    return result;
                }
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return 3;
            }
            state = injection.changed.wait_timeout(state, remaining).unwrap().0;
        }
    }
    fn inject_dispatch(
        &self,
        w: &mut Windows,
        e: &Event,
        uid: i32,
        injection: &Arc<Injection>,
    ) -> i32 {
        // The native window owner currently publishes only display 0. Explicit
        // unknown displays must never fall through to the focused display.
        if !matches!(e.display(), -1 | DEFAULT_DISPLAY) {
            return 2;
        }
        let action = e.action();
        let start = e.pointer() && matches!(action, 0 | 7 | 8 | 9 | 10);
        let (at, outside) = if start {
            let regions: Vec<_> = w.stack.iter().map(|t| t.region).collect();
            touched(&regions, e.position())
        } else if e.pointer() {
            (
                w.gesture
                    .and_then(|i| w.stack.iter().position(|t| t.index == i)),
                Vec::new(),
            )
        } else {
            (w.stack.len().checked_sub(1), Vec::new())
        };
        if uid != -1 && at.is_none_or(|i| w.stack[i].region.owner != uid) {
            return 1;
        }
        let Some(at) = at else {
            return 2;
        };
        let target = &w.stack[at];
        if start && action == 0 {
            for i in outside {
                let t = &w.stack[i];
                // Outside observers are not foreground completions.
                send(
                    &t.channel,
                    &e.message(
                        0,
                        self.ids().0,
                        t.origin(),
                        Some(t.region.owner == target.region.owner),
                    ),
                );
            }
            w.gesture = Some(target.index);
        }
        for sample in 0..e.messages.len() {
            let seq = self.ids().0;
            target.channel.enqueue(
                e.message(sample, seq, target.origin(), None),
                Some(Arc::downgrade(injection)),
            );
        }
        if e.pointer() && matches!(action, 1 | 3) {
            w.gesture = None;
        }
        if !matches!(action, 1 | 3 | 6) {
            w.interactions = vec![target.index];
        }
        0
    }
    pub(super) fn finished(&self, index: i32, seq: u32) {
        if let Some(t) = self
            .windows
            .lock()
            .unwrap()
            .stack
            .iter()
            .find(|t| t.index == index)
        {
            finish(t.channel.state.lock().unwrap().pending.remove(&seq));
        }
    }
}
pub(super) fn finish(injection: Option<Weak<Injection>>) {
    if let Some(injection) = injection.and_then(|i| i.upgrade()) {
        injection.state.lock().unwrap().1 -= 1;
        injection.changed.notify_all();
    }
}

#[derive(Default)]
struct Outbound {
    queue: std::collections::VecDeque<(Vec<u8>, Option<Weak<Injection>>)>,
    pending: HashMap<u32, Weak<Injection>>,
    closed: bool,
}
pub(super) struct Channel {
    pub(super) socket: OwnedFd,
    state: Mutex<Outbound>,
    changed: Condvar,
}
impl Channel {
    pub(super) fn new(socket: OwnedFd) -> Arc<Self> {
        let c = Arc::new(Self {
            socket,
            state: Mutex::new(Outbound::default()),
            changed: Condvar::new(),
        });
        let writer = c.clone();
        std::thread::spawn(move || writer.publish());
        c
    }
    pub(super) fn enqueue(&self, message: Vec<u8>, injection: Option<Weak<Injection>>) {
        let mut s = self.state.lock().unwrap();
        if s.closed {
            return;
        }
        if let Some(i) = injection.as_ref().and_then(Weak::upgrade) {
            i.state.lock().unwrap().1 += 1;
        }
        s.queue.push_back((message, injection));
        self.changed.notify_one();
    }
    pub(super) fn close(&self) {
        let mut s = self.state.lock().unwrap();
        s.closed = true;
        for (_, i) in s.queue.drain(..) {
            finish(i);
        }
        for (_, i) in s.pending.drain() {
            finish(Some(i));
        }
        // Wake both the consumer reader and a publisher waiting for writability.
        unsafe {
            libc::shutdown(self.socket.as_raw_fd(), libc::SHUT_RDWR);
        }
        self.changed.notify_all();
    }
    fn publish(&self) {
        loop {
            let mut s = self.state.lock().unwrap();
            while s.queue.is_empty() && !s.closed {
                s = self.changed.wait(s).unwrap();
            }
            if s.closed {
                return;
            }
            let (m, _) = s.queue.front().unwrap();
            let n = unsafe {
                libc::send(
                    self.socket.as_raw_fd(),
                    m.as_ptr().cast(),
                    m.len(),
                    libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT,
                )
            };
            if n == m.len() as isize {
                let (m, i) = s.queue.pop_front().unwrap();
                if let Some(i) = i {
                    s.pending
                        .insert(u32::from_le_bytes(m[4..8].try_into().unwrap()), i);
                }
                continue;
            }
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            drop(s);
            if error.kind() != std::io::ErrorKind::WouldBlock
                && error.raw_os_error() != Some(libc::ENOBUFS)
            {
                log::warn!("input channel: {error}");
                self.close();
                return;
            }
            let mut fd = libc::pollfd {
                fd: self.socket.as_raw_fd(),
                events: libc::POLLOUT,
                revents: 0,
            };
            // Retain the outbound head until the nonblocking socket is writable.
            let result = unsafe { libc::poll(&mut fd, 1, -1) };
            if result < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
            {
                self.close();
                return;
            }
        }
    }
}

#[cfg(test)]
#[test]
fn result_and_finished_share_deadline_and_removal_releases_completion() {
    use std::os::unix::net::UnixDatagram;
    let d = Dispatcher::new();
    let (server, client) = UnixDatagram::pair().unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    d.add_window(
        0,
        server.into(),
        (100, 100),
        Region {
            frame: [0, 0, 100, 100],
            touchable: [0, 0, 100, 100],
            watch_outside: false,
            owner: 10100,
            owner_pid: 42,
        },
    );
    let mut buffer = [0; 512];
    client.recv(&mut buffer).unwrap();
    let e = Event {
        parcel: Vec::new(),
        messages: vec![vec![0; 104]],
        valid: true,
    };
    let locked = d.windows.lock().unwrap();
    std::thread::scope(|scope| {
        let start = Instant::now();
        let waiting = scope.spawn(|| d.inject_event(e.clone(), 2, -1, Duration::from_millis(450)));
        std::thread::sleep(Duration::from_millis(300));
        drop(locked);
        assert_eq!(waiting.join().unwrap(), 3);
        assert!(start.elapsed() < Duration::from_millis(650));
    });
    client.recv(&mut buffer).unwrap();
    std::thread::scope(|scope| {
        let waiting = scope.spawn(|| d.inject_event(e, 2, -1, Duration::from_secs(2)));
        client.recv(&mut buffer).unwrap();
        assert!(!waiting.is_finished());
        d.sync_windows(&[]);
        assert_eq!(waiting.join().unwrap(), 0);
    });
}
