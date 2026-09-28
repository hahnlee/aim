//! One stream: the guest's ring mapped on the host side, and an AUHAL unit
//! whose callback moves frames between the ring and the device.
//!
//! The callback runs on CoreAudio's I/O thread. It never blocks, never
//! allocates and never waits for the guest: it moves what the ring holds
//! (output) or has room for (input), counts the rest as xruns, and
//! publishes positions and timestamps through the ring header.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::sync::atomic::AtomicPtr;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use darwin_hostcall::audio::{
    DIRECTION_INPUT, DIRECTION_OUTPUT, FORMAT_FLOAT, FORMAT_PCM_16, Open, RING_DATA_OFFSET, Ring,
};
use darwin_hostcall::errno::{EINVAL, ENODEV};

use crate::coreaudio::{self as ca, AudioBufferList, AudioTimeStamp, AudioUnit, OSStatus};

/// State the callback reads; boxed so its address is stable. The null
/// sink (`crate::null`) drives the same callbacks from a timer.
pub(crate) struct Shared {
    base: *mut u8,
    length: usize,
    capacity: u64,
    frame_bytes: usize,
    float: bool,
    latency_ns: i64,
    /// The unit, for AudioUnitRender on the input side.
    unit: AtomicPtr<c_void>,
    /// Input capture buffer, touched only by the callback.
    scratch: UnsafeCell<Vec<u8>>,
}

// SAFETY: `scratch` is used by the single callback thread only (and by the
// setup before the callback can run); the rest is immutable or atomic.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Shared {
    /// Map the guest's ring for a stream of `args`' format and check it.
    pub(crate) fn new(args: &Open, latency_ns: i64) -> Result<Box<Self>, i32> {
        let sample_bytes = match args.format {
            FORMAT_PCM_16 => 2,
            FORMAT_FLOAT => 4,
            _ => return Err(EINVAL),
        };
        if !matches!(args.direction, DIRECTION_OUTPUT | DIRECTION_INPUT)
            || !(1..=8).contains(&args.channels)
            || !(8000..=192_000).contains(&args.sample_rate)
        {
            return Err(EINVAL);
        }
        let frame_bytes = args.channels as usize * sample_bytes;
        let (base, length) = map(args.fd, args.length)?;
        let mut shared = Box::new(Shared {
            base,
            length,
            capacity: 0,
            frame_bytes,
            float: args.format == FORMAT_FLOAT,
            latency_ns,
            unit: AtomicPtr::new(std::ptr::null_mut()),
            scratch: UnsafeCell::new(Vec::new()),
        });
        let ring = shared.ring();
        let capacity = ring.capacity_frames as u64;
        if ring.frame_bytes as usize != frame_bytes
            || capacity == 0
            || RING_DATA_OFFSET as u64 + capacity * frame_bytes as u64 > length as u64
        {
            return Err(EINVAL);
        }
        shared.capacity = capacity;
        Ok(shared)
    }

    pub(crate) fn frame_bytes(&self) -> usize {
        self.frame_bytes
    }

    pub(crate) fn ring(&self) -> &Ring {
        // SAFETY: the mapping starts with a Ring and lives as long as self.
        unsafe { &*(self.base as *const Ring) }
    }

    fn data(&self) -> *mut u8 {
        // SAFETY: the mapping is longer than the header (checked at open).
        unsafe { self.base.add(RING_DATA_OFFSET) }
    }

    /// Copy `frames` frames between the ring at position `at` and `buf`,
    /// wrapping at the end of the ring.
    ///
    /// # Safety
    /// `buf` holds `frames` frames; the ring slots are owned by the caller
    /// (the side that has not yet published them).
    unsafe fn copy(&self, at: u64, buf: *mut u8, frames: u64, to_ring: bool) {
        let first = frames.min(self.capacity - at % self.capacity);
        let spans = [(at % self.capacity, 0, first), (0, first, frames - first)];
        for (slot, offset, count) in spans {
            if count == 0 {
                continue;
            }
            let fb = self.frame_bytes;
            // SAFETY: both ranges are in bounds (caller contract).
            unsafe {
                let ring = self.data().add(slot as usize * fb);
                let buf = buf.add(offset as usize * fb);
                let n = count as usize * fb;
                if to_ring {
                    std::ptr::copy_nonoverlapping(buf, ring, n);
                } else {
                    std::ptr::copy_nonoverlapping(ring, buf, n);
                }
            }
        }
    }

    /// Raise the peak to the largest |sample| of `bytes`.
    fn note_peak(&self, bytes: &[u8]) {
        let peak = if self.float {
            bytes
                .chunks_exact(4)
                .map(|s| f32::from_ne_bytes(s.try_into().unwrap()).abs())
                .fold(0.0f32, f32::max)
        } else {
            bytes
                .chunks_exact(2)
                .map(|s| i16::from_ne_bytes(s.try_into().unwrap()).unsigned_abs())
                .max()
                .unwrap_or(0) as f32
                / 32768.0
        };
        let ring = self.ring();
        if peak > f32::from_bits(ring.peak_bits.load(Relaxed)) {
            ring.peak_bits.store(peak.to_bits(), Relaxed);
        }
    }

    fn stamp(&self, frames: u64, ns: i64) {
        let ring = self.ring();
        let seq = ring.stamp_seq.load(Relaxed);
        ring.stamp_seq.store(seq.wrapping_add(1), Relaxed);
        std::sync::atomic::fence(Release);
        ring.stamp_frames.store(frames, Relaxed);
        ring.stamp_ns.store(ns, Relaxed);
        ring.stamp_seq.store(seq.wrapping_add(2), Release);
    }

    pub(crate) fn account(&self, started: i64) {
        let ring = self.ring();
        ring.callbacks.fetch_add(1, Relaxed);
        ring.callback_ns
            .fetch_add((clock::now_ns() - started).max(0) as u64, Relaxed);
    }
}

