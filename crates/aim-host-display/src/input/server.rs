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

use super::codes::{
    ABS_CNT, ABS_MT_SLOT, EV_ABS, EV_CNT, EV_KEY, EV_SYN, KEY_MAX, KEY_RESERVED, SYN_DROPPED,
    SYN_REPORT,
};
use super::{
    AbsInfo, Descriptor, FLUSH, Hello, KeyEntry, MASK_SET, Mask, OP_DESCRIBE, OP_FLUSH, OP_GRAB,
    OP_KEYCODE, OP_MASK, OP_OPEN, OP_SET_ABS, Record, Spec, State, VERSION, is_masked, mask_codes,
    record_bytes, set_bit, test_bit,
};
use crate::wire;

const EBUSY: i32 = 16;
const EINVAL: i32 = 22;
const ENODEV: i32 = 19;

struct Client {
    id: u64,
    sock: OwnedFd,
    /// A packet was lost: send `SYN_DROPPED` first.
    dropped: AtomicBool,
    /// Event masks by type (`EVIOCSMASK`); None passes every code.
    masks: Mutex<Vec<Option<Mask>>>,
}

struct Inner {
    /// Mutable: `EVIOCSABS` changes axes, `EVIOCSKEYCODE` the keys.
    desc: Descriptor,
    state: State,
    /// Events since the last `SYN_REPORT`.
    pending: Vec<Record>,
    clients: Vec<Arc<Client>>,
    grab: Option<u64>,
    next_client: u64,
    /// Scan code and key, in the keymap's order; empty: no keymap.
    keymap: Vec<(u32, u16)>,
}

struct Device {
    index: u32,
    inner: Mutex<Inner>,
}

/// The devices of one display server.
pub struct Devices {
    dir: PathBuf,
    devices: Vec<Arc<Device>>,
    /// The device directory, locked while the devices exist: a node in an
    /// unlocked directory is stale (its server died without removing it).
    lock: Mutex<Option<std::fs::File>>,
}

impl Inner {
    /// Feed one event through the input core's rules; a `SYN_REPORT`
    /// sends the packet.
    fn event(&mut self, r: Record, time_ns: i64) {
        if r.kind == EV_SYN && r.code == SYN_REPORT {
            flush(self, time_ns);
        } else if r.kind != EV_SYN && self.desc.apply(&mut self.state, &r) {
            self.pending.push(r);
        }
    }

    fn client(&self, id: u64) -> Option<&Arc<Client>> {
        self.clients.iter().find(|c| c.id == id)
    }

    /// `EVIOCSABS`: the axis's range and value, for every open file.
    fn set_abs(&mut self, axis: u64, info: AbsInfo) -> i32 {
        if !test_bit(&self.desc.bits.ev, EV_ABS) || axis as usize >= ABS_CNT {
            return -EINVAL;
        }
        // The number of slots is fixed.
        if axis as u16 == ABS_MT_SLOT {
            return -EINVAL;
        }
        self.desc.absinfo[axis as usize] = info;
        self.state.abs[axis as usize] = info.value;
        0
    }

    /// The keymap position `e` names (`hidinput_locate_usage`).
    fn locate(&self, e: &KeyEntry) -> Option<usize> {
        if e.by_index != 0 {
            return ((e.index as usize) < self.keymap.len()).then_some(e.index as usize);
        }
        self.keymap.iter().position(|&(sc, _)| sc == e.scancode)
    }

    /// `EVIOCGKEYCODE`/`EVIOCSKEYCODE` (`input_get_keycode`,
    /// `input_set_keycode` with hid-input's keymap).
    fn keycode(&mut self, e: &mut KeyEntry, set: bool, time_ns: i64) -> i32 {
        if set && e.keycode > KEY_MAX as u32 {
            return -EINVAL;
        }
        let Some(i) = self.locate(e) else {
            return -EINVAL;
        };
        if !set {
            (e.index, e.scancode, e.keycode) =
                (i as u32, self.keymap[i].0, self.keymap[i].1 as u32);
            return 0;
        }
        let old = std::mem::replace(&mut self.keymap[i].1, e.keycode as u16);
        let keys = &mut self.desc.bits.key;
        set_bit(keys, old, false);
        set_bit(keys, e.keycode as u16, true);
        if self.keymap.iter().any(|&(_, k)| k == old) {
            set_bit(keys, old, true);
        }
        set_bit(keys, KEY_RESERVED, false);
        // A key held down that the device no longer has goes up.
        if !test_bit(&self.desc.bits.key, old) && test_bit(&self.state.key, old) {
            set_bit(&mut self.state.key, old, false);
            self.pending.push(Record {
                time_ns,
                kind: EV_KEY,
                code: old,
                value: 0,
            });
            flush(self, time_ns);
        }
        0
    }

