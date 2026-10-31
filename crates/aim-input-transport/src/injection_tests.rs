//! InputEventTest parcel round trips and pinned InputDispatcher normalization.
use super::*;
fn words(p: &mut Vec<u8>, values: &[i32]) {
    p.extend(values.iter().flat_map(|v| v.to_le_bytes()));
}
fn decode(p: &[u8]) -> Event {
    let (e, n) = Event::decode(p).unwrap();
    assert_eq!(n, p.len());
    assert_eq!(e.parcel, p);
    for n in 0..p.len() {
        assert!(Event::decode(&p[..n]).is_none(), "truncated at {n}");
    }
    e
}
#[test]
fn key_parcel_characters_fields_and_invalid_action() {
    for action in [0, 1, 2] {
        let mut p = Vec::new();
        words(&mut p, &[2, 123, 7, 0x101, -1, 32]);
        p.extend([9; 32]);
        words(&mut p, &[action, 29, 3, 0x1000, 30, 8]);
        p.extend(10i64.to_le_bytes());
        p.extend(20i64.to_le_bytes());
        words(&mut p, &[2]); // UTF-16, including a non-ASCII character and terminator.
        p.extend([0x41, 0, 0, 0xac, 0, 0, 0, 0]);
        let e = decode(&p);
        assert_eq!(e.valid, action != 2);
        assert!(!e.motion());
        let m = e.message(0, 42, (100.0, 200.0), None);
        for (at, v) in [
            (4, 42),
            (8, 123),
            (16, 20),
            (24, -1),
            (28, 0x101),
            (32, -1),
            (68, action),
            (72, 8),
            (76, 29),
            (80, 30),
            (84, 0x1000),
            (88, 3),
            (96, 10),
        ] {
            assert_eq!(int(&m, at), v);
        }
        assert_eq!(&m[36..68], &[0; 32]);
    }
}
#[test]
fn motion_parcel_history_sparse_axes_and_transforms() {
    let mut p = Vec::new();
    words(&mut p, &[1, 1, 2, 123, 7, 0x1002, -1, 32]);
    p.extend([9; 32]);
    words(&mut p, &[2, 0, 0, 4, 8, 16, 2]);
    for v in [
        2.0f32,
        0.0,
        100.0,
        0.0,
        3.0,
        200.0,
        0.5,
        0.25,
        f32::NAN,
        f32::NAN,
        9.0,
        0.0,
        900.0,
        0.0,
        9.0,
        900.0,
    ] {
        p.extend(v.to_le_bytes());
    }
    p.extend(10i64.to_le_bytes());
    words(&mut p, &[3, 2]);
    for time in [10i64, 20] {
        p.extend(time.to_le_bytes());
        p.extend(0xc000_0000_0000_0001u64.to_le_bytes());
        for v in [5.0f32, 6.0, 42.0] {
            p.extend(v.to_le_bytes());
        }
        words(&mut p, &[1]);
    }
    let e = decode(&p);
    assert!(e.valid && e.pointer());
    assert_eq!(e.display(), 0);
    assert_eq!(e.position(), (110.0, 218.0));
    for (sample, time) in [(0, 10), (1, 20)] {
        let m = e.message(sample, 77, (100.0, 200.0), None);
        for (at, v) in [(16, time), (24, -1), (32, 0), (168, 3), (172, 2)] {
            assert_eq!(int(&m, at), v);
        }
        for (at, v) in [
            (120, -100.0),
            (124, -200.0),
            (184, 110.0),
            (188, 218.0),
            (192, 42.0),
        ] {
            assert_eq!(f32::from_bits(int(&m, at) as u32), v);
        }
        assert_eq!(m[304], 1);
    }
    // Non-pointer position sources disregard both logical and window transforms.
    p[20..24].copy_from_slice(&0x100008i32.to_le_bytes());
    let e = decode(&p);
    assert!(!e.pointer());
    assert_eq!(e.display(), -1);
    assert_eq!(e.position(), (5.0, 6.0));
    let mut invalid = p;
    invalid[8..12].copy_from_slice(&0i32.to_le_bytes());
    assert!(Event::decode(&invalid).is_none());
}
