//! `IStreamOut`, `IStreamIn` and `IStreamCommon`, with the stream worker:
//! the state machine of `StreamDescriptor` driven by commands on the
//! command FMQ, as AOSP's reference `StreamOutWorkerLogic` and
//! `StreamInWorkerLogic` run it, over the host ring ([`HostStream`]).
//!
//! The client (libaudiohal in audioserver) writes PCM into the data FMQ and
//! sends `burst`; the worker moves it into the host ring, which CoreAudio
//! plays. A write blocks while the ring is full, which paces the mixer.

use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use android_hardware_audio_common::aidl::android::hardware::audio::common::{
    AudioOffloadMetadata::AudioOffloadMetadata, SinkMetadata::SinkMetadata,
    SourceMetadata::SourceMetadata,
};
use android_hardware_audio_core::aidl::android::hardware::audio::core::{
    IStreamCommon::{BnStreamCommon, IStreamCommon},
    IStreamIn::{IStreamIn, MicrophoneDirection::MicrophoneDirection},
    IStreamOut::IStreamOut,
    StreamDescriptor::{AudioBuffer::AudioBuffer, StreamDescriptor},
    VendorParameter::VendorParameter,
};
use android_hardware_audio_effect::aidl::android::hardware::audio::effect::IEffect::IEffect;
use android_media_audio_common_types::aidl::android::media::audio::common::{
    AudioDualMonoMode::AudioDualMonoMode, AudioLatencyMode::AudioLatencyMode,
    AudioPlaybackRate::AudioPlaybackRate, MicrophoneDynamicInfo::MicrophoneDynamicInfo,
};
use binder::{BinderFeatures, ExceptionCode, Interface, Status, Strong};

use crate::host::{HostStream, now_ns};
use aim_fmq::{Queue, TypedQueue};

pub fn exception<T>(code: ExceptionCode) -> binder::Result<T> {
    Err(Status::new_exception(code, None))
}

/// `StreamDescriptor.Command` as the client's C++ lays it out: an 8-bit tag,
/// then the (4-byte aligned) value.
#[repr(C)]
#[derive(Clone, Copy)]
struct Command {
    tag: i8,
    pad: [u8; 3],
    value: i32,
}

const HAL_RESERVED_EXIT: i8 = 0;
const GET_STATUS: i8 = 1;
const START: i8 = 2;
const BURST: i8 = 3;
const DRAIN: i8 = 4;
const STANDBY: i8 = 5;
const PAUSE: i8 = 6;
const FLUSH: i8 = 7;

/// `StreamDescriptor.DrainMode`.
const DRAIN_UNSPECIFIED: i8 = 0;
const DRAIN_ALL: i8 = 1;
const DRAIN_EARLY_NOTIFY: i8 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Position {
    frames: i64,
    time_ns: i64,
}

const UNKNOWN_POSITION: Position = Position {
    frames: -1,
    time_ns: -1,
};

/// `StreamDescriptor.Reply`, with its tail padding spelled out.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Reply {
    status: i32,
    fmq_byte_count: i32,
    observable: Position,
    hardware: Position,
    latency_ms: i32,
    xrun_frames: i32,
    state: i32,
    pad: i32,
}

const _: () = assert!(std::mem::size_of::<Command>() == 8);
const _: () = assert!(std::mem::size_of::<Reply>() == 56);

/// binder `status_t` values in `Reply.status`.
const STATUS_OK: i32 = 0;
const STATUS_BAD_VALUE: i32 = -libc::EINVAL;
const STATUS_INVALID_OPERATION: i32 = -libc::ENOSYS;
const STATUS_NOT_ENOUGH_DATA: i32 = -libc::ENODATA;

/// `StreamDescriptor.State`.
#[derive(Clone, Copy, Debug, PartialEq)]
#[repr(i32)]
enum State {
    Standby = 1,
    Idle = 2,
    Active = 3,
    Paused = 4,
    Draining = 5,
    DrainPaused = 6,
    Error = 100,
}

/// What a stream moves, fixed at open.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub input: bool,
    /// `aim_hostcall::audio::FORMAT_*`.
    pub host_format: u32,
    pub rate: u32,
    pub channels: u32,
    pub frame_bytes: usize,
    pub buffer_frames: usize,
    pub nominal_latency_ms: i32,
    /// Frames the host ring holds.
    pub ring_frames: u32,
}

/// State the module and the binder objects share with the worker.
pub struct Shared {
    pub connected: AtomicBool,
    closed: AtomicBool,
    command: TypedQueue<Command>,
}

