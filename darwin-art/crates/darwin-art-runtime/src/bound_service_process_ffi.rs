//! Narrow system-process FFI for daemon-owned bound-service children.
//!
//! Android framework code supplies process identity only. Activation tokens
//! remain Rust-owned and are represented to native callers by a process-local
//! opaque handle that is consumed after successful activation.

use darwin_art_profile::{BoundServiceProcessRequest, BoundServiceProcessResponse};
use std::collections::BTreeMap;
use std::ffi::{CStr, c_char};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

#[derive(Default)]
struct PreparedProcesses {
    next: u64,
    entries: BTreeMap<u64, BoundServiceProcessResponse>,
}

impl PreparedProcesses {
    fn insert(&mut self, response: BoundServiceProcessResponse) -> Result<u64, ()> {
        for _ in 0..u64::MAX {
            self.next = self.next.wrapping_add(1).max(1);
            if let std::collections::btree_map::Entry::Vacant(entry) = self.entries.entry(self.next)
            {
                entry.insert(response);
                return Ok(self.next);
            }
        }
        Err(())
    }
}

fn prepared() -> &'static Mutex<PreparedProcesses> {
    static PREPARED: OnceLock<Mutex<PreparedProcesses>> = OnceLock::new();
    PREPARED.get_or_init(|| Mutex::new(PreparedProcesses::default()))
}

/// Prepare a daemon child and publish only its PID and a local opaque handle.
///
/// Returns 0 on success, -1 for invalid arguments, -2 for daemon/transport
/// failure, -3 for local handle-table failure and -4 for an internal panic.
/// Outputs are unchanged on failure.
///
/// # Safety
/// Both strings must be valid NUL-terminated byte strings. Output pointers
/// must be writable, non-overlapping and valid for their respective types.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_runtime_prepare_bound_service_process(
    socket: *const c_char,
    package: *const c_char,
    process_name: *const c_char,
    uid: u32,
    isolated: i32,
    start_sequence: u64,
    output_pid: *mut u32,
    output_handle: *mut u64,
) -> i32 {
    if socket.is_null()
        || package.is_null()
        || process_name.is_null()
        || output_pid.is_null()
        || output_handle.is_null()
        || !matches!(isolated, 0 | 1)
    {
        return -1;
    }
    std::panic::catch_unwind(|| {
        let socket = unsafe { CStr::from_ptr(socket) }.to_bytes();
        let package = unsafe { CStr::from_ptr(package) };
        let process_name = unsafe { CStr::from_ptr(process_name) };
        let (Ok(package), Ok(process_name)) = (package.to_str(), process_name.to_str()) else {
            return -1;
        };
        if socket.is_empty() || package.is_empty() || process_name.is_empty() {
            return -1;
        }
        let request = BoundServiceProcessRequest {
            package: package.into(),
            process_name: process_name.into(),
            uid,
            isolated: isolated == 1,
            start_sequence,
        };
        let response = match darwin_art_profile::start_bound_service_process(
            Path::new(std::ffi::OsStr::from_bytes(socket)),
            &request,
        ) {
            Ok(response) => response,
            Err(error) => {
                eprintln!("bound-service process prepare: {error}");
                return -2;
            }
        };
        let handle = match prepared().lock() {
            Ok(mut table) => match table.insert(response) {
                Ok(handle) => handle,
                Err(()) => return -3,
            },
            Err(_) => return -3,
        };
        unsafe {
            output_pid.write(response.pid);
            output_handle.write(handle);
        }
        0
    })
    .unwrap_or(-4)
}

/// Activate exactly one previously prepared child. Successful activation
/// consumes the local handle; a transport failure retains it for a bounded
/// retry by the system process.
///
/// # Safety
/// `socket` must point to a valid NUL-terminated byte string for the duration
/// of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_runtime_activate_bound_service_process(
    socket: *const c_char,
    handle: u64,
) -> i32 {
    if socket.is_null() || handle == 0 {
        return -1;
    }
    std::panic::catch_unwind(|| {
        let socket = unsafe { CStr::from_ptr(socket) }.to_bytes();
        if socket.is_empty() {
            return -1;
        }
        let response = match prepared().lock() {
            Ok(mut table) => match table.entries.remove(&handle) {
                Some(response) => response,
                None => return -1,
            },
            Err(_) => return -3,
        };
        if let Err(error) = darwin_art_profile::activate_bound_service_process(
            Path::new(std::ffi::OsStr::from_bytes(socket)),
            response,
        ) {
            eprintln!("bound-service process activate: {error}");
            if let Ok(mut table) = prepared().lock() {
                table.entries.entry(handle).or_insert(response);
            }
            return -2;
        }
        0
    })
    .unwrap_or(-4)
}

