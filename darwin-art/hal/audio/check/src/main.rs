//! `audio-hal-check`: plays a tone and records through the audio HAL's
//! `IModule/default` the way audioserver's libaudiohal (`Hal2AidlMapper`,
//! `StreamHalAidl`) drives it: a mix port config, the device's initial
//! config, a patch between them, a stream with its buffer at the patch's
//! minimum, and `start`, `burst`, `drain`/`pause`, `standby` over the
//! stream's FMQs. It is the HAL-level check of
//! `crates/darwin-linux-abi/tests/audio.rs`; it is not part of the image.
//!
//! Prints one `key value...` line per measurement, then `ok done`.
//! Usage: audio-hal-check [play_ms] [record_ms]

use std::time::{Duration, Instant};

use android_hardware_audio_core::aidl::android::hardware::audio::core::{
    AudioPatch::AudioPatch,
    IModule::{
        IModule, OpenInputStreamArguments::OpenInputStreamArguments,
        OpenOutputStreamArguments::OpenOutputStreamArguments,
    },
    IStreamCommon::IStreamCommon,
    StreamDescriptor::{AudioBuffer::AudioBuffer, StreamDescriptor},
};
use android_media_audio_common_types::aidl::android::media::audio::common::{
    self as common, AudioChannelLayout::AudioChannelLayout,
    AudioFormatDescription::AudioFormatDescription, AudioFormatType::AudioFormatType,
    AudioIoFlags::AudioIoFlags, AudioPort::AudioPort, AudioPortConfig::AudioPortConfig,
    AudioPortExt::AudioPortExt, AudioPortMixExt::AudioPortMixExt, Int::Int, PcmType::PcmType,
};
use binder::Strong;
use darwin_fmq::{Queue, TypedQueue};

const MODULE: &str = "android.hardware.audio.core.IModule/default";
const RATE: i32 = 48000;
/// -90 dBFS.
const AMPLITUDE: f64 = 3.162e-5;

/// `StreamDescriptor.Command` and `.Reply` as the C++ backend lays them out.
#[repr(C)]
#[derive(Clone, Copy)]
struct Command {
    tag: i8,
    pad: [u8; 3],
    value: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
struct Reply {
    status: i32,
    fmq_byte_count: i32,
    observable_frames: i64,
    observable_ns: i64,
    hardware_frames: i64,
    hardware_ns: i64,
    latency_ms: i32,
    xrun_frames: i32,
    state: i32,
    pad: i32,
}

const START: i8 = 2;
const BURST: i8 = 3;
const DRAIN: i8 = 4;
const STANDBY: i8 = 5;
const PAUSE: i8 = 6;
const FLUSH: i8 = 7;
const GET_STATUS: i8 = 1;

const STATE_STANDBY: i32 = 1;
const STATE_IDLE: i32 = 2;
const STATE_ACTIVE: i32 = 3;
const STATE_PAUSED: i32 = 4;
const DRAIN_ALL: i32 = 1;

fn now_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a local timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

struct Stream {
    command: TypedQueue<Command>,
    reply: TypedQueue<Reply>,
    data: Queue,
    frame_bytes: usize,
}

impl Stream {
    fn new(desc: &StreamDescriptor) -> Self {
        let AudioBuffer::Fmq(data) = &desc.audio else {
            panic!("the stream has no data FMQ");
        };
        Self {
            command: TypedQueue::from_descriptor(&desc.command).expect("command FMQ"),
            reply: TypedQueue::from_descriptor(&desc.reply).expect("reply FMQ"),
            data: Queue::from_descriptor(data).expect("data FMQ"),
            frame_bytes: desc.frameSizeBytes as usize,
        }
    }

    fn send(&self, tag: i8, value: i32) -> Reply {
        let command = Command {
            tag,
            pad: [0; 3],
            value,
        };
        assert!(self.command.write_blocking(&command), "command FMQ write");
        self.reply.read_blocking().expect("reply FMQ read")
    }