/// Output: fill the device's buffer from the ring.
pub(crate) unsafe extern "C" fn render(
    refcon: *mut c_void,
    _flags: *mut u32,
    time: *const AudioTimeStamp,
    _bus: u32,
    frames: u32,
    data: *mut AudioBufferList,
) -> OSStatus {
    let started = clock::now_ns();
    // SAFETY: CoreAudio passes our refcon and a one-buffer interleaved list.
    let (s, buffer, host_time) = unsafe {
        (
            &*(refcon as *const Shared),
            &mut (*data).buffers[0],
            (*time).host_time,
        )
    };
    let ring = s.ring();
    let want = (buffer.byte_size as usize / s.frame_bytes).min(frames as usize) as u64;
    if ring.detached.load(Acquire) != 0 {
        // SAFETY: the device buffer holds `want` frames.
        unsafe { std::ptr::write_bytes(buffer.data as *mut u8, 0, want as usize * s.frame_bytes) };
        return 0;
    }
    let read = ring.read.load(Relaxed);
    let have = (ring.write.load(Acquire).wrapping_sub(read)).min(s.capacity);
    let n = have.min(want);
    let out = buffer.data as *mut u8;
    // SAFETY: frames [read, read + n) were published by the guest; the
    // device buffer holds `want` frames.
    unsafe {
        s.copy(read, out, n, false);
        std::ptr::write_bytes(
            out.add(n as usize * s.frame_bytes),
            0,
            (want - n) as usize * s.frame_bytes,
        );
        s.note_peak(std::slice::from_raw_parts(out, n as usize * s.frame_bytes));
    }
    if n < want {
        ring.xruns.fetch_add(want - n, Relaxed);
    }
    let mark = ring.mark.load(Acquire);
    if mark != 0 && mark - 1 < read + n {
        let delay = (started - ring.mark_ns.load(Relaxed)).max(0) as u64;
        ring.latency_count.fetch_add(1, Relaxed);
        ring.latency_sum_ns.fetch_add(delay, Relaxed);
        ring.latency_max_ns.fetch_max(delay, Relaxed);
        ring.mark.store(0, Release);
    }
    // An empty ring leaves the stamp at the last frame played, so the
    // position stops there.
    if n > 0 {
        s.stamp(read, clock::host_to_ns(host_time) + s.latency_ns);
    }
    ring.read.store(read + n, Release);
    s.account(started);
    0
}

