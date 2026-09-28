#![forbid(unsafe_op_in_unsafe_fn)]

use std::ffi::{CStr, c_char, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const OK: i32 = 0;
const ERROR_NULL: i32 = -899;
const ERROR_INVALID_STATE: i32 = -895;
const DIRECTION_OUTPUT: i32 = 0;
const FORMAT_PCM_I16: i32 = 1;
const FORMAT_PCM_FLOAT: i32 = 2;
const CALLBACK_CONTINUE: i32 = 0;
const STATE_OPEN: i32 = 2;
const STATE_STARTED: i32 = 4;
const STATE_STOPPED: i32 = 10;

type DataCallback = unsafe extern "C" fn(*mut Stream, *mut c_void, *mut c_void, i32) -> i32;
type ErrorCallback = unsafe extern "C" fn(*mut Stream, *mut c_void, i32);

type CreateOutput = unsafe extern "C" fn(i32, i32, i32, i32) -> *mut c_void;
type OutputVoid = unsafe extern "C" fn(*mut c_void);
type OutputWrite = unsafe extern "C" fn(*mut c_void, *const c_void, usize, bool) -> usize;

#[derive(Clone, Copy)]
struct OutputBackend {
    create: CreateOutput,
    destroy: OutputVoid,
    start: OutputVoid,
    stop: OutputVoid,
    write: OutputWrite,
}

static OUTPUT_BACKEND: OnceLock<OutputBackend> = OnceLock::new();

#[unsafe(no_mangle)]
pub extern "C" fn aim_android_aaudio_install_output_backend(
    create: CreateOutput,
    destroy: OutputVoid,
    start: OutputVoid,
    stop: OutputVoid,
    write: OutputWrite,
) -> bool {
    OUTPUT_BACKEND
        .set(OutputBackend {
            create,
            destroy,
            start,
            stop,
            write,
        })
        .is_ok()
}

#[derive(Clone, Copy)]
struct Config {
    sample_rate: i32,
    channel_count: i32,
    channel_mask: i32,
    format: i32,
    frames_per_callback: i32,
    device_id: i32,
    direction: i32,
    data_callback: Option<DataCallback>,
    data_user: usize,
    error_callback: Option<ErrorCallback>,
    error_user: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channel_count: 2,
            channel_mask: 0,
            format: FORMAT_PCM_FLOAT,
            frames_per_callback: 192,
            device_id: 0,
            direction: DIRECTION_OUTPUT,
            data_callback: None,
            data_user: 0,
            error_callback: None,
            error_user: 0,
        }
    }
}

fn effective_config(mut config: Config) -> Config {
    if config.channel_mask != 0 {
        let channels = (config.channel_mask as u32).count_ones() as i32;
        if channels > 0 {
            config.channel_count = channels;
        }
    }
    config
}

#[repr(C)]
pub struct Builder {
    config: Config,
}

#[repr(C)]
pub struct Stream {
    config: Config,
    state: AtomicI32,
    state_wait: (Mutex<()>, Condvar),
    running: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
    frames_read: AtomicI64,
    frames_written: AtomicI64,
    buffer_frames: AtomicI32,
    xrun_count: AtomicI32,
    output_track: usize,
}

fn bytes_per_sample(format: i32) -> usize {
    if format == FORMAT_PCM_I16 { 2 } else { 4 }
}

fn stream_ref(stream: *mut Stream) -> Result<&'static Stream, i32> {
    unsafe { stream.as_ref() }.ok_or(ERROR_NULL)
}

fn set_state(stream: &Stream, state: i32) {
    stream.state.store(state, Ordering::Release);
    stream.state_wait.1.notify_all();
}

