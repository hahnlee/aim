//! The controller against a fake radio: the HCI dialogue the Android stack
//! holds at start-up, scanning, a connection with GATT over ATT, and
//! advertising.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::*;
use crate::att::Op;
use crate::gatt::{CCCD, Characteristic, Descriptor, Service, Target, Uuid, props};

#[derive(Debug, PartialEq, Eq)]
enum Call {
    Scan(bool, bool),
    Connect(PeerId),
    Disconnect(PeerId),
    Discover(PeerId),
    Gatt(PeerId, Op),
    Advertise(Option<AdvertiseRequest>),
}

#[derive(Clone, Default)]
pub(crate) struct Fake(Arc<Mutex<Vec<Call>>>);

impl Fake {
    fn take(&self) -> Vec<Call> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

impl Backend for Fake {
    fn scan(&mut self, on: bool, duplicates: bool) {
        self.0.lock().unwrap().push(Call::Scan(on, duplicates));
    }
    fn connect(&mut self, peer: PeerId) {
        self.0.lock().unwrap().push(Call::Connect(peer));
    }
    fn disconnect(&mut self, peer: PeerId) {
        self.0.lock().unwrap().push(Call::Disconnect(peer));
    }
    fn discover(&mut self, peer: PeerId) {
        self.0.lock().unwrap().push(Call::Discover(peer));
    }
    fn gatt(&mut self, peer: PeerId, op: Op) {
        self.0.lock().unwrap().push(Call::Gatt(peer, op));
    }
    fn advertise(&mut self, request: Option<AdvertiseRequest>) {
        self.0.lock().unwrap().push(Call::Advertise(request));
    }
}

const LOCAL: Addr = Addr([1, 2, 3, 4, 5, 0x02]);
const PEER: Addr = Addr([0x11, 0x22, 0x33, 0x44, 0x55, 0xc6]);

/// A controller whose host enabled every event, LE Meta included (the
/// default mask lacks it), as the Android stack does.
fn new() -> (Controller, Fake) {
    let fake = Fake::default();
    let mut c = Controller::new(Box::new(fake.clone()), LOCAL);
    complete(&mut c, cmd::SET_EVENT_MASK, &[0xff; 8]);
    (c, fake)
}

fn command(c: &mut Controller, opcode: u16, params: &[u8]) -> Vec<Vec<u8>> {
    let mut p = opcode.to_le_bytes().to_vec();
    p.push(params.len() as u8);
    p.extend_from_slice(params);
    c.command(&p);
    events(c)
}

fn events(c: &mut Controller) -> Vec<Vec<u8>> {
    c.drain()
        .into_iter()
        .map(|(k, p)| {
            assert_eq!(k, kind::EVENT, "unexpected ACL {p:02x?}");
            p
        })
        .collect()
}

/// The Command Complete parameters of a command (status first).
fn complete(c: &mut Controller, opcode: u16, params: &[u8]) -> Vec<u8> {
    let e = command(c, opcode, params);
    assert_eq!(e.len(), 1, "{e:02x?}");
    assert_eq!(e[0][0], ev::COMMAND_COMPLETE);
    assert_eq!(u16_at(&e[0], 3), opcode);
    e[0][5..].to_vec()
}

fn advertisement(name: &str, connectable: bool) -> Event {
    Event::Advertisement {
        peer: 9,
        addr: PEER,
        rssi: -60,
        adv: Advertisement {
            connectable,
            local_name: Some(name.into()),
            services: vec![Uuid::short(0x180d)],
            ..Default::default()
        },
    }
}

#[test]
fn start_up_dialogue_of_the_android_stack() {
    let (mut c, _) = new();
    // Controller::impl::Start in system/gd/hci/controller.cc, in order;
    // each must succeed.
    assert_eq!(complete(&mut c, cmd::RESET, &[]), vec![0]);
    assert_eq!(complete(&mut c, cmd::SET_EVENT_MASK, &[0xff; 8]), vec![0]);
    assert_eq!(
        complete(&mut c, cmd::SET_EVENT_MASK_PAGE_2, &[0; 8]),
        vec![0]
    );
    assert_eq!(
        complete(&mut c, cmd::WRITE_LE_HOST_SUPPORT, &[1, 0]),
        vec![0]
    );
    let name = complete(&mut c, cmd::READ_LOCAL_NAME, &[]);
    assert_eq!((name[0], name.len()), (0, 249));
    assert_eq!(
        complete(&mut c, cmd::READ_LOCAL_VERSION_INFORMATION, &[]),
        vec![0, 0x0c, 0, 0, 0x0c, 0xff, 0xff, 0, 0]
    );
    let commands = complete(&mut c, cmd::READ_LOCAL_SUPPORTED_COMMANDS, &[]);
    assert_eq!(commands.len(), 65);
    assert_eq!(commands[1 + 5] & 0x80, 0x80); // HCI_Reset
    let le = complete(&mut c, cmd::LE_READ_LOCAL_SUPPORTED_FEATURES, &[]);
    assert_eq!(le, vec![0, 0, 0x10, 0, 0, 0, 0, 0, 0]);
    assert_eq!(complete(&mut c, cmd::LE_READ_SUPPORTED_STATES, &[])[0], 0);
    let f = complete(&mut c, cmd::READ_LOCAL_EXTENDED_FEATURES, &[0]);
    // Page 0 of 0: LE supported, BR/EDR not supported, SSP.
    assert_eq!(f, vec![0, 0, 0, 0, 0, 0, 0, 0x60, 0, 0x08, 0]);
    assert_eq!(
        complete(&mut c, cmd::LE_SET_EVENT_MASK, &[0xff; 8]),
        vec![0]
    );
    assert_eq!(
        complete(&mut c, cmd::READ_BUFFER_SIZE, &[]),
        vec![0, 251, 0, 0, 16, 0, 0, 0]
    );
    assert_eq!(
        complete(&mut c, cmd::LE_READ_BUFFER_SIZE_V1, &[]),
        vec![0, 251, 0, 16]
    );
    assert_eq!(
        complete(&mut c, cmd::LE_READ_FILTER_ACCEPT_LIST_SIZE, &[]),
        vec![0, 16]
    );
    assert_eq!(
        complete(&mut c, cmd::WRITE_SIMPLE_PAIRING_MODE, &[1]),
        vec![0]
    );
    assert_eq!(
        complete(&mut c, cmd::LE_READ_MAXIMUM_ADVERTISING_DATA_LENGTH, &[]),
        vec![0, 31, 0]
    );
    assert_eq!(
        complete(
            &mut c,
            cmd::LE_READ_NUMBER_OF_SUPPORTED_ADVERTISING_SETS,
            &[]
        ),
        vec![0, 1]
    );
    // The vendor capabilities command is unknown: the stack then takes
    // none.
    assert_eq!(complete(&mut c, 0xfd53, &[]), vec![status::UNKNOWN_COMMAND]);
    assert_eq!(
        complete(&mut c, cmd::READ_BD_ADDR, &[]),
        vec![0, 1, 2, 3, 4, 5, 0x02]
    );
    // Local name round trip.
    let mut n = [0u8; 248];
    n[..4].copy_from_slice(b"Mac!");
    assert_eq!(complete(&mut c, cmd::WRITE_LOCAL_NAME, &n), vec![0]);
    assert_eq!(&complete(&mut c, cmd::READ_LOCAL_NAME, &[])[1..5], b"Mac!");
    // Classic operations are unknown commands.
    assert_eq!(
        complete(&mut c, 0x0401, &[0x33, 0x8b, 0x9e, 8, 0]),
        vec![status::UNKNOWN_COMMAND]
    );
    // A short parameter block.
    assert_eq!(
        complete(&mut c, cmd::SET_EVENT_MASK, &[0; 4]),
        vec![status::INVALID_PARAMETERS]
    );
}

#[test]
fn legacy_scan_reports() {
    let (mut c, fake) = new();
    assert_eq!(
        complete(
            &mut c,
            cmd::LE_SET_SCAN_PARAMETERS,
            &[1, 0x10, 0, 0x10, 0, 0, 0]
        ),
        vec![0]
    );
    assert_eq!(complete(&mut c, cmd::LE_SET_SCAN_ENABLE, &[1, 1]), vec![0]);
    assert_eq!(fake.take(), vec![Call::Scan(true, false)]);
    c.on_event(advertisement("HR", true));
    let e = events(&mut c);
    assert_eq!(e.len(), 1);
    let r = &e[0];
    // LE Meta, Advertising Report: one report, ADV_IND, random address.
    assert_eq!(&r[..5], &[0x3e, r[1], 0x02, 1, 0x00]);
    assert_eq!(r[5], 1);
    assert_eq!(&r[6..12], &PEER.0);
    let data = &r[13..13 + r[12] as usize];
    assert_eq!(data, &[3, 0x03, 0x0d, 0x18, 3, 0x09, b'H', b'R']);
    assert_eq!(*r.last().unwrap() as i8, -60);
    // Duplicates are filtered as asked.
    c.on_event(advertisement("HR", true));
    assert!(events(&mut c).is_empty());
    // Parameters cannot change while scanning.
    assert_eq!(
        complete(
            &mut c,
            cmd::LE_SET_SCAN_PARAMETERS,
            &[1, 0x10, 0, 0x10, 0, 0, 0]
        ),
        vec![status::COMMAND_DISALLOWED]
    );
    assert_eq!(complete(&mut c, cmd::LE_SET_SCAN_ENABLE, &[0, 0]), vec![0]);
    assert_eq!(fake.take(), vec![Call::Scan(false, false)]);
    c.on_event(advertisement("HR", true));
    assert!(events(&mut c).is_empty());
}

#[test]
fn extended_scan_reports() {
    let (mut c, fake) = new();
    complete(&mut c, cmd::LE_SET_EVENT_MASK, &[0xff; 8]);
    assert_eq!(
        complete(
            &mut c,
            cmd::LE_SET_EXTENDED_SCAN_PARAMETERS,
            &[1, 0, 1, 1, 0x12, 0, 0x12, 0]
        ),
        vec![0]
    );
    assert_eq!(
        complete(
            &mut c,
            cmd::LE_SET_EXTENDED_SCAN_ENABLE,
            &[1, 0, 0, 0, 0, 0]
        ),
        vec![0]
    );
    assert_eq!(fake.take(), vec![Call::Scan(true, true)]);
    c.on_event(advertisement("HR", true));
    let e = events(&mut c);
    assert_eq!(e.len(), 1);
    let r = &e[0];
    assert_eq!(&r[2..4], &[0x0d, 1]);
    // Connectable, scannable legacy PDU; complete.
    assert_eq!(u16_at(r, 4), 0b10011);
    assert_eq!(&r[7..13], &PEER.0);
    assert_eq!(r[17] as i8, -60);
    assert_eq!(r[27], 8);
    // No duplicate filtering: every advertisement.
    c.on_event(advertisement("HR", true));
    assert_eq!(events(&mut c).len(), 1);

    // Data beyond a legacy PDU in one extended report, beyond 229 bytes in
    // fragments.
    let long = Event::Advertisement {
        peer: 9,
        addr: PEER,
        rssi: -70,
        adv: Advertisement {
            connectable: false,
            manufacturer: Some(vec![7; 240]),
            ..Default::default()
        },
    };
    c.on_event(long);
    let e = events(&mut c);
    assert_eq!(e.len(), 2);
    assert_eq!(u16_at(&e[0], 4), 0b01 << 5);
    assert_eq!(e[0][27], 229);
    assert_eq!(u16_at(&e[1], 4), 0);
    assert_eq!(e[1][27], 242 - 229);
    assert_eq!(e[0][1] as usize + 2, e[0].len());
}

#[test]
fn scan_reports_only_within_windows() {
    let (mut c, _) = new();
    let t0 = Instant::now();
    let now = Arc::new(Mutex::new(t0));
    let clock = now.clone();
    c.now = Box::new(move || *clock.lock().unwrap());
    let at = |ms| *now.lock().unwrap() = t0 + std::time::Duration::from_millis(ms);
    // Windows must be 4 to `interval` units; extended PHYs are 1M and Coded.
    for (opcode, params) in [
        (
            cmd::LE_SET_SCAN_PARAMETERS,
            &[1, 0x10, 0, 0x11, 0, 0, 0][..],
        ),
        (cmd::LE_SET_SCAN_PARAMETERS, &[1, 0x10, 0, 0x03, 0, 0, 0]),
        (cmd::LE_SET_SCAN_PARAMETERS, &[1, 0x01, 0x40, 0x10, 0, 0, 0]),
        (
            cmd::LE_SET_EXTENDED_SCAN_PARAMETERS,
            &[1, 0, 0b010, 1, 0x10, 0, 0x10, 0],
        ),
        (
            cmd::LE_SET_EXTENDED_SCAN_PARAMETERS,
            &[1, 0, 0b101, 1, 0x10, 0, 0x10, 0],
        ),
    ] {
        assert_eq!(
            complete(&mut c, opcode, params),
            vec![status::INVALID_PARAMETERS],
            "{params:02x?}"
        );
    }
    // Windows of 80 ms every 1.28 s, reporting duplicates.
    assert_eq!(
        complete(
            &mut c,
            cmd::LE_SET_SCAN_PARAMETERS,
            &[1, 0x00, 0x08, 0x80, 0, 0, 0]
        ),
        vec![0]
    );
    assert_eq!(complete(&mut c, cmd::LE_SET_SCAN_ENABLE, &[1, 0]), vec![0]);
    for (ms, reported) in [
        (10, true),
        (79, true),
        (81, false),
        (1000, false),
        (1285, true),
        (1370, false),
    ] {
        at(ms);
        c.on_event(advertisement("HR", true));
        assert_eq!(events(&mut c).len(), usize::from(reported), "at {ms} ms");
    }
    // A scan enabled later has its windows from then on.
    complete(&mut c, cmd::LE_SET_SCAN_ENABLE, &[0, 0]);
    at(2000);
    complete(&mut c, cmd::LE_SET_SCAN_ENABLE, &[1, 0]);
    at(2010);
    c.on_event(advertisement("HR", true));
    assert_eq!(events(&mut c).len(), 1);
}

fn services() -> Vec<Service> {
    vec![Service {
        uuid: Uuid::short(0x180d),
        primary: true,
        characteristics: vec![Characteristic {
            id: 1,
            uuid: Uuid::short(0x2a37),
            properties: props::NOTIFY | props::READ,
            descriptors: vec![Descriptor { id: 2, uuid: CCCD }],
        }],
    }]
}

/// Scan, then connect to PEER with LE Create Connection; returns the link
/// handle.
fn connect(c: &mut Controller, fake: &Fake) -> u16 {
    c.on_event(advertisement("HR", true));
    fake.take();
    let mut p = vec![0x60, 0, 0x30, 0, 0, 1];
    p.extend_from_slice(&PEER.0);
    p.extend_from_slice(&[0, 0x18, 0, 0x28, 0, 0, 0, 0xf4, 0x01, 0, 0, 0, 0]);
    let e = command(c, cmd::LE_CREATE_CONNECTION, &p);
    assert_eq!(e, vec![hci::command_status(cmd::LE_CREATE_CONNECTION, 0)]);
    // The device is known from the scan: connect at once.
    assert_eq!(fake.take(), vec![Call::Connect(9)]);
    c.on_event(Event::Connected {
        peer: 9,
        name: "Band".into(),
        mtu: 185,
    });
    assert_eq!(fake.take(), vec![Call::Discover(9)]);
    assert!(events(c).is_empty());
    c.on_event(Event::Discovered {
        peer: 9,
        services: Ok(services()),
    });
    let e = events(c);
    assert_eq!(e.len(), 1);
    let r = &e[0];
    // LE Connection Complete: success, central, the peer, the requested
    // interval max, latency and timeout.
    assert_eq!(&r[2..4], &[le::CONNECTION_COMPLETE, 0]);
    let handle = u16_at(r, 4);
    assert_eq!(&r[6..8], &[0, 1]);
    assert_eq!(&r[8..14], &PEER.0);
    assert_eq!(&r[14..20], &[0x28, 0, 0, 0, 0xf4, 0x01]);
    handle
}

/// An ATT PDU from the host on `handle`; returns the ATT PDUs back and the
/// events.
fn att(c: &mut Controller, handle: u16, pdu: &[u8]) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let mut frame = vec![];
    for p in l2cap::fragment(handle, l2cap::CID_ATT, pdu, 251) {
        let mut p = p;
        // Host packets start non-flushable.
        p[1] &= 0x0f;
        frame = p;
    }
    c.acl(&frame);
    take_acl(c)
}

