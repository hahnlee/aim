//! The display's hardware cursor is the Mac's cursor (`docs/input.md`,
//! "The pointer icon").
//!
//! The composer HAL passes on Android's `CURSOR` layer, the pointer's
//! sprite: its buffer and where it lies. The server reads the buffer's
//! pixels once its content is ready, finds the hot spot from the mouse
//! (`input::cursor`) and shows the image as an `NSCursor` over its views
//! and the window hosts' (whatever icon Android draws: its arrow, I-beam,
//! hand, resize arrows, or an app's own bitmap). Without a sprite the
//! views show the arrow. The image is display pixels: its size in points
//! is its pixels over the view's pixels per point, so a Retina screen
//! shows the sprite's own pixels.

use std::hash::{Hash, Hasher};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::{Arc, Mutex};

use crate::metal::Texture;
use crate::objc::{CGPoint, CGSize, Id, Sel, class, nsstring, on_main};

/// A cursor image: premultiplied RGBA rows, `width * 4` bytes each.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    /// The hot spot, in pixels from the top left.
    pub hot: (i32, i32),
}

/// The server's current sprite: its buffer and the image read from it.
struct Sprite {
    id: u64,
    key: u64,
    image: Arc<Image>,
}

static SPRITE: Mutex<Option<Sprite>> = Mutex::new(None);

/// `PixelFormat::RGBA_8888`, the sprite's format.
const RGBA_8888: i32 = 1;

/// The composer's cursor: buffer `id` (0: none), `t` if the server has
/// it, at display pixel `(x, y)`; its content new (`changed`) once
/// `acquire` signals.
pub fn serve(
    id: u64,
    t: Option<&Arc<Texture>>,
    changed: bool,
    acquire: Option<OwnedFd>,
    x: i32,
    y: i32,
) {
    let mut s = SPRITE.lock().unwrap();
    let mut fresh = false;
    if id == 0 || changed || s.as_ref().is_none_or(|c| c.id != id) {
        let Some(t) = t.filter(|t| id != 0 && t.import.format == RGBA_8888) else {
            if s.take().is_some() {
                publish(None);
            }
            return;
        };
        if let Some(a) = acquire
            && !aim_sync_file::wait(a.as_fd(), crate::ACQUIRE_TIMEOUT_MS)
        {
            eprintln!("aim-display: cursor's acquire fence not signaled");
        }
        let i = &t.import;
        let row = i.width as usize * 4;
        let pixels: Vec<u8> = (0..i.height as usize)
            .flat_map(|y| &t.pixels()[y * i.stride_bytes as usize..][..row])
            .copied()
            .collect();
        let mut hasher = std::hash::DefaultHasher::new();
        (i.width, i.height, &pixels).hash(&mut hasher);
        let key = hasher.finish();
        fresh = s.as_ref().is_none_or(|c| c.key != key);
        let hot = s.as_ref().map_or((0, 0), |c| c.image.hot);
        let image = Arc::new(Image {
            width: i.width,
            height: i.height,
            pixels,
            hot,
        });
        *s = Some(Sprite { id, key, image });
    }
    let c = s.as_mut().unwrap();
    let (w, h) = (c.image.width as i32, c.image.height as i32);
    let Some(hot) = crate::input::input().map(|input| input.hotspot(c.key, w, h, x, y)) else {
        return;
    };
    if fresh || hot != c.image.hot {
        c.image = Arc::new(Image {
            width: c.image.width,
            height: c.image.height,
            pixels: c.image.pixels.clone(),
            hot,
        });
        publish(Some(c.image.clone()));
    }
}

/// Show `image` in this process's views and the window hosts'.
fn publish(image: Option<Arc<Image>>) {
    crate::hosts::cursor(image.as_deref());
    on_main(move || show(image));
}

