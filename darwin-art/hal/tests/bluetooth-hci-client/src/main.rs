//! Test client for the Bluetooth HAL (not part of the image): speaks HCI
//! to `IBluetoothHci/default` the way the Android stack's HAL layer does,
//! for `crates/darwin-linux-abi/tests/bluetooth.rs`.
//!
//! `bluetooth-hci-client [SCAN_SECONDS] [connect]`: reset, read the local
//! version, scan with the extended commands and print each device, then,
//! with `connect`, connect to the strongest named connectable device and
//! read its Device Name and primary services over ATT. Lines starting with
//! `ok` mark each step that passed.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use android_hardware_bluetooth::aidl::android::hardware::bluetooth::{
    IBluetoothHci::IBluetoothHci,
    IBluetoothHciCallbacks::{BnBluetoothHciCallbacks, IBluetoothHciCallbacks},
    Status::Status,
};
use binder::{BinderFeatures, Interface, Strong};

const INSTANCE: &str = "android.hardware.bluetooth.IBluetoothHci/default";

enum Packet {
    Init(Status),
    Event(Vec<u8>),
    Acl(Vec<u8>),
}

struct Callbacks(Mutex<Sender<Packet>>);

impl Interface for Callbacks {}

impl IBluetoothHciCallbacks for Callbacks {
    fn initializationComplete(&self, status: Status) -> binder::Result<()> {
        let _ = self.0.lock().unwrap().send(Packet::Init(status));
        Ok(())
    }
    fn hciEventReceived(&self, event: &[u8]) -> binder::Result<()> {
        let _ = self.0.lock().unwrap().send(Packet::Event(event.to_vec()));
        Ok(())
    }
    fn aclDataReceived(&self, data: &[u8]) -> binder::Result<()> {
        let _ = self.0.lock().unwrap().send(Packet::Acl(data.to_vec()));
        Ok(())
    }
    fn scoDataReceived(&self, _: &[u8]) -> binder::Result<()> {
        Ok(())
    }
    fn isoDataReceived(&self, _: &[u8]) -> binder::Result<()> {
        Ok(())
    }
}

struct Client {
    hci: Strong<dyn IBluetoothHci>,
    rx: Receiver<Packet>,
    /// Events that arrived while waiting for something else.
    events: Vec<Vec<u8>>,
    acl: Vec<Vec<u8>>,
}

fn u16_at(p: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([p[i], p[i + 1]])
}

fn addr_text(a: &[u8]) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        a[5], a[4], a[3], a[2], a[1], a[0]
    )
}

impl Client {
    fn next(&mut self, deadline: Instant) -> Option<Packet> {
        self.rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok()
    }

    /// Wait for an event `f` accepts; others are kept.
    fn wait_event(&mut self, timeout: Duration, f: impl Fn(&[u8]) -> bool) -> Option<Vec<u8>> {
        if let Some(i) = self.events.iter().position(|e| f(e)) {
            return Some(self.events.remove(i));
        }
        let deadline = Instant::now() + timeout;
        loop {
            match self.next(deadline)? {
                Packet::Event(e) if f(&e) => return Some(e),
                Packet::Event(e) => self.events.push(e),
                Packet::Acl(a) => self.acl.push(a),
                Packet::Init(_) => {}
            }
        }
    }

    /// Send a command; return its Command Complete parameters (status
    /// first) or Command Status.
    fn command(&mut self, opcode: u16, params: &[u8]) -> Vec<u8> {
        let mut p = opcode.to_le_bytes().to_vec();
        p.push(params.len() as u8);
        p.extend_from_slice(params);
        self.hci.sendHciCommand(&p).expect("sendHciCommand");
        let e = self
            .wait_event(Duration::from_secs(10), |e| {
                (e[0] == 0x0e && u16_at(e, 3) == opcode) || (e[0] == 0x0f && u16_at(e, 4) == opcode)
            })
            .unwrap_or_else(|| panic!("no completion for {opcode:#06x}"));
        if e[0] == 0x0e {
            e[5..].to_vec()
        } else {
            vec![e[2]]
        }
    }

    fn le_meta(&mut self, timeout: Duration, sub: u8) -> Option<Vec<u8>> {
        self.wait_event(timeout, |e| e[0] == 0x3e && e[2] == sub)
    }

