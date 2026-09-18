use super::*;
use darwin_art_scm_transfer::LeaseState;
use std::{
    fs::File,
    io::{Read, Write},
    os::{fd::AsRawFd, unix::net::UnixStream},
};

fn fixture() -> (Owner, ProcessEpoch, RegisteredPair, RegisteredPair) {
    let mut owner = Owner::new(AuthorityEpoch { instance: 9081 }).unwrap();
    let peer = ProcessEpoch {
        pid: 8,
        instance: 9001,
    };
    let carrier = owner.register_pair(peer).unwrap();
    let payload = owner.register_pair(peer).unwrap();
    (owner, peer, carrier, payload)
}

fn assert_native_eof_retirement(owner: &mut Owner) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while owner.has_pending().unwrap() {
        // NotReady (including EINTR) is legitimate for a nonblocking scan.
        // The deadline only fails this fixture; only observed native EOF
        // permits the production owner to retire its aliases.
        assert!(
            std::time::Instant::now() < deadline,
            "guardian EOF not observed"
        );
        std::thread::yield_now();
    }
}

#[test]
fn actual_queued_group_keeps_alias_until_settlement_and_native_eof() {
    let (mut owner, peer, carrier, grant) = fixture();
    let (sender, receiver) = UnixStream::pair().unwrap();
    let (payload, mut payload_peer) = UnixStream::pair().unwrap();
    let payloads = vec![OwnedFd::from(payload)];
    let prepared = owner
        .prepare(
            peer,
            carrier.holder_a.id(),
            &payloads,
            &[(0, grant.holder_a.id())],
        )
        .unwrap();
    let key = prepared.transfer.key();
    let manifest = owner.ledger[&key]
        .managed
        .iter()
        .map(|(ordinal, id)| (*ordinal, id.get()))
        .collect::<Vec<_>>();
    crate::fd_passing::send_many(
        &sender,
        &[
            prepared.metadata.as_fd(),
            prepared.transfer.guardian_writer(),
            payloads[0].as_fd(),
        ],
    )
    .unwrap();
    drop(prepared);
    drop(payloads);
    owner.release_holder(peer, grant.holder_a.id()).unwrap();
    assert!(owner.has_pending().unwrap());

    let mut received = crate::fd_passing::receive_many(&receiver).unwrap();
    assert_eq!(received.len(), 3);
    let mut metadata = File::from(received.remove(0));
    let guardian = received.remove(0);
    let mut bytes = Vec::new();
    metadata.read_to_end(&mut bytes).unwrap();
    assert!(!bytes.is_empty());
    assert!(
        metadata.write_all(b"invalid").is_err(),
        "metadata creation description must not escape writable"
    );
    let claims = owner
        .admit(peer, carrier.holder_b.id(), key, 1, &manifest, &[0])
        .unwrap();
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].1.endpoint, grant.endpoint_a);
    assert!(owner.has_pending().unwrap(), "admission is not retirement");
    let foreign = ProcessEpoch {
        pid: 9,
        instance: 9002,
    };
    assert!(
        owner
            .settle(foreign, key, DeliveryDisposition::Finished)
            .is_err()
    );
    owner
        .settle(peer, key, DeliveryDisposition::Finished)
        .unwrap();
    assert!(owner.has_pending().unwrap(), "settlement is not retirement");
    drop(guardian);
    assert_native_eof_retirement(&mut owner);
    owner
        .registry
        .authenticated_holder_id(peer, owner.registry.authority(), claims[0].1.grant.id())
        .unwrap();

    payload_peer.write_all(b"ok").unwrap();
    let mut imported = UnixStream::from(received.remove(0));
    imported
        .set_read_timeout(Some(std::time::Duration::from_secs(1)))
        .unwrap();
    let mut data = [0u8; 2];
    imported.read_exact(&mut data).unwrap();
    assert_eq!(
        &data, b"ok",
        "payload stays functional after guardian retirement"
    );
    drop(imported);
    owner.release_holder(peer, claims[0].1.grant.id()).unwrap();
    assert_eq!(owner.registry.delegation_count(), 0);
}

