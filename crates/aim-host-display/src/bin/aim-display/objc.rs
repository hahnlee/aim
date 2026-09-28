//! The few Objective-C runtime calls AppKit, Core Animation and Metal need.

use std::ffi::{CStr, CString, c_char, c_void};

pub type Id = *mut c_void;
pub type Sel = *const c_void;

#[link(name = "objc")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    pub fn objc_msgSend();
    fn objc_release(obj: Id);
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
    pub fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    pub fn objc_registerClassPair(class: Id);
    pub fn class_addMethod(class: Id, name: Sel, imp: *const c_void, types: *const c_char) -> bool;
}

#[link(name = "Foundation", kind = "framework")]
unsafe extern "C" {}

pub fn class(name: &CStr) -> Id {
    // SAFETY: a NUL-terminated class name.
    unsafe { objc_getClass(name.as_ptr()) }
}

pub fn sel(name: &CStr) -> Sel {
    // SAFETY: a NUL-terminated selector name.
    unsafe { sel_registerName(name.as_ptr()) }
}

/// `objc_msgSend` cast to the method's C signature (arm64 has no `_stret`
/// variant, so structs return the same way).
macro_rules! send {
    ($obj:expr, $sel:expr => $ret:ty $(, $t:ty = $a:expr)*) => {{
        use $crate::objc::{Id, Sel, objc_msgSend, sel};
        // SAFETY: the selector's method has exactly this signature.
        let f: unsafe extern "C" fn(Id, Sel $(, $t)*) -> $ret =
            unsafe { std::mem::transmute(objc_msgSend as unsafe extern "C" fn()) };
        unsafe { f($obj, sel($sel) $(, $a)*) }
    }};
}

unsafe extern "C" {
    static _NSConcreteGlobalBlock: [*const c_void; 32];
}

/// A block literal without captures (`_NSConcreteGlobalBlock`). Copying
/// it returns it unchanged.
#[repr(C)]
pub struct GlobalBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: *const c_void,
    descriptor: &'static BlockDescriptor,
}

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

// SAFETY: immutable after construction.
unsafe impl Send for GlobalBlock {}
unsafe impl Sync for GlobalBlock {}

impl GlobalBlock {
    /// A block whose invoke function is `invoke` (its first argument is the
    /// block itself).
    pub fn new(invoke: *const c_void) -> GlobalBlock {
        static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
            reserved: 0,
            size: size_of::<GlobalBlock>(),
        };
        GlobalBlock {
            isa: (&raw const _NSConcreteGlobalBlock).cast(),
            flags: 1 << 28, // BLOCK_IS_GLOBAL
            reserved: 0,
            invoke,
            descriptor: &DESCRIPTOR,
        }
    }
}

pub fn release(obj: Id) {
    if !obj.is_null() {
        // SAFETY: the caller owns one reference to `obj`.
        unsafe { objc_release(obj) }
    }
}

/// An autorelease pool for the current scope.
pub struct Pool(*mut c_void);

impl Pool {
    pub fn new() -> Pool {
        // SAFETY: no preconditions.
        Pool(unsafe { objc_autoreleasePoolPush() })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // SAFETY: pushed by `new` on this thread.
        unsafe { objc_autoreleasePoolPop(self.0) }
    }
}

/// An autoreleased `NSString`.
pub fn nsstring(s: &str) -> Id {
    let c = CString::new(s).unwrap_or_default();
    send!(class(c"NSString"), c"stringWithUTF8String:" => Id, *const c_char = c.as_ptr())
}

/// The UTF-8 text of an `NSString` (or an `NSError`'s description).
pub fn text(s: Id) -> String {
    if s.is_null() {
        return String::new();
    }
    let p = send!(s, c"UTF8String" => *const c_char);
    if p.is_null() {
        return String::new();
    }
    // SAFETY: NUL-terminated, owned by `s`.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct CGRect {
    pub x: f64,
    pub y: f64,
    pub size: CGSize,
}
