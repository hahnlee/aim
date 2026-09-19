use darwin_art_binder_device::{
    authority_protocol::{ConnectionToken, TransferToken},
    transfer_image::TransferImage,
};
use std::{collections::HashMap, os::fd::OwnedFd};

const MAX_TRANSFERS_PER_SOURCE: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Key {
    pub(super) source: ConnectionToken,
    pub(super) token: TransferToken,
}

struct Entry {
    destination: Option<ConnectionToken>,
    descriptors: Vec<OwnedFd>,
}

/// Fully validated native group and reserved namespace storage. No transfer
/// is visible until its exact capability batch commits and Install consumes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CancelUnrouted {
    Absent,
    Discarded,
    Routed,
}

pub(super) struct PreparedDeposit {
    key: Key,
    descriptors: Vec<OwnedFd>,
    pub(super) managed: Vec<darwin_art_scm_transfer::capabilities::BinderManifestItem>,
}

#[derive(Default)]
pub(crate) struct TransferTable {
    entries: HashMap<Key, Entry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    Duplicate,
    SourceLimit,
    Unknown,
    NotRouted,
    WrongDestination,
    InvalidCarrier,
    OutOfMemory,
}

impl TransferTable {
    #[cfg(test)]
    pub(crate) fn deposit(
        &mut self,
        source: ConnectionToken,
        token: TransferToken,
        descriptors: Vec<OwnedFd>,
    ) -> Result<(), Error> {
        let prepared = self.prepare_deposit(source, token, descriptors)?;
        self.install_deposit(prepared);
        Ok(())
    }

    pub(super) fn prepare_deposit(
        &mut self,
        source: ConnectionToken,
        token: TransferToken,
        mut descriptors: Vec<OwnedFd>,
    ) -> Result<PreparedDeposit, Error> {
        if descriptors.is_empty() {
            return Err(Error::InvalidCarrier);
        }
        let carrier = descriptors.remove(0);
        let image = TransferImage::import_with_fds(carrier, descriptors)
            .map_err(|_| Error::InvalidCarrier)?;
        let mut managed = Vec::new();
        managed
            .try_reserve_exact(image.files().len())
            .map_err(|_| Error::OutOfMemory)?;
        for file in image.files() {
            if file.metadata().is_empty() {
                continue;
            }
            let attributes = file
                .metadata()
                .try_into()
                .map_err(|_| Error::InvalidCarrier)?;
            managed.push(darwin_art_scm_transfer::capabilities::BinderManifestItem {
                ordinal: file.ordinal(),
                object_offset: file.offset() as u64,
                attributes,
            });
        }
        let descriptors = image
            .into_transport_descriptors()
            .map_err(|_| Error::OutOfMemory)?;
        let key = Key { source, token };
        if self.entries.contains_key(&key) {
            return Err(Error::Duplicate);
        }
        if self
            .entries
            .keys()
            .filter(|key| key.source == source)
            .count()
            >= MAX_TRANSFERS_PER_SOURCE
        {
            return Err(Error::SourceLimit);
        }
        self.entries
            .try_reserve(1)
            .map_err(|_| Error::OutOfMemory)?;
        Ok(PreparedDeposit {
            key,
            descriptors,
            managed,
        })
    }

    // The same transfer lock stays held from Prepare through capability commit.
    // Preallocated insertion cannot fail or disclose a prefix of the FD group.
    pub(super) fn install_deposit(&mut self, prepared: PreparedDeposit) {
        let old = self.entries.insert(
            prepared.key,
            Entry {
                destination: None,
                descriptors: prepared.descriptors,
            },
        );
        assert!(
            old.is_none(),
            "reserved Binder deposit replaced a live token"
        );
    }

    pub(super) fn contains_transfer(&self, source: ConnectionToken, token: TransferToken) -> bool {
        self.entries.contains_key(&Key { source, token })
    }

    pub(crate) fn ensure_pending(
        &self,
        source: ConnectionToken,
        token: TransferToken,
    ) -> Result<(), Error> {
        match self.entries.get(&Key { source, token }) {
            Some(Entry {
                destination: None, ..
            }) => Ok(()),
            Some(_) => Err(Error::NotRouted),
            None => Err(Error::Unknown),
        }
    }

    pub(crate) fn route(
        &mut self,
        source: ConnectionToken,
        token: TransferToken,
        destination: ConnectionToken,
    ) -> Result<(), Error> {
        let entry = self
            .entries
            .get_mut(&Key { source, token })
            .ok_or(Error::Unknown)?;
        if entry.destination.is_some() {
            return Err(Error::NotRouted);
        }
        entry.destination = Some(destination);
        Ok(())
    }

    pub(crate) fn discard_pending(
        &mut self,
        source: ConnectionToken,
        token: TransferToken,
    ) -> Result<(), Error> {
        self.ensure_pending(source, token)?;
        self.entries.remove(&Key { source, token });
        Ok(())
    }

    pub(super) fn verify_take(
        &self,
        destination: ConnectionToken,
        source: ConnectionToken,
        token: TransferToken,
    ) -> Result<(), Error> {
        match self.entries.get(&Key { source, token }) {
            Some(Entry {
                destination: None, ..
            }) => Err(Error::NotRouted),
            Some(Entry {
                destination: Some(expected),
                ..
            }) if *expected != destination => Err(Error::WrongDestination),
            Some(_) => Ok(()),
            None => Err(Error::Unknown),
        }
    }

    pub(super) fn cancel_unrouted(
        &mut self,
        source: ConnectionToken,
        token: TransferToken,
    ) -> Result<CancelUnrouted, Error> {
        match self.entries.get(&Key { source, token }) {
            Some(Entry {
                destination: None, ..
            }) => {
                self.entries.remove(&Key { source, token });
                Ok(CancelUnrouted::Discarded)
            }
            Some(_) => Ok(CancelUnrouted::Routed), // A lost route ACK cannot revoke a routed delivery.
            None => Ok(CancelUnrouted::Absent), // Capture failed before deposit; pending Bind can cancel.
        }
    }

    pub(crate) fn take(
        &mut self,
        destination: ConnectionToken,
        source: ConnectionToken,
        token: TransferToken,
    ) -> Result<Vec<OwnedFd>, Error> {
        let key = Key { source, token };
        match self.entries.get(&key) {
            Some(Entry {
                destination: None, ..
            }) => return Err(Error::NotRouted),
            Some(Entry {
                destination: Some(expected),
                ..
            }) if *expected != destination => return Err(Error::WrongDestination),
            Some(_) => {}
            None => return Err(Error::Unknown),
        }
        Ok(self.entries.remove(&key).unwrap().descriptors)
    }

    /// Remove one table-proven unreachable transfer. The service repeats this
    /// and cleans its capability outside the table lock without allocating a
    /// teardown list or losing keys on an OOM failure.
    pub(super) fn remove_next_connection_transfer(
        &mut self,
        connection: ConnectionToken,
    ) -> Option<Key> {
        let key = self.entries.iter().find_map(|(key, entry)| {
            (entry.destination == Some(connection)
                || (key.source == connection && entry.destination.is_none()))
            .then_some(*key)
        })?;
        self.entries.remove(&key);
        Some(key)
    }

    #[cfg(test)]
    pub(crate) fn remove_connection(&mut self, connection: ConnectionToken) {
        while self.remove_next_connection_transfer(connection).is_some() {}
    }
}

#[cfg(test)]
mod tests;
