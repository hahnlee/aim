//! Vsync from the display's own refresh, through a Core Video display link.
//!
//! The link's callback runs on its own thread once per refresh, but late by
//! a varying amount: its `inNow` is the callback's own time (about 1 ms of
//! jitter). Its `inOutputTime` is the refresh a frame started now would be
//! shown at, from the link's model of the display's timing, exact to the
//! host tick. The vsync reported is the model's last refresh before the
//! callback: `inOutputTime` minus whole periods, once per refresh. Host ticks
//! (`mach_absolute_time`) are the guest's `CLOCK_MONOTONIC`.
//!
//! The link runs only while a client has vsync enabled, as a display
//! controller raises its vsync interrupt only while the driver asks for it.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use aim_hostcall::clock::ticks_to_ns;

#[repr(C)]
struct CVSMPTETime {
    subframes: i16,
    subframe_divisor: i16,
    counter: u32,
    kind: u32,
    flags: u32,
    hours: i16,
    minutes: i16,
    seconds: i16,
    frames: i16,
}

#[repr(C)]
struct CVTimeStamp {
    version: u32,
    video_time_scale: i32,
    video_time: i64,
    host_time: u64,
    rate_scalar: f64,
    video_refresh_period: i64,
    smpte_time: CVSMPTETime,
    flags: u64,
    reserved: u64,
}

#[repr(C)]
struct CVTime {
    time_value: i64,
    time_scale: i32,
    flags: i32,
}

type Callback = extern "C" fn(
    link: *mut c_void,
    now: *const CVTimeStamp,
    output: *const CVTimeStamp,
    flags_in: u64,
    flags_out: *mut u64,
    context: *mut c_void,
) -> i32;

#[link(name = "CoreVideo", kind = "framework")]
unsafe extern "C" {
    fn CVDisplayLinkCreateWithCGDisplay(display: u32, link: *mut *mut c_void) -> i32;
    fn CVDisplayLinkSetOutputCallback(link: *mut c_void, cb: Callback, ctx: *mut c_void) -> i32;
    fn CVDisplayLinkStart(link: *mut c_void) -> i32;
    fn CVDisplayLinkStop(link: *mut c_void) -> i32;
    fn CVDisplayLinkGetNominalOutputVideoRefreshPeriod(link: *mut c_void) -> CVTime;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> u32;
}

pub use aim_host_display::monotonic_ns;

/// A host uptime timestamp (`NSEvent.timestamp`: `mach_absolute_time` in
/// seconds) in the guest's `CLOCK_MONOTONIC`.
pub fn uptime_to_monotonic(seconds: f64) -> i64 {
    (seconds * 1e9) as i64
}

/// One refresh, in guest time.
#[derive(Clone, Copy, Debug)]
pub struct Tick {
    pub timestamp_ns: i64,
    pub period_ns: i64,
    /// When the callback ran.
    pub now_ns: i64,
    /// For the first refresh after the link started: when it was started.
    pub started_ns: Option<i64>,
}

type Sink = Box<dyn Fn(Tick) + Send + Sync>;

static SINK: Mutex<Option<Sink>> = Mutex::new(None);

/// The link, and when it was started if it runs.
struct Link {
    link: *mut c_void,
    started_ns: Option<i64>,
    /// Whether a refresh has been reported since the start.
    ticked: bool,
    /// The last vsync reported and the period.
    last: Option<(i64, i64)>,
}

// SAFETY: a CVDisplayLinkRef may be used from any thread.
unsafe impl Send for Link {}

static LINK: Mutex<Link> = Mutex::new(Link {
    link: std::ptr::null_mut(),
    started_ns: None,
    ticked: false,
    last: None,
});

