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

mod coreaudio;
mod stream;

use std::collections::HashMap;
use std::sync::Mutex;

use darwin_hostcall::audio::{
    Devices, FN_CLOSE, FN_DEVICES, FN_OPEN, FN_START, FN_STOP, Open, Stream as StreamArgs, VERSION,
};
use darwin_hostcall::{HostModule, args_mut, errno, module};

pub use stream::clock;

pub static MODULE: HostModule = HostModule {
    id: module::AUDIO,
    name: "audio",
    version: VERSION,
    call,
};

struct Streams {
    next: u64,
    open: HashMap<u64, stream::Stream>,
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

unsafe fn call(func: u32, args: u64, len: u64) -> i64 {
    match func {
        FN_DEVICES => {
            // SAFETY: the registry passes the guest's argument block.
            match unsafe { args_mut::<Devices>(args, len) } {
                Ok(out) => {
                    *out = devices();
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
            match stream::Stream::open(open) {
                Ok(s) => with_streams(|streams| {
                    let id = streams.next;
                    streams.next += 1;
                    streams.open.insert(id, s);
                    open.stream = id;
                    0
                }),
                Err(e) => neg(e),
            }
        }
        FN_START | FN_STOP | FN_CLOSE => {
            // SAFETY: as above.
            let id = match unsafe { args_mut::<StreamArgs>(args, len) } {
                Ok(s) => s.stream,
                Err(e) => return e,
            };
            with_streams(|streams| {
                let ok = match func {
                    FN_START => streams.open.get(&id).map(|s| s.start()),
                    FN_STOP => streams.open.get(&id).map(|s| s.stop()),
                    _ => streams.open.remove(&id).map(|_| true),
                };
                match ok {
                    Some(true) => 0,
                    _ => neg(errno::EINVAL),
                }
            })
        }
        _ => neg(errno::ENOSYS),
    }
}

/// The default devices.
pub fn devices() -> Devices {
    Devices {
        output: coreaudio::describe(false),
        input: coreaudio::describe(true),
    }
}

#[cfg(test)]
mod tests;
