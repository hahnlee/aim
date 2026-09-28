use super::*;
use crate::{AuthorityEpoch, ProcessEpoch, TransferKey};

fn fixture() -> (
    CapabilityRegistry,
    RegisteredPair,
    ProcessEpoch,
    TransferKey,
    Vec<(u64, DelegationId)>,
) {
    let authority = AuthorityEpoch { instance: 888 };
    let source = ProcessEpoch {
        pid: 22,
        instance: 9,
    };
    let receiver = ProcessEpoch {
        pid: 23,
        instance: 10,
    };
    let key = TransferKey {
        authority,
        ticket: 11,
    };
    let mut registry = CapabilityRegistry::new(authority).unwrap();
    let pair = registry.register_pair(source).unwrap();
    let ids = registry
        .prepare_scm_batch(source, key, &[(0, pair.holder_a), (1, pair.holder_b)])
        .unwrap();
    (
        registry,
        pair,
        receiver,
        key,
        vec![(0, ids[0]), (1, ids[1])],
    )
}

#[test]
fn invalid_last_binding_never_installs_the_first_grant() {
    let (mut registry, _, receiver, key, managed) = fixture();
    let authorizer = registry.trusted_authorizer();
    let invalid = [(0, managed[0].1), (2, managed[1].1)];
    assert!(matches!(
        registry.prepare_scm_claim(&authorizer, receiver, key, &invalid, &[0, 2]),
        Err(CapabilityError::WrongBinding)
    ));
    assert_eq!(registry.holder_count(), 2);
    for (_, id) in managed {
        assert_eq!(
            registry.delegation(id).unwrap().state,
            DelegationState::Committed { receiver: None }
        );
    }
}

#[test]
fn mixed_publication_and_discard_then_finish_preserves_only_installed_holder() {
    let (mut registry, pair, receiver, key, managed) = fixture();
    let authorizer = registry.trusted_authorizer();
    let reserved = registry
        .prepare_scm_claim(&authorizer, receiver, key, &managed, &[1])
        .unwrap();
    assert_eq!(registry.holder_count(), 2);
    let installed = registry.commit_scm_claim(reserved).unwrap();
    assert_eq!(installed.len(), 1);
    assert_eq!(installed[0].endpoint, pair.endpoint_b);
    assert_eq!(registry.holder_count(), 3);
    assert_eq!(registry.delegation_count(), 1);
    registry
        .settle_scm_batch(&authorizer, key, DeliveryDisposition::Finished)
        .unwrap();
    assert_eq!(registry.delegation_count(), 0);
    registry
        .authenticated_holder_id(receiver, key.authority, installed[0].grant.id())
        .unwrap();
    registry.release_holder(installed[0].grant).unwrap();
    assert_eq!(registry.holder_count(), 2);
}

#[test]
fn rollback_revokes_claimed_and_unclaimed_without_revoking_source_holders() {
    let (mut registry, pair, receiver, key, managed) = fixture();
    let authorizer = registry.trusted_authorizer();
    let reserved = registry
        .prepare_scm_claim(&authorizer, receiver, key, &managed, &[0, 1])
        .unwrap();
    let installed = registry.commit_scm_claim(reserved).unwrap();
    assert!(matches!(
        registry.prepare_scm_claim(&authorizer, receiver, key, &managed, &[0, 1]),
        Err(CapabilityError::WrongState)
    ));
    registry
        .settle_scm_batch(
            &authorizer,
            TransferKey { ticket: 99, ..key },
            DeliveryDisposition::Aborted,
        )
        .unwrap();
    assert_eq!(registry.holder_count(), 4);
    registry
        .settle_scm_batch(&authorizer, key, DeliveryDisposition::Aborted)
        .unwrap();
    assert_eq!(registry.holder_count(), 2);
    assert_eq!(registry.delegation_count(), 0);
    assert!(registry.guest_dup(pair.holder_a).is_ok());
    for claimed in installed {
        assert!(registry.guest_dup(claimed.grant).is_err());
    }
    let unclaimed = registry
        .prepare_scm_batch(
            pair.holder_a.process(),
            TransferKey { ticket: 12, ..key },
            &[(0, pair.holder_a)],
        )
        .unwrap();
    registry
        .settle_scm_batch(
            &authorizer,
            TransferKey { ticket: 12, ..key },
            DeliveryDisposition::Aborted,
        )
        .unwrap();
    assert!(registry.delegation(unclaimed[0]).is_err());
}

#[test]
fn stale_reservation_fails_before_any_mutation() {
    let (mut registry, _, receiver, key, managed) = fixture();
    let authorizer = registry.trusted_authorizer();
    let reserved = registry
        .prepare_scm_claim(&authorizer, receiver, key, &managed, &[0, 1])
        .unwrap();
    registry
        .settle_scm_batch(&authorizer, key, DeliveryDisposition::Aborted)
        .unwrap();
    assert!(matches!(
        registry.commit_scm_claim(reserved),
        Err(CapabilityError::UnknownDelegation)
    ));
    assert_eq!(registry.holder_count(), 2);
    assert_eq!(registry.delegation_count(), 0);
}

#[test]
fn insufficient_budget_for_last_holder_rejects_the_whole_claim() {
    let (mut registry, pair, receiver, key, managed) = fixture();
    let authorizer = registry.trusted_authorizer();
    for ticket in 100..163 {
        let next = TransferKey { ticket, ..key };
        let payloads: Vec<_> = (0..16).map(|ordinal| (ordinal, pair.holder_a)).collect();
        let ids = registry
            .prepare_scm_batch(pair.holder_a.process(), next, &payloads)
            .unwrap();
        let bindings: Vec<_> = ids
            .into_iter()
            .enumerate()
            .map(|(ordinal, id)| (ordinal as u64, id))
            .collect();
        let selected: Vec<_> = (0..16).collect();
        let batch = registry
            .prepare_scm_claim(&authorizer, receiver, next, &bindings, &selected)
            .unwrap();
        registry.commit_scm_claim(batch).unwrap();
        registry
            .settle_scm_batch(&authorizer, next, DeliveryDisposition::Finished)
            .unwrap();
    }
    let next = TransferKey { ticket: 163, ..key };
    let payloads: Vec<_> = (0..13).map(|ordinal| (ordinal, pair.holder_a)).collect();
    let ids = registry
        .prepare_scm_batch(pair.holder_a.process(), next, &payloads)
        .unwrap();
    let bindings: Vec<_> = ids
        .into_iter()
        .enumerate()
        .map(|(ordinal, id)| (ordinal as u64, id))
        .collect();
    let selected: Vec<_> = (0..13).collect();
    let batch = registry
        .prepare_scm_claim(&authorizer, receiver, next, &bindings, &selected)
        .unwrap();
    registry.commit_scm_claim(batch).unwrap();
    registry
        .settle_scm_batch(&authorizer, next, DeliveryDisposition::Finished)
        .unwrap();
    assert_eq!(registry.holder_count(), MAX_HOLDERS - 1);
    assert!(matches!(
        registry.prepare_scm_claim(&authorizer, receiver, key, &managed, &[0, 1]),
        Err(CapabilityError::QuotaExceeded)
    ));
    assert_eq!(registry.holder_count(), MAX_HOLDERS - 1);
    for (_, id) in managed {
        assert_eq!(
            registry.delegation(id).unwrap().state,
            DelegationState::Committed { receiver: None }
        );
    }
}
