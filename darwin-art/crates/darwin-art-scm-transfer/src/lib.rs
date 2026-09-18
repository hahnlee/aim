//! Narrow native ownership for a provider-managed SCM_RIGHTS transfer.
//!
//! This crate owns only the sender-side aliases and their EOF guardian. It
//! does not authenticate protocol messages, inspect fd/SO/PCB identity, or
//! implement a daemon wire protocol. The caller supplies already-authenticated
//! sender and receiver grants and performs the actual ancillary-FD transfer.

use std::collections::{HashMap, HashSet};
mod guardian;
use guardian::{duplicate, guardian_is_eof, make_guardian};
pub mod capabilities;
pub mod inheritance;
use std::fmt;
use std::io;
#[cfg(test)]
use std::os::fd::RawFd;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};

pub const MAX_PAYLOADS: usize = 16;
pub const MAX_GLOBAL_LEASES: usize = 256;
pub const MAX_PROCESS_LEASES: usize = 64;
pub const MAX_TOMBSTONES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AuthorityEpoch {
    pub instance: u128,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProcessEpoch {
    pub pid: u32,
    pub instance: u128,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CarrierId {
    pub authority: AuthorityEpoch,
    pub serial: u64,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Side {
    A,
    B,
}

impl Side {
    fn opposite(self, other: Self) -> bool {
        matches!((self, other), (Self::A, Self::B) | (Self::B, Self::A))
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EndpointId {
    pub carrier: CarrierId,
    pub side: Side,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TrustedSendContext {
    pub sender: ProcessEpoch,
    pub endpoint: EndpointId,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TrustedReceiveContext {
    pub receiver: ProcessEpoch,
    pub endpoint: EndpointId,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TransferKey {
    pub authority: AuthorityEpoch,
    pub ticket: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeaseState {
    Prepared,
    Queued,
    Imported,
    TerminalSender,
    TerminalCarrier,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Retirement {
    NotReady,
    Retired,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImportAdmission {
    pub key: TransferKey,
    pub authority: AuthorityEpoch,
    pub payload_count: usize,
}

#[derive(Debug)]
pub enum LeaseError {
    InvalidIdentity,
    InvalidKey,
    InvalidPayloadCount,
    LimitExceeded,
    Sealed,
    UnknownTransfer,
    WrongState(LeaseState),
    IdentityMismatch,
    WrongEndpoint,
    SenderDead,
    CarrierTerminated,
    CountMismatch {
        expected: usize,
        actual: usize,
    },
    Io {
        operation: &'static str,
        source: io::Error,
    },
}

impl fmt::Display for LeaseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentity => write!(formatter, "invalid trusted transfer identity"),
            Self::InvalidKey => write!(formatter, "invalid transfer key"),
            Self::InvalidPayloadCount => write!(formatter, "invalid payload count"),
            Self::LimitExceeded => write!(formatter, "SCM transfer lease limit exceeded"),
            Self::Sealed => write!(formatter, "SCM transfer owner is sealed"),
            Self::UnknownTransfer => write!(formatter, "unknown SCM transfer"),
            Self::WrongState(state) => write!(formatter, "invalid lease state: {state:?}"),
            Self::IdentityMismatch => write!(formatter, "transfer identity mismatch"),
            Self::WrongEndpoint => write!(formatter, "transfer endpoints are not opposite sides"),
            Self::SenderDead => write!(formatter, "sender process epoch is dead"),
            Self::CarrierTerminated => write!(formatter, "carrier is terminated"),
            Self::CountMismatch { expected, actual } => {
                write!(
                    formatter,
                    "payload count mismatch: expected {expected}, got {actual}"
                )
            }
            Self::Io { operation, source } => write!(formatter, "{operation} failed: {source}"),
        }
    }
}

impl std::error::Error for LeaseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct Lease {
    key: TransferKey,
    send: TrustedSendContext,
    payload_aliases: Vec<OwnedFd>,
    guardian_read: OwnedFd,
    state: LeaseState,
}

#[derive(Debug)]
pub struct PreparedTransfer {
    key: TransferKey,
    guardian_write: OwnedFd,
    payload_count: usize,
}

impl PreparedTransfer {
    pub fn key(&self) -> TransferKey {
        self.key
    }

    pub fn payload_count(&self) -> usize {
        self.payload_count
    }

    /// Borrow the guardian writer for the caller's authenticated SCM transfer.
    /// The writer remains owned by this object until the caller drops it.
    pub fn guardian_writer(&self) -> BorrowedFd<'_> {
        self.guardian_write.as_fd()
    }
}

#[derive(Debug)]
pub struct ScmTransferLeaseOwner {
    authority: AuthorityEpoch,
    leases: HashMap<TransferKey, Lease>,
    process_counts: HashMap<ProcessEpoch, usize>,
    dead_senders: HashSet<ProcessEpoch>,
    terminated_carriers: HashSet<CarrierId>,
    sealed: bool,
    max_global: usize,
    max_per_process: usize,
    next_ticket: u64,
    last_prepared_ticket: u64,
}

impl ScmTransferLeaseOwner {
    pub fn new(authority: AuthorityEpoch, max_global: usize, max_per_process: usize) -> Self {
        Self {
            authority,
            leases: HashMap::new(),
            process_counts: HashMap::new(),
            dead_senders: HashSet::new(),
            terminated_carriers: HashSet::new(),
            sealed: false,
            max_global: max_global.min(MAX_GLOBAL_LEASES),
            max_per_process: max_per_process.min(MAX_PROCESS_LEASES),
            next_ticket: 0,
            last_prepared_ticket: 0,
        }
    }

    pub fn authority(&self) -> AuthorityEpoch {
        self.authority
    }

    pub fn active_count(&self) -> usize {
        self.leases.len()
    }

    pub fn active_count_for(&self, process: ProcessEpoch) -> usize {
        self.process_counts.get(&process).copied().unwrap_or(0)
    }

    pub fn state(&self, key: TransferKey) -> Result<LeaseState, LeaseError> {
        self.leases
            .get(&key)
            .map(|lease| lease.state)
            .ok_or(LeaseError::UnknownTransfer)
    }

    /// Mint a ticket that this owner will accept exactly once. The ticket is
    /// intentionally owner-generated; protocol authentication still belongs to
    /// the caller that conveys the key to the receiver.
    pub fn mint_key(&mut self) -> Result<TransferKey, LeaseError> {
        if self.sealed {
            return Err(LeaseError::Sealed);
        }
        let Some(ticket) = self.next_ticket.checked_add(1) else {
            self.sealed = true;
            return Err(LeaseError::Sealed);
        };
        self.next_ticket = ticket;
        Ok(TransferKey {
            authority: self.authority,
            ticket,
        })
    }

    /// Duplicate payload aliases and create a nonblocking EOF guardian. The
    /// sender context is already authenticated outside this crate.
    pub fn prepare(
        &mut self,
        send: TrustedSendContext,
        key: TransferKey,
        payloads: &[BorrowedFd<'_>],
    ) -> Result<PreparedTransfer, LeaseError> {
        self.check_open()?;
        validate_send(self.authority, send)?;
        self.validate_minted_key(key)?;
        if self.dead_senders.contains(&send.sender) {
            return Err(LeaseError::SenderDead);
        }
        if self.terminated_carriers.contains(&send.endpoint.carrier) {
            return Err(LeaseError::CarrierTerminated);
        }
        if payloads.is_empty() || payloads.len() > MAX_PAYLOADS {
            return Err(LeaseError::InvalidPayloadCount);
        }
        if self.leases.len() >= self.max_global
            || self.active_count_for(send.sender) >= self.max_per_process
        {
            return Err(LeaseError::LimitExceeded);
        }

        self.leases
            .try_reserve(1)
            .map_err(|_| LeaseError::LimitExceeded)?;
        if !self.process_counts.contains_key(&send.sender) {
            self.process_counts
                .try_reserve(1)
                .map_err(|_| LeaseError::LimitExceeded)?;
        }
        let mut aliases = Vec::new();
        aliases
            .try_reserve(payloads.len())
            .map_err(|_| LeaseError::LimitExceeded)?;
        let (guardian_read, guardian_write) = make_guardian()?;
        for payload in payloads {
            aliases.push(duplicate(*payload).map_err(|source| LeaseError::Io {
                operation: "duplicate payload alias",
                source,
            })?);
        }
        self.leases.insert(
            key,
            Lease {
                key,
                send,
                payload_aliases: aliases,
                guardian_read,
                state: LeaseState::Prepared,
            },
        );
        self.process_counts
            .entry(send.sender)
            .and_modify(|count| *count += 1)
            .or_insert(1);
        self.last_prepared_ticket = key.ticket;
        Ok(PreparedTransfer {
            key,
            guardian_write,
            payload_count: payloads.len(),
        })
    }

    /// Arm before enqueue/send so an immediate receiver admission cannot race
    /// a post-send state update. This method does not perform the send.
    pub fn arm_enqueued(
        &mut self,
        send: TrustedSendContext,
        key: TransferKey,
    ) -> Result<(), LeaseError> {
        self.check_open()?;
        validate_send(self.authority, send)?;
        if self.dead_senders.contains(&send.sender) {
            return Err(LeaseError::SenderDead);
        }
        if self.terminated_carriers.contains(&send.endpoint.carrier) {
            return Err(LeaseError::CarrierTerminated);
        }
        let lease = self
            .leases
            .get_mut(&key)
            .ok_or(LeaseError::UnknownTransfer)?;
        if lease.send != send {
            return Err(LeaseError::IdentityMismatch);
        }
        if lease.state != LeaseState::Prepared {
            return Err(LeaseError::WrongState(lease.state));
        }
        lease.state = LeaseState::Queued;
        Ok(())
    }

    /// Remove only a never-armed transfer. Armed send failures must instead
    /// drop all guardian writers and retire by observed EOF.
    pub fn cancel_unsent(
        &mut self,
        send: TrustedSendContext,
        key: TransferKey,
    ) -> Result<(), LeaseError> {
        validate_send(self.authority, send)?;
        let lease = self.leases.get(&key).ok_or(LeaseError::UnknownTransfer)?;
        if lease.send != send {
            return Err(LeaseError::IdentityMismatch);
        }
        if lease.state != LeaseState::Prepared {
            return Err(LeaseError::WrongState(lease.state));
        }
        self.remove_lease(key);
        Ok(())
    }

    /// Admit a receiver that owns the received payload FDs and guardian writer.
    /// The writer is deliberately not passed here: receiver-side publication
    /// keeps it alive transactionally and closes it after this admission.
    pub fn admit_import(
        &mut self,
        receive: TrustedReceiveContext,
        key: TransferKey,
        payload_count: usize,
    ) -> Result<ImportAdmission, LeaseError> {
        let admission = self.validate_import(receive, key, payload_count)?;
        self.leases.get_mut(&key).expect("validated lease").state = LeaseState::Imported;
        Ok(admission)
    }

    /// Read-only preflight for a service-held atomic capability claim batch.
    /// The service must keep the same owner lock through reservation/admission;
    /// this result is not a reusable authorization token across mutations.
    pub fn validate_import(
        &self,
        receive: TrustedReceiveContext,
        key: TransferKey,
        payload_count: usize,
    ) -> Result<ImportAdmission, LeaseError> {
        self.check_open()?;
        validate_receive(self.authority, receive)?;
        if self.terminated_carriers.contains(&receive.endpoint.carrier) {
            return Err(LeaseError::CarrierTerminated);
        }
        let lease = self.leases.get(&key).ok_or(LeaseError::UnknownTransfer)?;
        if lease.key != key {
            return Err(LeaseError::InvalidKey);
        }
        if lease.send.endpoint.carrier != receive.endpoint.carrier
            || !lease.send.endpoint.side.opposite(receive.endpoint.side)
        {
            return Err(LeaseError::WrongEndpoint);
        }
        if lease.state == LeaseState::TerminalCarrier {
            return Err(LeaseError::WrongState(LeaseState::TerminalCarrier));
        }
        if !matches!(lease.state, LeaseState::Queued | LeaseState::TerminalSender) {
            return Err(LeaseError::WrongState(lease.state));
        }
        if payload_count != lease.payload_aliases.len() {
            return Err(LeaseError::CountMismatch {
                expected: lease.payload_aliases.len(),
                actual: payload_count,
            });
        }
        Ok(ImportAdmission {
            key,
            authority: self.authority,
            payload_count,
        })
    }

    /// Mark one authenticated sender incarnation dead. Queued leases remain
    /// admissible, while new preparation/arming for this epoch is rejected.
    pub fn sender_died(&mut self, sender: ProcessEpoch) -> Result<(), LeaseError> {
        validate_process(sender)?;
        self.record_sender_tombstone(sender)?;
        for lease in self.leases.values_mut() {
            if lease.send.sender == sender && lease.state == LeaseState::Queued {
                lease.state = LeaseState::TerminalSender;
            }
        }
        Ok(())
    }

    /// Seal a carrier. Existing aliases remain owned until guardian EOF, but
    /// no later admission may use the carrier.
    pub fn carrier_terminated(&mut self, carrier: CarrierId) -> Result<(), LeaseError> {
        validate_carrier(self.authority, carrier)?;
        self.record_carrier_tombstone(carrier)?;
        for lease in self.leases.values_mut() {
            if lease.send.endpoint.carrier == carrier
                && matches!(lease.state, LeaseState::Queued | LeaseState::Imported)
            {
                lease.state = LeaseState::TerminalCarrier;
            }
        }
        Ok(())
    }

    /// Poll with timeout zero and retire only after actual guardian EOF. No
    /// native wait occurs and no timeout is treated as proof of release.
    /// Exclusive owner access keeps the guardian alive through this poll;
    /// no temporary duplicate FD is needed.
    pub fn retire_if_eof(&mut self, key: TransferKey) -> Result<Retirement, LeaseError> {
        let lease = self.leases.get(&key).ok_or(LeaseError::UnknownTransfer)?;
        if lease.state == LeaseState::Prepared {
            return Err(LeaseError::WrongState(lease.state));
        }
        let eof =
            guardian_is_eof(lease.guardian_read.as_raw_fd()).map_err(|source| LeaseError::Io {
                operation: "poll guardian read",
                source,
            })?;
        if !eof {
            return Ok(Retirement::NotReady);
        }
        self.remove_lease(key);
        Ok(Retirement::Retired)
    }

    fn check_open(&self) -> Result<(), LeaseError> {
        if self.sealed {
            Err(LeaseError::Sealed)
        } else {
            Ok(())
        }
    }

    fn validate_minted_key(&self, key: TransferKey) -> Result<(), LeaseError> {
        if key.authority != self.authority
            || key.ticket == 0
            || key.ticket > self.next_ticket
            || key.ticket <= self.last_prepared_ticket
            || self.leases.contains_key(&key)
        {
            Err(LeaseError::InvalidKey)
        } else {
            Ok(())
        }
    }

    fn record_sender_tombstone(&mut self, sender: ProcessEpoch) -> Result<(), LeaseError> {
        if self.dead_senders.contains(&sender) {
            return Ok(());
        }
        if self.dead_senders.len() >= MAX_TOMBSTONES {
            self.sealed = true;
            return Err(LeaseError::Sealed);
        }
        self.dead_senders.insert(sender);
        Ok(())
    }

    fn record_carrier_tombstone(&mut self, carrier: CarrierId) -> Result<(), LeaseError> {
        if self.terminated_carriers.contains(&carrier) {
            return Ok(());
        }
        if self.terminated_carriers.len() >= MAX_TOMBSTONES {
            self.sealed = true;
            return Err(LeaseError::Sealed);
        }
        self.terminated_carriers.insert(carrier);
        Ok(())
    }

    fn remove_lease(&mut self, key: TransferKey) {
        if let Some(lease) = self.leases.remove(&key) {
            if let Some(count) = self.process_counts.get_mut(&lease.send.sender) {
                *count -= 1;
                if *count == 0 {
                    self.process_counts.remove(&lease.send.sender);
                }
            }
        }
    }

    #[cfg(test)]
    fn alias_fd_for_test(&self, key: TransferKey) -> RawFd {
        self.leases[&key].payload_aliases[0].as_raw_fd()
    }
}

fn validate_process(process: ProcessEpoch) -> Result<(), LeaseError> {
    if process.pid == 0 || process.instance == 0 {
        Err(LeaseError::InvalidIdentity)
    } else {
        Ok(())
    }
}

fn validate_carrier(authority: AuthorityEpoch, carrier: CarrierId) -> Result<(), LeaseError> {
    if carrier.authority != authority || carrier.serial == 0 {
        Err(LeaseError::InvalidIdentity)
    } else {
        Ok(())
    }
}

fn validate_send(authority: AuthorityEpoch, send: TrustedSendContext) -> Result<(), LeaseError> {
    validate_process(send.sender)?;
    validate_carrier(authority, send.endpoint.carrier)
}

fn validate_receive(
    authority: AuthorityEpoch,
    receive: TrustedReceiveContext,
) -> Result<(), LeaseError> {
    validate_process(receive.receiver)?;
    validate_carrier(authority, receive.endpoint.carrier)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::fd::{FromRawFd, IntoRawFd};

    const AUTHORITY: AuthorityEpoch = AuthorityEpoch { instance: 0xabc };

    fn sender() -> ProcessEpoch {
        ProcessEpoch {
            pid: 10,
            instance: 1,
        }
    }
    fn receiver() -> ProcessEpoch {
        ProcessEpoch {
            pid: 20,
            instance: 1,
        }
    }
    fn carrier() -> CarrierId {
        CarrierId {
            authority: AUTHORITY,
            serial: 7,
        }
    }
    fn send() -> TrustedSendContext {
        TrustedSendContext {
            sender: sender(),
            endpoint: EndpointId {
                carrier: carrier(),
                side: Side::A,
            },
        }
    }
    fn receive() -> TrustedReceiveContext {
        TrustedReceiveContext {
            receiver: receiver(),
            endpoint: EndpointId {
                carrier: carrier(),
                side: Side::B,
            },
        }
    }
    fn socket_pair() -> (OwnedFd, OwnedFd) {
        let mut fds = [-1; 2];
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) },
            0
        );
        // SAFETY: socketpair initialized both descriptors and ownership moves here.
        (unsafe { OwnedFd::from_raw_fd(fds[0]) }, unsafe {
            OwnedFd::from_raw_fd(fds[1])
        })
    }

    #[test]
    fn admission_retains_alias_until_eof_and_imported_fd_stays_live() {
        let (original, peer) = socket_pair();
        let mut owner = ScmTransferLeaseOwner::new(AUTHORITY, 8, 4);
        let key = owner.mint_key().unwrap();
        let prepared = owner.prepare(send(), key, &[original.as_fd()]).unwrap();
        let alias_fd = owner.alias_fd_for_test(key);
        let receiver_fd = duplicate(original.as_fd()).unwrap();
        owner.arm_enqueued(send(), key).unwrap();
        let imported_guardian = duplicate(prepared.guardian_writer()).unwrap();
        assert_eq!(owner.retire_if_eof(key).unwrap(), Retirement::NotReady);
        assert!(matches!(
            owner.validate_import(receive(), key, 2),
            Err(LeaseError::CountMismatch {
                expected: 1,
                actual: 2
            })
        ));
        assert_eq!(
            owner
                .validate_import(receive(), key, 1)
                .unwrap()
                .payload_count,
            1
        );
        assert_eq!(
            owner
                .validate_import(receive(), key, 1)
                .unwrap()
                .payload_count,
            1
        );
        let admission = owner.admit_import(receive(), key, 1).unwrap();
        assert_eq!(admission.payload_count, 1);
        assert!(matches!(
            owner.admit_import(receive(), key, 1),
            Err(LeaseError::WrongState(LeaseState::Imported))
        ));
        drop(imported_guardian);
        drop(prepared);
        drop(original);
        assert_eq!(owner.retire_if_eof(key).unwrap(), Retirement::Retired);
        assert_eq!(unsafe { libc::fcntl(alias_fd, libc::F_GETFD) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::EBADF));
        let byte = [b'P'];
        assert_eq!(
            unsafe { libc::write(peer.as_raw_fd(), byte.as_ptr().cast(), 1) },
            1
        );
        let mut received = [0u8; 1];
        let mut receiver_file = unsafe { std::fs::File::from_raw_fd(receiver_fd.into_raw_fd()) };
        receiver_file.read_exact(&mut received).unwrap();
        assert_eq!(received, byte);
    }

    #[test]
    fn delayed_admission_and_terminal_sender_still_allow_receive() {
        let (original, _peer) = socket_pair();
        let mut owner = ScmTransferLeaseOwner::new(AUTHORITY, 8, 4);
        let key = owner.mint_key().unwrap();
        let prepared = owner.prepare(send(), key, &[original.as_fd()]).unwrap();
        owner.arm_enqueued(send(), key).unwrap();
        owner.sender_died(sender()).unwrap();
        assert_eq!(owner.state(key).unwrap(), LeaseState::TerminalSender);
        assert!(owner.admit_import(receive(), key, 1).is_ok());
        let next = owner.mint_key().unwrap();
        let (next_payload, _next_peer) = socket_pair();
        assert!(matches!(
            owner.prepare(send(), next, &[next_payload.as_fd()]),
            Err(LeaseError::SenderDead)
        ));
        drop(prepared);
        assert_eq!(owner.retire_if_eof(key).unwrap(), Retirement::Retired);
    }

    #[test]
    fn quotas_are_per_process_epoch_and_keys_cannot_replay() {
        let (payload, _peer) = socket_pair();
        let mut owner = ScmTransferLeaseOwner::new(AUTHORITY, 8, 1);
        let first = owner.mint_key().unwrap();
        let prepared = owner.prepare(send(), first, &[payload.as_fd()]).unwrap();
        let reconnect = TrustedSendContext {
            sender: ProcessEpoch {
                pid: sender().pid,
                instance: 2,
            },
            ..send()
        };
        let second = owner.mint_key().unwrap();
        let second_prepared = owner
            .prepare(reconnect, second, &[payload.as_fd()])
            .unwrap();
        let third = owner.mint_key().unwrap();
        assert!(matches!(
            owner.prepare(send(), third, &[payload.as_fd()]),
            Err(LeaseError::LimitExceeded)
        ));
        owner.cancel_unsent(send(), first).unwrap();
        owner.cancel_unsent(reconnect, second).unwrap();
        assert_eq!(owner.active_count(), 0);
        drop(prepared);
        drop(second_prepared);
        assert!(matches!(
            owner.prepare(send(), first, &[payload.as_fd()]),
            Err(LeaseError::InvalidKey)
        ));
    }

    #[test]
    fn stale_epoch_side_carrier_and_count_are_rejected() {
        let (payload, _peer) = socket_pair();
        let mut owner = ScmTransferLeaseOwner::new(AUTHORITY, 8, 4);
        let key = owner.mint_key().unwrap();
        let prepared = owner.prepare(send(), key, &[payload.as_fd()]).unwrap();
        owner.arm_enqueued(send(), key).unwrap();
        let wrong_side = TrustedReceiveContext {
            endpoint: EndpointId {
                side: Side::A,
                ..receive().endpoint
            },
            ..receive()
        };
        assert!(matches!(
            owner.admit_import(wrong_side, key, 1),
            Err(LeaseError::WrongEndpoint)
        ));
        assert!(matches!(
            owner.admit_import(receive(), key, 2),
            Err(LeaseError::CountMismatch { .. })
        ));
        let stale_authority = TrustedReceiveContext {
            endpoint: EndpointId {
                carrier: CarrierId {
                    authority: AuthorityEpoch { instance: 0xdef },
                    ..carrier()
                },
                ..receive().endpoint
            },
            ..receive()
        };
        assert!(matches!(
            owner.admit_import(stale_authority, key, 1),
            Err(LeaseError::InvalidIdentity)
        ));
        drop(prepared);
        assert_eq!(owner.retire_if_eof(key).unwrap(), Retirement::Retired);
    }

    #[test]
    fn carrier_termination_rejects_admission_but_retains_until_eof() {
        let (payload, _peer) = socket_pair();
        let mut owner = ScmTransferLeaseOwner::new(AUTHORITY, 8, 4);
        let key = owner.mint_key().unwrap();
        let prepared = owner.prepare(send(), key, &[payload.as_fd()]).unwrap();
        owner.arm_enqueued(send(), key).unwrap();
        owner.carrier_terminated(carrier()).unwrap();
        assert_eq!(owner.state(key).unwrap(), LeaseState::TerminalCarrier);
        assert!(matches!(
            owner.admit_import(receive(), key, 1),
            Err(LeaseError::CarrierTerminated)
        ));
        drop(prepared);
        assert_eq!(owner.retire_if_eof(key).unwrap(), Retirement::Retired);
    }
}
