//! Thread-owned Android linker diagnostics, independent of macOS libdyld.
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char};

#[derive(Default)]
struct ErrorState {
    message: Option<CString>,
    pending: bool,
}
thread_local! { static ERROR: RefCell<ErrorState> = RefCell::new(ErrorState::default()); }

/// # Safety
/// message is a live C string, copied before return. NULL is invalid; it does
/// not clear an earlier error. Successful linker operations need not clear it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_linker_set_error(message: *const c_char) -> i32 {
    if message.is_null() {
        return -1;
    }
    let message = unsafe { CStr::from_ptr(message) }.to_owned();
    ERROR.with(|state| {
        *state.borrow_mut() = ErrorState {
            message: Some(message),
            pending: true,
        }
    });
    0
}

/// Borrowed thread-owned storage, valid until the next error update or thread
/// exit. Reading consumes pending status, not the message allocation. Caller
/// must not free/write this pointer. Subsequent reads return NULL.
#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_linker_dlerror() -> *mut c_char {
    ERROR.with(|state| {
        let mut state = state.borrow_mut();
        if !state.pending {
            return std::ptr::null_mut();
        }
        state.pending = false;
        state
            .message
            .as_ref()
            .map_or(std::ptr::null_mut(), |message| message.as_ptr().cast_mut())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copied_read_once_and_thread_local() {
        assert!(darwin_art_linker_dlerror().is_null());
        let source = CString::new("original failure").unwrap();
        assert_eq!(unsafe { darwin_art_linker_set_error(source.as_ptr()) }, 0);
        drop(source);
        std::thread::spawn(|| {
            assert!(darwin_art_linker_dlerror().is_null());
            unsafe {
                darwin_art_linker_set_error(c"worker".as_ptr());
            }
            assert_eq!(
                unsafe { CStr::from_ptr(darwin_art_linker_dlerror()) },
                c"worker"
            );
        })
        .join()
        .unwrap();
        let message = darwin_art_linker_dlerror();
        assert_eq!(unsafe { CStr::from_ptr(message) }, c"original failure");
        assert!(darwin_art_linker_dlerror().is_null());
        assert_eq!(unsafe { CStr::from_ptr(message) }, c"original failure");
        unsafe {
            darwin_art_linker_set_error(c"replacement".as_ptr());
        }
        assert_eq!(unsafe { darwin_art_linker_set_error(std::ptr::null()) }, -1);
        assert_eq!(
            unsafe { CStr::from_ptr(darwin_art_linker_dlerror()) },
            c"replacement"
        );
    }
}
