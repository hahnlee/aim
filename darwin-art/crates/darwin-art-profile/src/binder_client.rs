//! Process-local client for the profile-wide Binder routing authority.
//! Transaction payload ownership belongs to the Binder endpoint, not here.

use crate::{ProfileError, protocol};
use darwin_art_binder_device::{
    authority_protocol::{self, ConnectionToken, Message, TransferToken},
    transfer_image::TransferImage,
};
use std::{
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub struct BinderAuthorityConnection {
    socket: PathBuf,
    connection: ConnectionToken,
    android_uid: u32,
    reader: Mutex<UnixStream>,
    writer: Mutex<UnixStream>,
}

impl BinderAuthorityConnection {
    pub fn connection(&self) -> ConnectionToken {
        self.connection
    }

    pub fn android_uid(&self) -> u32 {
        self.android_uid
    }

    pub fn deposit_transfer(
        &self,
        token: TransferToken,
        image: &TransferImage,
    ) -> Result<(), ProfileError> {
        let mut stream = crate::unix_connect::connect_with_timeout(
            &self.socket,
            std::time::Duration::from_secs(5),
        )?;
        protocol::write_request(
            &mut stream,
            protocol::OP_BINDER_TRANSFER_DEPOSIT,
            &token.get().to_le_bytes(),
        )?;
        let mut descriptors = Vec::with_capacity(image.files().len() + 1);
        descriptors.push(image.descriptor());
        descriptors.extend(image.files().iter().map(|file| file.descriptor()));
        crate::fd_passing::send_many(&stream, &descriptors)?;
        let payload = protocol::expect_ok(&mut stream, protocol::OP_BINDER_TRANSFER_DEPOSIT)?;
        if !payload.is_empty() {
            return Err(ProfileError::Daemon(
                "Binder transfer deposit returned trailing payload".into(),
            ));
        }
        Ok(())
    }

    pub fn take_transfer(
        &self,
        source: ConnectionToken,
        token: TransferToken,
    ) -> Result<TransferImage, ProfileError> {
        let mut stream = crate::unix_connect::connect_with_timeout(
            &self.socket,
            std::time::Duration::from_secs(5),
        )?;
        let mut key = [0_u8; 16];
        key[..8].copy_from_slice(&source.get().to_le_bytes());
        key[8..].copy_from_slice(&token.get().to_le_bytes());
        protocol::write_request(&mut stream, protocol::OP_BINDER_TRANSFER_TAKE, &key)?;
        let result = (|| {
            let payload = protocol::expect_ok(&mut stream, protocol::OP_BINDER_TRANSFER_TAKE)?;
            crate::host_fd_delivery::transport::receive(&mut stream, &payload)
        })();
        if result.is_err() {
            if let Err(error) = self.settle_received_transfer(source, token, false) {
                eprintln!("Binder failed TAKE capability cleanup: {error}");
            }
        }
        result
    }

    pub fn cancel_unrouted_transfer(&self, token: TransferToken) -> Result<(), ProfileError> {
        self.transfer_control(
            protocol::OP_BINDER_TRANSFER_CANCEL,
            &token.get().to_le_bytes(),
        )
    }
    pub fn settle_received_transfer(
        &self,
        source: ConnectionToken,
        token: TransferToken,
        finished: bool,
    ) -> Result<(), ProfileError> {
        let mut body = [0; 17];
        body[..8].copy_from_slice(&source.get().to_le_bytes());
        body[8..16].copy_from_slice(&token.get().to_le_bytes());
        body[16] = if finished { 1 } else { 2 };
        self.transfer_control(protocol::OP_BINDER_TRANSFER_SETTLE, &body)
    }
    fn transfer_control(&self, operation: u16, body: &[u8]) -> Result<(), ProfileError> {
        let mut stream = crate::unix_connect::connect_with_timeout(
            &self.socket,
            std::time::Duration::from_secs(5),
        )?;
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
        protocol::write_request(&mut stream, operation, body)?;
        if !protocol::expect_ok(&mut stream, operation)?.is_empty() {
            return Err(ProfileError::Daemon(
                "Binder transfer control returned trailing payload".into(),
            ));
        }
        Ok(())
    }

    /// Calls from multiple binder_thread writers are serialized into complete
    /// frames. This lock never covers the blocking receive path.
    pub fn send(&self, message: Message) -> Result<(), ProfileError> {
        if !matches!(
            message,
            Message::PublishNode { .. }
                | Message::SetContextManager { .. }
                | Message::GetContextManager
                | Message::RouteTransaction { .. }
                | Message::CompleteReply { .. }
                | Message::RequestDeath { .. }
                | Message::ClearDeath { .. }
                | Message::CloseConnection
        ) {
            return Err(ProfileError::Daemon(
                "attempted to send a server-only Binder message".into(),
            ));
        }
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| ProfileError::Daemon("Binder writer is poisoned".into()))?;
        authority_protocol::encode(&mut *writer, message)?;
        Ok(())
    }

    /// Exactly one endpoint dispatcher should receive authority events and
    /// publish them into local binder_proc/thread queues.
    pub fn receive(&self) -> Result<Message, ProfileError> {
        let mut reader = self
            .reader
            .lock()
            .map_err(|_| ProfileError::Daemon("Binder reader is poisoned".into()))?;
        let message = authority_protocol::decode(&mut *reader)
            .map_err(|error| ProfileError::Daemon(format!("Binder receive failed: {error:?}")))?;
        if matches!(
            message,
            Message::OpenConnection
                | Message::PublishNode { .. }
                | Message::SetContextManager { .. }
                | Message::GetContextManager
                | Message::RouteTransaction { .. }
                | Message::CompleteReply { .. }
                | Message::RequestDeath { .. }
                | Message::ClearDeath { .. }
                | Message::CloseConnection
        ) {
            return Err(ProfileError::Daemon(
                "received a client-only Binder message".into(),
            ));
        }
        Ok(message)
    }
}

