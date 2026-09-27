//! What the controller needs from the host's Bluetooth: a GATT central
//! (CoreBluetooth's `CBCentralManager`) and an advertiser
//! (`CBPeripheralManager`). Requests never block; results come back as
//! [`Event`]s, in order per device.

use crate::adv::{Addr, Advertisement};
use crate::att::Op;
use crate::gatt::{Service, Target, Uuid};

/// A device the backend knows, as long as the controller is open.
pub type PeerId = u32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The host radio is usable (or no longer is).
    Power(bool),
    Advertisement {
        peer: PeerId,
        addr: Addr,
        rssi: i8,
        adv: Advertisement,
    },
    /// The link is up. `name` is the device's GAP name as the host knows
    /// it; `mtu` is the ATT MTU the host negotiated.
    Connected {
        peer: PeerId,
        name: String,
        mtu: u16,
    },
    ConnectFailed {
        peer: PeerId,
    },
    /// The link went down, with the HCI reason code.
    Disconnected {
        peer: PeerId,
        reason: u8,
    },
    /// The device's full service tree, after [`Backend::discover`] or when
    /// the device changed its services. `Err` when discovery failed.
    Discovered {
        peer: PeerId,
        services: Result<Vec<Service>, ()>,
    },
    /// Results of [`Op`]s, with ATT error codes.
    Read {
        peer: PeerId,
        target: Target,
        result: Result<Vec<u8>, u8>,
    },
    Written {
        peer: PeerId,
        target: Target,
        result: Result<(), u8>,
    },
    NotifyState {
        peer: PeerId,
        char: u32,
        result: Result<(), u8>,
    },
    /// A notified or indicated value.
    Value {
        peer: PeerId,
        char: u32,
        value: Vec<u8>,
    },
}

/// What CoreBluetooth can advertise: a local name and service UUIDs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdvertiseRequest {
    pub local_name: Option<String>,
    pub services: Vec<Uuid>,
}

pub trait Backend: Send {
    /// Scan for advertisements (while the radio is on), reporting every
    /// advertisement when `duplicates`, else each device once.
    fn scan(&mut self, on: bool, duplicates: bool);
    /// Connect to a device, however long it takes to appear.
    fn connect(&mut self, peer: PeerId);
    /// Drop a link, or give up connecting.
    fn disconnect(&mut self, peer: PeerId);
    /// Discover every service, characteristic and descriptor.
    fn discover(&mut self, peer: PeerId);
    fn gatt(&mut self, peer: PeerId, op: Op);
    fn advertise(&mut self, request: Option<AdvertiseRequest>);
}
