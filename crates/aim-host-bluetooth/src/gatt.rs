//! A remote device's GATT database as the Android stack sees it over ATT.
//!
//! CoreBluetooth reports services, characteristics and descriptors, not
//! attribute handles. [`Db::build`] lays them out as a GATT server would:
//! each service is a declaration followed by its characteristics, each a
//! declaration, a value and its descriptors. The numbering depends only on
//! the discovered tree, so a reconnection yields the same handles.
//!
//! CoreBluetooth hides the Generic Access (0x1800) and Generic Attribute
//! (0x1801) services, which the system reads itself. They are rebuilt from
//! what it exposes: the Device Name (`CBPeripheral.name`, which macOS
//! reads from the device's own Device Name characteristic) and a Service
//! Changed characteristic, which we indicate when CoreBluetooth reports
//! modified services.

/// A Bluetooth UUID as its 128-bit value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Uuid(pub u128);

/// `0000xxxx-0000-1000-8000-00805F9B34FB`.
const BASE: u128 = 0x0000_0000_0000_1000_8000_0080_5f9b_34fb;

impl Uuid {
    pub const fn short(v: u32) -> Self {
        Uuid(BASE | (v as u128) << 96)
    }

    /// The 16-bit alias, when this is one.
    pub fn as_u16(self) -> Option<u16> {
        let v = (self.0 >> 96) as u32;
        (self.0 & !(0xffff_ffffu128 << 96) == BASE && v <= 0xffff).then_some(v as u16)
    }

    /// The 32-bit alias, when this is one (16-bit aliases included).
    pub fn as_u32(self) -> Option<u32> {
        (self.0 & !(0xffff_ffffu128 << 96) == BASE).then_some((self.0 >> 96) as u32)
    }