fn take_acl(c: &mut Controller) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let (mut acl, mut evs) = (vec![], vec![]);
    for (k, p) in c.drain() {
        if k == kind::ACL {
            assert_eq!(u16_at(&p, 6), l2cap::CID_ATT);
            acl.push(p[8..].to_vec());
        } else {
            evs.push(p);
        }
    }
    (acl, evs)
}

#[test]
fn connection_and_gatt() {
    let (mut c, fake) = new();
    let h = connect(&mut c, &fake);
    let nocp = hci::event(ev::NUMBER_OF_COMPLETED_PACKETS, &[1, h as u8, 0, 1, 0]);

    // Exchange MTU: the host's MTU.
    let (acl, evs) = att(&mut c, h, &[0x02, 0x00, 0x02]);
    assert_eq!(acl, vec![vec![0x03, 185, 0]]);
    assert_eq!(evs, vec![nocp.clone()]);

    // Device Name by type: the GAP service is rebuilt.
    let (acl, _) = att(&mut c, h, &[0x08, 1, 0, 0xff, 0xff, 0x00, 0x2a]);
    assert_eq!(acl, vec![vec![0x09, 6, 3, 0, b'B', b'a', b'n', b'd']]);

    // A read goes to the device.
    let (acl, _) = att(&mut c, h, &[0x0a, 10, 0]);
    assert!(acl.is_empty());
    assert_eq!(fake.take(), vec![Call::Gatt(9, Op::Read(Target::Char(1)))]);
    c.on_event(Event::Read {
        peer: 9,
        target: Target::Char(1),
        result: Ok(vec![0x06, 70]),
    });
    assert_eq!(take_acl(&mut c).0, vec![vec![0x0b, 0x06, 70]]);

    // Subscribe, then a notification.
    let (acl, _) = att(&mut c, h, &[0x12, 11, 0, 1, 0]);
    assert!(acl.is_empty());
    assert_eq!(
        fake.take(),
        vec![Call::Gatt(
            9,
            Op::SetNotify {
                char: 1,
                enable: true
            }
        )]
    );
    c.on_event(Event::NotifyState {
        peer: 9,
        char: 1,
        result: Ok(()),
    });
    assert_eq!(take_acl(&mut c).0, vec![vec![0x13]]);
    c.on_event(Event::Value {
        peer: 9,
        char: 1,
        value: vec![0x06, 72],
    });
    assert_eq!(take_acl(&mut c).0, vec![vec![0x1b, 10, 0, 0x06, 72]]);

    // Pairing is macOS's: the Security Manager refuses.
    let mut smp = vec![h as u8, 0, 11, 0, 7, 0, 6, 0];
    smp.extend_from_slice(&[0x01, 3, 0, 1, 16, 7, 7]);
    c.acl(&smp);
    let out = c.drain();
    assert_eq!(out[0], (kind::EVENT, nocp.clone()));
    assert_eq!(
        out[1],
        (kind::ACL, vec![h as u8, 0x20, 6, 0, 2, 0, 6, 0, 0x05, 0x05])
    );

    // The host disconnects.
    assert_eq!(
        command(&mut c, cmd::DISCONNECT, &[h as u8, 0, 0x13]),
        vec![hci::command_status(cmd::DISCONNECT, 0)]
    );
    assert_eq!(fake.take(), vec![Call::Disconnect(9)]);
    c.on_event(Event::Disconnected {
        peer: 9,
        reason: status::REMOTE_USER_TERMINATED,
    });
    assert_eq!(
        events(&mut c),
        vec![hci::event(
            ev::DISCONNECTION_COMPLETE,
            &[0, h as u8, 0, status::LOCAL_HOST_TERMINATED]
        )]
    );
    // Packets to a gone link are dropped.
    c.acl(&[h as u8, 0, 5, 0, 1, 0, 4, 0, 0x02]);
    assert!(c.drain().is_empty());
}

