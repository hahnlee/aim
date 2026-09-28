//! The `property_service` socket protocol (bionic
//! libc/include/sys/system_properties.h, libc/bionic/system_property_set.cpp,
//! init property_service.cpp `handle_property_set_fd`).
//!
//! Sockets: `/dev/socket/property_service` (mode 0666, owner root:root) and
//! `/dev/socket/property_service_for_system` (mode 0660, group system), both
//! `SOCK_STREAM` and listening with a backlog of 8. bionic sends
//! `sys.powerctl` to the second one when it can write to it.
//!
//! A connection carries one request. All integers are native-endian `u32`
//! (little-endian on arm64):
//!
//! ```text
//! PROP_MSG_SETPROP2 (0x00020001):
//!   u32 cmd | u32 name_len | name bytes | u32 value_len | value bytes
//!   reply: u32 status (PROP_SUCCESS or PROP_ERROR_*)
//! PROP_MSG_SETPROP (1), legacy, used when ro.property_service.version < 2:
//!   u32 cmd | char name[32] | char value[92]     (no reply; init closes)
//! ```
//!
//! init reads with a 5 s total timeout and refuses strings over 0xffff bytes.

use super::area::AreaMemory;
use super::area::{PROP_NAME_MAX, PROP_VALUE_MAX};
use super::service::{PropertyService, SetEffect, Ucred};

pub const PROP_SERVICE_NAME: &str = "property_service";
pub const PROP_SERVICE_FOR_SYSTEM_NAME: &str = "property_service_for_system";
pub const PROP_SERVICE_SOCKET: &str = "/dev/socket/property_service";
pub const PROP_SERVICE_FOR_SYSTEM_SOCKET: &str = "/dev/socket/property_service_for_system";

pub const PROP_MSG_SETPROP: u32 = 1;
pub const PROP_MSG_SETPROP2: u32 = 0x0002_0001;

pub const PROP_SUCCESS: u32 = 0;
pub const PROP_ERROR_READ_CMD: u32 = 0x0004;
pub const PROP_ERROR_READ_DATA: u32 = 0x0008;
pub const PROP_ERROR_READ_ONLY_PROPERTY: u32 = 0x000B;
pub const PROP_ERROR_INVALID_NAME: u32 = 0x0010;
pub const PROP_ERROR_INVALID_VALUE: u32 = 0x0014;
pub const PROP_ERROR_PERMISSION_DENIED: u32 = 0x0018;
pub const PROP_ERROR_INVALID_CMD: u32 = 0x001B;
pub const PROP_ERROR_HANDLE_CONTROL_MESSAGE: u32 = 0x0020;
pub const PROP_ERROR_SET_FAILED: u32 = 0x0024;

/// init's `kDefaultSocketTimeout` for reading one request.
pub const SOCKET_TIMEOUT_MS: u32 = 5000;
/// `RecvString` refuses larger strings ("don't allow init to make
/// arbitrarily large allocations").
pub const MAX_STRING_LEN: u32 = 0xffff;

/// bionic's `__prop_error_to_string`.
pub fn error_name(code: u32) -> &'static str {
    match code {
        PROP_ERROR_READ_CMD => "PROP_ERROR_READ_CMD",
        PROP_ERROR_READ_DATA => "PROP_ERROR_READ_DATA",
        PROP_ERROR_READ_ONLY_PROPERTY => "PROP_ERROR_READ_ONLY_PROPERTY",
        PROP_ERROR_INVALID_NAME => "PROP_ERROR_INVALID_NAME",
        PROP_ERROR_INVALID_VALUE => "PROP_ERROR_INVALID_VALUE",
        PROP_ERROR_PERMISSION_DENIED => "PROP_ERROR_PERMISSION_DENIED",
        PROP_ERROR_INVALID_CMD => "PROP_ERROR_INVALID_CMD",
        PROP_ERROR_HANDLE_CONTROL_MESSAGE => "PROP_ERROR_HANDLE_CONTROL_MESSAGE",
        PROP_ERROR_SET_FAILED => "PROP_ERROR_SET_FAILED",
        _ => "<unknown>",
    }
}

