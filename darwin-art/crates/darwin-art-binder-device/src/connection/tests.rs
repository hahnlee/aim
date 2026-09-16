use super::*;
use crate::{command::Kind, objects, reference_table::Strength, transaction_request};

fn pair() -> (ConnectionOwner, TargetReferences) {
    let mut owner = Session::default();
    let mut bytes = [0; 24];
    bytes[..4].copy_from_slice(&objects::Kind::Binder.tag().to_le_bytes());
    bytes[8..16].copy_from_slice(&123u64.to_le_bytes());
    let objects = objects::validate(&bytes, &0u64.to_le_bytes()).unwrap();
    let node = owner.resolve_local(&objects[0]).unwrap();
    let mut sender = Session::default();
    sender.retain_transferred(node, Strength::Strong).unwrap();
    let mut request = Kind::Transaction.word().to_le_bytes().to_vec();
    request.resize(68, 0);
    request[4..8].copy_from_slice(&1u32.to_le_bytes());
    let request = transaction_request::decode(&request).unwrap().0;
    let references = sender.resolve_transaction_target(&request).unwrap();
    (ConnectionOwner::new(owner), references)
}

#[test]
fn wrong_owner_and_disconnected_target_never_publish() {
    let (connection, refs) = pair();
    let wrong = ConnectionOwner::new(Session::default());
    assert!(matches!(wrong.bind_target(refs), Err(Error::WrongOwner)));
    let (other, refs) = pair();
    let target = other.bind_target(refs).unwrap();
    assert_eq!(target.commit(|n| Ok(n.pointer())).unwrap().unwrap(), 123);
    other.disconnect();
    other.disconnect();
    assert!(matches!(
        target.commit(|_| -> io::Result<()> { panic!("closed target published") }),
        Err(Error::Closed)
    ));
    assert_eq!(
        other.execute(&Kind::EnterLooper.word().to_le_bytes()),
        Err(Error::Closed)
    );
    assert!(!target.references.node().owner_alive());
    drop(connection);
}

#[test]
fn disconnect_cannot_cross_an_active_publication_gate() {
    let (connection, refs) = pair();
    let target = connection.bind_target(refs).unwrap();
    std::thread::scope(|scope| {
        let (entered, receiving) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        let (attempting, attempted) = std::sync::mpsc::channel();
        let target_ref = &target;
        let publishing = scope.spawn(move || {
            target_ref.commit(|_| {
                entered.send(()).unwrap();
                // Test-only synchronization forces overlap; production commits must
                // be nonblocking and may not wait while holding this gate.
                released
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
                Ok(())
            })
        });
        receiving
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert!(matches!(
            connection.session.try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        ));
        let connection_ref = &connection;
        let closing = scope.spawn(move || {
            attempting.send(()).unwrap();
            connection_ref.disconnect();
        });
        attempted
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        release.send(()).unwrap();
        publishing.join().unwrap().unwrap().unwrap();
        closing.join().unwrap();
    });
    assert!(matches!(target.commit(|_| Ok(())), Err(Error::Closed)));
}

#[test]
fn owner_drop_and_error_unwind_close_retained_targets() {
    let (owner, refs) = pair();
    let target = owner.bind_target(refs).unwrap();
    let fail = || -> Result<(), ()> {
        let _owner = owner;
        Err(())
    };
    assert_eq!(fail(), Err(()));
    assert!(!target.references.node().owner_alive());
    assert!(matches!(target.commit(|_| Ok(())), Err(Error::Closed)));
}

#[test]
fn panicking_publication_fails_closed_and_owner_drop_still_cleans_up() {
    let (owner, refs) = pair();
    let target = owner.bind_target(refs).unwrap();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = target.commit(|_| -> io::Result<()> { panic!("publication fault") });
    }));
    assert!(panic.is_err());
    assert!(matches!(target.commit(|_| Ok(())), Err(Error::Poisoned)));
    assert_eq!(
        owner.execute(&Kind::EnterLooper.word().to_le_bytes()),
        Err(Error::Poisoned)
    );
    drop(owner);
    assert!(!target.references.node().owner_alive());
}
