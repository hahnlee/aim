//! Process-owned `/dev/binder` endpoint behind Bionic's central FD broker.
//!
//! The central broker owns descriptor numbers, dup and last-close ordering.
//! This module owns Android Binder process/thread state and is the only place
//! where that C callback ABI crosses into Rust.  Framework policy and Parcel
//! interpretation remain in `darwin-art-binder-device`.

use crate::{AuthorityTransport, Client, ExecuteError};
use core::ffi::c_void;
use darwin_art_binder_device::{
    device::{
        BINDER_SET_CONTEXT_MGR, BINDER_SET_CONTEXT_MGR_EXT, BINDER_SET_MAX_THREADS, BINDER_VERSION,
        BINDER_WRITE_READ,
    },
    ioctl::ClientMemory,
};
use std::{
    os::fd::IntoRawFd,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicU64, Ordering},
};

const OWNER_ABI_V7: u32 = 7;
const BROKER_OK: u32 = 0;
const ANDROID_EBADF: i32 = 9;
const ANDROID_EFAULT: i32 = 14;
const ANDROID_EINVAL: i32 = 22;
const RECEIVE_ARENA_BYTES: usize = 1024 * 1024;

type ReadFn = unsafe extern "C" fn(*mut c_void, u64, *mut c_void, usize, *mut i32) -> isize;
type WriteFn = unsafe extern "C" fn(*mut c_void, u64, *const c_void, usize, *mut i32) -> isize;
type PollFn = unsafe extern "C" fn(*mut c_void, u64, i16, *mut i16, *mut i32) -> i32;
type IoctlFn = unsafe extern "C" fn(*mut c_void, u64, u64, *mut c_void, *mut i32) -> i32;
type CloseFn = unsafe extern "C" fn(*mut c_void, u64, *mut i32) -> i32;
type ReadAtFn =
    unsafe extern "C" fn(*mut c_void, u64, *mut i64, *mut c_void, usize, *mut i32) -> isize;
type WriteAtFn =
    unsafe extern "C" fn(*mut c_void, u64, *mut i64, *const c_void, usize, *mut i32) -> isize;
type SocketOperationFn =
    unsafe extern "C" fn(*mut c_void, u64, *const c_void, *mut c_void, *mut i32) -> isize;
type PollManyFn = unsafe extern "C" fn(
    *mut c_void,
    *const u64,
    *const i16,
    *mut i16,
    usize,
    i32,
    *mut i32,
) -> i32;
type SetStatusFlagsFn = unsafe extern "C" fn(*mut c_void, u64, i32, *mut i32) -> i32;
type CreatePollWakeFn = unsafe extern "C" fn(*mut c_void, *mut i32) -> u64;
type SignalPollWakeFn = unsafe extern "C" fn(*mut c_void, u64, *mut i32) -> i32;
type ClosePollWakeFn = unsafe extern "C" fn(*mut c_void, u64, *mut i32) -> i32;
type PollManyWithWakeFn = unsafe extern "C" fn(
    *mut c_void,
    *const u64,
    *const i16,
    *mut i16,
    usize,
    i32,
    u64,
    *mut i32,
    *mut i32,
) -> i32;
type ExportHostFdFn = unsafe extern "C" fn(*mut c_void, u64, *mut i32, *mut i32) -> i32;

/// Exact prefix-compatible shape of `DarwinArtFdOwnerV1` ABI v7.
#[repr(C)]
pub struct OwnerCallbacks {
    abi_version: u32,
    struct_size: u32,
    context: *mut c_void,
    read: Option<ReadFn>,
    write: Option<WriteFn>,
    poll: Option<PollFn>,
    ioctl: Option<IoctlFn>,
    close: Option<CloseFn>,
    read_at: Option<ReadAtFn>,
    write_at: Option<WriteAtFn>,
    socket_operation: Option<SocketOperationFn>,
    poll_many: Option<PollManyFn>,
    set_status_flags: Option<SetStatusFlagsFn>,
    create_poll_wake: Option<CreatePollWakeFn>,
    signal_poll_wake: Option<SignalPollWakeFn>,
    close_poll_wake: Option<ClosePollWakeFn>,
    poll_many_with_wake: Option<PollManyWithWakeFn>,
    export_host_fd: Option<ExportHostFdFn>,
}

