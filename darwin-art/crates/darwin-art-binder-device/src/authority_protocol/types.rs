macro_rules! token {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub struct $name(u64);

        impl $name {
            pub fn from_nonzero(raw: u64) -> Option<Self> {
                (raw != 0).then_some(Self(raw))
            }

            pub fn get(self) -> u64 {
                self.0
            }
        }
    };
}

token!(ConnectionToken);
token!(LocalNodeToken);
token!(CallToken);
token!(TransferToken);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransactionFailure {
    DeadReply,
    FailedReply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct NodeToken {
    owner: ConnectionToken,
    local: LocalNodeToken,
}

impl NodeToken {
    pub fn new(owner: ConnectionToken, local: LocalNodeToken) -> Self {
        Self { owner, local }
    }

    pub fn owner(self) -> ConnectionToken {
        self.owner
    }

    pub fn local(self) -> LocalNodeToken {
        self.local
    }
}

/// A single control message. Requests are scoped to the authenticated stream;
/// authority events contain only opaque routing/data-plane tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    OpenConnection,
    PublishNode {
        local: LocalNodeToken,
    },
    SetContextManager {
        local: LocalNodeToken,
    },
    GetContextManager,
    RouteTransaction {
        target: NodeToken,
        caller_thread: u64,
        transfer: TransferToken,
        code: u32,
        flags: u32,
    },
    CompleteReply {
        call: CallToken,
        transfer: TransferToken,
        code: u32,
        flags: u32,
    },
    RequestDeath {
        target: NodeToken,
        cookie: u64,
    },
    ClearDeath {
        target: NodeToken,
        cookie: u64,
    },
    CloseConnection,
    ConnectionOpened {
        connection: ConnectionToken,
        android_uid: u32,
    },
    NodePublished {
        node: NodeToken,
    },
    ContextManagerSet {
        node: NodeToken,
    },
    ContextManagerFound {
        node: Option<NodeToken>,
    },
    RouteAccepted {
        call: Option<CallToken>,
    },
    /// The target disappeared before the authority accepted the transaction.
    /// Unlike `TargetDead`, no profile-wide call was published.
    RouteRejected {
        reason: TransactionFailure,
    },
    ReplyAccepted {
        call: CallToken,
    },
    DeathRequested,
    DeathCleared,
    DeliverTransaction {
        call: Option<CallToken>,
        sender: ConnectionToken,
        sender_pid: i32,
        sender_euid: u32,
        target: LocalNodeToken,
        caller_thread: u64,
        transfer: TransferToken,
        code: u32,
        flags: u32,
    },
    DeliverReply {
        call: CallToken,
        source: ConnectionToken,
        sender_pid: i32,
        sender_euid: u32,
        target_thread: u64,
        transfer: TransferToken,
        code: u32,
        flags: u32,
    },
    TargetDead {
        call: CallToken,
        target_thread: u64,
    },
    CallerDead {
        call: CallToken,
    },
    NodeDead {
        cookie: u64,
    },
}