    /// The key at scan code `scancode`.
    fn key_of(&self, scancode: u32) -> Option<u16> {
        self.keymap
            .iter()
            .find(|&&(sc, _)| sc == scancode)
            .map(|&(_, k)| k)
    }
}

impl Device {
    /// Serve one connection.
    fn serve(self: Arc<Self>, conn: UnixStream) {
        let _ = conn.set_read_timeout(Some(Duration::from_secs(5)));
        let Ok(Some(hello)) = wire::recv_record::<Hello>(conn.as_fd()) else {
            return;
        };
        if hello.version != VERSION {
            return;
        }
        let answer = |r: i32| (&conn).write_all(&r.to_ne_bytes());
        match hello.op {
            OP_OPEN => self.open(conn),
            OP_GRAB => {
                let _ = answer(self.grab(hello.client, hello.arg != 0));
            }
            OP_DESCRIBE => {
                let inner = self.inner.lock().unwrap();
                let desc = Descriptor {
                    client: 0,
                    state: inner.state,
                    ..inner.desc
                };
                drop(inner);
                let _ = wire::send(conn.as_fd(), wire::bytes(&desc), None);
            }
            OP_SET_ABS => {
                if let Ok(Some(info)) = wire::recv_record::<AbsInfo>(conn.as_fd()) {
                    let _ = answer(self.inner.lock().unwrap().set_abs(hello.arg, info));
                }
            }
            OP_MASK => {
                let kind = hello.arg as u32;
                let set = if hello.arg & MASK_SET != 0 {
                    match wire::recv_record::<Mask>(conn.as_fd()) {
                        Ok(Some(m)) => Some(m),
                        _ => return,
                    }
                } else {
                    None
                };
                let (r, mask) = self.mask(hello.client, kind, set);
                if answer(r).is_ok() {
                    let _ = wire::send(conn.as_fd(), wire::bytes(&mask), None);
                }
            }
            OP_KEYCODE => {
                let Ok(Some(mut e)) = wire::recv_record::<KeyEntry>(conn.as_fd()) else {
                    return;
                };
                let now = crate::monotonic_ns();
                let r = self
                    .inner
                    .lock()
                    .unwrap()
                    .keycode(&mut e, hello.arg != 0, now);
                if answer(r).is_ok() {
                    let _ = wire::send(conn.as_fd(), wire::bytes(&e), None);
                }
            }
            OP_FLUSH => {
                let _ = answer(self.flush_client(hello.client));
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
                ..inner.desc
            };
            if wire::send(conn.as_fd(), wire::bytes(&desc), None).is_err() {
                return;
            }
            let client = Arc::new(Client {
                id,
                sock: sock.into(),
                dropped: AtomicBool::new(false),
                masks: Mutex::new(vec![None; EV_CNT]),
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
                    inner.event(*r, now);
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
        if inner.client(id).is_none() {
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

    /// `EVIOCGMASK` (`set` None) or `EVIOCSMASK` of type `kind` for client
    /// `id`; the mask as it is after (`evdev_get_mask`: all set when none
    /// was given). A type without codes to mask is accepted and ignored.
    fn mask(&self, id: u64, kind: u32, set: Option<Mask>) -> (i32, Mask) {
        let inner = self.inner.lock().unwrap();
        let Some(c) = inner.client(id) else {
            return (-ENODEV, Mask::default());
        };
        let mut masks = c.masks.lock().unwrap();
        if mask_codes(kind) == 0 {
            return (0, Mask([0xff; 96]));
        }
        if let Some(m) = set {
            masks[kind as usize] = Some(m);
        }
        (0, masks[kind as usize].unwrap_or(Mask([0xff; 96])))
    }

    /// Drop what client `id` has queued: a [`FLUSH`] record ends it, and
    /// `SYN_DROPPED` follows (`evdev_set_clk_type`).
    fn flush_client(&self, id: u64) -> i32 {
        let inner = self.inner.lock().unwrap();
        let Some(c) = inner.client(id) else {
            return -ENODEV;
        };
        let time_ns = crate::monotonic_ns();
        let rec = |kind, code| Record {
            time_ns,
            kind,
            code,
            value: 0,
        };
        match wire::send(
            c.sock.as_fd(),
            record_bytes(&[rec(FLUSH, 0), rec(EV_SYN, SYN_DROPPED)]),
            None,
        ) {
            Ok(()) => 0,
            Err(_) => -ENODEV,
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
        let masks = c.masks.lock().unwrap();
        if masks.iter().all(Option::is_none) {
            send_packet(c, record_bytes(&packet[from..]));
            continue;
        }
        // Masked events are not queued; a packet left empty is not sent
        // (evdev drops an empty SYN_REPORT).
        let passed: Vec<Record> = packet[from..]
            .iter()
            .filter(|r| !is_masked(&masks, r.kind, r.code))
            .copied()
            .collect();
        if passed.iter().any(|r| r.kind != EV_SYN) {
            send_packet(c, record_bytes(&passed));
        }
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

/// Lock the device directory `dir`.
fn lock_dir(dir: &Path, how: libc::c_int) -> io::Result<std::fs::File> {
    let d = std::fs::File::open(dir)?;
    // SAFETY: flock on the directory we just opened.
    if unsafe { libc::flock(d.as_raw_fd(), how) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(d)
}

/// Remove the device nodes in `dir`.
fn remove_nodes(dir: &Path) -> io::Result<()> {
    for e in std::fs::read_dir(dir)?.flatten() {
        if e.file_name().to_string_lossy().starts_with("event") {
            let _ = std::fs::remove_file(e.path());
        }
    }
    Ok(())
}

/// Remove the nodes of a server that died without removing them: `dir`'s
/// nodes if no server holds it. Returns whether they were stale.
pub fn remove_stale(dir: &Path) -> bool {
    match lock_dir(dir, libc::LOCK_EX | libc::LOCK_NB) {
        Ok(_lock) => {
            let _ = remove_nodes(dir);
            true
        }
        Err(_) => false,
    }
}

impl Devices {
    /// Create the devices in `dir` (created if needed; stale device sockets
    /// of an earlier server are removed). Each socket is bound and
    /// listening before it appears in `dir`, so a client that sees it can
    /// open it. The directory stays locked while the devices exist.
    pub fn create(dir: &Path, specs: Vec<Spec>) -> io::Result<Devices> {
        std::fs::create_dir_all(dir)?;
        // A client removing stale nodes holds the lock for a moment; a
        // live server holds it for good.
        let mut tries = 0;
        let lock = loop {
            match lock_dir(dir, libc::LOCK_EX | libc::LOCK_NB) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock && tries < 40 => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(50));
                }
                r => break r?,
            }
        };
        remove_nodes(dir)?;
        let mut staging = dir.as_os_str().to_owned();
        staging.push(".new");
        let staging = PathBuf::from(staging);
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging)?;
        let mut devices = Vec::new();
        for Spec { desc, keymap } in specs {
            let name = format!("event{}", desc.index);
            let listener = super::listen_in(&staging, &name)?;
            // Access is the guest's business (the syscall layer shows the
            // node as root:input 0660).
            std::fs::set_permissions(staging.join(&name), std::fs::Permissions::from_mode(0o666))?;
            let device = Arc::new(Device {
                index: desc.index,
                inner: Mutex::new(Inner {
                    desc,
                    state: desc.state,
                    pending: Vec::new(),
                    clients: Vec::new(),
                    grab: None,
                    next_client: 1,
                    keymap,
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
            lock: Mutex::new(Some(lock)),
        })
    }

    fn device(&self, index: u32) -> Option<&Arc<Device>> {
        self.devices.iter().find(|d| d.index == index)
    }

    /// The key device `index` sends for scan code `scancode` (its keymap).
    pub fn key_of(&self, index: u32, scancode: u32) -> Option<u16> {
        self.device(index)?.inner.lock().unwrap().key_of(scancode)
    }

    /// Report `events` (type, code, value) of device `index` as one packet
    /// at `time_ns` (the guest's `CLOCK_MONOTONIC`).
    pub fn emit(&self, index: u32, time_ns: i64, events: &[(u16, u16, i32)]) {
        let Some(d) = self.device(index) else {
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
            inner.event(r, time_ns);
        }
        flush(&mut inner, time_ns);
    }

    /// How many clients have device `index` open.
    pub fn clients(&self, index: u32) -> usize {
        self.device(index)
            .map_or(0, |d| d.inner.lock().unwrap().clients.len())
    }

    /// The devices go away: their nodes disappear (hotplug), their clients
    /// see the end of their connection, and the directory is free.
    pub fn close(&self) {
        for d in &self.devices {
            let _ = std::fs::remove_file(self.dir.join(format!("event{}", d.index)));
            for c in &d.inner.lock().unwrap().clients {
                // SAFETY: shutting down a client socket we own.
                unsafe { libc::shutdown(c.sock.as_raw_fd(), libc::SHUT_RDWR) };
            }
        }
        self.lock.lock().unwrap().take();
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

    /// Control request `op` to device `index`, with `payload`; the answer.
    fn control(
        dir: &Path,
        index: u32,
        op: u32,
        client: u64,
        arg: u64,
        payload: &[u8],
    ) -> (i32, UnixStream) {
        let mut s = connect_in(dir, &format!("event{index}")).unwrap();
        let hello = Hello {
            version: VERSION,
            op,
            client,
            arg,
        };
        wire::send(s.as_fd(), wire::bytes(&hello), None).unwrap();
        if !payload.is_empty() {
            wire::send(s.as_fd(), payload, None).unwrap();
        }
        let mut r = [0u8; 4];
        s.read_exact(&mut r).unwrap();
        (i32::from_ne_bytes(r), s)
    }

    fn grab(dir: &Path, client: u64, on: bool) -> i32 {
        control(dir, TOUCHSCREEN, OP_GRAB, client, on as u64, &[]).0
    }

    fn keycode(dir: &Path, e: KeyEntry, set: bool) -> (i32, KeyEntry) {
        let (r, s) = control(dir, KEYBOARD, OP_KEYCODE, 0, set as u64, wire::bytes(&e));
        let got = if r == 0 {
            wire::recv_record::<KeyEntry>(s.as_fd()).unwrap().unwrap()
        } else {
            e
        };
        (r, got)
    }

    fn describe(dir: &Path, index: u32) -> Descriptor {
        let s = connect_in(dir, &format!("event{index}")).unwrap();
        let hello = Hello {
            version: VERSION,
            op: OP_DESCRIBE,
            ..Default::default()
        };
        wire::send(s.as_fd(), wire::bytes(&hello), None).unwrap();
        wire::recv_record::<Descriptor>(s.as_fd()).unwrap().unwrap()
    }

    #[test]
    fn masks_axes_keymap_and_flush() {
        let dir = std::env::temp_dir().join(format!("evdev-ctl-{}", std::process::id()));
        let devs = Devices::create(&dir, devices(100, 200, 160.0, 160.0)).unwrap();
        let (mut k, dk) = open(&dir, KEYBOARD);

        // A mask passing KEY_A alone: KEY_B's packet is not sent at all.
        let mut m = Mask::default();
        set_bit(&mut m.0, 30, true);
        let (r, s) = control(
            &dir,
            KEYBOARD,
            OP_MASK,
            dk.client,
            MASK_SET | EV_KEY as u64,
            wire::bytes(&m),
        );
        assert_eq!(r, 0);
        assert_eq!(wire::recv_record::<Mask>(s.as_fd()).unwrap().unwrap(), m);
        let (r, s) = control(&dir, KEYBOARD, OP_MASK, dk.client, EV_REL as u64, &[]);
        assert_eq!(r, 0);
        assert_eq!(
            wire::recv_record::<Mask>(s.as_fd()).unwrap().unwrap(),
            Mask([0xff; 96])
        );
        devs.emit(KEYBOARD, 1, &[(EV_KEY, 48, 1)]);
        devs.emit(KEYBOARD, 2, &[(EV_KEY, 48, 0), (EV_KEY, 30, 1)]);
        let got = read_records(&mut k, 2);
        assert_eq!(
            (got[0].code, got[0].value, got[1].code),
            (30, 1, SYN_REPORT)
        );
        // The device's state has KEY_B's changes all the same.
        devs.emit(KEYBOARD, 3, &[(EV_KEY, 48, 1)]);
        assert!(test_bit(&describe(&dir, KEYBOARD).state.key, 48));

        // The keymap: A's usage is KEY_A; remapped to KEY_Z it is Z, and
        // KEY_A (held) goes up, as the device no longer has it.
        let a = KeyEntry {
            scancode: 0x0007_0004,
            ..Default::default()
        };
        assert_eq!(keycode(&dir, a, false), (0, KeyEntry { keycode: 30, ..a }));
        let by_index = KeyEntry {
            by_index: 1,
            index: 0,
            ..Default::default()
        };
        assert_eq!(keycode(&dir, by_index, false).1.scancode, 0x0007_0004);
        assert_eq!(
            keycode(&dir, KeyEntry { scancode: 1, ..a }, false).0,
            -EINVAL
        );
        assert_eq!(
            keycode(
                &dir,
                KeyEntry {
                    keycode: 0x300,
                    ..a
                },
                true
            )
            .0,
            -EINVAL
        );
        assert_eq!(keycode(&dir, KeyEntry { keycode: 44, ..a }, true).0, 0);
        let got = read_records(&mut k, 2);
        assert_eq!((got[0].code, got[0].value), (30, 0));
        assert_eq!(devs.key_of(KEYBOARD, 0x0007_0004), Some(44));
        let d = describe(&dir, KEYBOARD);
        assert!(!test_bit(&d.bits.key, 30) && test_bit(&d.bits.key, 44));
        assert_eq!(keycode(&dir, a, false).1.keycode, 44);
        // Devices without a keymap have none.
        let (r, _) = control(&dir, TOUCHSCREEN, OP_KEYCODE, 0, 0, wire::bytes(&a));
        assert_eq!(r, -EINVAL);

        // An axis for every open file; the slot count cannot change.
        let info = AbsInfo {
            value: 7,
            minimum: 0,
            maximum: 49,
            resolution: 3,
            ..Default::default()
        };
        let (r, _) = control(&dir, MOUSE, OP_SET_ABS, 0, ABS_X as u64, wire::bytes(&info));
        assert_eq!(r, 0);
        let d = describe(&dir, MOUSE);
        assert_eq!(
            (
                d.absinfo[ABS_X as usize].maximum,
                d.state.abs[ABS_X as usize]
            ),
            (49, 7)
        );
        let (r, _) = control(
            &dir,
            TOUCHSCREEN,
            OP_SET_ABS,
            0,
            ABS_MT_SLOT as u64,
            wire::bytes(&info),
        );
        assert_eq!(r, -EINVAL);
        let (r, _) = control(
            &dir,
            KEYBOARD,
            OP_SET_ABS,
            0,
            ABS_X as u64,
            wire::bytes(&info),
        );
        assert_eq!(r, -EINVAL);

        // A flush: its record, then SYN_DROPPED.
        let (r, _) = control(&dir, KEYBOARD, OP_FLUSH, dk.client, 0, &[]);
        assert_eq!(r, 0);
        let got = read_records(&mut k, 2);
        assert_eq!(
            (got[0].kind, got[1].kind, got[1].code),
            (FLUSH, EV_SYN, SYN_DROPPED)
        );

        // A live server's nodes are not stale.
        assert!(!remove_stale(&dir));
        drop(devs);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_nodes_go() {
        let dir = std::env::temp_dir().join(format!("evdev-stale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("event0"), b"").unwrap();
        assert!(remove_stale(&dir));
        assert!(!dir.join("event0").exists());
        let _ = std::fs::remove_dir_all(&dir);
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
        assert_eq!(da.name(), b"aim-touchscreen");
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
