//! Advertisements: CoreBluetooth's parsed advertisement data back into AD
//! structures (Core spec Supplement Part A), and device addresses.
//!
//! CoreBluetooth hands over a dictionary merged from the advertising PDU
//! and the scan response, and a peripheral identifier instead of an
//! address. The AD structures are rebuilt in a fixed order, and the address
//! is derived from the identifier as a random static address, which is what
//! a device with no public identity shows.

use crate::gatt::Uuid;

/// A device address, least significant byte first (HCI order).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Addr(pub [u8; 6]);

impl Addr {
    /// Fold `seed` into six bytes. A random static address has the two
    /// most significant bits set (Vol 6 Part B 1.3.2.1). Otherwise the
    /// result is marked locally administered (the IEEE bit), so it never
    /// claims a real vendor's block.
    pub fn derive(seed: &[u8], random_static: bool) -> Addr {
        // FNV-1a over the seed, two rounds for 48 bits.
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in seed.iter().chain(seed.iter()) {
            h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        let mut a = [0u8; 6];
        a.copy_from_slice(&h.to_le_bytes()[..6]);
        if random_static {
            a[5] |= 0xc0;
        } else {
            a[5] = a[5] & 0xfc | 0x02;
        }
        Addr(a)
    }
}

impl std::fmt::Display for Addr {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let a = self.0;
        write!(
            f,
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            a[5], a[4], a[3], a[2], a[1], a[0]
        )
    }
}

/// What CoreBluetooth tells of one advertisement.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Advertisement {
    pub connectable: bool,
    pub local_name: Option<String>,
    pub tx_power: Option<i8>,
    pub services: Vec<Uuid>,
    pub solicited: Vec<Uuid>,
    pub service_data: Vec<(Uuid, Vec<u8>)>,
    pub manufacturer: Option<Vec<u8>>,
}

/// AD types.
mod ad {
    pub const COMPLETE_16: u8 = 0x03;
    pub const COMPLETE_32: u8 = 0x05;
    pub const COMPLETE_128: u8 = 0x07;
    pub const SHORT_NAME: u8 = 0x08;
    pub const COMPLETE_NAME: u8 = 0x09;
    pub const TX_POWER: u8 = 0x0a;
    pub const SOLICIT_16: u8 = 0x14;
    pub const SOLICIT_128: u8 = 0x15;
    pub const SERVICE_DATA_16: u8 = 0x16;
    pub const SOLICIT_32: u8 = 0x1f;
    pub const SERVICE_DATA_32: u8 = 0x20;
    pub const SERVICE_DATA_128: u8 = 0x21;
    pub const MANUFACTURER: u8 = 0xff;
    pub const INCOMPLETE_16: u8 = 0x02;
    pub const INCOMPLETE_32: u8 = 0x04;
    pub const INCOMPLETE_128: u8 = 0x06;
}

/// The UUID's AD form: its size class (0 = 16, 1 = 32, 2 = 128 bits) and
/// little-endian bytes.
fn ad_uuid(u: Uuid) -> (usize, Vec<u8>) {
    if let Some(v) = u.as_u16() {
        (0, v.to_le_bytes().to_vec())
    } else if let Some(v) = u.as_u32() {
        (1, v.to_le_bytes().to_vec())
    } else {
        (2, u.0.to_le_bytes().to_vec())
    }
}

fn structure(kind: u8, data: &[u8]) -> Option<Vec<u8>> {
    // The length byte counts the type too.
    let len = u8::try_from(data.len() + 1).ok()?;
    let mut s = vec![len, kind];
    s.extend_from_slice(data);
    Some(s)
}

