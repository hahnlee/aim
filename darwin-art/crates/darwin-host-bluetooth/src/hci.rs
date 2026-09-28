//! HCI numbers and encodings (Core spec Vol 4 Part E): command opcodes,
//! the Supported Commands bit of each, events, and the controller's fixed
//! capabilities. Packets carry no H4 type byte, as in `IBluetoothHci`.

/// Command opcodes (OGF << 10 | OCF) with their Supported Commands index
/// (octet * 10 + bit, Vol 4 Part E 6.27).
pub mod cmd {
    macro_rules! commands {
        ($($name:ident = $op:literal, $index:expr;)*) => {
            $(pub const $name: u16 = $op;)*
            /// Every command the controller implements.
            pub const SUPPORTED: &[(u16, u16)] = &[$(($op, $index)),*];
        };
    }
    commands! {
        DISCONNECT = 0x0406, 5;
        READ_REMOTE_VERSION_INFORMATION = 0x041d, 27;
        WRITE_DEFAULT_LINK_POLICY_SETTINGS = 0x080f, 54;
        SET_EVENT_MASK = 0x0c01, 56;
        RESET = 0x0c03, 57;
        SET_EVENT_FILTER = 0x0c05, 60;
        WRITE_LOCAL_NAME = 0x0c13, 70;
        READ_LOCAL_NAME = 0x0c14, 71;
        WRITE_PAGE_TIMEOUT = 0x0c18, 75;
        WRITE_SCAN_ENABLE = 0x0c1a, 77;
        WRITE_PAGE_SCAN_ACTIVITY = 0x0c1c, 81;
        WRITE_INQUIRY_SCAN_ACTIVITY = 0x0c1e, 83;
        WRITE_CLASS_OF_DEVICE = 0x0c24, 91;
        WRITE_VOICE_SETTING = 0x0c26, 93;
        WRITE_INQUIRY_SCAN_TYPE = 0x0c43, 125;
        WRITE_INQUIRY_MODE = 0x0c45, 127;
        WRITE_PAGE_SCAN_TYPE = 0x0c47, 131;
        WRITE_EXTENDED_INQUIRY_RESPONSE = 0x0c52, 171;
        WRITE_SIMPLE_PAIRING_MODE = 0x0c56, 176;
        SET_EVENT_MASK_PAGE_2 = 0x0c63, 222;
        WRITE_LE_HOST_SUPPORT = 0x0c6d, 246;
        READ_LOCAL_VERSION_INFORMATION = 0x1001, 143;
        READ_LOCAL_SUPPORTED_FEATURES = 0x1003, 145;
        READ_LOCAL_EXTENDED_FEATURES = 0x1004, 146;
        READ_BUFFER_SIZE = 0x1005, 147;
        READ_BD_ADDR = 0x1009, 151;
        READ_RSSI = 0x1405, 155;
        LE_SET_EVENT_MASK = 0x2001, 250;
        LE_READ_BUFFER_SIZE_V1 = 0x2002, 251;
        LE_READ_LOCAL_SUPPORTED_FEATURES = 0x2003, 252;
        LE_SET_RANDOM_ADDRESS = 0x2005, 254;
        LE_SET_ADVERTISING_PARAMETERS = 0x2006, 255;
        LE_READ_ADVERTISING_PHYSICAL_CHANNEL_TX_POWER = 0x2007, 256;
        LE_SET_ADVERTISING_DATA = 0x2008, 257;
        LE_SET_SCAN_RESPONSE_DATA = 0x2009, 260;
        LE_SET_ADVERTISING_ENABLE = 0x200a, 261;
        LE_SET_SCAN_PARAMETERS = 0x200b, 262;
        LE_SET_SCAN_ENABLE = 0x200c, 263;
        LE_CREATE_CONNECTION = 0x200d, 264;
        LE_CREATE_CONNECTION_CANCEL = 0x200e, 265;
        LE_READ_FILTER_ACCEPT_LIST_SIZE = 0x200f, 266;
        LE_CLEAR_FILTER_ACCEPT_LIST = 0x2010, 267;
        LE_ADD_DEVICE_TO_FILTER_ACCEPT_LIST = 0x2011, 270;
        LE_REMOVE_DEVICE_FROM_FILTER_ACCEPT_LIST = 0x2012, 271;
        LE_CONNECTION_UPDATE = 0x2013, 272;
        LE_SET_HOST_CHANNEL_CLASSIFICATION = 0x2014, 273;
        LE_READ_REMOTE_FEATURES = 0x2016, 275;
        LE_RAND = 0x2018, 277;
        LE_READ_SUPPORTED_STATES = 0x201c, 283;
        LE_SET_DATA_LENGTH = 0x2022, 336;
        LE_READ_SUGGESTED_DEFAULT_DATA_LENGTH = 0x2023, 337;
        LE_WRITE_SUGGESTED_DEFAULT_DATA_LENGTH = 0x2024, 340;
        LE_READ_PHY = 0x2030, 354;
        LE_SET_DEFAULT_PHY = 0x2031, 355;
        LE_SET_PHY = 0x2032, 356;
        LE_SET_ADVERTISING_SET_RANDOM_ADDRESS = 0x2035, 361;
        LE_SET_EXTENDED_ADVERTISING_PARAMETERS = 0x2036, 362;
        LE_SET_EXTENDED_ADVERTISING_DATA = 0x2037, 363;
        LE_SET_EXTENDED_SCAN_RESPONSE_DATA = 0x2038, 364;
        LE_SET_EXTENDED_ADVERTISING_ENABLE = 0x2039, 365;
        LE_READ_MAXIMUM_ADVERTISING_DATA_LENGTH = 0x203a, 366;
        LE_READ_NUMBER_OF_SUPPORTED_ADVERTISING_SETS = 0x203b, 367;
        LE_REMOVE_ADVERTISING_SET = 0x203c, 370;
        LE_CLEAR_ADVERTISING_SETS = 0x203d, 371;
        LE_SET_EXTENDED_SCAN_PARAMETERS = 0x2041, 375;
        LE_SET_EXTENDED_SCAN_ENABLE = 0x2042, 376;
        LE_EXTENDED_CREATE_CONNECTION = 0x2043, 377;
    }
    /// Always supported, so it has no bit.
    pub const READ_LOCAL_SUPPORTED_COMMANDS: u16 = 0x1002;
}