unsafe impl Send for OwnerCallbacks {}
unsafe impl Sync for OwnerCallbacks {}

/// Dynamically loaded central-broker entrypoints from the live RuntimeEntry
/// image.  Keeping them as function pointers prevents a second copy of the
/// broker from being linked into the host executable.
#[derive(Clone, Copy)]
pub struct BrokerApi {
    pub install_owner: unsafe extern "C" fn(*const c_void, *mut u64) -> u32,
    pub publish: unsafe extern "C" fn(u64, u64, *mut i32) -> u32,
    pub uninstall_owner: unsafe extern "C" fn(u64) -> u32,
    pub descriptor: crate::DescriptorApi,
}

#[derive(Debug)]
pub enum BinderFdError {
    Broker(u32),
    InvalidDescriptor,
    Device(String),
}

/// One Android process has one instance, even if `/dev/binder` is duplicated.
pub struct BinderFdEndpoint<T: AuthorityTransport> {
    client: Client<T>,
    broker: BrokerApi,
    owner: u64,
}

impl<T: AuthorityTransport> BinderFdEndpoint<T> {
    pub fn install(client: Client<T>, broker: BrokerApi) -> Result<Box<Self>, BinderFdError> {
        client
            .install_descriptor_api(broker.descriptor)
            .map_err(|error| BinderFdError::Device(format!("{error:?}")))?;
        client
            .device()
            .map_receive::<()>(RECEIVE_ARENA_BYTES)
            .map_err(|error| BinderFdError::Device(format!("{error:?}")))?;
        let mut endpoint = Box::new(Self {
            client,
            broker,
            owner: 0,
        });
        let callbacks = endpoint.callbacks();
        let status = unsafe {
            (broker.install_owner)(
                (&callbacks as *const OwnerCallbacks).cast(),
                &mut endpoint.owner,
            )
        };
        if status != BROKER_OK || endpoint.owner == 0 {
            return Err(BinderFdError::Broker(status));
        }
        Ok(endpoint)
    }

    /// Publish another descriptor for the same binder_proc. The central broker
    /// makes dup/close descriptions share this object and calls close once.
    pub fn open(&self) -> Result<i32, BinderFdError> {
        let mut descriptor = -1;
        let status = unsafe { (self.broker.publish)(self.owner, 1, &mut descriptor) };
        if status != BROKER_OK || descriptor < 0 {
            return Err(BinderFdError::Broker(status));
        }
        Ok(descriptor)
    }

    pub fn client(&self) -> &Client<T> {
        &self.client
    }

    fn callbacks(&self) -> OwnerCallbacks {
        OwnerCallbacks {
            abi_version: OWNER_ABI_V7,
            struct_size: std::mem::size_of::<OwnerCallbacks>() as u32,
            context: (self as *const Self).cast_mut().cast(),
            read: None,
            write: None,
            poll: Some(poll),
            ioctl: Some(ioctl::<T>),
            close: Some(close),
            read_at: None,
            write_at: None,
            socket_operation: None,
            poll_many: None,
            set_status_flags: None,
            create_poll_wake: None,
            signal_poll_wake: None,
            close_poll_wake: None,
            poll_many_with_wake: None,
            export_host_fd: Some(export_host_fd::<T>),
        }
    }
}

impl<T: AuthorityTransport> Drop for BinderFdEndpoint<T> {
    fn drop(&mut self) {
        if self.owner != 0 {
            let status = unsafe { (self.broker.uninstall_owner)(self.owner) };
            if status != BROKER_OK {
                std::process::abort();
            }
            self.owner = 0;
        }
    }
}

