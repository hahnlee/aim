use super::{
    CallToken, ConnectionToken, LocalNodeToken, Message, NodeToken, TransactionFailure,
    TransferToken,
};
use std::io::{self, Read, Write};

const MAGIC: &[u8; 8] = b"DABND001";
const VERSION: u16 = 5;
const HEADER_BYTES: usize = 16;
pub const MAX_PAYLOAD_BYTES: usize = 56;

const OPEN: u16 = 1;
const PUBLISH_NODE: u16 = 2;
const ROUTE_TRANSACTION: u16 = 3;
const COMPLETE_REPLY: u16 = 4;
const CLOSE: u16 = 5;
const SET_CONTEXT_MANAGER: u16 = 6;
const GET_CONTEXT_MANAGER: u16 = 7;
const REQUEST_DEATH: u16 = 8;
const CLEAR_DEATH: u16 = 9;
const OPENED: u16 = 0x8001;
const NODE_PUBLISHED: u16 = 0x8002;
const ROUTE_ACCEPTED: u16 = 0x8003;
const DELIVER_TRANSACTION: u16 = 0x8004;
const DELIVER_REPLY: u16 = 0x8005;
const TARGET_DEAD: u16 = 0x8006;
const CALLER_DEAD: u16 = 0x8007;
const CONTEXT_MANAGER_SET: u16 = 0x8008;
const CONTEXT_MANAGER_FOUND: u16 = 0x8009;
const REPLY_ACCEPTED: u16 = 0x800a;
const DEATH_REQUESTED: u16 = 0x800b;
const DEATH_CLEARED: u16 = 0x800c;
const NODE_DEAD: u16 = 0x800d;
const ROUTE_REJECTED: u16 = 0x800e;

#[derive(Debug)]
pub enum DecodeError {
    Io(io::Error),
    BadMagic,
    UnsupportedVersion,
    Oversized,
    UnknownOperation,
    WrongPayloadSize,
    ZeroToken,
    InvalidOptionalToken,
    InvalidTransactionFailure,
}

impl From<io::Error> for DecodeError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn encode(output: &mut impl Write, message: Message) -> io::Result<()> {
    let (operation, payload) = payload(message);
    let mut header = [0_u8; HEADER_BYTES];
    header[..8].copy_from_slice(MAGIC);
    header[8..10].copy_from_slice(&VERSION.to_le_bytes());
    header[10..12].copy_from_slice(&operation.to_le_bytes());
    header[12..16].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    output.write_all(&header)?;
    output.write_all(&payload)
}

pub fn decode(input: &mut impl Read) -> Result<Message, DecodeError> {
    let mut header = [0_u8; HEADER_BYTES];
    input.read_exact(&mut header)?;
    if &header[..8] != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    if u16::from_le_bytes(header[8..10].try_into().unwrap()) != VERSION {
        return Err(DecodeError::UnsupportedVersion);
    }
    let operation = u16::from_le_bytes(header[10..12].try_into().unwrap());
    let length = u32::from_le_bytes(header[12..16].try_into().unwrap()) as usize;
    if length > MAX_PAYLOAD_BYTES {
        return Err(DecodeError::Oversized);
    }
    let mut payload = [0_u8; MAX_PAYLOAD_BYTES];
    input.read_exact(&mut payload[..length])?;
    parse(operation, &payload[..length])
}

