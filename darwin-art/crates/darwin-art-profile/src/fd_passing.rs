//! Narrow macOS SCM_RIGHTS boundary for a bounded descriptor envelope.

use std::{
    io,
    os::{
        fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
};

const MAX_DESCRIPTORS: usize = 257;

#[cfg(test)]
pub(crate) fn send_one(stream: &UnixStream, descriptor: BorrowedFd<'_>) -> io::Result<()> {
    send_many(stream, &[descriptor])
}

pub(crate) fn send_many(stream: &UnixStream, descriptors: &[BorrowedFd<'_>]) -> io::Result<()> {
    if descriptors.is_empty() || descriptors.len() > MAX_DESCRIPTORS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid SCM_RIGHTS descriptor count",
        ));
    }
    let mut marker = (descriptors.len() as u32).to_le_bytes();
    let mut vector = libc::iovec {
        iov_base: marker.as_mut_ptr().cast(),
        iov_len: marker.len(),
    };
    let rights_bytes = descriptors.len() * size_of::<i32>();
    // SAFETY: the bounded payload size is representable by CMSG_SPACE.
    let control_bytes = unsafe { libc::CMSG_SPACE(rights_bytes as u32) } as usize;
    let mut control = vec![0_usize; control_bytes.div_ceil(size_of::<usize>())];
    // SAFETY: all pointers refer to live stack storage for the duration of the
    // syscall; CMSG_* offsets are bounded by the oversized aligned buffer.
    let result = unsafe {
        let mut message = std::mem::zeroed::<libc::msghdr>();
        message.msg_iov = &mut vector;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = control_bytes as _;
        let header = libc::CMSG_FIRSTHDR(&message);
        if header.is_null() {
            return Err(io::Error::other("SCM_RIGHTS header does not fit"));
        }
        (*header).cmsg_level = libc::SOL_SOCKET;
        (*header).cmsg_type = libc::SCM_RIGHTS;
        (*header).cmsg_len = libc::CMSG_LEN(rights_bytes as u32) as _;
        for (index, descriptor) in descriptors.iter().enumerate() {
            std::ptr::write_unaligned(
                libc::CMSG_DATA(header).cast::<i32>().add(index),
                descriptor.as_raw_fd(),
            );
        }
        loop {
            let result = libc::sendmsg(stream.as_raw_fd(), &message, 0);
            if result < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break result;
        }
    };
    if result == marker.len() as isize {
        Ok(())
    } else if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Err(io::Error::new(
            io::ErrorKind::WriteZero,
            "SCM_RIGHTS marker was not sent",
        ))
    }
}

#[cfg(test)]
pub(crate) fn receive_one(stream: &UnixStream) -> io::Result<OwnedFd> {
    let mut descriptors = receive_many(stream)?;
    if descriptors.len() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected exactly one SCM_RIGHTS descriptor",
        ));
    }
    Ok(descriptors.pop().unwrap())
}

