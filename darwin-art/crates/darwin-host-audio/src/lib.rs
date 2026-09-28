//! Host-call module [`darwin_hostcall::module::AUDIO`]: PCM streams on the
//! Mac's default output and input devices, for the guest
//! `android.hardware.audio.core` HAL (`docs/audio.md`).
//!
//! The guest owns the Android side (ports, patches, the stream state
//! machine and its FMQs). The host side is a transport: a stream is an AUHAL
//! unit on the default device and a lock-free ring in a guest memfd that
//! both sides map ([`darwin_hostcall::audio::Ring`]). Output uses the
//! DefaultOutput unit, which follows the user's output device and converts
//! rate and format; input uses the HAL output unit on the default input
//! device, at that device's rate. The first capture asks macOS for
//! microphone access.

mod client;
mod coreaudio;
pub mod io;
mod null;
mod stream;
mod wire;

use std::collections::HashMap;
use std::sync::atomic::{AtomicI32, Ordering::Relaxed};
use std::sync::{Arc, Mutex};

use darwin_hostcall::audio::{
    DIRECTION_INPUT, Device, Devices, FN_CLOSE, FN_DEVICES, FN_OPEN, FN_START, FN_STOP, Open,
    Stream as StreamArgs, VERSION,
};
use darwin_hostcall::{HostModule, args_mut, errno, module};

pub use stream::clock;

static LOG_FD: AtomicI32 = AtomicI32::new(2);

/// Where the module's messages go (the syscall layer's diagnostics
/// descriptor), and the CoreAudio process's stderr.
pub fn set_log_fd(fd: i32) {
    LOG_FD.store(fd, Relaxed);
}

fn log_fd() -> i32 {
    LOG_FD.load(Relaxed)
}

/// One line on the log descriptor, in a single write.
fn log(args: std::fmt::Arguments) {
    let line = format!("audio: {args}\n");
    // SAFETY: writing our buffer to a descriptor we were given.
    unsafe { libc::write(log_fd(), line.as_ptr().cast(), line.len()) };
}

pub static MODULE: HostModule = HostModule {
    id: module::AUDIO,
    name: "audio",
    version: VERSION,
    call,
};

/// Where a stream's frames go.
enum Backend {
    /// A unit in the CoreAudio process, by its handle there.
    Device(u64),
    /// The null sink, while running.
    Null(Option<null::NullSink>),
}

struct Entry {
    /// This process's own mapping of the ring, for the null sink.
    shared: Arc<stream::Shared>,
    rate: u32,
    input: bool,
    running: bool,
    backend: Backend,
}

impl Entry {
    fn set_null_running(&mut self, running: bool) {
        self.running = running;
        if let Backend::Null(sink) = &mut self.backend {
            *sink =
                running.then(|| null::NullSink::start(self.shared.clone(), self.rate, self.input));
        }
    }
}

struct Streams {
    next: u64,
    open: HashMap<u64, Entry>,
}

impl Streams {
    /// The CoreAudio process is gone: move every stream to the null sink,
    /// telling its callbacks (should they still run) to leave the ring.
    fn detach_all(&mut self) {
        for e in self.open.values_mut() {
            if let Backend::Device(_) = e.backend {
                e.shared
                    .ring()
                    .detached
                    .store(1, std::sync::atomic::Ordering::Release);
                e.backend = Backend::Null(None);
                let running = e.running;
                e.set_null_running(running);
            }
        }
    }
}

static STREAMS: Mutex<Option<Streams>> = Mutex::new(None);

fn with_streams<T>(f: impl FnOnce(&mut Streams) -> T) -> T {
    let mut guard = STREAMS.lock().unwrap_or_else(|e| e.into_inner());
    f(guard.get_or_insert_with(|| Streams {
        next: 1,
        open: HashMap::new(),
    }))
}

fn neg(e: i32) -> i64 {
    -(e as i64)
}

fn request(op: u32, stream: u64, open: Option<&Open>) -> Result<wire::Reply, client::Error> {
    let req = wire::Request {
        op,
        stream,
        open: open.copied().unwrap_or_default(),
        ..Default::default()
    };
    client::request(&req, open.map(|o| o.fd))
}

unsafe fn call(func: u32, args: u64, len: u64) -> i64 {
    match func {
        FN_DEVICES => {
            // SAFETY: the registry passes the guest's argument block.
            match unsafe { args_mut::<Devices>(args, len) } {
                Ok(out) => {
                    *out = match request(FN_DEVICES, 0, None) {
                        Ok(reply) => reply.devices,
                        Err(_) => null_devices(),
                    };
                    0
                }
                Err(e) => e,
            }
        }
        FN_OPEN => {
            // SAFETY: as above.
            let open = match unsafe { args_mut::<Open>(args, len) } {
                Ok(open) => open,
                Err(e) => return e,
            };
            let shared: Arc<stream::Shared> = match stream::Shared::new(open, 0) {
                Ok(s) => s.into(),
                Err(e) => return neg(e),
            };
            let backend = match request(FN_OPEN, 0, Some(open)) {
                Ok(reply) => Backend::Device(reply.stream),
                Err(client::Error::Errno(e)) => return neg(e),
                Err(client::Error::Gone) => Backend::Null(None),
            };
            with_streams(|streams| {
                if let Backend::Null(_) = backend {
                    streams.detach_all();
                }
                let id = streams.next;
                streams.next += 1;
                streams.open.insert(
                    id,
                    Entry {
                        shared,
                        rate: open.sample_rate,
                        input: open.direction == DIRECTION_INPUT,
                        running: false,
                        backend,
                    },
                );
                open.stream = id;
                0
            })
        }
        FN_START | FN_STOP | FN_CLOSE => {
            // SAFETY: as above.
            let id = match unsafe { args_mut::<StreamArgs>(args, len) } {
                Ok(s) => s.stream,
                Err(e) => return e,
            };
            with_streams(|streams| {
                let Some(entry) = streams.open.get_mut(&id) else {
                    return neg(errno::EINVAL);
                };
                let running = func == FN_START;
                if let Backend::Device(handle) = entry.backend {
                    match request(func, handle, None) {
                        Ok(_) => entry.running = running,
                        Err(client::Error::Errno(e)) => return neg(e),
                        Err(client::Error::Gone) => streams.detach_all(),
                    }
                }
                let entry = streams.open.get_mut(&id).unwrap();
                if let Backend::Null(_) = entry.backend {
                    entry.set_null_running(running);
                }
                if func == FN_CLOSE {
                    // The null sink stops before the mapping goes.
                    streams.open.remove(&id);
                }
                0
            })
        }
        _ => neg(errno::ENOSYS),
    }
}

/// The default devices, as macOS reports them. Called in the CoreAudio
/// process.
pub fn devices() -> Devices {
    Devices {
        output: coreaudio::describe(false),
        input: coreaudio::describe(true),
    }
}

/// What the module reports without CoreAudio: a stereo 48 kHz output (the
/// null sink) and no input.
fn null_devices() -> Devices {
    let mut name = [0u8; 64];
    name[..11].copy_from_slice(b"Null output");
    Devices {
        output: Device {
            present: 1,
            sample_rate: 48_000,
            channels: 2,
            buffer_frames: 480,
            latency_frames: 0,
            reserved: 0,
            name,
        },
        input: Device::default(),
    }
}

#[cfg(test)]
mod tests;