fn payload(message: Message) -> (u16, Vec<u8>) {
    let mut bytes = Vec::with_capacity(MAX_PAYLOAD_BYTES);
    let operation = match message {
        Message::OpenConnection => OPEN,
        Message::PublishNode { local } => {
            push(&mut bytes, local.get());
            PUBLISH_NODE
        }
        Message::SetContextManager { local } => {
            push(&mut bytes, local.get());
            SET_CONTEXT_MANAGER
        }
        Message::GetContextManager => GET_CONTEXT_MANAGER,
        Message::RouteTransaction {
            target,
            caller_thread,
            transfer,
            code,
            flags,
        } => {
            push(&mut bytes, target.owner().get());
            push(&mut bytes, target.local().get());
            push(&mut bytes, caller_thread);
            push(&mut bytes, transfer.get());
            push(&mut bytes, code);
            push(&mut bytes, flags);
            ROUTE_TRANSACTION
        }
        Message::CompleteReply {
            call,
            transfer,
            code,
            flags,
        } => {
            push(&mut bytes, call.get());
            push(&mut bytes, transfer.get());
            push(&mut bytes, code);
            push(&mut bytes, flags);
            COMPLETE_REPLY
        }
        Message::RequestDeath { target, cookie } => {
            push(&mut bytes, target.owner().get());
            push(&mut bytes, target.local().get());
            push(&mut bytes, cookie);
            REQUEST_DEATH
        }
        Message::ClearDeath { target, cookie } => {
            push(&mut bytes, target.owner().get());
            push(&mut bytes, target.local().get());
            push(&mut bytes, cookie);
            CLEAR_DEATH
        }
        Message::CloseConnection => CLOSE,
        Message::ConnectionOpened {
            connection,
            android_uid,
        } => {
            push(&mut bytes, connection.get());
            push(&mut bytes, android_uid);
            OPENED
        }
        Message::NodePublished { node } => {
            push(&mut bytes, node.owner().get());
            push(&mut bytes, node.local().get());
            NODE_PUBLISHED
        }
        Message::ContextManagerSet { node } => {
            push(&mut bytes, node.owner().get());
            push(&mut bytes, node.local().get());
            CONTEXT_MANAGER_SET
        }
        Message::ContextManagerFound { node } => {
            push(&mut bytes, node.map_or(0, |node| node.owner().get()));
            push(&mut bytes, node.map_or(0, |node| node.local().get()));
            CONTEXT_MANAGER_FOUND
        }
        Message::RouteAccepted { call } => {
            push(&mut bytes, call.map_or(0, CallToken::get));
            ROUTE_ACCEPTED
        }
        Message::RouteRejected { reason } => {
            push(
                &mut bytes,
                match reason {
                    TransactionFailure::DeadReply => 1_u32,
                    TransactionFailure::FailedReply => 2_u32,
                },
            );
            ROUTE_REJECTED
        }
        Message::ReplyAccepted { call } => {
            push(&mut bytes, call.get());
            REPLY_ACCEPTED
        }
        Message::DeathRequested => DEATH_REQUESTED,
        Message::DeathCleared => DEATH_CLEARED,
        Message::DeliverTransaction {
            call,
            sender,
            sender_pid,
            sender_euid,
            target,
            caller_thread,
            transfer,
            code,
            flags,
        } => {
            push(&mut bytes, call.map_or(0, CallToken::get));
            push(&mut bytes, sender.get());
            push(&mut bytes, sender_pid);
            push(&mut bytes, sender_euid);
            push(&mut bytes, target.get());
            push(&mut bytes, caller_thread);
            push(&mut bytes, transfer.get());
            push(&mut bytes, code);
            push(&mut bytes, flags);
            DELIVER_TRANSACTION
        }
        Message::DeliverReply {
            call,
            source,
            sender_pid,
            sender_euid,
            target_thread,
            transfer,
            code,
            flags,
        } => {
            push(&mut bytes, call.get());
            push(&mut bytes, source.get());
            push(&mut bytes, sender_pid);
            push(&mut bytes, sender_euid);
            push(&mut bytes, target_thread);
            push(&mut bytes, transfer.get());
            push(&mut bytes, code);
            push(&mut bytes, flags);
            DELIVER_REPLY
        }
        Message::TargetDead {
            call,
            target_thread,
        } => {
            push(&mut bytes, call.get());
            push(&mut bytes, target_thread);
            TARGET_DEAD
        }
        Message::CallerDead { call } => {
            push(&mut bytes, call.get());
            CALLER_DEAD
        }
        Message::NodeDead { cookie } => {
            push(&mut bytes, cookie);
            NODE_DEAD
        }
    };
    debug_assert!(bytes.len() <= MAX_PAYLOAD_BYTES);
    (operation, bytes)
}