#[test]
fn remote_disconnect_and_services_changed() {
    let (mut c, fake) = new();
    let h = connect(&mut c, &fake);
    // Subscribe to Service Changed (value 6, CCCD 7).
    assert_eq!(att(&mut c, h, &[0x12, 7, 0, 2, 0]).0, vec![vec![0x13]]);
    c.on_event(Event::Discovered {
        peer: 9,
        services: Ok(vec![]),
    });
    assert_eq!(take_acl(&mut c).0, vec![vec![0x1d, 6, 0, 1, 0, 11, 0]]);
    c.on_event(Event::Disconnected {
        peer: 9,
        reason: status::REMOTE_USER_TERMINATED,
    });
    assert_eq!(
        events(&mut c),
        vec![hci::event(
            ev::DISCONNECTION_COMPLETE,
            &[0, h as u8, 0, status::REMOTE_USER_TERMINATED]
        )]
    );
}

#[test]
fn accept_list_connection_waits_for_the_device() {
    let (mut c, fake) = new();
    let mut add = vec![1];
    add.extend_from_slice(&PEER.0);
    assert_eq!(
        complete(&mut c, cmd::LE_ADD_DEVICE_TO_FILTER_ACCEPT_LIST, &add),
        vec![0]
    );
    // Extended create connection, accept list, one PHY.
    let mut p = vec![1, 0, 0, 0, 0, 0, 0, 0, 0, 1];
    p.extend_from_slice(&[
        0x60, 0, 0x30, 0, 0x18, 0, 0x28, 0, 0, 0, 0xf4, 0x01, 0, 0, 0, 0,
    ]);
    let e = command(&mut c, cmd::LE_EXTENDED_CREATE_CONNECTION, &p);
    assert_eq!(
        e,
        vec![hci::command_status(cmd::LE_EXTENDED_CREATE_CONNECTION, 0)]
    );
    // Unknown device: scan for it.
    assert_eq!(fake.take(), vec![Call::Scan(true, true)]);
    // The list is in use.
    assert_eq!(
        complete(&mut c, cmd::LE_CLEAR_FILTER_ACCEPT_LIST, &[]),
        vec![status::COMMAND_DISALLOWED]
    );
    // A non-connectable advertisement does not start a connection.
    c.on_event(advertisement("HR", false));
    assert!(fake.take().is_empty());
    c.on_event(advertisement("HR", true));
    assert_eq!(fake.take(), vec![Call::Connect(9)]);
    assert!(events(&mut c).is_empty());
    c.on_event(Event::Connected {
        peer: 9,
        name: "HR".into(),
        mtu: 23,
    });
    assert_eq!(
        fake.take(),
        vec![Call::Discover(9), Call::Scan(false, false)]
    );
    c.on_event(Event::Discovered {
        peer: 9,
        services: Ok(services()),
    });
    let e = events(&mut c);
    // Enhanced Connection Complete (the default mask lacks it, so legacy).
    assert_eq!(e[0][2], le::CONNECTION_COMPLETE);
    assert_eq!(&e[0][14..20], &[0x28, 0, 0, 0, 0xf4, 0x01]);

    // With the event unmasked, the extended command reports the enhanced
    // event.
    let (mut c, fake) = new();
    complete(
        &mut c,
        cmd::LE_SET_EVENT_MASK,
        &[0xff, 0xff, 0x0f, 0, 0, 0, 0, 0],
    );
    let mut p = vec![0, 0, 1];
    p.extend_from_slice(&PEER.0);
    p.push(1);
    p.extend_from_slice(&[
        0x60, 0, 0x30, 0, 0x18, 0, 0x28, 0, 0, 0, 0xf4, 0x01, 0, 0, 0, 0,
    ]);
    command(&mut c, cmd::LE_EXTENDED_CREATE_CONNECTION, &p);
    assert_eq!(fake.take(), vec![Call::Scan(true, true)]);
    c.on_event(advertisement("HR", true));
    c.on_event(Event::ConnectFailed { peer: 9 });
    let e = events(&mut c);
    assert_eq!(e.len(), 1);
    assert_eq!(
        &e[0][2..4],
        &[
            le::ENHANCED_CONNECTION_COMPLETE,
            status::CONNECTION_FAILED_TO_BE_ESTABLISHED
        ]
    );
    assert_eq!(e[0].len(), 2 + 31);
}