/// Forget a local prepare handle when Java process reservation fails. The
/// daemon independently kills and reaps an unactivated child at its deadline.
#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_runtime_discard_bound_service_process(handle: u64) -> i32 {
    if handle == 0 {
        return -1;
    }
    std::panic::catch_unwind(|| match prepared().lock() {
        Ok(mut table) => {
            if table.entries.remove(&handle).is_some() {
                0
            } else {
                -1
            }
        }
        Err(_) => -3,
    })
    .unwrap_or(-4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;

    fn accept_message(
        listener: &UnixListener,
        operation: u16,
    ) -> (std::os::unix::net::UnixStream, Vec<u8>) {
        let (mut stream, _) = listener.accept().unwrap();
        let mut header = [0_u8; 16];
        stream.read_exact(&mut header).unwrap();
        assert_eq!(&header[..8], b"DARTD001");
        assert_eq!(
            u16::from_le_bytes(header[10..12].try_into().unwrap()),
            operation
        );
        let length = u32::from_le_bytes(header[12..].try_into().unwrap()) as usize;
        let mut payload = vec![0; length];
        stream.read_exact(&mut payload).unwrap();
        (stream, payload)
    }

    fn respond(stream: &mut std::os::unix::net::UnixStream, operation: u16, payload: &[u8]) {
        let mut header = [0_u8; 16];
        header[..8].copy_from_slice(b"DARTD001");
        header[8..10].copy_from_slice(&1_u16.to_le_bytes());
        header[10..12].copy_from_slice(&(0x8000 | operation).to_le_bytes());
        header[12..].copy_from_slice(&((payload.len() + 4) as u32).to_le_bytes());
        stream.write_all(&header).unwrap();
        stream.write_all(&0_u32.to_le_bytes()).unwrap();
        stream.write_all(payload).unwrap();
    }

    #[test]
    fn ffi_keeps_activation_capability_out_of_native_outputs() {
        let directory = std::env::temp_dir().join(format!(
            "darwin-bound-service-ffi-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let socket_path = directory.join("s");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut start, payload) = accept_message(&listener, 18);
            assert_eq!(payload[0], 2);
            assert!(!payload.windows(4).any(|bytes| bytes == b"/bin"));
            let response = BoundServiceProcessResponse {
                pid: 4242,
                start_sequence: 9,
                incarnation: [10, 11],
                activation_token: [0x5a; 16],
            };
            respond(&mut start, 18, &response.encode().unwrap());

            let (mut activate, payload) = accept_message(&listener, 19);
            assert_eq!(payload, response.encode().unwrap());
            respond(&mut activate, 19, &[]);
        });
        let socket = std::ffi::CString::new(socket_path.as_os_str().as_bytes()).unwrap();
        let mut pid = 0_u32;
        let mut handle = 0_u64;
        assert_eq!(
            unsafe {
                darwin_art_runtime_prepare_bound_service_process(
                    socket.as_ptr(),
                    c"org.example.app".as_ptr(),
                    c"org.example.app:renderer".as_ptr(),
                    10_042,
                    0,
                    9,
                    &mut pid,
                    &mut handle,
                )
            },
            0
        );
        assert_eq!(pid, 4242);
        assert_ne!(handle, 0);
        assert_eq!(
            unsafe { darwin_art_runtime_activate_bound_service_process(socket.as_ptr(), handle) },
            0
        );
        assert_eq!(
            unsafe { darwin_art_runtime_activate_bound_service_process(socket.as_ptr(), handle) },
            -1
        );
        server.join().unwrap();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn invalid_or_discarded_handles_publish_nothing() {
        let mut pid = 7_u32;
        let mut handle = 8_u64;
        assert_eq!(
            unsafe {
                darwin_art_runtime_prepare_bound_service_process(
                    std::ptr::null(),
                    c"pkg".as_ptr(),
                    c"proc".as_ptr(),
                    10_000,
                    0,
                    1,
                    &mut pid,
                    &mut handle,
                )
            },
            -1
        );
        assert_eq!((pid, handle), (7, 8));
        assert_eq!(darwin_art_runtime_discard_bound_service_process(0), -1);
        assert_eq!(
            darwin_art_runtime_discard_bound_service_process(u64::MAX),
            -1
        );
    }
}
