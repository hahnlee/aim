use darwin_art_scm_transfer::{
    AuthorityEpoch, CarrierId, EndpointId, ProcessEpoch, Retirement, ScmTransferLeaseOwner, Side,
    TrustedReceiveContext, TrustedSendContext,
};
use std::io::{Read, Write};
use std::mem::{size_of, zeroed};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::ptr;

const MAX_NATIVE_RIGHTS: usize = 254;

fn authority() -> AuthorityEpoch {
    AuthorityEpoch { instance: 0xfeed }
}

fn carrier() -> CarrierId {
    CarrierId {
        authority: authority(),
        serial: 41,
    }
}

fn send_context() -> TrustedSendContext {
    TrustedSendContext {
        sender: ProcessEpoch {
            pid: 100,
            instance: 1,
        },
        endpoint: EndpointId {
            carrier: carrier(),
            side: Side::A,
        },
    }
}

fn receive_context() -> TrustedReceiveContext {
    TrustedReceiveContext {
        receiver: ProcessEpoch {
            pid: 200,
            instance: 1,
        },
        endpoint: EndpointId {
            carrier: carrier(),
            side: Side::B,
        },
    }
}

fn cmsg_align(length: usize) -> usize {
    #[cfg(target_os = "macos")]
    let alignment = size_of::<u32>();
    #[cfg(not(target_os = "macos"))]
    let alignment = size_of::<usize>();
    (length + alignment - 1) & !(alignment - 1)
}

fn cmsg_len(data_len: usize) -> usize {
    #[cfg(target_os = "macos")]
    {
        unsafe { libc::CMSG_LEN(u32::try_from(data_len).unwrap()) as usize }
    }
    #[cfg(not(target_os = "macos"))]
    {
        cmsg_align(size_of::<libc::cmsghdr>()) + data_len
    }
}

fn cmsg_space(data_len: usize) -> usize {
    #[cfg(target_os = "macos")]
    {
        unsafe { libc::CMSG_SPACE(u32::try_from(data_len).unwrap()) as usize }
    }
    #[cfg(not(target_os = "macos"))]
    {
        cmsg_align(size_of::<libc::cmsghdr>()) + cmsg_align(data_len)
    }
}

fn send_fds(carrier: &UnixStream, fds: &[RawFd]) {
    assert!(!fds.is_empty() && fds.len() <= MAX_NATIVE_RIGHTS);
    let header_size = size_of::<libc::cmsghdr>();
    let data_offset = cmsg_align(header_size);
    let data_len = size_of::<RawFd>() * fds.len();
    let cmsg_len = cmsg_len(data_len);
    let cmsg_space = cmsg_space(data_len);
    let mut storage = vec![0_usize; cmsg_space.div_ceil(size_of::<usize>())];
    let control = storage.as_mut_ptr().cast::<u8>();
    let header = control.cast::<libc::cmsghdr>();
    // SAFETY: control is aligned sufficiently for byte storage; cmsghdr fields
    // are written with unaligned operations and the buffer bounds are checked.
    unsafe {
        ptr::write_unaligned(
            ptr::addr_of_mut!((*header).cmsg_len),
            cmsg_len.try_into().unwrap(),
        );
        ptr::write_unaligned(ptr::addr_of_mut!((*header).cmsg_level), libc::SOL_SOCKET);
        ptr::write_unaligned(ptr::addr_of_mut!((*header).cmsg_type), libc::SCM_RIGHTS);
        ptr::copy_nonoverlapping(
            fds.as_ptr().cast::<u8>(),
            control.add(data_offset),
            size_of::<RawFd>() * fds.len(),
        );
    }

    let mut byte = [0x5a_u8];
    let mut iov = libc::iovec {
        iov_base: byte.as_mut_ptr().cast(),
        iov_len: byte.len(),
    };
    // SAFETY: all pointers reference live storage for the duration of sendmsg.
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.cast();
    message.msg_controllen = cmsg_space.try_into().unwrap();
    let sent = unsafe { libc::sendmsg(carrier.as_raw_fd(), &message, 0) };
    assert_eq!(
        sent,
        1,
        "sendmsg failed: {}",
        std::io::Error::last_os_error()
    );
}

