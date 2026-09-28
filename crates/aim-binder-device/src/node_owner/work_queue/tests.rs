use super::*;
use crate::{
    objects::{self, Kind},
    reference_table::{ReferenceTable, Strength},
};

#[test]
fn queued_before_wait_and_enqueue_racing_wait_are_not_lost() {
    let mut owner = NodeOwner::new();
    let node = node(&mut owner, 123);
    assert!(!owner.wait_for_node_work(Duration::ZERO));
    for _ in 0..32 {
        std::thread::scope(|scope| {
            let (ready, started) = std::sync::mpsc::channel();
            let owner_ref = &owner;
            let waiter = scope.spawn(move || {
                ready.send(()).unwrap();
                owner_ref.wait_for_node_work(Duration::from_secs(2))
            });
            started.recv_timeout(Duration::from_secs(2)).unwrap();
            let hold = node.hold(HoldKind::Temporary).unwrap();
            assert!(waiter.join().unwrap());
            assert!(owner.wait_for_node_work(Duration::ZERO));
            drop(hold);
        });
        // All holds vanished before any notification was published.
        assert_eq!(owner.read_pending_node_work(&mut [0; 40]).unwrap(), 0);
        assert!(!owner.wait_for_node_work(Duration::ZERO));
    }
}

#[test]
fn concurrent_producers_coalesce_before_read() {
    let mut owner = NodeOwner::new();
    let node = node(&mut owner, 123);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let node = Arc::clone(&node);
            scope.spawn(move || {
                for _ in 0..1000 {
                    drop(node.hold(HoldKind::Temporary).unwrap());
                }
            });
        }
    });
    assert_eq!(owner.state.work.pending.lock().unwrap().order.len(), 1);
    assert!(owner.wait_for_node_work(Duration::ZERO));
    assert_eq!(owner.read_pending_node_work(&mut [0; 40]).unwrap(), 0);
    assert!(!owner.wait_for_node_work(Duration::ZERO));
}

#[test]
fn concurrent_reader_and_producers_preserve_final_release() {
    let mut owner = NodeOwner::new();
    let node = node(&mut owner, 123);
    let pinned = node.hold(HoldKind::Weak).unwrap();
    let mut output = [0; 20];
    assert_eq!(owner.read_pending_node_work(&mut output).unwrap(), 20);
    owner.acknowledge(123, 456, Strength::Weak).unwrap();
    let start = std::sync::Barrier::new(5);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            let node = Arc::clone(&node);
            let start = &start;
            scope.spawn(move || {
                start.wait();
                for _ in 0..1000 {
                    drop(node.hold(HoldKind::Temporary).unwrap());
                    std::thread::yield_now();
                }
            });
        }
        start.wait();
        for _ in 0..1000 {
            // The pinned weak hold prevents a release while other threads
            // enqueue/coalesce work. No extra INCREFS may be published either.
            assert_eq!(owner.read_pending_node_work(&mut output).unwrap(), 0);
            std::thread::yield_now();
        }
    });
    drop(pinned);
    assert!(owner.wait_for_node_work(Duration::ZERO));
    assert_eq!(owner.read_pending_node_work(&mut output).unwrap(), 20);
    assert_eq!(
        u32::from_le_bytes(output[..4].try_into().unwrap()),
        0x8010720a
    );
    assert_eq!(u64::from_le_bytes(output[4..12].try_into().unwrap()), 123);
    assert_eq!(owner.read_pending_node_work(&mut output).unwrap(), 0);
    assert!(!owner.wait_for_node_work(Duration::ZERO));
}

fn node(owner: &mut NodeOwner, pointer: u64) -> Arc<Node> {
    let mut data = [0; 24];
    data[..4].copy_from_slice(&Kind::Binder.tag().to_le_bytes());
    data[8..16].copy_from_slice(&pointer.to_le_bytes());
    data[16..24].copy_from_slice(&456u64.to_le_bytes());
    let objects = objects::validate(&data, &0u64.to_le_bytes()).unwrap();
    owner.resolve(&objects[0]).unwrap()
}
#[test]
fn coalesces_updates_retries_short_read_and_schedules_ack_teardown() {
    let mut owner = NodeOwner::new();
    let node = node(&mut owner, 123);
    let mut refs = ReferenceTable::default();
    refs.retain(node, Strength::Strong).unwrap();
    for _ in 0..100 {
        refs.increment(1, Strength::Strong).unwrap();
    }
    assert_eq!(owner.state.work.pending.lock().unwrap().order.len(), 1);
    let mut short = [0xcc; 39];
    assert_eq!(
        owner
            .read_pending_node_work(&mut short)
            .unwrap_err()
            .raw_os_error(),
        Some(libc::ENOSPC)
    );
    assert_eq!(short, [0xcc; 39]);
    let mut output = [0; 40];
    assert_eq!(owner.read_pending_node_work(&mut output).unwrap(), 40);
    assert_eq!(owner.read_pending_node_work(&mut output).unwrap(), 0);
    owner.acknowledge(123, 456, Strength::Weak).unwrap();
    owner.acknowledge(123, 456, Strength::Strong).unwrap();
    assert_eq!(owner.read_pending_node_work(&mut output).unwrap(), 0);
    drop(refs);
    assert_eq!(owner.read_pending_node_work(&mut output).unwrap(), 40);
    assert_eq!(
        u32::from_le_bytes(output[..4].try_into().unwrap()),
        0x80107209
    );
    assert_eq!(owner.read_pending_node_work(&mut output).unwrap(), 0);
}
#[test]
fn preserves_first_schedule_order_across_distinct_nodes() {
    let mut owner = NodeOwner::new();
    let first = node(&mut owner, 999);
    let second = node(&mut owner, 123);
    let a = first.hold(HoldKind::Weak).unwrap();
    let b = second.hold(HoldKind::Weak).unwrap();
    let mut output = [0; 20];
    for pointer in [999u64, 123] {
        assert_eq!(owner.read_pending_node_work(&mut output).unwrap(), 20);
        assert_eq!(
            u64::from_le_bytes(output[4..12].try_into().unwrap()),
            pointer
        );
        owner.acknowledge(pointer, 456, Strength::Weak).unwrap();
    }
    drop((a, b));
    for pointer in [999u64, 123] {
        assert_eq!(owner.read_pending_node_work(&mut output).unwrap(), 20);
        assert_eq!(
            u64::from_le_bytes(output[4..12].try_into().unwrap()),
            pointer
        );
        assert_eq!(
            u32::from_le_bytes(output[..4].try_into().unwrap()),
            0x8010720a
        );
    }
}
