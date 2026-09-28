//! Host side: system libraries opened on first use.
//!
//! `linux-run` starts for every guest process and every fork, and most
//! never use a host module. Linking the frameworks the modules use
//! (Foundation, Metal, CoreAudio, ...) would make dyld load and initialize
//! them in every start (docs/fork.md, "Cost"). A module declares what it
//! calls with [`dylib!`](crate::dylib!) instead: the library is opened with
//! `dlopen` and each symbol resolved with `dlsym` the first time it is used.

use core::ffi::{CStr, c_char, c_void};
use core::sync::atomic::{AtomicPtr, Ordering};

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

const RTLD_NOW: i32 = 0x2;

/// A library opened on first use.
pub struct Lib {
    path: &'static CStr,
    handle: AtomicPtr<c_void>,
}

impl Lib {
    pub const fn new(path: &'static CStr) -> Lib {
        Lib {
            path,
            handle: AtomicPtr::new(core::ptr::null_mut()),
        }
    }

    /// Opens the library (once); false when it cannot be opened. Opening a
    /// framework also registers its Objective-C classes.
    pub fn load(&self) -> bool {
        !self.handle().is_null()
    }

    fn handle(&self) -> *mut c_void {
        let h = self.handle.load(Ordering::Acquire);
        if !h.is_null() {
            return h;
        }
        // SAFETY: a NUL-terminated path. dlopen of an open library returns
        // the same handle, so a race stores the same value twice.
        let h = unsafe { dlopen(self.path.as_ptr(), RTLD_NOW) };
        self.handle.store(h, Ordering::Release);
        h
    }

    /// The address of `name` (NUL-terminated), cached in `slot`. Panics
    /// when the library or the symbol is missing: these are system
    /// libraries, and the declaration says the symbol is there.
    pub fn sym(&self, slot: &AtomicPtr<c_void>, name: &'static str) -> *mut c_void {
        let p = slot.load(Ordering::Relaxed);
        if !p.is_null() {
            return p;
        }
        let h = self.handle();
        // SAFETY: `name` is NUL-terminated (the macro appends it).
        let p = if h.is_null() {
            h
        } else {
            unsafe { dlsym(h, name.as_ptr().cast()) }
        };
        if p.is_null() {
            panic!("{:?}: no {}", self.path, name.trim_end_matches('\0'));
        }
        slot.store(p, Ordering::Relaxed);
        p
    }
}

/// Declares a library opened on first use and the C functions and data
/// symbols used from it:
///
/// ```ignore
/// aim_hostcall::dylib! {
///     static CORE_FOUNDATION = c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation" {
///         fn CFRelease(cf: *const c_void);
///         static kCFRunLoopDefaultMode: *const c_void;
///     }
/// }
/// ```
///
/// Each `fn` becomes an `unsafe fn` of the same signature that calls the
/// library's symbol. Each `static NAME: T` becomes `fn NAME() -> *const T`,
/// the symbol's address (for data, or for a function such as
/// `objc_msgSend` that is called through casts). `CORE_FOUNDATION` is the
/// [`Lib`], for loading a framework whose classes are used.
#[macro_export]
macro_rules! dylib {
    (@items $lib:ident;) => {};
    (@items $lib:ident;
        $(#[$m:meta])* $vis:vis fn $name:ident($($arg:ident: $ty:ty),* $(,)?) $(-> $ret:ty)?;
        $($rest:tt)*
    ) => {
        $(#[$m])*
        #[allow(non_snake_case)]
        $vis unsafe fn $name($($arg: $ty),*) $(-> $ret)? {
            static SYM: ::core::sync::atomic::AtomicPtr<::core::ffi::c_void> =
                ::core::sync::atomic::AtomicPtr::new(::core::ptr::null_mut());
            let f = $lib.sym(&SYM, ::core::concat!(::core::stringify!($name), "\0"));
            // SAFETY: the library's symbol is the C function declared here.
            unsafe {
                let f: unsafe extern "C" fn($($ty),*) $(-> $ret)? = ::core::mem::transmute(f);
                f($($arg),*)
            }
        }
        $crate::dylib!(@items $lib; $($rest)*);
    };
    (@items $lib:ident;
        $(#[$m:meta])* $vis:vis static $name:ident: $ty:ty;
        $($rest:tt)*
    ) => {
        $(#[$m])*
        #[allow(non_snake_case)]
        $vis fn $name() -> *const $ty {
            static SYM: ::core::sync::atomic::AtomicPtr<::core::ffi::c_void> =
                ::core::sync::atomic::AtomicPtr::new(::core::ptr::null_mut());
            $lib.sym(&SYM, ::core::concat!(::core::stringify!($name), "\0")) as *const $ty
        }
        $crate::dylib!(@items $lib; $($rest)*);
    };
    ($(#[$m:meta])* $vis:vis static $lib:ident = $path:literal { $($items:tt)* }) => {
        $(#[$m])*
        $vis static $lib: $crate::dylib::Lib = $crate::dylib::Lib::new($path);
        $crate::dylib!(@items $lib; $($items)*);
    };
}
