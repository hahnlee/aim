//! The module's configuration: ports, routes and port configs, kept in
//! plain Rust types (the generated AIDL types are neither `Clone` nor
//! `PartialEq`) and converted at the binder boundary.
//!
//! The primary module is built from the Mac's default devices, in the shape
//! of AOSP's reference primary configuration (`Configuration.cpp`): a
//! speaker and a built-in microphone device port, each with dynamic
//! profiles, a "primary output" and a "primary input" mix port, and a route
//! between each pair. A device the Mac lacks is left out, and the original
//! services take their no-device paths.

use aim_hostcall::audio::Devices;
use android_media_audio_common_types::aidl::android::media::audio::common;
use android_media_audio_common_types::aidl::android::media::audio::common::{
    AudioChannelLayout::AudioChannelLayout, AudioDevice::AudioDevice,
    AudioDeviceDescription::AudioDeviceDescription, AudioDeviceType::AudioDeviceType,
    AudioFormatDescription::AudioFormatDescription, AudioFormatType::AudioFormatType,
    AudioIoFlags::AudioIoFlags, AudioOutputFlags::AudioOutputFlags, AudioPort::AudioPort,
    AudioPortConfig::AudioPortConfig, AudioPortDeviceExt::AudioPortDeviceExt,
    AudioPortExt::AudioPortExt, AudioPortMixExt::AudioPortMixExt,
    AudioPortMixExtUseCase::AudioPortMixExtUseCase, AudioProfile::AudioProfile,
    AudioSource::AudioSource, AudioStreamType::AudioStreamType, Int::Int, PcmType::PcmType,
};

pub const LAYOUT_MONO: i32 = common::AudioChannelLayout::LAYOUT_MONO;
pub const LAYOUT_STEREO: i32 = common::AudioChannelLayout::LAYOUT_STEREO;
/// The rate the output mix port offers; CoreAudio converts to the device.
pub const OUTPUT_RATE: i32 = 48000;

/// `AudioFormatDescription`.
#[derive(Clone, Debug, PartialEq)]
pub struct Format {
    pub kind: i8,
    pub pcm: i8,
    pub encoding: String,
}

impl Format {
    pub fn pcm(pcm: PcmType) -> Self {
        Self {
            kind: AudioFormatType::PCM.0,
            pcm: pcm.0,
            encoding: String::new(),
        }
    }

    /// The dynamic (unspecified) format.
    pub fn dynamic() -> Self {
        Self::from_aidl(&AudioFormatDescription::default())
    }

    pub fn from_aidl(f: &AudioFormatDescription) -> Self {
        Self {
            kind: f.r#type.0,
            pcm: f.pcm.0,
            encoding: f.encoding.clone(),
        }
    }

    pub fn to_aidl(&self) -> AudioFormatDescription {
        AudioFormatDescription {
            r#type: AudioFormatType(self.kind),
            pcm: PcmType(self.pcm),
            encoding: self.encoding.clone(),
        }
    }

    /// Bytes per sample of a PCM format this module streams.
    pub fn sample_bytes(&self) -> Option<usize> {
        if self.kind != AudioFormatType::PCM.0 || !self.encoding.is_empty() {
            return None;
        }
        match PcmType(self.pcm) {
            PcmType::INT_16_BIT => Some(2),
            PcmType::FLOAT_32_BIT => Some(4),
            _ => None,
        }
    }
}

/// `AudioChannelLayout`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Channels {
    None(i32),
    Invalid(i32),
    Index(i32),
    Layout(i32),
    Voice(i32),
}

impl Channels {
    pub fn from_aidl(c: &AudioChannelLayout) -> Self {
        match *c {
            AudioChannelLayout::None(v) => Self::None(v),
            AudioChannelLayout::Invalid(v) => Self::Invalid(v),
            AudioChannelLayout::IndexMask(v) => Self::Index(v),
            AudioChannelLayout::LayoutMask(v) => Self::Layout(v),
            AudioChannelLayout::VoiceMask(v) => Self::Voice(v),
        }
    }

    pub fn to_aidl(self) -> AudioChannelLayout {
        match self {
            Self::None(v) => AudioChannelLayout::None(v),
            Self::Invalid(v) => AudioChannelLayout::Invalid(v),
            Self::Index(v) => AudioChannelLayout::IndexMask(v),
            Self::Layout(v) => AudioChannelLayout::LayoutMask(v),
            Self::Voice(v) => AudioChannelLayout::VoiceMask(v),
        }
    }

    /// The channel count of a positional or index layout.
    pub fn count(self) -> u32 {
        match self {
            Self::Layout(m) | Self::Index(m) => m.count_ones(),
            _ => 0,
        }
    }
}

/// `AudioIoFlags`: whether a port is an input, and its flag bits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IoFlags {
    Input(i32),
    Output(i32),
}