#[test]
fn cancel_connection() {
    let (mut c, fake) = new();
    assert_eq!(
        complete(&mut c, cmd::LE_CREATE_CONNECTION_CANCEL, &[]),
        vec![status::COMMAND_DISALLOWED]
    );
    c.on_event(advertisement("HR", true));
    let mut p = vec![0x60, 0, 0x30, 0, 0, 1];
    p.extend_from_slice(&PEER.0);
    p.extend_from_slice(&[0, 0x18, 0, 0x28, 0, 0, 0, 0xf4, 0x01, 0, 0, 0, 0]);
    command(&mut c, cmd::LE_CREATE_CONNECTION, &p);
    fake.take();
    let e = command(&mut c, cmd::LE_CREATE_CONNECTION_CANCEL, &[]);
    assert_eq!(
        e[0],
        hci::command_complete(cmd::LE_CREATE_CONNECTION_CANCEL, &[0])
    );
    assert_eq!(
        &e[1][2..4],
        &[le::CONNECTION_COMPLETE, status::UNKNOWN_CONNECTION]
    );
    assert_eq!(fake.take(), vec![Call::Disconnect(9)]);
    // A late connection is dropped.
    c.on_event(Event::Connected {
        peer: 9,
        name: String::new(),
        mtu: 23,
    });
    assert_eq!(fake.take(), vec![Call::Disconnect(9)]);
    assert!(events(&mut c).is_empty());
}

