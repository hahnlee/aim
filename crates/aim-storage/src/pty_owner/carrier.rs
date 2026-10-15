//! One kqueue worker forwards actual opaque-pipe EOF into the owner port set.
use crate::{
    posix_control::{SendRight, task},
    private_fd::PrivateFd,
};
use std::{
    io,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};
const EVENT: i32 = 0x50545943;
#[repr(C)]
struct Message {
    bits: u32,
    size: u32,
    remote: u32,
    local: u32,
    voucher: u32,
    id: i32,
    tag: u64,
    fd: i32,
    eof: u32,
}
unsafe extern "C" {
    fn mach_msg(
        message: *mut u8,
        options: i32,
        send_size: u32,
        receive_size: u32,
        name: u32,
        timeout: u32,
        notify: u32,
    ) -> i32;
}
struct Shared {
    queue: PrivateFd,
    sender: SendRight,
    stopping: AtomicBool,
    error: Mutex<Option<i32>>,
}
pub(super) struct Bridge {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}
impl Bridge {
    pub(super) fn new(sender: SendRight) -> io::Result<Self> {
        let queue = PrivateFd::allocate(|| {
            let fd = unsafe { libc::kqueue() };
            if fd < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(unsafe { OwnedFd::from_raw_fd(fd) })
            }
        })?;
        let event = libc::kevent {
            ident: 1,
            filter: libc::EVFILT_USER,
            flags: libc::EV_ADD | libc::EV_CLEAR,
            fflags: 0,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        if unsafe {
            libc::kevent(
                queue.as_raw_fd(),
                &event,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let shared = Arc::new(Shared {
            queue,
            sender,
            stopping: AtomicBool::new(false),
            error: Mutex::new(None),
        });
        let source = shared.clone();
        let worker = std::thread::Builder::new()
            .name("pty-carrier-events".into())
            .spawn(move || source.run())?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }
    pub(super) fn register(&self, fd: i32, tag: u64) -> io::Result<()> {
        self.healthy()?;
        let event = libc::kevent {
            ident: fd as usize,
            filter: libc::EVFILT_READ,
            flags: libc::EV_ADD | libc::EV_ONESHOT | libc::EV_RECEIPT,
            fflags: 0,
            data: 0,
            udata: tag as *mut _,
        };
        let mut receipt: libc::kevent = unsafe { std::mem::zeroed() };
        let zero = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let count = unsafe {
            libc::kevent(
                self.shared.queue.as_raw_fd(),
                &event,
                1,
                &mut receipt,
                1,
                &zero,
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        if count != 1 || receipt.flags & libc::EV_ERROR == 0 {
            return Err(io::Error::from_raw_os_error(libc::EPROTO));
        }
        if receipt.data != 0 {
            return Err(io::Error::from_raw_os_error(receipt.data as i32));
        }
        Ok(())
    }
    pub(super) fn healthy(&self) -> io::Result<()> {
        match *self.shared.error.lock().unwrap() {
            Some(error) => Err(io::Error::from_raw_os_error(error)),
            None => Ok(()),
        }
    }
    pub(super) fn watch_process(&self, pid: i32) -> io::Result<()> {
        self.healthy()?;
        let event = libc::kevent {
            ident: pid as usize, filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ONESHOT | libc::EV_RECEIPT,
            fflags: libc::NOTE_EXIT, data: 0, udata: std::ptr::null_mut(),
        };
        let mut receipt: libc::kevent = unsafe { std::mem::zeroed() };
        let zero = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        let count = unsafe { libc::kevent(self.shared.queue.as_raw_fd(), &event, 1, &mut receipt, 1, &zero) };
        if count < 0 { return Err(io::Error::last_os_error()); }
        if count != 1 || receipt.flags & libc::EV_ERROR == 0 { return Err(io::Error::from_raw_os_error(libc::EPROTO)); }
        if receipt.data != 0 { return Err(io::Error::from_raw_os_error(receipt.data as i32)); }
        Ok(())
    }
    pub(super) fn stop(&mut self) {
        self.shared.stopping.store(true, Ordering::Release);
        let event = libc::kevent {
            ident: 1,
            filter: libc::EVFILT_USER,
            flags: 0,
            fflags: libc::NOTE_TRIGGER,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        unsafe {
            libc::kevent(
                self.shared.queue.as_raw_fd(),
                &event,
                1,
                std::ptr::null_mut(),
                0,
                std::ptr::null(),
            );
        }
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                eprintln!("PTY carrier worker panicked");
            }
        }
    }
    pub(super) fn stopping(&self) {
        self.shared.stopping.store(true, Ordering::Release);
    }
}
impl Shared {
    fn run(&self) {
        let result = (|| loop {
            let mut events: [libc::kevent; 16] = unsafe { std::mem::zeroed() };
            let count = unsafe {
                libc::kevent(
                    self.queue.as_raw_fd(),
                    std::ptr::null(),
                    0,
                    events.as_mut_ptr(),
                    16,
                    std::ptr::null(),
                )
            };
            if count < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            for event in &events[..count as usize] {
                if self.stopping.load(Ordering::Acquire) || event.filter == libc::EVFILT_USER {
                    return Ok(());
                }
                if event.flags & libc::EV_ERROR != 0 {
                    return Err(io::Error::from_raw_os_error(event.data as i32));
                }
                let mut message = Message {
                    bits: 19,
                    size: 40,
                    remote: self.sender.name(),
                    local: 0,
                    voucher: 0,
                    id: EVENT,
                    tag: event.udata as u64,
                    fd: event.ident as i32,
                    eof: u32::from(event.flags & libc::EV_EOF != 0
                        || event.filter == libc::EVFILT_PROC && event.fflags & libc::NOTE_EXIT != 0),
                };
                if unsafe { mach_msg((&mut message as *mut Message).cast(), 1, 40, 0, 0, 0, 0) }
                    != 0
                {
                    return Err(io::Error::from_raw_os_error(libc::EIO));
                }
            }
        })();
        if let Err(error) = result {
            if !self.stopping.load(Ordering::Acquire) {
                *self.error.lock().unwrap() = Some(error.raw_os_error().unwrap_or(libc::EIO));
            }
        }
    }
}
pub(super) fn event(header_id: i32, words: &[u64]) -> Option<(u64, i32, bool)> {
    if header_id != EVENT {
        return None;
    }
    let message = unsafe { &*words.as_ptr().cast::<Message>() };
    (message.size == 40).then_some((message.tag, message.fd, message.eof != 0))
}
