//! Profile registry transport FFI. Android PM interprets the returned record.
use std::ffi::{CStr, c_char};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// Query exactly the supplied profile socket. No profile selection or startup.
/// Returns 0 for a copied record, 1 for an explicit absent-package response,
/// -1 for invalid arguments, -2 for transport/record failure, -3 for insufficient
/// output capacity, and -4 for an internal panic. Outputs are unchanged on failure.
///
/// # Safety
/// Input strings must be valid NUL-terminated strings. `output` must be writable
/// for `capacity` bytes; `length` must be writable and not overlap that buffer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_runtime_query_package_record(
    socket: *const c_char,
    package: *const c_char,
    output: *mut u8,
    capacity: usize,
    length: *mut usize,
) -> i32 {
    if socket.is_null() || package.is_null() || output.is_null() || length.is_null() {
        return -1;
    }
    std::panic::catch_unwind(|| {
        let socket = unsafe { CStr::from_ptr(socket) }.to_bytes();
        let package = unsafe { CStr::from_ptr(package) };
        let Ok(package) = package.to_str() else {
            return -1;
        };
        if socket.is_empty() || package.is_empty() {
            return -1;
        }
        let socket = Path::new(std::ffi::OsStr::from_bytes(socket));
        let record = match darwin_art_profile::resolve_package_at(socket, package) {
            Ok(Some(record)) => record,
            Ok(None) => return 1,
            Err(_) => return -2,
        };
        if record.len() > capacity {
            return -3;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(record.as_ptr(), output, record.len());
            length.write(record.len());
        }
        0
    })
    .unwrap_or(-4)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn query_fixture(status: u32, record: &[u8], capacity: usize) -> (i32, Vec<u8>, usize) {
        let directory = Path::new("/tmp").join(format!(
            "darwin-package-ffi-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let socket = directory.join("s");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let record = record.to_vec();
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            std::time::Instant::now() < deadline,
                            "FFI client did not connect"
                        );
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut header = [0; 16];
            stream.read_exact(&mut header).unwrap();
            assert_eq!(&header[..8], b"DARTD001");
            assert_eq!(u16::from_le_bytes(header[10..12].try_into().unwrap()), 6);
            let count = u32::from_le_bytes(header[12..].try_into().unwrap()) as usize;
            assert!(count <= 255);
            let mut package = vec![0; count];
            stream.read_exact(&mut package).unwrap();
            assert_eq!(package, b"org.example");
            header[10..12].copy_from_slice(&0x8006u16.to_le_bytes());
            header[12..].copy_from_slice(&((record.len() + 4) as u32).to_le_bytes());
            stream.write_all(&header).unwrap();
            stream.write_all(&status.to_le_bytes()).unwrap();
            stream.write_all(&record).unwrap();
        });
        let socket = std::ffi::CString::new(socket.as_os_str().as_bytes()).unwrap();
        let mut output = vec![0xa5; capacity];
        let mut length = 99;
        let status = unsafe {
            darwin_art_runtime_query_package_record(
                socket.as_ptr(),
                c"org.example".as_ptr(),
                output.as_mut_ptr(),
                capacity,
                &mut length,
            )
        };
        let joined = server.join();
        std::fs::remove_dir_all(directory).unwrap();
        joined.unwrap();
        (status, output, length)
    }

    #[test]
    fn explicit_profile_results_preserve_ffi_ownership() {
        let record = "darwin-art-launch-v1\napk=/packages/🧪.apk\napp_id=10042\nmetadata=package=org.example\n".as_bytes();
        let (status, output, length) = query_fixture(0, record, 512);
        assert_eq!(status, 0);
        assert_eq!(length, record.len());
        assert_eq!(&output[..length], record);
        assert!(output[length..].iter().all(|byte| *byte == 0xa5));
        for (remote_status, bytes, capacity, expected) in [
            (2, &b""[..], 512, 1),
            (1, &b"registry failure"[..], 512, -2),
            (0, &b"invalid record"[..], 512, -2),
            (0, record, 1, -3),
        ] {
            let (status, output, length) = query_fixture(remote_status, bytes, capacity);
            assert_eq!(status, expected);
            assert_eq!(length, 99);
            assert!(output.iter().all(|byte| *byte == 0xa5));
        }
    }

    #[test]
    fn invalid_arguments_do_not_publish_output() {
        let mut bytes = [0xa5; 32];
        let mut length = 99;
        let result = unsafe {
            darwin_art_runtime_query_package_record(
                std::ptr::null(),
                c"pkg".as_ptr(),
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut length,
            )
        };
        assert_eq!(result, -1);
        assert_eq!(length, 99);
        assert_eq!(bytes, [0xa5; 32]);
        let result = unsafe {
            darwin_art_runtime_query_package_record(
                c"".as_ptr(),
                c"pkg".as_ptr(),
                bytes.as_mut_ptr(),
                bytes.len(),
                &mut length,
            )
        };
        assert_eq!(result, -1);
        assert_eq!(length, 99);
        assert_eq!(bytes, [0xa5; 32]);
    }
}
