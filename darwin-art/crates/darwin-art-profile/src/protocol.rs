use std::io::{self, Read, Write};

const MAGIC: &[u8; 8] = b"DARTD001";
const VERSION: u16 = 1;
const MAX_PAYLOAD: usize = 64 * 1024;

pub(crate) const OP_ENSURE: u16 = 1;
pub(crate) const OP_ACQUIRE: u16 = 2;
pub(crate) const OP_STATUS: u16 = 3;
pub(crate) const OP_SHUTDOWN: u16 = 4;
pub(crate) const OP_REGISTER: u16 = 5;
pub(crate) const OP_RESOLVE: u16 = 6;
pub(crate) const OP_LIST: u16 = 7;
pub(crate) const OP_PROCESSES: u16 = 8;
pub(crate) const OP_DAEMONIZE: u16 = 9;
pub(crate) const OP_UNREGISTER: u16 = 10;
pub(crate) const OP_PROCESS_IDENTITY: u16 = 11;
pub(crate) const OP_START_RUNTIME: u16 = 12;
pub(crate) const OP_RUNTIME_READY: u16 = 13;
pub(crate) const OP_RUNTIME_LOST: u16 = 14;
pub(crate) const OP_BINDER_SESSION: u16 = 15;
pub(crate) const OP_BINDER_TRANSFER_DEPOSIT: u16 = 16;
pub(crate) const OP_BINDER_TRANSFER_TAKE: u16 = 17;
pub(crate) const OP_START_BOUND_SERVICE: u16 = 18;
pub(crate) const OP_ACTIVATE_BOUND_SERVICE: u16 = 19;
pub(crate) const STATUS_NOT_FOUND: u32 = 2;
pub(crate) const RESPONSE_BIT: u16 = 0x8000;

pub(crate) struct Message {
    pub operation: u16,
    pub payload: Vec<u8>,
}

pub(crate) fn read_message(stream: &mut impl Read) -> io::Result<Message> {
    let mut header = [0_u8; 16];
    stream.read_exact(&mut header)?;
    if &header[..8] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad protocol magic",
        ));
    }
    if u16::from_le_bytes([header[8], header[9]]) != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported protocol version",
        ));
    }
    let operation = u16::from_le_bytes([header[10], header[11]]);
    let length = u32::from_le_bytes(header[12..16].try_into().unwrap()) as usize;
    if length > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "payload too large",
        ));
    }
    let mut payload = vec![0; length];
    stream.read_exact(&mut payload)?;
    Ok(Message { operation, payload })
}

pub(crate) fn write_request(
    stream: &mut impl Write,
    operation: u16,
    payload: &[u8],
) -> io::Result<()> {
    write_message(stream, operation, payload)
}

pub(crate) fn write_response(
    stream: &mut impl Write,
    operation: u16,
    status: u32,
    payload: &[u8],
) -> io::Result<()> {
    let mut response = Vec::with_capacity(4 + payload.len());
    response.extend_from_slice(&status.to_le_bytes());
    response.extend_from_slice(payload);
    write_message(stream, operation | RESPONSE_BIT, &response)
}

fn write_message(stream: &mut impl Write, operation: u16, payload: &[u8]) -> io::Result<()> {
    if payload.len() > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "payload too large",
        ));
    }
    let mut header = [0_u8; 16];
    header[..8].copy_from_slice(MAGIC);
    header[8..10].copy_from_slice(&VERSION.to_le_bytes());
    header[10..12].copy_from_slice(&operation.to_le_bytes());
    header[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    stream.write_all(&header)?;
    stream.write_all(payload)?;
    stream.flush()
}

pub(crate) fn expect_ok(stream: &mut impl Read, operation: u16) -> io::Result<Vec<u8>> {
    let message = read_message(stream)?;
    if message.operation != operation | RESPONSE_BIT || message.payload.len() < 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "bad response envelope",
        ));
    }
    let status = u32::from_le_bytes(message.payload[..4].try_into().unwrap());
    if status != 0 {
        let detail = String::from_utf8_lossy(&message.payload[4..]);
        return Err(io::Error::other(format!("status={status} {detail}")));
    }
    Ok(message.payload[4..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_round_trip_is_versioned_and_bounded() {
        let mut bytes = Vec::new();
        write_response(&mut bytes, OP_STATUS, 0, b"ready").unwrap();
        let message = read_message(&mut bytes.as_slice()).unwrap();
        assert_eq!(message.operation, OP_STATUS | RESPONSE_BIT);
        assert_eq!(&message.payload[4..], b"ready");
        assert!(write_request(&mut Vec::new(), OP_STATUS, &vec![0; MAX_PAYLOAD + 1]).is_err());
    }
}
