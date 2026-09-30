//! The virtual LE controller: HCI commands from the Android stack in,
//! events and ACL data out, with a [`Backend`] as the radio.
//!
//! - Scanning is the backend's scan; each advertisement received within a
//!   scan window becomes an LE Advertising Report, or an LE Extended
//!   Advertising Report when the host uses the extended commands.
//! - Initiating connects through the backend to the peer address, or to any
//!   device on the Filter Accept List, as soon as the backend knows it. The
//!   link is reported up once the device's GATT tree is discovered, so the
//!   ATT bearer ([`crate::att`]) can answer from the first request.
//! - Advertising maps the host's advertising data onto what the backend can
//!   advertise (local name and service UUIDs).
//! - BR/EDR is absent (the LMP features say so). The host's BR/EDR settings
//!   commands that the Android stack sends unconditionally are accepted and
//!   stored, as a dual-mode controller with an idle BR/EDR radio would.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use crate::adv::{self, Addr, Advertisement};
use crate::att::{Bearer, Output};
use crate::backend::{AdvertiseRequest, Backend, Event, PeerId};
use crate::gatt::Db;
use crate::hci::{self, cmd, ev, le, status};
use crate::l2cap::{self, Reassembler};
use aim_hostcall::bluetooth::kind;

const ADDRESS_RANDOM: u8 = 0x01;
/// Connection handles run from here (any value up to 0x0eff is valid).
const FIRST_HANDLE: u16 = 0x0040;
/// The most advertising data one extended report carries: an event's 255
/// parameter bytes less the subevent, the count and the report header.
const EXTENDED_REPORT_DATA: usize = 229;

struct Scan {
    enabled: bool,
    extended: bool,
    filter_duplicates: bool,
    seen: HashSet<Addr>,
    /// The scan interval and window, in units of 0.625 ms.
    interval: u16,
    window: u16,
    /// When the scan began: a window opens every interval from here.
    since: Instant,
}

impl Default for Scan {
    fn default() -> Self {
        // The spec's defaults: 10 ms windows, back to back.
        Scan {
            enabled: false,
            extended: false,
            filter_duplicates: false,
            seen: HashSet::new(),
            interval: 0x10,
            window: 0x10,
            since: Instant::now(),
        }
    }
}

impl Scan {
    /// Whether a duty-cycled radio would be listening at `t`.
    fn listening(&self, t: Instant) -> bool {
        let us = t.duration_since(self.since).as_micros();
        us % (u128::from(self.interval) * 625) < u128::from(self.window) * 625
    }
}

enum Peers {
    One(Addr),
    AcceptList,
}

struct Initiating {
    peers: Peers,
    extended: bool,
    /// Requested connection parameters, reported back as the link's.
    interval: u16,
    latency: u16,
    timeout: u16,
    connecting: HashSet<PeerId>,
    /// The device that answered, being discovered: its name and ATT MTU.
    discovering: Option<(PeerId, String, u16)>,
}

struct Link {
    peer: PeerId,
    /// The device's name, as the rebuilt Device Name holds it.
    name: String,
    bearer: Bearer,
    reassembler: Reassembler,
    /// Set when the host asked to disconnect: the reason to report.
    local_reason: Option<u8>,
}

#[derive(Default)]
struct AdvertisingSet {
    handle: u8,
    data: Vec<u8>,
    scan_response: Vec<u8>,
    enabled: bool,
}

struct State {
    event_mask: u64,
    le_event_mask: u64,
    local_name: [u8; 248],
    scan: Scan,
    accept_list: Vec<Addr>,
    initiating: Option<Initiating>,
    links: BTreeMap<u16, Link>,
    next_handle: u16,
    /// The one advertising set (legacy advertising uses it too).
    adv: Option<AdvertisingSet>,
    /// Whether the backend scans, and with duplicates.
    backend_scan: Option<bool>,
    /// What the backend advertises.
    backend_adv: Option<AdvertiseRequest>,
}

impl Default for State {
    fn default() -> Self {
        State {
            event_mask: hci::DEFAULT_EVENT_MASK,
            le_event_mask: hci::DEFAULT_LE_EVENT_MASK,
            local_name: [0; 248],
            scan: Scan::default(),
            accept_list: Vec::new(),
            initiating: None,
            links: BTreeMap::new(),
            next_handle: FIRST_HANDLE,
            adv: None,
            backend_scan: None,
            backend_adv: None,
        }
    }
}

pub struct Controller {
    backend: Box<dyn Backend>,
    address: Addr,
    /// Devices the backend reported, for the whole session: HCI names
    /// them by address.
    peers: HashMap<Addr, PeerId>,
    rssi: HashMap<PeerId, i8>,
    s: State,
    out: Vec<(u32, Vec<u8>)>,
    now: Box<dyn Fn() -> Instant + Send>,
}

fn u16_at(p: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([p[i], p[i + 1]])
}

fn addr_at(p: &[u8], i: usize) -> Addr {
    Addr(p[i..i + 6].try_into().unwrap())
}

