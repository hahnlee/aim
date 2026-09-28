//! A non-cloneable owner for the transport's device-open lifetime.
use super::*;

pub struct ConnectionOwner {
    connection: Arc<Connection>,
}
impl ConnectionOwner {
    pub(crate) fn handle(&self) -> Arc<Connection> {
        Arc::clone(&self.connection)
    }
    /// The transport must create this after authenticating the device open and
    /// drop it on disconnect/error. Construction does not authenticate a peer.
    pub fn new(session: Session) -> Self {
        Self {
            connection: Connection::new(session),
        }
    }

    pub fn bind_target(&self, references: TargetReferences) -> Result<TargetConnection, Error> {
        self.connection.bind_target(references)
    }
}
impl std::ops::Deref for ConnectionOwner {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        &self.connection
    }
}
impl Drop for ConnectionOwner {
    fn drop(&mut self) {
        // Outstanding target leases keep only closed metadata, never keep the
        // transport/session implicitly open after its unique owner disappears.
        self.connection.disconnect();
    }
}