pub(crate) fn receive_many(stream: &UnixStream) -> io::Result<Vec<OwnedFd>> {
    let mut marker = [0_u8; 4];
    let mut vector = libc::iovec {
        iov_base: marker.as_mut_ptr().cast(),
        iov_len: marker.len(),
    };
    // SAFETY: the bounded payload size is representable by CMSG_SPACE.
    let control_bytes =
        unsafe { libc::CMSG_SPACE((MAX_DESCRIPTORS * size_of::<i32>()) as u32) } as usize;
    let mut control = vec![0_usize; control_bytes.div_ceil(size_of::<usize>())];
    // SAFETY: msghdr points only at live writable stack storage.
    let (result, message) = unsafe {
        let mut message = std::mem::zeroed::<libc::msghdr>();
        message.msg_iov = &mut vector;
        message.msg_iovlen = 1;
        message.msg_control = control.as_mut_ptr().cast();
        message.msg_controllen = control_bytes as _;
        let result = loop {
            let result = libc::recvmsg(stream.as_raw_fd(), &mut message, 0);
            if result < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                continue;
            }
            break result;
        };
        (result, message)
    };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    let expected = u32::from_le_bytes(marker) as usize;
    if result != marker.len() as isize
        || expected == 0
        || expected > MAX_DESCRIPTORS
        || message.msg_flags & libc::MSG_CTRUNC != 0
    {
        close_rights(&message);
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid SCM_RIGHTS envelope",
        ));
    }
    let descriptors = rights(&message);
    if descriptors.len() != expected {
        for descriptor in descriptors {
            // SAFETY: each descriptor was newly installed by recvmsg.
            unsafe { libc::close(descriptor) };
        }
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SCM_RIGHTS descriptor count mismatch",
        ));
    }
    let mut owned = Vec::new();
    if let Err(error) = owned.try_reserve_exact(descriptors.len()) {
        for descriptor in descriptors {
            // SAFETY: each descriptor was newly installed by recvmsg.
            unsafe { libc::close(descriptor) };
        }
        return Err(io::Error::other(error));
    }
    for (index, descriptor) in descriptors.iter().copied().enumerate() {
        // SAFETY: query/update the newly received live descriptor.
        if unsafe { libc::fcntl(descriptor, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
            let error = io::Error::last_os_error();
            for descriptor in &descriptors[index..] {
                unsafe { libc::close(*descriptor) };
            }
            return Err(error);
        }
        // SAFETY: each newly received descriptor transfers to OwnedFd once.
        owned.push(unsafe { OwnedFd::from_raw_fd(descriptor) });
    }
    Ok(owned)
}

fn rights(message: &libc::msghdr) -> Vec<i32> {
    let mut descriptors = Vec::new();
    // SAFETY: kernel populated the control buffer and msg_controllen; traversal
    // uses libc's bounds-aware CMSG macros.
    unsafe {
        let mut header = libc::CMSG_FIRSTHDR(message);
        while !header.is_null() {
            if (*header).cmsg_level == libc::SOL_SOCKET && (*header).cmsg_type == libc::SCM_RIGHTS {
                let base = libc::CMSG_LEN(0) as usize;
                let length = (*header).cmsg_len as usize;
                if length >= base {
                    let count = (length - base) / size_of::<i32>();
                    for index in 0..count {
                        descriptors.push(std::ptr::read_unaligned(
                            libc::CMSG_DATA(header).cast::<i32>().add(index),
                        ));
                    }
                }
            }
            header = libc::CMSG_NXTHDR(message, header);
        }
    }
    descriptors
}

fn close_rights(message: &libc::msghdr) {
    for descriptor in rights(message) {
        // SAFETY: every SCM_RIGHTS descriptor was installed by this recvmsg.
        unsafe { libc::close(descriptor) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::os::fd::AsFd;

    #[test]
    fn one_descriptor_crosses_and_is_close_on_exec() {
        let (sender, receiver) = UnixStream::pair().unwrap();
        let (payload, mut peer) = UnixStream::pair().unwrap();
        send_one(&sender, payload.as_fd()).unwrap();
        drop(payload);
        let received = receive_one(&receiver).unwrap();
        assert_ne!(
            unsafe { libc::fcntl(received.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
            0
        );
        drop(received);
        assert_eq!(peer.read(&mut [0]).unwrap(), 0);
    }

    #[test]
    fn ordered_descriptor_envelope_crosses_as_one_message() {
        let (sender, receiver) = UnixStream::pair().unwrap();
        let pairs: Vec<_> = (0..3).map(|_| UnixStream::pair().unwrap()).collect();
        let borrowed: Vec<_> = pairs.iter().map(|(local, _)| local.as_fd()).collect();
        send_many(&sender, &borrowed).unwrap();
        let received = receive_many(&receiver).unwrap();
        assert_eq!(received.len(), 3);
        for descriptor in &received {
            assert_ne!(
                unsafe { libc::fcntl(descriptor.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
                0
            );
        }
    }
}