impl Shared {
    pub fn is_closed(&self) -> bool {
        self.closed.load(Acquire)
    }
}

/// A stream: its worker and the state it shares.
pub struct Core {
    pub shared: Arc<Shared>,
    worker: Mutex<Option<JoinHandle<()>>>,
    cookie: i32,
}

impl Core {
    /// Create the FMQs and the host stream, and start the worker.
    pub fn open(params: Params, connected: bool) -> io::Result<(Arc<Self>, StreamDescriptor)> {
        let command = TypedQueue::<Command>::new(1)?;
        let reply = TypedQueue::<Reply>::new(1)?;
        let data = Queue::new(1, params.buffer_frames * params.frame_bytes, false)?;
        let host = HostStream::open(
            params.input,
            params.host_format,
            params.rate,
            params.channels,
            params.frame_bytes,
            params.ring_frames,
        )?;
        let desc = StreamDescriptor {
            command: command.descriptor()?,
            reply: reply.descriptor()?,
            frameSizeBytes: params.frame_bytes as i32,
            bufferSizeFrames: params.buffer_frames as i64,
            audio: AudioBuffer::Fmq(data.descriptor()?),
        };
        let shared = Arc::new(Shared {
            connected: AtomicBool::new(connected),
            closed: AtomicBool::new(false),
            command,
        });
        let cookie = (now_ns() as i32) | 1;
        let worker = Worker {
            shared: shared.clone(),
            reply,
            buffer: vec![0; data.size()],
            data,
            host: Some(host),
            params,
            state: State::Standby,
            running: false,
            frames: 0,
            dropped: 0,
            underruns: 0,
            xruns_seen: 0,
            cookie,
        };
        let handle = std::thread::Builder::new()
            .name(if params.input { "reader" } else { "writer" }.into())
            .spawn(move || worker.run())?;
        Ok((
            Arc::new(Self {
                shared,
                worker: Mutex::new(Some(handle)),
                cookie,
            }),
            desc,
        ))
    }

    /// `IStreamCommon.close`: stop the worker with the internal exit command.
    pub fn close(&self) -> binder::Result<()> {
        if self.shared.closed.swap(true, Release) {
            log::error!("close: stream was already closed");
            return exception(ExceptionCode::ILLEGAL_STATE);
        }
        let exit = Command {
            tag: HAL_RESERVED_EXIT,
            pad: [0; 3],
            value: self.cookie,
        };
        if !self.shared.command.write_blocking(&exit) {
            log::error!("close: cannot send the exit command");
        }
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
        Ok(())
    }

    fn check_open(&self) -> binder::Result<()> {
        if self.shared.is_closed() {
            return exception(ExceptionCode::ILLEGAL_STATE);
        }
        Ok(())
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        if !self.shared.is_closed() {
            log::warn!("stream released without close");
            let _ = self.close();
        }
    }
}

struct Worker {
    shared: Arc<Shared>,
    reply: TypedQueue<Reply>,
    data: Queue,
    buffer: Vec<u8>,
    host: Option<HostStream>,
    params: Params,
    state: State,
    /// The device callback is running.
    running: bool,
    /// Frames moved through the data FMQ.
    frames: u64,
    /// Output frames consumed while not connected to a device.
    dropped: u64,
    /// Output: frames played as silence while the stream was active
    /// (between bursts), and the ring's xrun count last seen.
    underruns: u64,
    xruns_seen: u64,
    cookie: i32,
}

impl Worker {
    fn run(mut self) {
        loop {
            let Some(command) = self.shared.command.read_blocking() else {
                log::error!("reading a command failed");
                break;
            };
            if command.tag == HAL_RESERVED_EXIT {
                if command.value == self.cookie {
                    break;
                }
                log::warn!("exit command with a bad cookie {}", command.value);
                // Only a test's exit command (cookie 0) gets a reply.
                if command.value != 0 {
                    continue;
                }
            }
            let reply = if self.params.input {
                self.input_cycle(command)
            } else {
                self.output_cycle(command)
            };
            if !self.reply.write_blocking(&reply) {
                log::error!("writing a reply failed");
                break;
            }
        }
        self.log_stats("closed");
        self.host = None;
    }

    fn host(&self) -> &HostStream {
        self.host.as_ref().unwrap()
    }

    fn set_running(&mut self, running: bool) {
        if running != self.running {
            let ok = if running {
                self.host().start()
            } else {
                self.host().stop()
            };
            if !ok {
                log::error!(
                    "host stream {} failed",
                    if running { "start" } else { "stop" }
                );
            }
            self.running = running;
        }
    }

