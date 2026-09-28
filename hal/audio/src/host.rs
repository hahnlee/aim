//! A stream's host side: a ring in a memfd shared with the host module
//! `audio` (`aim_hostcall::audio`), which plays it on (or captures it
//! from) the Mac's default device through CoreAudio.
//!
//! The ring is lock-free and single producer, single consumer. For output
//! this side writes and blocks, by sleeping, only while the ring is full;
//! the device callback reads without ever waiting for us. Input mirrors it.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::time::{Duration, Instant};

use aim_hostcall::audio::{
    DIRECTION_INPUT, DIRECTION_OUTPUT, FN_CLOSE, FN_START, FN_STOP, Open, RING_DATA_OFFSET, Ring,
};
use aim_hostcall::guest;

/// CLOCK_MONOTONIC (Android's `uptimeNanos` clock base), in nanoseconds.
pub fn now_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a local timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

pub struct HostStream {
    _fd: OwnedFd,
    base: *mut u8,
    length: usize,
    handle: u64,
    capacity: u64,
    frame_bytes: usize,
    rate: u32,
    input: bool,
}

// SAFETY: the mapping is shared memory used through the ring protocol; the
// stream worker is its only user on this side.
unsafe impl Send for HostStream {}
unsafe impl Sync for HostStream {}