impl IoFlags {
    pub fn from_aidl(f: &AudioIoFlags) -> Self {
        match *f {
            AudioIoFlags::Input(v) => Self::Input(v),
            AudioIoFlags::Output(v) => Self::Output(v),
        }
    }

    pub fn to_aidl(self) -> AudioIoFlags {
        match self {
            Self::Input(v) => AudioIoFlags::Input(v),
            Self::Output(v) => AudioIoFlags::Output(v),
        }
    }

    pub fn is_input(self) -> bool {
        matches!(self, Self::Input(_))
    }
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub format: Format,
    pub channels: Vec<Channels>,
    pub rates: Vec<i32>,
}

#[derive(Clone, Debug)]
pub enum PortExt {
    /// An attached device port of `AudioDeviceType` `device`.
    Device {
        device: i32,
        default: bool,
    },
    Mix {
        max_open: i32,
        max_active: i32,
    },
}

#[derive(Clone, Debug)]
pub struct Port {
    pub id: i32,
    pub name: String,
    pub flags: IoFlags,
    pub profiles: Vec<Profile>,
    pub ext: PortExt,
}

fn device_aidl(device: i32) -> AudioDevice {
    AudioDevice {
        r#type: AudioDeviceDescription {
            r#type: AudioDeviceType(device),
            connection: String::new(),
        },
        ..Default::default()
    }
}

fn device_ext(device: i32, flags: i32) -> AudioPortExt {
    AudioPortExt::Device(AudioPortDeviceExt {
        device: device_aidl(device),
        flags,
        ..Default::default()
    })
}

impl Port {
    pub fn is_mix(&self) -> bool {
        matches!(self.ext, PortExt::Mix { .. })
    }

    pub fn to_aidl(&self) -> AudioPort {
        AudioPort {
            id: self.id,
            name: self.name.clone(),
            profiles: self
                .profiles
                .iter()
                .map(|p| AudioProfile {
                    format: p.format.to_aidl(),
                    channelMasks: p.channels.iter().map(|c| c.to_aidl()).collect(),
                    sampleRates: p.rates.clone(),
                    ..Default::default()
                })
                .collect(),
            flags: self.flags.to_aidl(),
            ext: match self.ext {
                PortExt::Device { device, default } => device_ext(
                    device,
                    if default {
                        1 << common::AudioPortDeviceExt::FLAG_INDEX_DEFAULT_DEVICE
                    } else {
                        0
                    },
                ),
                PortExt::Mix {
                    max_open,
                    max_active,
                } => AudioPortExt::Mix(AudioPortMixExt {
                    maxOpenStreamCount: max_open,
                    maxActiveStreamCount: max_active,
                    ..Default::default()
                }),
            },
            ..Default::default()
        }
    }

    /// Only dynamic profiles (as device ports have here).
    pub fn is_dynamic(&self) -> bool {
        self.profiles.iter().all(|p| {
            p.format == Format::dynamic()
                || p.channels.iter().all(|c| *c == Channels::None(0))
                || p.rates.iter().all(|r| *r == 0)
        })
    }

    pub fn profile(&self, format: &Format) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.format == *format)
    }
}

/// `AudioPortMixExtUseCase`.
#[derive(Clone, Debug, PartialEq)]
pub enum UseCase {
    Unspecified,
    Stream(i32),
    Source(i32),
}

/// A port config's `ext`.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigExt {
    Unspecified,
    Device(i32),
    Mix { handle: i32, usecase: UseCase },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PortConfig {
    pub id: i32,
    pub port_id: i32,
    pub rate: Option<i32>,
    pub channels: Option<Channels>,
    pub format: Option<Format>,
    pub flags: Option<IoFlags>,
    pub ext: ConfigExt,
}

