//! Host-call module [`darwin_hostcall::module::CAMERA`]: the Mac's cameras
//! over AVFoundation, for the guest `android.hardware.camera.provider`
//! service (`docs/camera.md`).
//!
//! - [`FN_DEVICES`] lists the cameras (built-in first) with their landscape
//!   capture sizes, and the camera permission. It shows no prompt.
//! - [`FN_OPEN`] starts a session: an `AVCaptureSession` delivering BGRA
//!   frames on a dispatch queue, or, when macOS gives the process no camera
//!   access (or the camera is suspended), a test pattern at the requested
//!   rate on a thread of its own. Either way the session keeps the latest
//!   frame, and says which source it streams, with a log line.
//! - [`FN_FRAME`] waits for a newer frame and writes it into the guest's
//!   mapped buffers: scaled with a centered crop, packed as RGBA, NV12/NV21
//!   or JPEG (ImageIO).

mod avf;
mod convert;
mod jpeg;
mod objc;
mod pattern;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use darwin_hostcall::camera::{
    Device, Devices, FN_CLOSE, FN_DEVICES, FN_FRAME, FN_OPEN, Frame, MAX_DEVICES, MAX_OUTPUTS,
    MAX_SIZES, Open, Output, Session, VERSION, access, format, source,
};
use darwin_hostcall::{HostModule, args_mut, errno, module};

pub use convert::Bgra;

static LOG_FD: AtomicI32 = AtomicI32::new(2);

/// Where the module's messages go: the syscall layer's diagnostics
/// descriptor, which a service's `--stdio-null` leaves pointing at its
/// log.
pub fn set_log_fd(fd: i32) {
    LOG_FD.store(fd, Relaxed);
}

/// One line on the log descriptor, in a single write.
fn write_log(args: std::fmt::Arguments) {
    let line = format!("camera: {args}\n");
    // SAFETY: writing our buffer to a descriptor we were given.
    unsafe { libc::write(LOG_FD.load(Relaxed), line.as_ptr().cast(), line.len()) };
}

macro_rules! log {
    ($($t:tt)*) => {
        $crate::write_log(format_args!($($t)*))
    };
}
pub(crate) use log;

pub static MODULE: HostModule = HostModule {
    id: module::CAMERA,
    name: "camera",
    version: VERSION,
    call,
};

fn err(e: i32) -> i64 {
    -(e as i64)
}

unsafe fn call(func: u32, args: u64, len: u64) -> i64 {
    // SAFETY (each arm): the registry passes the guest's argument block.
    match func {
        FN_DEVICES => match unsafe { args_mut::<Devices>(args, len) } {
            Ok(out) => {
                *out = devices();
                0
            }
            Err(e) => e,
        },
        FN_OPEN => match unsafe { args_mut::<Open>(args, len) } {
            Ok(a) => open(a),
            Err(e) => e,
        },
        FN_FRAME => match unsafe { args_mut::<Frame>(args, len) } {
            Ok(a) => frame(a),
            Err(e) => e,
        },
        FN_CLOSE => match unsafe { args_mut::<Session>(args, len) } {
            Ok(a) => close(a.session),
            Err(e) => e,
        },
        _ => err(errno::ENOSYS),
    }
}

fn copy_str(dst: &mut [u8], s: &str) {
    let n = s.len().min(dst.len() - 1);
    dst[..n].copy_from_slice(&s.as_bytes()[..n]);
}

fn devices() -> Devices {
    let mut out = Devices::default();
    let Some(api) = avf::api() else {
        return out;
    };
    out.access = avf::access_status(api);
    for (d, c) in out.devices.iter_mut().zip(avf::cameras(api)) {
        *d = Device::default();
        copy_str(&mut d.id, &c.id);
        copy_str(&mut d.name, &c.name);
        d.facing = c.facing;
        for (s, size) in d.sizes.iter_mut().zip(&c.sizes) {
            *s = *size;
        }
        d.size_count = c.sizes.len().min(MAX_SIZES) as u32;
        out.count += 1;
    }
    out
}

/// The latest frame of a session.
pub struct Latest {
    frame: Mutex<(u64, i64, Option<Arc<Bgra>>)>,
    fresh: Condvar,
}

impl Latest {
    fn new() -> Arc<Latest> {
        Arc::new(Latest {
            frame: Mutex::new((0, 0, None)),
            fresh: Condvar::new(),
        })
    }

    /// Make `f`, captured at `ns`, the latest frame.
    pub fn publish(&self, f: Bgra, ns: i64) {
        let mut g = self.frame.lock().unwrap();
        *g = (g.0 + 1, ns, Some(Arc::new(f)));
        self.fresh.notify_all();
    }