extern "C" fn on_refresh(
    _link: *mut c_void,
    _now: *const CVTimeStamp,
    output: *const CVTimeStamp,
    _flags_in: u64,
    _flags_out: *mut u64,
    _ctx: *mut c_void,
) -> i32 {
    // SAFETY: Core Video passes valid timestamps for the call.
    let output = unsafe { &*output };
    let now = monotonic_ns();
    if output.video_time_scale <= 0 || output.video_refresh_period <= 0 {
        return 0;
    }
    let period = (output.video_refresh_period as i128 * 1_000_000_000
        / output.video_time_scale as i128) as i64;
    let shown = ticks_to_ns(output.host_time);
    let timestamp_ns = shown - period * (shown - now + period - 1).div_euclid(period);
    let started_ns = {
        let mut l = LINK.lock().unwrap();
        // A callback late in a refresh and the next one early in it report
        // the same refresh, the model's times for it a microsecond apart:
        // it is one vsync.
        if let Some((last, _)) = l.last
            && (timestamp_ns - last).abs() < period / 2
        {
            return 0;
        }
        l.last = Some((timestamp_ns, period));
        (!std::mem::replace(&mut l.ticked, true))
            .then_some(l.started_ns)
            .flatten()
    };
    let tick = Tick {
        timestamp_ns,
        period_ns: period,
        now_ns: now,
        started_ns,
    };
    if let Some(sink) = SINK.lock().unwrap().as_ref() {
        sink(tick);
    }
    0
}

/// The first vsync at or after `ns`, on the timeline of the last one the
/// link reported (`ns` itself before it reported any). Always round forward:
/// rounding a nearby time back can put a present before its acquire fence.
pub fn vsync_at_or_after(ns: i64) -> i64 {
    let Some((last, period)) = LINK.lock().unwrap().last else {
        return ns;
    };
    on_timeline(ns, last, period)
}

fn on_timeline(ns: i64, last: i64, period: i64) -> i64 {
    last + period * (ns - last + period - 1).div_euclid(period)
}

/// Create the main display's link, stopped; `sink` gets every refresh
/// while it runs ([`update`]). Returns the nominal refresh period in
/// nanoseconds.
pub fn create(sink: Sink) -> Result<i64, String> {
    *SINK.lock().unwrap() = Some(sink);
    let mut link = std::ptr::null_mut();
    // SAFETY: Core Video calls; the link lives for the process.
    unsafe {
        if CVDisplayLinkCreateWithCGDisplay(CGMainDisplayID(), &mut link) != 0 {
            return Err("CVDisplayLinkCreateWithCGDisplay failed".into());
        }
        let nominal = CVDisplayLinkGetNominalOutputVideoRefreshPeriod(link);
        CVDisplayLinkSetOutputCallback(link, on_refresh, std::ptr::null_mut());
        if nominal.time_scale <= 0 || nominal.time_value <= 0 {
            return Err("the display link has no refresh period".into());
        }
        LINK.lock().unwrap().link = link;
        Ok((nominal.time_value as i128 * 1_000_000_000 / nominal.time_scale as i128) as i64)
    }
}

/// A virtual display has no physical refresh source. Its clock advances on a
/// fixed timeline, skipping missed refreshes instead of accumulating drift.
const HEADLESS_PERIOD: i64 = 1_000_000_000 / 60;
static HEADLESS: AtomicBool = AtomicBool::new(false);
static WAKE: Condvar = Condvar::new();

pub fn create_headless(sink: Sink) -> Result<i64, String> {
    *SINK.lock().unwrap() = Some(sink);
    HEADLESS.store(true, Ordering::Relaxed);
    std::thread::Builder::new()
        .name("headless-vsync".into())
        .spawn(|| {
            let period = Duration::from_nanos(HEADLESS_PERIOD as u64);
            loop {
                let mut link = LINK.lock().unwrap();
                while link.started_ns.is_none() {
                    link = WAKE.wait(link).unwrap();
                }
                let mut next = Instant::now() + period;
                while link.started_ns.is_some() {
                    let now = Instant::now();
                    if now < next {
                        link = WAKE.wait_timeout(link, next - now).unwrap().0;
                        continue;
                    }
                    let now_ns = monotonic_ns();
                    let timestamp_ns = now_ns - now.duration_since(next).as_nanos() as i64;
                    let started_ns = (!std::mem::replace(&mut link.ticked, true))
                        .then_some(link.started_ns)
                        .flatten();
                    link.last = Some((timestamp_ns, HEADLESS_PERIOD));
                    drop(link);
                    if let Some(sink) = SINK.lock().unwrap().as_ref() {
                        sink(Tick {
                            timestamp_ns,
                            period_ns: HEADLESS_PERIOD,
                            now_ns,
                            started_ns,
                        });
                    }
                    next += period
                        * ((now.duration_since(next).as_nanos() / period.as_nanos()) as u32 + 1);
                    link = LINK.lock().unwrap();
                }
            }
        })
        .map_err(|e| format!("headless vsync: {e}"))?;
    Ok(HEADLESS_PERIOD)
}