/// Minimum parameter length of each command that takes parameters.
fn min_len(opcode: u16) -> usize {
    match opcode {
        cmd::SET_EVENT_MASK | cmd::LE_SET_EVENT_MASK => 8,
        cmd::WRITE_LOCAL_NAME => 248,
        cmd::READ_LOCAL_EXTENDED_FEATURES => 1,
        cmd::DISCONNECT => 3,
        cmd::READ_REMOTE_VERSION_INFORMATION
        | cmd::READ_RSSI
        | cmd::LE_READ_REMOTE_FEATURES
        | cmd::LE_READ_PHY => 2,
        cmd::LE_SET_RANDOM_ADDRESS => 6,
        cmd::LE_SET_ADVERTISING_DATA | cmd::LE_SET_SCAN_RESPONSE_DATA => 1,
        cmd::LE_SET_ADVERTISING_ENABLE => 1,
        cmd::LE_SET_SCAN_PARAMETERS => 7,
        cmd::LE_SET_SCAN_ENABLE => 2,
        cmd::LE_CREATE_CONNECTION => 25,
        cmd::LE_ADD_DEVICE_TO_FILTER_ACCEPT_LIST
        | cmd::LE_REMOVE_DEVICE_FROM_FILTER_ACCEPT_LIST => 7,
        cmd::LE_CONNECTION_UPDATE => 14,
        cmd::LE_SET_DATA_LENGTH => 6,
        cmd::LE_SET_PHY => 7,
        cmd::LE_SET_ADVERTISING_SET_RANDOM_ADDRESS => 7,
        cmd::LE_SET_EXTENDED_ADVERTISING_PARAMETERS => 25,
        cmd::LE_SET_EXTENDED_ADVERTISING_DATA | cmd::LE_SET_EXTENDED_SCAN_RESPONSE_DATA => 4,
        cmd::LE_SET_EXTENDED_ADVERTISING_ENABLE => 2,
        cmd::LE_REMOVE_ADVERTISING_SET => 1,
        cmd::LE_SET_EXTENDED_SCAN_PARAMETERS => 8,
        cmd::LE_SET_EXTENDED_SCAN_ENABLE => 6,
        cmd::LE_EXTENDED_CREATE_CONNECTION => 26,
        _ => 0,
    }
}

impl Controller {
    pub fn new(backend: Box<dyn Backend>, address: Addr) -> Self {
        Controller {
            backend,
            address,
            peers: HashMap::new(),
            rssi: HashMap::new(),
            s: State::default(),
            out: Vec::new(),
            now: Box::new(Instant::now),
        }
    }

    /// Packets for the host since the last call: (H4 kind, bytes).
    pub fn drain(&mut self) -> Vec<(u32, Vec<u8>)> {
        std::mem::take(&mut self.out)
    }

    fn emit(&mut self, packet: Vec<u8>) {
        self.out.push((kind::EVENT, packet));
    }

    /// A maskable event.
    fn masked(&mut self, code: u8, params: &[u8]) {
        if self.s.event_mask & hci::event_bit(code) != 0 {
            self.emit(hci::event(code, params));
        }
    }

    fn le_meta(&mut self, subevent: u8, params: &[u8]) {
        if self.s.event_mask & hci::event_bit(ev::LE_META) != 0
            && self.s.le_event_mask & 1 << (subevent - 1) != 0
        {
            self.emit(hci::le_meta(subevent, params));
        }
    }

    fn complete(&mut self, opcode: u16, params: &[u8]) {
        self.emit(hci::command_complete(opcode, params));
    }

    fn ok(&mut self, opcode: u16) {
        self.complete(opcode, &[status::SUCCESS]);
    }

    fn cmd_status(&mut self, opcode: u16, st: u8) {
        self.emit(hci::command_status(opcode, st));
    }

    /// End every activity without events: HCI_Reset, or closing.
    pub fn reset(&mut self) {
        let peers: Vec<PeerId> = self
            .s
            .links
            .values()
            .map(|l| l.peer)
            .chain(self.s.initiating.iter().flat_map(|i| {
                i.connecting
                    .iter()
                    .copied()
                    .chain(i.discovering.as_ref().map(|d| d.0))
            }))
            .collect();
        for p in peers {
            self.backend.disconnect(p);
        }
        if self.s.backend_adv.is_some() {
            self.backend.advertise(None);
        }
        if self.s.backend_scan.is_some() {
            self.backend.scan(false, false);
        }
        self.s = State::default();
    }

