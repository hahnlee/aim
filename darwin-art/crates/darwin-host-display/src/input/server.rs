//! The device side: one listening socket per device in the device
//! directory, and the clients of each.
//!
//! A packet is sent to a client whole or not at all. A client whose socket
//! is full loses the packet and gets `SYN_DROPPED` before the next one, as
//! an evdev client whose buffer overflows does; a reader that falls behind
//! never stalls the window.

use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::codes::{EV_SYN, SYN_DROPPED, SYN_REPORT};
use super::{Descriptor, Hello, OP_GRAB, OP_OPEN, Record, State, VERSION, record_bytes};
use crate::wire;

const EBUSY: i32 = 16;
const EINVAL: i32 = 22;
const ENODEV: i32 = 19;

struct Client {
    id: u64,
    sock: OwnedFd,
    /// A packet was lost: send `SYN_DROPPED` first.
    dropped: AtomicBool,
}

struct Inner {
    state: State,
    /// Events since the last `SYN_REPORT`.
    pending: Vec<Record>,
    clients: Vec<Arc<Client>>,
    grab: Option<u64>,
    next_client: u64,
}

struct Device {
    desc: Descriptor,
    inner: Mutex<Inner>,
}

/// The devices of one display server.
pub struct Devices {
    dir: PathBuf,
    devices: Vec<Arc<Device>>,
}

impl Device {
    /// Feed one event through the input core's rules; a `SYN_REPORT`
    /// sends the packet.
    fn event(&self, inner: &mut Inner, r: Record, time_ns: i64) {
        if r.kind == EV_SYN && r.code == SYN_REPORT {
            flush(inner, time_ns);
        } else if r.kind != EV_SYN && self.desc.apply(&mut inner.state, &r) {
            inner.pending.push(r);
        }
    }

    /// Serve one connection.
    fn serve(self: Arc<Self>, conn: UnixStream) {
        let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
        let Ok(Some(hello)) = wire::recv_record::<Hello>(conn.as_fd()) else {
            return;
        };
        if hello.version != VERSION {
            return;
        }
        match hello.op {
            OP_OPEN => self.open(conn),
            OP_GRAB => {
                let r = self.grab(hello.client, hello.arg != 0);
                let _ = (&conn).write_all(&r.to_ne_bytes());
            }
            _ => {}
        }
    }

    fn open(&self, mut conn: UnixStream) {
        let _ = conn.set_read_timeout(None);
        // A full socket fails a send at once rather than blocking the
        // window; the rest of a packet already begun may wait a little.
        let _ = conn.set_write_timeout(Some(Duration::from_millis(250)));
        let Ok(sock) = conn.try_clone() else { return };
        let client = {
            let mut inner = self.inner.lock().unwrap();
            let id = inner.next_client;
            inner.next_client += 1;
            // The state and the events after it go out in order: the
            // descriptor is sent under the lock the events take.
            let desc = Descriptor {
                client: id,
                state: inner.state,
                ..self.desc
            };
            if wire::send(conn.as_fd(), wire::bytes(&desc), None).is_err() {
                return;
            }
            let client = Arc::new(Client {
                id,
                sock: sock.into(),
                dropped: AtomicBool::new(false),
            });
            inner.clients.push(client.clone());
            client
        };
        // What the client writes is injected into the device.
        let mut buf = [Record::default(); 64];
        let mut have = 0usize;
        loop {
            // SAFETY: `Record` is plain old data.
            let bytes = unsafe {
                std::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), size_of_val(&buf))
            };
            let n = match conn.read(&mut bytes[have..]) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            have += n;
            let whole = have / size_of::<Record>();
            if whole > 0 {
                let now = crate::monotonic_ns();
                let mut inner = self.inner.lock().unwrap();
                for r in &buf[..whole] {
                    self.event(&mut inner, *r, now);
                }
                drop(inner);
                let used = whole * size_of::<Record>();
                bytes.copy_within(used..have, 0);
                have -= used;
            }
        }
        let mut inner = self.inner.lock().unwrap();
        inner.clients.retain(|c| !Arc::ptr_eq(c, &client));
        if inner.grab == Some(client.id) {
            inner.grab = None;
        }
    }

    /// `EVIOCGRAB` for client `id`, with the kernel's answers.
    fn grab(&self, id: u64, on: bool) -> i32 {
        let mut inner = self.inner.lock().unwrap();
        if !inner.clients.iter().any(|c| c.id == id) {
            return -ENODEV;
        }
        match (on, inner.grab) {
            (true, Some(_)) => -EBUSY,
            (true, None) => {
                inner.grab = Some(id);
                0
            }
            (false, Some(g)) if g == id => {
                inner.grab = None;
                0
            }
            (false, _) => -EINVAL,
        }
    }
}