impl HostStream {
    /// Open a stream of `capacity` frames on the host's default device.
    pub fn open(
        input: bool,
        format: u32,
        rate: u32,
        channels: u32,
        frame_bytes: usize,
        capacity: u32,
    ) -> io::Result<Self> {
        let length = RING_DATA_OFFSET + capacity as usize * frame_bytes;
        // SAFETY: plain libc calls; the fd is owned from here on.
        let (fd, base) = unsafe {
            let fd = libc::memfd_create(c"audio-ring".as_ptr(), libc::MFD_CLOEXEC);
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let fd = OwnedFd::from_raw_fd(fd);
            if libc::ftruncate(fd.as_raw_fd(), length as i64) != 0 {
                return Err(io::Error::last_os_error());
            }
            let base = libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            );
            if base == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            (fd, base.cast::<u8>())
        };
        // SAFETY: the fresh, zeroed header, before the host sees it.
        unsafe {
            let header = &mut *(base as *mut Ring);
            header.capacity_frames = capacity;
            header.frame_bytes = frame_bytes as u32;
        }
        let mut args = Open {
            direction: if input {
                DIRECTION_INPUT
            } else {
                DIRECTION_OUTPUT
            },
            format,
            sample_rate: rate,
            channels,
            fd: fd.as_raw_fd(),
            length: length as u64,
            ..Default::default()
        };
        let handle = guest::audio_open(&mut args).map_err(|e| {
            // SAFETY: our mapping.
            unsafe { libc::munmap(base.cast(), length) };
            io::Error::from_raw_os_error(e.0)
        })?;
        Ok(Self {
            _fd: fd,
            base,
            length,
            handle,
            capacity: capacity as u64,
            frame_bytes,
            rate,
            input,
        })
    }

    pub fn ring(&self) -> &Ring {
        // SAFETY: the mapping starts with the header.
        unsafe { &*(self.base as *const Ring) }
    }

    fn data(&self) -> *mut u8 {
        // SAFETY: in bounds.
        unsafe { self.base.add(RING_DATA_OFFSET) }
    }

    pub fn start(&self) -> bool {
        guest::audio_stream(FN_START, self.handle).is_ok()
    }

    /// Stop the device callback; returns once it no longer runs.
    pub fn stop(&self) -> bool {
        guest::audio_stream(FN_STOP, self.handle).is_ok()
    }

    /// Frames buffered in the ring.
    pub fn fill(&self) -> u64 {
        let r = self.ring();
        r.write.load(Acquire).wrapping_sub(r.read.load(Acquire))
    }

    /// Drop what the ring holds (output, with the callback stopped).
    pub fn discard(&self) {
        let r = self.ring();
        r.write.store(r.read.load(Acquire), Release);
    }

    fn copy(&self, at: u64, buf: *mut u8, frames: u64, to_ring: bool) {
        let fb = self.frame_bytes;
        let slot = at % self.capacity;
        let first = frames.min(self.capacity - slot);
        // SAFETY: both spans lie within the ring and `buf`.
        unsafe {
            let a = self.data().add(slot as usize * fb);
            let (n1, n2) = (first as usize * fb, (frames - first) as usize * fb);
            if to_ring {
                std::ptr::copy_nonoverlapping(buf, a, n1);
                std::ptr::copy_nonoverlapping(buf.add(n1), self.data(), n2);
            } else {
                std::ptr::copy_nonoverlapping(a, buf, n1);
                std::ptr::copy_nonoverlapping(self.data(), buf.add(n1), n2);
            }
        }
    }

    fn frames_ns(&self, frames: u64) -> u64 {
        frames * 1_000_000_000 / self.rate as u64
    }

    /// Output: append `data` (whole frames), sleeping while the ring is
    /// full, as a blocking PCM write does.
    pub fn write(&self, data: &[u8]) {
        let ring = self.ring();
        let mut frames = (data.len() / self.frame_bytes) as u64;
        let mut src = data.as_ptr();
        let mut marked = false;
        while frames > 0 {
            let w = ring.write.load(Relaxed);
            let free = self.capacity - w.wrapping_sub(ring.read.load(Acquire));
            if free == 0 {
                // Wake about when the device has consumed this write's
                // share of the ring, but at least every millisecond.
                let wait = self.frames_ns(frames.min(self.capacity / 2)).max(1_000_000);
                std::thread::sleep(Duration::from_nanos(wait));
                continue;
            }
            let n = free.min(frames);
            self.copy(w, src.cast_mut(), n, true);
            if !marked && ring.mark.load(Acquire) == 0 {
                ring.mark_ns.store(now_ns(), Relaxed);
                ring.mark.store(w + 1, Release);
                marked = true;
            }
            ring.write.store(w + n, Release);
            frames -= n;
            // SAFETY: within `data`.
            src = unsafe { src.add(n as usize * self.frame_bytes) };
        }
    }

    /// Input: fill `data` (whole frames) from the ring, waiting up to
    /// `timeout` for captured frames; what has not arrived by then is
    /// silence. Until the device's first callback (while macOS asks the
    /// user for microphone access, or when it was denied) the stream is
    /// silence at the capture rate, as from a muted microphone. Returns
    /// the frames that were captured.
    pub fn read(&self, data: &mut [u8], timeout: Duration) -> u64 {
        let ring = self.ring();
        let want = (data.len() / self.frame_bytes) as u64;
        if ring.callbacks.load(Acquire) == 0 {
            std::thread::sleep(Duration::from_nanos(self.frames_ns(want)));
            data.fill(0);
            return 0;
        }
        let deadline = Instant::now() + timeout;
        let mut done = 0u64;
        while done < want {
            let r = ring.read.load(Relaxed);
            let have = ring.write.load(Acquire).wrapping_sub(r);
            if have == 0 {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                let wait = Duration::from_nanos(self.frames_ns(want - done).max(1_000_000));
                std::thread::sleep(wait.min(deadline - now));
                continue;
            }
            let n = have.min(want - done);
            // SAFETY: within `data`.
            let dst = unsafe { data.as_mut_ptr().add(done as usize * self.frame_bytes) };
            self.copy(r, dst, n, false);
            ring.read.store(r + n, Release);
            done += n;
        }
        data[done as usize * self.frame_bytes..].fill(0);
        done
    }

    /// The last position the host stamped: (frames, CLOCK_MONOTONIC ns).
    pub fn stamp(&self) -> Option<(u64, i64)> {
        let ring = self.ring();
        loop {
            let seq = ring.stamp_seq.load(Acquire);
            if seq & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let frames = ring.stamp_frames.load(Relaxed);
            let ns = ring.stamp_ns.load(Relaxed);
            std::sync::atomic::fence(Acquire);
            if ring.stamp_seq.load(Relaxed) == seq {
                return (seq != 0).then_some((frames, ns));
            }
        }
    }

    /// Output: frames played out by now, extrapolated from the last stamp
    /// and bounded by what the device has taken from the ring.
    pub fn presented(&self, now: i64) -> u64 {
        let consumed = self.ring().read.load(Acquire);
        match self.stamp() {
            Some((frames, ns)) => {
                let elapsed = (now - ns) as i128 * self.rate as i128 / 1_000_000_000;
                (frames as i128 + elapsed).clamp(0, consumed as i128) as u64
            }
            None => 0,
        }
    }

    /// Device and stream latency, plus (for output) what the ring holds.
    pub fn latency_ms(&self) -> i32 {
        let device = self.ring().device_latency_frames.load(Relaxed) as u64;
        let queued = if self.input { 0 } else { self.fill() };
        ((device + queued) * 1000 / self.rate as u64) as i32
    }
}

impl Drop for HostStream {
    fn drop(&mut self) {
        let _ = guest::audio_stream(FN_CLOSE, self.handle);
        // SAFETY: our mapping; the host no longer uses the ring.
        unsafe { libc::munmap(self.base.cast(), self.length) };
    }
}