    /// An HCI command packet.
    pub fn command(&mut self, p: &[u8]) {
        if p.len() < 3 || p.len() != 3 + p[2] as usize {
            return;
        }
        let opcode = u16_at(p, 0);
        let a = &p[3..];
        if a.len() < min_len(opcode) {
            if cmd::SUPPORTED.iter().any(|&(o, _)| o == opcode) {
                self.complete(opcode, &[status::INVALID_PARAMETERS]);
            } else {
                self.complete(opcode, &[status::UNKNOWN_COMMAND]);
            }
            return;
        }
        match opcode {
            cmd::RESET => {
                self.reset();
                self.ok(opcode);
            }
            cmd::SET_EVENT_MASK => {
                self.s.event_mask = u64::from_le_bytes(a[..8].try_into().unwrap());
                self.ok(opcode);
            }
            cmd::LE_SET_EVENT_MASK => {
                self.s.le_event_mask = u64::from_le_bytes(a[..8].try_into().unwrap());
                self.ok(opcode);
            }
            // Settings of the absent BR/EDR radio, and host-only settings.
            cmd::SET_EVENT_MASK_PAGE_2
            | cmd::WRITE_LE_HOST_SUPPORT
            | cmd::WRITE_SIMPLE_PAIRING_MODE
            | cmd::SET_EVENT_FILTER
            | cmd::WRITE_PAGE_TIMEOUT
            | cmd::WRITE_SCAN_ENABLE
            | cmd::WRITE_PAGE_SCAN_ACTIVITY
            | cmd::WRITE_INQUIRY_SCAN_ACTIVITY
            | cmd::WRITE_CLASS_OF_DEVICE
            | cmd::WRITE_VOICE_SETTING
            | cmd::WRITE_INQUIRY_MODE
            | cmd::WRITE_PAGE_SCAN_TYPE
            | cmd::WRITE_INQUIRY_SCAN_TYPE
            | cmd::WRITE_EXTENDED_INQUIRY_RESPONSE
            | cmd::WRITE_DEFAULT_LINK_POLICY_SETTINGS
            | cmd::LE_SET_HOST_CHANNEL_CLASSIFICATION
            | cmd::LE_SET_RANDOM_ADDRESS
            | cmd::LE_SET_DEFAULT_PHY
            | cmd::LE_WRITE_SUGGESTED_DEFAULT_DATA_LENGTH => self.ok(opcode),
            cmd::WRITE_LOCAL_NAME => {
                self.s.local_name.copy_from_slice(&a[..248]);
                self.ok(opcode);
            }
            cmd::READ_LOCAL_NAME => {
                let mut r = vec![status::SUCCESS];
                r.extend_from_slice(&self.s.local_name);
                self.complete(opcode, &r);
            }
            cmd::READ_LOCAL_VERSION_INFORMATION => {
                let m = hci::MANUFACTURER.to_le_bytes();
                self.complete(
                    opcode,
                    &[0, hci::VERSION, 0, 0, hci::VERSION, m[0], m[1], 0, 0],
                );
            }
            cmd::READ_LOCAL_SUPPORTED_COMMANDS => {
                let mut r = vec![status::SUCCESS];
                r.extend_from_slice(&hci::supported_commands());
                self.complete(opcode, &r);
            }
            cmd::READ_LOCAL_SUPPORTED_FEATURES => {
                let mut r = vec![status::SUCCESS];
                r.extend_from_slice(&hci::LMP_FEATURES.to_le_bytes());
                self.complete(opcode, &r);
            }
            cmd::READ_LOCAL_EXTENDED_FEATURES => {
                let page = a[0];
                let (st, features) = if page == 0 {
                    (status::SUCCESS, hci::LMP_FEATURES)
                } else {
                    (status::INVALID_PARAMETERS, 0)
                };
                let mut r = vec![st, page, 0];
                r.extend_from_slice(&features.to_le_bytes());
                self.complete(opcode, &r);
            }
            cmd::READ_BUFFER_SIZE => {
                // Shared with LE (the LE buffers are reported separately
                // too); no synchronous buffers.
                let (l, n) = (
                    hci::ACL_LENGTH.to_le_bytes(),
                    hci::ACL_PACKETS.to_le_bytes(),
                );
                self.complete(opcode, &[0, l[0], l[1], 0, n[0], n[1], 0, 0]);
            }
            cmd::READ_BD_ADDR => {
                let mut r = vec![status::SUCCESS];
                r.extend_from_slice(&self.address.0);
                self.complete(opcode, &r);
            }
            cmd::READ_RSSI => {
                let handle = u16_at(a, 0);
                let h = handle.to_le_bytes();
                match self.s.links.get(&handle) {
                    Some(l) => {
                        let rssi = self.rssi.get(&l.peer).copied().unwrap_or(127);
                        self.complete(opcode, &[0, h[0], h[1], rssi as u8]);
                    }
                    None => self.complete(opcode, &[status::UNKNOWN_CONNECTION, h[0], h[1], 0]),
                }
            }
            cmd::LE_READ_BUFFER_SIZE_V1 => {
                let l = hci::ACL_LENGTH.to_le_bytes();
                self.complete(opcode, &[0, l[0], l[1], hci::ACL_PACKETS as u8]);
            }
            cmd::LE_READ_LOCAL_SUPPORTED_FEATURES => {
                let mut r = vec![status::SUCCESS];
                r.extend_from_slice(&hci::LE_FEATURES.to_le_bytes());
                self.complete(opcode, &r);
            }
            cmd::LE_READ_SUPPORTED_STATES => {
                let mut r = vec![status::SUCCESS];
                r.extend_from_slice(&hci::LE_STATES.to_le_bytes());
                self.complete(opcode, &r);
            }
            cmd::LE_RAND => {
                let mut r = [0u8; 9];
                crate::random(&mut r[1..]);
                self.complete(opcode, &r);
            }
            cmd::LE_READ_SUGGESTED_DEFAULT_DATA_LENGTH => {
                // The initial values of an LE link (27 octets, 328 us).
                self.complete(opcode, &[0, 27, 0, 0x48, 0x01]);
            }
            cmd::LE_READ_FILTER_ACCEPT_LIST_SIZE => {
                self.complete(opcode, &[0, hci::FILTER_ACCEPT_LIST_SIZE]);
            }
            cmd::LE_CLEAR_FILTER_ACCEPT_LIST
            | cmd::LE_ADD_DEVICE_TO_FILTER_ACCEPT_LIST
            | cmd::LE_REMOVE_DEVICE_FROM_FILTER_ACCEPT_LIST => self.accept_list(opcode, a),
            cmd::LE_SET_SCAN_PARAMETERS | cmd::LE_SET_EXTENDED_SCAN_PARAMETERS => {
                let st = self.scan_parameters(opcode, a);
                self.complete(opcode, &[st]);
            }
            cmd::LE_SET_SCAN_ENABLE | cmd::LE_SET_EXTENDED_SCAN_ENABLE => {
                let (enable, filter) = (a[0] != 0, a[1] != 0);
                if enable && !self.s.scan.enabled {
                    self.s.scan.seen.clear();
                    self.s.scan.since = (self.now)();
                }
                self.s.scan.enabled = enable;
                self.s.scan.filter_duplicates = filter;
                self.s.scan.extended = opcode == cmd::LE_SET_EXTENDED_SCAN_ENABLE;
                self.ok(opcode);
                self.update_scan();
            }
            cmd::LE_CREATE_CONNECTION | cmd::LE_EXTENDED_CREATE_CONNECTION => {
                self.create_connection(opcode, a)
            }
            cmd::LE_CREATE_CONNECTION_CANCEL => self.cancel_connection(opcode),
            cmd::DISCONNECT => {
                let handle = u16_at(a, 0) & 0x0fff;
                match self.s.links.get_mut(&handle) {
                    Some(l) if l.local_reason.is_none() => {
                        l.local_reason = Some(status::LOCAL_HOST_TERMINATED);
                        let peer = l.peer;
                        self.cmd_status(opcode, status::SUCCESS);
                        self.backend.disconnect(peer);
                    }
                    Some(_) => self.cmd_status(opcode, status::COMMAND_DISALLOWED),
                    None => self.cmd_status(opcode, status::UNKNOWN_CONNECTION),
                }
            }
            cmd::READ_REMOTE_VERSION_INFORMATION => {
                // The host cannot ask the device's link layer for it.
                let handle = u16_at(a, 0);
                if self.link_status(opcode, handle) {
                    let h = handle.to_le_bytes();
                    self.masked(
                        ev::READ_REMOTE_VERSION_INFORMATION_COMPLETE,
                        &[
                            status::UNSUPPORTED_REMOTE_FEATURE,
                            h[0],
                            h[1],
                            0,
                            0,
                            0,
                            0,
                            0,
                        ],
                    );
                }
            }
            cmd::LE_READ_REMOTE_FEATURES => {
                // Unknown to the host; no feature is claimed.
                let handle = u16_at(a, 0);
                if self.link_status(opcode, handle) {
                    let mut r = vec![status::SUCCESS];
                    r.extend_from_slice(&handle.to_le_bytes());
                    r.extend_from_slice(&[0; 8]);
                    self.le_meta(le::READ_REMOTE_FEATURES_COMPLETE, &r);
                }
            }
            cmd::LE_CONNECTION_UPDATE => {
                // macOS sets the link's parameters; they stay as they are.
                let handle = u16_at(a, 0);
                if self.link_status(opcode, handle) {
                    let mut r = vec![status::UNACCEPTABLE_CONNECTION_PARAMETERS];
                    r.extend_from_slice(&handle.to_le_bytes());
                    r.extend_from_slice(&[0; 6]);
                    self.le_meta(le::CONNECTION_UPDATE_COMPLETE, &r);
                }
            }
            cmd::LE_SET_DATA_LENGTH => {
                // Taken as a hint; macOS negotiates the data length.
                let h = u16_at(a, 0);
                let st = if self.s.links.contains_key(&h) {
                    status::SUCCESS
                } else {
                    status::UNKNOWN_CONNECTION
                };
                let h = h.to_le_bytes();
                self.complete(opcode, &[st, h[0], h[1]]);
            }
            cmd::LE_READ_PHY => {
                let h = u16_at(a, 0);
                let st = if self.s.links.contains_key(&h) {
                    status::SUCCESS
                } else {
                    status::UNKNOWN_CONNECTION
                };
                let h = h.to_le_bytes();
                self.complete(opcode, &[st, h[0], h[1], 1, 1]);
            }
            cmd::LE_SET_PHY => {
                // LE 1M stays; a PHY Update Complete reports it even so.
                let handle = u16_at(a, 0);
                if self.link_status(opcode, handle) {
                    let h = handle.to_le_bytes();
                    self.le_meta(le::PHY_UPDATE_COMPLETE, &[0, h[0], h[1], 1, 1]);
                }
            }
            _ => self.advertising(opcode, a),
        }
    }

