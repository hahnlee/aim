//! The radio: CoreBluetooth's `CBCentralManager` (scanning, connections,
//! GATT) and `CBPeripheralManager` (advertising), on a serial dispatch
//! queue of their own.
//!
//! Every request is queued onto that queue, and every delegate callback runs
//! there, so the CoreBluetooth objects are only ever touched from it. The
//! framework is loaded on first use, so guest processes without Bluetooth
//! never load it. The first use asks for the Bluetooth permission (TCC);
//! until it is granted the central stays unauthorized and the controller
//! sees no devices.

mod objc;

use std::collections::HashMap;
use std::ffi::{CStr, c_void};
use std::sync::{Arc, Mutex, Once, Weak};

use crate::adv::{Addr, Advertisement};
use crate::att::{Op, err as att_err};
use crate::backend::{AdvertiseRequest, Backend, Event, PeerId};
use crate::gatt::{Characteristic, Descriptor, Service, Target, Uuid};
use crate::hci::status;
use objc::{Id, NIL, Obj, Pool, Queue, Sel, send};

const FRAMEWORK: &CStr = c"/System/Library/Frameworks/CoreBluetooth.framework/CoreBluetooth";
const DELEGATE_CLASS: &CStr = c"DarwinLinuxBluetoothDelegate";
/// `CBManagerStatePoweredOn`, `CBManagerStateUnauthorized`.
const POWERED_ON: i64 = 5;
const UNAUTHORIZED: i64 = 3;
/// `CBCharacteristicWriteType`.
const WITH_RESPONSE: i64 = 0;
const WITHOUT_RESPONSE: i64 = 1;
/// `CBError` codes.
const CONNECTION_TIMEOUT: i64 = 6;
const PERIPHERAL_DISCONNECTED: i64 = 7;

/// The advertisement dictionary keys and scan option, from the framework.
struct Keys {
    local_name: Id,
    manufacturer: Id,
    service_data: Id,
    services: Id,
    overflow: Id,
    tx_power: Id,
    connectable: Id,
    solicited: Id,
    allow_duplicates: Id,
}

struct Gatt {
    chars: Vec<Obj>,
    descs: Vec<Obj>,
    char_ids: HashMap<usize, u32>,
    desc_ids: HashMap<usize, u32>,
    /// Discovery callbacks still to come.
    outstanding: usize,
    discovering: bool,
    /// Reads outstanding per characteristic: a value update answers a read
    /// first, and is a notification otherwise.
    reads: HashMap<u32, u32>,
}

/// State touched only on the queue.
struct Cb {
    central: Obj,
    delegate: Obj,
    peripheral_manager: Option<Obj>,
    powered: bool,
    advertiser_powered: bool,
    /// What the controller asked for, applied while powered.
    want_scan: Option<bool>,
    scanning: Option<bool>,
    want_advertising: Option<AdvertiseRequest>,
    want_connect: Vec<PeerId>,
    peers: Vec<Obj>,
    by_uuid: HashMap<[u8; 16], PeerId>,
    gatt: HashMap<PeerId, Gatt>,
    warned: bool,
}

struct Inner {
    queue: Queue,
    sink: Box<dyn Fn(Event) + Send + Sync>,
    keys: Keys,
    cb: Mutex<Option<Cb>>,
}

// SAFETY: the queue and the key constants are immutable and thread-safe;
// the CoreBluetooth objects in `cb` are only used on the queue.
unsafe impl Send for Inner {}
unsafe impl Sync for Inner {}

/// Delegates by address, for the callbacks.
static DELEGATES: Mutex<Vec<(usize, Weak<Inner>)>> = Mutex::new(Vec::new());

pub struct CoreBluetooth {
    inner: Arc<Inner>,
}

