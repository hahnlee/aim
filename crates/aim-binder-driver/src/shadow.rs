//! Shadow copies: each transaction to a chosen node and its reply, handed
//! to a sink outside the driver for a comparison with a model of the
//! service (`guest-init --binder-shadow`, docs/m4-packagemanager.md,
//! slice A). A copy changes nothing the caller or the target sees: it is
//! taken from the image the driver builds anyway, and sent without
//! waiting (a full sink drops it and counts the drop).
//!
//! A node handed out in a shadowed reply by the process that replied (a
//! `ParceledListSlice`'s retriever, an installer object) is watched in turn,
//! and the copies of its transactions name the copy it came in, so the
//! comparison can follow a list through its binder without calling it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};

use crate::host::File;
use crate::state::{IdMap, NodeId, TxnId};

/// What a file stands for: its device and inode.
pub type FileId = (u64, u64);

/// A parcel as its sender wrote it: the data, with each object the driver
/// translated named by what it stands for. The object bytes are the
/// sender's.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShadowParcel {
    pub data: Vec<u8>,
    pub objects: Vec<ShadowObject>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShadowObject {
    /// Offset of the object in the data.
    pub offset: usize,
    pub kind: ShadowKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShadowKind {
    /// A binder, strong or weak: the driver's node and the pid of the
    /// process that owns it (0 once it is dead).
    Binder { node: u64, owner_pid: i32 },
    /// A file, as the sink's `identify` saw it.
    File(Option<FileId>),
    /// A scatter-gather buffer or an fd array (HIDL), not interpreted.
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShadowReply {
    /// The reply as the target sent it; `flags` has `TF_STATUS_CODE` when
    /// the data is a status instead of a parcel.
    Reply {
        flags: u32,
        parcel: ShadowParcel,
    },
    OneWay,
    /// None reached the caller: the target died or the call failed.
    Failed,
}

/// One transaction and its reply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShadowCopy {
    /// In the order the driver took the calls.
    pub seq: u64,
    /// The node called: the one watched ([`crate::Driver::shadow`]) or
    /// one followed from it.
    pub node: u64,
    /// The watched node it was followed from (`node` itself if watched).
    pub root: u64,
    /// The copy whose reply handed out `node`, for a followed node.
    pub follows: Option<u64>,
    pub from_pid: i32,
    pub from_euid: u32,
    pub from_tid: i32,
    pub to_pid: i32,
    pub code: u32,
    pub flags: u32,
    pub data: ShadowParcel,
    pub reply: ShadowReply,
}

/// Where copies go.
#[derive(Clone)]
pub struct ShadowSink {
    pub copies: SyncSender<ShadowCopy>,
    /// Names a file while the driver holds it, so the copy holds none.
    pub identify: fn(&File) -> Option<FileId>,
    /// Copies dropped because the sink was full.
    pub dropped: Arc<AtomicU64>,
}

impl ShadowSink {
    fn send(&self, copy: ShadowCopy) {
        if let Err(TrySendError::Full(_)) = self.copies.try_send(copy) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

struct Watch {
    sink: ShadowSink,
    root: NodeId,
    follows: Option<u64>,
}

#[derive(Default)]
pub(crate) struct Shadow {
    watched: IdMap<NodeId, Watch>,
    /// Calls waiting for their reply.
    pending: IdMap<TxnId, (ShadowSink, ShadowCopy)>,
    next_seq: u64,
}

impl Shadow {
    pub fn watch(&mut self, node: NodeId, sink: ShadowSink) {
        self.watched.insert(
            node,
            Watch {
                sink,
                root: node,
                follows: None,
            },
        );
    }

    /// The sink's `identify` when calls to `node` are copied.
    pub fn identify(&self, node: Option<NodeId>) -> Option<fn(&File) -> Option<FileId>> {
        self.watched.get(&node?).map(|w| w.sink.identify)
    }

    /// The same for the reply to `txn`.
    pub fn identify_reply(&self, txn: TxnId) -> Option<fn(&File) -> Option<FileId>> {
        self.pending.get(&txn).map(|(sink, _)| sink.identify)
    }

    /// A call to a watched node was sent: kept until its reply, or sent
    /// at once when none comes.
    pub fn sent(&mut self, id: TxnId, node: NodeId, mut copy: ShadowCopy, oneway: bool) {
        let Some(watch) = self.watched.get(&node) else {
            return;
        };
        self.next_seq += 1;
        copy.seq = self.next_seq;
        copy.node = node;
        copy.root = watch.root;
        copy.follows = watch.follows;
        let sink = watch.sink.clone();
        if oneway {
            copy.reply = ShadowReply::OneWay;
            sink.send(copy);
        } else {
            self.pending.insert(id, (sink, copy));
        }
    }

    /// `txn` was replied to; `own` are the nodes of the replying process
    /// the reply hands out, watched from now on.
    pub fn replied(&mut self, txn: TxnId, flags: u32, parcel: ShadowParcel, own: &[NodeId]) {
        let Some((sink, mut copy)) = self.pending.remove(&txn) else {
            return;
        };
        for &node in own {
            self.watched.entry(node).or_insert(Watch {
                sink: sink.clone(),
                root: copy.root,
                follows: Some(copy.seq),
            });
        }
        copy.reply = ShadowReply::Reply { flags, parcel };
        sink.send(copy);
    }

    /// `txn` ended without a reply.
    pub fn failed(&mut self, txn: TxnId) {
        if let Some((sink, mut copy)) = self.pending.remove(&txn) {
            copy.reply = ShadowReply::Failed;
            sink.send(copy);
        }
    }

    pub fn node_gone(&mut self, node: NodeId) {
        self.watched.remove(&node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    fn sink(capacity: usize) -> (ShadowSink, std::sync::mpsc::Receiver<ShadowCopy>) {
        let (copies, received) = sync_channel(capacity);
        let sink = ShadowSink {
            copies,
            identify: |_| None,
            dropped: Arc::default(),
        };
        (sink, received)
    }

    fn call(code: u32) -> ShadowCopy {
        ShadowCopy {
            seq: 0,
            node: 0,
            root: 0,
            follows: None,
            from_pid: 1,
            from_euid: 2,
            from_tid: 3,
            to_pid: 4,
            code,
            flags: 0,
            data: ShadowParcel::default(),
            reply: ShadowReply::Failed,
        }
    }

    #[test]
    fn pairs_calls_with_replies_and_follows_handed_out_nodes() {
        let (sink, received) = sink(8);
        let mut shadow = Shadow::default();
        shadow.watch(10, sink.clone());
        shadow.sent(1, 11, call(1), false);
        assert!(shadow.pending.is_empty(), "an unwatched node is not copied");

        shadow.sent(2, 10, call(7), false);
        let reply = ShadowParcel {
            data: vec![1, 2, 3, 4],
            objects: Vec::new(),
        };
        shadow.replied(2, 0, reply.clone(), &[20]);
        let first = received.try_recv().unwrap();
        assert_eq!((first.seq, first.node, first.follows), (1, 10, None));
        assert_eq!(
            first.reply,
            ShadowReply::Reply {
                flags: 0,
                parcel: reply
            }
        );

        // The node the reply handed out is followed, naming its copy.
        shadow.sent(3, 20, call(1), true);
        let fetch = received.try_recv().unwrap();
        assert_eq!((fetch.seq, fetch.node, fetch.root), (2, 20, 10));
        assert_eq!(fetch.follows, Some(1));
        assert_eq!(fetch.reply, ShadowReply::OneWay);

        shadow.sent(4, 10, call(8), false);
        shadow.failed(4);
        assert_eq!(received.try_recv().unwrap().reply, ShadowReply::Failed);

        shadow.node_gone(20);
        shadow.sent(5, 20, call(1), true);
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn a_full_sink_drops_and_counts() {
        let (sink, received) = sink(1);
        let mut shadow = Shadow::default();
        shadow.watch(10, sink.clone());
        shadow.sent(1, 10, call(1), true);
        shadow.sent(2, 10, call(1), true);
        assert_eq!(sink.dropped.load(Ordering::Relaxed), 1);
        assert_eq!(received.try_recv().unwrap().seq, 1);
    }
}
