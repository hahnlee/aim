//! L2CAP on an LE ACL link (Core spec Vol 3 Part A): basic frames split
//! over HCI ACL packets, and the fixed channels a link carries. ATT is
//! served by [`crate::att`]; the LE signaling channel and the Security
//! Manager get the answers of a device that offers neither credit-based
//! channels nor pairing through the host, since macOS owns pairing.

pub const CID_ATT: u16 = 0x0004;
pub const CID_SIGNALING: u16 = 0x0005;
pub const CID_SMP: u16 = 0x0006;

/// ACL packet boundary flags (bits 12-13 of the handle field).
const PB_FIRST_NON_FLUSHABLE: u16 = 0b00;
const PB_CONTINUING: u16 = 0b01;
const PB_FIRST_FLUSHABLE: u16 = 0b10;

/// Reassembles basic frames from the host's ACL packets on one link.
#[derive(Default)]
pub struct Reassembler {
    buf: Vec<u8>,
}

impl Reassembler {
    /// Take an ACL packet's payload with its boundary flag. Returns each
    /// complete frame as (channel, payload).
    pub fn push(&mut self, pb: u16, data: &[u8]) -> Option<(u16, Vec<u8>)> {
        match pb {
            PB_FIRST_NON_FLUSHABLE | PB_FIRST_FLUSHABLE => self.buf = data.to_vec(),
            PB_CONTINUING if !self.buf.is_empty() => self.buf.extend_from_slice(data),
            _ => return None,
        }
        if self.buf.len() < 4 {
            return None;
        }
        let len = u16::from_le_bytes([self.buf[0], self.buf[1]]) as usize;
        if self.buf.len() < 4 + len {
            return None;
        }
        let cid = u16::from_le_bytes([self.buf[2], self.buf[3]]);
        let frame = self.buf[4..4 + len].to_vec();
        self.buf.clear();
        Some((cid, frame))
    }
}

/// Split a basic frame into ACL packets (with HCI ACL headers) of at most
/// `max` payload bytes each.
pub fn fragment(handle: u16, cid: u16, payload: &[u8], max: usize) -> Vec<Vec<u8>> {
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    frame.extend_from_slice(&cid.to_le_bytes());
    frame.extend_from_slice(payload);
    frame
        .chunks(max)
        .enumerate()
        .map(|(i, chunk)| {
            let pb = if i == 0 {
                PB_FIRST_FLUSHABLE
            } else {
                PB_CONTINUING
            };
            let mut p = Vec::with_capacity(4 + chunk.len());
            p.extend_from_slice(&(handle & 0x0fff | pb << 12).to_le_bytes());
            p.extend_from_slice(&(chunk.len() as u16).to_le_bytes());
            p.extend_from_slice(chunk);
            p
        })
        .collect()
}

/// The reply to an LE signaling channel packet from the host, if any.
pub fn signaling(packet: &[u8]) -> Option<Vec<u8>> {
    const COMMAND_REJECT: u8 = 0x01;
    const DISCONNECTION_REQUEST: u8 = 0x06;
    const DISCONNECTION_RESPONSE: u8 = 0x07;
    const CONNECTION_PARAMETER_UPDATE_REQUEST: u8 = 0x12;
    const CONNECTION_PARAMETER_UPDATE_RESPONSE: u8 = 0x13;
    const LE_CREDIT_BASED_CONNECTION_REQUEST: u8 = 0x14;
    const LE_CREDIT_BASED_CONNECTION_RESPONSE: u8 = 0x15;
    const CREDIT_BASED_CONNECTION_REQUEST: u8 = 0x17;
    const CREDIT_BASED_CONNECTION_RESPONSE: u8 = 0x18;
    const SPSM_NOT_SUPPORTED: u16 = 0x0002;
    const PARAMETERS_REJECTED: u16 = 0x0001;

    if packet.len() < 4 {
        return None;
    }
    let (code, id) = (packet[0], packet[1]);
    let data = &packet[4..];
    let reply = |code: u8, body: &[u8]| {
        let mut r = vec![code, id];
        r.extend_from_slice(&(body.len() as u16).to_le_bytes());
        r.extend_from_slice(body);
        r
    };
    Some(match code {
        // Responses and rejects end an exchange.
        COMMAND_REJECT
        | DISCONNECTION_RESPONSE
        | CONNECTION_PARAMETER_UPDATE_RESPONSE
        | LE_CREDIT_BASED_CONNECTION_RESPONSE
        | CREDIT_BASED_CONNECTION_RESPONSE => return None,
        // No dynamic channel is ever open, but a disconnection request is
        // always answered with its channel ids.
        DISCONNECTION_REQUEST if data.len() >= 4 => reply(
            DISCONNECTION_RESPONSE,
            &[data[0], data[1], data[2], data[3]],
        ),
        // Only a peripheral asks; the device's link parameters are the
        // host's to choose.
        CONNECTION_PARAMETER_UPDATE_REQUEST => reply(
            CONNECTION_PARAMETER_UPDATE_RESPONSE,
            &PARAMETERS_REJECTED.to_le_bytes(),
        ),
        // Credit-based channels to the device would need
        // `CBPeripheral.openL2CAPChannel`, which is not wired up
        // (docs/bluetooth.md).
        LE_CREDIT_BASED_CONNECTION_REQUEST => {
            let mut body = [0u8; 10];
            body[8..].copy_from_slice(&SPSM_NOT_SUPPORTED.to_le_bytes());
            reply(LE_CREDIT_BASED_CONNECTION_RESPONSE, &body)
        }
        CREDIT_BASED_CONNECTION_REQUEST if data.len() >= 8 => {
            let channels = (data.len() - 8) / 2;
            let mut body = vec![0u8; 8 + 2 * channels];
            body[6..8].copy_from_slice(&SPSM_NOT_SUPPORTED.to_le_bytes());
            reply(CREDIT_BASED_CONNECTION_RESPONSE, &body)
        }
        // Command not understood.
        _ => reply(COMMAND_REJECT, &[0, 0]),
    })
}

