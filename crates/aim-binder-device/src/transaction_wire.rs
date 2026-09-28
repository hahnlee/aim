//! Exact 64-bit Binder read-side transaction records. This is wire encoding,
//! not target selection, scheduling or ownership.

pub const BR_TRANSACTION: u32 = 0x8040_7202;
pub const BR_REPLY: u32 = 0x8040_7203;
pub const RECORD_SIZE: usize = 4 + 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Metadata {
    pub target_pointer: u64,
    pub target_cookie: u64,
    pub code: u32,
    pub flags: u32,
    pub sender_pid: i32,
    pub sender_euid: u32,
    pub data_size: u64,
    pub offsets_size: u64,
    pub buffer: u64,
    pub offsets: u64,
}

pub fn encode_transaction(output: &mut [u8], reply: bool, value: Metadata) -> Option<usize> {
    let record = output.get_mut(..RECORD_SIZE)?;
    record[..4].copy_from_slice(&(if reply { BR_REPLY } else { BR_TRANSACTION }).to_le_bytes());
    let payload = &mut record[4..];
    payload[0..8].copy_from_slice(&value.target_pointer.to_le_bytes());
    payload[8..16].copy_from_slice(&value.target_cookie.to_le_bytes());
    payload[16..20].copy_from_slice(&value.code.to_le_bytes());
    payload[20..24].copy_from_slice(&value.flags.to_le_bytes());
    payload[24..28].copy_from_slice(&value.sender_pid.to_le_bytes());
    payload[28..32].copy_from_slice(&value.sender_euid.to_le_bytes());
    payload[32..40].copy_from_slice(&value.data_size.to_le_bytes());
    payload[40..48].copy_from_slice(&value.offsets_size.to_le_bytes());
    payload[48..56].copy_from_slice(&value.buffer.to_le_bytes());
    payload[56..64].copy_from_slice(&value.offsets.to_le_bytes());
    Some(RECORD_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_android_uapi_layout_and_short_output() {
        let value = Metadata {
            target_pointer: 1,
            target_cookie: 2,
            code: 3,
            flags: 4,
            sender_pid: -5,
            sender_euid: 6,
            data_size: 7,
            offsets_size: 8,
            buffer: 9,
            offsets: 10,
        };
        for size in 0..RECORD_SIZE {
            assert_eq!(
                encode_transaction(&mut vec![0xcc; size], false, value),
                None
            );
        }
        let mut bytes = [0xcc; RECORD_SIZE];
        assert_eq!(
            encode_transaction(&mut bytes, false, value),
            Some(RECORD_SIZE)
        );
        assert_eq!(
            u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            BR_TRANSACTION
        );
        assert_eq!(u64::from_le_bytes(bytes[4..12].try_into().unwrap()), 1);
        assert_eq!(u64::from_le_bytes(bytes[12..20].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(bytes[20..24].try_into().unwrap()), 3);
        assert_eq!(i32::from_le_bytes(bytes[28..32].try_into().unwrap()), -5);
        assert_eq!(u64::from_le_bytes(bytes[52..60].try_into().unwrap()), 9);
        assert_eq!(u64::from_le_bytes(bytes[60..68].try_into().unwrap()), 10);
        assert_eq!(
            encode_transaction(&mut bytes, true, value),
            Some(RECORD_SIZE)
        );
        assert_eq!(
            u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            BR_REPLY
        );
    }
}