    /// Command Status for a command on `handle`: true when the link exists.
    fn link_status(&mut self, opcode: u16, handle: u16) -> bool {
        let known = self.s.links.contains_key(&handle);
        let st = if known {
            status::SUCCESS
        } else {
            status::UNKNOWN_CONNECTION
        };
        self.cmd_status(opcode, st);
        known
    }

    fn accept_list(&mut self, opcode: u16, a: &[u8]) {
        if self
            .s
            .initiating
            .as_ref()
            .is_some_and(|i| matches!(i.peers, Peers::AcceptList))
        {
            return self.complete(opcode, &[status::COMMAND_DISALLOWED]);
        }
        let list = &mut self.s.accept_list;
        let st = match opcode {
            cmd::LE_CLEAR_FILTER_ACCEPT_LIST => {
                list.clear();
                status::SUCCESS
            }
            cmd::LE_ADD_DEVICE_TO_FILTER_ACCEPT_LIST => {
                let addr = addr_at(a, 1);
                if list.contains(&addr) {
                    status::SUCCESS
                } else if list.len() >= hci::FILTER_ACCEPT_LIST_SIZE as usize {
                    status::MEMORY_CAPACITY_EXCEEDED
                } else {
                    list.push(addr);
                    status::SUCCESS
                }
            }
            _ => {
                let addr = addr_at(a, 1);
                list.retain(|x| *x != addr);
                status::SUCCESS
            }
        };
        self.complete(opcode, &[st]);
    }

