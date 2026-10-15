//! Shadow comparison (docs/m4-packagemanager.md, slice A): every call to
//! an original service, answered again by a native model of it that
//! serves nothing, and the two replies compared.
//!
//! guest-init `--binder-shadow package,package_native --binder-shadow-log
//! FILE` has the binder driver copy each transaction to those services'
//! nodes and its reply ([`aim_binder_driver::Driver::shadow`]); the
//! callers get the original's replies as without it. One thread hands
//! each copy to the [`ShadowModel`] of its service, after the original
//! replied and in the driver's order, decodes both replies the same way
//! and writes one JSON line per call to the log; `differed` lines carry
//! both replies. A model answers from a state at least as new as the
//! call, which a change of the original's state can have overtaken: a
//! reply that differs but equals the model's answer from the state it
//! held before the call ([`ShadowModel::answer_before`]) is `raced`, a
//! change between the original's answer and the model's, logged with
//! both replies and counted apart from the differences. `tools/binder-shadow-report.py` summarizes a log per
//! method.
//!
//! A reply's `ParceledListSlice` hands out a binder the caller fetches
//! the rest of the list through; the driver follows it, and its chunks
//! are stitched into the reply before it is compared, so a model writes
//! the list inline ([`ListSlice`]). Binders and fds are compared by what
//! they stand for ([`compare`]).

mod compare;
mod value;