/// A decoded request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// `PROP_MSG_SETPROP2`. Bytes as sent (may be non-UTF-8).
    SetProp2 { name: Vec<u8>, value: Vec<u8> },
    /// Legacy `PROP_MSG_SETPROP`, strings cut at their last byte as init does.
    SetProp { name: Vec<u8>, value: Vec<u8> },
}

/// Result of decoding what has been received so far.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decoded {
    /// More bytes are needed.
    Incomplete,
    /// A full request and the number of bytes it used.
    Complete(Request, usize),
    /// init would reply with this code (if the command allows a reply) and
    /// close the connection.
    Reject(u32),
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    bytes
        .get(at..at + 4)
        .map(|b| u32::from_ne_bytes(b.try_into().unwrap()))
}

fn c_string(bytes: &[u8]) -> Vec<u8> {
    bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())].to_vec()
}

/// Decodes a request from the bytes received so far on a connection.
pub fn decode_request(bytes: &[u8]) -> Decoded {
    let Some(cmd) = read_u32(bytes, 0) else {
        return Decoded::Incomplete;
    };
    match cmd {
        PROP_MSG_SETPROP => {
            let total = 4 + PROP_NAME_MAX + PROP_VALUE_MAX;
            if bytes.len() < total {
                return Decoded::Incomplete;
            }
            let mut name = bytes[4..4 + PROP_NAME_MAX].to_vec();
            let mut value = bytes[4 + PROP_NAME_MAX..total].to_vec();
            name[PROP_NAME_MAX - 1] = 0;
            value[PROP_VALUE_MAX - 1] = 0;
            Decoded::Complete(
                Request::SetProp {
                    name: c_string(&name),
                    value: c_string(&value),
                },
                total,
            )
        }
        PROP_MSG_SETPROP2 => {
            let mut at = 4;
            let mut strings = Vec::with_capacity(2);
            for _ in 0..2 {
                let Some(len) = read_u32(bytes, at) else {
                    return Decoded::Incomplete;
                };
                if len > MAX_STRING_LEN {
                    return Decoded::Reject(PROP_ERROR_READ_DATA);
                }
                at += 4;
                let len = len as usize;
                let Some(string) = bytes.get(at..at + len) else {
                    return Decoded::Incomplete;
                };
                strings.push(string.to_vec());
                at += len;
            }
            let value = strings.pop().unwrap();
            let name = strings.pop().unwrap();
            Decoded::Complete(Request::SetProp2 { name, value }, at)
        }
        _ => Decoded::Reject(PROP_ERROR_INVALID_CMD),
    }
}

/// What a connection that ends before a full request gets: init replies
/// `PROP_ERROR_READ_CMD` when the command word is missing and
/// `PROP_ERROR_READ_DATA` for a truncated `SETPROP2`; legacy requests get
/// no reply.
pub fn truncated_reply(bytes: &[u8]) -> Option<u32> {
    match read_u32(bytes, 0) {
        None => Some(PROP_ERROR_READ_CMD),
        Some(PROP_MSG_SETPROP2) => Some(PROP_ERROR_READ_DATA),
        Some(PROP_MSG_SETPROP) => None,
        Some(_) => Some(PROP_ERROR_INVALID_CMD),
    }
}

/// bionic's `SocketWriter` output for `__system_property_set` (protocol 2).
pub fn encode_setprop2(name: &[u8], value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + name.len() + value.len());
    out.extend_from_slice(&PROP_MSG_SETPROP2.to_ne_bytes());
    out.extend_from_slice(&(name.len() as u32).to_ne_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(&(value.len() as u32).to_ne_bytes());
    out.extend_from_slice(value);
    out
}

/// bionic's legacy `prop_msg` (`strlcpy` into zeroed fixed buffers).
pub fn encode_setprop(name: &[u8], value: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 4 + PROP_NAME_MAX + PROP_VALUE_MAX];
    out[..4].copy_from_slice(&PROP_MSG_SETPROP.to_ne_bytes());
    let n = name.len().min(PROP_NAME_MAX - 1);
    out[4..4 + n].copy_from_slice(&name[..n]);
    let v = value.len().min(PROP_VALUE_MAX - 1);
    out[4 + PROP_NAME_MAX..4 + PROP_NAME_MAX + v].copy_from_slice(&value[..v]);
    out
}

