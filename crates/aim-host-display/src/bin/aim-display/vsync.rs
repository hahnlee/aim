//! Vsync from the display's own refresh, through a Core Video display link.
//!
//! The link's callback runs on its own thread once per refresh, but late by
//! a varying amount: its `inNow` is the callback's own time (about 1 ms of
//! jitter). Its `inOutputTime` is the refresh a frame started now would be
//! shown at, from the link's model of the display's timing, exact to the
//! host tick. The vsync reported is the model's last refresh before the
//! callback: `inOutputTime` minus whole periods. Host ticks
//! (`mach_absolute_time`) are the guest's `CLOCK_MONOTONIC`.
//!
//! The link runs only while a client has vsync enabled, as a display
//! controller raises its vsync interrupt only while the driver asks for it.

use std::ffi::c_void;
use std::sync::Mutex;

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
/// link reported (`ns` itself before it reported any). Times Core
/// Animation reports for a refresh lie within a microsecond of the model's,
/// on either side, so a time up to an eighth of a period past a vsync
/// counts as that vsync.
pub fn vsync_at_or_after(ns: i64) -> i64 {
    let Some((last, period)) = LINK.lock().unwrap().last else {
        return ns;
    };
    last + period * (ns - period / 8 - last + period - 1).div_euclid(period)
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

/// Run the link while `wanted()`. Updates are serialized, so concurrent
/// ones leave the link as the last of them found.
pub fn update(wanted: impl FnOnce() -> bool) {
    static UPDATE: Mutex<()> = Mutex::new(());
    let _serial = UPDATE.lock().unwrap();
    let want = wanted();
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
