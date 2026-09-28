//! The ATT bearer of one LE link: the remote device's ATT server, played
//! over its GATT database (Core spec Vol 3 Part F).
//!
//! The Android stack is the ATT client. CoreBluetooth offers GATT, not ATT,
//! so each request is answered from the [`Db`] laid out at connection time,
//! or turned into a GATT operation on the device ([`Op`]) whose completion
//! produces the response. ATT is sequential: one request is outstanding at a
//! time, and requests that arrive meanwhile wait their turn. Values that the
//! device notifies become Handle Value Notifications or Indications, as the
//! client configured the CCCD.

use std::collections::{HashMap, VecDeque};

use crate::gatt::{
    CCCD, CHARACTERISTIC, Db, PRIMARY_SERVICE, SECONDARY_SERVICE, Target, Uuid, Value, props,
};

/// ATT opcodes.
pub mod op {
    pub const ERROR_RSP: u8 = 0x01;
    pub const EXCHANGE_MTU_REQ: u8 = 0x02;
    pub const EXCHANGE_MTU_RSP: u8 = 0x03;
    pub const FIND_INFORMATION_REQ: u8 = 0x04;
    pub const FIND_INFORMATION_RSP: u8 = 0x05;
    pub const FIND_BY_TYPE_VALUE_REQ: u8 = 0x06;
    pub const FIND_BY_TYPE_VALUE_RSP: u8 = 0x07;
    pub const READ_BY_TYPE_REQ: u8 = 0x08;
    pub const READ_BY_TYPE_RSP: u8 = 0x09;
    pub const READ_REQ: u8 = 0x0a;
    pub const READ_RSP: u8 = 0x0b;
    pub const READ_BLOB_REQ: u8 = 0x0c;
    pub const READ_BLOB_RSP: u8 = 0x0d;
    pub const READ_BY_GROUP_TYPE_REQ: u8 = 0x10;
    pub const READ_BY_GROUP_TYPE_RSP: u8 = 0x11;
    pub const WRITE_REQ: u8 = 0x12;
    pub const WRITE_RSP: u8 = 0x13;
    pub const PREPARE_WRITE_REQ: u8 = 0x16;
    pub const PREPARE_WRITE_RSP: u8 = 0x17;
    pub const EXECUTE_WRITE_REQ: u8 = 0x18;
    pub const EXECUTE_WRITE_RSP: u8 = 0x19;
    pub const HANDLE_VALUE_NTF: u8 = 0x1b;
    pub const HANDLE_VALUE_IND: u8 = 0x1d;
    pub const HANDLE_VALUE_CFM: u8 = 0x1e;
    pub const WRITE_CMD: u8 = 0x52;
    /// Bit 6 of an opcode: a command, which gets no response.
    pub const COMMAND_FLAG: u8 = 0x40;
}

/// ATT error codes.
pub mod err {
    pub const INVALID_HANDLE: u8 = 0x01;
    pub const READ_NOT_PERMITTED: u8 = 0x02;
    pub const WRITE_NOT_PERMITTED: u8 = 0x03;
    pub const INVALID_PDU: u8 = 0x04;
    pub const REQUEST_NOT_SUPPORTED: u8 = 0x06;
    pub const INVALID_OFFSET: u8 = 0x07;
    pub const ATTRIBUTE_NOT_FOUND: u8 = 0x0a;
    pub const INVALID_ATTRIBUTE_VALUE_LENGTH: u8 = 0x0d;
    pub const UNLIKELY_ERROR: u8 = 0x0e;
    pub const UNSUPPORTED_GROUP_TYPE: u8 = 0x10;
}

pub const DEFAULT_MTU: u16 = 23;
/// The longest attribute value (Vol 3 Part F 3.2.9).
const MAX_VALUE: usize = 512;

/// A GATT operation on the device, for the backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Read(Target),
    Write {
        target: Target,
        value: Vec<u8>,
        with_response: bool,
    },
    SetNotify {
        char: u32,
        enable: bool,
    },
}

/// What the bearer asks of its link.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// An ATT PDU for the client.
    Pdu(Vec<u8>),
    Op(Op),
}

/// The response an outstanding [`Op`] completes.
#[derive(Clone, Debug)]
enum Pending {
    Read {
        opcode: u8,
        handle: u16,
        offset: u16,
    },
    Write {
        handle: u16,
    },
    Execute {
        writes: VecDeque<(u16, Target, Vec<u8>)>,
    },
    Cccd {
        handle: u16,
        value: u16,
    },
}

pub struct Bearer {
    db: Db,
    /// Our side of Exchange MTU: what the host's link supports.
    server_mtu: u16,
    mtu: u16,
    pending: Option<Pending>,
    waiting: VecDeque<Vec<u8>>,
    /// CCCD values by CCCD handle.
    cccd: HashMap<u16, u16>,
    /// Full values of the last reads, by handle, for Read Blob: the backend
    /// reads a whole value at once.
    long: HashMap<u16, Vec<u8>>,
    prepared: Vec<(u16, u16, Vec<u8>)>,
    indication_out: bool,
    indications: VecDeque<Vec<u8>>,
    out: Vec<Output>,
}