/// A reply word.
pub fn encode_reply(code: u32) -> [u8; 4] {
    code.to_ne_bytes()
}

/// What the socket thread does with one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Served {
    /// Send this word, then close. `None`: close without a reply (legacy),
    /// or keep the connection for a later reply (see `deferred`).
    pub reply: Option<u32>,
    /// The reply is owed later (control message handled by init's main
    /// loop, or an asynchronous persist write).
    pub deferred: bool,
    pub error: Option<String>,
    pub effects: Vec<SetEffect>,
}

/// `handle_property_set_fd` after the request has been read: the
/// permission hook sees `source_context` (from `getpeercon`, which fails
/// closed with `PROP_ERROR_PERMISSION_DENIED` when unavailable).
pub fn serve<M: AreaMemory>(
    service: &mut PropertyService<M>,
    request: &Request,
    source_context: Option<&str>,
    cred: &Ucred,
) -> Served {
    match request {
        Request::SetProp2 { name, value } => {
            let Some(source_context) = source_context else {
                return Served {
                    reply: Some(PROP_ERROR_PERMISSION_DENIED),
                    deferred: false,
                    error: Some("getpeercon() failed".to_string()),
                    effects: Vec::new(),
                };
            };
            let name = String::from_utf8_lossy(name).into_owned();
            let outcome = service.handle_set(&name, value, source_context, cred, true);
            Served {
                deferred: outcome.reply.is_none(),
                reply: outcome.reply,
                error: outcome.error,
                effects: outcome.effects,
            }
        }
        Request::SetProp { name, value } => {
            let Some(source_context) = source_context else {
                return Served {
                    reply: None,
                    deferred: false,
                    error: Some("getpeercon() failed".to_string()),
                    effects: Vec::new(),
                };
            };
            let name = String::from_utf8_lossy(name).into_owned();
            let outcome = service.handle_set(&name, value, source_context, cred, false);
            Served {
                reply: None,
                deferred: false,
                error: outcome.error,
                effects: outcome.effects,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setprop2_round_trip() {
        let bytes = encode_setprop2(b"debug.foo", b"bar");
        assert_eq!(
            bytes,
            [
                &0x0002_0001u32.to_le_bytes()[..],
                &9u32.to_le_bytes(),
                b"debug.foo",
                &3u32.to_le_bytes(),
                b"bar"
            ]
            .concat()
        );
        assert_eq!(
            decode_request(&bytes),
            Decoded::Complete(
                Request::SetProp2 {
                    name: b"debug.foo".to_vec(),
                    value: b"bar".to_vec()
                },
                bytes.len()
            )
        );
        for cut in 0..bytes.len() {
            assert_eq!(decode_request(&bytes[..cut]), Decoded::Incomplete);
        }
        let empty = encode_setprop2(b"a", b"");
        assert!(
            matches!(decode_request(&empty), Decoded::Complete(Request::SetProp2 { value, .. }, 13) if value.is_empty())
        );
    }

    #[test]
    fn rejects_like_init() {
        let mut huge = PROP_MSG_SETPROP2.to_ne_bytes().to_vec();
        huge.extend_from_slice(&0x10000u32.to_ne_bytes());
        assert_eq!(decode_request(&huge), Decoded::Reject(PROP_ERROR_READ_DATA));
        assert_eq!(
            decode_request(&7u32.to_ne_bytes()),
            Decoded::Reject(PROP_ERROR_INVALID_CMD)
        );
        assert_eq!(truncated_reply(&[]), Some(PROP_ERROR_READ_CMD));
    }

    #[test]
    fn legacy_message() {
        let bytes = encode_setprop(b"a.b", b"c");
        assert_eq!(bytes.len(), 128);
        assert_eq!(
            decode_request(&bytes),
            Decoded::Complete(
                Request::SetProp {
                    name: b"a.b".to_vec(),
                    value: b"c".to_vec()
                },
                128
            )
        );
    }
}
