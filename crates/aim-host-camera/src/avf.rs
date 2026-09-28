//! AVFoundation: the Mac's cameras, their formats, the camera permission,
//! and capture sessions that deliver BGRA frames on a dispatch queue.
//!
//! The frameworks are opened on first use, so guest processes that never
//! touch a camera never load them.

use std::collections::HashMap;
use std::ffi::{CStr, c_void};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use aim_hostcall::camera::{Size, access, facing};

use crate::convert::Bgra;
use crate::objc::{self, Framework, GlobalBlock, Id, NIL, Obj, Pool, Queue, Sel, send};
use crate::{Latest, clock};

/// `CMTime`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CmTime {
    pub value: i64,
    pub timescale: i32,
    pub flags: u32,
    pub epoch: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Dimensions {
    width: i32,
    height: i32,
}

const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const CM_TIME_VALID: u32 = 1;
const LOCK_READ_ONLY: u64 = 1;

/// The C functions and constants of the frameworks.
pub struct Api {
    pub media_type_video: Id,
    pixel_format_key: Id,
    device_types: Vec<Id>,
    wide_angle: Id,
    get_image_buffer: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    get_pts: unsafe extern "C" fn(*mut c_void) -> CmTime,
    host_time_to_system: unsafe extern "C" fn(CmTime) -> u64,
    dimensions: unsafe extern "C" fn(*mut c_void) -> Dimensions,
    lock: unsafe extern "C" fn(*mut c_void, u64) -> i32,
    unlock: unsafe extern "C" fn(*mut c_void, u64) -> i32,
    base: unsafe extern "C" fn(*mut c_void) -> *const u8,
    bytes_per_row: unsafe extern "C" fn(*mut c_void) -> usize,
    width: unsafe extern "C" fn(*mut c_void) -> usize,
    height: unsafe extern "C" fn(*mut c_void) -> usize,
    pixel_format: unsafe extern "C" fn(*mut c_void) -> u32,
    pub jpeg: crate::jpeg::Api,
}

// SAFETY: function pointers and immortal framework constants.
unsafe impl Send for Api {}
unsafe impl Sync for Api {}

macro_rules! resolve {
    ($fw:expr, $name:literal) => {
        // SAFETY: the exported function has the declared signature.
        unsafe { $fw.function($name)? }
    };
}
pub(crate) use resolve;

impl Api {
    fn load() -> Option<Api> {
        let fw = |p: &CStr| Framework::open(p);
        let avf = fw(c"/System/Library/Frameworks/AVFoundation.framework/AVFoundation")?;
        let cm = fw(c"/System/Library/Frameworks/CoreMedia.framework/CoreMedia")?;
        let cv = fw(c"/System/Library/Frameworks/CoreVideo.framework/CoreVideo")?;
        let device_types = [
            "AVCaptureDeviceTypeBuiltInWideAngleCamera",
            "AVCaptureDeviceTypeExternal",
            "AVCaptureDeviceTypeContinuityCamera",
        ]
        .iter()
        .map(|n| avf.constant(n))
        .filter(|c| !c.is_null())
        .collect();
        let api = Api {
            media_type_video: avf.constant("AVMediaTypeVideo"),
            pixel_format_key: cv.constant("kCVPixelBufferPixelFormatTypeKey"),
            device_types,
            wide_angle: avf.constant("AVCaptureDeviceTypeBuiltInWideAngleCamera"),
            get_image_buffer: resolve!(cm, "CMSampleBufferGetImageBuffer"),
            get_pts: resolve!(cm, "CMSampleBufferGetPresentationTimeStamp"),
            host_time_to_system: resolve!(cm, "CMClockConvertHostTimeToSystemUnits"),
            dimensions: resolve!(cm, "CMVideoFormatDescriptionGetDimensions"),
            lock: resolve!(cv, "CVPixelBufferLockBaseAddress"),
            unlock: resolve!(cv, "CVPixelBufferUnlockBaseAddress"),
            base: resolve!(cv, "CVPixelBufferGetBaseAddress"),
            bytes_per_row: resolve!(cv, "CVPixelBufferGetBytesPerRow"),
            width: resolve!(cv, "CVPixelBufferGetWidth"),
            height: resolve!(cv, "CVPixelBufferGetHeight"),
            pixel_format: resolve!(cv, "CVPixelBufferGetPixelFormatType"),
            jpeg: crate::jpeg::Api::load()?,
        };
        if api.media_type_video.is_null() || api.pixel_format_key.is_null() {
            crate::log!("AVFoundation constants are missing");
            return None;
        }
        Some(api)
    }
}

