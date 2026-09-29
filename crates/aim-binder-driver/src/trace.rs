//! Transaction tracing: which process calls which interface and method,
//! and where a synchronous call's time goes in the driver
//! (`guest-init --binder-trace`, docs/system-services.md).
//!
//! A synchronous call is timed at four points: the driver taking the
//! sender's `BC_TRANSACTION`, a target thread's read taking it
//! (`BR_TRANSACTION`), the driver taking the target's `BC_REPLY`, and the
//! sender's read taking the `BR_REPLY`. The first span is the wake of a
//! waiting target thread, or the wait for one when none was free; the
//! second is the target's work with its hops out of and back into the
//! driver; the third is the sender's wake. The sender's own hops into and
//! out of the driver are not part of it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::state::TxnId;

/// One transaction, as the driver saw it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TraceRecord {
    /// When the call was sent, since tracing started.
    pub at: Duration,
    /// The binder device (`/dev/binder`, ...).
    pub device: String,
    pub from_pid: i32,
    pub from_euid: u32,
    pub to_pid: i32,
    /// The interface token at the start of the data (AIDL's descriptor, or
    /// HIDL's interface name); empty when there is none (`PING`, ...).
    pub descriptor: String,
    pub code: u32,
    pub oneway: bool,
    /// The target's threads waiting for process work when it was sent.
    pub waiting: u32,
    /// The target's looper threads (registered or entered) then.
    pub loopers: u32,
    /// Until a target thread's read took it; `None` for one-way calls and
    /// calls never delivered.
    pub delivered: Option<Duration>,
    /// Until the target's reply; `None` for one-way calls and calls that
    /// got no reply (dead target, failed reply).
    pub latency: Option<Duration>,
    /// Until the sender's read took the reply.
    pub returned: Option<Duration>,
}

#[derive(Default)]
pub(crate) struct Trace {
    start: Option<Instant>,
    /// Sent and not yet replied to, by transaction.
    pending: HashMap<TxnId, (Instant, TraceRecord)>,
    /// Replied to and not yet read by the sender, by reply.
    returning: HashMap<TxnId, (Instant, TraceRecord)>,
    done: Vec<TraceRecord>,
}

impl Trace {
    pub fn new() -> Self {
        Self {
            start: Some(Instant::now()),
            ..Self::default()
        }
    }

    /// A transaction was sent.
    pub fn sent(&mut self, id: TxnId, mut record: TraceRecord) {
        let now = Instant::now();
        record.at = now - self.start.unwrap_or(now);
        if record.oneway {
            self.done.push(record);
        } else {
            self.pending.insert(id, (now, record));
        }
    }

    /// A target thread's read took transaction `id`.
    pub fn delivered(&mut self, id: TxnId) {
        if let Some((sent, record)) = self.pending.get_mut(&id) {
            record.delivered = Some(sent.elapsed());
        }
    }

    /// Transaction `id` was replied to with `reply`.
    pub fn replied(&mut self, id: TxnId, reply: TxnId) {
        if let Some((sent, mut record)) = self.pending.remove(&id) {
            record.latency = Some(sent.elapsed());
            self.returning.insert(reply, (sent, record));
        }
    }

    /// The sender's read took `reply`.
    pub fn returned(&mut self, reply: TxnId) {
        if let Some((sent, mut record)) = self.returning.remove(&reply) {
            record.returned = Some(sent.elapsed());
            self.done.push(record);
        }
    }

    /// Transaction or reply `id` is gone.
    pub fn dropped(&mut self, id: TxnId) {
        if let Some((_, record)) = self.pending.remove(&id).or(self.returning.remove(&id)) {
            self.done.push(record);
        }
    }

    pub fn take(&mut self) -> Vec<TraceRecord> {
        std::mem::take(&mut self.done)
    }
}

/// libbinder's interface token headers (`Parcel::kHeader`).
const HEADERS: [&[u8; 4]; 3] = [b"SYST", b"VNDR", b"RECO"];

/// The interface token at the start of a transaction's data: libbinder's
/// strict-mode policy, work source uid, header and UTF-16 descriptor, or
/// hwbinder's NUL-terminated interface name.
pub(crate) fn descriptor(data: &[u8]) -> String {
    let word = |at: usize| data.get(at..at + 4).map(|w| [w[0], w[1], w[2], w[3]]);
    if let Some(header) = word(8)
        && HEADERS.iter().any(|h| header == [h[3], h[2], h[1], h[0]])
        && let Some(len) = word(12).map(i32::from_le_bytes)
        && len > 0
    {
        let units: Vec<u16> = data[16..]
            .chunks_exact(2)
            .take(len as usize)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        if units.len() == len as usize {
            return String::from_utf16_lossy(&units);
        }
    }
    let name = data.split(|b| *b == 0).next().unwrap_or_default();
    if name.contains(&b'@') && name.iter().all(|b| b.is_ascii_graphic()) {
        return String::from_utf8_lossy(name).into_owned();
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(name: &str) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&0x4200_0004i32.to_le_bytes());
        data.extend_from_slice(&(-1i32).to_le_bytes());
        data.extend_from_slice(&u32::from_be_bytes(*b"SYST").to_le_bytes());
        let units: Vec<u16> = name.encode_utf16().collect();
        data.extend_from_slice(&(units.len() as i32).to_le_bytes());
        for u in units.iter().chain([&0u16]) {
            data.extend_from_slice(&u.to_le_bytes());
        }
        data
    }

    #[test]
    fn splits_a_call() {
        let mut trace = Trace::new();
        let record = TraceRecord {
            code: 1,
            ..TraceRecord::default()
        };
        trace.sent(1, record.clone());
        trace.delivered(1);
        trace.replied(1, 2);
        assert!(trace.take().is_empty());
        trace.returned(2);
        let [r] = &trace.take()[..] else { panic!() };
        let (d, l, b) = (r.delivered.unwrap(), r.latency.unwrap(), r.returned.unwrap());
        assert!(d <= l && l <= b);

        trace.sent(3, record.clone());
        trace.dropped(3);
        let [r] = &trace.take()[..] else { panic!() };
        assert_eq!((r.delivered, r.latency), (None, None));

        trace.sent(4, record);
        trace.replied(4, 5);
        trace.dropped(5);
        let [r] = &trace.take()[..] else { panic!() };
        assert!(r.latency.is_some() && r.returned.is_none());
    }

    #[test]
    fn reads_aidl_and_hidl_tokens() {
        assert_eq!(
            descriptor(&token("android.content.IClipboard")),
            "android.content.IClipboard"
        );
        assert_eq!(
            descriptor(b"android.hidl.manager@1.0::IServiceManager\0\0\0"),
            "android.hidl.manager@1.0::IServiceManager"
        );
        assert_eq!(descriptor(&[0; 16]), "");
        assert_eq!(descriptor(&token("android.os.IFoo")[..20]), "");
    }
}
