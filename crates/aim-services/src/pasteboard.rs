//! The Mac's general pasteboard (`NSPasteboard`), through the Objective-C
//! runtime directly, as the other host crates reach AppKit.
//!
//! The clipboard reads the pasteboard's text only when an Android app
//! pastes: a change is noticed from `changeCount` and the types, which
//! macOS lets any process see, and the content is read on demand.

use std::ffi::{CStr, c_char, c_void};

type Id = *mut c_void;
type Sel = *const c_void;

aim_hostcall::dylib! {
    static APPKIT = c"/System/Library/Frameworks/AppKit.framework/AppKit" {
        fn objc_getClass(name: *const c_char) -> Id;
        fn sel_registerName(name: *const c_char) -> Sel;
        static objc_msgSend: c_void;
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }
}

/// `NSPasteboardTypeString`, `NSPasteboardTypeHTML`.
const STRING_TYPE: &CStr = c"public.utf8-plain-text";
const HTML_TYPE: &CStr = c"public.html";
/// `NSUTF8StringEncoding`.
const UTF8: usize = 4;

macro_rules! send {
    ($obj:expr, $sel:expr => $ret:ty $(, $t:ty = $a:expr)*) => {{
        // SAFETY: the selector's method has exactly this signature.
        let f: unsafe extern "C" fn(Id, Sel $(, $t)*) -> $ret =
            unsafe { std::mem::transmute(objc_msgSend()) };
        unsafe { f($obj, sel_registerName($sel.as_ptr()) $(, $a)*) }
    }};
}

/// An autorelease pool for one call.
struct Pool(*mut c_void);

impl Pool {
    fn new() -> Pool {
        // SAFETY: no preconditions.
        Pool(unsafe { objc_autoreleasePoolPush() })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // SAFETY: popped on the thread that pushed it.
        unsafe { objc_autoreleasePoolPop(self.0) }
    }
}

fn general() -> Id {
    // SAFETY: a NUL-terminated class name.
    let class = unsafe { objc_getClass(c"NSPasteboard".as_ptr()) };
    send!(class, c"generalPasteboard" => Id)
}

fn nsstring(s: &[u8]) -> Id {
    // SAFETY: as above.
    let class = unsafe { objc_getClass(c"NSString".as_ptr()) };
    let obj = send!(class, c"alloc" => Id);
    let obj = send!(obj, c"initWithBytes:length:encoding:" => Id,
        *const u8 = s.as_ptr(), usize = s.len(), usize = UTF8);
    send!(obj, c"autorelease" => Id)
}

/// The pasteboard's change count: it grows with every change of its
/// content, by any process.
pub fn change_count() -> i64 {
    let _pool = Pool::new();
    send!(general(), c"changeCount" => i64)
}

fn has(kind: &CStr) -> bool {
    let _pool = Pool::new();
    // SAFETY: as above.
    let class = unsafe { objc_getClass(c"NSArray".as_ptr()) };
    let types = send!(class, c"arrayWithObject:" => Id, Id = nsstring(kind.to_bytes()));
    !send!(general(), c"availableTypeFromArray:" => Id, Id = types).is_null()
}

fn string(kind: &CStr) -> Option<String> {
    let _pool = Pool::new();
    let s = send!(general(), c"stringForType:" => Id, Id = nsstring(kind.to_bytes()));
    if s.is_null() {
        return None;
    }
    let p = send!(s, c"UTF8String" => *const c_char);
    // SAFETY: a NUL-terminated string the autoreleased object owns.
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// Whether the pasteboard holds text, without reading it.
pub fn has_text() -> bool {
    has(STRING_TYPE)
}

/// Whether the pasteboard holds HTML, without reading it.
pub fn has_html() -> bool {
    has(HTML_TYPE)
}

/// The pasteboard's text.
pub fn text() -> Option<String> {
    string(STRING_TYPE)
}

/// The pasteboard's HTML.
pub fn html() -> Option<String> {
    string(HTML_TYPE)
}

/// Replaces the pasteboard's content with `text` and its `html` (or
/// nothing); returns the change count it then has.
pub fn set_text(text: Option<&str>, html: Option<&str>) -> i64 {
    let _pool = Pool::new();
    let board = general();
    send!(board, c"clearContents" => i64);
    for (value, kind) in [(text, STRING_TYPE), (html, HTML_TYPE)] {
        if let Some(value) = value {
            send!(board, c"setString:forType:" => bool,
                Id = nsstring(value.as_bytes()), Id = nsstring(kind.to_bytes()));
        }
    }
    send!(board, c"changeCount" => i64)
}
