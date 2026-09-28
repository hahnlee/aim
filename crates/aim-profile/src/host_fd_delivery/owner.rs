use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd};

use super::guardian;
use super::types::*;

#[derive(Debug)]
struct Lease {
    offer: DeliveryOffer,
    destination: DestinationEpoch,
    aliases: Vec<std::os::fd::OwnedFd>,
    guardian_read: std::os::fd::OwnedFd,
    state: DeliveryState,
}

#[derive(Debug)]
struct Usage {
    deliveries: usize,
    fds: usize,
}

#[derive(Debug)]
pub struct HostFdDeliveryOwner {
    daemon_epoch: u128,
    leases: HashMap<DeliveryOffer, Lease>,
    destination_usage: HashMap<DestinationEpoch, Usage>,
    limits: DeliveryLimits,
    retained_fds: usize,
    next_ticket: u64,
    retire_cursor: usize,
    sealed: bool,
}

impl HostFdDeliveryOwner {
    pub fn new(daemon_epoch: u128, limits: DeliveryLimits) -> Result<Self, DeliveryError> {
        if daemon_epoch == 0 {
            return Err(DeliveryError::InvalidEpoch);
        }
        Ok(Self {
            daemon_epoch,
            leases: HashMap::new(),
            destination_usage: HashMap::new(),
            limits: clamp_limits(limits),
            retained_fds: 0,
            next_ticket: 0,
            retire_cursor: 0,
            sealed: false,
        })
    }

    pub fn daemon_epoch(&self) -> u128 {
        self.daemon_epoch
    }
    pub fn active_count(&self) -> usize {
        self.leases.len()
    }

    pub fn state(&self, offer: DeliveryOffer) -> Result<DeliveryState, DeliveryError> {
        self.leases
            .get(&offer)
            .map(|lease| lease.state)
            .ok_or(DeliveryError::UnknownOffer)
    }

    pub fn reserve(
        &mut self,
        destination: DestinationEpoch,
        original_bundle: Vec<std::os::fd::OwnedFd>,
    ) -> Result<PreparedDelivery, DeliveryError> {
        self.check_open()?;
        validate_destination(destination)?;
        let count = original_bundle.len();
        if !(1..=MAX_PAYLOAD_FDS).contains(&count) {
            return Err(DeliveryError::InvalidCount);
        }
        let retained = count.checked_add(1).ok_or(DeliveryError::InvalidCount)?;
        if self.leases.len() >= self.limits.max_deliveries
            || self.retained_fds.checked_add(retained).is_none()
            || self.retained_fds + retained > self.limits.max_retained_fds
        {
            return Err(DeliveryError::LimitExceeded);
        }
        let usage = self.destination_usage.get(&destination);
        if usage.map_or(0, |item| item.deliveries) >= self.limits.max_destination_deliveries
            || usage
                .map_or(0, |item| item.fds)
                .checked_add(retained)
                .unwrap_or(usize::MAX)
                > self.limits.max_destination_fds
        {
            return Err(DeliveryError::LimitExceeded);
        }

        self.leases
            .try_reserve(1)
            .map_err(|_| DeliveryError::LimitExceeded)?;
        if !self.destination_usage.contains_key(&destination) {
            self.destination_usage
                .try_reserve(1)
                .map_err(|_| DeliveryError::LimitExceeded)?;
        }
        let mut aliases = Vec::new();
        aliases
            .try_reserve(count)
            .map_err(|_| DeliveryError::LimitExceeded)?;
        let ticket = self.next_ticket.checked_add(1).ok_or_else(|| {
            self.sealed = true;
            DeliveryError::Sealed
        })?;
        self.next_ticket = ticket;
        let offer = DeliveryOffer {
            daemon_epoch: self.daemon_epoch,
            ticket,
            count: count as u32,
        };
        let (guardian_read, guardian_write) = guardian::make_guardian()?;
        for original in &original_bundle {
            aliases.push(guardian::duplicate(original.as_fd()).map_err(|source| {
                DeliveryError::Io {
                    operation: "duplicate retained payload",
                    source,
                }
            })?);
        }
        self.leases.insert(
            offer,
            Lease {
                offer,
                destination,
                aliases,
                guardian_read,
                state: DeliveryState::Prepared,
            },
        );
        self.retained_fds += retained;
        self.destination_usage
            .entry(destination)
            .and_modify(|item| {
                item.deliveries += 1;
                item.fds += retained;
            })
            .or_insert(Usage {
                deliveries: 1,
                fds: retained,
            });
        Ok(PreparedDelivery {
            offer,
            destination,
            original_bundle,
            guardian_write,
        })
    }