fn error(request: u8, handle: u16, code: u8) -> Vec<u8> {
    let h = handle.to_le_bytes();
    vec![op::ERROR_RSP, request, h[0], h[1], code]
}

fn u16_at(p: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([p[i], p[i + 1]])
}

impl Bearer {
    pub fn new(db: Db, server_mtu: u16) -> Self {
        Bearer {
            db,
            server_mtu: server_mtu.max(DEFAULT_MTU),
            mtu: DEFAULT_MTU,
            pending: None,
            waiting: VecDeque::new(),
            cccd: HashMap::new(),
            long: HashMap::new(),
            prepared: Vec::new(),
            indication_out: false,
            indications: VecDeque::new(),
            out: Vec::new(),
        }
    }

    /// What the bearer asks for since the last call.
    pub fn drain(&mut self) -> Vec<Output> {
        std::mem::take(&mut self.out)
    }

    fn send(&mut self, pdu: Vec<u8>) {
        self.out.push(Output::Pdu(pdu));
    }

    fn start(&mut self, pending: Pending, op: Op) {
        self.pending = Some(pending);
        self.out.push(Output::Op(op));
    }

    /// A PDU from the client.
    pub fn on_pdu(&mut self, pdu: &[u8]) {
        let Some(&opcode) = pdu.first() else { return };
        match opcode {
            op::HANDLE_VALUE_CFM => {
                self.indication_out = false;
                if let Some(next) = self.indications.pop_front() {
                    self.indication_out = true;
                    self.send(next);
                }
            }
            op::WRITE_CMD => self.write_command(pdu),
            // Other commands (signed writes) are not supported and get no
            // response. Responses and server PDUs never come from a client.
            o if o & op::COMMAND_FLAG != 0 => {}
            op::ERROR_RSP
            | op::EXCHANGE_MTU_RSP
            | op::FIND_INFORMATION_RSP
            | op::FIND_BY_TYPE_VALUE_RSP
            | op::READ_BY_TYPE_RSP
            | op::READ_RSP
            | op::READ_BLOB_RSP
            | op::READ_BY_GROUP_TYPE_RSP
            | op::WRITE_RSP
            | op::PREPARE_WRITE_RSP
            | op::EXECUTE_WRITE_RSP
            | op::HANDLE_VALUE_NTF
            | op::HANDLE_VALUE_IND => {}
            _ if self.pending.is_some() => self.waiting.push_back(pdu.to_vec()),
            _ => self.request(pdu),
        }
    }

    fn resume(&mut self) {
        while self.pending.is_none() {
            let Some(next) = self.waiting.pop_front() else {
                return;
            };
            self.request(&next);
        }
    }

    fn request(&mut self, p: &[u8]) {
        let opcode = p[0];
        let need = match opcode {
            op::EXCHANGE_MTU_REQ => 3,
            op::FIND_INFORMATION_REQ | op::READ_BLOB_REQ => 5,
            op::FIND_BY_TYPE_VALUE_REQ => 7,
            op::READ_BY_TYPE_REQ | op::READ_BY_GROUP_TYPE_REQ => 7,
            op::READ_REQ | op::WRITE_REQ => 3,
            op::PREPARE_WRITE_REQ => 5,
            op::EXECUTE_WRITE_REQ => 2,
            _ => {
                self.send(error(opcode, 0, err::REQUEST_NOT_SUPPORTED));
                return;
            }
        };
        if p.len() < need {
            self.send(error(opcode, 0, err::INVALID_PDU));
            return;
        }
        match opcode {
            op::EXCHANGE_MTU_REQ => {
                self.mtu = u16_at(p, 1).clamp(DEFAULT_MTU, self.server_mtu);
                let m = self.server_mtu.to_le_bytes();
                self.send(vec![op::EXCHANGE_MTU_RSP, m[0], m[1]]);
            }
            op::FIND_INFORMATION_REQ => self.find_information(u16_at(p, 1), u16_at(p, 3)),
            op::FIND_BY_TYPE_VALUE_REQ => {
                self.find_by_type_value(u16_at(p, 1), u16_at(p, 3), u16_at(p, 5), &p[7..])
            }
            op::READ_BY_TYPE_REQ | op::READ_BY_GROUP_TYPE_REQ => {
                let (start, end) = (u16_at(p, 1), u16_at(p, 3));
                let Some(kind) = Uuid::from_le(&p[5..]).filter(|_| p.len() == 7 || p.len() == 21)
                else {
                    self.send(error(opcode, start, err::INVALID_PDU));
                    return;
                };
                if opcode == op::READ_BY_TYPE_REQ {
                    self.read_by_type(start, end, kind);
                } else {
                    self.read_by_group_type(start, end, kind);
                }
            }
            op::READ_REQ => self.read(opcode, u16_at(p, 1), 0),
            op::READ_BLOB_REQ => self.read(opcode, u16_at(p, 1), u16_at(p, 3)),
            op::WRITE_REQ => self.write(u16_at(p, 1), &p[3..]),
            op::PREPARE_WRITE_REQ => self.prepare_write(u16_at(p, 1), u16_at(p, 3), &p[5..]),
            op::EXECUTE_WRITE_REQ => self.execute_write(p[1]),
            _ => unreachable!(),
        }
    }