    /// The first frame after sequence number `after`, waiting up to
    /// `timeout`.
    fn next(&self, after: u64, timeout: Duration) -> Option<(u64, i64, Arc<Bgra>)> {
        let deadline = Instant::now() + timeout;
        let mut g = self.frame.lock().unwrap();
        loop {
            if g.0 > after
                && let Some(f) = &g.2
            {
                return Some((g.0, g.1, f.clone()));
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            g = self.fresh.wait_timeout(g, left).unwrap().0;
        }
    }
}

enum Source {
    Camera(avf::Capture),
    Pattern(Arc<AtomicBool>),
}

struct Live {
    device: String,
    latest: Arc<Latest>,
    source: Source,
}

static SESSIONS: Mutex<Option<HashMap<u64, Live>>> = Mutex::new(None);
static NEXT: AtomicU64 = AtomicU64::new(1);

/// Stream the test pattern at `fps` into `latest` until `stop`.
fn pattern(latest: Arc<Latest>, width: u32, height: u32, fps: u32, stop: Arc<AtomicBool>) {
    let period = Duration::from_nanos(1_000_000_000 / fps.clamp(1, 120) as u64);
    let mut next = Instant::now();
    let mut n = 0;
    while !stop.load(Relaxed) {
        let mut f = Bgra::new(width, height);
        pattern::draw(&mut f, n);
        latest.publish(f, clock::now_ns());
        n += 1;
        next += period;
        if let Some(d) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(d);
        } else {
            next = Instant::now();
        }
    }
}

fn open(a: &mut Open) -> i64 {
    if a.width == 0 || a.height == 0 || a.width > 16384 || a.height > 16384 {
        return err(errno::EINVAL);
    }
    let Some(api) = avf::api() else {
        return err(errno::ENODEV);
    };
    let cams = avf::cameras(api);
    let Some(cam) = cams
        .get(a.device as usize)
        .filter(|_| (a.device as usize) < MAX_DEVICES)
    else {
        return err(errno::ENODEV);
    };
    let mut sessions = SESSIONS.lock().unwrap();
    let sessions = sessions.get_or_insert_with(HashMap::new);
    if sessions.values().any(|s| s.device == cam.id) {
        return err(errno::EBUSY);
    }
    let latest = Latest::new();
    let fps = a.fps.clamp(1, 120);
    let status = avf::access_status(api);
    let why_not = match status {
        access::AUTHORIZED if cam.suspended => Some("the camera is suspended (lid closed?)".into()),
        access::AUTHORIZED => None,
        access::NOT_DETERMINED => {
            avf::request_access(api);
            Some("camera access is not granted yet".to_string())
        }
        _ => Some("camera access is denied".to_string()),
    };
    let started = match why_not {
        None => avf::Capture::start(api, cam, a.width, a.height, fps, &latest),
        Some(why) => Err(why),
    };
    let source = match started {
        Ok(c) => {
            crate::log!(
                "{}: capturing {}x{} at {fps} fps",
                cam.name,
                a.width,
                a.height
            );
            a.source = source::CAMERA;
            Source::Camera(c)
        }
        Err(why) => {
            crate::log!(
                "{}: {why}; streaming a test pattern ({}x{} at {fps} fps)",
                cam.name,
                a.width,
                a.height
            );
            let stop = Arc::new(AtomicBool::new(false));
            let (l, s, w, h) = (latest.clone(), stop.clone(), a.width, a.height);
            std::thread::Builder::new()
                .name("camera-pattern".into())
                .spawn(move || pattern(l, w, h, fps, s))
                .expect("spawn the test pattern thread");
            a.source = source::TEST_PATTERN;
            Source::Pattern(stop)
        }
    };
    let id = NEXT.fetch_add(1, Relaxed);
    sessions.insert(
        id,
        Live {
            device: cam.id.clone(),
            latest,
            source,
        },
    );
    a.session = id;
    0
}

fn close(id: u64) -> i64 {
    let live = SESSIONS
        .lock()
        .unwrap()
        .as_mut()
        .and_then(|s| s.remove(&id));
    let Some(live) = live else {
        return err(errno::EINVAL);
    };
    match live.source {
        Source::Camera(c) => drop(c),
        Source::Pattern(stop) => stop.store(true, Relaxed),
    }
    0
}