impl Advertisement {
    /// The AD structures, in the order devices usually put them.
    pub fn structures(&self) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let lists = |uuids: &[Uuid], kinds: [u8; 3], out: &mut Vec<Vec<u8>>| {
            for (class, kind) in kinds.into_iter().enumerate() {
                let data: Vec<u8> = uuids
                    .iter()
                    .map(|u| ad_uuid(*u))
                    .filter(|(c, _)| *c == class)
                    .flat_map(|(_, b)| b)
                    .collect();
                if !data.is_empty() {
                    out.extend(structure(kind, &data));
                }
            }
        };
        lists(
            &self.services,
            [ad::COMPLETE_16, ad::COMPLETE_32, ad::COMPLETE_128],
            &mut out,
        );
        if let Some(name) = &self.local_name {
            out.extend(structure(ad::COMPLETE_NAME, name.as_bytes()));
        }
        if let Some(p) = self.tx_power {
            out.extend(structure(ad::TX_POWER, &[p as u8]));
        }
        lists(
            &self.solicited,
            [ad::SOLICIT_16, ad::SOLICIT_32, ad::SOLICIT_128],
            &mut out,
        );
        for (uuid, value) in &self.service_data {
            let (class, mut data) = ad_uuid(*uuid);
            data.extend_from_slice(value);
            let kind = [
                ad::SERVICE_DATA_16,
                ad::SERVICE_DATA_32,
                ad::SERVICE_DATA_128,
            ][class];
            out.extend(structure(kind, &data));
        }
        if let Some(m) = &self.manufacturer {
            out.extend(structure(ad::MANUFACTURER, m));
        }
        out
    }

    /// All AD structures as one data block.
    pub fn data(&self) -> Vec<u8> {
        self.structures().concat()
    }

    /// Split for legacy advertising: the advertising PDU's data and the
    /// scan response's, 31 bytes each, whole structures only. Structures
    /// that fit neither are left out.
    pub fn legacy(&self) -> (Vec<u8>, Vec<u8>) {
        let (mut adv, mut rsp) = (Vec::new(), Vec::new());
        for s in self.structures() {
            if adv.len() + s.len() <= 31 {
                adv.extend(s);
            } else if rsp.len() + s.len() <= 31 {
                rsp.extend(s);
            }
        }
        (adv, rsp)
    }
}

/// What CoreBluetooth can advertise out of AD structures the host set: the
/// local name and the service UUIDs. Anything else has no API.
pub fn parse(data: &[u8]) -> (Option<String>, Vec<Uuid>) {
    let (mut name, mut services) = (None, Vec::new());
    let mut rest = data;
    while let [len, tail @ ..] = rest {
        let len = *len as usize;
        if len == 0 || tail.len() < len {
            break;
        }
        let (kind, value) = (tail[0], &tail[1..len]);
        match kind {
            ad::SHORT_NAME | ad::COMPLETE_NAME => {
                name = Some(String::from_utf8_lossy(value).into_owned())
            }
            ad::INCOMPLETE_16 | ad::COMPLETE_16 => {
                services.extend(value.chunks_exact(2).filter_map(Uuid::from_le))
            }
            ad::INCOMPLETE_32 | ad::COMPLETE_32 => {
                services.extend(value.chunks_exact(4).filter_map(Uuid::from_le))
            }
            ad::INCOMPLETE_128 | ad::COMPLETE_128 => {
                services.extend(value.chunks_exact(16).filter_map(Uuid::from_le))
            }
            _ => {}
        }
        rest = &tail[len..];
    }
    (name, services)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        let a = Addr::derive(&[1; 16], true);
        assert_eq!(a.0[5] & 0xc0, 0xc0);
        assert_eq!(a, Addr::derive(&[1; 16], true));
        assert_ne!(a, Addr::derive(&[2; 16], true));
        let p = Addr::derive(&[1; 16], false);
        assert_eq!(p.0[5] & 0x03, 0x02);
        assert_eq!(Addr([1, 2, 3, 4, 5, 0xc6]).to_string(), "C6:05:04:03:02:01");
    }

    #[test]
    fn rebuilds_ad_structures() {
        let custom = Uuid::parse("6E400001-B5A3-F393-E0A9-E50E24DCCA9E").unwrap();
        let a = Advertisement {
            connectable: true,
            local_name: Some("Thermo".into()),
            tx_power: Some(-8),
            services: vec![Uuid::short(0x180d), Uuid::short(0x180f), custom],
            solicited: vec![],
            service_data: vec![(Uuid::short(0xfeaa), vec![0x10, 0x20])],
            manufacturer: Some(vec![0x4c, 0x00, 0x02]),
        };
        let s = a.structures();
        assert_eq!(s[0], vec![5, 0x03, 0x0d, 0x18, 0x0f, 0x18]);
        assert_eq!(s[1][..2], [17, 0x07]);
        assert_eq!(s[1][17], 0x6e);
        assert_eq!(s[2], b"\x07\x09Thermo".to_vec());
        assert_eq!(s[3], vec![2, 0x0a, 0xf8]);
        assert_eq!(s[4], vec![5, 0x16, 0xaa, 0xfe, 0x10, 0x20]);
        assert_eq!(s[5], vec![4, 0xff, 0x4c, 0x00, 0x02]);
        let (adv, rsp) = a.legacy();
        // The name does not fit after the UUIDs; the TX power does.
        assert_eq!(adv.len(), 6 + 18 + 3);
        assert_eq!(rsp.len(), 8 + 6 + 5);
        assert_eq!(adv.len() + rsp.len(), a.data().len());

        assert_eq!(
            parse(&a.data()),
            (Some("Thermo".into()), a.services.clone())
        );
        assert_eq!(parse(&[3, 0x03, 0x0d]), (None, vec![]));
        assert_eq!(parse(&[0, 1, 2]), (None, vec![]));
    }
}