/// Input: pull the captured frames and append them to the ring.
unsafe extern "C" fn capture(
    refcon: *mut c_void,
    flags: *mut u32,
    time: *const AudioTimeStamp,
    bus: u32,
    frames: u32,
    _data: *mut AudioBufferList,
) -> OSStatus {
    let started = clock::now_ns();
    // SAFETY: our refcon; the scratch buffer is this thread's alone.
    let s = unsafe { &*(refcon as *const Shared) };
    if s.ring().detached.load(Acquire) != 0 {
        return 0;
    }
    // SAFETY: the scratch buffer is this callback thread's alone.
    let scratch = unsafe { &mut *s.scratch.get() };
    let bytes = frames as usize * s.frame_bytes;
    if bytes > scratch.len() {
        s.ring().xruns.fetch_add(frames as u64, Relaxed);
        return 0;
    }
    let mut list = AudioBufferList {
        count: 1,
        buffers: [ca::AudioBuffer {
            channels: (s.frame_bytes / if s.float { 4 } else { 2 }) as u32,
            byte_size: bytes as u32,
            data: scratch.as_mut_ptr().cast(),
        }],
    };
    // SAFETY: the unit renders `frames` frames into our buffer.
    let status =
        unsafe { ca::AudioUnitRender(s.unit.load(Acquire), flags, time, bus, frames, &mut list) };
    if status != 0 {
        return status;
    }
    // SAFETY: CoreAudio's timestamp for this capture.
    let host_time = unsafe { (*time).host_time };
    s.push_captured(&scratch[..bytes], clock::host_to_ns(host_time));
    s.account(started);
    0
}

impl Shared {
    /// Append captured frames, captured at CLOCK_MONOTONIC `captured_ns`,
    /// dropping what does not fit (counted as xruns).
    pub(crate) fn push_captured(&self, bytes: &[u8], captured_ns: i64) {
        let ring = self.ring();
        let frames = (bytes.len() / self.frame_bytes) as u64;
        self.note_peak(bytes);
        let write = ring.write.load(Relaxed);
        let free = self.capacity
            - write
                .wrapping_sub(ring.read.load(Acquire))
                .min(self.capacity);
        let n = frames.min(free);
        // SAFETY: slots [write, write + n) are free until published below.
        unsafe { self.copy(write, bytes.as_ptr().cast_mut(), n, true) };
        if n < frames {
            ring.xruns.fetch_add(frames - n, Relaxed);
        }
        self.stamp(write, captured_ns - self.latency_ns);
        ring.write.store(write + n, Release);
    }
}

impl Drop for Shared {
    fn drop(&mut self) {
        unmap(self.base, self.length);
    }
}

/// The unit's lifecycle, under the stream's lock.
#[derive(Default)]
struct State {
    /// Set once the unit is configured and initialized.
    unit: Option<Unit>,
    running: bool,
    /// An input unit is being set up on its own thread.
    setting_up: bool,
    closed: bool,
}

struct Unit(AudioUnit);

// SAFETY: an AudioUnit may be driven from any thread, one at a time (the
// stream's lock).
unsafe impl Send for Unit {}

impl Drop for Unit {
    fn drop(&mut self) {
        // SAFETY: after dispose no callback runs.
        unsafe {
            ca::AudioOutputUnitStop(self.0);
            ca::AudioUnitUninitialize(self.0);
            ca::AudioComponentInstanceDispose(self.0);
        }
    }
}

struct Inner {
    // Dropped in this order: the unit (and its callback) before the ring.
    state: Mutex<State>,
    shared: Box<Shared>,
    args: Open,
    device_channels: u32,
}

pub struct Stream(Arc<Inner>);

/// Input units being set up, which [`wait_for_setups`] waits for.
static SETUPS: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

/// Wait up to `timeout` for input units still being set up to finish, so
/// that none is left half made when the process exits.
pub fn wait_for_setups(timeout: Duration) -> bool {
    let (count, done) = &SETUPS;
    let guard = count.lock().unwrap_or_else(|e| e.into_inner());
    let (guard, _) = done
        .wait_timeout_while(guard, timeout, |n| *n > 0)
        .unwrap_or_else(|e| e.into_inner());
    *guard == 0
}

impl Stream {
    /// Map the ring and set up (but do not start) an output unit. An input
    /// unit is set up on the first start, on a thread of its own: its first
    /// use asks macOS for microphone access and waits for the user's
    /// answer, and the guest must not wait with it. Until then the ring
    /// stays empty.
    pub fn open(args: &Open) -> Result<Self, i32> {
        let input = args.direction == DIRECTION_INPUT;
        let device = ca::describe(input);
        if device.present == 0 {
            return Err(ENODEV);
        }
        // Without a converter, AUHAL captures at the device's own rate.
        if input && device.sample_rate != args.sample_rate {
            return Err(EINVAL);
        }
        let latency_ns = device.latency_frames as i64 * 1_000_000_000 / device.sample_rate as i64;
        let shared = Shared::new(args, latency_ns)?;
        shared
            .ring()
            .device_latency_frames
            .store(device.latency_frames, Relaxed);
        let inner = Arc::new(Inner {
            state: Mutex::new(State::default()),
            shared,
            args: *args,
            device_channels: device.channels,
        });
        if !input {
            let unit = new_unit(&inner.shared, args, device.channels).ok_or(ENODEV)?;
            inner.lock().unit = Some(unit);
        }
        Ok(Self(inner))
    }

