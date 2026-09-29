//! Vsync from the display's own refresh, through a Core Video display link.
//!
//! The link's callback runs on its own thread once per refresh, but late by
//! a varying amount: its `inNow` is the callback's own time (about 1 ms of
//! jitter). Its `inOutputTime` is the refresh a frame started now would be
//! shown at, from the link's model of the display's timing, exact to the
//! host tick. The vsync reported is the model's last refresh before the
//! callback: `inOutputTime` minus whole periods. Host ticks
//! (`mach_absolute_time`) are the guest's `CLOCK_MONOTONIC`.

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
}

type Sink = Box<dyn Fn(Tick) + Send + Sync>;

static SINK: Mutex<Option<Sink>> = Mutex::new(None);

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
    let tick = Tick {
        timestamp_ns: shown - period * (shown - now + period - 1).div_euclid(period),
        period_ns: period,
        now_ns: now,
    };
    if let Some(sink) = SINK.lock().unwrap().as_ref() {
        sink(tick);
    }
    0
}

/// Start the main display's link; `sink` gets every refresh. Returns the
/// nominal refresh period in nanoseconds.
pub fn start(sink: Sink) -> Result<i64, String> {
    *SINK.lock().unwrap() = Some(sink);
    let mut link = std::ptr::null_mut();
    // SAFETY: Core Video calls; the link lives for the process.
    unsafe {
        if CVDisplayLinkCreateWithCGDisplay(CGMainDisplayID(), &mut link) != 0 {
            return Err("CVDisplayLinkCreateWithCGDisplay failed".into());
        }
        let nominal = CVDisplayLinkGetNominalOutputVideoRefreshPeriod(link);
        CVDisplayLinkSetOutputCallback(link, on_refresh, std::ptr::null_mut());
        if CVDisplayLinkStart(link) != 0 {
            return Err("CVDisplayLinkStart failed".into());
        }
        if nominal.time_scale <= 0 || nominal.time_value <= 0 {
            return Err("the display link has no refresh period".into());
        }
        Ok((nominal.time_value as i128 * 1_000_000_000 / nominal.time_scale as i128) as i64)
    }
}