/// The frameworks, or `None` when this macOS lacks them.
pub fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(Api::load).as_ref()
}

/// Camera access for this process (its responsible app, for TCC).
pub fn access_status(api: &Api) -> u32 {
    let _pool = Pool::new();
    let s = send!(objc::class(c"AVCaptureDevice"),
        c"authorizationStatusForMediaType:" => isize, Id = api.media_type_video);
    match s {
        0 => access::NOT_DETERMINED,
        1 => access::RESTRICTED,
        2 => access::DENIED,
        _ => access::AUTHORIZED,
    }
}

/// Ask macOS for camera access once per process: the TCC prompt,
/// attributed to the app responsible for this process. The answer comes
/// later; [`access_status`] reports it.
pub fn request_access(api: &Api) {
    extern "C" fn answered(_block: *const GlobalBlock, granted: bool) {
        crate::log!(
            "camera access {}",
            if granted { "granted" } else { "denied" }
        );
    }
    static BLOCK: OnceLock<GlobalBlock> = OnceLock::new();
    static ASKED: OnceLock<()> = OnceLock::new();
    ASKED.get_or_init(|| {
        crate::log!("asking macOS for camera access (the TCC prompt, attributed to the terminal)");
        let block = BLOCK.get_or_init(|| {
            type F = extern "C" fn(*const GlobalBlock, bool);
            GlobalBlock::new(answered as F as *const c_void)
        });
        let _pool = Pool::new();
        send!(objc::class(c"AVCaptureDevice"),
            c"requestAccessForMediaType:completionHandler:" => (),
            Id = api.media_type_video, *const GlobalBlock = block);
    });
}

/// A camera as the guest sees it.
pub struct Camera {
    pub id: String,
    pub name: String,
    pub facing: u32,
    /// Landscape sizes, largest first.
    pub sizes: Vec<Size>,
    /// macOS reports the camera suspended (a closed lid).
    pub suspended: bool,
    device: Obj,
}

fn retain(obj: Id) -> Obj {
    Obj(send!(obj, c"retain" => Id))
}

fn format_size(api: &Api, format: Id) -> (u32, u32, u32) {
    let desc = send!(format, c"formatDescription" => *mut c_void);
    // SAFETY: a CMVideoFormatDescription.
    let d = unsafe { (api.dimensions)(desc) };
    let fps = objc::array(send!(format, c"videoSupportedFrameRateRanges" => Id))
        .into_iter()
        .map(|r| send!(r, c"maxFrameRate" => f64))
        .fold(0.0, f64::max);
    (
        d.width.max(0) as u32,
        d.height.max(0) as u32,
        (fps + 0.01) as u32,
    )
}

/// Distinct landscape sizes of `formats`, largest first, each with its
/// highest frame rate.
pub fn sizes_of(formats: impl IntoIterator<Item = (u32, u32, u32)>) -> Vec<Size> {
    let mut best: HashMap<(u32, u32), u32> = HashMap::new();
    for (w, h, fps) in formats {
        if w >= h && h > 0 {
            let e = best.entry((w, h)).or_default();
            *e = (*e).max(fps);
        }
    }
    let mut sizes: Vec<Size> = best
        .into_iter()
        .map(|((width, height), max_fps)| Size {
            width,
            height,
            max_fps,
            reserved: 0,
        })
        .collect();
    sizes.sort_by_key(|s| {
        (
            std::cmp::Reverse(s.width as u64 * s.height as u64),
            std::cmp::Reverse(s.width),
        )
    });
    sizes
}

/// The cameras, built-in first, then by unique id.
pub fn cameras(api: &Api) -> Vec<Camera> {
    let _pool = Pool::new();
    let types = objc::nsarray(&api.device_types);
    let discovery = send!(objc::class(c"AVCaptureDeviceDiscoverySession"),
        c"discoverySessionWithDeviceTypes:mediaType:position:" => Id,
        Id = types, Id = api.media_type_video, isize = 0);
    let responds =
        |obj: Id, s: &CStr| send!(obj, c"respondsToSelector:" => bool, Sel = objc::sel(s));
    let mut cams: Vec<Camera> = objc::array(send!(discovery, c"devices" => Id))
        .into_iter()
        .map(|d| {
            let kind = send!(d, c"deviceType" => Id);
            let built_in = send!(kind, c"isEqualToString:" => bool, Id = api.wide_angle);
            let position = send!(d, c"position" => isize);
            let formats = objc::array(send!(d, c"formats" => Id));
            Camera {
                id: objc::string(send!(d, c"uniqueID" => Id)),
                name: objc::string(send!(d, c"localizedName" => Id)),
                facing: if built_in {
                    facing::FRONT
                } else if position == 1 {
                    facing::BACK
                } else {
                    facing::EXTERNAL
                },
                sizes: sizes_of(formats.into_iter().map(|f| format_size(api, f))),
                suspended: responds(d, c"isSuspended") && send!(d, c"isSuspended" => bool),
                device: retain(d),
            }
        })
        .filter(|c| !c.sizes.is_empty())
        .collect();
    cams.sort_by(|a, b| {
        (a.facing != facing::FRONT, &a.id).cmp(&(b.facing != facing::FRONT, &b.id))
    });
    cams
}