    fn position(&self, now: i64) -> Position {
        if !self.shared.connected.load(Relaxed) {
            return UNKNOWN_POSITION;
        }
        let frames = if self.params.input {
            self.frames
        } else {
            self.host().presented(now) + self.dropped
        };
        Position {
            frames: frames as i64,
            time_ns: now,
        }
    }

    fn reply(&self, status: i32) -> Reply {
        Reply {
            status,
            observable: self.position(now_ns()),
            hardware: UNKNOWN_POSITION,
            latency_ms: self.host().latency_ms().max(self.params.nominal_latency_ms),
            xrun_frames: self.underruns as i32,
            ..Default::default()
        }
    }

    /// Count the ring's xruns since the last look as underruns if the
    /// stream was active meanwhile; idle silence is not an underrun.
    fn note_xruns(&mut self, active: bool) {
        let xruns = self.host().ring().xruns.load(Relaxed);
        if active {
            self.underruns += xruns - self.xruns_seen;
        }
        self.xruns_seen = xruns;
    }

    fn wrong_state(&self, command: Command) -> Reply {
        log::warn!(
            "command {} cannot be handled in state {:?}",
            command.tag,
            self.state
        );
        Reply {
            status: STATUS_INVALID_OPERATION,
            ..Default::default()
        }
    }

    fn finish(&self, mut reply: Reply) -> Reply {
        reply.state = self.state as i32;
        reply
    }

    fn output_cycle(&mut self, command: Command) -> Reply {
        // A synchronous drain completes on the next command, as in the
        // reference with no transient-state delay; the ring keeps playing.
        if self.state == State::Draining {
            self.state = State::Idle;
        }
        let reply = match command.tag {
            HAL_RESERVED_EXIT => Reply {
                status: STATUS_BAD_VALUE,
                ..Default::default()
            },
            GET_STATUS => self.reply(STATUS_OK),
            START => match self.state {
                State::Standby => {
                    self.state = State::Idle;
                    self.reply(STATUS_OK)
                }
                State::Paused => {
                    self.set_running(true);
                    self.state = State::Active;
                    self.reply(STATUS_OK)
                }
                State::DrainPaused => {
                    self.set_running(true);
                    self.state = State::Draining;
                    self.reply(STATUS_OK)
                }
                _ => self.wrong_state(command),
            },
            BURST if command.value < 0 => Reply {
                status: STATUS_BAD_VALUE,
                ..Default::default()
            },
            BURST => {
                if self.state == State::Error {
                    self.wrong_state(command)
                } else {
                    // A burst while stopped primes the ring without playing.
                    let primes = matches!(
                        self.state,
                        State::Standby | State::Paused | State::DrainPaused
                    );
                    self.note_xruns(self.state == State::Active);
                    let reply = self.write(command.value as usize, !primes);
                    self.state = if primes { State::Paused } else { State::Active };
                    reply
                }
            }
            DRAIN => match command.value as i8 {
                DRAIN_ALL | DRAIN_EARLY_NOTIFY if self.state == State::Active => {
                    self.state = State::Draining;
                    self.reply(STATUS_OK)
                }
                DRAIN_ALL | DRAIN_EARLY_NOTIFY => self.wrong_state(command),
                _ => Reply {
                    status: STATUS_BAD_VALUE,
                    ..Default::default()
                },
            },
            STANDBY if self.state == State::Idle => {
                let reply = self.reply(STATUS_OK);
                self.set_running(false);
                self.host().discard();
                self.log_stats("in standby");
                self.state = State::Standby;
                reply
            }
            PAUSE => match self.state {
                State::Active => {
                    self.set_running(false);
                    self.state = State::Paused;
                    self.reply(STATUS_OK)
                }
                State::Draining => {
                    self.set_running(false);
                    self.state = State::DrainPaused;
                    self.reply(STATUS_OK)
                }
                _ => self.wrong_state(command),
            },
            FLUSH if matches!(self.state, State::Paused | State::DrainPaused) => {
                self.host().discard();
                self.state = State::Idle;
                self.reply(STATUS_OK)
            }
            STANDBY | FLUSH => self.wrong_state(command),
            _ => Reply {
                status: STATUS_BAD_VALUE,
                ..Default::default()
            },
        };
        self.finish(reply)
    }

