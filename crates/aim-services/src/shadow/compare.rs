//! One call compared: the identities of its binders and fds, the
//! original's reply with a `ParceledListSlice`'s chunks stitched in, both
//! replies decoded the same way, and the call's log line.
//!
//! **Identities.** Before anything reads them, the call and both replies
//! are rewritten so that each binder object is a handle and each fd
//! object an fd whose number indexes the call's identity table: a node of
//! the side that answers (the process that replied, or the model's
//! `Binder::Local`) is that side's own object, and all own objects are
//! one identity (an installer, a key set's token: what they stand for is
//! the service's object); another node is itself; a file is its device
//! and inode, and a file not identified equals nothing.
//!
//! **Stitching.** `BaseParceledListSlice.writeToParcel` writes as many
//! items as fit, then `0` and a binder the reader fetches the rest
//! through (`FIRST_CALL_TRANSACTION` with the next index; each reply is
//! `writeNoException`, items, and `0` while more remain). A reply that
//! ends in `0` and a binder of its own is held until its fetches stop
//! (one that does not end in `0` is the last, else [`STITCH_WAIT`] after
//! the latest); the fetches' items then replace the `0` and the binder,
//! as if all had fit.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aim_binder_driver::uapi::{
    BINDER_TYPE_BINDER, BINDER_TYPE_FD, BINDER_TYPE_HANDLE, BINDER_TYPE_WEAK_BINDER,
    BINDER_TYPE_WEAK_HANDLE, FLAT_BINDER_OBJECT_SIZE, FlatBinderObject, TF_ONE_WAY, TF_STATUS_CODE,
};
use aim_binder_driver::{FileId, ShadowCopy, ShadowKind, ShadowParcel, ShadowReply};
use aim_binder_host::parcel::{Parcel, Reader, Result as ParcelResult};

use super::value::{IntoValue, Value, json_string};
use super::{Answer, ShadowCall, ShadowModel};

/// How long a list's fetches may pause before it counts as complete.
pub(crate) const STITCH_WAIT: Duration = Duration::from_secs(1);
/// `IBinder.FIRST_CALL_TRANSACTION`, a slice's fetch.
const FIRST_CALL_TRANSACTION: u32 = 1;
/// A binder object and the stability libbinder writes after it.
const BINDER_WITH_STABILITY: usize = FLAT_BINDER_OBJECT_SIZE + 4;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Identity {
    Own,
    Node { node: u64, owner_pid: i32 },
    File(FileId),
    UnknownFile,
}

#[derive(Default)]
struct Identities(Vec<Identity>);

impl Identities {
    fn index(&mut self, id: Identity) -> u32 {
        let known = id != Identity::UnknownFile;
        match self.0.iter().position(|i| known && *i == id) {
            Some(i) => i as u32,
            None => {
                self.0.push(id);
                self.0.len() as u32 - 1
            }
        }
    }

    fn json(&self, out: &mut String) {
        out.push('[');
        for (i, id) in self.0.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str(&match id {
                Identity::Own => "{\"own\":true}".to_string(),
                Identity::Node { node, owner_pid } => {
                    format!("{{\"node\":{node},\"pid\":{owner_pid}}}")
                }
                Identity::File((dev, ino)) => format!("{{\"file\":[{dev},{ino}]}}"),
                Identity::UnknownFile => "{\"file\":null}".to_string(),
            });
        }
        out.push(']');
    }
}

/// A parcel rewritten for reading: data and object offsets.
struct Normal {
    data: Vec<u8>,
    objects: Vec<u64>,
}

impl Normal {
    fn reader(&self) -> Reader<'_> {
        Reader::new(&self.data, &self.objects)
    }
}

fn put_object(data: &mut [u8], at: usize, kind: u32, index: u32) {
    let object = FlatBinderObject {
        kind,
        flags: 0,
        binder: index as u64,
        cookie: 0,
    };
    data[at..at + FLAT_BINDER_OBJECT_SIZE].copy_from_slice(&object.encode());
}