/// Send the pending events as one packet, all stamped `time_ns` as the
/// input core stamps a packet.
fn flush(inner: &mut Inner, time_ns: i64) {
    if inner.pending.is_empty() {
        return;
    }
    let mut packet = Vec::with_capacity(inner.pending.len() + 2);
    packet.push(Record {
        time_ns,
        kind: EV_SYN,
        code: SYN_DROPPED,
        value: 0,
    });
    packet.append(&mut inner.pending);
    packet.push(Record {
        time_ns,
        kind: EV_SYN,
        code: SYN_REPORT,
        value: 0,
    });
    for r in &mut packet {
        r.time_ns = time_ns;
    }
    for c in &inner.clients {
        if inner.grab.is_some_and(|g| g != c.id) {
            continue;
        }
        let from = if c.dropped.load(Ordering::Relaxed) {
            0
        } else {
            1
        };
        send_packet(c, record_bytes(&packet[from..]));
    }
}

fn send_packet(c: &Client, data: &[u8]) {
    let fd = c.sock.as_raw_fd();
    // SAFETY: sends from our buffer on the client's socket.
    let n = unsafe { libc::send(fd, data.as_ptr().cast(), data.len(), libc::MSG_DONTWAIT) };
    if n < 0 {
        if io::Error::last_os_error().kind() == io::ErrorKind::WouldBlock {
            c.dropped.store(true, Ordering::Relaxed);
        }
        return;
    }
    c.dropped.store(false, Ordering::Relaxed);
    let n = n as usize;
    // A partial send leaves a record cut: finish it (blocking, with the
    // socket's send timeout) or end the client.
    if n < data.len() && wire::send(c.sock.as_fd(), &data[n..], None).is_err() {
        // SAFETY: shutting down the client's socket; its reader ends it.
        unsafe { libc::shutdown(fd, libc::SHUT_RDWR) };
    }
}

impl Devices {
    /// Create the devices in `dir` (created if needed; stale device sockets
    /// of an earlier server are removed). Each socket is bound and
    /// listening before it appears in `dir`, so a client that sees it can
    /// open it.
    pub fn create(dir: &Path, descriptors: Vec<Descriptor>) -> io::Result<Devices> {
        std::fs::create_dir_all(dir)?;
        for e in std::fs::read_dir(dir)?.flatten() {
            if e.file_name().to_string_lossy().starts_with("event") {
                let _ = std::fs::remove_file(e.path());
            }
        }
        let mut staging = dir.as_os_str().to_owned();
        staging.push(".new");
        let staging = PathBuf::from(staging);
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging)?;
        let mut devices = Vec::new();
        for desc in descriptors {
            let name = format!("event{}", desc.index);
            let listener = super::listen_in(&staging, &name)?;
            // Access is the guest's business (the syscall layer shows the
            // node as root:input 0660).
            std::fs::set_permissions(staging.join(&name), std::fs::Permissions::from_mode(0o666))?;
            let device = Arc::new(Device {
                desc,
                inner: Mutex::new(Inner {
                    state: desc.state,
                    pending: Vec::new(),
                    clients: Vec::new(),
                    grab: None,
                    next_client: 1,
                }),
            });
            let d = device.clone();
            std::thread::spawn(move || {
                for conn in listener.incoming().flatten() {
                    let d = d.clone();
                    std::thread::spawn(move || d.serve(conn));
                }
            });
            std::fs::rename(staging.join(&name), dir.join(&name))?;
            devices.push(device);
        }
        let _ = std::fs::remove_dir(&staging);
        Ok(Devices {
            dir: dir.to_owned(),
            devices,
        })
    }

    /// Report `events` (type, code, value) of device `index` as one packet
    /// at `time_ns` (the guest's `CLOCK_MONOTONIC`).
    pub fn emit(&self, index: u32, time_ns: i64, events: &[(u16, u16, i32)]) {
        let Some(d) = self.devices.iter().find(|d| d.desc.index == index) else {
            return;
        };
        let mut inner = d.inner.lock().unwrap();
        for &(kind, code, value) in events {
            let r = Record {
                time_ns,
                kind,
                code,
                value,
            };
            d.event(&mut inner, r, time_ns);
        }
        flush(&mut inner, time_ns);
    }

    /// How many clients have device `index` open.
    pub fn clients(&self, index: u32) -> usize {
        self.devices
            .iter()
            .find(|d| d.desc.index == index)
            .map_or(0, |d| d.inner.lock().unwrap().clients.len())
    }

    /// The devices go away: their nodes disappear (hotplug) and their
    /// clients see the end of their connection.
    pub fn close(&self) {
        for d in &self.devices {
            let _ = std::fs::remove_file(self.dir.join(format!("event{}", d.desc.index)));
            for c in &d.inner.lock().unwrap().clients {
                // SAFETY: shutting down a client socket we own.
                unsafe { libc::shutdown(c.sock.as_raw_fd(), libc::SHUT_RDWR) };
            }
        }
    }
}