fn callback_loop(stream_address: usize) {
    let stream_ptr = stream_address as *mut Stream;
    let Some(stream) = (unsafe { stream_ptr.as_ref() }) else {
        return;
    };
    let config = stream.config;
    let frames = config.frames_per_callback.max(1);
    let byte_count =
        frames as usize * config.channel_count.max(1) as usize * bytes_per_sample(config.format);
    let mut buffer = vec![0u8; byte_count];
    let period = Duration::from_secs_f64(frames as f64 / config.sample_rate.max(1) as f64);
    while stream.running.load(Ordering::Acquire) {
        let started = Instant::now();
        let result = config
            .data_callback
            .map_or(CALLBACK_CONTINUE, |callback| unsafe {
                callback(
                    stream_ptr,
                    config.data_user as *mut c_void,
                    buffer.as_mut_ptr().cast(),
                    frames,
                )
            });
        if result != CALLBACK_CONTINUE {
            break;
        }
        if config.direction == DIRECTION_OUTPUT {
            if stream.output_track != 0
                && let Some(backend) = OUTPUT_BACKEND.get()
            {
                let written = unsafe {
                    (backend.write)(
                        stream.output_track as *mut c_void,
                        buffer.as_ptr().cast(),
                        buffer.len(),
                        true,
                    )
                };
                if written != buffer.len() {
                    stream.xrun_count.fetch_add(1, Ordering::Relaxed);
                }
            }
            stream
                .frames_written
                .fetch_add(frames as i64, Ordering::Relaxed);
        } else {
            stream
                .frames_read
                .fetch_add(frames as i64, Ordering::Relaxed);
        }
        if let Some(remaining) = period.checked_sub(started.elapsed()) {
            thread::sleep(remaining);
        }
    }
    stream.running.store(false, Ordering::Release);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudio_createStreamBuilder(out: *mut *mut Builder) -> i32 {
    let Some(out) = (unsafe { out.as_mut() }) else {
        return ERROR_NULL;
    };
    *out = Box::into_raw(Box::new(Builder {
        config: Config::default(),
    }));
    OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudioStreamBuilder_delete(builder: *mut Builder) -> i32 {
    if builder.is_null() {
        return ERROR_NULL;
    }
    drop(unsafe { Box::from_raw(builder) });
    OK
}

macro_rules! builder_setter {
    ($name:ident, $field:ident) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(builder: *mut Builder, value: i32) -> i32 {
            let Some(builder) = (unsafe { builder.as_mut() }) else {
                return ERROR_NULL;
            };
            builder.config.$field = value;
            OK
        }
    };
}

builder_setter!(AAudioStreamBuilder_setChannelCount, channel_count);
builder_setter!(AAudioStreamBuilder_setChannelMask, channel_mask);
builder_setter!(AAudioStreamBuilder_setDeviceId, device_id);
builder_setter!(AAudioStreamBuilder_setDirection, direction);
builder_setter!(AAudioStreamBuilder_setFormat, format);
builder_setter!(
    AAudioStreamBuilder_setFramesPerDataCallback,
    frames_per_callback
);
builder_setter!(AAudioStreamBuilder_setSampleRate, sample_rate);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudioStreamBuilder_setInputPreset(_: *mut Builder, _: i32) -> i32 {
    OK
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudioStreamBuilder_setPerformanceMode(_: *mut Builder, _: i32) -> i32 {
    OK
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudioStreamBuilder_setUsage(_: *mut Builder, _: i32) -> i32 {
    OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudioStreamBuilder_setDataCallback(
    builder: *mut Builder,
    callback: Option<DataCallback>,
    user: *mut c_void,
) -> i32 {
    let Some(builder) = (unsafe { builder.as_mut() }) else {
        return ERROR_NULL;
    };
    builder.config.data_callback = callback;
    builder.config.data_user = user as usize;
    OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudioStreamBuilder_setErrorCallback(
    builder: *mut Builder,
    callback: Option<ErrorCallback>,
    user: *mut c_void,
) -> i32 {
    let Some(builder) = (unsafe { builder.as_mut() }) else {
        return ERROR_NULL;
    };
    builder.config.error_callback = callback;
    builder.config.error_user = user as usize;
    OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudioStreamBuilder_openStream(
    builder: *mut Builder,
    out: *mut *mut Stream,
) -> i32 {
    let Some(builder) = (unsafe { builder.as_ref() }) else {
        return ERROR_NULL;
    };
    let Some(out) = (unsafe { out.as_mut() }) else {
        return ERROR_NULL;
    };
    let config = effective_config(builder.config);
    // Android treats an explicit channel mask as the authoritative stream
    // layout. Chromium requests mono using setChannelMask(), while the
    // builder's channel-count field still contains its stereo default. The
    // opened stream must report the effective layout, not that stale default.
    if config.sample_rate <= 0 || config.channel_count <= 0 || config.frames_per_callback <= 0 {
        return ERROR_INVALID_STATE;
    }
    let output_track = if config.direction == DIRECTION_OUTPUT
        && let Some(backend) = OUTPUT_BACKEND.get()
    {
        let format = if config.format == FORMAT_PCM_I16 {
            2
        } else {
            4
        };
        let mask = if config.channel_mask != 0 {
            config.channel_mask
        } else if config.channel_count >= 31 {
            -1
        } else {
            (1_i32 << config.channel_count) - 1
        };
        (unsafe {
            (backend.create)(
                config.sample_rate,
                mask,
                format,
                config.frames_per_callback
                    * config.channel_count
                    * bytes_per_sample(config.format) as i32
                    * 4,
            )
        }) as usize
    } else {
        0
    };
    if config.direction == DIRECTION_OUTPUT && OUTPUT_BACKEND.get().is_some() && output_track == 0 {
        return ERROR_INVALID_STATE;
    }
    *out = Box::into_raw(Box::new(Stream {
        config,
        state: AtomicI32::new(STATE_OPEN),
        state_wait: (Mutex::new(()), Condvar::new()),
        running: AtomicBool::new(false),
        worker: Mutex::new(None),
        frames_read: AtomicI64::new(0),
        frames_written: AtomicI64::new(0),
        buffer_frames: AtomicI32::new(config.frames_per_callback * 4),
        xrun_count: AtomicI32::new(0),
        output_track,
    }));
    OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudioStream_requestStart(stream: *mut Stream) -> i32 {
    let stream_ref = match stream_ref(stream) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if stream_ref.running.swap(true, Ordering::AcqRel) {
        return OK;
    }
    if stream_ref.output_track != 0
        && let Some(backend) = OUTPUT_BACKEND.get()
    {
        unsafe { (backend.start)(stream_ref.output_track as *mut c_void) };
    }
    set_state(stream_ref, STATE_STARTED);
    let stream_address = stream as usize;
    let worker = thread::Builder::new()
        .name("AAudioCallback".into())
        .spawn(move || callback_loop(stream_address));
    match worker {
        Ok(worker) => {
            *stream_ref
                .worker
                .lock()
                .expect("AAudio worker mutex poisoned") = Some(worker);
            OK
        }
        Err(_) => {
            stream_ref.running.store(false, Ordering::Release);
            ERROR_INVALID_STATE
        }
    }
}

fn stop(stream: &Stream) {
    stream.running.store(false, Ordering::Release);
    if let Some(worker) = stream
        .worker
        .lock()
        .expect("AAudio worker mutex poisoned")
        .take()
    {
        let _ = worker.join();
    }
    if stream.output_track != 0
        && let Some(backend) = OUTPUT_BACKEND.get()
    {
        unsafe { (backend.stop)(stream.output_track as *mut c_void) };
    }
    set_state(stream, STATE_STOPPED);
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudioStream_requestStop(stream: *mut Stream) -> i32 {
    let stream = match stream_ref(stream) {
        Ok(v) => v,
        Err(e) => return e,
    };
    stop(stream);
    OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudioStream_close(stream: *mut Stream) -> i32 {
    if stream.is_null() {
        return ERROR_NULL;
    }
    let stream = unsafe { Box::from_raw(stream) };
    stop(&stream);
    if stream.output_track != 0
        && let Some(backend) = OUTPUT_BACKEND.get()
    {
        unsafe { (backend.destroy)(stream.output_track as *mut c_void) };
    }
    OK
}

macro_rules! stream_getter_i32 {
    ($name:ident, $value:expr) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(stream: *mut Stream) -> i32 {
            let stream = match stream_ref(stream) {
                Ok(v) => v,
                Err(e) => return e,
            };
            $value(stream)
        }
    };
}

stream_getter_i32!(AAudioStream_getBufferSizeInFrames, |s: &Stream| s
    .buffer_frames
    .load(Ordering::Acquire));
stream_getter_i32!(AAudioStream_getChannelCount, |s: &Stream| s
    .config
    .channel_count);
stream_getter_i32!(AAudioStream_getDeviceId, |s: &Stream| s.config.device_id);
stream_getter_i32!(AAudioStream_getFormat, |s: &Stream| s.config.format);
stream_getter_i32!(AAudioStream_getFramesPerBurst, |s: &Stream| s
    .config
    .frames_per_callback);
stream_getter_i32!(AAudioStream_getFramesPerDataCallback, |s: &Stream| s
    .config
    .frames_per_callback);
stream_getter_i32!(AAudioStream_getXRunCount, |s: &Stream| s
    .xrun_count
    .load(Ordering::Relaxed));

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudioStream_getFramesRead(stream: *mut Stream) -> i64 {
    stream_ref(stream).map_or(0, |s| s.frames_read.load(Ordering::Relaxed))
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudioStream_getFramesWritten(stream: *mut Stream) -> i64 {
    stream_ref(stream).map_or(0, |s| s.frames_written.load(Ordering::Relaxed))
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudioStream_setBufferSizeInFrames(
    stream: *mut Stream,
    frames: i32,
) -> i32 {
    let stream = match stream_ref(stream) {
        Ok(v) => v,
        Err(e) => return e,
    };
    if frames <= 0 {
        return ERROR_INVALID_STATE;
    }
    stream.buffer_frames.store(frames, Ordering::Release);
    frames
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudioStream_waitForStateChange(
    stream: *mut Stream,
    input_state: i32,
    next_state: *mut i32,
    timeout_ns: i64,
) -> i32 {
    let stream = match stream_ref(stream) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let Some(next_state) = (unsafe { next_state.as_mut() }) else {
        return ERROR_NULL;
    };
    let current = stream.state.load(Ordering::Acquire);
    if current == input_state && timeout_ns > 0 {
        let guard = stream
            .state_wait
            .0
            .lock()
            .expect("AAudio state mutex poisoned");
        let _ = stream
            .state_wait
            .1
            .wait_timeout(guard, Duration::from_nanos(timeout_ns as u64));
    }
    *next_state = stream.state.load(Ordering::Acquire);
    OK
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn AAudioStream_getTimestamp(
    stream: *mut Stream,
    _: i32,
    frame_position: *mut i64,
    time_ns: *mut i64,
) -> i32 {
    let stream = match stream_ref(stream) {
        Ok(v) => v,
        Err(e) => return e,
    };
    let Some(frame_position) = (unsafe { frame_position.as_mut() }) else {
        return ERROR_NULL;
    };
    let Some(time_ns) = (unsafe { time_ns.as_mut() }) else {
        return ERROR_NULL;
    };
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    *frame_position = if stream.config.direction == DIRECTION_OUTPUT {
        stream.frames_written.load(Ordering::Relaxed)
    } else {
        stream.frames_read.load(Ordering::Relaxed)
    };
    *time_ns = ORIGIN
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .min(i64::MAX as u128) as i64;
    OK
}

const SYMBOLS: &[(&[u8], *const c_void)] = &[
    (
        b"AAudioStreamBuilder_delete\0",
        AAudioStreamBuilder_delete as *const c_void,
    ),
    (
        b"AAudioStreamBuilder_openStream\0",
        AAudioStreamBuilder_openStream as *const c_void,
    ),
    (
        b"AAudioStreamBuilder_setChannelCount\0",
        AAudioStreamBuilder_setChannelCount as *const c_void,
    ),
    (
        b"AAudioStreamBuilder_setChannelMask\0",
        AAudioStreamBuilder_setChannelMask as *const c_void,
    ),
    (
        b"AAudioStreamBuilder_setDataCallback\0",
        AAudioStreamBuilder_setDataCallback as *const c_void,
    ),
    (
        b"AAudioStreamBuilder_setDeviceId\0",
        AAudioStreamBuilder_setDeviceId as *const c_void,
    ),
    (
        b"AAudioStreamBuilder_setDirection\0",
        AAudioStreamBuilder_setDirection as *const c_void,
    ),
    (
        b"AAudioStreamBuilder_setErrorCallback\0",
        AAudioStreamBuilder_setErrorCallback as *const c_void,
    ),
    (
        b"AAudioStreamBuilder_setFormat\0",
        AAudioStreamBuilder_setFormat as *const c_void,
    ),
    (
        b"AAudioStreamBuilder_setFramesPerDataCallback\0",
        AAudioStreamBuilder_setFramesPerDataCallback as *const c_void,
    ),
    (
        b"AAudioStreamBuilder_setInputPreset\0",
        AAudioStreamBuilder_setInputPreset as *const c_void,
    ),
    (
        b"AAudioStreamBuilder_setPerformanceMode\0",
        AAudioStreamBuilder_setPerformanceMode as *const c_void,
    ),
    (
        b"AAudioStreamBuilder_setSampleRate\0",
        AAudioStreamBuilder_setSampleRate as *const c_void,
    ),
    (
        b"AAudioStreamBuilder_setUsage\0",
        AAudioStreamBuilder_setUsage as *const c_void,
    ),
    (b"AAudioStream_close\0", AAudioStream_close as *const c_void),
    (
        b"AAudioStream_getBufferSizeInFrames\0",
        AAudioStream_getBufferSizeInFrames as *const c_void,
    ),
    (
        b"AAudioStream_getChannelCount\0",
        AAudioStream_getChannelCount as *const c_void,
    ),
    (
        b"AAudioStream_getDeviceId\0",
        AAudioStream_getDeviceId as *const c_void,
    ),
    (
        b"AAudioStream_getFormat\0",
        AAudioStream_getFormat as *const c_void,
    ),
    (
        b"AAudioStream_getFramesPerBurst\0",
        AAudioStream_getFramesPerBurst as *const c_void,
    ),
    (
        b"AAudioStream_getFramesPerDataCallback\0",
        AAudioStream_getFramesPerDataCallback as *const c_void,
    ),
    (
        b"AAudioStream_getFramesRead\0",
        AAudioStream_getFramesRead as *const c_void,
    ),
    (
        b"AAudioStream_getFramesWritten\0",
        AAudioStream_getFramesWritten as *const c_void,
    ),
    (
        b"AAudioStream_getTimestamp\0",
        AAudioStream_getTimestamp as *const c_void,
    ),
    (
        b"AAudioStream_getXRunCount\0",
        AAudioStream_getXRunCount as *const c_void,
    ),
    (
        b"AAudioStream_requestStart\0",
        AAudioStream_requestStart as *const c_void,
    ),
    (
        b"AAudioStream_requestStop\0",
        AAudioStream_requestStop as *const c_void,
    ),
    (
        b"AAudioStream_setBufferSizeInFrames\0",
        AAudioStream_setBufferSizeInFrames as *const c_void,
    ),
    (
        b"AAudioStream_waitForStateChange\0",
        AAudioStream_waitForStateChange as *const c_void,
    ),
    (
        b"AAudio_createStreamBuilder\0",
        AAudio_createStreamBuilder as *const c_void,
    ),
];

// Exact resolver: unversioned libaaudio imports are the NDK ABI.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn aim_android_aaudio_resolve(
    soname: *const c_char,
    symbol: *const c_char,
    version: *const c_char,
) -> *mut c_void {
    if soname.is_null() || symbol.is_null() || !version.is_null() {
        return ptr::null_mut();
    }
    if unsafe { CStr::from_ptr(soname) }.to_bytes() != b"libaaudio.so" {
        return ptr::null_mut();
    }
    let requested = unsafe { CStr::from_ptr(symbol) }.to_bytes_with_nul();
    SYMBOLS
        .iter()
        .find_map(|(name, address)| (*name == requested).then_some(*address as *mut c_void))
        .unwrap_or(ptr::null_mut())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_channel_mask_defines_opened_stream_count() {
        let mono = effective_config(Config {
            channel_count: 2,
            channel_mask: 0b1,
            ..Config::default()
        });
        assert_eq!(mono.channel_count, 1);

        let stereo = effective_config(Config {
            channel_count: 1,
            channel_mask: 0b11,
            ..Config::default()
        });
        assert_eq!(stereo.channel_count, 2);
    }
}
