use super::*;
use crate::authority_protocol::{
    CallToken, ConnectionToken, LocalNodeToken, Message, NodeToken, TransferToken,
};
use std::{
    collections::HashSet,
    sync::{Arc, Barrier},
    thread,
};

fn peer(pid: u32) -> PeerIdentity {
    PeerIdentity::verified(pid, 10_000 + pid, [pid as u64 * 10, 0]).unwrap()
}

fn system_peer(pid: u32) -> PeerIdentity {
    PeerIdentity::verified(pid, 1000, [pid as u64 * 10, 0]).unwrap()
}

fn local(raw: u64) -> LocalNodeToken {
    LocalNodeToken::from_nonzero(raw).unwrap()
}

fn transfer(raw: u64) -> TransferToken {
    TransferToken::from_nonzero(raw).unwrap()
}

fn call_from(message: Message) -> CallToken {
    let Message::RouteAccepted { call: Some(call) } = message else {
        panic!("expected synchronous call")
    };
    call
}

#[test]
fn authenticated_peers_publish_nodes_route_and_reply_without_payload_ownership() {
    let authority = RoutingAuthority::default();
    let caller = authority.open_authenticated(peer(10)).unwrap();
    let callee = authority.open_authenticated(peer(20)).unwrap();
    assert_eq!(authority.peer(&caller.session).unwrap(), peer(10));

    let Message::NodePublished { node } =
        authority.publish_node(&callee.session, local(7)).unwrap()
    else {
        panic!("expected node publication")
    };
    let routed = authority
        .route_transaction(&caller.session, node, 44, transfer(8), 9, 0)
        .unwrap();
    let call = call_from(routed.acknowledgement);
    assert_eq!(routed.delivery.destination, callee.session.connection());
    assert_eq!(
        routed.delivery.message,
        Message::DeliverTransaction {
            call: Some(call),
            sender: caller.session.connection(),
            sender_pid: 10,
            sender_euid: 10_010,
            target: local(7),
            caller_thread: 44,
            transfer: transfer(8),
            code: 9,
            flags: 0,
        }
    );
    assert_eq!(
        authority
            .complete_reply(&callee.session, call, transfer(10), 11, 12)
            .unwrap(),
        Some(Outbound {
            destination: caller.session.connection(),
            message: Message::DeliverReply {
                call,
                source: callee.session.connection(),
                sender_pid: 20,
                sender_euid: 10_020,
                target_thread: 44,
                transfer: transfer(10),
                code: 11,
                flags: 12,
            },
        })
    );
    assert_eq!(
        authority.complete_reply(&callee.session, call, transfer(11), 0, 0),
        Err(Error::UnknownCall)
    );
}

#[test]
fn context_manager_requires_system_uid_and_atomically_publishes_owner_node() {
    let authority = RoutingAuthority::default();
    let app = authority.open_authenticated(peer(10)).unwrap();
    let system = authority.open_authenticated(system_peer(20)).unwrap();
    assert_eq!(
        authority.get_context_manager(&app.session).unwrap(),
        Message::ContextManagerFound { node: None }
    );
    assert_eq!(
        authority.set_context_manager(&app.session, local(1)),
        Err(Error::ContextManagerSecurity)
    );
    let Message::ContextManagerSet { node } = authority
        .set_context_manager(&system.session, local(1))
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(
        authority.get_context_manager(&app.session).unwrap(),
        Message::ContextManagerFound { node: Some(node) }
    );
    assert_eq!(
        authority.set_context_manager(&system.session, local(1)),
        Err(Error::ContextManagerBusy)
    );
    authority.close(&system.session).unwrap();
    assert_eq!(
        authority.get_context_manager(&app.session).unwrap(),
        Message::ContextManagerFound { node: None }
    );
}