    fn bad_range(&mut self, opcode: u8, start: u16, end: u16) -> bool {
        if start == 0 || start > end {
            self.send(error(opcode, start, err::INVALID_HANDLE));
            return true;
        }
        false
    }

    fn find_information(&mut self, start: u16, end: u16) {
        let opcode = op::FIND_INFORMATION_REQ;
        if self.bad_range(opcode, start, end) {
            return;
        }
        let room = self.mtu as usize - 2;
        let mut body = Vec::new();
        let mut format = 0;
        for a in self.db.range(start, end) {
            let uuid = a.kind.to_att();
            let f = if uuid.len() == 2 { 1 } else { 2 };
            if format != 0 && (f != format || body.len() + 2 + uuid.len() > room) {
                break;
            }
            format = f;
            body.extend_from_slice(&a.handle.to_le_bytes());
            body.extend(uuid);
        }
        if body.is_empty() {
            self.send(error(opcode, start, err::ATTRIBUTE_NOT_FOUND));
            return;
        }
        let mut pdu = vec![op::FIND_INFORMATION_RSP, format];
        pdu.extend(body);
        self.send(pdu);
    }

    fn find_by_type_value(&mut self, start: u16, end: u16, kind: u16, value: &[u8]) {
        let opcode = op::FIND_BY_TYPE_VALUE_REQ;
        if self.bad_range(opcode, start, end) {
            return;
        }
        let kind = Uuid::short(kind as u32);
        let room = (self.mtu as usize - 1) / 4;
        let mut pdu = vec![op::FIND_BY_TYPE_VALUE_RSP];
        // Only values held here can be compared (declarations, in
        // practice: discovery of a primary service by UUID).
        let found = self.db.range(start, end).filter(|a| {
            a.kind == kind && matches!(&a.value, Value::Local { bytes, .. } if bytes == value)
        });
        for a in found.take(room) {
            pdu.extend_from_slice(&a.handle.to_le_bytes());
            pdu.extend_from_slice(&a.end.to_le_bytes());
        }
        if pdu.len() == 1 {
            pdu = error(opcode, start, err::ATTRIBUTE_NOT_FOUND);
        }
        self.send(pdu);
    }

    fn read_by_group_type(&mut self, start: u16, end: u16, kind: Uuid) {
        let opcode = op::READ_BY_GROUP_TYPE_REQ;
        if self.bad_range(opcode, start, end) {
            return;
        }
        if kind != PRIMARY_SERVICE && kind != SECONDARY_SERVICE {
            self.send(error(opcode, start, err::UNSUPPORTED_GROUP_TYPE));
            return;
        }
        let room = self.mtu as usize - 2;
        let mut entries: Vec<u8> = Vec::new();
        let mut len = 0;
        for a in self.db.range(start, end).filter(|a| a.kind == kind) {
            let Value::Local { bytes, .. } = &a.value else {
                continue;
            };
            let l = 4 + bytes.len();
            if len != 0 && (l != len || entries.len() + l > room) {
                break;
            }
            len = l;
            entries.extend_from_slice(&a.handle.to_le_bytes());
            entries.extend_from_slice(&a.end.to_le_bytes());
            entries.extend_from_slice(bytes);
        }
        if entries.is_empty() {
            self.send(error(opcode, start, err::ATTRIBUTE_NOT_FOUND));
            return;
        }
        let mut pdu = vec![op::READ_BY_GROUP_TYPE_RSP, len as u8];
        pdu.extend(entries);
        self.send(pdu);
    }

    fn read_by_type(&mut self, start: u16, end: u16, kind: Uuid) {
        let opcode = op::READ_BY_TYPE_REQ;
        if self.bad_range(opcode, start, end) {
            return;
        }
        let first = self.db.range(start, end).find(|a| a.kind == kind).cloned();
        let Some(first) = first else {
            self.send(error(opcode, start, err::ATTRIBUTE_NOT_FOUND));
            return;
        };
        if kind == CHARACTERISTIC {
            // Declarations of equal length, as many as fit.
            let room = self.mtu as usize - 2;
            let mut entries = Vec::new();
            let mut len = 0;
            for a in self.db.range(start, end).filter(|a| a.kind == kind) {
                let Value::Local { bytes, .. } = &a.value else {
                    continue;
                };
                let l = 2 + bytes.len();
                if len != 0 && (l != len || entries.len() + l > room) {
                    break;
                }
                len = l;
                entries.extend_from_slice(&a.handle.to_le_bytes());
                entries.extend_from_slice(bytes);
            }
            let mut pdu = vec![op::READ_BY_TYPE_RSP, len as u8];
            pdu.extend(entries);
            self.send(pdu);
            return;
        }
        // Any other type: the first match alone, read like a Read Request.
        self.read(opcode, first.handle, 0);
    }