static NEXT_THREAD_ID: AtomicU64 = AtomicU64::new(1);
thread_local! {
    static THREAD_ID: u64 = NEXT_THREAD_ID.fetch_add(1, Ordering::Relaxed);
}

fn current_thread_id() -> u64 {
    THREAD_ID.with(|id| *id)
}

unsafe extern "C" fn ioctl<T: AuthorityTransport>(
    context: *mut c_void,
    object: u64,
    request: u64,
    argument: *mut c_void,
    android_errno: *mut i32,
) -> i32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() || object != 1 || android_errno.is_null() {
            return Err(ANDROID_EBADF);
        }
        let endpoint = unsafe { &*context.cast::<BinderFdEndpoint<T>>() };
        dispatch_ioctl(endpoint, request, argument)
    }))
    .unwrap_or(Err(ANDROID_EINVAL));
    match result {
        Ok(()) => {
            unsafe { *android_errno = 0 };
            0
        }
        Err(error) => {
            if !android_errno.is_null() {
                unsafe { *android_errno = error };
            }
            -1
        }
    }
}

fn dispatch_ioctl<T: AuthorityTransport>(
    endpoint: &BinderFdEndpoint<T>,
    request: u64,
    argument: *mut c_void,
) -> Result<(), i32> {
    let size = match request {
        BINDER_WRITE_READ => 48,
        BINDER_VERSION | BINDER_SET_MAX_THREADS | BINDER_SET_CONTEXT_MGR => 4,
        BINDER_SET_CONTEXT_MGR_EXT => 24,
        _ => return Err(ANDROID_EINVAL),
    };
    if argument.is_null() {
        return Err(ANDROID_EFAULT);
    }
    let mut memory = ProcessMemory;
    let mut bytes = vec![0; size];
    memory
        .copy_from(argument as u64, &mut bytes)
        .map_err(|_| ANDROID_EFAULT)?;
    if request == BINDER_WRITE_READ {
        endpoint
            .client
            .execute_write_read_indefinite(current_thread_id(), &mut bytes, &mut memory)
            .map_err(map_execute_error)?;
    } else if matches!(request, BINDER_SET_CONTEXT_MGR | BINDER_SET_CONTEXT_MGR_EXT) {
        endpoint
            .client
            .set_context_manager_control(request, &bytes)
            .map_err(|_| ANDROID_EINVAL)?;
    } else {
        endpoint
            .client
            .device()
            .execute_control::<()>(request, &mut bytes)
            .map_err(|_| ANDROID_EINVAL)?;
    }
    memory
        .copy_to(argument as u64, &bytes)
        .map_err(|_| ANDROID_EFAULT)
}

fn map_execute_error<E: std::fmt::Debug>(error: ExecuteError<E>) -> i32 {
    if std::env::var_os("DARWIN_ART_DEBUG_BINDER").is_some() {
        eprintln!("ART Binder device: BINDER_WRITE_READ failed: {error:?}");
    }
    ANDROID_EINVAL
}

unsafe extern "C" fn poll(
    _: *mut c_void,
    object: u64,
    _: i16,
    revents: *mut i16,
    android_errno: *mut i32,
) -> i32 {
    if object != 1 || revents.is_null() || android_errno.is_null() {
        if !android_errno.is_null() {
            unsafe { *android_errno = ANDROID_EBADF };
        }
        return -1;
    }
    unsafe {
        *revents = 0;
        *android_errno = 0;
    }
    0
}

unsafe extern "C" fn close(_: *mut c_void, object: u64, android_errno: *mut i32) -> i32 {
    if object != 1 || android_errno.is_null() {
        return -1;
    }
    unsafe { *android_errno = 0 };
    0
}

