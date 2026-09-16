use darwin_art_binder_device::{
    authority_protocol::{ConnectionToken, TransferToken},
    transfer_image::TransferImage,
};
use std::{
    collections::HashMap,
    os::fd::{AsFd, OwnedFd},
};

const MAX_TRANSFERS_PER_SOURCE: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    source: ConnectionToken,
    token: TransferToken,
}

struct Entry {
    destination: Option<ConnectionToken>,
    descriptors: Vec<OwnedFd>,
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
    pub(crate) fn deposit(
        &mut self,
        source: ConnectionToken,
        token: TransferToken,
        descriptors: Vec<OwnedFd>,
    ) -> Result<(), Error> {
        let Some(carrier) = descriptors.first() else {
            return Err(Error::InvalidCarrier);
        };
        TransferImage::validate_carrier(carrier.as_fd()).map_err(|_| Error::InvalidCarrier)?;
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
        self.entries.insert(
            key,
            Entry {
                destination: None,
                descriptors,
            },
        );
        Ok(())
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

    pub(crate) fn remove_connection(&mut self, connection: ConnectionToken) {
        self.entries.retain(|key, entry| {
            // Once routed, the authority owns the immutable carrier until the
            // destination takes it. Source teardown must not revoke a reply or
            // transaction which was already published to another process.
            // Pending source deposits and transfers whose destination died
            // still have no possible consumer and are released immediately.
            entry.destination != Some(connection)
                && !(key.source == connection && entry.destination.is_none())
        });
    }
}

#[cfg(test)]
mod tests;