/// A copied parcel; binders of the process `own_pid` are its own.
fn normalize_copy(parcel: &ShadowParcel, own_pid: Option<i32>, ids: &mut Identities) -> Normal {
    let mut data = parcel.data.clone();
    let mut objects = Vec::new();
    for object in &parcel.objects {
        let at = object.offset;
        match object.kind {
            ShadowKind::Binder { node, owner_pid } => {
                let id = if Some(owner_pid) == own_pid {
                    Identity::Own
                } else {
                    Identity::Node { node, owner_pid }
                };
                put_object(&mut data, at, BINDER_TYPE_HANDLE, ids.index(id));
            }
            ShadowKind::File(file) => {
                let id = file.map_or(Identity::UnknownFile, Identity::File);
                put_object(&mut data, at, BINDER_TYPE_FD, ids.index(id));
            }
            ShadowKind::Other => {}
        }
        objects.push(at as u64);
    }
    Normal { data, objects }
}

/// A model's reply: its own binders are its own, its handles the call's.
fn normalize_model(parcel: &Parcel, ids: &mut Identities) -> Normal {
    let mut data = parcel.data().to_vec();
    for &at in parcel.objects() {
        let at = at as usize;
        let object = FlatBinderObject::decode(&data[at..at + FLAT_BINDER_OBJECT_SIZE]);
        match object.kind {
            BINDER_TYPE_BINDER | BINDER_TYPE_WEAK_BINDER => {
                put_object(&mut data, at, BINDER_TYPE_HANDLE, ids.index(Identity::Own))
            }
            BINDER_TYPE_HANDLE | BINDER_TYPE_WEAK_HANDLE => {
                put_object(&mut data, at, BINDER_TYPE_HANDLE, object.handle())
            }
            BINDER_TYPE_FD => {
                let file = parcel
                    .files()
                    .iter()
                    .find(|(o, _)| *o as usize == at)
                    .and_then(|(_, f)| super::identify(f));
                let id = file.map_or(Identity::UnknownFile, Identity::File);
                put_object(&mut data, at, BINDER_TYPE_FD, ids.index(id));
            }
            _ => {}
        }
    }
    Normal {
        data,
        objects: parcel.objects().to_vec(),
    }
}

/// The interface token at the start of a call (`enforceInterface`).
fn descriptor(data: &[u8]) -> String {
    let mut r = Reader::new(data, &[]);
    let token = (|| {
        r.read_i32()?;
        r.read_i32()?;
        r.read_i32()?;
        r.read_string16()
    })();
    token.ok().flatten().unwrap_or_default()
}

/// Where a reply's slice binder starts, when it ends in `0` and a binder
/// of the process that replied.
fn slice_binder(parcel: &ShadowParcel, replier: i32) -> Option<usize> {
    let last = parcel.objects.last()?;
    let own = matches!(last.kind, ShadowKind::Binder { owner_pid, .. } if owner_pid == replier);
    let at = last.offset;
    let zero = at
        .checked_sub(4)
        .and_then(|z| parcel.data.get(z..at))
        .is_some_and(|w| w == [0; 4]);
    (own && zero && at + BINDER_WITH_STABILITY == parcel.data.len()).then_some(at)
}