/// Check that `o` describes a writable destination of its format.
fn check(o: &Output) -> bool {
    let (w, h) = (o.width as u64, o.height as u64);
    if o.address == 0 || w == 0 || h == 0 || w > 16384 || h > 16384 {
        return false;
    }
    let stride = o.stride as u64;
    match o.format {
        format::RGBA_8888 | format::RGBX_8888 => stride >= w * 4 && stride * h <= o.length,
        format::YCBCR_420_888 | format::YCRCB_420_SP => {
            let chroma_end = o.chroma_offset + o.chroma_stride as u64 * h.div_ceil(2);
            stride >= w
                && o.chroma_stride as u64 >= w.div_ceil(2) * 2
                && o.chroma_offset >= stride * h
                && chroma_end <= o.length
        }
        format::BLOB => o.length > 0,
        _ => false,
    }
}

/// Write `scaled` (the frame at the output's size) into `o`; returns the
/// bytes written, 0 when it did not fit.
fn write(scaled: &Bgra, o: &Output) -> u64 {
    // SAFETY: `check` passed; the guest's mapping of `length` bytes at
    // `address` is valid for the call.
    let dst = unsafe { std::slice::from_raw_parts_mut(o.address as *mut u8, o.length as usize) };
    match o.format {
        format::RGBA_8888 | format::RGBX_8888 => {
            convert::to_rgba(scaled, dst, o.stride as usize);
            o.stride as u64 * o.height as u64
        }
        format::YCBCR_420_888 | format::YCRCB_420_SP => {
            convert::to_nv(
                scaled,
                dst,
                o.stride as usize,
                o.chroma_offset as usize,
                o.chroma_stride as usize,
                o.format == format::YCRCB_420_SP,
            );
            o.chroma_offset + o.chroma_stride as u64 * (o.height as u64).div_ceil(2)
        }
        _ => {
            let Some(api) = avf::api() else { return 0 };
            match jpeg::encode(&api.jpeg, scaled, o.jpeg_quality, o.jpeg_orientation) {
                Some(j) if j.len() <= dst.len() => {
                    dst[..j.len()].copy_from_slice(&j);
                    j.len() as u64
                }
                _ => 0,
            }
        }
    }
}

fn frame(a: &mut Frame) -> i64 {
    let n = a.output_count as usize;
    if n > MAX_OUTPUTS || (n > 0 && a.outputs == 0) || a.outputs % 8 != 0 {
        return err(errno::EINVAL);
    }
    let outputs: &mut [Output] = if n == 0 {
        &mut []
    } else {
        // SAFETY: the guest's array of `n` outputs, checked for null and
        // alignment above.
        unsafe { std::slice::from_raw_parts_mut(a.outputs as *mut Output, n) }
    };
    if !outputs.iter().all(check) {
        return err(errno::EINVAL);
    }
    let latest = match SESSIONS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|s| s.get(&a.session))
    {
        Some(l) => l.latest.clone(),
        None => return err(errno::EINVAL),
    };
    let timeout = Duration::from_millis(a.timeout_ms.min(5000) as u64);
    let Some((seq, ns, f)) = latest.next(a.after, timeout) else {
        return err(errno::EAGAIN);
    };
    // Outputs of one size (a preview and a YUV reader, say) share one
    // scaling.
    let mut sizes: Vec<((u32, u32), Bgra)> = Vec::new();
    for o in outputs.iter_mut() {
        let size = (o.width, o.height);
        let i = match sizes.iter().position(|(s, _)| *s == size) {
            Some(i) => i,
            None => {
                sizes.push((size, f.scaled(o.width, o.height)));
                sizes.len() - 1
            }
        };
        o.written = write(&sizes[i].1, o);
    }
    a.seq = seq;
    a.timestamp_ns = ns;
    0
}

/// Host time on the guest's clocks: the syscall layer serves the guest's
/// CLOCK_MONOTONIC and CLOCK_BOOTTIME from the host's CLOCK_MONOTONIC.
mod clock {
    use std::sync::OnceLock;

    #[repr(C)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }

    unsafe extern "C" {
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
        fn mach_absolute_time() -> u64;
    }

    pub fn now_ns() -> i64 {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: a local timespec.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
        ts.tv_sec * 1_000_000_000 + ts.tv_nsec
    }

    fn timebase() -> (i64, i64) {
        static TIMEBASE: OnceLock<(i64, i64)> = OnceLock::new();
        *TIMEBASE.get_or_init(|| {
            let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
            // SAFETY: a local out-parameter.
            unsafe { mach_timebase_info(&mut info) };
            (info.numer as i64, info.denom.max(1) as i64)
        })
    }