impl PortConfig {
    /// The requested fields of a client's config; `None` for a tag this
    /// module does not model (a session ext), and whether a gain was asked.
    pub fn from_aidl(c: &AudioPortConfig) -> Option<(Self, bool)> {
        let ext = match &c.ext {
            AudioPortExt::Unspecified(_) => ConfigExt::Unspecified,
            AudioPortExt::Device(d) => ConfigExt::Device(d.device.r#type.r#type.0),
            AudioPortExt::Mix(m) => ConfigExt::Mix {
                handle: m.handle,
                usecase: match &m.usecase {
                    AudioPortMixExtUseCase::Unspecified(_) => UseCase::Unspecified,
                    AudioPortMixExtUseCase::Stream(s) => UseCase::Stream(s.0),
                    AudioPortMixExtUseCase::Source(s) => UseCase::Source(s.0),
                },
            },
            AudioPortExt::Session(_) => return None,
        };
        Some((
            Self {
                id: c.id,
                port_id: c.portId,
                rate: c.sampleRate.as_ref().map(|r| r.value),
                channels: c.channelMask.as_ref().map(Channels::from_aidl),
                format: c.format.as_ref().map(Format::from_aidl),
                flags: c.flags.as_ref().map(IoFlags::from_aidl),
                ext,
            },
            c.gain.is_some(),
        ))
    }

    pub fn to_aidl(&self) -> AudioPortConfig {
        AudioPortConfig {
            id: self.id,
            portId: self.port_id,
            sampleRate: self.rate.map(|value| Int { value }),
            channelMask: self.channels.map(Channels::to_aidl),
            format: self.format.as_ref().map(Format::to_aidl),
            gain: None,
            flags: self.flags.map(IoFlags::to_aidl),
            ext: match &self.ext {
                ConfigExt::Unspecified => AudioPortExt::Unspecified(false),
                ConfigExt::Device(device) => device_ext(*device, 0),
                ConfigExt::Mix { handle, usecase } => AudioPortExt::Mix(AudioPortMixExt {
                    handle: *handle,
                    usecase: match usecase {
                        UseCase::Unspecified => AudioPortMixExtUseCase::Unspecified(false),
                        UseCase::Stream(s) => AudioPortMixExtUseCase::Stream(AudioStreamType(*s)),
                        UseCase::Source(s) => AudioPortMixExtUseCase::Source(AudioSource(*s)),
                    },
                    ..Default::default()
                }),
            },
        }
    }

    /// The same kind of ext as `port`'s.
    pub fn ext_of(port: &Port) -> ConfigExt {
        match port.ext {
            PortExt::Device { device, .. } => ConfigExt::Device(device),
            PortExt::Mix { .. } => ConfigExt::Mix {
                handle: 0,
                usecase: UseCase::Unspecified,
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct Route {
    pub sources: Vec<i32>,
    pub sink: i32,
    pub exclusive: bool,
}

pub struct Configuration {
    pub ports: Vec<Port>,
    pub routes: Vec<Route>,
    pub initial_configs: Vec<PortConfig>,
    pub next_id: i32,
}

fn standard_profiles(channels: &[i32], rate: i32) -> Vec<Profile> {
    [PcmType::INT_16_BIT, PcmType::FLOAT_32_BIT]
        .into_iter()
        .map(|pcm| Profile {
            format: Format::pcm(pcm),
            channels: channels.iter().map(|&c| Channels::Layout(c)).collect(),
            rates: vec![rate],
        })
        .collect()
}

/// The primary module over the Mac's default devices.
pub fn primary(devices: &Devices) -> Configuration {
    let mut c = Configuration {
        ports: Vec::new(),
        routes: Vec::new(),
        initial_configs: Vec::new(),
        next_id: 1,
    };
    let add = |c: &mut Configuration, port: Port| {
        if let PortExt::Device { device, .. } = port.ext {
            c.initial_configs.push(PortConfig {
                id: port.id,
                port_id: port.id,
                rate: Some(0),
                channels: Some(Channels::None(0)),
                format: Some(Format::dynamic()),
                flags: Some(port.flags),
                ext: ConfigExt::Device(device),
            });
        }
        c.ports.push(port);
    };
    if devices.output.present != 0 {
        let (speaker, mix) = (c.next_id, c.next_id + 1);
        c.next_id += 2;
        add(
            &mut c,
            Port {
                id: speaker,
                name: "Speaker".into(),
                flags: IoFlags::Output(0),
                profiles: Vec::new(),
                ext: PortExt::Device {
                    device: AudioDeviceType::OUT_SPEAKER.0,
                    default: true,
                },
            },
        );
        add(
            &mut c,
            Port {
                id: mix,
                name: "primary output".into(),
                flags: IoFlags::Output(1 << AudioOutputFlags::PRIMARY.0),
                profiles: standard_profiles(&[LAYOUT_STEREO], OUTPUT_RATE),
                ext: PortExt::Mix {
                    max_open: 0,
                    max_active: 0,
                },
            },
        );
        c.routes.push(Route {
            sources: vec![mix],
            sink: speaker,
            exclusive: false,
        });
    }
    if devices.input.present != 0 {
        let (mic, mix) = (c.next_id, c.next_id + 1);
        c.next_id += 2;
        add(
            &mut c,
            Port {
                id: mic,
                name: "Built-In Mic".into(),
                flags: IoFlags::Input(0),
                profiles: Vec::new(),
                ext: PortExt::Device {
                    device: AudioDeviceType::IN_MICROPHONE.0,
                    default: true,
                },
            },
        );
        // AUHAL captures at the device's own rate.
        add(
            &mut c,
            Port {
                id: mix,
                name: "primary input".into(),
                flags: IoFlags::Input(0),
                profiles: standard_profiles(
                    &[LAYOUT_MONO, LAYOUT_STEREO],
                    devices.input.sample_rate as i32,
                ),
                ext: PortExt::Mix {
                    max_open: 0,
                    max_active: 1,
                },
            },
        );
        c.routes.push(Route {
            sources: vec![mic],
            sink: mix,
            exclusive: false,
        });
    }
    c
}