/// Run the link while `wanted()`. Updates are serialized, so concurrent
/// ones leave the link as the last of them found.
pub fn update(wanted: impl FnOnce() -> bool) {
    static UPDATE: Mutex<()> = Mutex::new(());
    let _serial = UPDATE.lock().unwrap();
    let want = wanted();
    if HEADLESS.load(Ordering::Relaxed) {
        let mut link = LINK.lock().unwrap();
        if want != link.started_ns.is_some() {
            link.started_ns = want.then(monotonic_ns);
            link.ticked = false;
            WAKE.notify_one();
        }
        return;
    }
    let link = {
        let mut l = LINK.lock().unwrap();
        if l.link.is_null() || want == l.started_ns.is_some() {
            return;
        }
        if want {
            l.started_ns = Some(monotonic_ns());
            l.ticked = false;
        }
        l.link
    };
    // SAFETY: the link from `create`. `CVDisplayLinkStop` waits for a
    // running callback, which takes `LINK`, so it is not held here.
    let err = unsafe {
        if want {
            CVDisplayLinkStart(link)
        } else {
            CVDisplayLinkStop(link)
        }
    };
    let running = if err == 0 {
        want
    } else {
        eprintln!("aim-display: display link start={want}: CVReturn {err}");
        !want
    };
    if !running {
        LINK.lock().unwrap().started_ns = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_present_time_never_precedes_its_source_time() {
        for period in [1_000_000_000 / 60, 1_000_000_000 / 120] {
            let last = 10 * period;
            for refresh in [-3, -1, 0, 1, 3] {
                for offset in [0, 1, 100_000, period / 8, period / 8 + 1, period - 1] {
                    let ns = last + refresh * period + offset;
                    let at = on_timeline(ns, last, period);
                    assert!(at >= ns, "source {ns}, reported {at}, period {period}");
                    assert!(at - ns < period);
                    assert_eq!((at - last).rem_euclid(period), 0);
                }
            }
        }
    }

    #[test]
    fn headless_clock_starts_stops_and_restarts_without_a_display_link() {
        let (send, receive) = std::sync::mpsc::channel();
        assert_eq!(
            create_headless(Box::new(move |tick| {
                send.send(tick).unwrap();
            }))
            .unwrap(),
            HEADLESS_PERIOD
        );
        assert!(receive.recv_timeout(Duration::from_millis(40)).is_err());
        for _ in 0..2 {
            update(|| true);
            let first = receive.recv_timeout(Duration::from_secs(1)).unwrap();
            assert!(first.started_ns.is_some());
            let next = receive.recv_timeout(Duration::from_secs(1)).unwrap();
            assert!(next.started_ns.is_none());
            assert_eq!(next.period_ns, HEADLESS_PERIOD);
            assert!(next.timestamp_ns > first.timestamp_ns);
            assert!(next.timestamp_ns <= next.now_ns);
            update(|| false);
            // A callback already outside LINK may finish; none starts after it.
            std::thread::sleep(Duration::from_millis(40));
            while receive.try_recv().is_ok() {}
            assert!(receive.recv_timeout(Duration::from_millis(40)).is_err());
        }
    }
}