#[test]
fn reset_drops_everything_silently() {
    let (mut c, fake) = new();
    let h = connect(&mut c, &fake);
    complete(&mut c, cmd::LE_SET_SCAN_ENABLE, &[1, 0]);
    assert_eq!(fake.take(), vec![Call::Scan(true, true)]);
    assert_eq!(complete(&mut c, cmd::RESET, &[]), vec![0]);
    assert_eq!(
        fake.take(),
        vec![Call::Disconnect(9), Call::Scan(false, false)]
    );
    c.on_event(Event::Disconnected { peer: 9, reason: 0 });
    assert!(c.drain().is_empty());
    assert_eq!(
        command(&mut c, cmd::DISCONNECT, &[h as u8, 0, 0x13]),
        vec![hci::command_status(
            cmd::DISCONNECT,
            status::UNKNOWN_CONNECTION
        )]
    );
}

#[test]
fn advertising_maps_name_and_services() {
    let (mut c, fake) = new();
    // Extended advertising set 0: parameters, data in two fragments, enable.
    let mut params = vec![0; 25];
    params[1] = 0x13;
    assert_eq!(
        complete(&mut c, cmd::LE_SET_EXTENDED_ADVERTISING_PARAMETERS, &params),
        vec![0, 0]
    );
    params[0] = 1;
    assert_eq!(
        complete(&mut c, cmd::LE_SET_EXTENDED_ADVERTISING_PARAMETERS, &params),
        vec![status::MEMORY_CAPACITY_EXCEEDED, 0]
    );
    assert_eq!(
        complete(
            &mut c,
            cmd::LE_SET_EXTENDED_ADVERTISING_DATA,
            &[0, 1, 1, 4, 3, 0x03, 0x0f, 0x18]
        ),
        vec![0]
    );
    assert_eq!(
        complete(
            &mut c,
            cmd::LE_SET_EXTENDED_ADVERTISING_DATA,
            &[0, 2, 1, 5, 4, 0x09, b'M', b'a', b'c']
        ),
        vec![0]
    );
    assert!(fake.take().is_empty());
    assert_eq!(
        complete(
            &mut c,
            cmd::LE_SET_EXTENDED_ADVERTISING_ENABLE,
            &[1, 1, 0, 0, 0, 0]
        ),
        vec![0]
    );
    assert_eq!(
        fake.take(),
        vec![Call::Advertise(Some(AdvertiseRequest {
            local_name: Some("Mac".into()),
            services: vec![Uuid::short(0x180f)],
        }))]
    );
    assert_eq!(
        complete(&mut c, cmd::LE_REMOVE_ADVERTISING_SET, &[0]),
        vec![status::COMMAND_DISALLOWED]
    );
    assert_eq!(
        complete(&mut c, cmd::LE_SET_EXTENDED_ADVERTISING_ENABLE, &[0, 0]),
        vec![0]
    );
    assert_eq!(fake.take(), vec![Call::Advertise(None)]);
    assert_eq!(
        complete(&mut c, cmd::LE_CLEAR_ADVERTISING_SETS, &[]),
        vec![0]
    );
    assert_eq!(
        complete(
            &mut c,
            cmd::LE_SET_EXTENDED_ADVERTISING_ENABLE,
            &[1, 1, 0, 0, 0, 0]
        ),
        vec![status::UNKNOWN_ADVERTISING_IDENTIFIER]
    );

    // Legacy advertising.
    let mut data = vec![6, 5, 0x09, b'A', b'n', b'd', b'y'];
    data.resize(32, 0);
    assert_eq!(
        complete(&mut c, cmd::LE_SET_ADVERTISING_DATA, &data),
        vec![0]
    );
    assert_eq!(
        complete(&mut c, cmd::LE_SET_ADVERTISING_ENABLE, &[1]),
        vec![0]
    );
    assert_eq!(
        fake.take(),
        vec![Call::Advertise(Some(AdvertiseRequest {
            local_name: Some("Andy".into()),
            services: vec![],
        }))]
    );
}

#[test]
fn masked_events_are_not_sent() {
    let (mut c, fake) = new();
    let h = connect(&mut c, &fake);
    complete(&mut c, cmd::SET_EVENT_MASK, &[0; 8]);
    c.on_event(Event::Disconnected {
        peer: 9,
        reason: status::REMOTE_USER_TERMINATED,
    });
    assert!(c.drain().is_empty());
    let _ = h;
}