    fn input_cycle(&mut self, command: Command) -> Reply {
        let reply = match command.tag {
            HAL_RESERVED_EXIT => Reply {
                status: STATUS_BAD_VALUE,
                ..Default::default()
            },
            GET_STATUS => self.reply(STATUS_OK),
            START if matches!(self.state, State::Standby | State::Draining) => {
                self.set_running(true);
                self.state = if self.state == State::Standby {
                    State::Idle
                } else {
                    State::Active
                };
                self.reply(STATUS_OK)
            }
            BURST if command.value < 0 => Reply {
                status: STATUS_BAD_VALUE,
                ..Default::default()
            },
            BURST
                if matches!(
                    self.state,
                    State::Idle | State::Active | State::Paused | State::Draining
                ) =>
            {
                if self.state == State::Paused {
                    self.set_running(true);
                }
                let reply = self.read(command.value as usize);
                self.state = if self.state == State::Draining {
                    State::Standby
                } else {
                    State::Active
                };
                reply
            }
            DRAIN if command.value as i8 == DRAIN_UNSPECIFIED => {
                if self.state == State::Active {
                    self.state = State::Draining;
                    self.reply(STATUS_OK)
                } else {
                    self.wrong_state(command)
                }
            }
            STANDBY if self.state == State::Idle => {
                let reply = self.reply(STATUS_OK);
                self.set_running(false);
                self.host().discard();
                self.log_stats("in standby");
                self.state = State::Standby;
                reply
            }
            PAUSE if self.state == State::Active => {
                self.set_running(false);
                self.state = State::Paused;
                self.reply(STATUS_OK)
            }
            FLUSH if self.state == State::Paused => {
                self.host().discard();
                self.state = State::Standby;
                self.reply(STATUS_OK)
            }
            START | BURST | STANDBY | PAUSE | FLUSH => self.wrong_state(command),
            _ => Reply {
                status: STATUS_BAD_VALUE,
                ..Default::default()
            },
        };
        self.finish(reply)
    }

    fn frames_duration(&self, frames: usize) -> Duration {
        Duration::from_nanos(frames as u64 * 1_000_000_000 / self.params.rate as u64)
    }

    /// `burst` on an output: move the data FMQ's bytes into the ring.
    fn write(&mut self, client_bytes: usize, play: bool) -> Reply {
        let available = self.data.available_to_read();
        if !self.data.read(&mut self.buffer[..available]) {
            log::warn!("reading {available} bytes from the data FMQ failed");
            return Reply {
                status: STATUS_NOT_ENOUGH_DATA,
                ..Default::default()
            };
        }
        let fb = self.params.frame_bytes;
        let bytes = client_bytes.min(available) / fb * fb;
        let frames = bytes / fb;
        if self.shared.connected.load(Relaxed) {
            if play {
                self.set_running(true);
            }
            self.host().write(&self.buffer[..bytes]);
        } else {
            std::thread::sleep(self.frames_duration(frames));
            self.dropped += frames as u64;
        }
        self.frames += frames as u64;
        let mut reply = self.reply(STATUS_OK);
        reply.fmq_byte_count = bytes as i32;
        reply
    }

    /// `burst` on an input: capture into the data FMQ.
    fn read(&mut self, client_bytes: usize) -> Reply {
        let fb = self.params.frame_bytes;
        let bytes = client_bytes.min(self.data.available_to_write()) / fb * fb;
        let frames = bytes / fb;
        if self.shared.connected.load(Relaxed) {
            // Frames that have not arrived after twice their duration (plus
            // the device's period) are silence.
            let timeout = self.frames_duration(frames) * 2 + Duration::from_millis(20);
            let host = self.host.as_ref().unwrap();
            host.read(&mut self.buffer[..bytes], timeout);
        } else {
            std::thread::sleep(self.frames_duration(frames));
            self.buffer[..bytes].fill(0);
        }
        if !self.data.write(&self.buffer[..bytes]) {
            log::warn!("writing {bytes} bytes to the data FMQ failed");
            return Reply {
                status: STATUS_NOT_ENOUGH_DATA,
                ..Default::default()
            };
        }
        self.frames += frames as u64;
        let mut reply = self.reply(STATUS_OK);
        reply.fmq_byte_count = bytes as i32;
        reply
    }