    fn att(&mut self, handle: u16, pdu: &[u8]) -> Option<Vec<u8>> {
        let mut p = (handle & 0x0fff).to_le_bytes().to_vec();
        p.extend_from_slice(&((pdu.len() + 4) as u16).to_le_bytes());
        p.extend_from_slice(&(pdu.len() as u16).to_le_bytes());
        p.extend_from_slice(&4u16.to_le_bytes());
        p.extend_from_slice(pdu);
        self.hci.sendAclData(&p).expect("sendAclData");
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut frame: Vec<u8> = Vec::new();
        loop {
            let a = if self.acl.is_empty() {
                match self.next(deadline)? {
                    Packet::Acl(a) => a,
                    Packet::Event(e) => {
                        self.events.push(e);
                        continue;
                    }
                    Packet::Init(_) => continue,
                }
            } else {
                self.acl.remove(0)
            };
            let pb = u16_at(&a, 0) >> 12 & 0b11;
            if pb != 0b01 {
                frame.clear();
            }
            frame.extend_from_slice(&a[4..]);
            if frame.len() >= 4 && frame.len() >= 4 + u16_at(&frame, 0) as usize {
                if u16_at(&frame, 2) == 4 {
                    return Some(frame[4..].to_vec());
                }
                frame.clear();
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let scan_seconds: u64 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(8);
    let connect = args.iter().any(|a| a == "connect");

    binder::ProcessState::start_thread_pool();
    let hci: Strong<dyn IBluetoothHci> =
        binder::wait_for_interface(INSTANCE).expect("IBluetoothHci/default");
    let (tx, rx) = channel();
    let callbacks =
        BnBluetoothHciCallbacks::new_binder(Callbacks(Mutex::new(tx)), BinderFeatures::default());
    hci.initialize(&callbacks).expect("initialize");
    let mut c = Client {
        hci,
        rx,
        events: Vec::new(),
        acl: Vec::new(),
    };
    match c.next(Instant::now() + Duration::from_secs(10)) {
        Some(Packet::Init(Status::SUCCESS)) => println!("ok initialized"),
        Some(Packet::Init(s)) => panic!("initializationComplete({s:?})"),
        _ => panic!("no initializationComplete"),
    }

    assert_eq!(c.command(0x0c03, &[]), vec![0], "reset");
    println!("ok reset");
    let v = c.command(0x1001, &[]);
    assert_eq!(v[0], 0);
    println!(
        "ok version hci={:#04x} lmp={:#04x} manufacturer={:#06x}",
        v[1],
        v[4],
        u16_at(&v, 5)
    );
    let addr = c.command(0x1009, &[]);
    println!("ok bd_addr {}", addr_text(&addr[1..7]));
    let features = c.command(0x1003, &[]);
    println!(
        "ok features le={} br_edr_not_supported={}",
        features[5] & 0x40 != 0,
        features[5] & 0x20 != 0
    );
    assert_eq!(c.command(0x0c01, &[0xff; 8]), vec![0]);
    assert_eq!(c.command(0x2001, &[0xff; 8]), vec![0]);

    // Extended scanning, as the stack does when the controller has LE
    // Extended Advertising: active, 1M PHY, no duplicate filtering.
    assert_eq!(
        c.command(0x2041, &[0x01, 0x00, 0x01, 0x01, 0x12, 0x00, 0x12, 0x00]),
        vec![0]
    );
    assert_eq!(c.command(0x2042, &[1, 0, 0, 0, 0, 0]), vec![0]);
    // Address -> (rssi, name, connectable).
    let mut devices: HashMap<[u8; 6], (i8, String, bool)> = HashMap::new();
    let deadline = Instant::now() + Duration::from_secs(scan_seconds);
    let mut reports = 0;
    while let Some(e) = c.le_meta(deadline.saturating_duration_since(Instant::now()), 0x0d) {
        reports += 1;
        let r = &e[4..];
        let kind = u16_at(r, 0);
        let addr: [u8; 6] = r[3..9].try_into().unwrap();
        let rssi = r[13] as i8;
        let data = &r[24..24 + r[23] as usize];
        let mut name = String::new();
        let mut rest = data;
        while let [len, tail @ ..] = rest {
            let len = *len as usize;
            if len == 0 || tail.len() < len {
                break;
            }
            if tail[0] == 0x08 || tail[0] == 0x09 {
                name = String::from_utf8_lossy(&tail[1..len]).into_owned();
            }
            rest = &tail[len..];
        }
        let d = devices.entry(addr).or_insert((rssi, String::new(), false));
        d.0 = rssi;
        if !name.is_empty() {
            d.1 = name;
        }
        d.2 |= kind & 1 != 0;
    }
    assert_eq!(c.command(0x2042, &[0, 0, 0, 0, 0, 0]), vec![0]);
    let mut list: Vec<_> = devices.iter().collect();
    list.sort_by_key(|(_, d)| -(d.0 as i32));
    for (a, (rssi, name, connectable)) in &list {
        println!(
            "device {} rssi={rssi} connectable={connectable} name={name:?}",
            addr_text(*a)
        );
    }
    println!("ok scan reports={reports} devices={}", list.len());

    if connect {
        match list.iter().find(|(_, d)| d.2 && !d.1.is_empty()) {
            Some((a, (_, name, _))) => gatt(&mut c, **a, name),
            None => println!("skip connect: no named connectable device"),
        }
    }
    c.hci.close().expect("close");
    println!("ok done");
}

fn gatt(c: &mut Client, addr: [u8; 6], advertised: &str) {
    println!("connecting {} ({advertised})", addr_text(&addr));
    let mut p = vec![0x00, 0x00, 0x01];
    p.extend_from_slice(&addr);
    p.push(0x01);
    p.extend_from_slice(&[
        0x60, 0x00, 0x30, 0x00, 0x18, 0x00, 0x28, 0x00, 0x00, 0x00, 0xf4, 0x01, 0, 0, 0, 0,
    ]);
    assert_eq!(c.command(0x2043, &p), vec![0]);
    let Some(e) = c.le_meta(Duration::from_secs(40), 0x0a) else {
        assert_eq!(c.command(0x200e, &[]), vec![0]);
        println!("connect-failed timeout");
        return;
    };
    if e[3] != 0 {
        println!("connect-failed status={:#04x}", e[3]);
        return;
    }
    let handle = u16_at(&e, 4);
    println!("ok connected handle={handle:#06x}");

    // Exchange MTU, then the Device Name by type, as the stack's GATT
    // client reads it.
    if let Some(r) = c.att(handle, &[0x02, 0x00, 0x02]) {
        println!("ok mtu {}", u16_at(&r, 1));
    }
    match c.att(handle, &[0x08, 0x01, 0x00, 0xff, 0xff, 0x00, 0x2a]) {
        Some(r) if r[0] == 0x09 => {
            let name = String::from_utf8_lossy(&r[4..]);
            println!("ok device-name {name:?} handle={}", u16_at(&r, 2));
        }
        other => println!("device-name failed: {other:02x?}"),
    }
    // Primary services, the way GATT discovery walks them.
    let mut start = 1u16;
    let mut services = Vec::new();
    while let Some(r) = c.att(
        handle,
        &[
            0x10,
            start as u8,
            (start >> 8) as u8,
            0xff,
            0xff,
            0x00,
            0x28,
        ],
    ) {
        if r[0] != 0x11 {
            break;
        }
        let len = r[1] as usize;
        let mut end = 0;
        for e in r[2..].chunks_exact(len) {
            end = u16_at(e, 2);
            let uuid: Vec<String> = e[4..].iter().rev().map(|b| format!("{b:02x}")).collect();
            services.push(format!(
                "{:#06x}-{end:#06x}:{}",
                u16_at(e, 0),
                uuid.concat()
            ));
        }
        if end == 0xffff {
            break;
        }
        start = end + 1;
    }
    println!("ok services {}", services.join(" "));

    let h = handle.to_le_bytes();
    assert_eq!(c.command(0x0406, &[h[0], h[1], 0x13]), vec![0]);
    match c.wait_event(Duration::from_secs(10), |e| e[0] == 0x05) {
        Some(e) => println!("ok disconnected reason={:#04x}", e[5]),
        None => println!("disconnect: no Disconnection Complete"),
    }
}