    /// CoreBluetooth scans actively and without pause; the interval and
    /// window make the reports those of a duty-cycled radio. The type is
    /// the radio's business. Extended parameters take the first PHY's.
    fn scan_parameters(&mut self, opcode: u16, a: &[u8]) -> u8 {
        const PHYS: u8 = 0b101; // LE 1M, LE Coded
        if self.s.scan.enabled {
            return status::COMMAND_DISALLOWED;
        }
        let timing = if opcode == cmd::LE_SET_SCAN_PARAMETERS {
            Some((u16_at(a, 1), u16_at(a, 3), 0x4000))
        } else {
            let phys = a[2];
            (phys != 0 && phys & !PHYS == 0 && a.len() >= 3 + 5 * phys.count_ones() as usize)
                .then(|| (u16_at(a, 4), u16_at(a, 6), 0xffff))
        };
        match timing {
            Some((interval, window, max))
                if (4..=interval).contains(&window) && interval <= max =>
            {
                self.s.scan.interval = interval;
                self.s.scan.window = window;
                status::SUCCESS
            }
            _ => status::INVALID_PARAMETERS,
        }
    }

    /// Scan in the backend while the host scans, or while initiating waits
    /// for a device the backend does not know yet.
    fn update_scan(&mut self) {
        let initiating = self.s.initiating.as_ref().is_some_and(|i| {
            i.discovering.is_none()
                && match &i.peers {
                    Peers::One(a) => !self.peers.contains_key(a),
                    Peers::AcceptList => true,
                }
        });
        let want = if self.s.scan.enabled || initiating {
            Some(!self.s.scan.filter_duplicates || initiating)
        } else {
            None
        };
        if want != self.s.backend_scan {
            self.s.backend_scan = want;
            self.backend.scan(want.is_some(), want.unwrap_or(false));
        }
    }

    fn create_connection(&mut self, opcode: u16, a: &[u8]) {
        let extended = opcode == cmd::LE_EXTENDED_CREATE_CONNECTION;
        // Filter policy, then the peer: legacy at 5, extended at 0.
        let (policy, peer_at) = if extended { (a[0], 3) } else { (a[4], 6) };
        // Connection interval max, latency and timeout (of the first PHY).
        let params = if extended { 16 } else { 15 };
        if a.len() < params + 6 {
            return self.cmd_status(opcode, status::INVALID_PARAMETERS);
        }
        if self.s.initiating.is_some() {
            return self.cmd_status(opcode, status::COMMAND_DISALLOWED);
        }
        let peers = if policy == 0 {
            Peers::One(addr_at(a, peer_at))
        } else {
            Peers::AcceptList
        };
        self.cmd_status(opcode, status::SUCCESS);
        let known: Vec<PeerId> = match &peers {
            Peers::One(addr) => self.peers.get(addr).copied().into_iter().collect(),
            Peers::AcceptList => self
                .s
                .accept_list
                .iter()
                .filter_map(|x| self.peers.get(x).copied())
                .collect(),
        };
        self.s.initiating = Some(Initiating {
            peers,
            extended,
            interval: u16_at(a, params),
            latency: u16_at(a, params + 2),
            timeout: u16_at(a, params + 4),
            connecting: known.iter().copied().collect(),
            discovering: None,
        });
        for p in known {
            self.backend.connect(p);
        }
        self.update_scan();
    }

    fn cancel_connection(&mut self, opcode: u16) {
        let Some(i) = self.s.initiating.take() else {
            return self.complete(opcode, &[status::COMMAND_DISALLOWED]);
        };
        self.ok(opcode);
        for p in i
            .connecting
            .iter()
            .chain(i.discovering.as_ref().map(|d| &d.0))
        {
            self.backend.disconnect(*p);
        }
        self.connection_failed(&i, status::UNKNOWN_CONNECTION);
        self.update_scan();
    }

    fn connection_complete(&mut self, i: &Initiating, st: u8, handle: u16, addr: Addr) {
        let mut r = vec![st];
        r.extend_from_slice(&handle.to_le_bytes());
        r.push(0); // central
        r.push(ADDRESS_RANDOM);
        r.extend_from_slice(&addr.0);
        let enhanced =
            i.extended && self.s.le_event_mask & 1 << (le::ENHANCED_CONNECTION_COMPLETE - 1) != 0;
        if enhanced {
            r.extend_from_slice(&[0; 12]); // no resolvable private addresses
        }
        r.extend_from_slice(&i.interval.to_le_bytes());
        r.extend_from_slice(&i.latency.to_le_bytes());
        r.extend_from_slice(&i.timeout.to_le_bytes());
        r.push(0); // central clock accuracy: 500 ppm
        let sub = if enhanced {
            le::ENHANCED_CONNECTION_COMPLETE
        } else {
            le::CONNECTION_COMPLETE
        };
        self.le_meta(sub, &r);
    }

    fn connection_failed(&mut self, i: &Initiating, st: u8) {
        let addr = match i.peers {
            Peers::One(a) => a,
            Peers::AcceptList => Addr::default(),
        };
        self.connection_complete(i, st, 0, addr);
    }

    fn addr_of(&self, peer: PeerId) -> Addr {
        self.peers
            .iter()
            .find(|(_, p)| **p == peer)
            .map(|(a, _)| *a)
            .unwrap_or_default()
    }

