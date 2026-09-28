//! The application, and device mode's window: an `NSWindow` whose content
//! view hosts a `CAMetalLayer`, on the main thread.
//!
//! In device mode the display mode is the window's content in backing
//! pixels. By default the window fills the main screen's visible frame, so
//! the display has the screen's scale and density; `--size` asks for a
//! smaller one. Window mode's windows are in `windows.rs`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use aim_hostcall::display::mode;

use crate::objc::{CGRect, CGSize, Id, Sel, class, class_addMethod, nsstring, sel};
use crate::objc::{objc_allocateClassPair, objc_registerClassPair};

type CGDirectDisplayID = u32;

#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGMainDisplayID() -> CGDirectDisplayID;
    fn CGDisplayScreenSize(display: CGDirectDisplayID) -> CGSize;
    fn CGDisplayCopyDisplayMode(display: CGDirectDisplayID) -> *const c_void;
    fn CGDisplayModeGetPixelWidth(mode: *const c_void) -> usize;
    fn CGDisplayModeGetPixelHeight(mode: *const c_void) -> usize;
    fn CGDisplayModeRelease(mode: *const c_void);
    fn CGColorSpaceCreateWithName(name: *const c_void) -> *const c_void;
    fn CGColorSpaceRelease(space: *const c_void);
    static kCGColorSpaceSRGB: *const c_void;
}

pub const NS_WINDOW_STYLE: usize = 1 | 2 | 4 | 8; // titled, closable, miniaturizable, resizable
pub const NS_BACKING_STORE_BUFFERED: usize = 2;
const NS_APPLICATION_ACTIVATION_POLICY_REGULAR: isize = 0;
const MTL_PIXEL_FORMAT_BGRA8_UNORM: usize = 80;

/// The display server's mode, one of `aim_hostcall::display::mode`.
static MODE: AtomicU32 = AtomicU32::new(mode::DEVICE);
// NSActivityOptions
const NS_ACTIVITY_USER_INITIATED_ALLOWING_IDLE_SYSTEM_SLEEP: u64 = 0x00ff_ffff & !(1 << 20);
const NS_ACTIVITY_LATENCY_CRITICAL: u64 = 0xff_0000_0000;

/// The display the server shows, and device mode's window.
pub struct Window {
    /// The device window's layer.
    pub layer: Option<Id>,
    /// The display mode, in pixels.
    pub width: u32,
    pub height: u32,
    pub dpi_x: f64,
    pub dpi_y: f64,
    /// The device window's number; `screencapture -l` takes it.
    pub number: Option<isize>,
}

/// In device mode the server quits when its window closes; in window mode
/// windows come and go with tasks.
extern "C" fn quit_after_last_window(_this: Id, _sel: Sel, _app: Id) -> bool {
    MODE.load(Ordering::Relaxed) == mode::DEVICE
}

/// Quit when the window closes; the input devices go with the window.
fn app_delegate() -> Id {
    // SAFETY: registers a new NSObject subclass with two methods, once.
    let cls = unsafe {
        let cls = objc_allocateClassPair(class(c"NSObject"), c"DarwinDisplayDelegate".as_ptr(), 0);
        class_addMethod(
            cls,
            sel(c"applicationShouldTerminateAfterLastWindowClosed:"),
            quit_after_last_window as *const c_void,
            c"B@:@".as_ptr(),
        );
        class_addMethod(
            cls,
            sel(c"applicationWillTerminate:"),
            crate::input::will_terminate as *const c_void,
            c"v@:@".as_ptr(),
        );
        objc_registerClassPair(cls);
        cls
    };
    send!(cls, c"new" => Id)
}

/// The main display's density, in dots per inch of backing pixels.
pub fn main_display_dpi() -> (f64, f64) {
    // SAFETY: CoreGraphics queries on the main display; the mode is released.
    unsafe {
        let id = CGMainDisplayID();
        let mm = CGDisplayScreenSize(id);
        let mode = CGDisplayCopyDisplayMode(id);
        if mode.is_null() || mm.width <= 0.0 || mm.height <= 0.0 {
            return (160.0, 160.0);
        }
        let (w, h) = (
            CGDisplayModeGetPixelWidth(mode),
            CGDisplayModeGetPixelHeight(mode),
        );
        CGDisplayModeRelease(mode);
        (w as f64 * 25.4 / mm.width, h as f64 * 25.4 / mm.height)
    }
}

