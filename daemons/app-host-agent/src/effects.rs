//! The sound effects AudioService plays (`SoundEffectsHelper`): each
//! effect's file decoded once, as SoundPool loads it (NdkMediaExtractor and
//! NdkMediaCodec, which decode through the media services), and played
//! as SoundPool plays it: a sonification stream (AAudio, through
//! audioserver at the system stream's volume) at the effect's gain, at
//! most four at once, the oldest stopped for a fifth.

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `SoundEffectsHelper.NUM_SOUNDPOOL_CHANNELS`.
const CHANNELS: usize = 4;
/// How long a decode or a drain may take.
const PATIENCE: Duration = Duration::from_secs(5);

#[repr(C)]
struct Opaque {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Default)]
struct BufferInfo {
    offset: i32,
    size: i32,
    presentation_time_us: i64,
    flags: u32,
}

const AMEDIA_OK: i32 = 0;
const BUFFER_FLAG_END_OF_STREAM: u32 = 4;
const INFO_TRY_AGAIN_LATER: isize = -1;
const INFO_OUTPUT_FORMAT_CHANGED: isize = -2;
const INFO_OUTPUT_BUFFERS_CHANGED: isize = -3;
const AAUDIO_OK: i32 = 0;
const AAUDIO_FORMAT_PCM_I16: i32 = 1;
const AAUDIO_USAGE_ASSISTANCE_SONIFICATION: i32 = 13;
const AAUDIO_CONTENT_TYPE_SONIFICATION: i32 = 4;

#[link(name = "mediandk")]
unsafe extern "C" {
    fn AMediaExtractor_new() -> *mut Opaque;
    fn AMediaExtractor_delete(ex: *mut Opaque) -> i32;
    fn AMediaExtractor_setDataSourceFd(ex: *mut Opaque, fd: i32, offset: i64, length: i64) -> i32;
    fn AMediaExtractor_getTrackCount(ex: *mut Opaque) -> usize;
    fn AMediaExtractor_getTrackFormat(ex: *mut Opaque, idx: usize) -> *mut Opaque;
    fn AMediaExtractor_selectTrack(ex: *mut Opaque, idx: usize) -> i32;
    fn AMediaExtractor_readSampleData(ex: *mut Opaque, buffer: *mut u8, capacity: usize) -> isize;
    fn AMediaExtractor_getSampleTime(ex: *mut Opaque) -> i64;
    fn AMediaExtractor_advance(ex: *mut Opaque) -> bool;
    fn AMediaFormat_delete(format: *mut Opaque) -> i32;
    fn AMediaFormat_getString(
        format: *mut Opaque,
        name: *const c_char,
        out: *mut *const c_char,
    ) -> bool;
    fn AMediaFormat_getInt32(format: *mut Opaque, name: *const c_char, out: *mut i32) -> bool;
    fn AMediaCodec_createDecoderByType(mime: *const c_char) -> *mut Opaque;
    fn AMediaCodec_configure(
        codec: *mut Opaque,
        format: *const Opaque,
        surface: *mut c_void,
        crypto: *mut c_void,
        flags: u32,
    ) -> i32;
    fn AMediaCodec_start(codec: *mut Opaque) -> i32;
    fn AMediaCodec_stop(codec: *mut Opaque) -> i32;
    fn AMediaCodec_delete(codec: *mut Opaque) -> i32;
    fn AMediaCodec_dequeueInputBuffer(codec: *mut Opaque, timeout_us: i64) -> isize;
    fn AMediaCodec_getInputBuffer(codec: *mut Opaque, idx: usize, size: *mut usize) -> *mut u8;
    fn AMediaCodec_queueInputBuffer(
        codec: *mut Opaque,
        idx: usize,
        offset: i64,
        size: usize,
        time: u64,
        flags: u32,
    ) -> i32;
    fn AMediaCodec_dequeueOutputBuffer(
        codec: *mut Opaque,
        info: *mut BufferInfo,
        timeout_us: i64,
    ) -> isize;
    fn AMediaCodec_getOutputBuffer(codec: *mut Opaque, idx: usize, size: *mut usize) -> *mut u8;
    fn AMediaCodec_releaseOutputBuffer(codec: *mut Opaque, idx: usize, render: bool) -> i32;
    fn AMediaCodec_getOutputFormat(codec: *mut Opaque) -> *mut Opaque;
}

