use super::*;
use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, Parcel};

// oracle.py executes the pinned Java writers; only native transport and
// unrelated object APIs are substitutes. These are original writeMap offsets.
fn pinned() -> Vec<u8> {
    include_str!("../../tests/fixtures/wct/wct.hex")
        .split_whitespace()
        .flat_map(|line| {
            (0..line.len())
                .step_by(2)
                .map(|at| u8::from_str_radix(&line[at..at + 2], 16).unwrap())
        })
        .collect()
}
pub(in crate::app_host) fn fixture(token: Binder) -> Parcel {
    let data = pinned();
    let mut p = Parcel::new();
    let mut start = 0;
    for at in [8, 372, 412] {
        p.write_raw(&data[start..at], &[]);
        p.write_binder(Some(token));
        start = at + 28;
    }
    p.write_raw(&data[start..], &[]);
    p
}
/// A `HierarchyOp` as `HierarchyOp.writeToParcel` writes it.
pub(in crate::app_host) struct Op {
    pub kind: i32,
    pub container: Option<Binder>,
    pub containers: Option<Vec<Binder>>,
    pub reparent: Option<Binder>,
    pub to_top: bool,
    pub top_only: bool,
    pub modes: Option<Vec<i32>>,
    pub types: Option<Vec<i32>>,
}
/// A transaction of the pinned fixture's Change, with the edits at its
/// offsets, for each token, then `ops`.
pub(in crate::app_host) fn transaction(
    changes: &[(Binder, &[(usize, i32)])],
    ops: &[Op],
) -> Parcel {
    let data = pinned();
    let mut p = Parcel::new();
    p.write_i32(changes.len() as i32);
    for (token, edits) in changes {
        let mut change = Parcel::new();
        change.write_raw(&data[36..360], &[]);
        for &(at, value) in *edits {
            change.set_i32_at(at - 36, value);
        }
        p.write_i32(15);
        p.write_binder(Some(*token));
        p.write_raw(change.data(), &[]);
    }
    p.write_i32(ops.len() as i32);
    for op in ops {
        p.write_i32(1);
        p.write_i32(op.kind);
        p.write_binder(op.container);
        match &op.containers {
            Some(binders) => {
                p.write_i32(binders.len() as i32);
                binders.iter().for_each(|&b| p.write_binder(Some(b)));
            }
            None => p.write_i32(-1),
        }
        p.write_raw(&data[404..412], &[]);
        p.write_binder(op.reparent);
        p.write_raw(&data[440..472], &[]);
        p.write_bool(op.to_top);
        p.write_bool(op.top_only);
        for array in [&op.modes, &op.types] {
            match array {
                Some(values) => {
                    p.write_i32(values.len() as i32);
                    values.iter().for_each(|&v| p.write_i32(v));
                }
                None => p.write_i32(-1),
            }
        }
        p.write_raw(&data[488..540], &[]);
    }
    p.write_raw(&data[540..], &[]);
    p
}
pub(in crate::app_host) fn reparent(task: Binder, parent: Option<Binder>, to_top: bool) -> Op {
    Op {
        kind: 0,
        container: Some(task),
        containers: None,
        reparent: parent,
        to_top,
        top_only: false,
        modes: None,
        types: None,
    }
}
#[test]
fn pinned_java_wct_map_change_and_reorder_decode() {
    let t = Transaction::read(&mut fixture(Binder::Local(64)).reader()).unwrap();
    assert_eq!(
        t.changes,
        [Change {
            token: Binder::Local(64),
            bounds: Some(Some(Rect {
                left: -80,
                top: 120,
                right: 720,
                bottom: 1120
            })),
            mode: None,
        }]
    );
    assert_eq!(t.hierarchy, [Hop::Reorder(Binder::Local(64), true)]);
    // The same written by the builder.
    let built = transaction(
        &[(Binder::Local(64), &[])],
        &[Op {
            kind: 1,
            reparent: Some(Binder::Local(64)),
            ..reparent(Binder::Local(64), None, true)
        }],
    );
    assert_eq!(built.data(), fixture(Binder::Local(64)).data());
}
// HierarchyOp.createForReparent and createForChildrenTasksReparent.
#[test]
fn reparent_and_reparent_tasks_decode() {
    let (task, root) = (Binder::Local(64), Binder::Local(72));
    let read = |ops: &[Op]| Transaction::read(&mut transaction(&[], ops).reader());
    assert_eq!(
        read(&[
            reparent(task, Some(root), true),
            reparent(task, None, false)
        ])
        .unwrap()
        .hierarchy,
        [
            Hop::Reparent {
                task,
                parent: Some(root),
                to_top: true
            },
            Hop::Reparent {
                task,
                parent: None,
                to_top: false
            }
        ]
    );
    let children = Op {
        kind: 2,
        container: Some(root),
        containers: None,
        reparent: None,
        to_top: true,
        top_only: true,
        modes: Some(vec![1, 6, 0]),
        types: Some(vec![]),
    };
    assert_eq!(
        read(&[children]).unwrap().hierarchy,
        [Hop::ReparentChildren {
            from: Some(root),
            to: None,
            modes: Some(vec![1, 6, 0]),
            types: Some(vec![]),
            to_top: true,
            top_only: true
        }]
    );
    // The filters and reparentTopOnly are reparentTasks' only; a reparent
    // names its task, a reorder its own parent.
    for op in [
        Op {
            top_only: true,
            ..reparent(task, None, true)
        },
        Op {
            modes: Some(vec![1]),
            ..reparent(task, None, true)
        },
        Op {
            types: Some(vec![]),
            ..reparent(task, None, true)
        },
        Op {
            kind: 3,
            ..reparent(task, Some(root), true)
        },
    ] {
        assert_eq!(read(&[op]).unwrap_err().code, EX_UNSUPPORTED_OPERATION);
    }
    for op in [
        Op {
            container: None,
            ..reparent(task, None, true)
        },
        Op {
            kind: 1,
            ..reparent(task, Some(root), true)
        },
    ] {
        assert_eq!(read(&[op]).unwrap_err().code, EX_ILLEGAL_ARGUMENT);
    }
    let mut truncated = transaction(&[], &[reparent(task, None, true)]);
    truncated.set_i32_at(4 + 4 + 4 + 4 + 28 + 12 + 28 + 32 + 8, i32::MAX);
    assert_eq!(
        Transaction::read(&mut truncated.reader()).unwrap_err().code,
        EX_ILLEGAL_ARGUMENT
    );
}
// setAdjacentRootSet (its containers only), clearAdjacentRoots and
// setLaunchAdjacentFlagRoot / clearLaunchAdjacentFlagRoot (clearRoot in mToTop).
#[test]
fn adjacency_ops_decode() {
    let (a, b) = (Binder::Local(64), Binder::Local(72));
    let read = |ops: &[Op]| Transaction::read(&mut transaction(&[], ops).reader());
    let op = |kind, container, containers: Option<Vec<Binder>>, to_top| Op {
        kind,
        container,
        containers,
        to_top,
        ..reparent(a, None, false)
    };
    assert_eq!(
        read(&[
            op(4, None, Some(vec![a, b]), false),
            op(6, Some(b), None, false),
            op(6, Some(a), None, true),
            op(15, Some(a), None, false),
        ])
        .unwrap()
        .hierarchy,
        [
            Hop::SetAdjacentRoots(vec![a, b]),
            Hop::LaunchAdjacentFlagRoot {
                root: b,
                clear: false
            },
            Hop::LaunchAdjacentFlagRoot {
                root: a,
                clear: true
            },
            Hop::ClearAdjacentRoots(a),
        ]
    );
    // createForAdjacentRoots' container and reparent, which the server
    // does not read, and other ops' containers are not served.
    for bad in [
        op(4, Some(a), Some(vec![a, b]), false),
        Op {
            reparent: Some(b),
            ..op(4, None, Some(vec![a, b]), false)
        },
        op(4, None, Some(vec![a, b]), true),
        op(15, Some(a), None, true),
        Op {
            modes: Some(vec![6]),
            ..op(6, Some(a), None, false)
        },
        op(6, Some(a), Some(vec![a]), false),
        op(0, Some(a), Some(vec![]), false),
    ] {
        assert_eq!(read(&[bad]).unwrap_err().code, EX_UNSUPPORTED_OPERATION);
    }
    // The server dereferences the containers and the container.
    for bad in [
        op(4, None, None, false),
        op(6, None, None, false),
        op(15, None, None, false),
    ] {
        assert_eq!(read(&[bad]).unwrap_err().code, EX_ILLEGAL_ARGUMENT);
    }
}
// Field offsets of the pinned writer's Change: window bounds 244..260,
// mConfigSetMask 340, mWindowSetMask 344, mWindowingMode 352.
#[test]
fn windowing_modes_and_inherited_bounds_decode() {
    let change = |edits: &[(usize, i32)]| {
        let mut p = fixture(Binder::Local(64));
        for &(at, value) in edits {
            p.set_i32_at(at, value);
        }
        Transaction::read(&mut p.reader()).map(|t| t.changes)
    };
    let only = |bounds, mode| {
        vec![Change {
            token: Binder::Local(64),
            bounds,
            mode,
        }]
    };
    // setWindowingMode alone sets no configuration mask.
    for mode in [0, 1, 5, 6, 3, 999] {
        assert_eq!(
            change(&[(340, 0), (344, 0), (352, mode)]).unwrap(),
            only(None, Some(mode))
        );
    }
    // setBounds(null) is an empty rectangle with the bounds mask, also an inverted one.
    let null = [(244, 0), (248, 0), (252, 0), (256, 0)];
    assert_eq!(
        change(&[null.as_slice(), &[(352, 1)]].concat()).unwrap(),
        only(Some(None), Some(1))
    );
    assert_eq!(change(&[(252, -100)]).unwrap(), only(Some(None), None));
    // applyChanges ignores a mode below -1; an empty Change applies nothing.
    assert_eq!(change(&[(340, 0), (344, 0), (352, -2)]).unwrap(), []);
    // Pinned needs an owner this host does not have yet.
    assert_eq!(
        change(&[(352, 2)]).unwrap_err().code,
        EX_UNSUPPORTED_OPERATION
    );
    for masks in [[(340, 0)], [(344, 0)]] {
        assert_eq!(change(&masks).unwrap_err().code, EX_UNSUPPORTED_OPERATION);
    }
    assert_eq!(
        change(&[(244, i32::MIN), (252, i32::MAX)])
            .unwrap_err()
            .code,
        EX_ILLEGAL_ARGUMENT
    );
}
#[test]
fn truncated_lengths_unknown_ops_and_unsupported_fields_are_rejected() {
    let p = fixture(Binder::Local(64));
    for len in 0..p.data().len() {
        assert!(
            Transaction::read(&mut Reader::new(&p.data()[..len], p.objects())).is_err(),
            "length {len}"
        );
    }
    // Change length, type tags, flags/config masks, window modes, reparent,
    // arrays, all optional payloads, booleans, callback and trailing data.
    for (at, value) in [
        (0, -1),
        (4, 4),
        (36, 15),
        (40, 4),
        (40, i32::MAX),
        (336, 1),
        (340, 0),
        (344, 2),
        (348, 1),
        (352, 2),
        (356, 1),
        (364, 0),
        (368, -1),
        (368, 3),
        (368, 4),
        (368, 6),
        (368, 999),
        (400, 0),
        (404, 1),
        (408, 1),
        (412, 0),
        (440, 1),
        (444, 0), // invalid null Binder object type, rejected by Parcel::unflattenBinder
        (540, 0),
        (568, 0),
        (472, 2),
        (476, 1),
        (480, 0),
        (484, 0),
        (488, 0),
        (492, 0),
        (496, 1),
        (500, 1),
        (504, 1),
        (508, 1),
        (512, 1),
        (516, 1),
        (520, 1),
        (524, 1),
        (528, 1),
        (532, 1),
        (536, 1),
    ] {
        let mut bad = fixture(Binder::Local(64));
        bad.set_i32_at(at, value);
        assert!(
            Transaction::read(&mut bad.reader()).is_err(),
            "offset {at}, value {value}"
        );
    }
    // Native Parcel ignores the stability word of null Binders, including metadata.
    for at in [468, 564, 592] {
        let mut valid = p.clone();
        valid.set_i32_at(at, 1);
        assert!(
            Transaction::read(&mut valid.reader()).is_ok(),
            "stability {at}"
        );
    }
    for at in [540, 568] {
        // non-null error callback / fragment organizer
        let mut metadata = Parcel::new();
        metadata.write_raw(&p.data()[..at], p.objects());
        metadata.write_binder(Some(Binder::Local(64)));
        metadata.write_raw(&p.data()[at + 28..], &[]);
        assert_eq!(
            Transaction::read(&mut metadata.reader()).unwrap_err().code,
            EX_UNSUPPORTED_OPERATION
        );
    }
    assert!(Transaction::read(&mut Reader::new(p.data(), &[])).is_err());
    let mut tail = p.clone();
    tail.write_i32(0);
    assert_eq!(
        Transaction::read(&mut tail.reader()).unwrap_err().code,
        EX_ILLEGAL_ARGUMENT
    );
    let mut empty = Parcel::new();
    empty.write_i32(0);
    empty.write_i32(0);
    empty.write_binder(None);
    empty.write_binder(None);
    assert_eq!(
        Transaction::read(&mut empty.reader()).unwrap_err().code,
        EX_UNSUPPORTED_OPERATION
    );
}
