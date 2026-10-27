//! liblog at android-16.0.0_r1, `logger_write.cpp::__android_log_bwrite`
//! and `logd_writer.cpp::LogdWrite`:
//! https://android.googlesource.com/platform/system/logging/+/android-16.0.0_r1/liblog/
//! The same LOG_ID_EVENTS, tag, tid and realtime framing, sent to
//! /dev/socket/logdw. logd takes the uid from this system-uid process's
//! socket credentials, never the host service's Binder caller.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::io;
use std::os::unix::net::UnixDatagram;

pub fn write(tag: i32, payload: &[u8]) -> io::Result<()> {
    // liblog truncates beyond LOGGER_ENTRY_MAX_PAYLOAD (including the tag).
    if payload.is_empty() || payload.len() > 4068 - 4 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut time = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: clock_gettime initializes the timespec on success.
    if unsafe { libc::clock_gettime(libc::CLOCK_REALTIME, time.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: clock_gettime succeeded; gettid takes no arguments.
    let (time, tid) = unsafe { (time.assume_init(), libc::gettid()) };
    // `android_log_header_t` is packed: uint8 id, uint16 tid, log_time
    // (uint32 seconds, uint32 nanoseconds). __android_log_bwrite follows
    // it with the little-endian tag and the typed payload.
    let mut message = Vec::with_capacity(11 + 4 + payload.len());
    message.push(2); // LOG_ID_EVENTS
    message.extend((tid as u16).to_le_bytes());
    message.extend((time.tv_sec as u32).to_le_bytes());
    message.extend((time.tv_nsec as u32).to_le_bytes());
    message.extend(tag.to_le_bytes());
    message.extend(payload);
    let socket = UnixDatagram::unbound()?;
    socket.set_nonblocking(true)?;
    socket.connect("/dev/socket/logdw")?;
    loop {
        match socket.send(&message) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
            Ok(written) if written != message.len() => return Err(io::ErrorKind::WriteZero.into()),
            Ok(_) => return Ok(()),
        }
    }
}