impl CoreBluetooth {
    pub fn new(sink: Box<dyn Fn(Event) + Send + Sync>) -> Option<CoreBluetooth> {
        // SAFETY: loading a system framework.
        let handle = unsafe { libc::dlopen(FRAMEWORK.as_ptr(), libc::RTLD_NOW) };
        if handle.is_null() {
            return None;
        }
        let _pool = Pool::new();
        let k = |name| objc::constant(handle, name);
        let keys = Keys {
            local_name: k("CBAdvertisementDataLocalNameKey"),
            manufacturer: k("CBAdvertisementDataManufacturerDataKey"),
            service_data: k("CBAdvertisementDataServiceDataKey"),
            services: k("CBAdvertisementDataServiceUUIDsKey"),
            overflow: k("CBAdvertisementDataOverflowServiceUUIDsKey"),
            tx_power: k("CBAdvertisementDataTxPowerLevelKey"),
            connectable: k("CBAdvertisementDataIsConnectable"),
            solicited: k("CBAdvertisementDataSolicitedServiceUUIDsKey"),
            allow_duplicates: k("CBCentralManagerScanOptionAllowDuplicatesKey"),
        };
        let class = delegate_class();
        let alloc = send!(class, c"alloc" => Id);
        let delegate = send!(alloc, c"init" => Id);
        // SAFETY: a serial queue with a static label.
        let queue = unsafe {
            objc::dispatch_queue_create(c"darwin.linux-abi.bluetooth".as_ptr(), std::ptr::null())
        };
        let inner = Arc::new(Inner {
            queue,
            sink,
            keys,
            cb: Mutex::new(None),
        });
        DELEGATES
            .lock()
            .unwrap()
            .push((delegate as usize, Arc::downgrade(&inner)));
        let delegate = Obj(delegate);
        let this = CoreBluetooth { inner };
        this.run(move |inner, cb| {
            let alloc = send!(objc::class(c"CBCentralManager"), c"alloc" => Id);
            let central = send!(alloc, c"initWithDelegate:queue:options:" => Id,
                Id = delegate.0, Queue = inner.queue, Id = NIL);
            *cb = Some(Cb {
                central: Obj(central),
                delegate,
                peripheral_manager: None,
                powered: false,
                advertiser_powered: false,
                want_scan: None,
                scanning: None,
                want_advertising: None,
                want_connect: Vec::new(),
                peers: Vec::new(),
                by_uuid: HashMap::new(),
                gatt: HashMap::new(),
                warned: false,
            });
            Vec::new()
        });
        Some(this)
    }

    /// Run `f` on the queue with the state; the events it returns go to
    /// the controller afterwards.
    fn run(&self, f: impl FnOnce(&Inner, &mut Option<Cb>) -> Vec<Event> + Send + 'static) {
        type Task = (
            Arc<Inner>,
            Box<dyn FnOnce(&Inner, &mut Option<Cb>) -> Vec<Event> + Send>,
        );
        extern "C" fn trampoline(ctx: *mut c_void) {
            // SAFETY: the box dispatch_async_f was given below.
            let (inner, f) = *unsafe { Box::from_raw(ctx as *mut Task) };
            let _pool = Pool::new();
            let events = f(&inner, &mut inner.cb.lock().unwrap());
            for e in events {
                (inner.sink)(e);
            }
        }
        let task: Box<Task> = Box::new((self.inner.clone(), Box::new(f)));
        // SAFETY: the queue lives as long as `inner`, which the task holds.
        unsafe { objc::dispatch_async_f(self.inner.queue, Box::into_raw(task).cast(), trampoline) };
    }

    /// Run `f` on the queue once the state exists.
    fn with(&self, f: impl FnOnce(&Inner, &mut Cb) -> Vec<Event> + Send + 'static) {
        self.run(move |inner, cb| cb.as_mut().map(|cb| f(inner, cb)).unwrap_or_default());
    }
}

impl Drop for CoreBluetooth {
    fn drop(&mut self) {
        self.run(move |_, cb| {
            if let Some(cb) = cb.take() {
                if cb.scanning.is_some() {
                    send!(cb.central.0, c"stopScan" => ());
                }
                for p in &cb.peers {
                    send!(cb.central.0, c"cancelPeripheralConnection:" => (), Id = p.0);
                    send!(p.0, c"setDelegate:" => (), Id = NIL);
                }
                if let Some(pm) = &cb.peripheral_manager {
                    send!(pm.0, c"stopAdvertising" => ());
                    send!(pm.0, c"setDelegate:" => (), Id = NIL);
                }
                send!(cb.central.0, c"setDelegate:" => (), Id = NIL);
                let d = cb.delegate.0 as usize;
                DELEGATES.lock().unwrap().retain(|(k, _)| *k != d);
            }
            Vec::new()
        });
        // SAFETY: pending tasks hold the queue until they ran.
        unsafe { objc::dispatch_release(self.inner.queue) };
    }
}

