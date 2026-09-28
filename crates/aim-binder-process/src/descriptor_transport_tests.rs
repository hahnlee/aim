use super::*;
use std::ffi::c_void;
use std::io::Read;
use std::os::fd::{AsRawFd, IntoRawFd};
use std::os::unix::net::UnixStream;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};

static TEST_SERIAL: Mutex<()> = Mutex::new(());
static EXPORTED_PEER: Mutex<Option<UnixStream>> = Mutex::new(None);
static LAST_IMPORTED_FD: AtomicI32 = AtomicI32::new(-1);
static LAST_SOURCE: AtomicU64 = AtomicU64::new(0);
static LAST_TRANSFER: AtomicU64 = AtomicU64::new(0);
static LAST_ORDINAL: AtomicU64 = AtomicU64::new(0);
static LAST_OFFSET: AtomicU64 = AtomicU64::new(0);
static LAST_ATTRIBUTE: AtomicU64 = AtomicU64::new(0);
static RETAINED_RELEASES: AtomicU64 = AtomicU64::new(0);
static RETAINED_EXPORTS: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" fn export_bare(_: i32) -> i32 {
    std::fs::File::open("/dev/null").unwrap().into_raw_fd()
}
unsafe extern "C" fn import_bare(_: i32) -> i32 {
    -1
}
unsafe extern "C" fn close_bare(_: i32) -> i32 {
    -1
}

unsafe extern "C" fn release_retained(_: *mut c_void) {
    RETAINED_RELEASES.fetch_add(1, Ordering::SeqCst);
}

unsafe extern "C" fn export_retained(
    _: i32,
    _: *const DescriptorTransferBinding,
    result: *mut RetainedExportedDescriptor,
) -> i32 {
    RETAINED_EXPORTS.fetch_add(1, Ordering::SeqCst);
    unsafe {
        (*result).host_fd = std::fs::File::open("/dev/null").unwrap().into_raw_fd();
        (*result).lease = 1_usize as *mut c_void;
        (*result).attributes_length = 1;
        (*result).attributes[0] = 0xa7;
    }
    0
}

unsafe extern "C" fn export_retained_oversized(
    _: i32,
    _: *const DescriptorTransferBinding,
    result: *mut RetainedExportedDescriptor,
) -> i32 {
    unsafe {
        let (endpoint, peer) = UnixStream::pair().unwrap();
        *EXPORTED_PEER.lock().unwrap() = Some(peer);
        (*result).host_fd = endpoint.into_raw_fd();
        (*result).lease = 1_usize as *mut c_void;
        (*result).attributes_length = (MAX_ATTRIBUTES_BYTES as u32) + 1;
    }
    0
}

unsafe extern "C" fn export_retained_partial(
    _: i32,
    _: *const DescriptorTransferBinding,
    result: *mut RetainedExportedDescriptor,
) -> i32 {
    let call = RETAINED_EXPORTS.fetch_add(1, Ordering::SeqCst);
    if call == 0 {
        unsafe {
            (*result).host_fd = std::fs::File::open("/dev/null").unwrap().into_raw_fd();
            (*result).lease = 1_usize as *mut c_void;
        }
        0
    } else {
        -1
    }
}

unsafe extern "C" fn export_retained_without_fd(
    _: i32,
    _: *const DescriptorTransferBinding,
    result: *mut RetainedExportedDescriptor,
) -> i32 {
    unsafe {
        (*result).lease = 1_usize as *mut c_void;
    }
    0
}

unsafe extern "C" fn export_retained_without_lease(
    _: i32,
    _: *const DescriptorTransferBinding,
    result: *mut RetainedExportedDescriptor,
) -> i32 {
    unsafe {
        let (endpoint, peer) = UnixStream::pair().unwrap();
        *EXPORTED_PEER.lock().unwrap() = Some(peer);
        (*result).host_fd = endpoint.into_raw_fd();
    }
    0
}

unsafe extern "C" fn export_oversized(
    _: i32,
    _: *const DescriptorTransferBinding,
    result: *mut ExportedDescriptor,
) -> i32 {
    unsafe {
        let (endpoint, peer) = UnixStream::pair().unwrap();
        *EXPORTED_PEER.lock().unwrap() = Some(peer);
        let fd = endpoint.into_raw_fd();
        (*result).host_fd = fd;
        (*result).attributes_length = (MAX_ATTRIBUTES_BYTES as u32) + 1;
    }
    0
}