#[link(name = "aaudio")]
unsafe extern "C" {
    fn AAudio_createStreamBuilder(builder: *mut *mut Opaque) -> i32;
    fn AAudioStreamBuilder_setFormat(builder: *mut Opaque, format: i32);
    fn AAudioStreamBuilder_setChannelCount(builder: *mut Opaque, count: i32);
    fn AAudioStreamBuilder_setSampleRate(builder: *mut Opaque, rate: i32);
    fn AAudioStreamBuilder_setUsage(builder: *mut Opaque, usage: i32);
    fn AAudioStreamBuilder_setContentType(builder: *mut Opaque, content: i32);
    fn AAudioStreamBuilder_openStream(builder: *mut Opaque, stream: *mut *mut Opaque) -> i32;
    fn AAudioStreamBuilder_delete(builder: *mut Opaque) -> i32;
    fn AAudioStream_requestStart(stream: *mut Opaque) -> i32;
    fn AAudioStream_requestStop(stream: *mut Opaque) -> i32;
    fn AAudioStream_close(stream: *mut Opaque) -> i32;
    fn AAudioStream_write(
        stream: *mut Opaque,
        buffer: *const c_void,
        frames: i32,
        timeout_ns: i64,
    ) -> i32;
    fn AAudioStream_getFramesRead(stream: *mut Opaque) -> i64;
}

/// A decoded effect: interleaved 16-bit samples.
struct Sample {
    rate: i32,
    channels: i32,
    pcm: Vec<i16>,
}

/// A media format read and freed.
fn format_ints(format: *mut Opaque) -> (i32, i32) {
    let int = |name: &CStr| {
        let mut v = 0;
        // SAFETY: a format the caller got from the NDK; `v` outlives the call.
        unsafe { AMediaFormat_getInt32(format, name.as_ptr(), &mut v) };
        v
    };
    (int(c"sample-rate"), int(c"channel-count"))
}

/// `SoundPool.load`: the file's first track, decoded to 16-bit PCM.
fn decode(path: &str) -> Result<Sample, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("{path}: {e}"))?;
    let length = file.metadata().map_err(|e| format!("{path}: {e}"))?.len() as i64;
    // SAFETY: each NDK object is used only while alive and freed once.
    unsafe {
        let ex = AMediaExtractor_new();
        let result = (|| {
            if AMediaExtractor_setDataSourceFd(ex, file.as_raw_fd(), 0, length) != AMEDIA_OK
                || AMediaExtractor_getTrackCount(ex) == 0
            {
                return Err(format!("{path}: not a media file"));
            }
            let format = AMediaExtractor_getTrackFormat(ex, 0);
            let mut mime: *const c_char = std::ptr::null();
            AMediaFormat_getString(format, c"mime".as_ptr(), &mut mime);
            let (mut rate, mut channels) = format_ints(format);
            let codec = if mime.is_null() {
                std::ptr::null_mut()
            } else {
                AMediaCodec_createDecoderByType(mime)
            };
            if codec.is_null() {
                AMediaFormat_delete(format);
                return Err(format!("{path}: no decoder"));
            }
            let configured =
                AMediaCodec_configure(codec, format, std::ptr::null_mut(), std::ptr::null_mut(), 0);
            AMediaFormat_delete(format);
            if configured != AMEDIA_OK || AMediaCodec_start(codec) != AMEDIA_OK {
                AMediaCodec_delete(codec);
                return Err(format!("{path}: the decoder did not start"));
            }
            AMediaExtractor_selectTrack(ex, 0);
            let mut pcm = Vec::new();
            let (mut input_done, deadline) = (false, Instant::now() + PATIENCE);
            loop {
                if Instant::now() > deadline {
                    break Err(format!("{path}: decoding took too long"));
                }
                if !input_done {
                    let idx = AMediaCodec_dequeueInputBuffer(codec, 10_000);
                    if idx >= 0 {
                        let mut capacity = 0;
                        let buffer = AMediaCodec_getInputBuffer(codec, idx as usize, &mut capacity);
                        let size = AMediaExtractor_readSampleData(ex, buffer, capacity);
                        if size < 0 {
                            AMediaCodec_queueInputBuffer(
                                codec,
                                idx as usize,
                                0,
                                0,
                                0,
                                BUFFER_FLAG_END_OF_STREAM,
                            );
                            input_done = true;
                        } else {
                            let time = AMediaExtractor_getSampleTime(ex).max(0) as u64;
                            AMediaCodec_queueInputBuffer(
                                codec,
                                idx as usize,
                                0,
                                size as usize,
                                time,
                                0,
                            );
                            AMediaExtractor_advance(ex);
                        }
                    }
                }
                let mut info = BufferInfo::default();
                let idx = AMediaCodec_dequeueOutputBuffer(codec, &mut info, 10_000);
                if idx >= 0 {
                    let mut size = 0;
                    let buffer = AMediaCodec_getOutputBuffer(codec, idx as usize, &mut size);
                    if !buffer.is_null() && info.size > 0 {
                        let bytes = std::slice::from_raw_parts(
                            buffer.add(info.offset as usize),
                            info.size as usize,
                        );
                        pcm.extend(
                            bytes
                                .chunks_exact(2)
                                .map(|b| i16::from_le_bytes([b[0], b[1]])),
                        );
                    }
                    AMediaCodec_releaseOutputBuffer(codec, idx as usize, false);
                    if info.flags & BUFFER_FLAG_END_OF_STREAM != 0 {
                        break Ok(());
                    }
                } else if idx == INFO_OUTPUT_FORMAT_CHANGED {
                    let format = AMediaCodec_getOutputFormat(codec);
                    (rate, channels) = format_ints(format);
                    AMediaFormat_delete(format);
                } else if idx != INFO_TRY_AGAIN_LATER && idx != INFO_OUTPUT_BUFFERS_CHANGED {
                    break Err(format!("{path}: decoding failed ({idx})"));
                }
            }
            .map(|()| Sample {
                rate,
                channels,
                pcm,
            })
            .inspect(|_| {
                AMediaCodec_stop(codec);
                AMediaCodec_delete(codec);
            })
        })();
        AMediaExtractor_delete(ex);
        result
    }
}

