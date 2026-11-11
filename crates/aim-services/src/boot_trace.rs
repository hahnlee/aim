//! The boot's trace: what each part of the boot did, when, and what it
//! waited on, so that `tools/boot-trace.py` can follow the boot's critical
//! path to `sys.boot_completed` (docs/boot-status.md, "Boot trace").
//!
//! Events are kept in memory, at the cost of a lock and an allocation
//! each, and written whole to a file by guest-init. Times count from the
//! boot's epoch, as `ro.boottime.*` do. A lane is a sequence of work: init's
//! main loop (`init`), a service (`svc/NAME`), a thread of this process.
//! What an event waited on, its edge, is `property:NAME=VALUE`, `lane:LANE`
//! (that lane, up to this event's end) or `stage:NAME` (the latest span
//! named NAME), or a free description when nothing records the other end.

use std::fmt::Write as _;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Events after this many are dropped: the trace is the boot's, and a
/// resident guest keeps setting properties.
const LIMIT: usize = 20_000;

struct Event {
    start: Duration,
    end: Duration,
    lane: String,
    what: String,
    on: String,
}

/// One boot's events; the process's is behind [`start`] and [`between`].
pub struct Trace {
    epoch: Instant,
    events: Vec<Event>,
}

impl Trace {
    pub fn new(epoch: Instant) -> Self {
        Self {
            epoch,
            events: Vec::new(),
        }
    }

    /// Records `what`, on `lane`, from `from` until `to`, waiting on `on`.
    pub fn record(&mut self, lane: &str, what: &str, on: &str, from: Instant, to: Instant) {
        if self.events.len() >= LIMIT {
            return;
        }
        self.events.push(Event {
            start: from.saturating_duration_since(self.epoch),
            end: to.saturating_duration_since(self.epoch),
            lane: lane.to_string(),
            what: what.to_string(),
            on: on.to_string(),
        });
    }

    /// A line per event, `START END LANE WHAT ON` separated by tabs, times
    /// in microseconds since the epoch, in recording order.
    pub fn render(&self) -> String {
        let mut out = String::new();
        for e in &self.events {
            let clean = |s: &str| s.replace(['\t', '\n'], " ");
            let _ = writeln!(
                out,
                "{}\t{}\t{}\t{}\t{}",
                e.start.as_micros(),
                e.end.as_micros(),
                clean(&e.lane),
                clean(&e.what),
                clean(&e.on),
            );
        }
        out
    }
}

static TRACE: Mutex<Option<Trace>> = Mutex::new(None);

/// Starts the process's trace anew (a boot, or the boot after a reboot)
/// at `epoch`.
pub fn start(epoch: Instant) {
    *TRACE.lock().unwrap() = Some(Trace::new(epoch));
}

/// Records `what`, on `lane`, from `from` until now, waiting on `on`.
pub fn span(lane: &str, what: &str, on: &str, from: Instant) {
    between(lane, what, on, from, Instant::now());
}

/// Records `what`, on `lane`, from `from` until `to`, waiting on `on`.
pub fn between(lane: &str, what: &str, on: &str, from: Instant, to: Instant) {
    if let Some(trace) = TRACE.lock().unwrap().as_mut() {
        trace.record(lane, what, on, from, to);
    }
}

/// Records an instant `what` on `lane`.
pub fn mark(lane: &str, what: &str, on: &str) {
    span(lane, what, on, Instant::now());
}

/// The calling thread's lane: its name, else its id.
pub fn thread_lane() -> String {
    let thread = std::thread::current();
    match thread.name() {
        Some(name) => name.to_string(),
        None => format!("{:?}", thread.id()),
    }
}

/// Runs `f` as a span `what` of the calling thread's lane.
pub fn timed<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let from = Instant::now();
    let result = f();
    span(&thread_lane(), what, "", from);
    result
}

/// Writes the trace to `path`, replacing what an earlier write wrote.
pub fn write(path: &std::path::Path) -> std::io::Result<()> {
    let trace = TRACE.lock().unwrap();
    std::fs::write(path, trace.as_ref().map(Trace::render).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_render_from_the_epoch_in_recording_order() {
        let epoch = Instant::now();
        let mut trace = Trace::new(epoch);
        let later = epoch + Duration::from_micros(1500);
        trace.record("init", "exec\tx", "lane:svc/x", epoch, later);
        trace.record("svc/x", "set a=b", "", later, later);
        assert_eq!(
            trace.render(),
            "0\t1500\tinit\texec x\tlane:svc/x\n1500\t1500\tsvc/x\tset a=b\t\n"
        );
    }

    #[test]
    fn a_trace_drops_events_past_its_limit() {
        let epoch = Instant::now();
        let mut trace = Trace::new(epoch);
        for _ in 0..=LIMIT {
            trace.record("init", "x", "", epoch, epoch);
        }
        assert_eq!(trace.render().lines().count(), LIMIT);
    }

    #[test]
    fn a_named_thread_is_its_own_lane() {
        let lane = std::thread::Builder::new()
            .name("app-host boot".into())
            .spawn(thread_lane)
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(lane, "app-host boot");
    }
}