use std::collections::HashMap;
use std::io::{LineWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aim_binder_driver::{
    Credentials, Device, Driver, File, FileId, ShadowCopy, ShadowObject, ShadowReply, ShadowSink,
};
use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{Binder, Parcel, Reader, Result as ParcelResult, StatusCode};
use aim_service_aidl::android_os_iservicemanager as sm;

pub use value::{IntoValue, ListSlice, Opaque, Slice, Value, decode};

use crate::system::System;
use crate::{SYSTEM_SERVER_CONTEXT, SYSTEM_UID};
use compare::Comparator;

/// Copies on their way from the driver: the driver never waits for
/// them, and a thread moves each at once to the comparison's backlog.
const CAPACITY: usize = 4096;
/// The bytes of copies the comparison may fall behind by (about a first
/// boot's); more are dropped (and counted) rather than holding up the
/// driver.
const BACKLOG: usize = 512 << 20;
/// How often a watcher looks for its service until it is published: the
/// calls before it finds it are not compared.
const RETRY: Duration = Duration::from_millis(100);
/// How long the comparison waits for a copy when no list is waiting, to
/// report drops.
const IDLE: Duration = Duration::from_millis(500);

/// A native model of a service, answering the calls the original got.
pub trait ShadowModel: Send + Sync {
    /// Answers `call` as the native service would. Called after the
    /// original replied, in the driver's order, on one thread.
    fn answer(&self, call: &mut ShadowCall<'_>) -> Answer;

    /// Answers `call` again, as `answer` would, from the newest state the
    /// model held before the call was sent: `None` when it held none or
    /// does not answer the call that way. Called only when `answer`'s
    /// reply differs from the original's; it changes nothing.
    fn answer_before(&self, _call: &mut ShadowCall<'_>) -> Option<Answer> {
        None
    }

    /// Decodes a reply of `descriptor`'s method `code`, the original's and
    /// the model's alike, with the generated reader ([`decode`]). `None`:
    /// compared raw (the exception, then the bytes, binders and fds by
    /// identity).
    fn decode_reply(
        &self,
        _descriptor: &str,
        _code: u32,
        _reply: &mut Reader<'_>,
    ) -> Option<ParcelResult<Value>> {
        None
    }

    /// The state checks that are due: each compares what calls answered
    /// earlier changed in the model with what they changed in the
    /// original. Called on the comparison's thread after each step.
    fn checks(&self) -> Vec<Check> {
        Vec::new()
    }
}

/// A check of the state a model's calls left against the original's:
/// a write's effect, compared once the original has made it.
pub struct Check {
    pub service: String,
    pub descriptor: String,
    /// What the calls did, e.g. an install.
    pub operation: String,
    /// The calls it covers, if any: each one's sequence number and code.
    pub calls: Vec<(u64, u32)>,
    /// What was compared, e.g. a package in a user.
    pub subject: String,
    pub outcome: CheckOutcome,
}

pub enum CheckOutcome {
    Matched,
    Differed {
        original: Value,
        model: Value,
    },
    /// A call it covers changed what the model does not hold.
    NotModelled(String),
}

impl Check {
    /// The check's log line.
    pub(crate) fn line(&self) -> String {
        let mut line = String::from("{\"check\":\"state\",\"service\":");
        value::json_string(&self.service, &mut line);
        line.push_str(",\"descriptor\":");
        value::json_string(&self.descriptor, &mut line);
        line.push_str(",\"operation\":");
        value::json_string(&self.operation, &mut line);
        line.push_str(",\"calls\":[");
        for (i, (seq, code)) in self.calls.iter().enumerate() {
            if i > 0 {
                line.push(',');
            }
            line.push_str(&format!("[{seq},{code}]"));
        }
        line.push_str("],\"subject\":");
        value::json_string(&self.subject, &mut line);
        match &self.outcome {
            CheckOutcome::Matched => line.push_str(",\"outcome\":\"matched\""),
            CheckOutcome::Differed { original, model } => {
                line.push_str(",\"outcome\":\"differed\",\"original\":");
                original.json(&mut line);
                line.push_str(",\"model\":");
                model.json(&mut line);
            }
            CheckOutcome::NotModelled(reason) => {
                line.push_str(",\"outcome\":\"not_modelled\",\"reason\":");
                value::json_string(reason, &mut line);
            }
        }
        line.push('}');
        line
    }
}

/// A call as the original received it.
pub struct ShadowCall<'a> {
    /// The name of the watched service it came through.
    pub service: &'a str,
    /// Its interface token.
    pub descriptor: &'a str,
    pub code: u32,
    pub flags: u32,
    pub sender_pid: i32,
    pub sender_euid: u32,
    /// The driver's sequence number of the call.
    pub seq: u64,
    /// When the driver took the call, before the original could see it.
    pub sent: Instant,
    /// The copies the shadow dropped so far, without comparing them.
    pub dropped: u64,
    /// At the interface token. A binder in it reads as
    /// `Binder::Handle(i)` and an fd as `i`, its identity's index.
    pub data: Reader<'a>,
}

/// A model's answer.
pub enum Answer {
    NotModelled,
    /// Written with the generated `write_*_reply`. A binder of the model
    /// is an object of its own; `Binder::Handle(i)` read from the call
    /// stands for the call's binder.
    Reply(Parcel),
    /// A binder status instead of a reply.
    Status(StatusCode),
}

/// A model with no methods yet.
struct Unmodelled;

impl ShadowModel for Unmodelled {
    fn answer(&self, _: &mut ShadowCall<'_>) -> Answer {
        Answer::NotModelled
    }
}

/// The models, by the name of the original service they model; a service
/// without one is [`Unmodelled`].
type MakeModel = fn(&Arc<System>) -> Arc<dyn ShadowModel>;
const MODELS: &[(&str, MakeModel)] = &[];

fn model(name: &str, system: &Arc<System>) -> Arc<dyn ShadowModel> {
    match MODELS.iter().find(|(n, _)| *n == name) {
        Some((_, make)) => make(system),
        None => Arc::new(Unmodelled),
    }
}

/// What a file stands for: its device and inode.
pub(crate) fn identify(file: &File) -> Option<FileId> {
    let fd = aim_binder_host::server::file_fd(file)?;
    // SAFETY: a stat buffer for a file this process owns.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::fstat(std::os::fd::AsRawFd::as_raw_fd(&fd), &mut st) } == 0;
    ok.then_some((st.st_dev as u64, st.st_ino))
}

type Log = Arc<Mutex<LineWriter<std::fs::File>>>;

fn log_line(log: &Log, line: &str) {
    let mut log = log.lock().unwrap();
    let _ = log
        .write_all(line.as_bytes())
        .and_then(|_| log.write_all(b"\n"));
}

/// Starts comparing the original services `names` with their models,
/// logging to `log`; `image` is the guest's root and `props` its
/// properties, which the package model reads the device's configuration
/// from.
pub(crate) fn start(
    driver: &Arc<Driver>,
    system: &Arc<System>,
    names: &[String],
    log: &Path,
    image: &Path,
    props: crate::package::system_config::Properties,
    files: crate::package::write::Files,
) -> Result<(), String> {
    // One model of package and package_native, over the state the
    // original feeds it, written beside the log (crate::package::feed).
    let package: Option<Arc<dyn ShadowModel>> = if names
        .iter()
        .any(|n| n == "package" || n == "package_native")
    {
        let dump = log.with_extension("package-feed.txt");
        Some(crate::package::query::start(
            system, image, props, dump, files,
        )?)
    } else {
        None
    };
    let file = std::fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let log: Log = Arc::new(Mutex::new(LineWriter::new(file)));
    let (copies, received) = mpsc::sync_channel(CAPACITY);
    let dropped = Arc::new(AtomicU64::new(0));
    let sink = ShadowSink {
        copies,
        identify,
        dropped: dropped.clone(),
    };
    let roots = Arc::new(Mutex::new(HashMap::new()));
    let process = LocalProcess::open(
        driver,
        Device::Binder,
        Credentials {
            pid: std::process::id() as i32,
            euid: SYSTEM_UID,
            security_context: Some(SYSTEM_SERVER_CONTEXT.into()),
        },
    );
    process.start();
    for name in names {
        let watcher = Watcher {
            process: process.clone(),
            driver: driver.clone(),
            name: name.clone(),
            sink: sink.clone(),
            roots: roots.clone(),
            log: log.clone(),
        };
        std::thread::Builder::new()
            .name("binder-shadow-watch".into())
            .spawn(move || watcher.run())
            .map_err(|e| format!("binder shadow: {e}"))?;
    }
    let models = names
        .iter()
        .map(|n| {
            let m = match (n.as_str(), &package) {
                ("package" | "package_native", Some(p)) => p.clone(),
                _ => model(n, system),
            };
            (n.clone(), m)
        })
        .collect();
    let (backlog, queued) = mpsc::channel();
    let bytes = Arc::new(AtomicUsize::new(0));
    {
        let (bytes, dropped) = (bytes.clone(), dropped.clone());
        std::thread::Builder::new()
            .name("binder-shadow-drain".into())
            .spawn(move || drain(&received, &backlog, &bytes, &dropped, BACKLOG))
            .map_err(|e| format!("binder shadow: {e}"))?;
    }
    let mut comparator = Comparator::new(models, roots, dropped);
    std::thread::Builder::new()
        .name("binder-shadow".into())
        .spawn(move || {
            loop {
                let wait = comparator
                    .deadline()
                    .map_or(IDLE, |d| d.saturating_duration_since(Instant::now()));
                let copy: Option<ShadowCopy> = match queued.recv_timeout(wait) {
                    Ok(copy) => {
                        bytes.fetch_sub(size(&copy), Ordering::Relaxed);
                        Some(copy)
                    }
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                };
                for line in comparator.step(copy, Instant::now()) {
                    log_line(&log, &line);
                }
            }
        })
        .map(drop)
        .map_err(|e| format!("binder shadow: {e}"))
}

/// Moves each copy from the driver's channel to the backlog as it comes,
/// dropping (and counting) one that would take the backlog's `bytes` past
/// `limit`; the comparison takes them off.
fn drain(
    received: &mpsc::Receiver<ShadowCopy>,
    backlog: &mpsc::Sender<ShadowCopy>,
    bytes: &AtomicUsize,
    dropped: &AtomicU64,
    limit: usize,
) {
    while let Ok(copy) = received.recv() {
        let size = size(&copy);
        if bytes.fetch_add(size, Ordering::Relaxed) + size > limit {
            bytes.fetch_sub(size, Ordering::Relaxed);
            dropped.fetch_add(1, Ordering::Relaxed);
        } else if backlog.send(copy).is_err() {
            return;
        }
    }
}

/// The bytes a copy holds, as the backlog counts them.
fn size(copy: &ShadowCopy) -> usize {
    let object = std::mem::size_of::<ShadowObject>();
    let reply = match &copy.reply {
        ShadowReply::Reply { parcel, .. } => parcel.data.len() + parcel.objects.len() * object,
        _ => 0,
    };
    std::mem::size_of::<ShadowCopy>()
        + copy.data.data.len()
        + copy.data.objects.len() * object
        + reply
}

/// Watches one service: whenever it is published, its node is shadowed.
struct Watcher {
    process: Arc<LocalProcess>,
    driver: Arc<Driver>,
    name: String,
    sink: ShadowSink,
    /// The service of each watched node.
    roots: Arc<Mutex<HashMap<u64, String>>>,
    log: Log,
}

impl Watcher {
    fn run(self) {
        loop {
            let Some(service) = self.find() else {
                std::thread::sleep(RETRY);
                continue;
            };
            {
                // Held while the node is shadowed, so the comparator knows
                // its first copy's service.
                let mut roots = self.roots.lock().unwrap();
                match self.driver.shadow(
                    self.process.proc_handle(),
                    service.handle,
                    self.sink.clone(),
                ) {
                    Ok(node) => {
                        roots.insert(node, self.name.clone());
                        self.event("watching", Some(node));
                    }
                    Err(e) => {
                        drop(roots);
                        eprintln!("guest-init: binder shadow: {}: errno {e}", self.name);
                        std::thread::sleep(RETRY);
                        continue;
                    }
                }
            }
            let (died, death) = mpsc::channel();
            self.process.link_to_death(
                &service,
                Box::new(move || {
                    let _ = died.send(());
                }),
            );
            let _ = death.recv();
            self.event("died", None);
        }
    }

    fn event(&self, event: &str, node: Option<u64>) {
        let mut line = format!("{{\"event\":\"{event}\",\"service\":");
        value::json_string(&self.name, &mut line);
        if let Some(node) = node {
            line.push_str(&format!(",\"node\":{node}"));
        }
        line.push('}');
        log_line(&self.log, &line);
    }

    /// The service from servicemanager.
    fn find(&self) -> Option<Strong> {
        let mut data = Parcel::new();
        sm::CheckService {
            name: Some(self.name.clone()),
        }
        .write(&mut data);
        let reply = self
            .process
            .transact(0, sm::CHECK_SERVICE, &data, false)
            .ok()?;
        let Ok(Ok(Some(Binder::Handle(h)))) = sm::read_check_service_reply(&mut reply.reader())
        else {
            return None;
        };
        Some(self.process.strong(h))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_binder_driver::ShadowParcel;

    fn copy(seq: u64, len: usize) -> ShadowCopy {
        ShadowCopy {
            seq,
            node: 0,
            root: 0,
            follows: None,
            from_pid: 1,
            from_euid: 2,
            from_tid: 3,
            to_pid: 4,
            sent: Instant::now(),
            code: 1,
            flags: 0,
            data: ShadowParcel {
                data: vec![0; len],
                objects: Vec::new(),
            },
            reply: ShadowReply::OneWay,
        }
    }

    #[test]
    fn the_backlog_is_bounded_by_bytes() {
        let (copies, received) = mpsc::sync_channel(8);
        let (backlog, queued) = mpsc::channel();
        let (bytes, dropped) = (AtomicUsize::new(0), AtomicU64::new(0));
        let limit = 2 * size(&copy(0, 100));
        for seq in 1..=3 {
            copies.send(copy(seq, 100)).unwrap();
        }
        drop(copies);
        drain(&received, &backlog, &bytes, &dropped, limit);
        let taken: Vec<u64> = queued.try_iter().map(|c| c.seq).collect();
        assert_eq!(taken, [1, 2]);
        assert_eq!(dropped.load(Ordering::Relaxed), 1);
        assert_eq!(bytes.load(Ordering::Relaxed), limit);
    }
}