unsafe extern "C" fn export_host_fd<T: AuthorityTransport>(
    context: *mut c_void,
    object: u64,
    host_fd: *mut i32,
    android_errno: *mut i32,
) -> i32 {
    if context.is_null() || object != 1 || host_fd.is_null() || android_errno.is_null() {
        if !android_errno.is_null() {
            unsafe { *android_errno = ANDROID_EBADF };
        }
        return -1;
    }
    let endpoint = unsafe { &*context.cast::<BinderFdEndpoint<T>>() };
    match endpoint.client.device().duplicate_receive_mapping::<()>() {
        Ok(descriptor) => {
            unsafe {
                *host_fd = descriptor.into_raw_fd();
                *android_errno = 0;
            }
            0
        }
        Err(_) => {
            unsafe { *android_errno = ANDROID_EINVAL };
            -1
        }
    }
}

struct ProcessMemory;

impl ClientMemory for ProcessMemory {
    type Error = ();

    fn copy_from(&mut self, address: u64, destination: &mut [u8]) -> Result<(), Self::Error> {
        platform_copy_from(address, destination)
    }

    fn check_write(&mut self, address: u64, size: usize) -> Result<(), Self::Error> {
        if size == 0 {
            return Ok(());
        }
        let mut last = [0_u8; 1];
        platform_copy_from(address, &mut last)?;
        platform_copy_from(address.checked_add(size as u64 - 1).ok_or(())?, &mut last)
    }

    fn copy_to(&mut self, address: u64, source: &[u8]) -> Result<(), Self::Error> {
        platform_copy_to(address, source)
    }
}

#[cfg(target_os = "macos")]
fn platform_copy_from(address: u64, destination: &mut [u8]) -> Result<(), ()> {
    unsafe extern "C" {
        fn mach_task_self() -> u32;
        fn mach_vm_read_overwrite(
            task: u32,
            address: u64,
            size: u64,
            output: u64,
            output_size: *mut u64,
        ) -> i32;
    }
    let mut copied = 0_u64;
    let status = unsafe {
        mach_vm_read_overwrite(
            mach_task_self(),
            address,
            destination.len() as u64,
            destination.as_mut_ptr() as u64,
            &mut copied,
        )
    };
    (status == 0 && copied == destination.len() as u64)
        .then_some(())
        .ok_or(())
}

#[cfg(target_os = "macos")]
fn platform_copy_to(address: u64, source: &[u8]) -> Result<(), ()> {
    unsafe extern "C" {
        fn mach_task_self() -> u32;
        fn mach_vm_write(task: u32, address: u64, data: u64, size: u32) -> i32;
    }
    let size = u32::try_from(source.len()).map_err(|_| ())?;
    let status = unsafe { mach_vm_write(mach_task_self(), address, source.as_ptr() as u64, size) };
    (status == 0).then_some(()).ok_or(())
}

