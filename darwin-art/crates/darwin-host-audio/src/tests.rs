//! The module against the Mac's real default devices. Output plays a
//! square wave of one LSB (-90 dBFS, inaudible) for a fraction of a
//! second, checked through the callback's counters and peak; the
//! microphone test is ignored by default because it asks macOS for
//! microphone access.

use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::time::{Duration, Instant};

use darwin_hostcall::audio::{
    DIRECTION_INPUT, DIRECTION_OUTPUT, FORMAT_FLOAT, FORMAT_PCM_16, Open, RING_DATA_OFFSET, Ring,
    Stream,
};
use darwin_hostcall::errno::{EINVAL, ENOSYS};

use super::*;

/// A ring file like the guest's memfd, mapped by the test.
struct TestRing {
    _file: std::fs::File,
    fd: i32,
    base: *mut u8,
    length: usize,
}

impl TestRing {
    fn new(name: &str, capacity: u32, frame_bytes: u32) -> Self {
        use std::os::fd::AsRawFd;
        let path = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        let _ = std::fs::remove_file(&path);
        let length = RING_DATA_OFFSET + (capacity * frame_bytes) as usize;
        file.set_len(length as u64).unwrap();
        let fd = file.as_raw_fd();
        // SAFETY: a shared mapping of our own file.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        assert_ne!(base, libc::MAP_FAILED);
        let ring = Self {
            _file: file,
            fd,
            base: base.cast(),
            length,
        };
        // SAFETY: the header of the fresh (zeroed) mapping.
        unsafe {
            let header = &mut *(ring.base as *mut Ring);
            header.capacity_frames = capacity;
            header.frame_bytes = frame_bytes;
        }
        ring
    }

    fn header(&self) -> &Ring {
        // SAFETY: the mapping starts with the header.
        unsafe { &*(self.base as *const Ring) }
    }

    fn data(&self) -> *mut u8 {
        // SAFETY: in bounds.
        unsafe { self.base.add(RING_DATA_OFFSET) }
    }
}

impl Drop for TestRing {
    fn drop(&mut self) {
        // SAFETY: our mapping.
        unsafe { libc::munmap(self.base.cast(), self.length) };
    }
}

fn host_call<T>(func: u32, args: &mut T) -> i64 {
    // SAFETY: a live argument block of its exact size.
    unsafe { call(func, args as *mut T as u64, std::mem::size_of::<T>() as u64) }
}

fn open(ring: &TestRing, direction: u32, format: u32, rate: u32, channels: u32) -> i64 {
    let mut args = Open {
        direction,
        format,
        sample_rate: rate,
        channels,
        fd: ring.fd,
        length: ring.length as u64,
        ..Default::default()
    };
    match host_call(FN_OPEN, &mut args) {
        0 => args.stream as i64,
        e => e,
    }
}

#[test]
fn rejects_bad_calls() {
    let mut s = Stream { stream: 12345 };
    assert_eq!(host_call(FN_START, &mut s), -(EINVAL as i64));
    assert_eq!(host_call(FN_CLOSE, &mut s), -(EINVAL as i64));
    assert_eq!(host_call(99, &mut s), -(ENOSYS as i64));
    // SAFETY: a wrong length is refused before the block is touched.
    assert_eq!(
        unsafe { call(FN_START, &mut s as *mut _ as u64, 4) },
        -(EINVAL as i64)
    );
    let ring = TestRing::new("audio-bad", 1024, 4);
    assert_eq!(open(&ring, DIRECTION_OUTPUT, 7, 48000, 2), -(EINVAL as i64));
    assert_eq!(open(&ring, 5, FORMAT_PCM_16, 48000, 2), -(EINVAL as i64));
    // The header's frame size must be the format's.
    assert_eq!(
        open(&ring, DIRECTION_OUTPUT, FORMAT_FLOAT, 48000, 2),
        -(EINVAL as i64)
    );
}

#[test]
fn describes_the_default_output() {
    let d = devices();
    if d.output.present == 0 {
        eprintln!("skipped: no default output device");
        return;
    }
    assert!(d.output.sample_rate >= 8000, "{:?}", d.output);
    assert!(d.output.channels >= 1, "{:?}", d.output);
    assert!(d.output.buffer_frames > 0, "{:?}", d.output);
    let name = String::from_utf8_lossy(&d.output.name);
    eprintln!(
        "output: {} {} Hz {} ch, buffer {} frames, latency {} frames",
        name.trim_end_matches('\0'),
        d.output.sample_rate,
        d.output.channels,
        d.output.buffer_frames,
        d.output.latency_frames
    );
}

