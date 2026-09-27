//! `log` records to logcat through liblog, mirrored to stderr (as
//! android-base's StderrLogger does for native daemons).

use std::ffi::{CString, c_char, c_int};
use std::io::Write;

#[link(name = "log")]
unsafe extern "C" {
    fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
}

struct Logger {
    tag: CString,
}

impl log::Log for Logger {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        // android_LogPriority: VERBOSE 2 .. ERROR 6.
        let prio = match record.level() {
            log::Level::Trace => 2,
            log::Level::Debug => 3,
            log::Level::Info => 4,
            log::Level::Warn => 5,
            log::Level::Error => 6,
        };
        let text = record.args().to_string();
        let _ = writeln!(std::io::stderr(), "{}: {text}", self.tag.to_string_lossy());
        if let Ok(c) = CString::new(text) {
            // SAFETY: NUL-terminated strings that outlive the call.
            unsafe { __android_log_write(prio, self.tag.as_ptr(), c.as_ptr()) };
        }
    }

    fn flush(&self) {}
}

/// Log under `tag` from now on.
pub fn init(tag: &str) {
    let logger = Box::leak(Box::new(Logger {
        tag: CString::new(tag).unwrap(),
    }));
    log::set_logger(logger).unwrap();
    log::set_max_level(log::LevelFilter::Info);
}
