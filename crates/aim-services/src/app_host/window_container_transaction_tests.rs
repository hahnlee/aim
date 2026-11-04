use super::*;
use aim_binder_host::parcel::{EX_ILLEGAL_ARGUMENT, Parcel};

// oracle.py executes the pinned Java writers; only native transport and
// unrelated object APIs are substitutes. These are original writeMap offsets.
pub(in crate::app_host) fn fixture(token: Binder) -> Parcel {
    let hex = include_str!("../../tests/fixtures/wct/wct.hex");
    let data: Vec<u8> = hex
        .split_whitespace()
        .flat_map(|line| {
            (0..line.len())
                .step_by(2)
                .map(|at| u8::from_str_radix(&line[at..at + 2], 16).unwrap())
        })
        .collect();
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
    assert_eq!(t.reorders, [(Binder::Local(64), true)]);
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
    for mode in [0, 1, 5, 3, 999] {
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
    // Pinned and multi-window need owners this host does not have yet.
    for mode in [2, 6] {
        assert_eq!(
            change(&[(352, mode)]).unwrap_err().code,
            EX_UNSUPPORTED_OPERATION
        );
    }
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
        (352, 6),
        (356, 1),
        (364, 0),
        (368, 0),
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