/// The Supported Commands bitmap of [`cmd::SUPPORTED`].
pub fn supported_commands() -> [u8; 64] {
    let mut m = [0u8; 64];
    for &(_, i) in cmd::SUPPORTED {
        m[(i / 10) as usize] |= 1 << (i % 10);
    }
    m
}

/// Event codes.
pub mod ev {
    pub const DISCONNECTION_COMPLETE: u8 = 0x05;
    pub const READ_REMOTE_VERSION_INFORMATION_COMPLETE: u8 = 0x0c;
    pub const COMMAND_COMPLETE: u8 = 0x0e;
    pub const COMMAND_STATUS: u8 = 0x0f;
    pub const NUMBER_OF_COMPLETED_PACKETS: u8 = 0x13;
    pub const LE_META: u8 = 0x3e;
}

/// LE Meta subevent codes.
pub mod le {
    pub const CONNECTION_COMPLETE: u8 = 0x01;
    pub const ADVERTISING_REPORT: u8 = 0x02;
    pub const CONNECTION_UPDATE_COMPLETE: u8 = 0x03;
    pub const READ_REMOTE_FEATURES_COMPLETE: u8 = 0x04;
    pub const ENHANCED_CONNECTION_COMPLETE: u8 = 0x0a;
    pub const PHY_UPDATE_COMPLETE: u8 = 0x0c;
    pub const EXTENDED_ADVERTISING_REPORT: u8 = 0x0d;
}

/// Error codes (Vol 1 Part F).
pub mod status {
    pub const SUCCESS: u8 = 0x00;
    pub const UNKNOWN_COMMAND: u8 = 0x01;
    pub const UNKNOWN_CONNECTION: u8 = 0x02;
    pub const MEMORY_CAPACITY_EXCEEDED: u8 = 0x07;
    pub const CONNECTION_TIMEOUT: u8 = 0x08;
    pub const COMMAND_DISALLOWED: u8 = 0x0c;
    pub const INVALID_PARAMETERS: u8 = 0x12;
    pub const REMOTE_USER_TERMINATED: u8 = 0x13;
    pub const LOCAL_HOST_TERMINATED: u8 = 0x16;
    pub const UNSUPPORTED_REMOTE_FEATURE: u8 = 0x1a;
    pub const UNACCEPTABLE_CONNECTION_PARAMETERS: u8 = 0x3b;
    pub const CONNECTION_FAILED_TO_BE_ESTABLISHED: u8 = 0x3e;
    pub const UNKNOWN_ADVERTISING_IDENTIFIER: u8 = 0x42;
}