/// A running `AVCaptureSession` feeding [`Latest`].
pub struct Capture {
    session: Obj,
    _delegate: Obj,
    queue: Queue,
}

// SAFETY: AVCaptureSession may be stopped from any thread; the queue
// handle is thread-safe.
unsafe impl Send for Capture {}
unsafe impl Sync for Capture {}

const DELEGATE_CLASS: &CStr = c"DarwinLinuxCameraDelegate";

/// Frame sinks by delegate address, for the callback.
static SINKS: Mutex<Vec<(usize, Weak<Latest>)>> = Mutex::new(Vec::new());

extern "C" fn did_output(this: Id, _: Sel, _output: Id, sample: *mut c_void, _connection: Id) {
    let Some(latest) = SINKS
        .lock()
        .unwrap()
        .iter()
        .find(|(d, _)| *d == this as usize)
        .and_then(|(_, w)| w.upgrade())
    else {
        return;
    };
    let Some(api) = api() else { return };
    // SAFETY: a CMSampleBuffer the callback lends us; its image buffer is
    // locked while it is read.
    unsafe {
        let pb = (api.get_image_buffer)(sample);
        if pb.is_null() || (api.pixel_format)(pb) != BGRA {
            return;
        }
        let pts = (api.get_pts)(sample);
        let ns = if pts.flags & CM_TIME_VALID != 0 {
            clock::host_to_ns((api.host_time_to_system)(pts))
        } else {
            clock::now_ns()
        };
        if (api.lock)(pb, LOCK_READ_ONLY) != 0 {
            return;
        }
        let (w, h, stride) = ((api.width)(pb), (api.height)(pb), (api.bytes_per_row)(pb));
        let base = (api.base)(pb);
        if !base.is_null() && w > 0 && h > 0 {
            let mut f = Bgra::new(w as u32, h as u32);
            for y in 0..h {
                let src = std::slice::from_raw_parts(base.add(y * stride), w * 4);
                f.data[y * w * 4..][..w * 4].copy_from_slice(src);
            }
            latest.publish(f, ns);
        }
        (api.unlock)(pb, LOCK_READ_ONLY);
    }
}

fn delegate_class() -> objc::Class {
    static CLASS: OnceLock<usize> = OnceLock::new();
    *CLASS.get_or_init(|| {
        // SAFETY: a new class under a name only this module uses, with an
        // IMP of the selector's signature.
        unsafe {
            let cls =
                objc::objc_allocateClassPair(objc::class(c"NSObject"), DELEGATE_CLASS.as_ptr(), 0);
            type F = extern "C" fn(Id, Sel, Id, *mut c_void, Id);
            objc::class_addMethod(
                cls,
                objc::sel(c"captureOutput:didOutputSampleBuffer:fromConnection:"),
                did_output as F as *const c_void,
                c"v@:@^v@".as_ptr(),
            );
            objc::objc_registerClassPair(cls);
            cls as usize
        }
    }) as objc::Class
}

/// The device format to capture `width` × `height` at `fps`: the smallest
/// that covers the size at that rate, else the largest.
fn pick_format(api: &Api, device: Id, width: u32, height: u32, fps: u32) -> Option<Id> {
    let formats = objc::array(send!(device, c"formats" => Id));
    let sized: Vec<(Id, (u32, u32, u32))> = formats
        .into_iter()
        .map(|f| (f, format_size(api, f)))
        .filter(|(_, (w, h, _))| w >= h)
        .collect();
    let area = |(w, h, _): (u32, u32, u32)| w as u64 * h as u64;
    sized
        .iter()
        .filter(|(_, (w, h, r))| *w >= width && *h >= height && *r >= fps)
        .min_by_key(|(_, s)| area(*s))
        .or_else(|| sized.iter().max_by_key(|(_, s)| area(*s)))
        .map(|(f, _)| *f)
}

