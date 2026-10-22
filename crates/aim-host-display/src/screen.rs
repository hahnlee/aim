//! Whether the Mac shows windows now: while its screen is locked or its
//! main display asleep, Core Animation shows no window's drawable, and a
//! drawable presented then waits, its presented handler unrun, until the
//! screen shows windows again. The same holds for one window whose content
//! other windows cover entirely ([`covered`]), though AppKit counts it
//! visible while its title bar shows.

use std::ffi::c_void;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CGRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGSessionCopyCurrentDictionary() -> *const c_void;
    fn CGMainDisplayID() -> u32;
    fn CGDisplayIsAsleep(display: u32) -> u32;
    fn CGWindowListCopyWindowInfo(option: u32, relative_to: u32) -> *const c_void;
    fn CGRectMakeWithDictionaryRepresentation(dict: *const c_void, rect: *mut CGRect) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDictionaryGetValue(dict: *const c_void, key: *const c_void) -> *const c_void;
    fn CFStringCreateWithCString(
        alloc: *const c_void,
        s: *const std::ffi::c_char,
        encoding: u32,
    ) -> *const c_void;
    fn CFBooleanGetValue(b: *const c_void) -> bool;
    fn CFNumberGetValue(number: *const c_void, kind: isize, value: *mut c_void) -> bool;
    fn CFArrayGetCount(array: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(array: *const c_void, index: isize) -> *const c_void;
    fn CFRelease(obj: *const c_void);
}

const UTF8: u32 = 0x0800_0100;
/// `kCGWindowListOptionOnScreenOnly`, `kCGWindowListOptionOnScreenAboveWindow`,
/// `kCGWindowListExcludeDesktopElements`.
const ON_SCREEN_ONLY: u32 = 1 << 0;
const ON_SCREEN_ABOVE_WINDOW: u32 = 1 << 1;
const EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
/// `kCFNumberSInt64Type`, `kCFNumberFloat64Type`.
const SINT64: isize = 4;
const FLOAT64: isize = 6;

/// A key of a window info dictionary.
struct Key(*const c_void);

impl Key {
    fn new(name: &std::ffi::CStr) -> Key {
        // SAFETY: creates a string that `drop` releases.
        Key(unsafe { CFStringCreateWithCString(std::ptr::null(), name.as_ptr(), UTF8) })
    }

    /// The dictionary's number under this key, as `kind` into `T`.
    fn number<T: Default>(&self, dict: *const c_void, kind: isize) -> Option<T> {
        let mut out = T::default();
        // SAFETY: `dict` is a live window info dictionary; the value is a
        // CFNumber read into `out` as `kind`, which `T` matches.
        unsafe {
            let v = CFDictionaryGetValue(dict, self.0);
            (!v.is_null() && CFNumberGetValue(v, kind, (&mut out as *mut T).cast())).then_some(out)
        }
    }

    fn rect(&self, dict: *const c_void) -> Option<CGRect> {
        let mut rect = CGRect::default();
        // SAFETY: as above; the value is a bounds dictionary.
        unsafe {
            let v = CFDictionaryGetValue(dict, self.0);
            (!v.is_null() && CGRectMakeWithDictionaryRepresentation(v, &mut rect)).then_some(rect)
        }
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: created in `new`.
        unsafe { CFRelease(self.0) };
    }
}

/// The window list of `option`, `relative_to` a window, each entry's info
/// passed to `f`; only the entries' numbers, bounds and alpha are read.
fn windows(option: u32, relative_to: u32, mut f: impl FnMut(*const c_void)) {
    // SAFETY: a copied array, released below; its entries live with it.
    unsafe {
        let list = CGWindowListCopyWindowInfo(option, relative_to);
        if list.is_null() {
            return;
        }
        for i in 0..CFArrayGetCount(list) {
            f(CFArrayGetValueAtIndex(list, i));
        }
        CFRelease(list);
    }
}

/// Whether the content of process `pid`'s on-screen window, the
/// `width` x `height` points at the bottom of its frame (below the title
/// bar), is covered entirely by windows above it, so Core Animation shows
/// none of it; `None` when `pid` shows no such window. Of the windows above
/// it, only their bounds and alpha are read.
pub fn covered(pid: i32, width: f64, height: f64) -> Option<bool> {
    let (owner, number, bounds, alpha) = (
        Key::new(c"kCGWindowOwnerPID"),
        Key::new(c"kCGWindowNumber"),
        Key::new(c"kCGWindowBounds"),
        Key::new(c"kCGWindowAlpha"),
    );
    let mut own = None;
    windows(ON_SCREEN_ONLY | EXCLUDE_DESKTOP_ELEMENTS, 0, |w| {
        if own.is_none()
            && owner.number::<i64>(w, SINT64) == Some(pid as i64)
            && let (Some(n), Some(b)) = (number.number::<i64>(w, SINT64), bounds.rect(w))
            && b.width >= width
            && b.height >= height
        {
            own = Some((n as u32, b));
        }
    });
    let (n, frame) = own?;
    let mut above = Vec::new();
    windows(ON_SCREEN_ABOVE_WINDOW | EXCLUDE_DESKTOP_ELEMENTS, n, |w| {
        if alpha.number::<f64>(w, FLOAT64).unwrap_or(1.0) > 0.0
            && let Some(b) = bounds.rect(w)
        {
            above.push(b);
        }
    });
    let content = CGRect {
        x: frame.x,
        y: frame.y + frame.height - height,
        width,
        height,
    };
    Some(covers(&above, content))
}

/// Whether `rects` together cover `area`: every point of it, sampled at
/// each point's centre, lies in one of them.
fn covers(rects: &[CGRect], area: CGRect) -> bool {
    let inside = |r: &CGRect, x: f64, y: f64| {
        x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
    };
    (0..area.height.ceil() as u32).all(|j| {
        (0..area.width.ceil() as u32).all(|i| {
            let (x, y) = (area.x + i as f64 + 0.5, area.y + j as f64 + 0.5);
            rects.iter().any(|r| inside(r, x, y))
        })
    })
}

/// Why the Mac shows no windows now, if it does not: its main display is
/// asleep, or its screen is locked.
pub fn hidden() -> Option<&'static str> {
    // SAFETY: plain queries; the session dictionary and key are released.
    unsafe {
        if CGDisplayIsAsleep(CGMainDisplayID()) != 0 {
            return Some("the Mac's main display is asleep");
        }
        let session = CGSessionCopyCurrentDictionary();
        if session.is_null() {
            return Some("no window server session");
        }
        let key =
            CFStringCreateWithCString(std::ptr::null(), c"CGSSessionScreenIsLocked".as_ptr(), UTF8);
        let v = CFDictionaryGetValue(session, key);
        let locked = !v.is_null() && CFBooleanGetValue(v);
        CFRelease(key);
        CFRelease(session);
        locked.then_some("the Mac's screen is locked")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> CGRect {
        CGRect {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn an_area_is_covered_only_by_all_of_it() {
        let area = rect(10.0, 10.0, 32.0, 24.0);
        assert!(covers(&[rect(0.0, 0.0, 100.0, 100.0)], area));
        assert!(covers(
            &[rect(0.0, 0.0, 26.0, 50.0), rect(26.0, 0.0, 30.0, 50.0)],
            area
        ));
        assert!(!covers(&[rect(0.0, 0.0, 41.0, 100.0)], area));
        assert!(!covers(&[], area));
    }
}
