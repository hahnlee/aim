use super::{CloseOutcome, Error, Opened, Outbound, PeerIdentity, RouteOutcome, Session};
use crate::authority_protocol::{
    CallToken, ConnectionToken, LocalNodeToken, Message, NodeToken, TransferToken,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

struct ConnectionState {
    peer: PeerIdentity,
    nodes: HashSet<LocalNodeToken>,
}

#[derive(Clone, Copy)]
struct CallRoute {
    sender: ConnectionToken,
    caller_thread: u64,
    target: NodeToken,
}

#[derive(Default)]
struct State {
    next_connection: u64,
    next_call: u64,
    connections: HashMap<ConnectionToken, ConnectionState>,
    calls: HashMap<CallToken, CallRoute>,
    deaths: HashMap<(ConnectionToken, NodeToken), u64>,
    context_manager: Option<NodeToken>,
    context_manager_uid: Option<u32>,
}

pub struct RoutingAuthority {
    identity: Arc<()>,
    state: Mutex<State>,
}

impl Default for RoutingAuthority {
    fn default() -> Self {
        Self {
            identity: Arc::new(()),
            state: Mutex::new(State::default()),
        }
    }
}

impl RoutingAuthority {
    pub fn open_authenticated(&self, peer: PeerIdentity) -> Result<Opened, Error> {
        let mut state = self.state.lock().map_err(|_| Error::Poisoned)?;
        let serial = state
            .next_connection
            .checked_add(1)
            .ok_or(Error::ConnectionIdsExhausted)?;
        let connection = ConnectionToken::from_nonzero(serial).unwrap();
        state
            .connections
            .try_reserve(1)
            .map_err(|_| Error::OutOfMemory)?;
        state.connections.insert(
            connection,
            ConnectionState {
                peer,
                nodes: HashSet::new(),
            },
        );
        state.next_connection = serial;
        Ok(Opened {
            session: Session {
                authority: Arc::clone(&self.identity),
                connection,
            },
            response: Message::ConnectionOpened {
                connection,
                android_uid: peer.android_uid(),
            },
        })
    }

    pub fn peer(&self, session: &Session) -> Result<PeerIdentity, Error> {
        let state = self.state.lock().map_err(|_| Error::Poisoned)?;
        Ok(self.connection(&state, session)?.peer)
    }

    pub fn publish_node(&self, session: &Session, local: LocalNodeToken) -> Result<Message, Error> {
        let mut state = self.state.lock().map_err(|_| Error::Poisoned)?;
        self.check(session)?;
        let connection = state
            .connections
            .get_mut(&session.connection)
            .ok_or(Error::UnknownConnection)?;
        if !connection.nodes.contains(&local) {
            connection
                .nodes
                .try_reserve(1)
                .map_err(|_| Error::OutOfMemory)?;
            connection.nodes.insert(local);
        }
        Ok(Message::NodePublished {
            node: NodeToken::new(session.connection, local),
        })
    }

    /// Implements the global Binder context-manager ownership contract. The
    /// authenticated Android UID, never a request field or host UID, controls
    /// registration. Like the kernel driver, the first authorized UID remains
    /// sticky even if a later node check fails.
    pub fn set_context_manager(
        &self,
        session: &Session,
        local: LocalNodeToken,
    ) -> Result<Message, Error> {
        let mut state = self.state.lock().map_err(|_| Error::Poisoned)?;
        self.check(session)?;
        if state.context_manager.is_some() {
            return Err(Error::ContextManagerBusy);
        }
        let uid = state
            .connections
            .get(&session.connection)
            .map(|connection| connection.peer.android_uid())
            .ok_or(Error::UnknownConnection)?;
        if uid != 0 && uid != 1000 {
            return Err(Error::ContextManagerSecurity);
        }
        if state.context_manager_uid.is_some_and(|owner| owner != uid) {
            return Err(Error::ContextManagerWrongUid);
        }
        state.context_manager_uid = Some(uid);
        let connection = state
            .connections
            .get_mut(&session.connection)
            .ok_or(Error::UnknownConnection)?;
        if !connection.nodes.contains(&local) {
            connection
                .nodes
                .try_reserve(1)
                .map_err(|_| Error::OutOfMemory)?;
            connection.nodes.insert(local);
        }
        let node = NodeToken::new(session.connection, local);
        state.context_manager = Some(node);
        Ok(Message::ContextManagerSet { node })
    }

    pub fn get_context_manager(&self, session: &Session) -> Result<Message, Error> {
        let state = self.state.lock().map_err(|_| Error::Poisoned)?;
        self.connection(&state, session)?;
        Ok(Message::ContextManagerFound {
            node: state.context_manager,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn route_transaction(
        &self,
        sender: &Session,
        target: NodeToken,
        caller_thread: u64,
        transfer: TransferToken,
        code: u32,
        flags: u32,
    ) -> Result<RouteOutcome, Error> {
        if caller_thread == 0 {
            return Err(Error::InvalidThread);
        }
        let mut state = self.state.lock().map_err(|_| Error::Poisoned)?;
        let sender_peer = self.connection(&state, sender)?.peer;
        let target_connection = state
            .connections
            .get(&target.owner())
            .ok_or(Error::DeadTarget)?;
        if !target_connection.nodes.contains(&target.local()) {
            return Err(Error::UnknownNode);
        }
        let call = if flags & 1 == 0 {
            let serial = state
                .next_call
                .checked_add(1)
                .ok_or(Error::CallIdsExhausted)?;
            state.calls.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
            let call = CallToken::from_nonzero(serial).unwrap();
            state.calls.insert(
                call,
                CallRoute {
                    sender: sender.connection,
                    caller_thread,
                    target,
                },
            );
            state.next_call = serial;
            Some(call)
        } else {
            None
        };
        Ok(RouteOutcome {
            acknowledgement: Message::RouteAccepted { call },
            delivery: Outbound {
                destination: target.owner(),
                message: Message::DeliverTransaction {
                    call,
                    sender: sender.connection,
                    sender_pid: if flags & 1 == 0 {
                        sender_peer.pid() as i32
                    } else {
                        0
                    },
                    sender_euid: sender_peer.android_uid(),
                    target: target.local(),
                    caller_thread,
                    transfer,
                    code,
                    flags,
                },
            },
        })
    }

    pub fn complete_reply(
        &self,
        replier: &Session,
        call: CallToken,
        transfer: TransferToken,
        code: u32,
        flags: u32,
    ) -> Result<Option<Outbound>, Error> {
        let mut state = self.state.lock().map_err(|_| Error::Poisoned)?;
        let replier_peer = self.connection(&state, replier)?.peer;
        let route = state.calls.get(&call).ok_or(Error::UnknownCall)?;
        if route.target.owner() != replier.connection {
            return Err(Error::WrongReplier);
        }
        let destination = route.sender;
        let target_thread = route.caller_thread;
        state.calls.remove(&call);
        if !state.connections.contains_key(&destination) {
            // Linux Binder accepts a reply that was already in flight when
            // the caller died. There is no recipient left, so consume the
            // exact call and let the transport discard its payload.
            return Ok(None);
        }
        Ok(Some(Outbound {
            destination,
            message: Message::DeliverReply {
                call,
                source: replier.connection,
                sender_pid: replier_peer.pid() as i32,
                sender_euid: replier_peer.android_uid(),
                target_thread,
                transfer,
                code,
                flags,
            },
        }))
    }

    pub fn request_death(
        &self,
        subscriber: &Session,
        target: NodeToken,
        cookie: u64,
    ) -> Result<Message, Error> {
        let mut state = self.state.lock().map_err(|_| Error::Poisoned)?;
        self.connection(&state, subscriber)?;
        let target_connection = state
            .connections
            .get(&target.owner())
            .ok_or(Error::UnknownConnection)?;
        if !target_connection.nodes.contains(&target.local()) {
            return Err(Error::UnknownNode);
        }
        let key = (subscriber.connection, target);
        if state.deaths.contains_key(&key) {
            return Err(Error::DeathAlreadyRequested);
        }
        state
            .deaths
            .try_reserve(1)
            .map_err(|_| Error::OutOfMemory)?;
        state.deaths.insert(key, cookie);
        Ok(Message::DeathRequested)
    }

    pub fn clear_death(
        &self,
        subscriber: &Session,
        target: NodeToken,
        cookie: u64,
    ) -> Result<Message, Error> {
        let mut state = self.state.lock().map_err(|_| Error::Poisoned)?;
        self.connection(&state, subscriber)?;
        let key = (subscriber.connection, target);
        match state.deaths.get(&key) {
            Some(registered) if *registered == cookie => {}
            _ => return Err(Error::UnknownDeath),
        }
        state.deaths.remove(&key);
        Ok(Message::DeathCleared)
    }

    pub fn close(&self, session: &Session) -> Result<CloseOutcome, Error> {
        let mut state = self.state.lock().map_err(|_| Error::Poisoned)?;
        self.connection(&state, session)?;
        let affected = state
            .calls
            .values()
            .filter(|route| {
                route.sender == session.connection || route.target.owner() == session.connection
            })
            .count();
        let mut call_ids = Vec::new();
        call_ids
            .try_reserve(affected)
            .map_err(|_| Error::OutOfMemory)?;
        call_ids.extend(state.calls.iter().filter_map(|(&call, route)| {
            (route.sender == session.connection || route.target.owner() == session.connection)
                .then_some(call)
        }));
        let call_notifications_needed = call_ids
            .iter()
            .filter(|call| {
                state.calls.get(call).is_some_and(|route| {
                    (route.target.owner() == session.connection
                        && route.sender != session.connection
                        && state.connections.contains_key(&route.sender))
                        || (route.sender == session.connection
                            && route.target.owner() != session.connection
                            && state.connections.contains_key(&route.target.owner()))
                })
            })
            .count();
        let death_notifications_needed = state
            .deaths
            .iter()
            .filter(|((subscriber, target), _)| {
                target.owner() == session.connection
                    && *subscriber != session.connection
                    && state.connections.contains_key(subscriber)
            })
            .count();
        let mut notifications = Vec::new();
        notifications
            .try_reserve(call_notifications_needed + death_notifications_needed)
            .map_err(|_| Error::OutOfMemory)?;
        for call in call_ids {
            let route = *state.calls.get(&call).unwrap();
            if route.target.owner() == session.connection
                && route.sender != session.connection
                && state.connections.contains_key(&route.sender)
            {
                state.calls.remove(&call);
                notifications.push(Outbound {
                    destination: route.sender,
                    message: Message::TargetDead {
                        call,
                        target_thread: route.caller_thread,
                    },
                });
            } else if route.sender == session.connection
                && route.target.owner() != session.connection
                && state.connections.contains_key(&route.target.owner())
            {
                // Keep the route as a caller-death tombstone. The exact
                // target may already be executing and must be allowed to
                // submit one late reply, whose payload will be discarded.
                notifications.push(Outbound {
                    destination: route.target.owner(),
                    message: Message::CallerDead { call },
                });
            } else {
                state.calls.remove(&call);
            }
        }
        for (&(subscriber, target), &cookie) in &state.deaths {
            if target.owner() == session.connection
                && subscriber != session.connection
                && state.connections.contains_key(&subscriber)
            {
                notifications.push(Outbound {
                    destination: subscriber,
                    message: Message::NodeDead { cookie },
                });
            }
        }
        // A target death publishes BR_DEAD_BINDER, but the subscriber still
        // owns the registration until BC_CLEAR_DEATH_NOTIFICATION or its own
        // disconnect. Retain that exact key so a clear racing with target
        // teardown receives BR_CLEAR_DEATH_NOTIFICATION_DONE instead of
        // poisoning the Binder connection.
        state
            .deaths
            .retain(|(subscriber, _), _| *subscriber != session.connection);
        state.connections.remove(&session.connection);
        if state
            .context_manager
            .is_some_and(|node| node.owner() == session.connection)
        {
            state.context_manager = None;
        }
        Ok(CloseOutcome { notifications })
    }

    fn check(&self, session: &Session) -> Result<(), Error> {
        Arc::ptr_eq(&self.identity, &session.authority)
            .then_some(())
            .ok_or(Error::ForeignAuthority)
    }

    fn connection<'a>(
        &self,
        state: &'a State,
        session: &Session,
    ) -> Result<&'a ConnectionState, Error> {
        self.check(session)?;
        state
            .connections
            .get(&session.connection)
            .ok_or(Error::UnknownConnection)
    }
}