impl Backend for CoreBluetooth {
    fn scan(&mut self, on: bool, duplicates: bool) {
        self.with(move |inner, cb| {
            cb.want_scan = on.then_some(duplicates);
            apply_scan(inner, cb);
            Vec::new()
        });
    }

    fn connect(&mut self, peer: PeerId) {
        self.with(move |_, cb| {
            if !cb.want_connect.contains(&peer) {
                cb.want_connect.push(peer);
            }
            apply_connect(cb);
            Vec::new()
        });
    }

    fn disconnect(&mut self, peer: PeerId) {
        self.with(move |_, cb| {
            cb.want_connect.retain(|p| *p != peer);
            if let Some(p) = cb.peers.get(peer as usize) {
                send!(cb.central.0, c"cancelPeripheralConnection:" => (), Id = p.0);
            }
            cb.gatt.remove(&peer);
            Vec::new()
        });
    }

    fn discover(&mut self, peer: PeerId) {
        self.with(move |_, cb| {
            let Some(p) = cb.peers.get(peer as usize) else {
                return vec![Event::Discovered {
                    peer,
                    services: Err(()),
                }];
            };
            cb.gatt.insert(
                peer,
                Gatt {
                    chars: Vec::new(),
                    descs: Vec::new(),
                    char_ids: HashMap::new(),
                    desc_ids: HashMap::new(),
                    // The services callback.
                    outstanding: 1,
                    discovering: true,
                    reads: HashMap::new(),
                },
            );
            send!(p.0, c"discoverServices:" => (), Id = NIL);
            Vec::new()
        });
    }

    fn gatt(&mut self, peer: PeerId, op: Op) {
        self.with(move |_, cb| gatt_op(cb, peer, op));
    }

    fn advertise(&mut self, request: Option<AdvertiseRequest>) {
        self.with(move |inner, cb| {
            if request.is_some() && cb.peripheral_manager.is_none() {
                let alloc = send!(objc::class(c"CBPeripheralManager"), c"alloc" => Id);
                let pm = send!(alloc, c"initWithDelegate:queue:options:" => Id,
                    Id = cb.delegate.0, Queue = inner.queue, Id = NIL);
                cb.peripheral_manager = Some(Obj(pm));
            }
            cb.want_advertising = request;
            apply_advertising(inner, cb);
            Vec::new()
        });
    }
}

fn apply_scan(inner: &Inner, cb: &mut Cb) {
    if !cb.powered || cb.want_scan == cb.scanning {
        return;
    }
    match cb.want_scan {
        Some(duplicates) => {
            let options =
                objc::dictionary(&[(inner.keys.allow_duplicates, objc::nsnumber_bool(duplicates))]);
            send!(cb.central.0, c"scanForPeripheralsWithServices:options:" => (),
                Id = NIL, Id = options);
        }
        None => send!(cb.central.0, c"stopScan" => ()),
    }
    cb.scanning = cb.want_scan;
}

fn apply_connect(cb: &mut Cb) {
    if !cb.powered {
        return;
    }
    for peer in std::mem::take(&mut cb.want_connect) {
        if let Some(p) = cb.peers.get(peer as usize) {
            send!(p.0, c"setDelegate:" => (), Id = cb.delegate.0);
            send!(cb.central.0, c"connectPeripheral:options:" => (), Id = p.0, Id = NIL);
        }
    }
}

fn apply_advertising(inner: &Inner, cb: &mut Cb) {
    let Some(pm) = &cb.peripheral_manager else {
        return;
    };
    if !cb.advertiser_powered {
        return;
    }
    send!(pm.0, c"stopAdvertising" => ());
    let Some(request) = &cb.want_advertising else {
        return;
    };
    // The keys CBPeripheralManager accepts are these two alone.
    let mut entries = Vec::new();
    if let Some(name) = &request.local_name {
        entries.push((inner.keys.local_name, objc::nsstring(name)));
    }
    if !request.services.is_empty() {
        let uuids: Vec<Id> = request.services.iter().map(|u| cbuuid(*u)).collect();
        entries.push((inner.keys.services, objc::nsarray(&uuids)));
    }
    send!(pm.0, c"startAdvertising:" => (), Id = objc::dictionary(&entries));
}