fn parse(operation: u16, bytes: &[u8]) -> Result<Message, DecodeError> {
    match operation {
        OPEN => exact(bytes, 0).map(|_| Message::OpenConnection),
        PUBLISH_NODE => Ok(Message::PublishNode {
            local: token(read_exact::<8>(bytes)?)?,
        }),
        SET_CONTEXT_MANAGER => Ok(Message::SetContextManager {
            local: token(read_exact::<8>(bytes)?)?,
        }),
        GET_CONTEXT_MANAGER => exact(bytes, 0).map(|_| Message::GetContextManager),
        ROUTE_TRANSACTION => {
            exact(bytes, 40)?;
            Ok(Message::RouteTransaction {
                target: NodeToken::new(token(read_u64(bytes, 0))?, token(read_u64(bytes, 8))?),
                caller_thread: read_u64(bytes, 16),
                transfer: token(read_u64(bytes, 24))?,
                code: read_u32(bytes, 32),
                flags: read_u32(bytes, 36),
            })
        }
        COMPLETE_REPLY => {
            exact(bytes, 24)?;
            Ok(Message::CompleteReply {
                call: token(read_u64(bytes, 0))?,
                transfer: token(read_u64(bytes, 8))?,
                code: read_u32(bytes, 16),
                flags: read_u32(bytes, 20),
            })
        }
        REQUEST_DEATH | CLEAR_DEATH => {
            exact(bytes, 24)?;
            let target = NodeToken::new(token(read_u64(bytes, 0))?, token(read_u64(bytes, 8))?);
            let cookie = read_u64(bytes, 16);
            Ok(if operation == REQUEST_DEATH {
                Message::RequestDeath { target, cookie }
            } else {
                Message::ClearDeath { target, cookie }
            })
        }
        CLOSE => exact(bytes, 0).map(|_| Message::CloseConnection),
        OPENED => {
            exact(bytes, 12)?;
            Ok(Message::ConnectionOpened {
                connection: token(read_u64(bytes, 0))?,
                android_uid: read_u32(bytes, 8),
            })
        }
        NODE_PUBLISHED => {
            exact(bytes, 16)?;
            Ok(Message::NodePublished {
                node: NodeToken::new(token(read_u64(bytes, 0))?, token(read_u64(bytes, 8))?),
            })
        }
        CONTEXT_MANAGER_SET => {
            exact(bytes, 16)?;
            Ok(Message::ContextManagerSet {
                node: NodeToken::new(token(read_u64(bytes, 0))?, token(read_u64(bytes, 8))?),
            })
        }
        CONTEXT_MANAGER_FOUND => {
            exact(bytes, 16)?;
            let owner = read_u64(bytes, 0);
            let local = read_u64(bytes, 8);
            let node = match (owner, local) {
                (0, 0) => None,
                (0, _) | (_, 0) => return Err(DecodeError::InvalidOptionalToken),
                _ => Some(NodeToken::new(token(owner)?, token(local)?)),
            };
            Ok(Message::ContextManagerFound { node })
        }
        ROUTE_ACCEPTED => Ok(Message::RouteAccepted {
            call: optional_token(read_exact::<8>(bytes)?)?,
        }),
        ROUTE_REJECTED => {
            exact(bytes, 4)?;
            let reason = read_u32(bytes, 0);
            match reason {
                1 => Ok(Message::RouteRejected {
                    reason: TransactionFailure::DeadReply,
                }),
                2 => Ok(Message::RouteRejected {
                    reason: TransactionFailure::FailedReply,
                }),
                _ => Err(DecodeError::InvalidTransactionFailure),
            }
        }
        REPLY_ACCEPTED => Ok(Message::ReplyAccepted {
            call: token(read_exact::<8>(bytes)?)?,
        }),
        DEATH_REQUESTED => exact(bytes, 0).map(|_| Message::DeathRequested),
        DEATH_CLEARED => exact(bytes, 0).map(|_| Message::DeathCleared),
        DELIVER_TRANSACTION => {
            exact(bytes, 56)?;
            Ok(Message::DeliverTransaction {
                call: optional_token(read_u64(bytes, 0))?,
                sender: token(read_u64(bytes, 8))?,
                sender_pid: read_i32(bytes, 16),
                sender_euid: read_u32(bytes, 20),
                target: token(read_u64(bytes, 24))?,
                caller_thread: read_u64(bytes, 32),
                transfer: token(read_u64(bytes, 40))?,
                code: read_u32(bytes, 48),
                flags: read_u32(bytes, 52),
            })
        }
        DELIVER_REPLY => {
            exact(bytes, 48)?;
            Ok(Message::DeliverReply {
                call: token(read_u64(bytes, 0))?,
                source: token(read_u64(bytes, 8))?,
                sender_pid: read_i32(bytes, 16),
                sender_euid: read_u32(bytes, 20),
                target_thread: read_u64(bytes, 24),
                transfer: token(read_u64(bytes, 32))?,
                code: read_u32(bytes, 40),
                flags: read_u32(bytes, 44),
            })
        }
        TARGET_DEAD => {
            exact(bytes, 16)?;
            Ok(Message::TargetDead {
                call: token(read_u64(bytes, 0))?,
                target_thread: read_u64(bytes, 8),
            })
        }
        CALLER_DEAD => Ok(Message::CallerDead {
            call: token(read_exact::<8>(bytes)?)?,
        }),
        NODE_DEAD => Ok(Message::NodeDead {
            cookie: read_exact::<8>(bytes)?,
        }),
        _ => Err(DecodeError::UnknownOperation),
    }
}

