use std::fmt;
use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

pub const MAX_PAYLOAD_FDS: usize = 253;
pub const DEFAULT_MAX_DELIVERIES: usize = 128;
pub const DEFAULT_MAX_RETAINED_FDS: usize = 4096;
pub const DEFAULT_MAX_DESTINATION_DELIVERIES: usize = 32;
pub const DEFAULT_MAX_DESTINATION_FDS: usize = 512;
pub const MAX_RETIRE_SCAN: usize = 128;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DestinationEpoch {
    pub pid: u32,
    pub birth: [u64; 2],
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DeliveryOffer {
    pub daemon_epoch: u128,
    pub ticket: u64,
    pub count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryState {
    Prepared,
    Armed,
    Admitted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Admission {
    pub offer: DeliveryOffer,
    pub destination: DestinationEpoch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeliveryLimits {
    pub max_deliveries: usize,
    pub max_retained_fds: usize,
    pub max_destination_deliveries: usize,
    pub max_destination_fds: usize,
}

impl Default for DeliveryLimits {
    fn default() -> Self {
        Self {
            max_deliveries: DEFAULT_MAX_DELIVERIES,
            max_retained_fds: DEFAULT_MAX_RETAINED_FDS,
            max_destination_deliveries: DEFAULT_MAX_DESTINATION_DELIVERIES,
            max_destination_fds: DEFAULT_MAX_DESTINATION_FDS,
        }
    }
}

#[derive(Debug)]
pub enum DeliveryError {
    InvalidEpoch,
    InvalidDestination,
    InvalidCount,
    Sealed,
    LimitExceeded,
    UnknownOffer,
    WrongState(DeliveryState),
    OfferMismatch,
    DestinationMismatch,
    Io {
        operation: &'static str,
        source: io::Error,
    },
}

impl fmt::Display for DeliveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEpoch => write!(f, "invalid daemon epoch"),
            Self::InvalidDestination => write!(f, "invalid destination epoch"),
            Self::InvalidCount => write!(f, "invalid delivery FD count"),
            Self::Sealed => write!(f, "host FD delivery owner is sealed"),
            Self::LimitExceeded => write!(f, "host FD delivery limit exceeded"),
            Self::UnknownOffer => write!(f, "unknown host FD delivery offer"),
            Self::WrongState(state) => write!(f, "invalid delivery state: {state:?}"),
            Self::OfferMismatch => write!(f, "delivery offer mismatch"),
            Self::DestinationMismatch => write!(f, "delivery destination mismatch"),
            Self::Io { operation, source } => write!(f, "{operation} failed: {source}"),
        }
    }
}

impl std::error::Error for DeliveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct PreparedDelivery {
    pub(crate) offer: DeliveryOffer,
    pub(crate) destination: DestinationEpoch,
    pub(crate) original_bundle: Vec<OwnedFd>,
    pub(crate) guardian_write: OwnedFd,
}

impl PreparedDelivery {
    pub fn offer(&self) -> DeliveryOffer {
        self.offer
    }
    pub fn destination(&self) -> DestinationEpoch {
        self.destination
    }
    pub fn payload_count(&self) -> usize {
        self.original_bundle.len()
    }

    /// Guardian first, followed by the original bundle order.
    pub fn descriptors(&self) -> Result<Vec<BorrowedFd<'_>>, DeliveryError> {
        let mut descriptors = Vec::new();
        descriptors
            .try_reserve(self.original_bundle.len() + 1)
            .map_err(|_| DeliveryError::LimitExceeded)?;
        descriptors.push(self.guardian_write.as_fd());
        descriptors.extend(self.original_bundle.iter().map(AsFd::as_fd));
        Ok(descriptors)
    }

    pub fn guardian_writer(&self) -> BorrowedFd<'_> {
        self.guardian_write.as_fd()
    }
}