fn gatt_op(cb: &mut Cb, peer: PeerId, op: Op) -> Vec<Event> {
    let (Some(p), Some(g)) = (cb.peers.get(peer as usize), cb.gatt.get_mut(&peer)) else {
        return Vec::new();
    };
    let obj = |t: Target| match t {
        Target::Char(i) => g.chars.get(i as usize).map(|o| o.0),
        Target::Desc(i) => g.descs.get(i as usize).map(|o| o.0),
    };
    match op {
        Op::Read(target) => {
            let Some(o) = obj(target) else {
                return vec![Event::Read {
                    peer,
                    target,
                    result: Err(att_err::INVALID_HANDLE),
                }];
            };
            match target {
                Target::Char(i) => {
                    *g.reads.entry(i).or_default() += 1;
                    send!(p.0, c"readValueForCharacteristic:" => (), Id = o);
                }
                Target::Desc(_) => send!(p.0, c"readValueForDescriptor:" => (), Id = o),
            }
        }
        Op::Write {
            target,
            value,
            with_response,
        } => {
            let Some(o) = obj(target) else {
                return vec![Event::Written {
                    peer,
                    target,
                    result: Err(att_err::INVALID_HANDLE),
                }];
            };
            let data = objc::nsdata(&value);
            match target {
                Target::Char(_) => {
                    let kind = if with_response {
                        WITH_RESPONSE
                    } else {
                        WITHOUT_RESPONSE
                    };
                    send!(p.0, c"writeValue:forCharacteristic:type:" => (),
                        Id = data, Id = o, i64 = kind);
                }
                Target::Desc(_) => {
                    send!(p.0, c"writeValue:forDescriptor:" => (), Id = data, Id = o)
                }
            }
        }
        Op::SetNotify { char, enable } => {
            let Some(o) = obj(Target::Char(char)) else {
                return vec![Event::NotifyState {
                    peer,
                    char,
                    result: Err(att_err::INVALID_HANDLE),
                }];
            };
            send!(p.0, c"setNotifyValue:forCharacteristic:" => (), bool = enable, Id = o);
        }
    }
    Vec::new()
}

fn uuid_of(cbuuid: Id) -> Uuid {
    let d = objc::data(send!(cbuuid, c"data" => Id));
    match d.len() {
        2 => Uuid::short(u16::from_be_bytes([d[0], d[1]]) as u32),
        4 => Uuid::short(u32::from_be_bytes([d[0], d[1], d[2], d[3]])),
        16 => Uuid(u128::from_be_bytes(d.try_into().unwrap())),
        _ => Uuid(0),
    }
}

fn cbuuid(u: Uuid) -> Id {
    let bytes = match u.as_u16() {
        Some(v) => v.to_be_bytes().to_vec(),
        None => u.0.to_be_bytes().to_vec(),
    };
    send!(objc::class(c"CBUUID"), c"UUIDWithData:" => Id, Id = objc::nsdata(&bytes))
}

fn identifier(peripheral: Id) -> [u8; 16] {
    let mut b = [0u8; 16];
    let id = send!(peripheral, c"identifier" => Id);
    send!(id, c"getUUIDBytes:" => (), *mut u8 = b.as_mut_ptr());
    b
}

/// An ATT error code from a CoreBluetooth error.
fn att_code(error: Id) -> u8 {
    let domain = objc::string(send!(error, c"domain" => Id)).unwrap_or_default();
    let code = send!(error, c"code" => i64);
    if domain == "CBATTErrorDomain" && (1..=0xff).contains(&code) {
        code as u8
    } else {
        att_err::UNLIKELY_ERROR
    }
}

fn result<T>(error: Id, ok: impl FnOnce() -> T) -> Result<T, u8> {
    if error.is_null() {
        Ok(ok())
    } else {
        Err(att_code(error))
    }
}