    fn link_of(&mut self, peer: PeerId) -> Option<u16> {
        self.s
            .links
            .iter()
            .find(|(_, l)| l.peer == peer)
            .map(|(h, _)| *h)
    }

    /// An event from the backend.
    pub fn on_event(&mut self, e: Event) {
        match e {
            Event::Power(on) => {
                if !on {
                    let handles: Vec<u16> = self.s.links.keys().copied().collect();
                    for h in handles {
                        self.link_down(h, status::CONNECTION_TIMEOUT);
                    }
                }
            }
            Event::Advertisement {
                peer,
                addr,
                rssi,
                adv,
            } => self.advertisement(peer, addr, rssi, &adv),
            Event::Connected { peer, name, mtu } => {
                let answered = self
                    .s
                    .initiating
                    .as_mut()
                    .is_some_and(|i| i.discovering.is_none() && i.connecting.remove(&peer));
                if !answered {
                    self.backend.disconnect(peer);
                    return;
                }
                let i = self.s.initiating.as_mut().unwrap();
                i.discovering = Some((peer, name, mtu));
                let others: Vec<PeerId> = i.connecting.drain().collect();
                for p in others {
                    self.backend.disconnect(p);
                }
                self.backend.discover(peer);
                self.update_scan();
            }
            Event::ConnectFailed { peer } => self.initiation_lost(peer),
            Event::Disconnected { peer, reason } => match self.link_of(peer) {
                Some(h) => self.link_down(h, reason),
                None => self.initiation_lost(peer),
            },
            Event::Discovered { peer, services } => self.discovered(peer, services),
            Event::Read {
                peer,
                target,
                result,
            } => self.with_bearer(peer, |b| b.on_read(target, result)),
            Event::Written {
                peer,
                target,
                result,
            } => self.with_bearer(peer, |b| b.on_write(target, result)),
            Event::NotifyState { peer, char, result } => {
                self.with_bearer(peer, |b| b.on_notify_state(char, result))
            }
            Event::Value { peer, char, value } => {
                self.with_bearer(peer, |b| b.on_value(char, &value))
            }
        }
    }

    fn advertisement(&mut self, peer: PeerId, addr: Addr, rssi: i8, adv: &Advertisement) {
        self.peers.insert(addr, peer);
        self.rssi.insert(peer, rssi);
        let accept = self.s.accept_list.contains(&addr);
        if let Some(i) = self.s.initiating.as_mut()
            && i.discovering.is_none()
            && adv.connectable
            && !i.connecting.contains(&peer)
            && match i.peers {
                Peers::One(a) => a == addr,
                Peers::AcceptList => accept,
            }
        {
            i.connecting.insert(peer);
            self.backend.connect(peer);
            self.update_scan();
        }
        if !self.s.scan.enabled
            || !self.s.scan.listening((self.now)())
            || self.s.scan.filter_duplicates && !self.s.scan.seen.insert(addr)
        {
            return;
        }
        if self.s.scan.extended {
            self.extended_report(addr, rssi, adv);
        } else {
            self.legacy_reports(addr, rssi, adv);
        }
    }

    fn legacy_reports(&mut self, addr: Addr, rssi: i8, adv: &Advertisement) {
        const ADV_IND: u8 = 0x00;
        const ADV_NONCONN_IND: u8 = 0x03;
        const SCAN_RSP: u8 = 0x04;
        let (data, scan_response) = adv.legacy();
        let kind = if adv.connectable {
            ADV_IND
        } else {
            ADV_NONCONN_IND
        };
        for (kind, data) in [(kind, data), (SCAN_RSP, scan_response)] {
            if kind == SCAN_RSP && data.is_empty() {
                continue;
            }
            let mut r = vec![1, kind, ADDRESS_RANDOM];
            r.extend_from_slice(&addr.0);
            r.push(data.len() as u8);
            r.extend_from_slice(&data);
            r.push(rssi as u8);
            self.le_meta(le::ADVERTISING_REPORT, &r);
        }
    }

    fn extended_report(&mut self, addr: Addr, rssi: i8, adv: &Advertisement) {
        const CONNECTABLE: u16 = 1 << 0;
        const SCANNABLE: u16 = 1 << 1;
        const LEGACY: u16 = 1 << 4;
        const MORE_DATA: u16 = 0b01 << 5;
        const NO_ADI: u8 = 0xff;
        const TX_POWER_UNKNOWN: u8 = 0x7f;
        let data = adv.data();
        // A device's data that fits a legacy PDU is reported as one (as
        // ADV_IND or ADV_NONCONN_IND, over LE 1M).
        let legacy = data.len() <= 31;
        let mut kind = if legacy { LEGACY } else { 0 };
        if adv.connectable {
            kind |= CONNECTABLE | if legacy { SCANNABLE } else { 0 };
        }
        let chunks: Vec<&[u8]> = if data.is_empty() {
            vec![&[]]
        } else {
            data.chunks(EXTENDED_REPORT_DATA).collect()
        };
        let n = chunks.len();
        for (i, chunk) in chunks.into_iter().enumerate() {
            let status = if i + 1 < n { MORE_DATA } else { 0 };
            let mut r = vec![1];
            r.extend_from_slice(&(kind | status).to_le_bytes());
            r.push(ADDRESS_RANDOM);
            r.extend_from_slice(&addr.0);
            r.push(1); // primary PHY: LE 1M
            r.push(if legacy { 0 } else { 1 }); // secondary PHY
            r.push(NO_ADI);
            r.push(adv.tx_power.map_or(TX_POWER_UNKNOWN, |p| p as u8));
            r.push(rssi as u8);
            r.extend_from_slice(&[0, 0]); // no periodic advertising
            r.push(0); // direct address type
            r.extend_from_slice(&[0; 6]);
            r.push(chunk.len() as u8);
            r.extend_from_slice(chunk);
            self.le_meta(le::EXTENDED_ADVERTISING_REPORT, &r);
        }
    }

