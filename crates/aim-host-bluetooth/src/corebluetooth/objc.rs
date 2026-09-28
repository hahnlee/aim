//! The little of the Objective-C runtime, Foundation and libdispatch the
//! CoreBluetooth backend needs, called directly (no bindings crate), as
//! `aim-host-gpu` does for Metal.

use std::ffi::{CStr, CString, c_char, c_void};

pub type Id = *mut c_void;
pub type Sel = *const c_void;
pub type Class = *mut c_void;
pub const NIL: Id = std::ptr::null_mut();

aim_hostcall::dylib! {
    static FOUNDATION = c"/System/Library/Frameworks/Foundation.framework/Foundation" {
        pub fn objc_getClass(name: *const c_char) -> Class;
        fn sel_registerName(name: *const c_char) -> Sel;
        pub static objc_msgSend: c_void;
        pub fn objc_retain(obj: Id) -> Id;
        pub fn objc_release(obj: Id);
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
        pub fn objc_allocateClassPair(superclass: Class, name: *const c_char, extra: usize) -> Class;
        pub fn objc_registerClassPair(cls: Class);
        pub fn class_addMethod(cls: Class, name: Sel, imp: *const c_void, types: *const c_char)
        -> bool;
    }
}

pub type Queue = *mut c_void;

unsafe extern "C" {
    pub fn dispatch_queue_create(label: *const c_char, attr: *const c_void) -> Queue;
    pub fn dispatch_async_f(queue: Queue, context: *mut c_void, work: extern "C" fn(*mut c_void));
    pub fn dispatch_release(object: Queue);
}

pub fn sel(name: &CStr) -> Sel {
    // SAFETY: a NUL-terminated selector name.
    unsafe { sel_registerName(name.as_ptr()) }
}

pub fn class(name: &CStr) -> Class {
    // SAFETY: a NUL-terminated class name.
    unsafe { objc_getClass(name.as_ptr()) }
}

/// `objc_msgSend` cast to the method's C signature.
macro_rules! send {
    ($obj:expr, $sel:expr => $ret:ty $(, $t:ty = $a:expr)*) => {{
        // SAFETY: the selector's method has exactly this signature.
        let f: unsafe extern "C" fn($crate::corebluetooth::objc::Id, $crate::corebluetooth::objc::Sel $(, $t)*) -> $ret =
            unsafe { std::mem::transmute($crate::corebluetooth::objc::objc_msgSend()) };
        unsafe { f($obj as $crate::corebluetooth::objc::Id, $crate::corebluetooth::objc::sel($sel) $(, $a)*) }
    }};
}
pub(crate) use send;

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
        // SAFETY: popped on the thread that pushed it.
        unsafe { objc_autoreleasePoolPop(self.0) }
    }
}

/// A retained object reference.
pub struct Obj(pub Id);

// SAFETY: the backend only touches its objects on its serial queue.
unsafe impl Send for Obj {}

impl Obj {
    /// Retain `id` (not nil).
    pub fn retain(id: Id) -> Obj {
        // SAFETY: a live object.
        Obj(unsafe { objc_retain(id) })
    }
}

impl Drop for Obj {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: we hold a reference.
            unsafe { objc_release(self.0) }
        }
    }
}

pub fn nsstring(s: &str) -> Id {
    let obj = send!(class(c"NSString"), c"alloc" => Id);
    let s = send!(obj, c"initWithBytes:length:encoding:" => Id,
        *const u8 = s.as_ptr(), usize = s.len(), usize = 4);
    send!(s, c"autorelease" => Id)
}

pub fn string(s: Id) -> Option<String> {
    if s.is_null() {
        return None;
    }
    let p = send!(s, c"UTF8String" => *const c_char);
    if p.is_null() {
        return None;
    }
    // SAFETY: a NUL-terminated string the object owns.
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

pub fn nsdata(bytes: &[u8]) -> Id {
    send!(class(c"NSData"), c"dataWithBytes:length:" => Id,
        *const u8 = bytes.as_ptr(), usize = bytes.len())
}

pub fn is_kind(obj: Id, name: &CStr) -> bool {
    !obj.is_null() && send!(obj, c"isKindOfClass:" => bool, Class = class(name))
}

pub fn data(d: Id) -> Vec<u8> {
    if d.is_null() {
        return Vec::new();
    }
    let len = send!(d, c"length" => usize);
    let p = send!(d, c"bytes" => *const u8);
    if p.is_null() || len == 0 {
        return Vec::new();
    }
    // SAFETY: the object's `len` bytes, copied at once.
    unsafe { std::slice::from_raw_parts(p, len) }.to_vec()
}

pub fn array(a: Id) -> Vec<Id> {
    if a.is_null() {
        return Vec::new();
    }
    let n = send!(a, c"count" => usize);
    (0..n)
        .map(|i| send!(a, c"objectAtIndex:" => Id, usize = i))
        .collect()
}

pub fn get(dict: Id, key: Id) -> Id {
    if dict.is_null() || key.is_null() {
        return NIL;
    }
    send!(dict, c"objectForKey:" => Id, Id = key)
}

pub fn number(n: Id) -> Option<i64> {
    (!n.is_null()).then(|| send!(n, c"longLongValue" => i64))
}

pub fn nsnumber_bool(v: bool) -> Id {
    send!(class(c"NSNumber"), c"numberWithBool:" => Id, bool = v)
}

pub fn dictionary(entries: &[(Id, Id)]) -> Id {
    let d = send!(class(c"NSMutableDictionary"), c"dictionary" => Id);
    for &(k, v) in entries {
        send!(d, c"setObject:forKey:" => (), Id = v, Id = k);
    }
    d
}

pub fn nsarray(items: &[Id]) -> Id {
    let a = send!(class(c"NSMutableArray"), c"array" => Id);
    for &i in items {
        send!(a, c"addObject:" => (), Id = i);
    }
    a
}

/// A framework's exported `NSString *const`, by symbol name.
pub fn constant(handle: *mut c_void, name: &str) -> Id {
    let c = CString::new(name).unwrap();
    // SAFETY: dlsym on a handle from dlopen; the symbol is an object
    // pointer variable.
    unsafe {
        let p = libc::dlsym(handle, c.as_ptr()) as *const Id;
        if p.is_null() { NIL } else { *p }
    }
}