fn receive_fds(carrier: &UnixStream) -> Vec<OwnedFd> {
    let capacity = cmsg_space(MAX_NATIVE_RIGHTS * size_of::<RawFd>());
    let mut control = vec![0_usize; capacity.div_ceil(size_of::<usize>())];
    // Allocate before intake; no allocation follows externalizing owned FDs.
    let mut rights = Vec::with_capacity(MAX_NATIVE_RIGHTS);
    let mut byte = [0u8; 1];
    let mut iov = libc::iovec {
        iov_base: byte.as_mut_ptr().cast(),
        iov_len: byte.len(),
    };
    // SAFETY: all pointers reference live writable storage for recvmsg.
    let mut message: libc::msghdr = unsafe { zeroed() };
    message.msg_iov = &mut iov;
    message.msg_iovlen = 1;
    message.msg_control = control.as_mut_ptr().cast();
    message.msg_controllen = capacity.try_into().unwrap();
    let received = unsafe { libc::recvmsg(carrier.as_raw_fd(), &mut message, 0) };
    if received >= 0 {
        // Acquire every visible native right before assertions so malformed
        // marker/count/truncation failures unwind through actual FD owners.
        unsafe {
            let mut header = libc::CMSG_FIRSTHDR(&message);
            while !header.is_null() {
                if (*header).cmsg_level == libc::SOL_SOCKET
                    && (*header).cmsg_type == libc::SCM_RIGHTS
                {
                    let length = usize::try_from((*header).cmsg_len).unwrap();
                    let base = cmsg_len(0);
                    assert!(length >= base);
                    let count = (length - base) / size_of::<RawFd>();
                    for index in 0..count {
                        let raw =
                            ptr::read_unaligned(libc::CMSG_DATA(header).cast::<RawFd>().add(index));
                        if raw >= 0 {
                            rights.push(OwnedFd::from_raw_fd(raw));
                        }
                    }
                }
                header = libc::CMSG_NXTHDR(&message, header);
            }
        }
    }
    assert_eq!(
        received,
        1,
        "recvmsg failed: {}",
        std::io::Error::last_os_error()
    );
    assert_eq!(
        message.msg_flags & libc::MSG_CTRUNC,
        0,
        "SCM_RIGHTS control was truncated"
    );

    assert_eq!(
        rights.len(),
        2,
        "expected payload and guardian in one SCM group"
    );
    rights
}

#[test]
fn actual_scm_rights_group_retains_payload_until_guardian_eof() {
    let (carrier_sender, carrier_receiver) = UnixStream::pair().unwrap();
    let (payload_sender, mut payload_peer) = UnixStream::pair().unwrap();
    let mut owner = ScmTransferLeaseOwner::new(authority(), 8, 4);
    let key = owner.mint_key().unwrap();
    let prepared = owner
        .prepare(send_context(), key, &[payload_sender.as_fd()])
        .unwrap();
    owner.arm_enqueued(send_context(), key).unwrap();

    send_fds(
        &carrier_sender,
        &[
            payload_sender.as_raw_fd(),
            prepared.guardian_writer().as_raw_fd(),
        ],
    );
    // The kernel now owns queued SCM references. Closing local sender copies
    // must not retire the lease while the receiver has not consumed them.
    drop(payload_sender);
    drop(prepared);
    assert_eq!(owner.retire_if_eof(key).unwrap(), Retirement::NotReady);

    let mut received = receive_fds(&carrier_receiver);
    let imported_payload = received.remove(0);
    let imported_guardian = received.remove(0);
    assert!(received.is_empty());
    let admission = owner
        .admit_import(receive_context(), key, 1)
        .expect("trusted fixture context should admit opposite carrier side");
    assert_eq!(admission.payload_count, 1);
    // Receiver-side transaction retains the guardian through admission, then
    // closes it. The core never takes ownership of this descriptor.
    drop(imported_guardian);
    assert_eq!(owner.retire_if_eof(key).unwrap(), Retirement::Retired);
    assert_eq!(owner.active_count(), 0);

    let mut imported = unsafe { UnixStream::from_raw_fd(imported_payload.into_raw_fd()) };
    payload_peer.write_all(b"ok").unwrap();
    let mut response = [0u8; 2];
    imported.read_exact(&mut response).unwrap();
    assert_eq!(&response, b"ok");
}

#[test]
fn abandoned_native_queue_retires_only_when_guardian_reaches_eof() {
    let (carrier_sender, carrier_receiver) = UnixStream::pair().unwrap();
    let (payload_sender, mut payload_peer) = UnixStream::pair().unwrap();
    let mut owner = ScmTransferLeaseOwner::new(authority(), 8, 4);
    let key = owner.mint_key().unwrap();
    let prepared = owner
        .prepare(send_context(), key, &[payload_sender.as_fd()])
        .unwrap();
    owner.arm_enqueued(send_context(), key).unwrap();
    send_fds(
        &carrier_sender,
        &[
            payload_sender.as_raw_fd(),
            prepared.guardian_writer().as_raw_fd(),
        ],
    );
    drop(payload_sender);
    drop(prepared);
    assert_eq!(owner.retire_if_eof(key).unwrap(), Retirement::NotReady);
    drop(carrier_receiver);
    drop(carrier_sender);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while owner.retire_if_eof(key).unwrap() != Retirement::Retired {
        assert!(
            std::time::Instant::now() < deadline,
            "native queue did not release guardian"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(owner.active_count(), 0);
    payload_peer.set_nonblocking(true).unwrap();
    assert_eq!(payload_peer.read(&mut [0]).unwrap(), 0);
}