#[test]
fn foreign_stale_spoofed_node_and_wrong_replier_are_rejected() {
    let first = RoutingAuthority::default();
    let second = RoutingAuthority::default();
    let caller = first.open_authenticated(peer(10)).unwrap();
    let callee = first.open_authenticated(peer(20)).unwrap();
    let foreign = second.open_authenticated(peer(30)).unwrap();
    assert_eq!(
        first.publish_node(&foreign.session, local(1)),
        Err(Error::ForeignAuthority)
    );
    let forged = NodeToken::new(callee.session.connection(), local(99));
    assert_eq!(
        first.route_transaction(&caller.session, forged, 1, transfer(1), 0, 0),
        Err(Error::UnknownNode)
    );
    let Message::NodePublished { node } = first.publish_node(&callee.session, local(1)).unwrap()
    else {
        unreachable!()
    };
    let routed = first
        .route_transaction(&caller.session, node, 1, transfer(1), 0, 0)
        .unwrap();
    let call = call_from(routed.acknowledgement);
    assert_eq!(
        first.complete_reply(&caller.session, call, transfer(2), 0, 0),
        Err(Error::WrongReplier)
    );
    assert!(
        first
            .complete_reply(&callee.session, call, transfer(2), 0, 0)
            .is_ok_and(|delivery| delivery.is_some())
    );
    first.close(&caller.session).unwrap();
    assert_eq!(
        first.publish_node(&caller.session, local(2)),
        Err(Error::UnknownConnection)
    );
}

#[test]
fn oneway_has_no_call_and_target_death_unwinds_only_live_callers() {
    let authority = RoutingAuthority::default();
    let caller = authority.open_authenticated(peer(10)).unwrap();
    let callee = authority.open_authenticated(peer(20)).unwrap();
    let Message::NodePublished { node } =
        authority.publish_node(&callee.session, local(1)).unwrap()
    else {
        unreachable!()
    };
    let oneway = authority
        .route_transaction(&caller.session, node, 1, transfer(1), 0, 1)
        .unwrap();
    assert_eq!(
        oneway.acknowledgement,
        Message::RouteAccepted { call: None }
    );
    assert_eq!(
        oneway.delivery.message,
        Message::DeliverTransaction {
            call: None,
            sender: caller.session.connection(),
            sender_pid: 0,
            sender_euid: 10_010,
            target: local(1),
            caller_thread: 1,
            transfer: transfer(1),
            code: 0,
            flags: 1,
        }
    );

    let sync = authority
        .route_transaction(&caller.session, node, 77, transfer(2), 0, 0)
        .unwrap();
    let call = call_from(sync.acknowledgement);
    let closed = authority.close(&callee.session).unwrap();
    assert_eq!(
        closed.notifications,
        vec![Outbound {
            destination: caller.session.connection(),
            message: Message::TargetDead {
                call,
                target_thread: 77,
            },
        }]
    );
    assert_eq!(
        authority.route_transaction(&caller.session, node, 1, transfer(3), 0, 0),
        Err(Error::DeadTarget)
    );
}

#[test]
fn equal_wire_numbers_from_other_authorities_do_not_cross_capability_boundary() {
    let first = RoutingAuthority::default();
    let second = RoutingAuthority::default();
    let a = first.open_authenticated(peer(1)).unwrap();
    let b = second.open_authenticated(peer(2)).unwrap();
    assert_eq!(a.session.connection(), b.session.connection());
    assert_eq!(first.peer(&b.session), Err(Error::ForeignAuthority));
    assert_eq!(
        ConnectionToken::from_nonzero(1),
        Some(a.session.connection())
    );
}