    /// What the host ring saw since the stream opened; the peak and the
    /// latency probe restart after each report (the callback is stopped).
    fn log_stats(&self, when: &str) {
        let ring = self.host().ring();
        let count = ring.latency_count.load(Relaxed);
        let callbacks = ring.callbacks.load(Relaxed);
        log::info!(
            "{} stream {when}: {} frames, {callbacks} device callbacks ({} us of host time each), \
             {} xrun frames ({} while active), peak {:.1} dBFS, \
             write-to-callback latency avg {} us max {} us over {count}",
            if self.params.input { "input" } else { "output" },
            self.frames,
            ring.callback_ns.load(Relaxed) / callbacks.max(1) / 1000,
            ring.xruns.load(Relaxed),
            self.underruns,
            20.0 * f32::from_bits(ring.peak_bits.load(Relaxed)).log10(),
            ring.latency_sum_ns.load(Relaxed) / count.max(1) / 1000,
            ring.latency_max_ns.load(Relaxed) / 1000,
        );
        ring.peak_bits.store(0, Relaxed);
        ring.mark.store(0, Relaxed);
        ring.latency_count.store(0, Relaxed);
        ring.latency_sum_ns.store(0, Relaxed);
        ring.latency_max_ns.store(0, Relaxed);
    }
}

/// `IStreamCommon`.
pub struct StreamCommon(pub Arc<Core>);

impl Interface for StreamCommon {}

impl IStreamCommon for StreamCommon {
    fn close(&self) -> binder::Result<()> {
        self.0.close()
    }

    fn prepareToClose(&self) -> binder::Result<()> {
        self.0.check_open()
    }

    fn updateHwAvSyncId(&self, _id: i32) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn getVendorParameters(&self, _ids: &[String]) -> binder::Result<Vec<VendorParameter>> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn setVendorParameters(&self, _p: &[VendorParameter], _async: bool) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn addEffect(&self, _effect: &Strong<dyn IEffect>) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn removeEffect(&self, _effect: &Strong<dyn IEffect>) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }
}

fn common(core: &Arc<Core>) -> Strong<dyn IStreamCommon> {
    BnStreamCommon::new_binder(StreamCommon(core.clone()), BinderFeatures::default())
}

/// `IStreamOut`.
pub struct StreamOut {
    core: Arc<Core>,
    common: Strong<dyn IStreamCommon>,
}

impl StreamOut {
    pub fn new(core: Arc<Core>) -> Self {
        let common = common(&core);
        Self { core, common }
    }
}

impl Interface for StreamOut {}

impl IStreamOut for StreamOut {
    fn getStreamCommon(&self) -> binder::Result<Strong<dyn IStreamCommon>> {
        Ok(self.common.clone())
    }

    fn updateMetadata(&self, _metadata: &SourceMetadata) -> binder::Result<()> {
        self.core.check_open()
    }

    fn updateOffloadMetadata(&self, _metadata: &AudioOffloadMetadata) -> binder::Result<()> {
        exception(ExceptionCode::ILLEGAL_STATE)
    }

    fn getHwVolume(&self) -> binder::Result<Vec<f32>> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn setHwVolume(&self, _volumes: &[f32]) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn getAudioDescriptionMixLevel(&self) -> binder::Result<f32> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn setAudioDescriptionMixLevel(&self, _level: f32) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn getDualMonoMode(&self) -> binder::Result<AudioDualMonoMode> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn setDualMonoMode(&self, _mode: AudioDualMonoMode) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn getRecommendedLatencyModes(&self) -> binder::Result<Vec<AudioLatencyMode>> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn setLatencyMode(&self, _mode: AudioLatencyMode) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn getPlaybackRateParameters(&self) -> binder::Result<AudioPlaybackRate> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn setPlaybackRateParameters(&self, _rate: &AudioPlaybackRate) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn selectPresentation(&self, _presentation: i32, _program: i32) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }
}

/// `IStreamIn`.
pub struct StreamIn {
    core: Arc<Core>,
    common: Strong<dyn IStreamCommon>,
}

impl StreamIn {
    pub fn new(core: Arc<Core>) -> Self {
        let common = common(&core);
        Self { core, common }
    }
}

impl Interface for StreamIn {}

impl IStreamIn for StreamIn {
    fn getStreamCommon(&self) -> binder::Result<Strong<dyn IStreamCommon>> {
        Ok(self.common.clone())
    }

    fn getActiveMicrophones(&self) -> binder::Result<Vec<MicrophoneDynamicInfo>> {
        self.core.check_open()?;
        Ok(Vec::new())
    }

    fn getMicrophoneDirection(&self) -> binder::Result<MicrophoneDirection> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn setMicrophoneDirection(&self, _direction: MicrophoneDirection) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn getMicrophoneFieldDimension(&self) -> binder::Result<f32> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn setMicrophoneFieldDimension(&self, _zoom: f32) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn updateMetadata(&self, _metadata: &SinkMetadata) -> binder::Result<()> {
        self.core.check_open()
    }

    fn getHwGain(&self) -> binder::Result<Vec<f32>> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn setHwGain(&self, _gains: &[f32]) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }
}