    /// Run the callback (as soon as the unit is set up, for input).
    pub fn start(&self) -> bool {
        let mut state = self.0.lock();
        state.running = true;
        if let Some(unit) = &state.unit {
            return start(unit);
        }
        if !state.setting_up {
            state.setting_up = true;
            *SETUPS.0.lock().unwrap_or_else(|e| e.into_inner()) += 1;
            let pending = self.0.clone();
            let spawned = std::thread::Builder::new()
                .name("audio-input-setup".into())
                .spawn(move || pending.set_up());
            if spawned.is_err() {
                self.0.set_up_done(&mut state);
                return false;
            }
        }
        true
    }

    /// Stop the callback; returns once it no longer runs.
    pub fn stop(&self) -> bool {
        let mut state = self.0.lock();
        state.running = false;
        // SAFETY: an initialized unit.
        state
            .unit
            .as_ref()
            .is_none_or(|u| unsafe { ca::AudioOutputUnitStop(u.0) == 0 })
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // A pending input setup finds the stream closed and drops its
        // unit; the last reference unmaps the ring.
        let mut state = self.0.lock();
        state.closed = true;
        state.unit = None;
    }
}

fn start(unit: &Unit) -> bool {
    // SAFETY: an initialized unit.
    unsafe { ca::AudioOutputUnitStart(unit.0) == 0 }
}

impl Inner {
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Configure and initialize the input unit, then start it if the
    /// guest still wants it running.
    fn set_up(&self) {
        let unit = new_unit(&self.shared, &self.args, self.device_channels);
        let mut state = self.lock();
        if let Some(unit) = unit
            && !state.closed
            && (!state.running || start(&unit))
        {
            state.unit = Some(unit);
        }
        self.set_up_done(&mut state);
    }

    fn set_up_done(&self, state: &mut State) {
        state.setting_up = false;
        let (count, done) = &SETUPS;
        let mut n = count.lock().unwrap_or_else(|e| e.into_inner());
        *n -= 1;
        done.notify_all();
    }
}

fn new_unit(shared: &Shared, args: &Open, device_channels: u32) -> Option<Unit> {
    let input = args.direction == DIRECTION_INPUT;
    let float = args.format == FORMAT_FLOAT;
    let format = ca::AudioStreamBasicDescription {
        sample_rate: args.sample_rate as f64,
        format_id: ca::FORMAT_LINEAR_PCM,
        format_flags: ca::FORMAT_FLAG_IS_PACKED
            | if float {
                ca::FORMAT_FLAG_IS_FLOAT
            } else {
                ca::FORMAT_FLAG_IS_SIGNED_INTEGER
            },
        bytes_per_packet: shared.frame_bytes as u32,
        frames_per_packet: 1,
        bytes_per_frame: shared.frame_bytes as u32,
        channels_per_frame: args.channels,
        bits_per_channel: if float { 32 } else { 16 },
        reserved: 0,
    };
    let refcon = (shared as *const Shared).cast_mut().cast::<c_void>();
    let unit = Unit(ca::new_unit(if input {
        ca::UNIT_SUBTYPE_HAL_OUTPUT
    } else {
        ca::UNIT_SUBTYPE_DEFAULT_OUTPUT
    })?);
    let u = unit.0;
    let configured = if input {
        let device = ca::default_device(true)?;
        ca::set_property(u, ca::PROPERTY_ENABLE_IO, ca::SCOPE_INPUT, 1, &1u32)
            && ca::set_property(u, ca::PROPERTY_ENABLE_IO, ca::SCOPE_OUTPUT, 0, &0u32)
            && ca::set_property(u, ca::PROPERTY_CURRENT_DEVICE, ca::SCOPE_GLOBAL, 0, &device)
            && ca::set_property(u, ca::PROPERTY_STREAM_FORMAT, ca::SCOPE_OUTPUT, 1, &format)
            // A mono microphone feeds every channel of the stream.
            && (device_channels != 1
                || ca::set_property_slice(
                    u,
                    ca::PROPERTY_CHANNEL_MAP,
                    ca::SCOPE_OUTPUT,
                    1,
                    &vec![0i32; args.channels as usize],
                ))
            && ca::set_property(
                u,
                ca::PROPERTY_SET_INPUT_CALLBACK,
                ca::SCOPE_GLOBAL,
                0,
                &ca::AURenderCallbackStruct {
                    proc_: capture,
                    refcon,
                },
            )
    } else {
        ca::set_property(u, ca::PROPERTY_STREAM_FORMAT, ca::SCOPE_INPUT, 0, &format)
            && ca::set_property(
                u,
                ca::PROPERTY_SET_RENDER_CALLBACK,
                ca::SCOPE_INPUT,
                0,
                &ca::AURenderCallbackStruct {
                    proc_: render,
                    refcon,
                },
            )
    };
    if input {
        let mut slice = 0u32;
        let mut size = 4u32;
        // SAFETY: a u32 property into a local; the callback cannot run
        // yet, so the scratch buffer is ours.
        unsafe {
            ca::AudioUnitGetProperty(
                u,
                ca::PROPERTY_MAXIMUM_FRAMES_PER_SLICE,
                ca::SCOPE_GLOBAL,
                0,
                (&mut slice as *mut u32).cast(),
                &mut size,
            );
            *shared.scratch.get() = vec![0; slice.max(4096) as usize * shared.frame_bytes];
        }
        shared.unit.store(u, Release);
    }
    // SAFETY: a configured unit.
    (configured && unsafe { ca::AudioUnitInitialize(u) } == 0).then_some(unit)
}