pub fn connect_binder_authority_at(
    socket: &Path,
) -> Result<BinderAuthorityConnection, ProfileError> {
    if !socket.is_absolute() {
        return Err(ProfileError::Daemon(
            "Binder authority socket must be absolute".into(),
        ));
    }
    let mut stream =
        crate::unix_connect::connect_with_timeout(socket, std::time::Duration::from_secs(5))?;
    protocol::write_request(&mut stream, protocol::OP_BINDER_SESSION, b"")?;
    let response = protocol::expect_ok(&mut stream, protocol::OP_BINDER_SESSION)?;
    if !response.is_empty() {
        return Err(ProfileError::Daemon(
            "Binder authority upgrade returned trailing payload".into(),
        ));
    }
    let Message::ConnectionOpened {
        connection,
        android_uid,
    } = authority_protocol::decode(&mut stream)
        .map_err(|error| ProfileError::Daemon(format!("Binder open failed: {error:?}")))?
    else {
        return Err(ProfileError::Daemon(
            "Binder authority did not open a connection".into(),
        ));
    };
    let writer = stream.try_clone()?;
    Ok(BinderAuthorityConnection {
        socket: socket.to_owned(),
        connection,
        android_uid,
        reader: Mutex::new(stream),
        writer: Mutex::new(writer),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use darwin_art_binder_device::authority_protocol::LocalNodeToken;
    use std::{fs, os::unix::net::UnixListener, sync::Arc, thread};

    #[test]
    fn upgrade_and_split_reader_writer_preserve_message_direction() {
        let path = std::env::temp_dir().join(format!(
            "dabc-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let request = protocol::read_message(&mut stream).unwrap();
            assert_eq!(request.operation, protocol::OP_BINDER_SESSION);
            assert!(request.payload.is_empty());
            protocol::write_response(&mut stream, request.operation, 0, b"").unwrap();
            let connection = ConnectionToken::from_nonzero(7).unwrap();
            authority_protocol::encode(
                &mut stream,
                Message::ConnectionOpened {
                    connection,
                    android_uid: 10_123,
                },
            )
            .unwrap();
            assert_eq!(
                authority_protocol::decode(&mut stream).unwrap(),
                Message::PublishNode {
                    local: LocalNodeToken::from_nonzero(3).unwrap(),
                }
            );
            authority_protocol::encode(
                &mut stream,
                Message::NodePublished {
                    node: authority_protocol::NodeToken::new(
                        connection,
                        LocalNodeToken::from_nonzero(3).unwrap(),
                    ),
                },
            )
            .unwrap();
        });
        let client = Arc::new(connect_binder_authority_at(&path).unwrap());
        assert_eq!(client.connection().get(), 7);
        assert_eq!(client.android_uid(), 10_123);
        let writer = Arc::clone(&client);
        thread::spawn(move || {
            writer
                .send(Message::PublishNode {
                    local: LocalNodeToken::from_nonzero(3).unwrap(),
                })
                .unwrap();
        })
        .join()
        .unwrap();
        assert!(matches!(
            client.receive().unwrap(),
            Message::NodePublished { .. }
        ));
        server.join().unwrap();
        drop(client);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn relative_socket_and_wrong_message_direction_fail_before_transport_use() {
        assert!(connect_binder_authority_at(Path::new("relative.sock")).is_err());
        let (first, _second) = UnixStream::pair().unwrap();
        let connection = BinderAuthorityConnection {
            socket: PathBuf::from("/unused"),
            connection: ConnectionToken::from_nonzero(1).unwrap(),
            android_uid: 10_001,
            reader: Mutex::new(first.try_clone().unwrap()),
            writer: Mutex::new(first),
        };
        assert!(
            connection
                .send(Message::ConnectionOpened {
                    connection: ConnectionToken::from_nonzero(1).unwrap(),
                    android_uid: 10_001,
                })
                .is_err()
        );
    }
}