/// A descriptor's value in ATT form. CoreBluetooth hands some descriptors
/// over decoded: a user description as a string, the configuration ones as
/// numbers.
fn descriptor_value(v: Id) -> Vec<u8> {
    if objc::is_kind(v, c"NSData") {
        objc::data(v)
    } else if objc::is_kind(v, c"NSString") {
        objc::string(v).unwrap_or_default().into_bytes()
    } else if objc::is_kind(v, c"NSNumber") {
        (objc::number(v).unwrap_or(0) as u16).to_le_bytes().to_vec()
    } else {
        Vec::new()
    }
}

fn advertisement(keys: &Keys, d: Id) -> Advertisement {
    let uuids = |k: Id| -> Vec<Uuid> {
        objc::array(objc::get(d, k))
            .into_iter()
            .map(uuid_of)
            .collect()
    };
    let service_data = objc::get(d, keys.service_data);
    let mut services = uuids(keys.services);
    services.extend(uuids(keys.overflow));
    Advertisement {
        connectable: objc::number(objc::get(d, keys.connectable)).is_some_and(|v| v != 0),
        local_name: objc::string(objc::get(d, keys.local_name)),
        tx_power: objc::number(objc::get(d, keys.tx_power)).map(|v| v.clamp(-127, 127) as i8),
        services,
        solicited: uuids(keys.solicited),
        service_data: if service_data.is_null() {
            Vec::new()
        } else {
            objc::array(send!(service_data, c"allKeys" => Id))
                .into_iter()
                .map(|k| (uuid_of(k), objc::data(objc::get(service_data, k))))
                .collect()
        },
        manufacturer: {
            let m = objc::get(d, keys.manufacturer);
            (!m.is_null()).then(|| objc::data(m))
        },
    }
}

// Delegate callbacks. Each finds its backend by the delegate, and runs on
// the queue.

fn on(this: Id, f: impl FnOnce(&Inner, &mut Cb) -> Vec<Event>) {
    let inner = DELEGATES
        .lock()
        .unwrap()
        .iter()
        .find(|(d, _)| *d == this as usize)
        .and_then(|(_, w)| w.upgrade());
    let Some(inner) = inner else { return };
    let events = match inner.cb.lock().unwrap().as_mut() {
        Some(cb) => f(&inner, cb),
        None => return,
    };
    for e in events {
        (inner.sink)(e);
    }
}

fn peer_of(cb: &Cb, peripheral: Id) -> Option<PeerId> {
    cb.by_uuid.get(&identifier(peripheral)).copied()
}

extern "C" fn did_update_state(this: Id, _: Sel, central: Id) {
    on(this, |inner, cb| {
        let state = send!(central, c"state" => i64);
        let powered = state == POWERED_ON;
        if state == UNAUTHORIZED && !cb.warned {
            cb.warned = true;
            eprintln!(
                "[bluetooth] CoreBluetooth is not authorized for this process: the \
                 controller sees no devices until Bluetooth is allowed for it \
                 (Privacy & Security > Bluetooth)"
            );
        }
        if powered == cb.powered {
            return Vec::new();
        }
        cb.powered = powered;
        if powered {
            apply_scan(inner, cb);
            apply_connect(cb);
        } else {
            cb.scanning = None;
            cb.gatt.clear();
        }
        vec![Event::Power(powered)]
    });
}

extern "C" fn did_discover(this: Id, _: Sel, _central: Id, peripheral: Id, data: Id, rssi: Id) {
    on(this, |inner, cb| {
        let uuid = identifier(peripheral);
        let peer = *cb.by_uuid.entry(uuid).or_insert_with(|| {
            cb.peers.push(Obj::retain(peripheral));
            cb.peers.len() as PeerId - 1
        });
        // 127 means unknown, as in HCI.
        let rssi = objc::number(rssi).unwrap_or(127).clamp(-127, 127) as i8;
        vec![Event::Advertisement {
            peer,
            addr: Addr::derive(&uuid, true),
            rssi,
            adv: advertisement(&inner.keys, data),
        }]
    });
}

