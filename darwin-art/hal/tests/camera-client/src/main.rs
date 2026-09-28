//! `camera-client [JPEG_PATH]`: captures through the original cameraserver
//! with the NDK's Camera2 API, as an app's native code would.
//!
//! It lists the cameras (facing, hardware level), opens the first, streams
//! 30 preview frames into a 640x480 YUV_420_888 `AImageReader` and a
//! GPU-sampled (IMPLEMENTATION_DEFINED) one, as a TextureView's, then takes
//! a still capture into a JPEG reader at the largest size and a YUV frame
//! beside it. It prints the frame interval, the latency of the sensor
//! timestamps against CLOCK_BOOTTIME and the luma of the YUV frame's left
//! and right edges, and writes the JPEG to JPEG_PATH
//! (default /data/local/tmp/camera-client.jpg). Exit status 0 means every
//! step worked.

use std::ffi::{CStr, c_char, c_void};
use std::ptr::null_mut;
use std::time::{Duration, Instant};

type Status = i32;

#[repr(C)]
struct IdList {
    count: i32,
    ids: *const *const c_char,
}

#[repr(C)]
struct DeviceCallbacks {
    context: *mut c_void,
    on_disconnected: extern "C" fn(*mut c_void, *mut c_void),
    on_error: extern "C" fn(*mut c_void, *mut c_void, i32),
}

#[repr(C)]
struct SessionCallbacks {
    context: *mut c_void,
    on_closed: extern "C" fn(*mut c_void, *mut c_void),
    on_ready: extern "C" fn(*mut c_void, *mut c_void),
    on_active: extern "C" fn(*mut c_void, *mut c_void),
}

/// `ACameraMetadata_const_entry`.
#[repr(C)]
struct Entry {
    tag: u32,
    kind: u8,
    count: u32,
    data: *const c_void,
}

#[link(name = "camera2ndk")]
unsafe extern "C" {
    fn ACameraManager_create() -> *mut c_void;
    fn ACameraManager_delete(m: *mut c_void);
    fn ACameraManager_getCameraIdList(m: *mut c_void, list: *mut *mut IdList) -> Status;
    fn ACameraManager_getCameraCharacteristics(
        m: *mut c_void,
        id: *const c_char,
        out: *mut *mut c_void,
    ) -> Status;
    fn ACameraMetadata_getConstEntry(meta: *const c_void, tag: u32, out: *mut Entry) -> Status;
    fn ACameraMetadata_free(meta: *mut c_void);
    fn ACameraManager_openCamera(
        m: *mut c_void,
        id: *const c_char,
        cb: *mut DeviceCallbacks,
        out: *mut *mut c_void,
    ) -> Status;
    fn ACameraDevice_close(d: *mut c_void) -> Status;
    fn ACaptureSessionOutputContainer_create(out: *mut *mut c_void) -> Status;
    fn ACaptureSessionOutput_create(window: *mut c_void, out: *mut *mut c_void) -> Status;
    fn ACaptureSessionOutputContainer_add(c: *mut c_void, o: *const c_void) -> Status;
    fn ACameraDevice_createCaptureSession(
        d: *mut c_void,
        outputs: *const c_void,
        cb: *const SessionCallbacks,
        out: *mut *mut c_void,
    ) -> Status;
    fn ACameraCaptureSession_close(s: *mut c_void);
    fn ACameraDevice_createCaptureRequest(d: *mut c_void, t: i32, out: *mut *mut c_void) -> Status;
    fn ACameraOutputTarget_create(window: *mut c_void, out: *mut *mut c_void) -> Status;
    fn ACaptureRequest_addTarget(r: *mut c_void, t: *const c_void) -> Status;
    fn ACameraCaptureSession_capture(
        s: *mut c_void,
        cb: *mut c_void,
        n: i32,
        requests: *mut *mut c_void,
        seq: *mut i32,
    ) -> Status;
    fn ACameraCaptureSession_setRepeatingRequest(
        s: *mut c_void,
        cb: *mut c_void,
        n: i32,
        requests: *mut *mut c_void,
        seq: *mut i32,
    ) -> Status;
    fn ACameraCaptureSession_stopRepeating(s: *mut c_void) -> Status;
}

/// `AHardwareBuffer_Desc`.
#[repr(C)]
#[derive(Default)]
struct BufferDesc {
    width: u32,
    height: u32,
    layers: u32,
    format: u32,
    usage: u64,
    stride: u32,
    rfu0: u32,
    rfu1: u64,
}

#[link(name = "nativewindow")]
unsafe extern "C" {
    fn AHardwareBuffer_describe(b: *const c_void, out: *mut BufferDesc);
    fn AHardwareBuffer_lock(
        b: *mut c_void,
        usage: u64,
        fence: i32,
        rect: *const c_void,
        out: *mut *mut c_void,
    ) -> Status;
    fn AHardwareBuffer_unlock(b: *mut c_void, fence: *mut i32) -> Status;
}