/// `SoundPool.play` of `sample` at `gain` until it ends or is stopped.
fn play(sample: &Sample, gain: f32, stop: &AtomicBool) -> Result<(), String> {
    let pcm: Vec<i16> = sample
        .pcm
        .iter()
        .map(|&s| (s as f32 * gain) as i16)
        .collect();
    let frames = pcm.len() as i32 / sample.channels.max(1);
    // SAFETY: the builder and stream are used only while alive and freed once.
    unsafe {
        let mut builder = std::ptr::null_mut();
        if AAudio_createStreamBuilder(&mut builder) != AAUDIO_OK {
            return Err("no AAudio stream builder".into());
        }
        AAudioStreamBuilder_setFormat(builder, AAUDIO_FORMAT_PCM_I16);
        AAudioStreamBuilder_setChannelCount(builder, sample.channels);
        AAudioStreamBuilder_setSampleRate(builder, sample.rate);
        AAudioStreamBuilder_setUsage(builder, AAUDIO_USAGE_ASSISTANCE_SONIFICATION);
        AAudioStreamBuilder_setContentType(builder, AAUDIO_CONTENT_TYPE_SONIFICATION);
        let mut stream = std::ptr::null_mut();
        let opened = AAudioStreamBuilder_openStream(builder, &mut stream);
        AAudioStreamBuilder_delete(builder);
        if opened != AAUDIO_OK {
            return Err(format!("the effect's stream did not open ({opened})"));
        }
        let result = (|| {
            if AAudioStream_requestStart(stream) != AAUDIO_OK {
                return Err("the effect's stream did not start".to_string());
            }
            let mut written = 0;
            while written < frames && !stop.load(Ordering::Relaxed) {
                let at = pcm.as_ptr().add((written * sample.channels) as usize);
                let n = AAudioStream_write(stream, at.cast(), frames - written, 100_000_000);
                if n < 0 {
                    return Err(format!("the effect's stream failed ({n})"));
                }
                written += n;
            }
            let deadline = Instant::now() + PATIENCE;
            while !stop.load(Ordering::Relaxed)
                && AAudioStream_getFramesRead(stream) < written as i64
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        })();
        AAudioStream_requestStop(stream);
        AAudioStream_close(stream);
        result
    }
}

/// The effects decoded, and the streams playing.
#[derive(Default)]
pub struct Effects {
    samples: Mutex<HashMap<String, Arc<Sample>>>,
    playing: Mutex<Vec<Arc<AtomicBool>>>,
}

impl Effects {
    /// Plays the effect of `path` at `gain` on a stream of its own.
    pub fn play(self: &Arc<Self>, path: &str, gain: f32) -> Result<(), String> {
        let sample = {
            let mut samples = self.samples.lock().unwrap();
            match samples.get(path) {
                Some(s) => s.clone(),
                None => {
                    let s = Arc::new(decode(path)?);
                    samples.insert(path.to_string(), s.clone());
                    s
                }
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut playing = self.playing.lock().unwrap();
            if playing.len() >= CHANNELS {
                playing.remove(0).store(true, Ordering::Relaxed);
            }
            playing.push(stop.clone());
        }
        let me = self.clone();
        std::thread::spawn(move || {
            if let Err(e) = play(&sample, gain, &stop) {
                log::error!("sound effect: {e}");
            }
            me.playing
                .lock()
                .unwrap()
                .retain(|s| !Arc::ptr_eq(s, &stop));
        });
        Ok(())
    }
}