/// Local Version Information: HCI and LMP 5.3, "no company" (0xffff, for
/// controllers without an assigned identifier).
pub const VERSION: u8 = 0x0c;
pub const MANUFACTURER: u16 = 0xffff;

/// LMP features page 0: LE supported (bit 38), BR/EDR not supported
/// (bit 37). The stack then runs LE only. Secure Simple Pairing (bit 51)
/// too: the Android stack aborts at start-up without it
/// (`btm_sec_dev_reset`: "only controllers with SSP is supported"), LE only
/// or not.
pub const LMP_FEATURES: u64 = 1 << 51 | 1 << 38 | 1 << 37;

/// LE features: only Extended Advertising (bit 12), which carries a
/// device's whole advertisement (CoreBluetooth merges the advertising PDU
/// and the scan response) in one report. No LE encryption: macOS pairs and
/// encrypts on its own.
pub const LE_FEATURES: u64 = 1 << 12;

/// LE Supported States: every single state and combination of scanning,
/// initiating, central and advertising.
pub const LE_STATES: u64 = 0x0000_03ff_ffff_ffff;

/// ACL buffers toward the controller: LE data packets of 251 bytes (the
/// largest LE payload), 16 of them.
pub const ACL_LENGTH: u16 = 251;
pub const ACL_PACKETS: u16 = 16;

pub const FILTER_ACCEPT_LIST_SIZE: u8 = 16;

/// Default event masks after HCI_Reset.
pub const DEFAULT_EVENT_MASK: u64 = 0x0000_1fff_ffff_ffff;
pub const DEFAULT_LE_EVENT_MASK: u64 = 0x1f;

/// Event mask bit of each maskable event (code - 1).
pub fn event_bit(code: u8) -> u64 {
    1 << (code - 1)
}

pub fn event(code: u8, params: &[u8]) -> Vec<u8> {
    let mut e = Vec::with_capacity(2 + params.len());
    e.push(code);
    e.push(params.len() as u8);
    e.extend_from_slice(params);
    e
}

pub fn command_complete(opcode: u16, params: &[u8]) -> Vec<u8> {
    let o = opcode.to_le_bytes();
    let mut p = vec![1, o[0], o[1]];
    p.extend_from_slice(params);
    event(ev::COMMAND_COMPLETE, &p)
}

pub fn command_status(opcode: u16, status: u8) -> Vec<u8> {
    let o = opcode.to_le_bytes();
    event(ev::COMMAND_STATUS, &[status, 1, o[0], o[1]])
}

pub fn le_meta(subevent: u8, params: &[u8]) -> Vec<u8> {
    let mut p = vec![subevent];
    p.extend_from_slice(params);
    event(ev::LE_META, &p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_commands_bits() {
        let m = supported_commands();
        // Octet 5 bit 7: HCI_Reset; octet 25 bit 0: LE Set Event Mask;
        // octet 37 bit 7: LE Extended Create Connection.
        assert_eq!(m[5] & 0x80, 0x80);
        assert_eq!(m[25] & 0x01, 0x01);
        assert_eq!(m[37] & 0x80, 0x80);
        // Not supported: HCI_Inquiry (octet 0 bit 0), LE Encrypt (27.6).
        assert_eq!(m[0] & 0x01, 0);
        assert_eq!(m[27] & 0xc0, 0x80);
        let mut seen = std::collections::HashSet::new();
        for &(op, _) in cmd::SUPPORTED {
            assert!(seen.insert(op), "{op:#06x} twice");
        }
    }

    #[test]
    fn encodings() {
        assert_eq!(
            command_complete(cmd::RESET, &[0]),
            vec![0x0e, 4, 1, 0x03, 0x0c, 0]
        );
        assert_eq!(
            command_status(cmd::LE_CREATE_CONNECTION, 0),
            vec![0x0f, 4, 0, 1, 0x0d, 0x20]
        );
        assert_eq!(le_meta(0x02, &[1]), vec![0x3e, 2, 0x02, 1]);
        assert_eq!(LMP_FEATURES.to_le_bytes()[4], 0x60);
        // The GD controller's SupportsSimplePairing: page 0, byte 6, bit 3.
        assert_eq!(LMP_FEATURES.to_le_bytes()[6], 0x08);
    }
}