    /// The largest value part that fits a response to `opcode`.
    fn room(&self, opcode: u8) -> usize {
        match opcode {
            // opcode, length, handle; the length byte caps an entry at 255.
            op::READ_BY_TYPE_REQ => (self.mtu as usize - 4).min(253),
            _ => self.mtu as usize - 1,
        }
    }

    fn read(&mut self, opcode: u8, handle: u16, offset: u16) {
        let Some(a) = self.db.get(handle).cloned() else {
            self.send(error(opcode, handle, err::INVALID_HANDLE));
            return;
        };
        match a.value {
            Value::Local { bytes, readable } => {
                if !readable {
                    self.send(error(opcode, handle, err::READ_NOT_PERMITTED));
                } else {
                    self.respond_read(opcode, handle, offset, &bytes);
                }
            }
            Value::Cccd { .. } => {
                let v = self.cccd.get(&handle).copied().unwrap_or(0);
                self.respond_read(opcode, handle, offset, &v.to_le_bytes());
            }
            Value::Remote { target, properties } => {
                if matches!(target, Target::Char(_)) && properties & props::READ == 0 {
                    self.send(error(opcode, handle, err::READ_NOT_PERMITTED));
                    return;
                }
                // A blob read continues the value the last read returned.
                if offset > 0
                    && let Some(v) = self.long.get(&handle).cloned()
                {
                    self.respond_read(opcode, handle, offset, &v);
                    return;
                }
                self.start(
                    Pending::Read {
                        opcode,
                        handle,
                        offset,
                    },
                    Op::Read(target),
                );
            }
        }
    }

    fn respond_read(&mut self, opcode: u8, handle: u16, offset: u16, value: &[u8]) {
        let offset = offset as usize;
        if offset > value.len() {
            self.send(error(opcode, handle, err::INVALID_OFFSET));
            return;
        }
        let part = &value[offset..(offset + self.room(opcode)).min(value.len())];
        let mut pdu = match opcode {
            op::READ_BY_TYPE_REQ => {
                let h = handle.to_le_bytes();
                vec![op::READ_BY_TYPE_RSP, 2 + part.len() as u8, h[0], h[1]]
            }
            op::READ_BLOB_REQ => vec![op::READ_BLOB_RSP],
            _ => vec![op::READ_RSP],
        };
        pdu.extend_from_slice(part);
        self.send(pdu);
    }

    /// A write's target, or the error to answer with.
    fn writable(&self, handle: u16, len: usize) -> Result<Value, u8> {
        let a = self.db.get(handle).ok_or(err::INVALID_HANDLE)?;
        if len > MAX_VALUE {
            return Err(err::INVALID_ATTRIBUTE_VALUE_LENGTH);
        }
        match &a.value {
            Value::Local { .. } => Err(err::WRITE_NOT_PERMITTED),
            Value::Cccd { .. } if len != 2 => Err(err::INVALID_ATTRIBUTE_VALUE_LENGTH),
            Value::Remote {
                target: Target::Char(_),
                properties,
            } if properties & (props::WRITE | props::WRITE_WITHOUT_RESPONSE) == 0 => {
                Err(err::WRITE_NOT_PERMITTED)
            }
            v => Ok(v.clone()),
        }
    }

    fn write(&mut self, handle: u16, value: &[u8]) {
        let opcode = op::WRITE_REQ;
        match self.writable(handle, value.len()) {
            Err(e) => self.send(error(opcode, handle, e)),
            Ok(Value::Cccd { char }) => self.write_cccd(handle, char, u16_at(value, 0)),
            Ok(Value::Remote { target, .. }) => {
                self.long.remove(&handle);
                self.start(
                    Pending::Write { handle },
                    Op::Write {
                        target,
                        value: value.to_vec(),
                        with_response: true,
                    },
                );
            }
            Ok(Value::Local { .. }) => unreachable!(),
        }
    }

    fn write_cccd(&mut self, handle: u16, char: u16, value: u16) {
        let old = self.cccd.get(&handle).copied().unwrap_or(0);
        let target = match self.db.get(char).map(|a| &a.value) {
            Some(Value::Remote {
                target: Target::Char(id),
                ..
            }) => Some(*id),
            _ => None,
        };
        match target {
            // The same subscription state, or a local characteristic
            // (Service Changed): nothing to tell the device.
            Some(id) if (old != 0) != (value != 0) => self.start(
                Pending::Cccd { handle, value },
                Op::SetNotify {
                    char: id,
                    enable: value != 0,
                },
            ),
            _ => {
                self.cccd.insert(handle, value);
                self.send(vec![op::WRITE_RSP]);
            }
        }
    }

    fn write_command(&mut self, p: &[u8]) {
        if p.len() < 3 {
            return;
        }
        let handle = u16_at(p, 1);
        if let Ok(Value::Remote { target, .. }) = self.writable(handle, p.len() - 3) {
            self.long.remove(&handle);
            self.out.push(Output::Op(Op::Write {
                target,
                value: p[3..].to_vec(),
                with_response: false,
            }));
        }
    }