unsafe extern "C" fn import_bundle(
    fd: i32,
    binding: *const DescriptorTransferBinding,
    attributes: *const u8,
    length: usize,
) -> i32 {
    LAST_IMPORTED_FD.store(fd, Ordering::Relaxed);
    let (source, transfer, ordinal, offset) = unsafe {
        (
            (*binding).source_connection,
            (*binding).transfer,
            (*binding).ordinal,
            (*binding).object_offset,
        )
    };
    LAST_SOURCE.store(source, Ordering::Relaxed);
    LAST_TRANSFER.store(transfer, Ordering::Relaxed);
    LAST_ORDINAL.store(ordinal, Ordering::Relaxed);
    LAST_OFFSET.store(offset, Ordering::Relaxed);
    LAST_ATTRIBUTE.store(
        if length == 0 {
            0
        } else {
            unsafe { *attributes as u64 }
        },
        Ordering::Relaxed,
    );
    unsafe {
        libc::close(fd);
    }
    33
}

unsafe extern "C" fn import_failure(
    fd: i32,
    _: *const DescriptorTransferBinding,
    _: *const u8,
    _: usize,
) -> i32 {
    LAST_IMPORTED_FD.store(fd, Ordering::Relaxed);
    unsafe {
        libc::close(fd);
    }
    -1
}

fn binding() -> DescriptorTransferBinding {
    DescriptorTransferBinding::new(7, 11, 0, 24)
}

// Observe the actual endpoint's lifetime, not a raw FD number that another
// parallel test could have reused between close and F_GETFD.
fn assert_endpoint_closed(mut peer: UnixStream) {
    peer.set_nonblocking(true).unwrap();
    assert_eq!(peer.read(&mut [0]).unwrap(), 0);
}

#[test]
fn bare_port_preserves_exact_binding_and_owned_fd() {
    let _serial = TEST_SERIAL.lock().unwrap();
    let api = DescriptorApi {
        export: export_bare,
        import: import_bare,
        close: close_bare,
        bundle: None,
        retained: None,
    };
    let exported = api.export_bound(17, binding()).unwrap();
    assert_eq!((exported.offset(), exported.ordinal()), (24, 0));
    assert!(exported.attributes().as_bytes().is_empty());
    assert!(exported.descriptor().as_raw_fd() >= 0);
}

#[test]
fn retained_export_lease_releases_exactly_once() {
    let _serial = TEST_SERIAL.lock().unwrap();
    RETAINED_RELEASES.store(0, Ordering::SeqCst);
    RETAINED_EXPORTS.store(0, Ordering::SeqCst);
    let api = DescriptorApi {
        export: export_bare,
        import: import_bare,
        close: close_bare,
        bundle: None,
        retained: Some(DescriptorRetainedApi {
            export: export_retained,
            release: release_retained,
        }),
    };
    let (bundle, lease) = api.export_bound_retained(17, binding()).unwrap();
    assert_eq!(bundle.attributes().as_bytes(), &[0xa7]);
    assert_eq!(RETAINED_RELEASES.load(Ordering::SeqCst), 0);
    drop(bundle);
    drop(lease);
    assert_eq!(RETAINED_RELEASES.load(Ordering::SeqCst), 1);
}

#[test]
fn retained_oversized_attributes_close_fd_and_release_lease() {
    let _serial = TEST_SERIAL.lock().unwrap();
    RETAINED_RELEASES.store(0, Ordering::SeqCst);
    let api = DescriptorApi {
        export: export_bare,
        import: import_bare,
        close: close_bare,
        bundle: None,
        retained: Some(DescriptorRetainedApi {
            export: export_retained_oversized,
            release: release_retained,
        }),
    };
    assert!(api.export_bound_retained(17, binding()).is_err());
    assert_endpoint_closed(EXPORTED_PEER.lock().unwrap().take().unwrap());
    assert_eq!(RETAINED_RELEASES.load(Ordering::SeqCst), 1);
}

#[test]
fn retained_incomplete_output_retires_any_partial_resources() {
    let _serial = TEST_SERIAL.lock().unwrap();
    RETAINED_RELEASES.store(0, Ordering::SeqCst);
    let no_fd = DescriptorApi {
        export: export_bare,
        import: import_bare,
        close: close_bare,
        bundle: None,
        retained: Some(DescriptorRetainedApi {
            export: export_retained_without_fd,
            release: release_retained,
        }),
    };
    assert!(no_fd.export_bound_retained(17, binding()).is_err());
    assert_eq!(RETAINED_RELEASES.load(Ordering::SeqCst), 1);

    let no_lease = DescriptorApi {
        export: export_bare,
        import: import_bare,
        close: close_bare,
        bundle: None,
        retained: Some(DescriptorRetainedApi {
            export: export_retained_without_lease,
            release: release_retained,
        }),
    };
    assert!(no_lease.export_bound_retained(17, binding()).is_err());
    assert_endpoint_closed(EXPORTED_PEER.lock().unwrap().take().unwrap());
    assert_eq!(RETAINED_RELEASES.load(Ordering::SeqCst), 1);
}