#[link(name = "mediandk")]
unsafe extern "C" {
    fn AImageReader_newWithUsage(
        w: i32,
        h: i32,
        format: i32,
        usage: u64,
        max: i32,
        out: *mut *mut c_void,
    ) -> Status;
    fn AImage_getHardwareBuffer(i: *const c_void, out: *mut *mut c_void) -> Status;
    fn AImageReader_getWindow(r: *mut c_void, out: *mut *mut c_void) -> Status;
    fn AImageReader_acquireNextImage(r: *mut c_void, out: *mut *mut c_void) -> Status;
    fn AImageReader_delete(r: *mut c_void);
    fn AImage_getTimestamp(i: *const c_void, out: *mut i64) -> Status;
    fn AImage_getPlaneData(
        i: *const c_void,
        plane: i32,
        data: *mut *mut u8,
        len: *mut i32,
    ) -> Status;
    fn AImage_getPlaneRowStride(i: *const c_void, plane: i32, out: *mut i32) -> Status;
    fn AImage_delete(i: *mut c_void);
}

const ACAMERA_LENS_FACING: u32 = 0x8_0005;
const ACAMERA_INFO_SUPPORTED_HARDWARE_LEVEL: u32 = 0x15_0000;
const ACAMERA_SCALER_AVAILABLE_STREAM_CONFIGURATIONS: u32 = 0xd_000a;
const AIMAGE_FORMAT_YUV_420_888: i32 = 0x23;
const AIMAGE_FORMAT_JPEG: i32 = 0x100;
/// IMPLEMENTATION_DEFINED: a preview surface's format.
const AIMAGE_FORMAT_PRIVATE: i32 = 0x22;
const USAGE_CPU_READ_OFTEN: u64 = 3;
const USAGE_GPU_SAMPLED_IMAGE: u64 = 1 << 8;
const TEMPLATE_PREVIEW: i32 = 1;
const TEMPLATE_STILL_CAPTURE: i32 = 2;

extern "C" fn device_disconnected(_: *mut c_void, _: *mut c_void) {
    eprintln!("camera-client: device disconnected");
}
extern "C" fn device_error(_: *mut c_void, _: *mut c_void, e: i32) {
    eprintln!("camera-client: device error {e}");
}
extern "C" fn session_state(_: *mut c_void, _: *mut c_void) {}

fn check(what: &str, s: Status) {
    if s != 0 {
        eprintln!("camera-client: {what} failed: {s}");
        std::process::exit(1);
    }
}

fn boottime_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a local timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) };
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

struct Reader {
    reader: *mut c_void,
    window: *mut c_void,
}

impl Reader {
    fn new(w: i32, h: i32, format: i32, usage: u64) -> Reader {
        let mut r = Reader {
            reader: null_mut(),
            window: null_mut(),
        };
        // SAFETY: out-parameters.
        unsafe {
            check(
                "AImageReader_new",
                AImageReader_newWithUsage(w, h, format, usage, 4, &mut r.reader),
            );
            check(
                "AImageReader_getWindow",
                AImageReader_getWindow(r.reader, &mut r.window),
            );
        }
        r
    }