#[test]
fn plays_a_ring_through_the_default_output() {
    if devices().output.present == 0 {
        eprintln!("skipped: no default output device");
        return;
    }
    const RATE: u32 = 48000;
    const CAPACITY: u32 = 4096;
    let ring = TestRing::new("audio-out", CAPACITY, 4);
    let stream = open(&ring, DIRECTION_OUTPUT, FORMAT_PCM_16, RATE, 2);
    assert!(stream > 0, "open: {stream}");
    let h = ring.header();
    let mut s = Stream {
        stream: stream as u64,
    };
    assert_eq!(host_call(FN_START, &mut s), 0);
    // A 375 Hz square wave of one LSB, kept topped up for 300 ms.
    let mut phase = 0u64;
    let end = Instant::now() + Duration::from_millis(300);
    let mut marked = false;
    while Instant::now() < end {
        let write = h.write.load(Relaxed);
        let free = CAPACITY as u64 - (write - h.read.load(Acquire));
        for i in 0..free {
            let v: i16 = if phase / 64 % 2 == 0 { 1 } else { -1 };
            let slot = ((write + i) % CAPACITY as u64) as usize * 4;
            // SAFETY: a free slot of the ring.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    [v, v].as_ptr().cast::<u8>(),
                    ring.data().add(slot),
                    4,
                )
            };
            phase += 1;
        }
        if !marked && free > 0 {
            h.mark_ns.store(clock::now_ns(), Relaxed);
            h.mark.store(write + 1, Release);
            marked = true;
        }
        h.write.store(write + free, Release);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(host_call(FN_STOP, &mut s), 0);
    let read = h.read.load(Acquire);
    let peak = f32::from_bits(h.peak_bits.load(Relaxed));
    eprintln!(
        "callbacks {} read {read} xruns {} peak {peak} latency {} us",
        h.callbacks.load(Relaxed),
        h.xruns.load(Relaxed),
        h.latency_sum_ns.load(Relaxed) / h.latency_count.load(Relaxed).max(1) / 1000
    );
    assert!(h.callbacks.load(Relaxed) > 5);
    assert!(read > RATE as u64 / 10, "read {read}");
    assert_eq!(peak, 1.0 / 32768.0, "peak");
    assert_eq!(h.latency_count.load(Relaxed), 1);
    // The stamp maps a consumed position to a time near now.
    let (frames, ns) = (h.stamp_frames.load(Relaxed), h.stamp_ns.load(Relaxed));
    assert!(frames <= read);
    assert!((ns - clock::now_ns()).abs() < 1_000_000_000, "stamp {ns}");
    assert_eq!(host_call(FN_CLOSE, &mut s), 0);
    assert_eq!(host_call(FN_START, &mut s), -(EINVAL as i64));
}

#[test]
#[ignore = "opens the microphone, which asks macOS for access on first use"]
fn captures_from_the_default_input() {
    let d = devices();
    if d.input.present == 0 {
        eprintln!("skipped: no default input device");
        return;
    }
    let ring = TestRing::new("audio-in", 8192, 8);
    let stream = open(&ring, DIRECTION_INPUT, FORMAT_FLOAT, d.input.sample_rate, 2);
    assert!(stream > 0, "open: {stream}");
    let h = ring.header();
    let mut s = Stream {
        stream: stream as u64,
    };
    assert_eq!(host_call(FN_START, &mut s), 0);
    let end = Instant::now() + Duration::from_secs(2);
    while Instant::now() < end {
        // Consume everything, as the guest reader would.
        h.read.store(h.write.load(Acquire), Release);
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(host_call(FN_CLOSE, &mut s), 0);
    eprintln!(
        "captured {} frames in {} callbacks, peak {}",
        h.write.load(Relaxed),
        h.callbacks.load(Relaxed),
        f32::from_bits(h.peak_bits.load(Relaxed))
    );
    assert!(h.write.load(Relaxed) > d.input.sample_rate as u64 / 4);
}