    fn prepare_write(&mut self, handle: u16, offset: u16, part: &[u8]) {
        let opcode = op::PREPARE_WRITE_REQ;
        match self.writable(handle, 0) {
            Ok(Value::Remote { .. }) => {}
            Ok(_) => return self.send(error(opcode, handle, err::WRITE_NOT_PERMITTED)),
            Err(e) => return self.send(error(opcode, handle, e)),
        }
        self.prepared.push((handle, offset, part.to_vec()));
        let mut pdu = vec![op::PREPARE_WRITE_RSP];
        pdu.extend_from_slice(&handle.to_le_bytes());
        pdu.extend_from_slice(&offset.to_le_bytes());
        pdu.extend_from_slice(part);
        self.send(pdu);
    }

    fn execute_write(&mut self, flags: u8) {
        let opcode = op::EXECUTE_WRITE_REQ;
        let prepared = std::mem::take(&mut self.prepared);
        if flags == 0 {
            self.send(vec![op::EXECUTE_WRITE_RSP]);
            return;
        }
        // Assemble each attribute's value from its parts, in order.
        let mut writes: Vec<(u16, Target, Vec<u8>)> = Vec::new();
        for (handle, offset, part) in prepared {
            let i = match writes.iter().position(|w| w.0 == handle) {
                Some(i) => i,
                None => {
                    let Some(Value::Remote { target, .. }) =
                        self.db.get(handle).map(|a| a.value.clone())
                    else {
                        return self.send(error(opcode, handle, err::INVALID_HANDLE));
                    };
                    writes.push((handle, target, Vec::new()));
                    writes.len() - 1
                }
            };
            let v = &mut writes[i].2;
            if offset as usize != v.len() {
                return self.send(error(opcode, handle, err::INVALID_OFFSET));
            }
            v.extend(part);
            if v.len() > MAX_VALUE {
                return self.send(error(opcode, handle, err::INVALID_ATTRIBUTE_VALUE_LENGTH));
            }
        }
        self.next_execute(writes.into())
    }

    fn next_execute(&mut self, mut writes: VecDeque<(u16, Target, Vec<u8>)>) {
        match writes.pop_front() {
            None => self.send(vec![op::EXECUTE_WRITE_RSP]),
            Some((handle, target, value)) => {
                self.long.remove(&handle);
                writes.push_front((handle, target, Vec::new()));
                self.start(
                    Pending::Execute { writes },
                    Op::Write {
                        target,
                        value,
                        with_response: true,
                    },
                );
            }
        }
    }

    /// A [`Op::Read`] of `target` completed.
    pub fn on_read(&mut self, target: Target, result: Result<Vec<u8>, u8>) {
        let Some(Pending::Read {
            opcode,
            handle,
            offset,
        }) = self.pending.clone()
        else {
            return;
        };
        if !self.is_target(handle, target) {
            return;
        }
        self.pending = None;
        match result {
            Ok(value) => {
                self.respond_read(opcode, handle, offset, &value);
                self.long.insert(handle, value);
            }
            Err(e) => self.send(error(opcode, handle, e)),
        }
        self.resume();
    }

    /// A [`Op::Write`] with response of `target` completed.
    pub fn on_write(&mut self, target: Target, result: Result<(), u8>) {
        match self.pending.clone() {
            Some(Pending::Write { handle }) if self.is_target(handle, target) => {
                self.pending = None;
                match result {
                    Ok(()) => self.send(vec![op::WRITE_RSP]),
                    Err(e) => self.send(error(op::WRITE_REQ, handle, e)),
                }
            }
            Some(Pending::Execute { mut writes })
                if writes.front().is_some_and(|w| self.is_target(w.0, target)) =>
            {
                self.pending = None;
                let (handle, ..) = writes.pop_front().unwrap();
                match result {
                    Ok(()) => self.next_execute(writes),
                    Err(e) => self.send(error(op::EXECUTE_WRITE_REQ, handle, e)),
                }
            }
            _ => return,
        }
        self.resume();
    }

    /// A [`Op::SetNotify`] of characteristic `char` completed.
    pub fn on_notify_state(&mut self, char: u32, result: Result<(), u8>) {
        let Some(Pending::Cccd { handle, value }) = self.pending.clone() else {
            return;
        };
        let owner = match self.db.get(handle).map(|a| &a.value) {
            Some(Value::Cccd { char: c }) => *c,
            _ => return,
        };
        if !self.is_target(owner, Target::Char(char)) {
            return;
        }
        self.pending = None;
        match result {
            Ok(()) => {
                self.cccd.insert(handle, value);
                self.send(vec![op::WRITE_RSP]);
            }
            Err(e) => self.send(error(op::WRITE_REQ, handle, e)),
        }
        self.resume();
    }

    fn is_target(&self, handle: u16, target: Target) -> bool {
        matches!(self.db.get(handle).map(|a| &a.value),
            Some(Value::Remote { target: t, .. }) if *t == target)
    }