    /// `peer` dropped out of an attempt to connect.
    fn initiation_lost(&mut self, peer: PeerId) {
        let Some(i) = self.s.initiating.as_mut() else {
            return;
        };
        let was_discovering = i.discovering.as_ref().is_some_and(|d| d.0 == peer);
        if !i.connecting.remove(&peer) && !was_discovering {
            return;
        }
        // Another device on the accept list may still answer.
        if !was_discovering && matches!(i.peers, Peers::AcceptList) {
            return;
        }
        let i = self.s.initiating.take().unwrap();
        self.connection_failed(&i, status::CONNECTION_FAILED_TO_BE_ESTABLISHED);
        self.update_scan();
    }

    fn discovered(&mut self, peer: PeerId, services: Result<Vec<crate::gatt::Service>, ()>) {
        if let Some(h) = self.link_of(peer) {
            if let Ok(services) = services {
                let link = self.s.links.get_mut(&h).unwrap();
                link.bearer.replace_db(Db::build(&link.name, &services));
                self.flush(h);
            }
            return;
        }
        let Some(i) = self.s.initiating.as_ref() else {
            return;
        };
        if i.discovering.as_ref().is_none_or(|d| d.0 != peer) {
            return;
        }
        let i = self.s.initiating.take().unwrap();
        let Ok(services) = services else {
            self.backend.disconnect(peer);
            self.connection_failed(&i, status::CONNECTION_FAILED_TO_BE_ESTABLISHED);
            self.update_scan();
            return;
        };
        let (_, name, mtu) = i.discovering.clone().unwrap();
        let handle = self.s.next_handle;
        self.s.next_handle = if handle >= 0x0eff {
            FIRST_HANDLE
        } else {
            handle + 1
        };
        self.s.links.insert(
            handle,
            Link {
                peer,
                bearer: Bearer::new(Db::build(&name, &services), mtu),
                name,
                reassembler: Reassembler::default(),
                local_reason: None,
            },
        );
        let addr = self.addr_of(peer);
        self.connection_complete(&i, status::SUCCESS, handle, addr);
        self.update_scan();
    }

    fn link_down(&mut self, handle: u16, reason: u8) {
        let Some(link) = self.s.links.remove(&handle) else {
            return;
        };
        let h = handle.to_le_bytes();
        let reason = link.local_reason.unwrap_or(reason);
        self.masked(ev::DISCONNECTION_COMPLETE, &[0, h[0], h[1], reason]);
    }

    fn with_bearer(&mut self, peer: PeerId, f: impl FnOnce(&mut Bearer)) {
        if let Some(h) = self.link_of(peer) {
            f(&mut self.s.links.get_mut(&h).unwrap().bearer);
            self.flush(h);
        }
    }

    /// Carry out what a link's bearer asks for.
    fn flush(&mut self, handle: u16) {
        let Some(link) = self.s.links.get_mut(&handle) else {
            return;
        };
        let peer = link.peer;
        for o in link.bearer.drain() {
            match o {
                Output::Pdu(pdu) => self.send_l2cap(handle, l2cap::CID_ATT, &pdu),
                Output::Op(op) => self.backend.gatt(peer, op),
            }
        }
    }

    fn send_l2cap(&mut self, handle: u16, cid: u16, payload: &[u8]) {
        for p in l2cap::fragment(handle, cid, payload, hci::ACL_LENGTH as usize) {
            self.out.push((kind::ACL, p));
        }
    }

    /// An ACL data packet from the host.
    pub fn acl(&mut self, p: &[u8]) {
        if p.len() < 4 || p.len() != 4 + u16_at(p, 2) as usize {
            return;
        }
        let field = u16_at(p, 0);
        let (handle, pb) = (field & 0x0fff, field >> 12 & 0b11);
        let Some(link) = self.s.links.get_mut(&handle) else {
            return;
        };
        let frame = link.reassembler.push(pb, &p[4..]);
        // The controller's buffer is free again at once.
        let h = handle.to_le_bytes();
        self.emit(hci::event(
            ev::NUMBER_OF_COMPLETED_PACKETS,
            &[1, h[0], h[1], 1, 0],
        ));
        let Some((cid, payload)) = frame else { return };
        match cid {
            l2cap::CID_ATT => {
                self.s
                    .links
                    .get_mut(&handle)
                    .unwrap()
                    .bearer
                    .on_pdu(&payload);
                self.flush(handle);
            }
            l2cap::CID_SIGNALING => {
                if let Some(r) = l2cap::signaling(&payload) {
                    self.send_l2cap(handle, cid, &r);
                }
            }
            l2cap::CID_SMP => {
                if let Some(r) = l2cap::security_manager(&payload) {
                    self.send_l2cap(handle, cid, &r);
                }
            }
            _ => {}
        }
    }