    pub fn arm(
        &mut self,
        destination: DestinationEpoch,
        offer: DeliveryOffer,
    ) -> Result<(), DeliveryError> {
        self.check_open()?;
        validate_destination(destination)?;
        let lease = self
            .leases
            .get_mut(&offer)
            .ok_or(DeliveryError::UnknownOffer)?;
        if lease.destination != destination {
            return Err(DeliveryError::DestinationMismatch);
        }
        if lease.offer != offer {
            return Err(DeliveryError::OfferMismatch);
        }
        if lease.state != DeliveryState::Prepared {
            return Err(DeliveryError::WrongState(lease.state));
        }
        lease.state = DeliveryState::Armed;
        Ok(())
    }

    pub fn admit_acquired(
        &mut self,
        destination: DestinationEpoch,
        offer: DeliveryOffer,
    ) -> Result<Admission, DeliveryError> {
        self.check_open()?;
        validate_destination(destination)?;
        let lease = self
            .leases
            .get_mut(&offer)
            .ok_or(DeliveryError::UnknownOffer)?;
        if lease.destination != destination {
            return Err(DeliveryError::DestinationMismatch);
        }
        if lease.offer != offer {
            return Err(DeliveryError::OfferMismatch);
        }
        if lease.state != DeliveryState::Armed {
            return Err(DeliveryError::WrongState(lease.state));
        }
        lease.state = DeliveryState::Admitted;
        Ok(Admission { offer, destination })
    }

    /// Scan at most 128 entries per call with timeout-zero native polling.
    /// A rotating cursor prevents an eligible entry from being starved when
    /// callers pass a smaller advisory budget; `_budget` is retained for API
    /// compatibility and is intentionally not a release condition.
    pub fn retire_ready(&mut self, _budget: usize) -> Result<Vec<DeliveryOffer>, DeliveryError> {
        let scan_capacity = self.leases.len().min(MAX_RETIRE_SCAN);
        let mut offers = Vec::new();
        offers
            .try_reserve(scan_capacity)
            .map_err(|_| DeliveryError::LimitExceeded)?;
        offers.extend(self.leases.keys().copied().take(scan_capacity));
        if offers.is_empty() {
            return Ok(Vec::new());
        }
        let start = self.retire_cursor % offers.len();
        let scan = offers.len().min(MAX_RETIRE_SCAN);
        self.retire_cursor = (start + scan) % offers.len();
        let mut retired = Vec::new();
        retired
            .try_reserve(scan)
            .map_err(|_| DeliveryError::LimitExceeded)?;
        for offset in 0..scan {
            let offer = offers[(start + offset) % offers.len()];
            let eof = {
                let lease = self.leases.get(&offer).ok_or(DeliveryError::UnknownOffer)?;
                guardian::is_eof(lease.guardian_read.as_raw_fd()).map_err(|source| {
                    DeliveryError::Io {
                        operation: "poll delivery guardian",
                        source,
                    }
                })?
            };
            if eof {
                self.remove(offer);
                retired.push(offer);
            }
        }
        Ok(retired)
    }

    fn check_open(&self) -> Result<(), DeliveryError> {
        if self.sealed {
            Err(DeliveryError::Sealed)
        } else {
            Ok(())
        }
    }

    fn remove(&mut self, offer: DeliveryOffer) {
        if let Some(lease) = self.leases.remove(&offer) {
            let retained = lease.aliases.len() + 1;
            self.retained_fds -= retained;
            if let Some(usage) = self.destination_usage.get_mut(&lease.destination) {
                usage.deliveries -= 1;
                usage.fds -= retained;
                if usage.deliveries == 0 {
                    self.destination_usage.remove(&lease.destination);
                }
            }
        }
    }
}

fn clamp_limits(mut limits: DeliveryLimits) -> DeliveryLimits {
    limits.max_deliveries = limits.max_deliveries.min(DEFAULT_MAX_DELIVERIES);
    limits.max_retained_fds = limits.max_retained_fds.min(DEFAULT_MAX_RETAINED_FDS);
    limits.max_destination_deliveries = limits
        .max_destination_deliveries
        .min(DEFAULT_MAX_DESTINATION_DELIVERIES);
    limits.max_destination_fds = limits.max_destination_fds.min(DEFAULT_MAX_DESTINATION_FDS);
    limits
}

fn validate_destination(destination: DestinationEpoch) -> Result<(), DeliveryError> {
    if !(1..=i32::MAX as u32).contains(&destination.pid)
        || destination.birth[0] == 0
        || destination.birth[1] >= 1_000_000
    {
        Err(DeliveryError::InvalidDestination)
    } else {
        Ok(())
    }
}