    /// The device sent a new value of characteristic `char`: notify or
    /// indicate it as the client configured, or drop it if unsubscribed.
    pub fn on_value(&mut self, char: u32, value: &[u8]) {
        let Some(n) = self.db.notifiable(Target::Char(char)) else {
            return;
        };
        let cfg = self.cccd.get(&n.cccd).copied().unwrap_or(0);
        let indicate = cfg & 0x0001 == 0 && cfg & 0x0002 != 0;
        if cfg & 0x0003 == 0 {
            return;
        }
        self.long.remove(&n.value_handle);
        self.server_pdu(indicate, n.value_handle, value);
    }

    fn server_pdu(&mut self, indicate: bool, handle: u16, value: &[u8]) {
        let opcode = if indicate {
            op::HANDLE_VALUE_IND
        } else {
            op::HANDLE_VALUE_NTF
        };
        let mut pdu = vec![opcode];
        pdu.extend_from_slice(&handle.to_le_bytes());
        pdu.extend_from_slice(&value[..value.len().min(self.mtu as usize - 3)]);
        if !indicate {
            self.send(pdu);
        } else if self.indication_out {
            self.indications.push_back(pdu);
        } else {
            self.indication_out = true;
            self.send(pdu);
        }
    }

    /// The device's services changed: take the new database, and indicate
    /// Service Changed over the whole range if the client asked for it.
    pub fn replace_db(&mut self, db: Db) {
        let sc = self.db.service_changed();
        let subscribed = sc.and_then(|h| {
            let cccd = self
                .db
                .range(h + 1, h + 1)
                .find(|a| a.kind == CCCD)
                .map(|a| a.handle)?;
            self.cccd.get(&cccd).copied()
        });
        let end = db.last_handle().max(self.db.last_handle());
        self.db = db;
        self.long.clear();
        self.cccd.clear();
        if let (Some(h), Some(cfg)) = (self.db.service_changed(), subscribed) {
            if let Some(cccd) = self.db.range(h + 1, h + 1).find(|a| a.kind == CCCD) {
                self.cccd.insert(cccd.handle, cfg);
            }
            if cfg & 0x0003 != 0 {
                let mut range = 1u16.to_le_bytes().to_vec();
                range.extend_from_slice(&end.to_le_bytes());
                self.server_pdu(cfg & 0x0001 == 0, h, &range);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gatt::{Characteristic, Descriptor, Service};

    fn db() -> Db {
        Db::build(
            "Band",
            &[
                Service {
                    uuid: Uuid::short(0x180d),
                    primary: true,
                    characteristics: vec![
                        Characteristic {
                            id: 1,
                            uuid: Uuid::short(0x2a37),
                            properties: props::NOTIFY,
                            descriptors: vec![Descriptor { id: 2, uuid: CCCD }],
                        },
                        Characteristic {
                            id: 3,
                            uuid: Uuid::short(0x2a39),
                            properties: props::WRITE | props::READ,
                            descriptors: vec![],
                        },
                    ],
                },
                Service {
                    uuid: Uuid::parse("6E400001-B5A3-F393-E0A9-E50E24DCCA9E").unwrap(),
                    primary: true,
                    characteristics: vec![Characteristic {
                        id: 4,
                        uuid: Uuid::short(0x2a19),
                        properties: props::READ | props::INDICATE,
                        descriptors: vec![Descriptor {
                            id: 5,
                            uuid: Uuid::short(0x2901),
                        }],
                    }],
                },
            ],
        )
    }
    // Handles: 1 GAP, 2-3 Device Name, 4 GATT, 5-6 Service Changed, 7 its
    // CCCD; 8 heart rate, 9-10 measurement, 11 its CCCD, 12-13 control
    // point; 14 custom service, 15-16 battery level, 17 user description,
    // 18 CCCD.

    fn pdus(b: &mut Bearer) -> Vec<Vec<u8>> {
        b.drain()
            .into_iter()
            .map(|o| match o {
                Output::Pdu(p) => p,
                o => panic!("unexpected {o:?}"),
            })
            .collect()
    }

    fn one(b: &mut Bearer, req: &[u8]) -> Vec<u8> {
        b.on_pdu(req);
        let mut p = pdus(b);
        assert_eq!(p.len(), 1, "{p:?}");
        p.remove(0)
    }

    #[test]
    fn exchange_mtu() {
        let mut b = Bearer::new(db(), 185);
        assert_eq!(one(&mut b, &[0x02, 0x00, 0x02]), vec![0x03, 185, 0]);
        assert_eq!(b.mtu, 185);
        let mut b = Bearer::new(db(), 185);
        assert_eq!(one(&mut b, &[0x02, 50, 0]), vec![0x03, 185, 0]);
        assert_eq!(b.mtu, 50);
    }

    #[test]
    fn discover_primary_services() {
        let mut b = Bearer::new(db(), 23);
        // 16-bit services share an entry length; the 128-bit one comes in a
        // second request.
        assert_eq!(
            one(&mut b, &[0x10, 1, 0, 0xff, 0xff, 0x00, 0x28]),
            vec![
                0x11, 6, 1, 0, 3, 0, 0x00, 0x18, 4, 0, 7, 0, 0x01, 0x18, 8, 0, 13, 0, 0x0d, 0x18
            ]
        );
        let r = one(&mut b, &[0x10, 14, 0, 0xff, 0xff, 0x00, 0x28]);
        assert_eq!(&r[..6], &[0x11, 20, 14, 0, 18, 0]);
        assert_eq!(r[21], 0x6e);
        assert_eq!(
            one(&mut b, &[0x10, 19, 0, 0xff, 0xff, 0x00, 0x28]),
            vec![0x01, 0x10, 19, 0, err::ATTRIBUTE_NOT_FOUND]
        );
        assert_eq!(
            one(&mut b, &[0x10, 1, 0, 0xff, 0xff, 0x03, 0x28]),
            vec![0x01, 0x10, 1, 0, err::UNSUPPORTED_GROUP_TYPE]
        );
        // By UUID.
        assert_eq!(
            one(&mut b, &[0x06, 1, 0, 0xff, 0xff, 0x00, 0x28, 0x0d, 0x18]),
            vec![0x07, 8, 0, 13, 0]
        );
    }

    #[test]
    fn discover_characteristics_and_descriptors() {
        let mut b = Bearer::new(db(), 23);
        assert_eq!(
            one(&mut b, &[0x08, 8, 0, 13, 0, 0x03, 0x28]),
            vec![
                0x09, 7, 9, 0, 0x10, 10, 0, 0x37, 0x2a, 12, 0, 0x0a, 13, 0, 0x39, 0x2a
            ]
        );
        assert_eq!(
            one(&mut b, &[0x04, 11, 0, 11, 0]),
            vec![0x05, 1, 11, 0, 0x02, 0x29]
        );
        assert_eq!(
            one(&mut b, &[0x04, 17, 0, 0xff, 0xff]),
            vec![0x05, 1, 17, 0, 0x01, 0x29, 18, 0, 0x02, 0x29]
        );
        assert_eq!(
            one(&mut b, &[0x04, 0, 0, 5, 0]),
            vec![0x01, 0x04, 0, 0, err::INVALID_HANDLE]
        );
    }

    #[test]
    fn device_name_by_type_is_local() {
        let mut b = Bearer::new(db(), 23);
        assert_eq!(
            one(&mut b, &[0x08, 1, 0, 0xff, 0xff, 0x00, 0x2a]),
            vec![0x09, 6, 3, 0, b'B', b'a', b'n', b'd']
        );
        assert_eq!(
            one(&mut b, &[0x0a, 6, 0]),
            vec![0x01, 0x0a, 6, 0, err::READ_NOT_PERMITTED]
        );
    }

    #[test]
    fn remote_reads_and_blobs() {
        let mut b = Bearer::new(db(), 23);
        b.on_pdu(&[0x0a, 13, 0]);
        assert_eq!(b.drain(), vec![Output::Op(Op::Read(Target::Char(3)))]);
        // A request meanwhile waits.
        b.on_pdu(&[0x0a, 3, 0]);
        assert!(b.drain().is_empty());
        let long: Vec<u8> = (0..40).collect();
        b.on_read(Target::Char(3), Ok(long.clone()));
        let p = pdus(&mut b);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0][0], 0x0b);
        assert_eq!(&p[0][1..], &long[..22]);
        assert_eq!(p[1], vec![0x0b, b'B', b'a', b'n', b'd']);
        // The blob comes from the value already read.
        assert_eq!(one(&mut b, &[0x0c, 13, 0, 22, 0])[1..], long[22..]);
        assert_eq!(one(&mut b, &[0x0c, 13, 0, 40, 0]), vec![0x0d]);
        assert_eq!(
            one(&mut b, &[0x0c, 13, 0, 41, 0]),
            vec![0x01, 0x0c, 13, 0, err::INVALID_OFFSET]
        );
        // Errors from the device pass through; a characteristic that is not
        // readable is refused here.
        b.on_pdu(&[0x0a, 17, 0]);
        assert_eq!(b.drain(), vec![Output::Op(Op::Read(Target::Desc(5)))]);
        b.on_read(Target::Desc(5), Err(0x05));
        assert_eq!(pdus(&mut b), vec![vec![0x01, 0x0a, 17, 0, 0x05]]);
        assert_eq!(
            one(&mut b, &[0x0a, 10, 0]),
            vec![0x01, 0x0a, 10, 0, err::READ_NOT_PERMITTED]
        );
        assert_eq!(
            one(&mut b, &[0x0a, 99, 0]),
            vec![0x01, 0x0a, 99, 0, err::INVALID_HANDLE]
        );
        // Read by type of a device value.
        b.on_pdu(&[0x08, 14, 0, 0xff, 0xff, 0x19, 0x2a]);
        assert_eq!(b.drain(), vec![Output::Op(Op::Read(Target::Char(4)))]);
        b.on_read(Target::Char(4), Ok(vec![87]));
        assert_eq!(pdus(&mut b), vec![vec![0x09, 3, 16, 0, 87]]);
    }

    #[test]
    fn writes() {
        let mut b = Bearer::new(db(), 23);
        b.on_pdu(&[0x12, 13, 0, 1, 2]);
        assert_eq!(
            b.drain(),
            vec![Output::Op(Op::Write {
                target: Target::Char(3),
                value: vec![1, 2],
                with_response: true
            })]
        );
        b.on_write(Target::Char(3), Ok(()));
        assert_eq!(pdus(&mut b), vec![vec![0x13]]);
        b.on_pdu(&[0x52, 13, 0, 9]);
        assert_eq!(
            b.drain(),
            vec![Output::Op(Op::Write {
                target: Target::Char(3),
                value: vec![9],
                with_response: false
            })]
        );
        assert_eq!(
            one(&mut b, &[0x12, 3, 0, 1]),
            vec![0x01, 0x12, 3, 0, err::WRITE_NOT_PERMITTED]
        );
        assert_eq!(
            one(&mut b, &[0x12, 16, 0, 1]),
            vec![0x01, 0x12, 16, 0, err::WRITE_NOT_PERMITTED]
        );
        // A long write: prepared parts, one GATT write on execute.
        assert_eq!(
            one(&mut b, &[0x16, 13, 0, 0, 0, 1, 2, 3]),
            vec![0x17, 13, 0, 0, 0, 1, 2, 3]
        );
        assert_eq!(
            one(&mut b, &[0x16, 13, 0, 3, 0, 4]),
            vec![0x17, 13, 0, 3, 0, 4]
        );
        b.on_pdu(&[0x18, 1]);
        assert_eq!(
            b.drain(),
            vec![Output::Op(Op::Write {
                target: Target::Char(3),
                value: vec![1, 2, 3, 4],
                with_response: true
            })]
        );
        b.on_write(Target::Char(3), Ok(()));
        assert_eq!(pdus(&mut b), vec![vec![0x19]]);
        assert_eq!(one(&mut b, &[0x18, 0]), vec![0x19]);
    }

    #[test]
    fn notifications_and_indications() {
        let mut b = Bearer::new(db(), 23);
        // Unsubscribed values are dropped.
        b.on_value(1, &[1, 2]);
        assert!(b.drain().is_empty());
        b.on_pdu(&[0x12, 11, 0, 1, 0]);
        assert_eq!(
            b.drain(),
            vec![Output::Op(Op::SetNotify {
                char: 1,
                enable: true
            })]
        );
        b.on_notify_state(1, Ok(()));
        assert_eq!(pdus(&mut b), vec![vec![0x13]]);
        assert_eq!(one(&mut b, &[0x0a, 11, 0]), vec![0x0b, 1, 0]);
        b.on_value(1, &[0x06, 72]);
        assert_eq!(pdus(&mut b), vec![vec![0x1b, 10, 0, 0x06, 72]]);

        // Indications wait for the confirmation.
        b.on_pdu(&[0x12, 18, 0, 2, 0]);
        assert_eq!(
            b.drain(),
            vec![Output::Op(Op::SetNotify {
                char: 4,
                enable: true
            })]
        );
        b.on_notify_state(4, Ok(()));
        assert_eq!(pdus(&mut b), vec![vec![0x13]]);
        b.on_value(4, &[50]);
        b.on_value(4, &[49]);
        assert_eq!(pdus(&mut b), vec![vec![0x1d, 16, 0, 50]]);
        b.on_pdu(&[0x1e]);
        assert_eq!(pdus(&mut b), vec![vec![0x1d, 16, 0, 49]]);

        // Unsubscribing tells the device.
        b.on_pdu(&[0x12, 11, 0, 0, 0]);
        assert_eq!(
            b.drain(),
            vec![Output::Op(Op::SetNotify {
                char: 1,
                enable: false
            })]
        );
        b.on_notify_state(1, Err(0x0e));
        assert_eq!(pdus(&mut b), vec![vec![0x01, 0x12, 11, 0, 0x0e]]);
    }

    #[test]
    fn service_changed() {
        let mut b = Bearer::new(db(), 23);
        assert_eq!(one(&mut b, &[0x12, 7, 0, 2, 0]), vec![0x13]);
        b.replace_db(Db::build("Band", &[]));
        assert_eq!(pdus(&mut b), vec![vec![0x1d, 6, 0, 1, 0, 18, 0]]);
    }

    #[test]
    fn malformed_and_unsupported() {
        let mut b = Bearer::new(db(), 23);
        assert_eq!(
            one(&mut b, &[0x0a, 3]),
            vec![0x01, 0x0a, 0, 0, err::INVALID_PDU]
        );
        assert_eq!(
            one(&mut b, &[0x0e, 3, 0, 4, 0]),
            vec![0x01, 0x0e, 0, 0, err::REQUEST_NOT_SUPPORTED]
        );
        b.on_pdu(&[0xd2, 3, 0]);
        assert!(b.drain().is_empty());
    }
}
