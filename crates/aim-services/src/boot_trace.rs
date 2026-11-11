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

struct Trace {
    epoch: Instant,
    events: Vec<Event>,
}

static TRACE: Mutex<Option<Trace>> = Mutex::new(None);

/// Starts a new trace (a boot, or the boot after a reboot) at `epoch`.
pub fn start(epoch: Instant) {
    *TRACE.lock().unwrap() = Some(Trace {
        epoch,
        events: Vec::new(),
    });
}

/// Records `what`, on `lane`, from `from` until now, waiting on `on`.
pub fn span(lane: &str, what: &str, on: &str, from: Instant) {
    between(lane, what, on, from, Instant::now());
}

/// Records `what`, on `lane`, from `from` until `to`, waiting on `on`.
pub fn between(lane: &str, what: &str, on: &str, from: Instant, to: Instant) {
    let mut trace = TRACE.lock().unwrap();
    let Some(trace) = trace.as_mut().filter(|t| t.events.len() < LIMIT) else {
        return;
    };
    trace.events.push(Event {
        start: from.saturating_duration_since(trace.epoch),
        end: to.saturating_duration_since(trace.epoch),
        lane: lane.to_string(),
        what: what.to_string(),
        on: on.to_string(),
    });
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

/// The trace so far: a line per event, `START END LANE WHAT ON` separated
/// by tabs, times in microseconds since the epoch, in recording order.
pub fn render() -> String {
    let trace = TRACE.lock().unwrap();
    let mut out = String::new();
    for e in trace.iter().flat_map(|t| &t.events) {
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

/// Writes the trace to `path`, replacing what an earlier write wrote.
pub fn write(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::write(path, render())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One test owns the process-wide trace.
    #[test]
    fn events_render_from_the_epoch_and_a_new_boot_starts_empty() {
        let epoch = Instant::now();
        start(epoch);
        span("init", "exec\tx", "lane:svc/x", epoch);
        std::thread::Builder::new()
            .name("app-host boot".into())
            .spawn(|| timed("package boot", || ()))
            .unwrap()
            .join()
            .unwrap();
        mark("svc/x", "set a=b", "");
        let lines: Vec<Vec<String>> = render()
            .lines()
            .map(|l| l.split('\t').map(str::to_string).collect())
            .collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0][0], "0");
        assert_eq!(lines[0][2..], ["init", "exec x", "lane:svc/x"]);
        assert_eq!(lines[1][2..], ["app-host boot", "package boot", ""]);
        assert_eq!(lines[2][2..], ["svc/x", "set a=b", ""]);
        assert_eq!(lines[2][0], lines[2][1]);
        let at = |line: &[String], i: usize| line[i].parse::<u128>().unwrap();
        assert!(at(&lines[1], 0) <= at(&lines[1], 1));
        assert!(at(&lines[1], 1) <= at(&lines[2], 0));
        start(Instant::now());
        assert_eq!(render(), "");
    }
}
