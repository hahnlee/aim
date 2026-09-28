//! The null sink: a stream with no device. A timer thread runs the device
//! callbacks' ring logic at the stream's rate, so output is consumed (and
//! its position advances) and input delivers silence exactly as a device
//! would, and audioserver's threads keep their pace. It serves streams
//! while CoreAudio does not answer (`docs/audio.md`, "Without CoreAudio").

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::thread::JoinHandle;
use std::time::Duration;

use crate::coreaudio::{AudioBuffer, AudioBufferList, AudioTimeStamp};
use crate::stream::{Shared, clock, render};

/// The period the timer moves frames at.
const PERIOD: Duration = Duration::from_millis(10);

pub struct NullSink {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl NullSink {
    pub fn start(shared: Arc<Shared>, rate: u32, input: bool) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("audio-null".into())
            .spawn(move || run(&shared, rate, input, &flag))
            .ok();
        Self { stop, thread }
    }
}

impl Drop for NullSink {
    fn drop(&mut self) {
        self.stop.store(true, Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
}

fn run(s: &Shared, rate: u32, input: bool, stop: &AtomicBool) {
    let period_frames = (rate as u64 * PERIOD.as_nanos() as u64 / 1_000_000_000).max(1);
    let mut buf = vec![0u8; 2 * period_frames as usize * s.frame_bytes()];
    let start = clock::now_ns();
    let mut done = 0u64;
    while !stop.load(Relaxed) {
        let now = clock::now_ns();
        let due = (now - start) as u64 * rate as u64 / 1_000_000_000;
        while done < due {
            let frames = (due - done).min(2 * period_frames);
            let bytes = frames as usize * s.frame_bytes();
            if input {
                buf[..bytes].fill(0);
                s.push_captured(&buf[..bytes], now);
                s.account(now);
            } else {
                let mut list = AudioBufferList {
                    count: 1,
                    buffers: [AudioBuffer {
                        channels: 0,
                        byte_size: bytes as u32,
                        data: buf.as_mut_ptr().cast(),
                    }],
                };
                let time = AudioTimeStamp {
                    sample_time: done as f64,
                    // SAFETY: reads the clock.
                    host_time: unsafe { mach_absolute_time() },
                    rate_scalar: 1.0,
                    word_clock_time: 0,
                    smpte_time: [0; 24],
                    flags: 0,
                    reserved: 0,
                };
                let refcon = (s as *const Shared).cast_mut().cast();
                // SAFETY: the output callback with our buffer and stamp.
                unsafe {
                    render(
                        refcon,
                        std::ptr::null_mut(),
                        &time,
                        0,
                        frames as u32,
                        &mut list,
                    )
                };
            }
            done += frames;
        }
        // Sleep to the next period boundary.
        let next = start
            + ((done / period_frames + 1) * period_frames * 1_000_000_000 / rate as u64) as i64;
        let wait = (next - clock::now_ns()).max(0) as u64;
        std::thread::sleep(Duration::from_nanos(wait));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_hostcall::audio::{DIRECTION_OUTPUT, FORMAT_PCM_16, Open, RING_DATA_OFFSET, Ring};
    use std::os::fd::AsRawFd;
    use std::sync::atomic::Ordering::Release;

    #[test]
    fn plays_output_at_the_stream_rate() {
        const CAPACITY: u32 = 48_000;
        let file = tempfile();
        let length = RING_DATA_OFFSET + CAPACITY as usize * 4;
        file.set_len(length as u64).unwrap();
        // SAFETY: a shared mapping of our own file, for the guest side.
        let guest = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                length,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                file.as_raw_fd(),
                0,
            )
        };
        assert_ne!(guest, libc::MAP_FAILED);
        // SAFETY: the header of the zeroed file.
        let ring = unsafe { &mut *(guest as *mut Ring) };
        ring.capacity_frames = CAPACITY;
        ring.frame_bytes = 4;
        let open = Open {
            direction: DIRECTION_OUTPUT,
            format: FORMAT_PCM_16,
            sample_rate: 48_000,
            channels: 2,
            fd: file.as_raw_fd(),
            length: length as u64,
            ..Default::default()
        };
        let shared: Arc<Shared> = Shared::new(&open, 0).unwrap().into();
        // One second queued; the sink takes a quarter of it in 250 ms.
        ring.write.store(CAPACITY as u64, Release);
        let sink = NullSink::start(shared, 48_000, false);
        std::thread::sleep(Duration::from_millis(250));
        drop(sink);
        let read = ring.read.load(Relaxed);
        assert!((11_000..14_000).contains(&read), "read {read} frames");
        assert_eq!(ring.xruns.load(Relaxed), 0);
        assert!(ring.stamp_seq.load(Relaxed) > 0);
        // SAFETY: our mapping.
        unsafe { libc::munmap(guest, length) };
    }

    fn tempfile() -> std::fs::File {
        let path = std::env::temp_dir().join(format!("null-sink-{}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        let _ = std::fs::remove_file(&path);
        file
    }
}