/// Create the application for `display_mode`. Main thread only.
pub fn app(display_mode: u32) {
    MODE.store(display_mode, Ordering::Relaxed);
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    send!(app, c"setActivationPolicy:" => bool, isize = NS_APPLICATION_ACTIVATION_POLICY_REGULAR);
    send!(app, c"setDelegate:" => (), Id = app_delegate());
    // The display link must keep its rate while the windows are in the
    // background: no App Nap, and latency-critical scheduling.
    let info = send!(class(c"NSProcessInfo"), c"processInfo" => Id);
    let activity = send!(info, c"beginActivityWithOptions:reason:" => Id,
        u64 = NS_ACTIVITY_USER_INITIATED_ALLOWING_IDLE_SYSTEM_SLEEP | NS_ACTIVITY_LATENCY_CRITICAL,
        Id = nsstring("presenting the Android display"));
    // Held for the life of the process.
    send!(activity, c"retain" => Id);
}

/// A new `CAMetalLayer` on `device` with drawables of `width` x `height`
/// pixels, shown at `scale` pixels per point and placed by `gravity`.
pub fn metal_layer(device: Id, width: u32, height: u32, scale: f64, gravity: &str) -> Id {
    let layer = send!(class(c"CAMetalLayer"), c"new" => Id);
    send!(layer, c"setDevice:" => (), Id = device);
    send!(layer, c"setPixelFormat:" => (), usize = MTL_PIXEL_FORMAT_BGRA8_UNORM);
    send!(layer, c"setFramebufferOnly:" => (), bool = true);
    send!(layer, c"setOpaque:" => (), bool = true);
    send!(layer, c"setDrawableSize:" => (), CGSize = CGSize {
        width: width as f64,
        height: height as f64,
    });
    send!(layer, c"setContentsScale:" => (), f64 = scale);
    send!(layer, c"setContentsGravity:" => (), Id = nsstring(gravity));
    // SAFETY: a named CoreGraphics color space, released once the layer
    // has retained it.
    let srgb = unsafe { CGColorSpaceCreateWithName(kCGColorSpaceSRGB) };
    send!(layer, c"setColorspace:" => (), *const c_void = srgb);
    // SAFETY: created above.
    unsafe { CGColorSpaceRelease(srgb) };
    layer
}

/// Create device mode's window. Main thread only.
pub fn create(size: Option<(u32, u32)>, title: &str, device: Id) -> Window {
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    let screen = send!(class(c"NSScreen"), c"mainScreen" => Id);
    let scale = send!(screen, c"backingScaleFactor" => f64);
    let visible = send!(screen, c"visibleFrame" => CGRect);
    let mut content = send!(class(c"NSWindow"), c"contentRectForFrameRect:styleMask:" => CGRect,
        CGRect = visible, usize = NS_WINDOW_STYLE);
    if let Some((w, h)) = size {
        content.size = CGSize {
            width: w as f64 / scale,
            height: h as f64 / scale,
        };
    }
    let (width, height) = (
        (content.size.width * scale) as u32,
        (content.size.height * scale) as u32,
    );

    let window = send!(class(c"NSWindow"), c"alloc" => Id);
    let window = send!(window,
        c"initWithContentRect:styleMask:backing:defer:" => Id,
        CGRect = content, usize = NS_WINDOW_STYLE, usize = NS_BACKING_STORE_BUFFERED, bool = false);
    send!(window, c"setTitle:" => (), Id = nsstring(title));
    if size.is_some() {
        send!(window, c"center" => ());
    }

    // The display keeps its mode when the window is resized; the layer
    // scales it to fit.
    let layer = metal_layer(device, width, height, scale, "resizeAspect");
    let view = crate::input::view(CGRect {
        size: content.size,
        ..Default::default()
    });
    send!(window, c"setContentView:" => (), Id = view);
    send!(view, c"setLayer:" => (), Id = layer);
    send!(view, c"setWantsLayer:" => (), bool = true);
    send!(window, c"makeFirstResponder:" => bool, Id = view);
    send!(window, c"setDelegate:" => (), Id = crate::input::window_delegate());

    send!(window, c"makeKeyAndOrderFront:" => (), Id = std::ptr::null_mut());
    send!(app, c"activateIgnoringOtherApps:" => (), bool = true);
    let (dpi_x, dpi_y) = main_display_dpi();
    Window {
        layer: Some(layer),
        width,
        height,
        dpi_x,
        dpi_y,
        number: Some(send!(window, c"windowNumber" => isize)),
    }
}

/// Run AppKit's event loop. Never returns.
pub fn run() -> ! {
    let app = send!(class(c"NSApplication"), c"sharedApplication" => Id);
    send!(app, c"run" => ());
    std::process::exit(0)
}