extern "C" fn did_connect(this: Id, _: Sel, _central: Id, peripheral: Id) {
    on(this, |_, cb| {
        let Some(peer) = peer_of(cb, peripheral) else {
            return Vec::new();
        };
        let name = objc::string(send!(peripheral, c"name" => Id)).unwrap_or_default();
        let max =
            send!(peripheral, c"maximumWriteValueLengthForType:" => usize, i64 = WITHOUT_RESPONSE);
        vec![Event::Connected {
            peer,
            name,
            mtu: (max + 3).min(u16::MAX as usize) as u16,
        }]
    });
}

extern "C" fn did_fail_to_connect(this: Id, _: Sel, _central: Id, peripheral: Id, _error: Id) {
    on(this, |_, cb| match peer_of(cb, peripheral) {
        Some(peer) => vec![Event::ConnectFailed { peer }],
        None => Vec::new(),
    });
}

extern "C" fn did_disconnect(this: Id, _: Sel, _central: Id, peripheral: Id, error: Id) {
    on(this, |_, cb| {
        let Some(peer) = peer_of(cb, peripheral) else {
            return Vec::new();
        };
        cb.gatt.remove(&peer);
        let reason = if error.is_null() {
            status::LOCAL_HOST_TERMINATED
        } else {
            match send!(error, c"code" => i64) {
                PERIPHERAL_DISCONNECTED => status::REMOTE_USER_TERMINATED,
                CONNECTION_TIMEOUT => status::CONNECTION_TIMEOUT,
                _ => status::CONNECTION_TIMEOUT,
            }
        };
        vec![Event::Disconnected { peer, reason }]
    });
}

/// One discovery callback done: report the tree when it was the last.
fn discovery_step(cb: &mut Cb, peer: PeerId, peripheral: Id, started: usize) -> Vec<Event> {
    let Some(g) = cb.gatt.get_mut(&peer).filter(|g| g.discovering) else {
        return Vec::new();
    };
    g.outstanding = (g.outstanding + started).saturating_sub(1);
    if g.outstanding > 0 {
        return Vec::new();
    }
    g.discovering = false;
    g.chars.clear();
    g.descs.clear();
    g.char_ids.clear();
    g.desc_ids.clear();
    let mut services = Vec::new();
    for s in objc::array(send!(peripheral, c"services" => Id)) {
        let mut characteristics = Vec::new();
        for c in objc::array(send!(s, c"characteristics" => Id)) {
            let id = g.chars.len() as u32;
            g.chars.push(Obj::retain(c));
            g.char_ids.insert(c as usize, id);
            let mut descriptors = Vec::new();
            for d in objc::array(send!(c, c"descriptors" => Id)) {
                let did = g.descs.len() as u32;
                g.descs.push(Obj::retain(d));
                g.desc_ids.insert(d as usize, did);
                descriptors.push(Descriptor {
                    id: did,
                    uuid: uuid_of(send!(d, c"UUID" => Id)),
                });
            }
            characteristics.push(Characteristic {
                id,
                uuid: uuid_of(send!(c, c"UUID" => Id)),
                // The ATT property bits; CoreBluetooth's higher bits
                // (encryption required) are not ATT's.
                properties: send!(c, c"properties" => usize) as u8,
                descriptors,
            });
        }
        services.push(Service {
            uuid: uuid_of(send!(s, c"UUID" => Id)),
            primary: send!(s, c"isPrimary" => bool),
            characteristics,
        });
    }
    vec![Event::Discovered {
        peer,
        services: Ok(services),
    }]
}

fn discovery_failed(cb: &mut Cb, peer: PeerId) -> Vec<Event> {
    match cb.gatt.get_mut(&peer) {
        Some(g) if g.discovering => {
            g.discovering = false;
            vec![Event::Discovered {
                peer,
                services: Err(()),
            }]
        }
        _ => Vec::new(),
    }
}

extern "C" fn did_discover_services(this: Id, _: Sel, peripheral: Id, error: Id) {
    on(this, |_, cb| {
        let Some(peer) = peer_of(cb, peripheral) else {
            return Vec::new();
        };
        if !error.is_null() {
            return discovery_failed(cb, peer);
        }
        let services = objc::array(send!(peripheral, c"services" => Id));
        for s in &services {
            send!(peripheral, c"discoverCharacteristics:forService:" => (), Id = NIL, Id = *s);
        }
        discovery_step(cb, peer, peripheral, services.len())
    });
}