#[cfg(not(target_os = "macos"))]
fn platform_copy_from(address: u64, destination: &mut [u8]) -> Result<(), ()> {
    if address == 0 {
        return Err(());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(
            address as *const u8,
            destination.as_mut_ptr(),
            destination.len(),
        )
    };
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn platform_copy_to(address: u64, source: &[u8]) -> Result<(), ()> {
    if address == 0 {
        return Err(());
    }
    unsafe { std::ptr::copy_nonoverlapping(source.as_ptr(), address as *mut u8, source.len()) };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AuthorityTransport;
    use darwin_art_binder_device::{
        authority_protocol::{ConnectionToken, Message, TransferToken},
        device::{Device, ProcessIdentity},
        transfer_image::TransferImage,
    };

    struct NoTransport;
    impl AuthorityTransport for NoTransport {
        fn connection(&self) -> ConnectionToken {
            ConnectionToken::from_nonzero(1).unwrap()
        }
        fn android_uid(&self) -> u32 {
            1000
        }
        fn send(&self, _: Message) -> Result<(), crate::TransportError> {
            unreachable!()
        }
        fn receive(&self) -> Result<Message, crate::TransportError> {
            unreachable!()
        }
        fn deposit_transfer(
            &self,
            _: TransferToken,
            _: &TransferImage,
        ) -> Result<(), crate::TransportError> {
            unreachable!()
        }
        fn take_transfer(
            &self,
            _: ConnectionToken,
            _: TransferToken,
        ) -> Result<TransferImage, crate::TransportError> {
            unreachable!()
        }
    }

    unsafe extern "C" fn install(callbacks: *const c_void, owner: *mut u64) -> u32 {
        assert!(!callbacks.is_null());
        assert_eq!(
            unsafe { (*callbacks.cast::<OwnerCallbacks>()).abi_version },
            OWNER_ABI_V7
        );
        unsafe { *owner = 7 };
        BROKER_OK
    }
    unsafe extern "C" fn publish(owner: u64, object: u64, fd: *mut i32) -> u32 {
        assert_eq!((owner, object), (7, 1));
        unsafe { *fd = 0x4000_0001 };
        BROKER_OK
    }
    unsafe extern "C" fn uninstall(owner: u64) -> u32 {
        assert_eq!(owner, 7);
        BROKER_OK
    }
    unsafe extern "C" fn export_file(_: i32) -> i32 {
        -1
    }
    unsafe extern "C" fn import_file(_: i32) -> i32 {
        -1
    }
    unsafe extern "C" fn close_file(_: i32) -> i32 {
        0
    }

    fn descriptor_api() -> crate::DescriptorApi {
        crate::DescriptorApi {
            export: export_file,
            import: import_file,
            close: close_file,
            bundle: None,
            retained: None,
        }
    }

    #[test]
    fn broker_publication_and_original_version_ioctl_share_one_process_endpoint() {
        let connection = Device::default()
            .open(ProcessIdentity::new(33, 1000).unwrap())
            .unwrap();
        let (client, _) = Client::new(NoTransport, connection);
        let endpoint = BinderFdEndpoint::install(
            client,
            BrokerApi {
                install_owner: install,
                publish,
                uninstall_owner: uninstall,
                descriptor: descriptor_api(),
            },
        )
        .unwrap();
        assert_eq!(endpoint.open().unwrap(), 0x4000_0001);
        let callbacks = endpoint.callbacks();
        let mut version = 0_u32;
        let mut error = -1;
        let result = unsafe {
            callbacks.ioctl.unwrap()(
                callbacks.context,
                1,
                BINDER_VERSION,
                (&mut version as *mut u32).cast(),
                &mut error,
            )
        };
        assert_eq!((result, error, version), (0, 0, 8));

        let mut host_fd = -1;
        assert_eq!(
            unsafe {
                callbacks.export_host_fd.unwrap()(callbacks.context, 1, &mut host_fd, &mut error)
            },
            0
        );
        assert!(host_fd >= 0);
        let mut status = unsafe { std::mem::zeroed::<libc::stat>() };
        assert_eq!(unsafe { libc::fstat(host_fd, &mut status) }, 0);
        assert_eq!(status.st_size as usize, RECEIVE_ARENA_BYTES);
        unsafe { libc::close(host_fd) };

        let write = darwin_art_binder_device::command::Kind::EnterLooper
            .word()
            .to_le_bytes();
        let mut header = [0_u8; 48];
        header[..8].copy_from_slice(&(write.len() as u64).to_le_bytes());
        header[16..24].copy_from_slice(&(write.as_ptr() as u64).to_le_bytes());
        assert_eq!(
            unsafe {
                callbacks.ioctl.unwrap()(
                    callbacks.context,
                    1,
                    BINDER_WRITE_READ,
                    header.as_mut_ptr().cast(),
                    &mut error,
                )
            },
            0
        );
        assert_eq!(u64::from_le_bytes(header[8..16].try_into().unwrap()), 4);
    }

    #[test]
    fn production_ioctl_waits_for_work_without_an_idle_deadline() {
        use darwin_art_binder_device::{
            authority_protocol::LocalNodeToken, command::Kind,
            remote_transaction::RemoteTransaction, transaction_snapshot::TransactionSnapshot,
        };
        use std::{sync::mpsc, time::Duration};
        let connection = Device::default()
            .open(ProcessIdentity::new(35, 1000).unwrap())
            .unwrap();
        let (client, _) = Client::new(NoTransport, connection);
        let endpoint = BinderFdEndpoint::install(
            client,
            BrokerApi {
                install_owner: install,
                publish,
                uninstall_owner: uninstall,
                descriptor: descriptor_api(),
            },
        )
        .unwrap();
        let mut node = [0_u8; 24];
        node[..4].copy_from_slice(
            &darwin_art_binder_device::objects::Kind::Binder
                .tag()
                .to_le_bytes(),
        );
        endpoint
            .client()
            .device()
            .execute_control::<()>(BINDER_SET_CONTEXT_MGR_EXT, &mut node)
            .unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        std::thread::scope(|scope| {
            let endpoint_ref = &endpoint;
            let waiter = scope.spawn(move || {
                let callbacks = endpoint_ref.callbacks();
                let write = Kind::EnterLooper.word().to_le_bytes();
                let mut read = [0_u8; darwin_art_binder_device::transaction_wire::RECORD_SIZE];
                let mut header = [0_u8; 48];
                header[..8].copy_from_slice(&4_u64.to_le_bytes());
                header[16..24].copy_from_slice(&(write.as_ptr() as u64).to_le_bytes());
                header[24..32].copy_from_slice(&(read.len() as u64).to_le_bytes());
                header[40..48].copy_from_slice(&(read.as_mut_ptr() as u64).to_le_bytes());
                let mut error = -1;
                let result = unsafe {
                    callbacks.ioctl.unwrap()(
                        callbacks.context,
                        1,
                        BINDER_WRITE_READ,
                        header.as_mut_ptr().cast(),
                        &mut error,
                    )
                };
                done_tx.send((result, error, header, read)).unwrap();
            });
            assert!(matches!(
                done_rx.recv_timeout(Duration::from_millis(20)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ));
            let snapshot = TransactionSnapshot::capture(b"wake", &[], 0).unwrap();
            let image = TransferImage::capture(&snapshot, &[]).unwrap();
            endpoint
                .client()
                .device()
                .deliver_remote_transaction(
                    &image,
                    ConnectionToken::from_nonzero(1).unwrap(),
                    RemoteTransaction {
                        call: None,
                        sender: ConnectionToken::from_nonzero(2).unwrap(),
                        sender_pid: 0,
                        sender_euid: 1000,
                        target: LocalNodeToken::from_nonzero(1).unwrap(),
                        code: 7,
                        flags: 1,
                    },
                )
                .unwrap();
            let (result, error, header, read) =
                done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!((result, error), (0, 0));
            assert_eq!(u64::from_le_bytes(header[8..16].try_into().unwrap()), 4);
            assert_eq!(
                u32::from_le_bytes(read[..4].try_into().unwrap()),
                darwin_art_binder_device::transaction_wire::BR_TRANSACTION
            );
            waiter.join().unwrap();
        });
    }

    #[test]
    fn bad_object_and_null_argument_fail_without_dereference() {
        let connection = Device::default()
            .open(ProcessIdentity::new(34, 1000).unwrap())
            .unwrap();
        let (client, _) = Client::new(NoTransport, connection);
        let endpoint = BinderFdEndpoint {
            client,
            broker: BrokerApi {
                install_owner: install,
                publish,
                uninstall_owner: uninstall,
                descriptor: descriptor_api(),
            },
            owner: 0,
        };
        let callbacks = endpoint.callbacks();
        let mut error = 0;
        assert_eq!(
            unsafe {
                callbacks.ioctl.unwrap()(
                    callbacks.context,
                    2,
                    BINDER_VERSION,
                    std::ptr::null_mut(),
                    &mut error,
                )
            },
            -1
        );
        assert_eq!(error, ANDROID_EBADF);
        assert_eq!(
            unsafe {
                callbacks.ioctl.unwrap()(
                    callbacks.context,
                    1,
                    BINDER_VERSION,
                    std::ptr::null_mut(),
                    &mut error,
                )
            },
            -1
        );
        assert_eq!(error, ANDROID_EFAULT);
    }
}
