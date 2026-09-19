use darwin_art_binder_device::{
    authority_protocol::{ConnectionToken, Message, TransferToken},
    transfer_image::TransferImage,
};

#[derive(Debug)]
pub struct TransportError(pub String);

pub use crate::descriptor_transport::DescriptorApi;

pub trait AuthorityTransport: Send + Sync + 'static {
    fn connection(&self) -> ConnectionToken;
    fn android_uid(&self) -> u32;
    fn send(&self, message: Message) -> Result<(), TransportError>;
    fn receive(&self) -> Result<Message, TransportError>;
    fn deposit_transfer(
        &self,
        token: TransferToken,
        image: &TransferImage,
    ) -> Result<(), TransportError>;
    fn cancel_unrouted_transfer(&self, _token: TransferToken) -> Result<(), TransportError> {
        Err(TransportError(
            "transport has no exact source cleanup port".into(),
        ))
    }
    fn settle_received_transfer(
        &self,
        _source: ConnectionToken,
        _token: TransferToken,
        _finished: bool,
    ) -> Result<(), TransportError> {
        Err(TransportError(
            "transport has no exact receiver settlement port".into(),
        ))
    }
    fn take_transfer(
        &self,
        source: ConnectionToken,
        token: TransferToken,
    ) -> Result<TransferImage, TransportError>;
}

impl AuthorityTransport for darwin_art_profile::BinderAuthorityConnection {
    fn connection(&self) -> ConnectionToken {
        self.connection()
    }

    fn android_uid(&self) -> u32 {
        self.android_uid()
    }

    fn send(&self, message: Message) -> Result<(), TransportError> {
        self.send(message)
            .map_err(|error| TransportError(error.to_string()))
    }

    fn receive(&self) -> Result<Message, TransportError> {
        self.receive()
            .map_err(|error| TransportError(error.to_string()))
    }

    fn deposit_transfer(
        &self,
        token: TransferToken,
        image: &TransferImage,
    ) -> Result<(), TransportError> {
        self.deposit_transfer(token, image)
            .map_err(|error| TransportError(error.to_string()))
    }

    fn cancel_unrouted_transfer(&self, token: TransferToken) -> Result<(), TransportError> {
        self.cancel_unrouted_transfer(token)
            .map_err(|error| TransportError(error.to_string()))
    }
    fn settle_received_transfer(
        &self,
        source: ConnectionToken,
        token: TransferToken,
        finished: bool,
    ) -> Result<(), TransportError> {
        self.settle_received_transfer(source, token, finished)
            .map_err(|error| TransportError(error.to_string()))
    }

    fn take_transfer(
        &self,
        source: ConnectionToken,
        token: TransferToken,
    ) -> Result<TransferImage, TransportError> {
        self.take_transfer(source, token)
            .map_err(|error| TransportError(error.to_string()))
    }
}