#[test]
fn retained_partial_capture_drops_already_acquired_leases() {
    let _serial = TEST_SERIAL.lock().unwrap();
    RETAINED_RELEASES.store(0, Ordering::SeqCst);
    RETAINED_EXPORTS.store(0, Ordering::SeqCst);
    let api = DescriptorApi {
        export: export_bare,
        import: import_bare,
        close: close_bare,
        bundle: None,
        retained: Some(DescriptorRetainedApi {
            export: export_retained_partial,
            release: release_retained,
        }),
    };
    let mut bundles = Vec::new();
    let mut leases = Vec::new();
    let (bundle, lease) = api.export_bound_retained(17, binding()).unwrap();
    bundles.push(bundle);
    leases.push(lease);
    assert!(api.export_bound_retained(17, binding()).is_err());
    drop(bundles);
    drop(leases);
    assert_eq!(RETAINED_RELEASES.load(Ordering::SeqCst), 1);
}

#[test]
fn oversized_export_retires_provider_fd() {
    let _serial = TEST_SERIAL.lock().unwrap();
    let oversized = DescriptorApi {
        export: export_bare,
        import: import_bare,
        close: close_bare,
        bundle: Some(DescriptorBundleApi {
            export: export_oversized,
            import: import_bundle,
        }),
        retained: None,
    };
    assert!(oversized.export_bound(17, binding()).is_err());
    assert_endpoint_closed(EXPORTED_PEER.lock().unwrap().take().unwrap());
}

#[test]
fn v2_import_consumes_owned_bundle_and_preserves_binding_metadata() {
    let _serial = TEST_SERIAL.lock().unwrap();
    let api = DescriptorApi {
        export: export_bare,
        import: import_bare,
        close: close_bare,
        bundle: Some(DescriptorBundleApi {
            export: export_oversized,
            import: import_bundle,
        }),
        retained: None,
    };
    let (file, peer) = UnixStream::pair().unwrap();
    let attributes = DescriptorAttributes::new(vec![0xa7, 1]).unwrap();
    let bundle = DescriptorBundle::new(24, 0, file.into(), attributes);
    assert_eq!(api.import_bound(bundle, binding()).unwrap(), 33);
    assert_eq!(LAST_SOURCE.load(Ordering::Relaxed), 7);
    assert_eq!(LAST_TRANSFER.load(Ordering::Relaxed), 11);
    assert_eq!(LAST_ORDINAL.load(Ordering::Relaxed), 0);
    assert_eq!(LAST_OFFSET.load(Ordering::Relaxed), 24);
    assert_eq!(LAST_ATTRIBUTE.load(Ordering::Relaxed), 0xa7);
    assert_endpoint_closed(peer);
}

#[test]
fn legacy_import_refuses_nonempty_attributes_before_provider_call() {
    let _serial = TEST_SERIAL.lock().unwrap();
    let api = DescriptorApi {
        export: export_bare,
        import: import_bare,
        close: close_bare,
        bundle: None,
        retained: None,
    };
    let (file, peer) = UnixStream::pair().unwrap();
    let bundle = DescriptorBundle::new(
        24,
        0,
        file.into(),
        DescriptorAttributes::new(vec![9]).unwrap(),
    );
    assert!(api.import_bound(bundle, binding()).is_err());
    assert_endpoint_closed(peer);
}

#[test]
fn v2_import_failure_consumes_descriptor_for_rollback() {
    let _serial = TEST_SERIAL.lock().unwrap();
    let api = DescriptorApi {
        export: export_bare,
        import: import_bare,
        close: close_bare,
        bundle: Some(DescriptorBundleApi {
            export: export_oversized,
            import: import_failure,
        }),
        retained: None,
    };
    let (file, peer) = UnixStream::pair().unwrap();
    let fd = file.as_raw_fd();
    let bundle = DescriptorBundle::new(24, 0, file.into(), DescriptorAttributes::empty());
    assert!(api.import_bound(bundle, binding()).is_err());
    assert_eq!(LAST_IMPORTED_FD.load(Ordering::Relaxed), fd);
    assert_endpoint_closed(peer);
}