impl Capture {
    /// Capture from `camera` into `latest`, at least `width` × `height`.
    pub fn start(
        api: &Api,
        camera: &Camera,
        width: u32,
        height: u32,
        fps: u32,
        latest: &Arc<Latest>,
    ) -> Result<Capture, String> {
        let _pool = Pool::new();
        let device = camera.device.0;
        let mut error: Id = NIL;
        let input = send!(objc::class(c"AVCaptureDeviceInput"),
            c"deviceInputWithDevice:error:" => Id, Id = device, *mut Id = &mut error);
        if input.is_null() {
            return Err(format!(
                "no capture input: {}",
                objc::localized_description(error)
            ));
        }
        let session = send!(objc::class(c"AVCaptureSession"), c"alloc" => Id);
        let session = Obj(send!(session, c"init" => Id));
        send!(session.0, c"beginConfiguration" => ());
        if !send!(session.0, c"canAddInput:" => bool, Id = input) {
            return Err("the session cannot take the camera".into());
        }
        send!(session.0, c"addInput:" => (), Id = input);

        let output = send!(objc::class(c"AVCaptureVideoDataOutput"), c"alloc" => Id);
        let output = Obj(send!(output, c"init" => Id));
        let settings = objc::dictionary(&[(api.pixel_format_key, objc::number_u32(BGRA))]);
        send!(output.0, c"setVideoSettings:" => (), Id = settings);
        send!(output.0, c"setAlwaysDiscardsLateVideoFrames:" => (), bool = true);
        let delegate = send!(delegate_class(), c"alloc" => Id);
        let delegate = Obj(send!(delegate, c"init" => Id));
        // SAFETY: a serial queue with a static label.
        let queue = unsafe {
            objc::dispatch_queue_create(c"dev.aim.linux-abi.camera".as_ptr(), std::ptr::null())
        };
        SINKS
            .lock()
            .unwrap()
            .push((delegate.0 as usize, Arc::downgrade(latest)));
        send!(output.0, c"setSampleBufferDelegate:queue:" => (), Id = delegate.0, Queue = queue);
        let capture = Capture {
            session,
            _delegate: delegate,
            queue,
        };
        if !send!(capture.session.0, c"canAddOutput:" => bool, Id = output.0) {
            return Err("the session cannot take a video output".into());
        }
        send!(capture.session.0, c"addOutput:" => (), Id = output.0);

        if let Some(format) = pick_format(api, device, width, height, fps)
            && send!(device, c"lockForConfiguration:" => bool, *mut Id = &mut error)
        {
            send!(device, c"setActiveFormat:" => (), Id = format);
            let (_, _, max) = format_size(api, format);
            if max >= fps && fps > 0 {
                let d = CmTime {
                    value: 1,
                    timescale: fps as i32,
                    flags: CM_TIME_VALID,
                    epoch: 0,
                };
                send!(device, c"setActiveVideoMinFrameDuration:" => (), CmTime = d);
                send!(device, c"setActiveVideoMaxFrameDuration:" => (), CmTime = d);
            }
            send!(device, c"unlockForConfiguration" => ());
        }
        send!(capture.session.0, c"commitConfiguration" => ());
        send!(capture.session.0, c"startRunning" => ());
        if !send!(capture.session.0, c"isRunning" => bool) {
            return Err("the capture session did not start".into());
        }
        Ok(capture)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _pool = Pool::new();
        send!(self.session.0, c"stopRunning" => ());
        let d = self._delegate.0 as usize;
        SINKS
            .lock()
            .unwrap()
            .retain(|(k, w)| *k != d && w.strong_count() > 0);
        // SAFETY: our queue; the session no longer uses it.
        unsafe { objc::dispatch_release(self.queue) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_landscape_distinct_and_largest_first() {
        let s = sizes_of([
            (640, 480, 30),
            (1920, 1080, 30),
            (1080, 1920, 30),
            (640, 480, 60),
            (1280, 720, 30),
        ]);
        let got: Vec<_> = s.iter().map(|s| (s.width, s.height, s.max_fps)).collect();
        assert_eq!(got, [(1920, 1080, 30), (1280, 720, 30), (640, 480, 60)]);
    }

    #[test]
    fn lists_the_cameras_without_asking() {
        // Enumeration and the access status need no permission and show
        // no prompt.
        let Some(api) = api() else { return };
        eprintln!("access {}", access_status(api));
        for c in cameras(api) {
            eprintln!(
                "{} {:?} facing {} suspended {} sizes {:?}",
                c.id,
                c.name,
                c.facing,
                c.suspended,
                c.sizes
                    .iter()
                    .map(|s| (s.width, s.height, s.max_fps))
                    .collect::<Vec<_>>()
            );
            assert!(!c.sizes.is_empty());
        }
    }
}
