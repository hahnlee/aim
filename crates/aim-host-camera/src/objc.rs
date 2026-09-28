//! The little of the Objective-C runtime, libdispatch and the blocks ABI
//! the AVFoundation backend needs, called directly (no bindings crate), as
//! `aim-host-bluetooth` does for CoreBluetooth.

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
    pub fn dispatch_release(object: Queue);
    static _NSConcreteGlobalBlock: [*const c_void; 32];
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
        let f: unsafe extern "C" fn($crate::objc::Id, $crate::objc::Sel $(, $t)*) -> $ret =
            unsafe { std::mem::transmute($crate::objc::objc_msgSend()) };
        unsafe { f($obj as $crate::objc::Id, $crate::objc::sel($sel) $(, $a)*) }
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

/// An owned (+1) object reference.
pub struct Obj(pub Id);

// SAFETY: AVFoundation's capture objects may be used from any thread.
unsafe impl Send for Obj {}
unsafe impl Sync for Obj {}

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

pub fn string(s: Id) -> String {
    if s.is_null() {
        return String::new();
    }
    let p = send!(s, c"UTF8String" => *const c_char);
    if p.is_null() {
        return String::new();
    }
    // SAFETY: a NUL-terminated string the object owns.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
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

pub fn nsarray(items: &[Id]) -> Id {
    let a = send!(class(c"NSMutableArray"), c"array" => Id);
    for &i in items {
        send!(a, c"addObject:" => (), Id = i);
    }
    a
}

pub fn dictionary(entries: &[(Id, Id)]) -> Id {
    let d = send!(class(c"NSMutableDictionary"), c"dictionary" => Id);
    for &(k, v) in entries {
        if !k.is_null() && !v.is_null() {
            send!(d, c"setObject:forKey:" => (), Id = v, Id = k);
        }
    }
    d
}

pub fn number_u32(v: u32) -> Id {
    send!(class(c"NSNumber"), c"numberWithUnsignedInt:" => Id, u32 = v)
}

pub fn number_f64(v: f64) -> Id {
    send!(class(c"NSNumber"), c"numberWithDouble:" => Id, f64 = v)
}

pub fn localized_description(error: Id) -> String {
    if error.is_null() {
        return String::new();
    }
    string(send!(error, c"localizedDescription" => Id))
}

/// A framework opened with `dlopen`, for its exported functions and
/// constants. It is never closed: it stays loaded for the process's life.
pub struct Framework(*mut c_void);

// SAFETY: a dlopen handle is process-wide and immutable.
unsafe impl Send for Framework {}
unsafe impl Sync for Framework {}

impl Framework {
    pub fn open(path: &CStr) -> Option<Framework> {
        // SAFETY: loading a system framework.
        let h = unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_NOW) };
        (!h.is_null()).then_some(Framework(h))
    }

    /// The address of exported symbol `name`.
    pub fn symbol(&self, name: &str) -> *mut c_void {
        let c = CString::new(name).unwrap();
        // SAFETY: dlsym on a live handle.
        unsafe { libc::dlsym(self.0, c.as_ptr()) }
    }

    /// Exported function `name`, logged when missing.
    ///
    /// # Safety
    /// `F` must be the function's C signature.
    pub unsafe fn function<F>(&self, name: &str) -> Option<F> {
        let p = self.symbol(name);
        if p.is_null() {
            crate::log!("{name} is missing");
            return None;
        }
        // SAFETY: caller contract; F is a function pointer type.
        Some(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&p) })
    }

    /// An exported `NSString *const` (or `CFStringRef`), NIL if absent.
    pub fn constant(&self, name: &str) -> Id {
        let p = self.symbol(name) as *const Id;
        // SAFETY: the symbol is an object pointer variable.
        if p.is_null() { NIL } else { unsafe { *p } }
    }
}

/// A global block (no captures), as clang emits for a block literal that
/// uses no variables. The callee may copy it; copying a global block
/// returns it unchanged.
#[repr(C)]
pub struct GlobalBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    pub invoke: *const c_void,
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

const BLOCK_IS_GLOBAL: i32 = 1 << 28;

impl GlobalBlock {
    /// A block whose invoke function is `invoke` (its first argument is
    /// the block itself).
    pub fn new(invoke: *const c_void) -> GlobalBlock {
        static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
            reserved: 0,
            size: std::mem::size_of::<GlobalBlock>(),
        };
        GlobalBlock {
            // The address of libSystem's global block class.
            isa: (&raw const _NSConcreteGlobalBlock).cast(),
            flags: BLOCK_IS_GLOBAL,
            reserved: 0,
            invoke,
            descriptor: &DESCRIPTOR,
        }
    }
}
