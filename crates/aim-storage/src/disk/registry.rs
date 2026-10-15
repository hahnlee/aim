//! The few IOKit and CoreFoundation calls that read the IORegistry.

use std::ffi::{CStr, OsStr, c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

type CFTypeRef = *const c_void;

#[link(name = "IOKit", kind = "framework")]
unsafe extern "C" {
    fn IOServiceMatching(name: *const c_char) -> CFTypeRef;
    fn IOServiceGetMatchingServices(main_port: u32, matching: CFTypeRef, existing: *mut u32)
    -> i32;
    fn IOIteratorNext(iterator: u32) -> u32;
    fn IOIteratorIsValid(iterator: u32) -> i32;
    fn IOIteratorReset(iterator: u32);
    fn IOObjectRelease(object: u32) -> i32;
    fn IOObjectConformsTo(object: u32, class: *const c_char) -> i32;
    fn IORegistryEntryGetChildIterator(entry: u32, plane: *const c_char, iterator: *mut u32)
    -> i32;
    fn IORegistryEntryCreateCFProperty(
        entry: u32,
        key: CFTypeRef,
        allocator: CFTypeRef,
        options: u32,
    ) -> CFTypeRef;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFBooleanTrue: CFTypeRef;
    fn CFRelease(cf: CFTypeRef);
    fn CFGetTypeID(cf: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFTypeRef;
    fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, len: isize, encoding: u32) -> bool;
    fn CFURLCreateWithString(alloc: CFTypeRef, s: CFTypeRef, base: CFTypeRef) -> CFTypeRef;
    fn CFURLGetFileSystemRepresentation(
        url: CFTypeRef,
        resolve: bool,
        buf: *mut u8,
        len: isize,
    ) -> bool;
}

const UTF8: u32 = 0x0800_0100;

/// An owned (+1) CoreFoundation reference.
struct Cf(CFTypeRef);

impl Cf {
    fn new(r: CFTypeRef) -> Option<Self> {
        (!r.is_null()).then(|| Self(r))
    }

    fn string(s: &CStr) -> Option<Self> {
        // SAFETY: a NUL-terminated UTF-8 string.
        Self::new(unsafe { CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), UTF8) })
    }

    fn to_string(&self) -> Option<String> {
        // SAFETY: a type-checked CFString read into a local buffer.
        unsafe {
            if CFGetTypeID(self.0) != CFStringGetTypeID() {
                return None;
            }
            let mut buf = [0 as c_char; 1024];
            CFStringGetCString(self.0, buf.as_mut_ptr(), buf.len() as isize, UTF8)
                .then(|| CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
        }
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: we hold one reference.
        unsafe { CFRelease(self.0) };
    }
}

/// An IORegistry entry or iterator.
pub struct Object(u32);

impl Drop for Object {
    fn drop(&mut self) {
        // SAFETY: we hold one reference.
        unsafe { IOObjectRelease(self.0) };
    }
}

impl Object {
    /// The registered services of the IOKit class `class`.
    pub fn matching(class: &CStr) -> Result<Vec<Object>, String> {
        let mut iterator = 0;
        // SAFETY: the call consumes the matching dictionary.
        let status = unsafe {
            IOServiceGetMatchingServices(0, IOServiceMatching(class.as_ptr()), &mut iterator)
        };
        if status != 0 {
            return Err(format!(
                "IOServiceGetMatchingServices {class:?}: {status:#x}"
            ));
        }
        // IOKit returns a null iterator on a successful match with no services.
        if iterator == 0 {
            return Ok(Vec::new());
        }
        Ok(Object(iterator).all())
    }

    /// Every entry of this iterator. One the registry changed under while
    /// it was read is invalid, and read again from the start, as IOKit
    /// asks (IOIteratorIsValid(3)).
    fn all(&self) -> Vec<Object> {
        loop {
            // SAFETY: a valid iterator; each entry returned is ours.
            let entries: Vec<_> = std::iter::from_fn(|| {
                let entry = unsafe { IOIteratorNext(self.0) };
                (entry != 0).then(|| Object(entry))
            })
            .collect();
            // SAFETY: a valid iterator.
            if unsafe { IOIteratorIsValid(self.0) } != 0 {
                return entries;
            }
            unsafe { IOIteratorReset(self.0) };
        }
    }

    fn children(&self) -> Vec<Object> {
        let mut iterator = 0;
        // SAFETY: a valid entry and a NUL-terminated plane name.
        let status = unsafe {
            IORegistryEntryGetChildIterator(self.0, c"IOService".as_ptr(), &mut iterator)
        };
        if status != 0 || iterator == 0 {
            return Vec::new();
        }
        Object(iterator).all()
    }

    fn property(&self, key: &CStr) -> Option<Cf> {
        let key = Cf::string(key)?;
        // SAFETY: a valid entry and key.
        Cf::new(unsafe { IORegistryEntryCreateCFProperty(self.0, key.0, std::ptr::null(), 0) })
    }

    /// The path of the file URL in the property `key`.
    pub fn path(&self, key: &CStr) -> Option<PathBuf> {
        let url = self.property(key)?;
        url.to_string()?;
        // SAFETY: a CFString.
        let url =
            Cf::new(unsafe { CFURLCreateWithString(std::ptr::null(), url.0, std::ptr::null()) })?;
        let mut buf = [0u8; libc::PATH_MAX as usize];
        // SAFETY: a CFURL and a local buffer of that length.
        if !unsafe {
            CFURLGetFileSystemRepresentation(url.0, true, buf.as_mut_ptr(), buf.len() as isize)
        } {
            return None;
        }
        let path = CStr::from_bytes_until_nul(&buf).ok()?;
        Some(PathBuf::from(OsStr::from_bytes(path.to_bytes())))
    }

    /// The BSD devices (`/dev/diskN...`) of the IOMedia at and below this
    /// entry, each before those below it, and whether each is writable.
    pub fn media(&self, out: &mut Vec<(String, bool)>) {
        // SAFETY: a valid entry and a NUL-terminated class name.
        if unsafe { IOObjectConformsTo(self.0, c"IOMedia".as_ptr()) } != 0
            && let Some(name) = self.property(c"BSD Name").and_then(|n| n.to_string())
        {
            // SAFETY: kCFBooleanTrue is a constant of CoreFoundation.
            let writable = self
                .property(c"Writable")
                .is_some_and(|w| w.0 == unsafe { kCFBooleanTrue });
            out.push((format!("/dev/{name}"), writable));
        }
        for child in self.children() {
            child.media(out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_no_services_returns_an_empty_collection() {
        assert!(
            Object::matching(c"AimStorageNonexistentClass809")
                .unwrap()
                .is_empty()
        );
    }
}
