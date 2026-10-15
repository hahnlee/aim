//! Independent reads of system_server's live owners, sent ahead in one
//! `transactBatch`: a pass names the calls it will make, the bridge runs each
//! as its own call, and the pass's reads take those replies instead of a round
//! trip each.
use super::{Bridge, bridge};
use aim_binder_host::{
    local::Received,
    parcel::{Parcel, Reader, StatusCode},
};
use std::{cell::RefCell, collections::BTreeMap};

type Calls = BTreeMap<(u32, Vec<u8>), Vec<u8>>;

thread_local! {
    static AHEAD: RefCell<Calls> = RefCell::new(BTreeMap::new());
}

pub enum Reply {
    Live(Received),
    Ahead(Vec<u8>),
}

impl Reply {
    pub fn reader(&self) -> Reader<'_> {
        match self {
            Self::Live(reply) => reply.reader(),
            Self::Ahead(bytes) => Reader::new(bytes, &[]),
        }
    }
}

/// Restores the enclosing pass's replies, also on unwind.
struct Scope(Calls);
impl Drop for Scope {
    fn drop(&mut self) {
        AHEAD.with(|ahead| *ahead.borrow_mut() = std::mem::take(&mut self.0));
    }
}

impl Bridge {
    /// A call answered from this thread's pass, else made now.
    pub(crate) fn read(&self, code: u32, request: &Parcel) -> Result<Reply, StatusCode> {
        let key = (code, request.data().to_vec());
        match AHEAD.with(|ahead| ahead.borrow().get(&key).cloned()) {
            Some(bytes) => Ok(Reply::Ahead(bytes)),
            None => self.owner.transact(code, request, false).map(Reply::Live),
        }
    }

    /// Runs `pass` with the replies to `calls` read in one transaction. A
    /// batch that fails leaves each call to be made, and to fail, on its own.
    pub fn read_ahead<T>(&self, calls: Vec<(u32, Parcel)>, pass: impl FnOnce() -> T) -> T {
        let calls: Vec<_> = calls
            .into_iter()
            .map(|(code, request)| (code, request.data().to_vec()))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut ahead = AHEAD.with(|ahead| ahead.borrow().clone());
        if let Ok(replies) = self.transact_batch(&calls) {
            ahead.extend(calls.into_iter().zip(replies));
        }
        let _scope = Scope(AHEAD.with(|current| std::mem::replace(&mut *current.borrow_mut(), ahead)));
        pass()
    }

    fn transact_batch(&self, calls: &[(u32, Vec<u8>)]) -> Result<Vec<Vec<u8>>, StatusCode> {
        let mut batch = Parcel::new();
        batch.write_i32(i32::try_from(calls.len()).map_err(|_| aim_binder_host::parcel::BAD_VALUE)?);
        for (code, request) in calls {
            batch.write_i32(*code as i32);
            aim_service_aidl::write_byte_array(&mut batch, Some(request));
        }
        let mut request = Parcel::new();
        bridge::TransactBatch { calls: Some(batch.data().to_vec()) }.write(&mut request);
        let reply = self.owner.transact(bridge::TRANSACT_BATCH, &request, false)?;
        let mut reader = reply.reader();
        let bytes = bridge::read_transact_batch_reply(&mut reader)?
            .map_err(|_| aim_binder_host::parcel::BAD_VALUE)?
            .ok_or(aim_binder_host::parcel::BAD_VALUE)?;
        let mut replies = Reader::new(&bytes, &[]);
        if reader.remaining() != 0 || replies.read_i32()? != calls.len() as i32 {
            return Err(aim_binder_host::parcel::BAD_VALUE);
        }
        let replies = calls
            .iter()
            .map(|_| aim_service_aidl::read_byte_array(&mut replies)?.ok_or(aim_binder_host::parcel::BAD_VALUE))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(replies)
    }
}
