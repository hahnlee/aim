//! The few CoreFoundation calls the modules need.

use std::ffi::{CStr, c_char, c_void};

pub type CFTypeRef = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFTypeRef;
    fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, len: isize, encoding: u32) -> bool;
    fn CFNumberCreate(alloc: CFTypeRef, the_type: isize, value: *const c_void) -> CFTypeRef;
    fn CFDictionaryCreateMutable(
        alloc: CFTypeRef,
        capacity: isize,
        keys: *const c_void,
        values: *const c_void,
    ) -> CFTypeRef;
    fn CFDictionarySetValue(dict: CFTypeRef, key: CFTypeRef, value: CFTypeRef);
    pub fn CFArrayGetCount(array: CFTypeRef) -> isize;
    pub fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
}

const UTF8: u32 = 0x0800_0100;
const SINT32: isize = 3;

/// An owned (+1) CoreFoundation reference.
pub struct Owned(pub CFTypeRef);

// SAFETY: the CF objects held here (dictionaries, HID clients and devices)
// are used under the owner's lock or are immutable.
unsafe impl Send for Owned {}

impl Owned {
    pub fn new(r: CFTypeRef) -> Option<Self> {
        (!r.is_null()).then_some(Self(r))
    }

    pub fn string(s: &CStr) -> Self {
        // SAFETY: a NUL-terminated UTF-8 literal.
        Self(unsafe { CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), UTF8) })
    }

    /// A `{key: int}` dictionary, as IOKit matching takes.
    pub fn matching(pairs: &[(&CStr, i32)]) -> Self {
        // SAFETY: the dictionary retains each key and value, so ours are
        // released at the end of each iteration.
        unsafe {
            let dict = Self(CFDictionaryCreateMutable(
                std::ptr::null(),
                0,
                (&raw const kCFTypeDictionaryKeyCallBacks).cast(),
                (&raw const kCFTypeDictionaryValueCallBacks).cast(),
            ));
            for &(key, value) in pairs {
                let number = Self(CFNumberCreate(
                    std::ptr::null(),
                    SINT32,
                    (&raw const value).cast(),
                ));
                CFDictionarySetValue(dict.0, Self::string(key).0, number.0);
            }
            dict
        }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: we hold one reference.
        unsafe { CFRelease(self.0) };
    }
}

/// The contents of a CFString, if `value` is one.
pub fn string(value: CFTypeRef) -> Option<String> {
    // SAFETY: type-checked CFString read into a local buffer.
    unsafe {
        if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
            return None;
        }
        let mut buf = [0 as c_char; 128];
        CFStringGetCString(value, buf.as_mut_ptr(), buf.len() as isize, UTF8)
            .then(|| CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    }
}
