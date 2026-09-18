use super::*;
use crate::{AuthorityEpoch, ProcessEpoch, TransferKey};

fn authority() -> AuthorityEpoch {
    AuthorityEpoch { instance: 0xabc }
}
fn source() -> ProcessEpoch {
    ProcessEpoch {
        pid: 11,
        instance: 1,
    }
}
fn receiver() -> ProcessEpoch {
    ProcessEpoch {
        pid: 22,
        instance: 1,
    }
}
fn binding() -> Binding {
    Binding::Binder {
        source_connection: 4,
        transfer: 9,
        ordinal: 1,
        object_offset: 40,
    }
}
fn setup() -> (CapabilityRegistry, RegisteredPair, DelegationId) {
    let mut registry = CapabilityRegistry::new(authority()).unwrap();
    let pair = registry.register_pair(source()).unwrap();
    let id = registry
        .prepare_delegation(pair.holder_a, binding())
        .unwrap();
    (registry, pair, id)
}

#[test]
fn wrong_process_and_replay_are_rejected() {
    let (mut registry, pair, id) = setup();
    let wrong = ProcessEpoch {
        pid: source().pid,
        instance: 2,
    };
    assert_eq!(
        registry.authenticated_holder_id(wrong, authority(), pair.holder_a.id()),
        Err(CapabilityError::WrongHolder)
    );
    let authorizer = registry.trusted_authorizer();
    registry
        .commit_source_export(pair.holder_a, id, binding())
        .unwrap();
    registry
        .authorize_delivery(&authorizer, id, binding(), receiver())
        .unwrap();
    let attrs = encode_attributes(CapabilityAttributes {
        kind: AttributeKind::Binder,
        authority: authority(),
        delegation: id,
    });
    let claim = registry.claim(receiver(), &attrs, binding()).unwrap();
    assert_eq!(
        registry.claim(receiver(), &attrs, binding()),
        Err(CapabilityError::WrongState)
    );
    assert_eq!(
        registry.finish(id, pair.holder_a),
        Err(CapabilityError::WrongHolder)
    );
    registry.abort(id, claim.grant).unwrap();
}

#[test]
fn source_death_removes_pending_but_committed_unrouted_survives() {
    let (mut registry, pair, pending) = setup();
    let committed = registry
        .prepare_delegation(pair.holder_b, binding())
        .unwrap();
    registry
        .commit_source_export(pair.holder_b, committed, binding())
        .unwrap();
    registry.source_died(source()).unwrap();
    assert_eq!(
        registry.delegation(pending),
        Err(CapabilityError::UnknownDelegation)
    );
    assert_eq!(
        registry.delegation(committed).unwrap().state,
        DelegationState::Committed { receiver: None }
    );
}

#[test]
fn exact_binding_and_same_endpoint_claim_are_required() {
    let (mut registry, pair, id) = setup();
    let authorizer = registry.trusted_authorizer();
    registry
        .commit_source_export(pair.holder_a, id, binding())
        .unwrap();
    registry
        .authorize_delivery(&authorizer, id, binding(), receiver())
        .unwrap();
    let attrs = encode_attributes(CapabilityAttributes {
        kind: AttributeKind::Binder,
        authority: authority(),
        delegation: id,
    });
    let wrong_binding = Binding::Binder {
        source_connection: 4,
        transfer: 9,
        ordinal: 1,
        object_offset: 48,
    };
    assert_eq!(
        registry.claim(receiver(), &attrs, wrong_binding),
        Err(CapabilityError::WrongBinding)
    );
    let claim = registry.claim(receiver(), &attrs, binding()).unwrap();
    assert_eq!(claim.endpoint, pair.endpoint_a);
}

#[test]
fn attributes_are_strict_little_endian_metadata_not_authority() {
    let id = DelegationId(0x1234);
    let bytes = encode_attributes(CapabilityAttributes {
        kind: AttributeKind::Scm,
        authority: authority(),
        delegation: id,
    });
    assert_eq!(bytes.len(), 40);
    assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 1);
    assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 2);
    assert_eq!(decode_attributes(&bytes).unwrap().delegation, id);
    assert_eq!(
        decode_attributes(&bytes[..39]),
        Err(AttributeError::WrongLength)
    );
}

#[test]
fn quota_recovers_after_abort() {
    let (mut registry, pair, id) = setup();
    let authorizer = registry.trusted_authorizer();
    registry
        .commit_source_export(pair.holder_a, id, binding())
        .unwrap();
    registry
        .authorize_delivery(&authorizer, id, binding(), receiver())
        .unwrap();
    let attrs = encode_attributes(CapabilityAttributes {
        kind: AttributeKind::Binder,
        authority: authority(),
        delegation: id,
    });
    let claim = registry.claim(receiver(), &attrs, binding()).unwrap();
    registry.abort(id, claim.grant).unwrap();
    assert_eq!(registry.delegation_count(), 0);
    assert_eq!(registry.holder_count(), 2);
}

#[test]
fn zero_binding_fields_are_rejected_before_delegation() {
    let mut registry = CapabilityRegistry::new(authority()).unwrap();
    let pair = registry.register_pair(source()).unwrap();
    let bad = Binding::Binder {
        source_connection: 0,
        transfer: 3,
        ordinal: 0,
        object_offset: 0,
    };
    assert_eq!(
        registry.prepare_delegation(pair.holder_a, bad),
        Err(CapabilityError::WrongBinding)
    );
    let _ = TransferKey {
        authority: authority(),
        ticket: 1,
    };
}

#[test]
fn tombstone_saturation_seals_minting_but_still_cleans_dead_holders() {
    let mut registry = CapabilityRegistry::new(authority()).unwrap();
    registry.register_pair(source()).unwrap();
    for instance in 10..10 + MAX_DEAD_PROCESSES as u128 {
        registry
            .source_died(ProcessEpoch { pid: 100, instance })
            .unwrap();
    }
    assert_eq!(
        registry.source_died(source()),
        Err(CapabilityError::QuotaExceeded)
    );
    assert_eq!(registry.holder_count(), 0);
    assert_eq!(registry.carrier_count(), 0);
    assert_eq!(
        registry.register_pair(receiver()),
        Err(CapabilityError::QuotaExceeded)
    );
    assert_eq!(
        registry.source_died(receiver()),
        Err(CapabilityError::QuotaExceeded)
    );
}