/// Map the guest's ring file. The guest's fd is a host fd (the syscall
/// layer's descriptors are the process's own).
fn map(fd: i32, length: u64) -> Result<(*mut u8, usize), i32> {
    let length = usize::try_from(length).map_err(|_| EINVAL)?;
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat and mmap of a descriptor the caller names.
    unsafe {
        if length < RING_DATA_OFFSET
            || libc::fstat(fd, &mut st) != 0
            || (st.st_size as u64) < length as u64
        {
            return Err(EINVAL);
        }
        let base = libc::mmap(
            std::ptr::null_mut(),
            length,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_SHARED,
            fd,
            0,
        );
        if base == libc::MAP_FAILED {
            return Err(EINVAL);
        }
        Ok((base.cast(), length))
    }
}

fn unmap(base: *mut u8, length: usize) {
    // SAFETY: a mapping made by `map`.
    unsafe { libc::munmap(base.cast(), length) };
}

/// Host time (mach absolute time, which CoreAudio stamps) on the guest's
/// CLOCK_MONOTONIC, which the syscall layer takes from the host's.
pub mod clock {
    use std::sync::OnceLock;

    #[repr(C)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }

    unsafe extern "C" {
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> i32;
        fn mach_absolute_time() -> u64;
    }

    pub fn now_ns() -> i64 {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: a local timespec.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
        ts.tv_sec * 1_000_000_000 + ts.tv_nsec
    }

    fn timebase() -> (i64, i64) {
        static TIMEBASE: OnceLock<(i64, i64)> = OnceLock::new();
        *TIMEBASE.get_or_init(|| {
            let mut info = MachTimebaseInfo { numer: 0, denom: 0 };
            // SAFETY: a local out-parameter.
            unsafe { mach_timebase_info(&mut info) };
            (info.numer as i64, info.denom.max(1) as i64)
        })
    }

    /// CLOCK_MONOTONIC nanoseconds of a mach absolute time.
    pub fn host_to_ns(host_time: u64) -> i64 {
        let (numer, denom) = timebase();
        // SAFETY: reads the clock.
        let now = unsafe { mach_absolute_time() };
        now_ns() + (host_time as i64 - now as i64) * numer / denom
    }
}

#[cfg(test)]
mod tests {
    //! The callbacks' ring logic without a device: a loopback test device
    //! feeds what the output callback renders into an input ring.

    use super::*;

    const CAPACITY: u64 = 256;
    const FRAME: usize = 4; // PCM 16, stereo