    /// From its little-endian ATT or AD form: 2, 4 or 16 bytes.
    pub fn from_le(b: &[u8]) -> Option<Self> {
        Some(match b.len() {
            2 => Self::short(u16::from_le_bytes([b[0], b[1]]) as u32),
            4 => Self::short(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
            16 => Uuid(u128::from_le_bytes(b.try_into().ok()?)),
            _ => return None,
        })
    }

    /// Its ATT form: 2 bytes for a 16-bit alias, 16 otherwise (ATT has no
    /// 32-bit form).
    pub fn to_att(self) -> Vec<u8> {
        match self.as_u16() {
            Some(v) => v.to_le_bytes().to_vec(),
            None => self.0.to_le_bytes().to_vec(),
        }
    }

    /// From its string form: `180D`, `0000180D`, or the full 36 characters.
    pub fn parse(s: &str) -> Option<Self> {
        let hex: String = s.chars().filter(|c| *c != '-').collect();
        let v = u128::from_str_radix(&hex, 16).ok()?;
        Some(match hex.len() {
            4 | 8 => Self::short(v as u32),
            32 => Uuid(v),
            _ => return None,
        })
    }
}

pub const PRIMARY_SERVICE: Uuid = Uuid::short(0x2800);
pub const SECONDARY_SERVICE: Uuid = Uuid::short(0x2801);
pub const CHARACTERISTIC: Uuid = Uuid::short(0x2803);
pub const CCCD: Uuid = Uuid::short(0x2902);
pub const GAP_SERVICE: Uuid = Uuid::short(0x1800);
pub const GATT_SERVICE: Uuid = Uuid::short(0x1801);
pub const DEVICE_NAME: Uuid = Uuid::short(0x2a00);
pub const SERVICE_CHANGED: Uuid = Uuid::short(0x2a05);

/// Characteristic properties (the ATT declaration's bits).
pub mod props {
    pub const READ: u8 = 0x02;
    pub const WRITE_WITHOUT_RESPONSE: u8 = 0x04;
    pub const WRITE: u8 = 0x08;
    pub const NOTIFY: u8 = 0x10;
    pub const INDICATE: u8 = 0x20;
}

/// A backend object: characteristic or descriptor ids are the backend's,
/// unique within one connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Char(u32),
    Desc(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Descriptor {
    pub id: u32,
    pub uuid: Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Characteristic {
    pub id: u32,
    pub uuid: Uuid,
    pub properties: u8,
    pub descriptors: Vec<Descriptor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Service {
    pub uuid: Uuid,
    pub primary: bool,
    pub characteristics: Vec<Characteristic>,
}

/// What an attribute's value is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    /// Held here: declarations and the rebuilt GAP/GATT characteristics.
    Local { bytes: Vec<u8>, readable: bool },
    /// A characteristic value or descriptor on the device.
    Remote { target: Target, properties: u8 },
    /// A Client Characteristic Configuration descriptor. CoreBluetooth
    /// configures it through `setNotifyValue`, so its state lives in the
    /// ATT bearer. `char` is the characteristic's value handle.
    Cccd { char: u16 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attr {
    pub handle: u16,
    pub kind: Uuid,
    pub value: Value,
    /// Last handle of the group a service declaration opens; the handle
    /// itself for other attributes.
    pub end: u16,
}

/// A characteristic whose value can be notified, by its value handle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Notifiable {
    pub value_handle: u16,
    pub properties: u8,
    pub cccd: u16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Db {
    attrs: Vec<Attr>,
}

impl Db {
    /// The database of a device named `name` with `services`.
    pub fn build(name: &str, services: &[Service]) -> Db {
        let mut db = Db::default();
        if !services.iter().any(|s| s.uuid == GAP_SERVICE) {
            db.service(GAP_SERVICE, true);
            db.local_characteristic(DEVICE_NAME, props::READ, name.as_bytes().to_vec(), true);
            db.close_service();
        }
        if !services.iter().any(|s| s.uuid == GATT_SERVICE) {
            db.service(GATT_SERVICE, true);
            db.local_characteristic(SERVICE_CHANGED, props::INDICATE, Vec::new(), false);
            db.close_service();
        }
        for s in services {
            db.service(s.uuid, s.primary);
            for c in &s.characteristics {
                let value_handle = db.next() + 1;
                db.push(CHARACTERISTIC, decl(c.properties, value_handle, c.uuid));
                db.push(
                    c.uuid,
                    Value::Remote {
                        target: Target::Char(c.id),
                        properties: c.properties,
                    },
                );
                let mut has_cccd = false;
                for d in &c.descriptors {
                    if d.uuid == CCCD {
                        has_cccd = true;
                        db.push(CCCD, Value::Cccd { char: value_handle });
                    } else {
                        db.push(
                            d.uuid,
                            Value::Remote {
                                target: Target::Desc(d.id),
                                properties: props::READ | props::WRITE,
                            },
                        );
                    }
                }
                // A notifying characteristic has a CCCD; CoreBluetooth
                // does not always report it among the descriptors.
                if !has_cccd && c.properties & (props::NOTIFY | props::INDICATE) != 0 {
                    db.push(CCCD, Value::Cccd { char: value_handle });
                }
            }
            db.close_service();
        }
        db
    }

    fn next(&self) -> u16 {
        self.attrs.len() as u16 + 1
    }

    fn push(&mut self, kind: Uuid, value: Value) {
        let handle = self.next();
        self.attrs.push(Attr {
            handle,
            kind,
            value,
            end: handle,
        });
    }

    fn service(&mut self, uuid: Uuid, primary: bool) {
        let kind = if primary {
            PRIMARY_SERVICE
        } else {
            SECONDARY_SERVICE
        };
        self.push(kind, local(uuid.to_att()));
    }

    fn close_service(&mut self) {
        let end = self.next() - 1;
        if let Some(decl) = self
            .attrs
            .iter_mut()
            .rev()
            .find(|a| a.kind == PRIMARY_SERVICE || a.kind == SECONDARY_SERVICE)
        {
            decl.end = end;
        }
    }

    fn local_characteristic(&mut self, uuid: Uuid, properties: u8, bytes: Vec<u8>, readable: bool) {
        let value_handle = self.next() + 1;
        self.push(CHARACTERISTIC, decl(properties, value_handle, uuid));
        self.push(uuid, Value::Local { bytes, readable });
        if properties & (props::NOTIFY | props::INDICATE) != 0 {
            self.push(CCCD, Value::Cccd { char: value_handle });
        }
    }

    pub fn get(&self, handle: u16) -> Option<&Attr> {
        self.attrs.get((handle as usize).checked_sub(1)?)
    }

    /// Attributes with handles in `start..=end`.
    pub fn range(&self, start: u16, end: u16) -> impl Iterator<Item = &Attr> {
        let lo = (start as usize).saturating_sub(1).min(self.attrs.len());
        let hi = (end as usize).min(self.attrs.len()).max(lo);
        self.attrs[lo..hi].iter()
    }

    pub fn last_handle(&self) -> u16 {
        self.attrs.len() as u16
    }

    /// The characteristic whose value is `target`.
    pub fn notifiable(&self, target: Target) -> Option<Notifiable> {
        let a = self
            .attrs
            .iter()
            .find(|a| matches!(a.value, Value::Remote { target: t, .. } if t == target))?;
        let Value::Remote { properties, .. } = a.value else {
            unreachable!()
        };
        let cccd = self
            .attrs
            .iter()
            .find(|c| c.value == Value::Cccd { char: a.handle })?
            .handle;
        Some(Notifiable {
            value_handle: a.handle,
            properties,
            cccd,
        })
    }

    /// The value handle of the rebuilt Service Changed characteristic.
    pub fn service_changed(&self) -> Option<u16> {
        self.attrs
            .iter()
            .find(|a| a.kind == SERVICE_CHANGED && matches!(a.value, Value::Local { .. }))
            .map(|a| a.handle)
    }
}

fn local(bytes: Vec<u8>) -> Value {
    Value::Local {
        bytes,
        readable: true,
    }
}

fn decl(properties: u8, value_handle: u16, uuid: Uuid) -> Value {
    let mut v = vec![properties];
    v.extend_from_slice(&value_handle.to_le_bytes());
    v.extend(uuid.to_att());
    local(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_forms() {
        let hr = Uuid::short(0x180d);
        assert_eq!(hr.as_u16(), Some(0x180d));
        assert_eq!(hr.to_att(), vec![0x0d, 0x18]);
        assert_eq!(Uuid::from_le(&[0x0d, 0x18]), Some(hr));
        assert_eq!(Uuid::parse("180D"), Some(hr));
        assert_eq!(
            Uuid::parse("0000180D-0000-1000-8000-00805F9B34FB"),
            Some(hr)
        );
        let custom = Uuid::parse("6E400001-B5A3-F393-E0A9-E50E24DCCA9E").unwrap();
        assert_eq!(custom.as_u16(), None);
        assert_eq!(custom.to_att().len(), 16);
        assert_eq!(custom.to_att()[15], 0x6e);
        assert_eq!(Uuid::from_le(&custom.to_att()), Some(custom));
        // A 32-bit alias has a 128-bit ATT form.
        assert_eq!(Uuid::short(0x1234_5678).to_att().len(), 16);
    }

    #[test]
    fn layout_rebuilds_gap_and_gatt() {
        let db = Db::build(
            "Band",
            &[Service {
                uuid: Uuid::short(0x180d),
                primary: true,
                characteristics: vec![Characteristic {
                    id: 7,
                    uuid: Uuid::short(0x2a37),
                    properties: props::NOTIFY,
                    descriptors: vec![],
                }],
            }],
        );
        let kinds: Vec<_> = db
            .range(1, 0xffff)
            .map(|a| (a.handle, a.kind, a.end))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (1, PRIMARY_SERVICE, 3),
                (2, CHARACTERISTIC, 2),
                (3, DEVICE_NAME, 3),
                (4, PRIMARY_SERVICE, 7),
                (5, CHARACTERISTIC, 5),
                (6, SERVICE_CHANGED, 6),
                (7, CCCD, 7),
                (8, PRIMARY_SERVICE, 11),
                (9, CHARACTERISTIC, 9),
                (10, Uuid::short(0x2a37), 10),
                (11, CCCD, 11),
            ]
        );
        assert_eq!(
            db.get(9).unwrap().value,
            local(vec![props::NOTIFY, 10, 0, 0x37, 0x2a])
        );
        assert_eq!(
            db.notifiable(Target::Char(7)),
            Some(Notifiable {
                value_handle: 10,
                properties: props::NOTIFY,
                cccd: 11
            })
        );
        assert_eq!(db.service_changed(), Some(6));
        assert_eq!(db.range(10, 0xffff).count(), 2);
        assert_eq!(db.range(20, 30).count(), 0);
    }
}