#[test]
fn invalid_final_holder_never_prepares_partial_native_or_capability_state() {
    let (mut owner, peer, carrier, grant) = fixture();
    let foreign = owner
        .register_pair(ProcessEpoch {
            pid: 9,
            instance: 9002,
        })
        .unwrap();
    let (left, right) = UnixStream::pair().unwrap();
    let payloads = vec![OwnedFd::from(left), OwnedFd::from(right)];
    assert!(
        owner
            .prepare(
                peer,
                carrier.holder_a.id(),
                &payloads,
                &[(0, grant.holder_a.id()), (1, foreign.holder_a.id())]
            )
            .is_err()
    );
    assert_eq!(owner.leases.active_count(), 0);
    assert_eq!(owner.registry.delegation_count(), 0);
    assert!(owner.ledger.is_empty());
}

#[test]
fn wrong_final_manifest_leaves_core_queued_then_install_abort_revokes_grant() {
    let (mut owner, peer, carrier, grant) = fixture();
    let (left, right) = UnixStream::pair().unwrap();
    let payloads = vec![OwnedFd::from(left), OwnedFd::from(right)];
    let prepared = owner
        .prepare(
            peer,
            carrier.holder_a.id(),
            &payloads,
            &[(0, grant.holder_a.id()), (1, grant.holder_b.id())],
        )
        .unwrap();
    let key = prepared.transfer.key();
    let manifest = owner.ledger[&key]
        .managed
        .iter()
        .map(|(ordinal, id)| (*ordinal, id.get()))
        .collect::<Vec<_>>();
    let mut wrong = manifest.clone();
    wrong[1].0 = 0;
    let holders = owner.registry.holder_count();
    assert!(
        owner
            .admit(peer, carrier.holder_b.id(), key, 2, &wrong, &[0, 1])
            .is_err()
    );
    assert_eq!(owner.leases.state(key).unwrap(), LeaseState::Queued);
    assert_eq!(owner.registry.holder_count(), holders);
    let claims = owner
        .admit(peer, carrier.holder_b.id(), key, 2, &manifest, &[0])
        .unwrap();
    assert_eq!(claims.len(), 1, "discarded ordinal receives no holder");
    owner
        .settle(peer, key, DeliveryDisposition::Aborted)
        .unwrap();
    assert_eq!(owner.registry.holder_count(), holders);
    assert!(owner.has_pending().unwrap());
    drop(prepared);
    assert_native_eof_retirement(&mut owner);
    assert_eq!(owner.registry.delegation_count(), 0);
}

#[test]
fn lost_prepared_or_admitted_response_is_reaped_only_by_writer_eof() {
    let (mut owner, peer, carrier, grant) = fixture();
    let (left, _right) = UnixStream::pair().unwrap();
    let payloads = vec![OwnedFd::from(left)];
    for admitted in [false, true] {
        let prepared = owner
            .prepare(
                peer,
                carrier.holder_a.id(),
                &payloads,
                &[(0, grant.holder_a.id())],
            )
            .unwrap();
        let key = prepared.transfer.key();
        let baseline = owner.registry.holder_count();
        if admitted {
            let manifest = owner.ledger[&key]
                .managed
                .iter()
                .map(|(ordinal, id)| (*ordinal, id.get()))
                .collect::<Vec<_>>();
            owner
                .admit(peer, carrier.holder_b.id(), key, 1, &manifest, &[0])
                .unwrap();
            assert_eq!(owner.registry.holder_count(), baseline + 1);
        }
        assert!(owner.has_pending().unwrap());
        drop(prepared);
        assert_native_eof_retirement(&mut owner);
        assert_eq!(owner.registry.holder_count(), baseline);
        assert_eq!(owner.registry.delegation_count(), 0);
        assert!(owner.ledger.is_empty());
    }
    assert!(unsafe { libc::fcntl(payloads[0].as_raw_fd(), libc::F_GETFD) } >= 0);
}
