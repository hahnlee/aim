//! Exact transfer cleanup around capture/routing and native import/publication.
//! These receipts are Binder policy; native alias retirement has another owner.
use crate::{AuthorityTransport, Error};
use darwin_art_binder_device::authority_protocol::{ConnectionToken, TransferToken};

pub(crate) struct Source<'a, T: AuthorityTransport> {
    transport: &'a T,
    transfer: TransferToken,
    armed: bool,
}
impl<'a, T: AuthorityTransport> Source<'a, T> {
    pub fn new(transport: &'a T, transfer: TransferToken) -> Self {
        Self {
            transport,
            transfer,
            armed: true,
        }
    }
    pub fn routed(&mut self) {
        self.armed = false;
    }
}
impl<T: AuthorityTransport> Drop for Source<'_, T> {
    fn drop(&mut self) {
        if self.armed {
            if let Err(error) = self.transport.cancel_unrouted_transfer(self.transfer) {
                eprintln!("Binder source transfer cleanup failed: {}", error.0);
            }
        }
    }
}
pub(crate) struct Receiver<'a, T: AuthorityTransport> {
    transport: &'a T,
    source: ConnectionToken,
    transfer: TransferToken,
    armed: bool,
}
impl<'a, T: AuthorityTransport> Receiver<'a, T> {
    pub fn new(transport: &'a T, source: ConnectionToken, transfer: TransferToken) -> Self {
        Self {
            transport,
            source,
            transfer,
            armed: true,
        }
    }
    pub fn finish(&mut self) -> Result<(), Error> {
        self.transport
            .settle_received_transfer(self.source, self.transfer, true)
            .map_err(|error| Error::Transport(error.0))?;
        self.armed = false;
        Ok(())
    }
}
impl<T: AuthorityTransport> Drop for Receiver<'_, T> {
    fn drop(&mut self) {
        if self.armed {
            if let Err(error) =
                self.transport
                    .settle_received_transfer(self.source, self.transfer, false)
            {
                eprintln!("Binder receiver transfer discard failed: {}", error.0);
            }
        }
    }
}