extern "C" fn did_discover_characteristics(
    this: Id,
    _: Sel,
    peripheral: Id,
    service: Id,
    _error: Id,
) {
    on(this, |_, cb| {
        let Some(peer) = peer_of(cb, peripheral) else {
            return Vec::new();
        };
        // A service whose characteristics cannot be read is reported
        // without them.
        let chars = objc::array(send!(service, c"characteristics" => Id));
        for c in &chars {
            send!(peripheral, c"discoverDescriptorsForCharacteristic:" => (), Id = *c);
        }
        discovery_step(cb, peer, peripheral, chars.len())
    });
}

extern "C" fn did_discover_descriptors(this: Id, _: Sel, peripheral: Id, _c: Id, _error: Id) {
    on(this, |_, cb| match peer_of(cb, peripheral) {
        Some(peer) => discovery_step(cb, peer, peripheral, 0),
        None => Vec::new(),
    });
}

extern "C" fn did_modify_services(this: Id, _: Sel, peripheral: Id, _invalidated: Id) {
    on(this, |_, cb| {
        let Some(peer) = peer_of(cb, peripheral) else {
            return Vec::new();
        };
        if let Some(g) = cb.gatt.get_mut(&peer) {
            g.discovering = true;
            g.outstanding = 1;
            send!(peripheral, c"discoverServices:" => (), Id = NIL);
        }
        Vec::new()
    });
}

fn char_of(cb: &Cb, peripheral: Id, c: Id) -> Option<(PeerId, u32)> {
    let peer = peer_of(cb, peripheral)?;
    let id = *cb.gatt.get(&peer)?.char_ids.get(&(c as usize))?;
    Some((peer, id))
}

fn desc_of(cb: &Cb, peripheral: Id, d: Id) -> Option<(PeerId, u32)> {
    let peer = peer_of(cb, peripheral)?;
    let id = *cb.gatt.get(&peer)?.desc_ids.get(&(d as usize))?;
    Some((peer, id))
}

extern "C" fn did_update_value(this: Id, _: Sel, peripheral: Id, c: Id, error: Id) {
    on(this, |_, cb| {
        let Some((peer, id)) = char_of(cb, peripheral, c) else {
            return Vec::new();
        };
        let reads = cb.gatt.get_mut(&peer).unwrap().reads.entry(id).or_default();
        let value = || objc::data(send!(c, c"value" => Id));
        if *reads > 0 {
            *reads -= 1;
            vec![Event::Read {
                peer,
                target: Target::Char(id),
                result: result(error, value),
            }]
        } else if error.is_null() {
            vec![Event::Value {
                peer,
                char: id,
                value: value(),
            }]
        } else {
            Vec::new()
        }
    });
}

extern "C" fn did_write_value(this: Id, _: Sel, peripheral: Id, c: Id, error: Id) {
    on(this, |_, cb| match char_of(cb, peripheral, c) {
        Some((peer, id)) => vec![Event::Written {
            peer,
            target: Target::Char(id),
            result: result(error, || ()),
        }],
        None => Vec::new(),
    });
}

extern "C" fn did_update_notification_state(this: Id, _: Sel, peripheral: Id, c: Id, error: Id) {
    on(this, |_, cb| match char_of(cb, peripheral, c) {
        Some((peer, id)) => vec![Event::NotifyState {
            peer,
            char: id,
            result: result(error, || ()),
        }],
        None => Vec::new(),
    });
}

extern "C" fn did_update_descriptor(this: Id, _: Sel, peripheral: Id, d: Id, error: Id) {
    on(this, |_, cb| match desc_of(cb, peripheral, d) {
        Some((peer, id)) => vec![Event::Read {
            peer,
            target: Target::Desc(id),
            result: result(error, || descriptor_value(send!(d, c"value" => Id))),
        }],
        None => Vec::new(),
    });
}

