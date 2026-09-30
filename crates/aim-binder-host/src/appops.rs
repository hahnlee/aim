//! App ops noted for a caller while its call is served, sent back ahead of
//! the reply's status, as a Java service of the pinned framework does:
//! `Binder.execTransactInternal` collects for a two-way call that carries
//! `FLAG_COLLECT_NOTED_APP_OPS` (the caller listens for its noted ops),
//! `AppOpsManager.collectNotedOpSync` records an op noted for that caller,
//! and `Parcel.writeNoException` / `writeException` prefix the reply with
//! them (`prefixParcelWithAppOpsIfNeeded`), for the caller's
//! `AppOpsManager.readAndLogNotedAppops`.

use std::cell::RefCell;

use crate::parcel::Parcel;

/// `IBinder.FLAG_COLLECT_NOTED_APP_OPS`.
pub const FLAG_COLLECT_NOTED_APP_OPS: u32 = 0x2;
/// `AppOpsManager._NUM_OP` of the pinned framework.
pub const NUM_OP: usize = 163;
/// `AppOpsManager.BITMASK_LEN`: the words of an attribution's op mask.
const BITMASK_LEN: usize = (NUM_OP - 1) / 64 + 1;
/// `Parcel.EX_HAS_NOTED_APPOPS_REPLY_HEADER`.
pub(crate) const EX_HAS_NOTED_APPOPS_REPLY_HEADER: i32 = -127;

/// The collection of the call this thread serves.
struct Collection {
    /// `sBinderThreadCallingUid`.
    uid: u32,
    /// `sAppOpsNotedInThisBinderTransaction`: op masks by attribution tag.
    noted: Vec<(Option<String>, [u64; BITMASK_LEN])>,
}

thread_local! {
    static COLLECTION: RefCell<Option<Collection>> = const { RefCell::new(None) };
}

/// Serves a call with ops collected for `uid`, or none collected; the
/// collection of a call this thread serves around it (a nested call) is
/// set aside meanwhile, as `pauseNotedAppOpsCollection` sets it aside for
/// the outgoing call it nests in.
pub(crate) fn collecting<R>(uid: Option<u32>, serve: impl FnOnce() -> R) -> R {
    let outer = COLLECTION.replace(uid.map(|uid| Collection {
        uid,
        noted: Vec::new(),
    }));
    let result = serve();
    COLLECTION.set(outer);
    result
}

/// The uid ops are collected for in the call this thread serves
/// (`sBinderThreadCallingUid`).
pub fn collecting_uid() -> Option<u32> {
    COLLECTION.with_borrow(|c| c.as_ref().map(|c| c.uid))
}

/// `AppOpsManager.collectNotedOpSync`: `op` noted for the collecting
/// caller, under `attribution_tag`, to send back with the reply.
pub fn collect_sync(op: i32, attribution_tag: Option<&str>) {
    COLLECTION.with_borrow_mut(|c| {
        let Some(c) = c else { return };
        let at = c
            .noted
            .iter()
            .position(|(t, _)| t.as_deref() == attribution_tag);
        let at = at.unwrap_or_else(|| {
            c.noted
                .push((attribution_tag.map(Into::into), [0; BITMASK_LEN]));
            c.noted.len() - 1
        });
        let op = op as usize;
        c.noted[at].1[op / 64] |= 1 << (op % 64);
    });
}

/// `AppOpsManager.prefixParcelWithAppOpsIfNeeded`: the header, sized from
/// its size field on, ahead of a reply's status when ops were collected.
pub(crate) fn prefix_reply(p: &mut Parcel) {
    COLLECTION.with_borrow(|c| {
        let Some(c) = c.as_ref().filter(|c| !c.noted.is_empty()) else {
            return;
        };
        p.write_i32(EX_HAS_NOTED_APPOPS_REPLY_HEADER);
        let size_at = p.position();
        p.write_i32(0);
        p.write_i32(c.noted.len() as i32);
        for (tag, mask) in &c.noted {
            p.write_string16(tag.as_deref());
            for &word in mask {
                p.write_i64(word as i64);
            }
        }
        let size = p.position() - size_at;
        p.set_i32_at(size_at, size as i32);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parcel::{Exception, Reader};

    /// The header as `readAndLogNotedAppops` reads it.
    fn read_header(r: &mut Reader<'_>) -> Vec<(Option<String>, Vec<u64>)> {
        assert_eq!(r.read_i32(), Ok(EX_HAS_NOTED_APPOPS_REPLY_HEADER));
        let start = r.position();
        let size = r.read_i32().unwrap() as usize;
        let noted = (0..r.read_i32().unwrap())
            .map(|_| {
                let tag = r.read_string16().unwrap();
                let mask = (0..BITMASK_LEN)
                    .map(|_| r.read_i64().unwrap() as u64)
                    .collect();
                (tag, mask)
            })
            .collect();
        assert_eq!(r.position() - start, size);
        noted
    }

    #[test]
    fn collected_ops_prefix_the_reply() {
        let reply = collecting(Some(10_123), || {
            assert_eq!(collecting_uid(), Some(10_123));
            collect_sync(1, Some("tag"));
            collect_sync(0, Some("tag"));
            collect_sync(130, None);
            collect_sync(1, Some("tag"));
            let mut p = Parcel::new();
            p.write_no_exception();
            p.write_i32(7);
            p
        });
        assert_eq!(collecting_uid(), None);
        let mut r = Reader::new(reply.data(), &[]);
        assert_eq!(
            read_header(&mut r),
            [
                (Some("tag".into()), vec![0b11, 0, 0]),
                (None, vec![0, 0, 1 << 2]),
            ]
        );
        assert_eq!(r.read_i32(), Ok(0));
        assert_eq!(r.read_i32(), Ok(7));
        // A reader of the status skips the header by its size.
        let mut r = Reader::new(reply.data(), &[]);
        assert_eq!(r.read_exception(), Ok(Ok(())));
        assert_eq!(r.read_i32(), Ok(7));
    }

    #[test]
    fn exceptions_carry_the_header() {
        let reply = collecting(Some(10_123), || {
            collect_sync(0, None);
            let mut p = Parcel::new();
            p.write_exception(&Exception::security("no"));
            p
        });
        let mut r = Reader::new(reply.data(), &[]);
        assert_eq!(r.read_exception(), Ok(Err(Exception::security("no"))));
    }

    #[test]
    fn nothing_noted_or_collected_writes_no_header() {
        for uid in [Some(10_123), None] {
            let reply = collecting(uid, || {
                if uid.is_none() {
                    collect_sync(0, None);
                }
                let mut p = Parcel::new();
                p.write_no_exception();
                p
            });
            assert_eq!(reply.data(), 0i32.to_le_bytes());
        }
    }

    #[test]
    fn a_nested_call_sets_the_outer_collection_aside() {
        collecting(Some(10_123), || {
            collect_sync(3, None);
            let nested = collecting(None, || {
                assert_eq!(collecting_uid(), None);
                let mut p = Parcel::new();
                p.write_no_exception();
                p
            });
            assert_eq!(nested.data(), 0i32.to_le_bytes());
            assert_eq!(collecting_uid(), Some(10_123));
            let mut p = Parcel::new();
            p.write_no_exception();
            let mut r = Reader::new(p.data(), &[]);
            assert_eq!(read_header(&mut r), [(None, vec![1 << 3, 0, 0])]);
        });
    }
}