#[test]
fn concurrent_two_peer_routes_allocate_unique_calls_and_close_drains_them() {
    const ROUTES: usize = 24;
    let authority = Arc::new(RoutingAuthority::default());
    let caller = authority.open_authenticated(peer(1)).unwrap();
    let callee = authority.open_authenticated(peer(2)).unwrap();
    let Message::NodePublished { node } =
        authority.publish_node(&callee.session, local(1)).unwrap()
    else {
        unreachable!()
    };
    let barrier = Arc::new(Barrier::new(ROUTES));
    let mut workers = Vec::new();
    for index in 0..ROUTES {
        let authority = Arc::clone(&authority);
        let caller = caller.session.clone();
        let barrier = Arc::clone(&barrier);
        workers.push(thread::spawn(move || {
            barrier.wait();
            authority
                .route_transaction(
                    &caller,
                    node,
                    index as u64 + 1,
                    transfer(index as u64 + 1),
                    0,
                    0,
                )
                .unwrap()
                .acknowledgement
        }));
    }
    let calls: HashSet<_> = workers
        .into_iter()
        .map(|worker| call_from(worker.join().unwrap()))
        .collect();
    assert_eq!(calls.len(), ROUTES);

    let closed = authority.close(&callee.session).unwrap();
    assert_eq!(closed.notifications.len(), ROUTES);
    let dead: HashSet<_> = closed
        .notifications
        .into_iter()
        .map(|outbound| {
            assert_eq!(outbound.destination, caller.session.connection());
            let Message::TargetDead { call, .. } = outbound.message else {
                unreachable!()
            };
            call
        })
        .collect();
    assert_eq!(dead, calls);
}

#[test]
fn zero_thread_id_is_rejected_before_call_publication() {
    let authority = RoutingAuthority::default();
    let caller = authority.open_authenticated(peer(1)).unwrap();
    let callee = authority.open_authenticated(peer(2)).unwrap();
    let Message::NodePublished { node } =
        authority.publish_node(&callee.session, local(1)).unwrap()
    else {
        unreachable!()
    };
    assert_eq!(
        authority.route_transaction(&caller.session, node, 0, transfer(1), 0, 0),
        Err(Error::InvalidThread)
    );
    let valid = authority
        .route_transaction(&caller.session, node, 1, transfer(1), 0, 0)
        .unwrap();
    assert_eq!(call_from(valid.acknowledgement).get(), 1);
}

#[test]
fn caller_death_accepts_and_discards_one_exact_late_reply() {
    let authority = RoutingAuthority::default();
    let caller = authority.open_authenticated(peer(1)).unwrap();
    let callee = authority.open_authenticated(peer(2)).unwrap();
    let Message::NodePublished { node } =
        authority.publish_node(&callee.session, local(1)).unwrap()
    else {
        unreachable!()
    };
    let call = call_from(
        authority
            .route_transaction(&caller.session, node, 1, transfer(1), 0, 0)
            .unwrap()
            .acknowledgement,
    );
    assert_eq!(
        authority.close(&caller.session).unwrap().notifications,
        vec![Outbound {
            destination: callee.session.connection(),
            message: Message::CallerDead { call },
        }]
    );
    assert_eq!(
        authority.complete_reply(&callee.session, call, transfer(2), 0, 0),
        Ok(None)
    );
    assert_eq!(
        authority.complete_reply(&callee.session, call, transfer(3), 0, 0),
        Err(Error::UnknownCall)
    );
}

#[test]
fn death_registration_is_exact_and_target_close_notifies_subscriber() {
    let authority = RoutingAuthority::default();
    let subscriber = authority.open_authenticated(peer(1)).unwrap();
    let target_owner = authority.open_authenticated(peer(2)).unwrap();
    let Message::NodePublished { node } = authority
        .publish_node(&target_owner.session, local(7))
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(
        authority.request_death(&subscriber.session, node, 99),
        Ok(Message::DeathRequested)
    );
    assert_eq!(
        authority.request_death(&subscriber.session, node, 100),
        Err(Error::DeathAlreadyRequested)
    );
    assert_eq!(
        authority.clear_death(&subscriber.session, node, 100),
        Err(Error::UnknownDeath)
    );
    assert_eq!(
        authority
            .close(&target_owner.session)
            .unwrap()
            .notifications,
        vec![Outbound {
            destination: subscriber.session.connection(),
            message: Message::NodeDead { cookie: 99 },
        }]
    );
    assert_eq!(
        authority.clear_death(&subscriber.session, node, 99),
        Ok(Message::DeathCleared)
    );
    assert_eq!(
        authority.clear_death(&subscriber.session, node, 99),
        Err(Error::UnknownDeath)
    );
}