fn exact(bytes: &[u8], expected: usize) -> Result<(), DecodeError> {
    (bytes.len() == expected)
        .then_some(())
        .ok_or(DecodeError::WrongPayloadSize)
}

fn read_exact<const N: usize>(bytes: &[u8]) -> Result<u64, DecodeError> {
    exact(bytes, N)?;
    Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn read_i32(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn token<T>(raw: u64) -> Result<T, DecodeError>
where
    T: Token,
{
    T::from_nonzero(raw).ok_or(DecodeError::ZeroToken)
}

fn optional_token<T>(raw: u64) -> Result<Option<T>, DecodeError>
where
    T: Token,
{
    if raw == 0 {
        Ok(None)
    } else {
        T::from_nonzero(raw)
            .map(Some)
            .ok_or(DecodeError::InvalidOptionalToken)
    }
}

trait Token: Sized {
    fn from_nonzero(raw: u64) -> Option<Self>;
}

macro_rules! impl_token {
    ($($token:ty),+ $(,)?) => {$(
        impl Token for $token {
            fn from_nonzero(raw: u64) -> Option<Self> {
                Self::from_nonzero(raw)
            }
        }
    )+};
}

impl_token!(ConnectionToken, LocalNodeToken, CallToken, TransferToken);

trait PushLe {
    fn append(self, bytes: &mut Vec<u8>);
}

impl PushLe for u64 {
    fn append(self, bytes: &mut Vec<u8>) {
        bytes.extend_from_slice(&self.to_le_bytes());
    }
}

impl PushLe for u32 {
    fn append(self, bytes: &mut Vec<u8>) {
        bytes.extend_from_slice(&self.to_le_bytes());
    }
}

impl PushLe for i32 {
    fn append(self, bytes: &mut Vec<u8>) {
        bytes.extend_from_slice(&self.to_le_bytes());
    }
}

fn push(value: &mut Vec<u8>, field: impl PushLe) {
    field.append(value);
}