    fn expect(&self, tag: i8, value: i32, state: i32) -> Reply {
        let r = self.send(tag, value);
        assert_eq!((r.status, r.state), (0, state), "command {tag}: {r:?}");
        r
    }
}

fn format(pcm: PcmType) -> AudioFormatDescription {
    AudioFormatDescription {
        r#type: AudioFormatType::PCM,
        pcm,
        encoding: String::new(),
    }
}

fn find_port(ports: &[AudioPort], input: bool, mix: bool) -> &AudioPort {
    ports
        .iter()
        .find(|p| {
            matches!(p.flags, AudioIoFlags::Input(_)) == input
                && matches!(p.ext, AudioPortExt::Mix(_)) == mix
        })
        .expect("no such port")
}

/// A mix port config, the device's initial config and the patch between
/// them (source to sink).
fn connect(
    module: &Strong<dyn IModule>,
    mix: &AudioPort,
    device: &AudioPort,
    channels: i32,
    rate: i32,
) -> (AudioPortConfig, AudioPatch) {
    let requested = AudioPortConfig {
        portId: mix.id,
        sampleRate: Some(Int { value: rate }),
        channelMask: Some(AudioChannelLayout::LayoutMask(channels)),
        format: Some(format(PcmType::FLOAT_32_BIT)),
        flags: Some(match mix.flags {
            AudioIoFlags::Input(f) => AudioIoFlags::Input(f),
            AudioIoFlags::Output(f) => AudioIoFlags::Output(f),
        }),
        ext: AudioPortExt::Mix(AudioPortMixExt {
            handle: 42,
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut config = AudioPortConfig::default();
    assert!(
        module.setAudioPortConfig(&requested, &mut config).unwrap(),
        "mix port config not applied: {config:?}"
    );
    let device_config = module
        .getAudioPortConfigs()
        .unwrap()
        .into_iter()
        .find(|c| c.portId == device.id)
        .expect("no initial device port config");
    let input = matches!(mix.flags, AudioIoFlags::Input(_));
    let (sources, sinks) = if input {
        (vec![device_config.id], vec![config.id])
    } else {
        (vec![config.id], vec![device_config.id])
    };
    let patch = module
        .setAudioPatch(&AudioPatch {
            sourcePortConfigIds: sources,
            sinkPortConfigIds: sinks,
            ..Default::default()
        })
        .unwrap();
    println!(
        "patch {} minimum_buffer_frames {} latency_ms {:?}",
        patch.id, patch.minimumStreamBufferSizeFrames, patch.latenciesMs
    );
    (config, patch)
}

fn play(module: &Strong<dyn IModule>, ports: &[AudioPort], ms: i64) {
    let (mix, speaker) = (
        find_port(ports, false, true),
        find_port(ports, false, false),
    );
    let (config, patch) = connect(
        module,
        mix,
        speaker,
        common::AudioChannelLayout::LAYOUT_STEREO,
        RATE,
    );
    let ret = module
        .openOutputStream(&OpenOutputStreamArguments {
            portConfigId: config.id,
            bufferSizeFrames: patch.minimumStreamBufferSizeFrames as i64,
            ..Default::default()
        })
        .unwrap();
    let s = Stream::new(&ret.desc);
    let stream = ret.stream.unwrap();
    s.expect(START, 0, STATE_IDLE);
    let buffer_frames = ret.desc.bufferSizeFrames as usize;
    let total = RATE as i64 * ms / 1000;
    let (mut written, mut phase) = (0i64, 0i64);
    let (mut latency_sum, mut latency_n, mut worst_burst) = (0.0f64, 0, Duration::ZERO);
    let mut last = Reply::default();
    let started = Instant::now();
    while written < total {
        let frames = buffer_frames.min(s.data.available_to_write() / s.frame_bytes);
        let mut bytes = Vec::with_capacity(frames * s.frame_bytes);
        for _ in 0..frames {
            // -90 dBFS: inaudible, but not digital silence.
            let v = AMPLITUDE * (phase as f64 * 440.0 * std::f64::consts::TAU / RATE as f64).sin();
            for _ in 0..2 {
                bytes.extend_from_slice(&(v as f32).to_ne_bytes());
            }
            phase += 1;
        }
        assert!(s.data.write(&bytes), "data FMQ write");
        let t = Instant::now();
        last = s.expect(BURST, bytes.len() as i32, STATE_ACTIVE);
        worst_burst = worst_burst.max(t.elapsed());
        assert_eq!(last.fmq_byte_count as usize, bytes.len());
        written += frames as i64;
        // Frames written but not yet heard, at the reply's time.
        if written > RATE as i64 / 2 && last.observable_frames >= 0 {
            let pending = written - last.observable_frames;
            latency_sum += pending as f64 * 1000.0 / RATE as f64
                + (now_ns() - last.observable_ns) as f64 / 1e6;
            latency_n += 1;
        }
    }
    let elapsed = started.elapsed();
    println!(
        "output written {written} frames in {:.0} ms, position {} frames, reported latency {} ms",
        elapsed.as_secs_f64() * 1000.0,
        last.observable_frames,
        last.latency_ms
    );
    println!(
        "output presentation_latency_ms {:.1} worst_burst_ms {:.1}",
        latency_sum / latency_n.max(1) as f64,
        worst_burst.as_secs_f64() * 1000.0
    );
    // Drain, let the ring play out, and go to standby: the HAL logs what
    // the device callback saw.
    s.send(DRAIN, DRAIN_ALL);
    std::thread::sleep(Duration::from_millis(300));
    let status = s.expect(GET_STATUS, 0, STATE_IDLE);
    println!(
        "output drained position {} frames",
        status.observable_frames
    );
    s.expect(STANDBY, 0, STATE_STANDBY);
    close(&stream.getStreamCommon().unwrap());
    module.resetAudioPatch(patch.id).unwrap();
    module.resetAudioPortConfig(config.id).unwrap();
}

fn record(module: &Strong<dyn IModule>, ports: &[AudioPort], ms: i64) {
    let (mix, mic) = (find_port(ports, true, true), find_port(ports, true, false));
    let rate = mix
        .profiles
        .first()
        .and_then(|p| p.sampleRates.first().copied())
        .unwrap_or(RATE);
    let (config, patch) = connect(
        module,
        mix,
        mic,
        common::AudioChannelLayout::LAYOUT_MONO,
        rate,
    );
    let ret = module
        .openInputStream(&OpenInputStreamArguments {
            portConfigId: config.id,
            bufferSizeFrames: patch.minimumStreamBufferSizeFrames as i64,
            ..Default::default()
        })
        .unwrap();
    let s = Stream::new(&ret.desc);
    let stream = ret.stream.unwrap();
    s.expect(START, 0, STATE_IDLE);
    let burst = ret.desc.bufferSizeFrames as usize * s.frame_bytes;
    let total = rate as i64 * ms / 1000;
    let (mut got, mut peak) = (0i64, 0f32);
    let mut buf = vec![0u8; burst];
    let started = Instant::now();
    while got < total {
        let r = s.expect(BURST, burst as i32, STATE_ACTIVE);
        let n = r.fmq_byte_count as usize;
        assert!(s.data.read(&mut buf[..n]), "data FMQ read");
        for sample in buf[..n].chunks_exact(4) {
            peak = peak.max(f32::from_ne_bytes(sample.try_into().unwrap()).abs());
        }
        got += (n / s.frame_bytes) as i64;
    }
    println!(
        "input read {got} frames in {:.0} ms, peak {peak:.5}",
        started.elapsed().as_secs_f64() * 1000.0
    );
    s.expect(PAUSE, 0, STATE_PAUSED);
    s.expect(FLUSH, 0, STATE_STANDBY);
    close(&stream.getStreamCommon().unwrap());
    module.resetAudioPatch(patch.id).unwrap();
    module.resetAudioPortConfig(config.id).unwrap();
}

fn close(common: &Strong<dyn IStreamCommon>) {
    common.close().unwrap();
    assert!(common.close().is_err(), "a second close must fail");
}

fn main() {
    let mut args = std::env::args().skip(1);
    let play_ms: i64 = args.next().map_or(2000, |a| a.parse().unwrap());
    let record_ms: i64 = args.next().map_or(1000, |a| a.parse().unwrap());
    binder::ProcessState::start_thread_pool();
    let module: Strong<dyn IModule> = binder::wait_for_interface(MODULE).expect(MODULE);
    let ports = module.getAudioPorts().unwrap();
    for p in &ports {
        println!("port {} {:?} profiles {}", p.id, p.name, p.profiles.len());
    }
    if play_ms > 0 {
        play(&module, &ports, play_ms);
    }
    if record_ms > 0 {
        record(&module, &ports, record_ms);
    }
    println!("ok done");
}