/// The reply with the chunks' items in place of its `0` and binder at
/// `at`: every chunk but the last ends in `0`, which goes too.
fn stitch(reply: &ShadowParcel, at: usize, chunks: &[ShadowParcel]) -> Option<ShadowParcel> {
    let mut out = ShadowParcel {
        data: reply.data[..at - 4].to_vec(),
        objects: reply
            .objects
            .iter()
            .filter(|o| o.offset < at)
            .cloned()
            .collect(),
    };
    for (i, chunk) in chunks.iter().enumerate() {
        let mut r = Reader::new(&chunk.data, &[]);
        r.read_exception().ok()?.ok()?;
        let start = r.position();
        let end = if i + 1 < chunks.len() {
            chunk.data.len().checked_sub(4)?
        } else {
            chunk.data.len()
        };
        let base = out.data.len();
        out.data.extend_from_slice(chunk.data.get(start..end)?);
        out.objects.extend(
            chunk
                .objects
                .iter()
                .filter(|o| o.offset >= start && o.offset < end)
                .map(|o| aim_binder_driver::ShadowObject {
                    offset: o.offset - start + base,
                    kind: o.kind.clone(),
                }),
        );
    }
    Some(out)
}

/// The exception, then the rest of the reply's bytes.
fn raw(r: &mut Reader<'_>) -> ParcelResult<Value> {
    let exception = match r.read_exception()? {
        Ok(()) => Value::Null,
        Err(e) => e.into_value(),
    };
    let at = r.position();
    r.skip(r.remaining())?;
    Ok(Value::Fields(vec![
        ("exception".into(), exception),
        ("rest".into(), Value::Bytes(r.since(at).0.to_vec())),
    ]))
}

/// A call answered by its model, waiting for the comparison.
struct Exchange {
    copy: ShadowCopy,
    service: String,
    descriptor: String,
    model: Arc<dyn ShadowModel>,
    answer: Answer,
    ids: Identities,
    /// The slice binder's offset, while its chunks come.
    slice: Option<usize>,
    chunks: Vec<ShadowParcel>,
    /// A fetch got no reply: the list cannot be completed.
    broken: bool,
    deadline: Instant,
}

impl Exchange {
    /// The call's log line without its closing brace.
    fn line(&self, outcome: &str) -> String {
        let mut line = format!("{{\"seq\":{},\"service\":", self.copy.seq);
        json_string(&self.service, &mut line);
        line.push_str(",\"descriptor\":");
        json_string(&self.descriptor, &mut line);
        line.push_str(&format!(
            ",\"code\":{},\"from_pid\":{},\"from_euid\":{},\"outcome\":\"{outcome}\"",
            self.copy.code, self.copy.from_pid, self.copy.from_euid
        ));
        if !self.chunks.is_empty() {
            line.push_str(&format!(",\"chunks\":{}", self.chunks.len()));
        }
        line
    }

    fn finish(&self, outcome: &str) -> String {
        self.line(outcome) + "}"
    }

    /// Decodes a reply as the model decodes this method, else raw.
    fn decode(&self, reply: &Normal) -> ParcelResult<Value> {
        let decoded =
            self.model
                .decode_reply(&self.descriptor, self.copy.code, &mut reply.reader());
        decoded.unwrap_or_else(|| raw(&mut reply.reader()))
    }

    /// Compares the replies: the call's log line.
    fn compare(mut self) -> String {
        let ShadowReply::Reply { flags, parcel } = &self.copy.reply else {
            unreachable!("only replies are compared")
        };
        if self.broken {
            return self.finish("incomplete");
        }
        let original = if flags & TF_STATUS_CODE != 0 {
            let word = parcel.data.get(..4).map(|w| w.try_into().unwrap());
            Ok(Value::Status(word.map_or(0, i32::from_le_bytes)))
        } else {
            let stitched = match self.slice {
                Some(at) if !self.chunks.is_empty() => stitch(parcel, at, &self.chunks),
                _ => Some(parcel.clone()),
            };
            let Some(stitched) = stitched else {
                return self.finish("incomplete");
            };
            let normal = normalize_copy(&stitched, Some(self.copy.to_pid), &mut self.ids);
            self.decode(&normal).map_err(|s| (s, normal))
        };
        let model = match &self.answer {
            Answer::Reply(reply) => {
                let normal = normalize_model(reply, &mut self.ids);
                self.decode(&normal).map_err(|s| (s, normal))
            }
            Answer::Status(s) => Ok(Value::Status(*s)),
            Answer::NotModelled => unreachable!("compared only when answered"),
        };
        let (outcome, original, model) = match (original, model) {
            (Ok(a), Ok(b)) if a == b => return self.finish("matched"),
            (Ok(a), Ok(b)) => ("differed", a, b),
            (a, b) => {
                let shown = |r: Result<Value, (i32, Normal)>| match r {
                    Ok(v) => v,
                    Err((status, normal)) => Value::Fields(vec![
                        ("undecodable".into(), Value::Status(status)),
                        ("data".into(), Value::Bytes(normal.data)),
                    ]),
                };
                ("undecodable", shown(a), shown(b))
            }
        };
        let mut line = self.line(outcome);
        line.push_str(",\"original\":");
        original.json(&mut line);
        line.push_str(",\"model\":");
        model.json(&mut line);
        line.push_str(",\"identities\":");
        self.ids.json(&mut line);
        line.push('}');
        line
    }
}