/// What this process's views show (main thread).
struct Shown {
    image: Option<Arc<Image>>,
    /// Display pixels per point of the view under the pointer.
    scale: f64,
    /// The `NSCursor` made of them, retained, or null for the arrow.
    cursor: usize,
    /// The pointer is over one of the views.
    inside: bool,
}

static SHOWN: Mutex<Shown> = Mutex::new(Shown {
    image: None,
    scale: 0.0,
    cursor: 0,
    inside: false,
});

/// Show `image` (None: the arrow) over the views, from the main thread.
pub fn show(image: Option<Arc<Image>>) {
    let mut s = SHOWN.lock().unwrap();
    s.image = image;
    remake(&mut s);
}

/// The pointer is over a view showing `scale` display pixels per point
/// (main thread).
pub fn entered(scale: f64) {
    let mut s = SHOWN.lock().unwrap();
    let was = s.inside;
    s.inside = true;
    if s.scale != scale {
        s.scale = scale;
        remake(&mut s);
    } else if !was {
        set(&s);
    }
}

/// The pointer left the views (main thread).
pub fn exited() {
    SHOWN.lock().unwrap().inside = false;
}

/// `cursorUpdate:` of a view: its cursor is the sprite.
pub extern "C" fn cursor_update(_: Id, _: Sel, _event: Id) {
    set(&SHOWN.lock().unwrap());
}

fn set(s: &Shown) {
    let cursor = if s.cursor != 0 {
        s.cursor as Id
    } else {
        send!(class(c"NSCursor"), c"arrowCursor" => Id)
    };
    send!(cursor, c"set" => ());
}

/// Make the `NSCursor` for the image at the scale, and show it while the
/// pointer is over a view.
fn remake(s: &mut Shown) {
    if s.cursor != 0 {
        send!(s.cursor as Id, c"release" => ());
    }
    s.cursor = match &s.image {
        Some(i) if s.scale > 0.0 && i.width > 0 && i.height > 0 => ns_cursor(i, s.scale) as usize,
        _ => 0,
    };
    if s.inside {
        set(s);
    }
}

/// An `NSCursor` of `i`, `scale` pixels per point; retained.
fn ns_cursor(i: &Image, scale: f64) -> Id {
    let (w, h) = (i.width as isize, i.height as isize);
    let rep = send!(class(c"NSBitmapImageRep"), c"alloc" => Id);
    let rep = send!(rep,
        c"initWithBitmapDataPlanes:pixelsWide:pixelsHigh:bitsPerSample:samplesPerPixel:hasAlpha:isPlanar:colorSpaceName:bytesPerRow:bitsPerPixel:" => Id,
        *mut *mut u8 = std::ptr::null_mut(), isize = w, isize = h, isize = 8, isize = 4,
        bool = true, bool = false, Id = nsstring("NSDeviceRGBColorSpace"), isize = w * 4,
        isize = 32);
    let data = send!(rep, c"bitmapData" => *mut u8);
    // SAFETY: the representation owns `w * h * 4` bytes of planar-free
    // RGBA, the image's size.
    unsafe { std::ptr::copy_nonoverlapping(i.pixels.as_ptr(), data, i.pixels.len()) };
    let size = CGSize {
        width: w as f64 / scale,
        height: h as f64 / scale,
    };
    send!(rep, c"setSize:" => (), CGSize = size);
    let image = send!(class(c"NSImage"), c"alloc" => Id);
    let image = send!(image, c"initWithSize:" => Id, CGSize = size);
    send!(image, c"addRepresentation:" => (), Id = rep);
    send!(rep, c"release" => ());
    let hot = CGPoint {
        x: i.hot.0 as f64 / scale,
        y: i.hot.1 as f64 / scale,
    };
    let cursor = send!(class(c"NSCursor"), c"alloc" => Id);
    let cursor = send!(cursor, c"initWithImage:hotSpot:" => Id, Id = image, CGPoint = hot);
    send!(image, c"release" => ());
    cursor
}