impl Drop for Devices {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::super::codes::*;
    use super::super::*;
    use super::*;

    fn open(dir: &Path, index: u32) -> (UnixStream, Descriptor) {
        let s = connect_in(dir, &format!("event{index}")).unwrap();
        let hello = Hello {
            version: VERSION,
            op: OP_OPEN,
            ..Default::default()
        };
        wire::send(s.as_fd(), wire::bytes(&hello), None).unwrap();
        let d = wire::recv_record::<Descriptor>(s.as_fd()).unwrap().unwrap();
        (s, d)
    }

    fn read_records(s: &mut UnixStream, n: usize) -> Vec<Record> {
        let mut v = vec![Record::default(); n];
        // SAFETY: `Record` is plain old data.
        let b = unsafe {
            std::slice::from_raw_parts_mut(v.as_mut_ptr().cast::<u8>(), n * size_of::<Record>())
        };
        s.read_exact(b).unwrap();
        v
    }

    fn grab(dir: &Path, client: u64, on: bool) -> i32 {
        let mut s = connect_in(dir, "event0").unwrap();
        let hello = Hello {
            version: VERSION,
            op: OP_GRAB,
            client,
            arg: on as u64,
        };
        wire::send(s.as_fd(), wire::bytes(&hello), None).unwrap();
        let mut r = [0u8; 4];
        s.read_exact(&mut r).unwrap();
        i32::from_ne_bytes(r)
    }

    #[test]
    fn packets_state_and_grab() {
        let dir = std::env::temp_dir().join(format!("evdev-{}", std::process::id()));
        let devs = Devices::create(&dir, devices(100, 200, 160.0, 160.0)).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["event0", "event1", "event2"]);

        let (mut a, da) = open(&dir, TOUCHSCREEN);
        assert_eq!(da.name(), b"darwin-touchscreen");
        devs.emit(TOUCHSCREEN, 7, &[(EV_KEY, BTN_TOUCH, 1), (EV_KEY, 30, 1)]);
        // Nothing changes: no packet at all.
        devs.emit(TOUCHSCREEN, 8, &[(EV_KEY, BTN_TOUCH, 1)]);
        devs.emit(TOUCHSCREEN, 9, &[(EV_ABS, ABS_MT_POSITION_X, 5)]);
        let got = read_records(&mut a, 4);
        let kinds: Vec<(u16, u16, i32, i64)> = got
            .iter()
            .map(|r| (r.kind, r.code, r.value, r.time_ns))
            .collect();
        assert_eq!(
            kinds,
            [
                (EV_KEY, BTN_TOUCH, 1, 7),
                (EV_SYN, SYN_REPORT, 0, 7),
                (EV_ABS, ABS_MT_POSITION_X, 5, 9),
                (EV_SYN, SYN_REPORT, 0, 9)
            ]
        );

        // A later client starts from the current state.
        let (mut b, db) = open(&dir, TOUCHSCREEN);
        assert!(test_bit(&db.state.key, BTN_TOUCH));
        assert_eq!(db.state.mt_value(0, ABS_MT_POSITION_X), 5);

        // A grab keeps the others' events.
        assert_eq!(grab(&dir, db.client, true), 0);
        assert_eq!(grab(&dir, da.client, true), -EBUSY);
        devs.emit(TOUCHSCREEN, 10, &[(EV_KEY, BTN_TOUCH, 0)]);
        assert_eq!(read_records(&mut b, 2)[0].value, 0);
        assert_eq!(grab(&dir, da.client, false), -EINVAL);
        assert_eq!(grab(&dir, db.client, false), 0);

        // Writes are injected, through the same rules.
        let inj = [
            Record {
                kind: EV_ABS,
                code: ABS_MT_POSITION_Y,
                value: 3,
                ..Default::default()
            },
            Record {
                kind: EV_SYN,
                code: SYN_REPORT,
                ..Default::default()
            },
        ];
        b.write_all(record_bytes(&inj)).unwrap();
        let got = read_records(&mut a, 2);
        assert_eq!((got[0].code, got[0].value), (ABS_MT_POSITION_Y, 3));

        drop(devs);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let mut rest = Vec::new();
        a.read_to_end(&mut rest).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