    /// The next image, waiting up to 5 s.
    fn next(&self) -> Option<*mut c_void> {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            let mut img = null_mut();
            // SAFETY: a live reader.
            if unsafe { AImageReader_acquireNextImage(self.reader, &mut img) } == 0 {
                return Some(img);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }
}

fn plane(img: *mut c_void, i: i32) -> (&'static [u8], usize) {
    let (mut data, mut len, mut stride) = (null_mut(), 0, 0);
    // SAFETY: a live image; the plane lives until AImage_delete, and the
    // caller uses it before that.
    unsafe {
        check(
            "AImage_getPlaneData",
            AImage_getPlaneData(img, i, &mut data, &mut len),
        );
        AImage_getPlaneRowStride(img, i, &mut stride);
        (
            std::slice::from_raw_parts(data, len as usize),
            stride as usize,
        )
    }
}

fn timestamp(img: *mut c_void) -> i64 {
    let mut ts = 0;
    // SAFETY: a live image.
    unsafe { AImage_getTimestamp(img, &mut ts) };
    ts
}

fn main() {
    let jpeg_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/data/local/tmp/camera-client.jpg".into());
    // SAFETY: the NDK camera API with out-parameters, used in order; every
    // object is released before exit or the process ends.
    unsafe {
        // The image readers' buffer queues are served from this process:
        // cameraserver dequeues from them over binder.
        binder::ProcessState::set_thread_pool_max_thread_count(4);
        binder::ProcessState::start_thread_pool();
        let mgr = ACameraManager_create();
        let mut list = null_mut();
        check(
            "getCameraIdList",
            ACameraManager_getCameraIdList(mgr, &mut list),
        );
        let ids: Vec<*const c_char> =
            std::slice::from_raw_parts((*list).ids, (*list).count as usize).to_vec();
        println!("cameras: {}", ids.len());
        let mut largest = (0, 0);
        for (n, &id) in ids.iter().enumerate() {
            let mut meta = null_mut();
            check(
                "getCameraCharacteristics",
                ACameraManager_getCameraCharacteristics(mgr, id, &mut meta),
            );
            let mut e = Entry {
                tag: 0,
                kind: 0,
                count: 0,
                data: std::ptr::null(),
            };
            ACameraMetadata_getConstEntry(meta, ACAMERA_LENS_FACING, &mut e);
            let facing = *(e.data as *const u8);
            ACameraMetadata_getConstEntry(meta, ACAMERA_INFO_SUPPORTED_HARDWARE_LEVEL, &mut e);
            let level = *(e.data as *const u8);
            ACameraMetadata_getConstEntry(
                meta,
                ACAMERA_SCALER_AVAILABLE_STREAM_CONFIGURATIONS,
                &mut e,
            );
            let configs = std::slice::from_raw_parts(e.data as *const i32, e.count as usize);
            let jpeg: Vec<(i32, i32)> = configs
                .chunks(4)
                .filter(|c| c[0] == AIMAGE_FORMAT_JPEG && c[3] == 0)
                .map(|c| (c[1], c[2]))
                .collect();
            if n == 0 {
                largest = jpeg
                    .iter()
                    .copied()
                    .max_by_key(|&(w, h)| w * h)
                    .unwrap_or((0, 0));
            }
            println!(
                "camera {:?}: facing {} (0 front, 1 back, 2 external), hardware level {} (0 LIMITED), JPEG sizes {:?}",
                CStr::from_ptr(id),
                facing,
                level,
                jpeg
            );
            ACameraMetadata_free(meta);
        }
        if ids.is_empty() {
            std::process::exit(1);
        }

        let mut cb = DeviceCallbacks {
            context: null_mut(),
            on_disconnected: device_disconnected,
            on_error: device_error,
        };
        let mut dev = null_mut();
        check(
            "openCamera",
            ACameraManager_openCamera(mgr, ids[0], &mut cb, &mut dev),
        );
        let yuv = Reader::new(640, 480, AIMAGE_FORMAT_YUV_420_888, USAGE_CPU_READ_OFTEN);
        let jpeg = Reader::new(
            largest.0,
            largest.1,
            AIMAGE_FORMAT_JPEG,
            USAGE_CPU_READ_OFTEN,
        );
        // A preview surface as a TextureView's: sampled by the GPU. The
        // CPU read lets this client check its pixels.
        let gpu = Reader::new(
            640,
            480,
            AIMAGE_FORMAT_PRIVATE,
            USAGE_GPU_SAMPLED_IMAGE | USAGE_CPU_READ_OFTEN,
        );
        let mut container = null_mut();
        check(
            "container",
            ACaptureSessionOutputContainer_create(&mut container),
        );
        for r in [&yuv, &jpeg, &gpu] {
            let mut out = null_mut();
            check("output", ACaptureSessionOutput_create(r.window, &mut out));
            check("add", ACaptureSessionOutputContainer_add(container, out));
        }
        let scb = SessionCallbacks {
            context: null_mut(),
            on_closed: session_state,
            on_ready: session_state,
            on_active: session_state,
        };
        let mut session = null_mut();
        check(
            "createCaptureSession",
            ACameraDevice_createCaptureSession(dev, container, &scb, &mut session),
        );
        let target = |r: &Reader| {
            let mut t = null_mut();
            check("target", ACameraOutputTarget_create(r.window, &mut t));
            t
        };

        // Preview: 30 frames of a repeating request.
        let mut preview = null_mut();
        check(
            "createCaptureRequest",
            ACameraDevice_createCaptureRequest(dev, TEMPLATE_PREVIEW, &mut preview),
        );
        check(
            "addTarget",
            ACaptureRequest_addTarget(preview, target(&yuv)),
        );
        check(
            "addTarget",
            ACaptureRequest_addTarget(preview, target(&gpu)),
        );
        let mut seq = 0;
        let start = Instant::now();
        check(
            "setRepeatingRequest",
            ACameraCaptureSession_setRepeatingRequest(
                session,
                null_mut(),
                1,
                &mut preview,
                &mut seq,
            ),
        );
        let mut stamps = Vec::new();
        let mut latency = Vec::new();
        for _ in 0..30 {
            let Some(img) = yuv.next() else {
                eprintln!(
                    "camera-client: no preview frame after {} frames",
                    stamps.len()
                );
                std::process::exit(1);
            };
            let ts = timestamp(img);
            latency.push(boottime_ns() - ts);
            stamps.push(ts);
            AImage_delete(img);
        }
        ACameraCaptureSession_stopRepeating(session);
        let first = start.elapsed();
        let intervals: Vec<i64> = stamps.windows(2).map(|w| w[1] - w[0]).collect();
        let mean = intervals.iter().sum::<i64>() as f64 / intervals.len() as f64 / 1e6;
        latency.sort();
        println!(
            "preview: 30 frames 640x480 YUV in {:.2} s; mean interval {:.1} ms ({:.1} fps); \
             timestamps increasing: {}; median latency vs CLOCK_BOOTTIME {:.1} ms",
            first.as_secs_f64(),
            mean,
            1000.0 / mean,
            intervals.iter().all(|&d| d > 0),
            latency[latency.len() / 2] as f64 / 1e6
        );
        // The GPU surface's latest frame, read through its hardware buffer.
        let Some(g) = gpu.next() else {
            eprintln!("camera-client: no frame on the GPU surface");
            std::process::exit(1);
        };
        let mut ahb = null_mut();
        check(
            "AImage_getHardwareBuffer",
            AImage_getHardwareBuffer(g, &mut ahb),
        );
        let mut desc = BufferDesc::default();
        AHardwareBuffer_describe(ahb, &mut desc);
        let mut addr = null_mut();
        check(
            "AHardwareBuffer_lock",
            AHardwareBuffer_lock(ahb, USAGE_CPU_READ_OFTEN, -1, std::ptr::null(), &mut addr),
        );
        let px = |x: usize, y: usize| {
            let p = (addr as *const u8).add((y * desc.stride as usize + x) * 4);
            std::slice::from_raw_parts(p, 4).to_vec()
        };
        println!(
            "gpu surface: {}x{} format {:#x} (1 RGBA, 2 RGBX), stride {}; left/right pixels of rows 60, 240, 420: {:?}",
            desc.width,
            desc.height,
            desc.format,
            desc.stride,
            [60, 240, 420].map(|y| (px(4, y), px(635, y)))
        );
        AHardwareBuffer_unlock(ahb, null_mut());
        AImage_delete(g);
        // Drain frames left from the repeating request.
        std::thread::sleep(Duration::from_millis(300));
        for r in [&yuv, &gpu] {
            while let Some(img) = {
                let mut img = null_mut();
                (AImageReader_acquireNextImage(r.reader, &mut img) == 0).then_some(img)
            } {
                AImage_delete(img);
            }
        }

        // Still capture: JPEG at the largest size, and YUV beside it.
        let mut still = null_mut();
        check(
            "createCaptureRequest",
            ACameraDevice_createCaptureRequest(dev, TEMPLATE_STILL_CAPTURE, &mut still),
        );
        check("addTarget", ACaptureRequest_addTarget(still, target(&jpeg)));
        check("addTarget", ACaptureRequest_addTarget(still, target(&yuv)));
        check(
            "capture",
            ACameraCaptureSession_capture(session, null_mut(), 1, &mut still, &mut seq),
        );
        let Some(j) = jpeg.next() else {
            eprintln!("camera-client: no JPEG");
            std::process::exit(1);
        };
        // The YUV image of the same capture, after any preview frames the
        // repeating request left in flight.
        let jts = timestamp(j);
        let y = loop {
            let Some(y) = yuv.next() else {
                eprintln!("camera-client: no YUV beside the JPEG");
                std::process::exit(1);
            };
            if timestamp(y) >= jts {
                break y;
            }
            AImage_delete(y);
        };
        let (data, _) = plane(j, 0);
        std::fs::write(&jpeg_path, data).expect("write the JPEG");
        let (luma, stride) = plane(y, 0);
        let row = &luma[240 * stride..][..640];
        let edge = |r: &[u8]| r.iter().map(|&v| v as u32).sum::<u32>() / r.len() as u32;
        println!(
            "still: JPEG {}x{} {} bytes (SOI {:02x}{:02x}) -> {}; timestamps equal: {}; \
             YUV row 240 luma: left {} right {}",
            largest.0,
            largest.1,
            data.len(),
            data[0],
            data[1],
            jpeg_path,
            timestamp(j) == timestamp(y),
            edge(&row[..32]),
            edge(&row[608..])
        );
        AImage_delete(j);
        AImage_delete(y);
        ACameraCaptureSession_close(session);
        ACameraDevice_close(dev);
        AImageReader_delete(yuv.reader);
        AImageReader_delete(jpeg.reader);
        AImageReader_delete(gpu.reader);
        ACameraManager_delete(mgr);
    }
    println!("ok");
}
