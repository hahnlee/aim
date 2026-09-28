use crate::authority_protocol::{ConnectionToken, Message};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PeerIdentity {
    pid: u32,
    android_uid: u32,
    incarnation: [u64; 2],
}

impl PeerIdentity {
    /// The transport constructs this only after OS peer-credential and process
    /// incarnation verification. It is never decoded from Binder control bytes.
    pub fn verified(pid: u32, android_uid: u32, incarnation: [u64; 2]) -> Option<Self> {
        (pid != 0 && pid <= i32::MAX as u32).then_some(Self {
            pid,
            android_uid,
            incarnation,
        })
    }

    pub fn pid(self) -> u32 {
        self.pid
    }

    pub fn incarnation(self) -> [u64; 2] {
        self.incarnation
    }

    pub fn android_uid(self) -> u32 {
        self.android_uid
    }
}

#[derive(Clone)]
pub struct Session {
    pub(super) authority: Arc<()>,
    pub(super) connection: ConnectionToken,
}

impl Session {
    pub fn connection(&self) -> ConnectionToken {
        self.connection
    }
}

#[derive(Clone)]
pub struct Opened {
    pub session: Session,
    pub response: Message,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outbound {
    pub destination: ConnectionToken,
    pub message: Message,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteOutcome {
    pub acknowledgement: Message,
    pub delivery: Outbound,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CloseOutcome {
    pub notifications: Vec<Outbound>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    ForeignAuthority,
    UnknownConnection,
    DeadTarget,
    UnknownNode,
    UnknownCall,
    WrongReplier,
    ContextManagerBusy,
    ContextManagerSecurity,
    ContextManagerWrongUid,
    InvalidThread,
    ConnectionIdsExhausted,
    CallIdsExhausted,
    DeathAlreadyRequested,
    UnknownDeath,
    OutOfMemory,
    Poisoned,
}
