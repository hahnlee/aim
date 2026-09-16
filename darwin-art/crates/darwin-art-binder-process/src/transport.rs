use darwin_art_binder_device::{
    authority_protocol::{ConnectionToken, Message, TransferToken},
    transfer_image::TransferImage,
};

#[derive(Debug)]
pub struct TransportError(pub String);

#[derive(Clone, Copy)]
pub struct DescriptorApi {
    pub export: unsafe extern "C" fn(i32) -> i32,
    pub import: unsafe extern "C" fn(i32) -> i32,
    pub close: unsafe extern "C" fn(i32) -> i32,
}

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

    fn take_transfer(
        &self,
        source: ConnectionToken,
        token: TransferToken,
    ) -> Result<TransferImage, TransportError> {
        self.take_transfer(source, token)
            .map_err(|error| TransportError(error.to_string()))
    }
}