extern "C" fn did_write_descriptor(this: Id, _: Sel, peripheral: Id, d: Id, error: Id) {
    on(this, |_, cb| match desc_of(cb, peripheral, d) {
        Some((peer, id)) => vec![Event::Written {
            peer,
            target: Target::Desc(id),
            result: result(error, || ()),
        }],
        None => Vec::new(),
    });
}

extern "C" fn advertiser_did_update_state(this: Id, _: Sel, manager: Id) {
    on(this, |inner, cb| {
        cb.advertiser_powered = send!(manager, c"state" => i64) == POWERED_ON;
        apply_advertising(inner, cb);
        Vec::new()
    });
}

extern "C" fn advertiser_did_start(_this: Id, _: Sel, _manager: Id, error: Id) {
    if !error.is_null() {
        let text = objc::string(send!(error, c"localizedDescription" => Id));
        eprintln!(
            "[bluetooth] advertising failed: {}",
            text.unwrap_or_default()
        );
    }
}

/// The delegate class, registered once per process.
fn delegate_class() -> objc::Class {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: a new class under a name only this module uses.
        let cls = unsafe {
            objc::objc_allocateClassPair(objc::class(c"NSObject"), DELEGATE_CLASS.as_ptr(), 0)
        };
        let add = |name: &CStr, imp: *const c_void, types: &CStr| {
            // SAFETY: a new class, an IMP of the selector's signature.
            unsafe { objc::class_addMethod(cls, objc::sel(name), imp, types.as_ptr()) };
        };
        type F1 = extern "C" fn(Id, Sel, Id);
        type F2 = extern "C" fn(Id, Sel, Id, Id);
        type F3 = extern "C" fn(Id, Sel, Id, Id, Id);
        type F4 = extern "C" fn(Id, Sel, Id, Id, Id, Id);
        add(
            c"centralManagerDidUpdateState:",
            did_update_state as F1 as _,
            c"v@:@",
        );
        add(
            c"centralManager:didDiscoverPeripheral:advertisementData:RSSI:",
            did_discover as F4 as _,
            c"v@:@@@@",
        );
        add(
            c"centralManager:didConnectPeripheral:",
            did_connect as F2 as _,
            c"v@:@@",
        );
        add(
            c"centralManager:didFailToConnectPeripheral:error:",
            did_fail_to_connect as F3 as _,
            c"v@:@@@",
        );
        add(
            c"centralManager:didDisconnectPeripheral:error:",
            did_disconnect as F3 as _,
            c"v@:@@@",
        );
        add(
            c"peripheral:didDiscoverServices:",
            did_discover_services as F2 as _,
            c"v@:@@",
        );
        add(
            c"peripheral:didDiscoverCharacteristicsForService:error:",
            did_discover_characteristics as F3 as _,
            c"v@:@@@",
        );
        add(
            c"peripheral:didDiscoverDescriptorsForCharacteristic:error:",
            did_discover_descriptors as F3 as _,
            c"v@:@@@",
        );
        add(
            c"peripheral:didModifyServices:",
            did_modify_services as F2 as _,
            c"v@:@@",
        );
        add(
            c"peripheral:didUpdateValueForCharacteristic:error:",
            did_update_value as F3 as _,
            c"v@:@@@",
        );
        add(
            c"peripheral:didWriteValueForCharacteristic:error:",
            did_write_value as F3 as _,
            c"v@:@@@",
        );
        add(
            c"peripheral:didUpdateNotificationStateForCharacteristic:error:",
            did_update_notification_state as F3 as _,
            c"v@:@@@",
        );
        add(
            c"peripheral:didUpdateValueForDescriptor:error:",
            did_update_descriptor as F3 as _,
            c"v@:@@@",
        );
        add(
            c"peripheral:didWriteValueForDescriptor:error:",
            did_write_descriptor as F3 as _,
            c"v@:@@@",
        );
        add(
            c"peripheralManagerDidUpdateState:",
            advertiser_did_update_state as F1 as _,
            c"v@:@",
        );
        add(
            c"peripheralManagerDidStartAdvertising:error:",
            advertiser_did_start as F2 as _,
            c"v@:@@",
        );
        // SAFETY: the class built above.
        unsafe { objc::objc_registerClassPair(cls) };
    });
    objc::class(DELEGATE_CLASS)
}