/// The comparison's state: models, the calls waiting for their lists,
/// the drops seen.
pub(crate) struct Comparator {
    models: HashMap<String, Arc<dyn ShadowModel>>,
    roots: Arc<Mutex<HashMap<u64, String>>>,
    waiting: Vec<Exchange>,
    dropped: Arc<AtomicU64>,
    dropped_logged: u64,
}

impl Comparator {
    pub fn new(
        models: HashMap<String, Arc<dyn ShadowModel>>,
        roots: Arc<Mutex<HashMap<u64, String>>>,
        dropped: Arc<AtomicU64>,
    ) -> Self {
        Self {
            models,
            roots,
            waiting: Vec::new(),
            dropped,
            dropped_logged: 0,
        }
    }

    /// When the next waiting list completes.
    pub fn deadline(&self) -> Option<Instant> {
        self.waiting.iter().map(|e| e.deadline).min()
    }

    /// Takes a copy (or none, at a deadline): the log lines it completes.
    pub fn step(&mut self, copy: Option<ShadowCopy>, now: Instant) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(copy) = copy {
            self.take(copy, now, &mut lines);
        }
        let (done, waiting) = std::mem::take(&mut self.waiting)
            .into_iter()
            .partition(|e| e.deadline <= now);
        self.waiting = waiting;
        lines.extend(done.into_iter().map(Exchange::compare));
        lines.extend(self.checks());
        let dropped = self.dropped.load(Ordering::Relaxed);
        if dropped != self.dropped_logged {
            self.dropped_logged = dropped;
            lines.push(format!("{{\"event\":\"dropped\",\"total\":{dropped}}}"));
        }
        lines
    }

    /// The models' checks that are due, each model asked once.
    fn checks(&self) -> Vec<String> {
        let mut asked: Vec<*const ()> = Vec::new();
        let mut lines = Vec::new();
        for model in self.models.values() {
            let id = Arc::as_ptr(model) as *const ();
            if !asked.contains(&id) {
                asked.push(id);
                lines.extend(model.checks().iter().map(super::Check::line));
            }
        }
        lines
    }

    fn take(&mut self, copy: ShadowCopy, now: Instant, lines: &mut Vec<String>) {
        let fetch = copy.code == FIRST_CALL_TRANSACTION && copy.data.data.len() == 4;
        if let Some(parent) = copy.follows
            && fetch
        {
            // A slice's fetch: never a call of its own.
            if let Some(i) = self
                .waiting
                .iter()
                .position(|e| e.copy.seq == parent && e.slice.is_some())
            {
                let exchange = &mut self.waiting[i];
                exchange.deadline = now + STITCH_WAIT;
                match copy.reply {
                    ShadowReply::Reply { flags, parcel } if flags & TF_STATUS_CODE == 0 => {
                        let last = !parcel.data.ends_with(&[0; 4]);
                        exchange.chunks.push(parcel);
                        if last {
                            lines.push(self.waiting.remove(i).compare());
                        }
                    }
                    _ => {
                        exchange.broken = true;
                        lines.push(self.waiting.remove(i).compare());
                    }
                }
            }
            return;
        }
        let service = self
            .roots
            .lock()
            .unwrap()
            .get(&copy.root)
            .cloned()
            .unwrap_or_default();
        let model = self
            .models
            .get(&service)
            .cloned()
            .unwrap_or_else(|| Arc::new(super::Unmodelled));
        let descriptor = descriptor(&copy.data.data);
        let mut ids = Identities::default();
        let call = normalize_copy(&copy.data, None, &mut ids);
        let answer = model.answer(&mut ShadowCall {
            service: &service,
            descriptor: &descriptor,
            code: copy.code,
            flags: copy.flags,
            sender_pid: copy.from_pid,
            sender_euid: copy.from_euid,
            seq: copy.seq,
            sent: copy.sent,
            dropped: self.dropped.load(Ordering::Relaxed),
            data: call.reader(),
        });
        let slice = match &copy.reply {
            ShadowReply::Reply { flags, parcel } if flags & TF_STATUS_CODE == 0 => {
                slice_binder(parcel, copy.to_pid)
            }
            _ => None,
        };
        let exchange = Exchange {
            copy,
            service,
            descriptor,
            model,
            answer,
            ids,
            slice,
            chunks: Vec::new(),
            broken: false,
            deadline: now + STITCH_WAIT,
        };
        if matches!(exchange.answer, Answer::NotModelled) {
            lines.push(exchange.finish("not_modelled"));
        } else if exchange.copy.flags & TF_ONE_WAY != 0 {
            lines.push(exchange.finish("oneway"));
        } else if exchange.copy.reply == ShadowReply::Failed {
            lines.push(exchange.finish("failed"));
        } else if exchange.slice.is_some() {
            self.waiting.push(exchange);
        } else {
            lines.push(exchange.compare());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_driver::ShadowObject;
    use aim_binder_host::parcel::Binder;
    use aim_service_aidl::{read_typed, write_typed};

    const REPLIER: i32 = 50;

    fn words(words: &[i32]) -> Vec<u8> {
        words.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// The bytes of `p` as a copy with the given objects.
    fn copied(p: &Parcel, kinds: &[ShadowKind]) -> ShadowParcel {
        ShadowParcel {
            data: p.data().to_vec(),
            objects: p
                .objects()
                .iter()
                .zip(kinds)
                .map(|(&o, k)| ShadowObject {
                    offset: o as usize,
                    kind: k.clone(),
                })
                .collect(),
        }
    }

    fn call(seq: u64, code: u32, data: ShadowParcel, reply: ShadowReply) -> ShadowCopy {
        ShadowCopy {
            seq,
            node: 7,
            root: 7,
            follows: None,
            from_pid: 60,
            from_euid: 10_100,
            from_tid: 61,
            to_pid: REPLIER,
            sent: Instant::now(),
            code,
            flags: 0,
            data,
            reply,
        }
    }

    fn reply(p: &Parcel, kinds: &[ShadowKind]) -> ShadowReply {
        ShadowReply::Reply {
            flags: 0,
            parcel: copied(p, kinds),
        }
    }

    /// Answers with a fixed reply; decodes method 1 as a typed slice.
    struct Fixed(Mutex<Option<Answer>>);

    impl ShadowModel for Fixed {
        fn answer(&self, call: &mut ShadowCall<'_>) -> Answer {
            assert_eq!(call.descriptor, "test.IList");
            self.0.lock().unwrap().take().unwrap_or(Answer::NotModelled)
        }
        fn decode_reply(
            &self,
            _: &str,
            code: u32,
            reply: &mut Reader<'_>,
        ) -> Option<ParcelResult<Value>> {
            (code == 1).then(|| {
                super::super::decode(reply, |r| {
                    let _ = r.read_exception()?;
                    read_typed::<super::super::Slice>(r)
                })
            })
        }
    }

    fn comparator(answer: Answer) -> Comparator {
        let roots = Arc::new(Mutex::new(HashMap::from([(7, "list".to_string())])));
        let model: Arc<dyn ShadowModel> = Arc::new(Fixed(Mutex::new(Some(answer))));
        Comparator::new(
            HashMap::from([("list".to_string(), model)]),
            roots,
            Arc::default(),
        )
    }

    fn token() -> ShadowParcel {
        let mut p = Parcel::new();
        p.write_interface_token("test.IList");
        copied(&p, &[])
    }

    fn model_slice(items: &[i32]) -> Parcel {
        struct Item(i32);
        impl aim_service_aidl::WriteParcelable for Item {
            fn write_to(&self, p: &mut Parcel) {
                p.write_i32(self.0);
            }
        }
        let mut p = Parcel::new();
        p.write_no_exception();
        let slice = super::super::ListSlice {
            creator: "test.Item".into(),
            items: items.iter().map(|&i| Item(i)).collect(),
        };
        write_typed(&mut p, Some(&slice));
        p
    }

    #[test]
    fn stitches_a_slice_fetched_in_chunks() {
        let now = Instant::now();
        let mut c = comparator(Answer::Reply(model_slice(&[5, 6, 7])));
        // Inline: one item, then 0 and the retriever.
        let mut first = Parcel::new();
        first.write_no_exception();
        first.write_i32(1);
        first.write_i32(3);
        first.write_string16(Some("test.Item"));
        first.write_raw(&words(&[1, 5, 0]), &[]);
        first.write_binder(Some(Binder::Local(0x10)));
        let own = ShadowKind::Binder {
            node: 9,
            owner_pid: REPLIER,
        };
        let copy = call(1, 1, token(), reply(&first, &[own]));
        assert!(c.step(Some(copy), now).is_empty(), "held for its chunks");

        let fetch = |seq, index: i32, items: &[i32]| {
            let mut data = Parcel::new();
            data.write_i32(index);
            let mut chunk = Parcel::new();
            chunk.write_no_exception();
            chunk.write_raw(&words(items), &[]);
            let mut copy = call(seq, 1, copied(&data, &[]), reply(&chunk, &[]));
            copy.node = 9;
            copy.follows = Some(1);
            copy
        };
        assert!(c.step(Some(fetch(2, 1, &[1, 6, 0])), now).is_empty());
        let lines = c.step(Some(fetch(3, 2, &[1, 7])), now);
        let [line] = &lines[..] else {
            panic!("{lines:?}")
        };
        assert!(line.contains("\"outcome\":\"matched\""), "{line}");
        assert!(line.contains("\"chunks\":2"), "{line}");
        assert!(c.waiting.is_empty());
    }

    #[test]
    fn a_difference_is_logged_with_both_replies() {
        let now = Instant::now();
        let mut c = comparator(Answer::Reply(model_slice(&[5])));
        let lines = c.step(
            Some(call(1, 1, token(), reply(&model_slice(&[6]), &[]))),
            now,
        );
        let [line] = &lines[..] else {
            panic!("{lines:?}")
        };
        assert!(line.contains("\"outcome\":\"differed\""), "{line}");
        assert!(line.contains("\"original\":{\"slice\":1,"), "{line}");
        assert!(line.contains("\"items\":\"0100000006000000\""), "{line}");
        assert!(line.contains("\"items\":\"0100000005000000\""), "{line}");
    }

    #[test]
    fn a_held_reply_without_chunks_is_compared_at_its_deadline() {
        let now = Instant::now();
        // An own binder after the exception's 0 looks like a slice's tail.
        let mut model = Parcel::new();
        model.write_no_exception();
        model.write_binder(Some(Binder::Local(0x20)));
        let mut c = comparator(Answer::Reply(model.clone()));
        let mut original = Parcel::new();
        original.write_no_exception();
        original.write_binder(Some(Binder::Local(0x30)));
        let own = ShadowKind::Binder {
            node: 11,
            owner_pid: REPLIER,
        };
        assert!(
            c.step(Some(call(1, 2, token(), reply(&original, &[own]))), now)
                .is_empty()
        );
        assert_eq!(c.deadline(), Some(now + STITCH_WAIT));
        let lines = c.step(None, now + STITCH_WAIT);
        let [line] = &lines[..] else {
            panic!("{lines:?}")
        };
        // Raw: both binders are the answering side's own object.
        assert!(line.contains("\"outcome\":\"matched\""), "{line}");
    }

    #[test]
    fn binders_and_files_compare_by_identity() {
        let mut ids = Identities::default();
        let mut p = Parcel::new();
        p.write_binder(Some(Binder::Handle(3)));
        p.write_binder(Some(Binder::Handle(4)));
        let other = |node| ShadowKind::Binder {
            node,
            owner_pid: 70,
        };
        let call = normalize_copy(&copied(&p, &[other(100), other(100)]), None, &mut ids);
        let mut r = call.reader();
        assert_eq!(r.read_binder(), Ok(Some(Binder::Handle(0))));
        assert_eq!(r.read_binder(), Ok(Some(Binder::Handle(0))));

        // The model echoes the call's binder and adds one of its own.
        let mut answer = Parcel::new();
        answer.write_binder(Some(Binder::Handle(0)));
        answer.write_binder(Some(Binder::Local(0x40)));
        let model = normalize_model(&answer, &mut ids);
        let original = normalize_copy(
            &copied(
                &answer,
                &[
                    other(100),
                    ShadowKind::Binder {
                        node: 101,
                        owner_pid: REPLIER,
                    },
                ],
            ),
            Some(REPLIER),
            &mut ids,
        );
        assert_eq!(model.data, original.data);

        let files = [ShadowKind::File(Some((1, 2))), ShadowKind::File(None)];
        let mut p = Parcel::new();
        p.write_file(Arc::new(()));
        p.write_file(Arc::new(()));
        let a = normalize_copy(&copied(&p, &files), None, &mut ids);
        let b = normalize_copy(&copied(&p, &files), None, &mut ids);
        let fd = |n: &Normal, i: usize| {
            let mut r = n.reader();
            r.set_position(i * FLAT_BINDER_OBJECT_SIZE);
            r.read_fd().unwrap()
        };
        assert_eq!(fd(&a, 0), fd(&b, 0), "the same file");
        assert_ne!(fd(&a, 1), fd(&b, 1), "an unidentified file equals nothing");
    }

    #[test]
    fn unmodelled_oneway_and_failed_calls_are_counted() {
        let now = Instant::now();
        let mut c = comparator(Answer::NotModelled);
        let lines = c.step(Some(call(1, 3, token(), ShadowReply::Failed)), now);
        assert!(
            lines[0].ends_with("\"outcome\":\"not_modelled\"}"),
            "{lines:?}"
        );

        let mut c = comparator(Answer::Status(0));
        let mut oneway = call(2, 3, token(), ShadowReply::OneWay);
        oneway.flags = TF_ONE_WAY;
        assert!(c.step(Some(oneway), now)[0].contains("\"oneway\""));

        let mut c = comparator(Answer::Status(0));
        assert!(
            c.step(Some(call(3, 3, token(), ShadowReply::Failed)), now)[0].contains("\"failed\"")
        );

        // A status reply against a model's status.
        let mut c = comparator(Answer::Status(-74));
        let status = ShadowReply::Reply {
            flags: TF_STATUS_CODE,
            parcel: ShadowParcel {
                data: (-74i32).to_le_bytes().to_vec(),
                objects: Vec::new(),
            },
        };
        assert!(c.step(Some(call(4, 3, token(), status)), now)[0].contains("\"matched\""));
    }
}