    fn advertising(&mut self, opcode: u16, a: &[u8]) {
        match opcode {
            cmd::LE_SET_ADVERTISING_PARAMETERS => {
                self.set_for(0, true);
                self.ok(opcode)
            }
            cmd::LE_READ_ADVERTISING_PHYSICAL_CHANNEL_TX_POWER => self.complete(opcode, &[0, 0]),
            cmd::LE_SET_ADVERTISING_DATA | cmd::LE_SET_SCAN_RESPONSE_DATA => {
                let len = (a[0] as usize).min(31).min(a.len() - 1);
                let set = self.set_for(0, true).unwrap();
                let data = a[1..1 + len].to_vec();
                if opcode == cmd::LE_SET_ADVERTISING_DATA {
                    set.data = data;
                } else {
                    set.scan_response = data;
                }
                self.ok(opcode);
                self.update_advertising();
            }
            cmd::LE_SET_ADVERTISING_ENABLE => {
                self.set_for(0, true).unwrap().enabled = a[0] != 0;
                self.ok(opcode);
                self.update_advertising();
            }
            cmd::LE_SET_ADVERTISING_SET_RANDOM_ADDRESS => {
                let st = match self.set_for(a[0], false) {
                    Some(_) => status::SUCCESS,
                    None => status::UNKNOWN_ADVERTISING_IDENTIFIER,
                };
                self.complete(opcode, &[st]);
            }
            cmd::LE_SET_EXTENDED_ADVERTISING_PARAMETERS => match self.set_for(a[0], true) {
                // Selected TX power: 0 dBm.
                Some(_) => self.complete(opcode, &[status::SUCCESS, 0]),
                None => self.complete(opcode, &[status::MEMORY_CAPACITY_EXCEEDED, 0]),
            },
            cmd::LE_SET_EXTENDED_ADVERTISING_DATA | cmd::LE_SET_EXTENDED_SCAN_RESPONSE_DATA => {
                // Handle, operation (0 intermediate, 1 first, 2 last, 3
                // complete), fragment preference, length, data.
                let (handle, operation, len) = (a[0], a[1], a[3] as usize);
                if a.len() < 4 + len {
                    return self.complete(opcode, &[status::INVALID_PARAMETERS]);
                }
                let Some(set) = self.set_for(handle, false) else {
                    return self.complete(opcode, &[status::UNKNOWN_ADVERTISING_IDENTIFIER]);
                };
                let target = if opcode == cmd::LE_SET_EXTENDED_ADVERTISING_DATA {
                    &mut set.data
                } else {
                    &mut set.scan_response
                };
                if operation == 1 || operation == 3 {
                    target.clear();
                }
                target.extend_from_slice(&a[4..4 + len]);
                self.ok(opcode);
                self.update_advertising();
            }
            cmd::LE_SET_EXTENDED_ADVERTISING_ENABLE => {
                let (enable, n) = (a[0] != 0, a[1] as usize);
                if a.len() < 2 + 4 * n {
                    return self.complete(opcode, &[status::INVALID_PARAMETERS]);
                }
                let handles: Vec<u8> = (0..n).map(|i| a[2 + 4 * i]).collect();
                match self.s.adv.as_mut() {
                    // Disabling with no sets listed disables them all.
                    Some(set) if n == 0 && !enable => set.enabled = false,
                    Some(set) if handles.iter().all(|h| *h == set.handle) && n > 0 => {
                        set.enabled = enable
                    }
                    _ => {
                        return self.complete(opcode, &[status::UNKNOWN_ADVERTISING_IDENTIFIER]);
                    }
                }
                self.ok(opcode);
                self.update_advertising();
            }
            cmd::LE_READ_MAXIMUM_ADVERTISING_DATA_LENGTH => self.complete(opcode, &[0, 31, 0]),
            cmd::LE_READ_NUMBER_OF_SUPPORTED_ADVERTISING_SETS => self.complete(opcode, &[0, 1]),
            cmd::LE_REMOVE_ADVERTISING_SET | cmd::LE_CLEAR_ADVERTISING_SETS => {
                let removes = match &self.s.adv {
                    Some(set) => opcode == cmd::LE_CLEAR_ADVERTISING_SETS || set.handle == a[0],
                    None => opcode == cmd::LE_CLEAR_ADVERTISING_SETS,
                };
                if !removes {
                    return self.complete(opcode, &[status::UNKNOWN_ADVERTISING_IDENTIFIER]);
                }
                if self.s.adv.as_ref().is_some_and(|s| s.enabled) {
                    return self.complete(opcode, &[status::COMMAND_DISALLOWED]);
                }
                self.s.adv = None;
                self.ok(opcode);
            }
            _ => self.complete(opcode, &[status::UNKNOWN_COMMAND]),
        }
    }

    /// The advertising set `handle`, created if `create` and none exists.
    /// Only one set exists at a time.
    fn set_for(&mut self, handle: u8, create: bool) -> Option<&mut AdvertisingSet> {
        match &self.s.adv {
            Some(s) if s.handle != handle => return None,
            None if !create => return None,
            None => {
                self.s.adv = Some(AdvertisingSet {
                    handle,
                    ..Default::default()
                })
            }
            _ => {}
        }
        self.s.adv.as_mut()
    }

    fn update_advertising(&mut self) {
        let request = self.s.adv.as_ref().filter(|s| s.enabled).map(|s| {
            let (mut name, mut services) = adv::parse(&s.data);
            let (rsp_name, rsp_services) = adv::parse(&s.scan_response);
            name = name.or(rsp_name);
            services.extend(rsp_services);
            AdvertiseRequest {
                local_name: name,
                services,
            }
        });
        if request != self.s.backend_adv {
            self.s.backend_adv = request.clone();
            self.backend.advertise(request);
        }
    }
}

#[cfg(test)]
mod tests;
