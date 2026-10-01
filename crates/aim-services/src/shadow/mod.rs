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
//! both replies. `tools/binder-shadow-report.py` summarizes a log per
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
use std::sync::atomic::AtomicU64;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aim_binder_driver::{Credentials, Device, Driver, File, FileId, ShadowCopy, ShadowSink};
use aim_binder_host::local::{LocalProcess, Strong};
use aim_binder_host::parcel::{Binder, Parcel, Reader, Result as ParcelResult, StatusCode};
use aim_service_aidl::android_os_iservicemanager as sm;

pub use value::{IntoValue, ListSlice, Opaque, Slice, Value, decode};

use crate::system::System;
use crate::{SYSTEM_SERVER_CONTEXT, SYSTEM_UID};
use compare::Comparator;

/// Copies waiting for the comparison; more are dropped (and counted)
/// rather than holding up the driver.
const CAPACITY: usize = 4096;
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
/// logging to `log`.
pub(crate) fn start(
    driver: &Arc<Driver>,
    system: &Arc<System>,
    names: &[String],
    log: &Path,
) -> Result<(), String> {
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
        .map(|n| (n.clone(), model(n, system)))
        .collect();
    let mut comparator = Comparator::new(models, roots, dropped);
    std::thread::Builder::new()
        .name("binder-shadow".into())
        .spawn(move || {
            loop {
                let wait = comparator
                    .deadline()
                    .map_or(IDLE, |d| d.saturating_duration_since(Instant::now()));
                let copy: Option<ShadowCopy> = match received.recv_timeout(wait) {
                    Ok(copy) => Some(copy),
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