/// The reply to a Security Manager packet from the host, if any.
pub fn security_manager(packet: &[u8]) -> Option<Vec<u8>> {
    const PAIRING_REQUEST: u8 = 0x01;
    const PAIRING_FAILED: u8 = 0x05;
    const PAIRING_NOT_SUPPORTED: u8 = 0x05;
    // macOS pairs with the device itself when a protected attribute needs
    // it; the Android stack cannot pair over this link.
    (packet.first() == Some(&PAIRING_REQUEST)).then(|| vec![PAIRING_FAILED, PAIRING_NOT_SUPPORTED])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_and_reassembles() {
        let payload: Vec<u8> = (0..30).collect();
        let packets = fragment(0x0041, CID_ATT, &payload, 16);
        assert_eq!(packets.len(), 3);
        assert_eq!(&packets[0][..4], &[0x41, 0x20, 16, 0]);
        assert_eq!(&packets[0][4..8], &[30, 0, 4, 0]);
        assert_eq!(&packets[1][..4], &[0x41, 0x10, 16, 0]);
        assert_eq!(&packets[2][..4], &[0x41, 0x10, 2, 0]);

        let mut r = Reassembler::default();
        let mut out = None;
        for p in &packets {
            let pb = u16::from_le_bytes([p[0], p[1]]) >> 12 & 0b11;
            out = r.push(pb, &p[4..]);
        }
        assert_eq!(out, Some((CID_ATT, payload)));
        // A continuation with nothing started is dropped.
        assert_eq!(r.push(PB_CONTINUING, &[1, 2]), None);
        assert_eq!(
            r.push(PB_FIRST_NON_FLUSHABLE, &[1, 0, 6, 0, 9]),
            Some((CID_SMP, vec![9]))
        );
    }

    #[test]
    fn signaling_replies() {
        // LE credit based connection request: refused, SPSM not supported.
        let req = [0x14, 7, 10, 0, 0x80, 0, 0x40, 0, 0x17, 0, 0xf7, 0, 10, 0];
        assert_eq!(
            signaling(&req),
            Some(vec![0x15, 7, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0])
        );
        assert_eq!(
            signaling(&[0x06, 3, 4, 0, 0x40, 0, 0x41, 0]),
            Some(vec![0x07, 3, 4, 0, 0x40, 0, 0x41, 0])
        );
        assert_eq!(signaling(&[0x0a, 5, 0, 0]), Some(vec![0x01, 5, 2, 0, 0, 0]));
        assert_eq!(signaling(&[0x13, 5, 2, 0, 0, 0]), None);
        assert_eq!(
            security_manager(&[0x01, 3, 0, 1, 16, 7, 7]),
            Some(vec![0x05, 0x05])
        );
        assert_eq!(security_manager(&[0x0b, 1]), None);
    }
}