    /// CLOCK_MONOTONIC nanoseconds of a mach absolute time.
    pub fn host_to_ns(host_time: u64) -> i64 {
        let (numer, denom) = timebase();
        // SAFETY: reads the clock.
        let now = unsafe { mach_absolute_time() };
        now_ns() + (host_time as i64 - now as i64) * numer / denom
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(format: i32, w: u32, h: u32, buf: &mut [u8]) -> Output {
        let stride = if format == format::RGBA_8888 {
            w * 4
        } else {
            w
        };
        Output {
            format,
            width: w,
            height: h,
            stride,
            chroma_offset: (stride * h) as u64,
            chroma_stride: w,
            jpeg_quality: 90,
            address: buf.as_mut_ptr() as u64,
            length: buf.len() as u64,
            ..Output::default()
        }
    }

    #[test]
    fn outputs_are_checked() {
        let mut buf = vec![0u8; 64 * 48 * 4];
        assert!(check(&output(format::RGBA_8888, 64, 48, &mut buf)));
        assert!(!check(&output(format::RGBA_8888, 64, 49, &mut buf)));
        assert!(check(&output(format::YCBCR_420_888, 64, 48, &mut buf)));
        let mut small = vec![0u8; 64 * 48];
        assert!(!check(&output(format::YCBCR_420_888, 64, 48, &mut small)));
        assert!(!check(&output(0x16, 64, 48, &mut buf)));
    }

    #[test]
    fn latest_waits_for_a_newer_frame() {
        let l = Latest::new();
        assert!(l.next(0, Duration::from_millis(10)).is_none());
        l.publish(Bgra::new(2, 2), 42);
        let (seq, ns, _) = l.next(0, Duration::from_millis(10)).unwrap();
        assert_eq!((seq, ns), (1, 42));
        assert!(l.next(1, Duration::from_millis(10)).is_none());
    }

    #[test]
    fn pattern_session_fills_every_output_kind() {
        // A session on the pattern alone (no camera needed), then one frame
        // into RGBA, NV12 and JPEG outputs.
        let latest = Latest::new();
        let stop = Arc::new(AtomicBool::new(false));
        let (l, s) = (latest.clone(), stop.clone());
        let t = std::thread::spawn(move || pattern(l, 320, 240, 30, s));
        let (_, ns, f) = latest.next(0, Duration::from_secs(2)).unwrap();
        stop.store(true, Relaxed);
        t.join().unwrap();
        assert!((clock::now_ns() - ns).abs() < 2_000_000_000);

        let mut rgba = vec![0u8; 160 * 120 * 4];
        let o = output(format::RGBA_8888, 160, 120, &mut rgba);
        let small = f.scaled(160, 120);
        assert_eq!(write(&small, &o), 160 * 120 * 4);
        // The first bar is white, the last (x 7/8 onwards) black.
        assert_eq!(&rgba[60 * 640 + 5 * 4..][..4], &[255, 255, 255, 255]);
        assert_eq!(&rgba[60 * 640 + 150 * 4..][..4], &[0, 0, 0, 255]);

        let mut nv = vec![0u8; 160 * 120 * 3 / 2];
        let o = output(format::YCBCR_420_888, 160, 120, &mut nv);
        assert_eq!(write(&small, &o), 160 * 120 * 3 / 2);
        assert_eq!(nv[60 * 160 + 150], 0);

        if avf::api().is_some() {
            let mut blob = vec![0u8; 64 * 1024];
            let o = output(format::BLOB, 320, 240, &mut blob);
            let n = write(&f, &o) as usize;
            assert!(n > 100 && blob[..2] == [0xff, 0xd8]);
        }
    }

    #[test]
    fn call_checks_the_argument_blocks() {
        let mut d = Devices::default();
        let p = &mut d as *mut Devices as u64;
        let einval = err(errno::EINVAL);
        // SAFETY: live, correctly sized blocks; the bad calls never touch
        // them.
        unsafe {
            assert_eq!(call(FN_DEVICES, p, 8), einval);
            assert_eq!(call(99, 0, 0), err(errno::ENOSYS));
            let mut s = Session { session: 12345 };
            assert_eq!(call(FN_CLOSE, &mut s as *mut Session as u64, 8), einval);
            let mut f = Frame {
                session: 12345,
                ..Frame::default()
            };
            assert_eq!(call(FN_FRAME, &mut f as *mut Frame as u64, 48), einval);
            assert_eq!(
                call(FN_DEVICES, p, std::mem::size_of::<Devices>() as u64),
                0
            );
        }
        eprintln!("{} camera(s), access {}", d.count, d.access);
    }
}
