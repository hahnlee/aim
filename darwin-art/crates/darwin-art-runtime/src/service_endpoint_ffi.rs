//! Narrow native transport ABI; handles retain the Rust listener owner.
use super::service_endpoint::ServiceEndpoint;
use std::collections::BTreeMap;
use std::os::unix::ffi::OsStrExt;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Default)]
struct Endpoints {
    next: u64,
    entries: BTreeMap<u64, Arc<ServiceEndpoint>>,
}
fn endpoints() -> &'static Mutex<Endpoints> {
    static ENDPOINTS: OnceLock<Mutex<Endpoints>> = OnceLock::new();
    ENDPOINTS.get_or_init(Default::default)
}

/// # Safety
/// path must address length bytes; output pointers must be writable and valid.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn darwin_art_service_endpoint_open(
    path: *const u8,
    length: usize,
    handle: *mut u64,
    descriptor: *mut i32,
) -> i32 {
    if path.is_null() || handle.is_null() || descriptor.is_null() || length == 0 || length > 103 {
        return -1;
    }
    unsafe {
        *handle = 0;
        *descriptor = -1;
    }
    let bytes = unsafe { std::slice::from_raw_parts(path, length) };
    if bytes.contains(&0) {
        return -1;
    }
    let path = std::path::Path::new(std::ffi::OsStr::from_bytes(bytes));
    let endpoint = match ServiceEndpoint::bind(path) {
        Ok(endpoint) => Arc::new(endpoint),
        Err(error) => {
            eprintln!("runtime service endpoint: {error}");
            return -1;
        }
    };
    let Ok(mut table) = endpoints().lock() else {
        return -1;
    };
    let Some(id) = table.next.checked_add(1) else {
        return -1;
    };
    table.next = id;
    unsafe {
        *handle = id;
        *descriptor = endpoint.descriptor();
    }
    table.entries.insert(id, endpoint);
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_service_endpoint_close(handle: u64) -> i32 {
    let Ok(mut table) = endpoints().lock() else {
        return -1;
    };
    if table.entries.remove(&handle).is_some() {
        0
    } else {
        -1
    }
}

/// Requires a live owned endpoint. 0 = published, 1 = unmanaged/no publication,
/// -1 = invalid endpoint/configuration or rejected profile handshake.
#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_service_endpoint_ready(handle: u64, bits: u32) -> i32 {
    with_endpoint(handle, || crate::service_readiness::publish(bits))
}

/// The endpoint owner reports a transport capability's terminal loss. Holds
/// the owner across IPC, without holding the handle-table mutex.
#[unsafe(no_mangle)]
pub extern "C" fn darwin_art_service_endpoint_lost(handle: u64, bits: u32) -> i32 {
    with_endpoint(handle, || crate::service_readiness::report_lost(bits))
}

fn with_endpoint(handle: u64, action: impl FnOnce() -> Result<bool, String>) -> i32 {
    let owner = match endpoints().lock() {
        Ok(table) => table.entries.get(&handle).cloned(),
        Err(_) => None,
    };
    let Some(_owner) = owner else {
        return -1;
    };
    match action() {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(error) => {
            eprintln!("runtime service readiness: {error}");
            -1
        }
    }
}