    fn shared() -> Shared {
        let length = RING_DATA_OFFSET + CAPACITY as usize * FRAME;
        // SAFETY: a fresh anonymous shared mapping, unmapped on drop.
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED | libc::MAP_ANON,
                -1,
                0,
            )
        };
        assert_ne!(base, libc::MAP_FAILED);
        Shared {
            base: base.cast(),
            length,
            capacity: CAPACITY,
            frame_bytes: FRAME,
            float: false,
            latency_ns: 1_000_000,
            unit: AtomicPtr::new(std::ptr::null_mut()),
            scratch: UnsafeCell::new(Vec::new()),
        }
    }

    unsafe extern "C" {
        fn mach_absolute_time() -> u64;
    }

    /// The device asks the output callback for `frames` frames.
    fn device_pull(out: &Shared, frames: u32) -> Vec<u8> {
        let mut buf = vec![0xffu8; frames as usize * FRAME];
        let mut list = AudioBufferList {
            count: 1,
            buffers: [ca::AudioBuffer {
                channels: 2,
                byte_size: buf.len() as u32,
                data: buf.as_mut_ptr().cast(),
            }],
        };
        let time = AudioTimeStamp {
            sample_time: 0.0,
            // SAFETY: reads the clock.
            host_time: unsafe { mach_absolute_time() },
            rate_scalar: 1.0,
            word_clock_time: 0,
            smpte_time: [0; 24],
            flags: 0,
            reserved: 0,
        };
        let refcon = (out as *const Shared).cast_mut().cast();
        // SAFETY: the callback with a valid list and timestamp.
        let status = unsafe { render(refcon, std::ptr::null_mut(), &time, 0, frames, &mut list) };
        assert_eq!(status, 0);
        buf
    }

    /// The guest side of an output ring: append samples `from..from+frames`.
    fn guest_write(out: &Shared, from: u16, frames: u16) {
        let ring = out.ring();
        let w = ring.write.load(Relaxed);
        let bytes: Vec<u8> = (from..from + frames)
            .flat_map(|v| [v.to_ne_bytes(), v.to_ne_bytes()].concat())
            .collect();
        // SAFETY: free slots of the ring.
        unsafe { out.copy(w, bytes.as_ptr().cast_mut(), frames as u64, true) };
        ring.mark_ns.store(clock::now_ns(), Relaxed);
        ring.mark.store(w + 1, Release);
        ring.write.store(w + frames as u64, Release);
    }

    #[test]
    fn loopback_moves_frames_through_both_callbacks() {
        let (out, input) = (shared(), shared());
        guest_write(&out, 0, 100);
        // The device takes 64, then 64 more of which 28 are missing.
        let first = device_pull(&out, 64);
        let second = device_pull(&out, 64);
        let ring = out.ring();
        assert_eq!(ring.read.load(Relaxed), 100);
        assert_eq!(ring.xruns.load(Relaxed), 28);
        assert!(
            second[36 * FRAME..].iter().all(|b| *b == 0),
            "silence after the data"
        );
        assert_eq!(ring.latency_count.load(Relaxed), 1);
        assert_eq!(ring.mark.load(Relaxed), 0);
        assert_eq!(ring.stamp_frames.load(Relaxed), 64);
        assert!((ring.stamp_ns.load(Relaxed) - clock::now_ns()).abs() < 100_000_000);
        // An empty ring leaves the stamp where the data ended.
        device_pull(&out, 64);
        assert_eq!(ring.stamp_frames.load(Relaxed), 64);
        assert_eq!(ring.xruns.load(Relaxed), 28 + 64);

        // Loop both renders back into an input ring.
        input.push_captured(&first, clock::now_ns());
        input.push_captured(&second[..36 * FRAME], clock::now_ns());
        let ring = input.ring();
        assert_eq!(ring.write.load(Relaxed), 100);
        let mut got = vec![0u8; 100 * FRAME];
        // SAFETY: published frames.
        unsafe { input.copy(0, got.as_mut_ptr(), 100, false) };
        let samples: Vec<u16> = got
            .chunks_exact(FRAME)
            .map(|f| u16::from_ne_bytes([f[0], f[1]]))
            .collect();
        assert_eq!(samples, (0..100).collect::<Vec<u16>>());
        assert_eq!(f32::from_bits(ring.peak_bits.load(Relaxed)), 99.0 / 32768.0);
        // A full input ring drops what does not fit.
        input.push_captured(&vec![0u8; 200 * FRAME], clock::now_ns());
        assert_eq!(ring.write.load(Relaxed), CAPACITY);
        assert_eq!(ring.xruns.load(Relaxed), 300 - CAPACITY);
    }
}
