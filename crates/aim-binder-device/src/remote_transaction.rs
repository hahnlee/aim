//! Authenticated profile-authority delivery metadata entering one binder_proc.
//! Parcel storage remains in `TransactionPayload`; macOS transport details do
//! not enter this type.

use crate::authority_protocol::{CallToken, ConnectionToken, LocalNodeToken};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteTransaction {
    pub call: Option<CallToken>,
    pub sender: ConnectionToken,
    pub sender_pid: i32,
    pub sender_euid: u32,
    pub target: LocalNodeToken,
    pub code: u32,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoteReply {
    pub call: CallToken,
    pub source: ConnectionToken,
    pub sender_pid: i32,
    pub sender_euid: u32,
    pub target_thread: u64,
    pub code: u32,
    pub flags: u32,
}
