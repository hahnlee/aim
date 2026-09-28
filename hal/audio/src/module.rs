//! `IModule/default`: ports, port configs, patches and streams, following
//! AOSP's reference `Module.cpp` (the behaviour libaudiohal and VTS expect),
//! over the configuration of [`crate::config::primary`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use android_hardware_audio_core::aidl::android::hardware::audio::core::{
    AudioPatch::AudioPatch,
    AudioRoute::AudioRoute,
    IBluetooth::IBluetooth,
    IBluetoothA2dp::IBluetoothA2dp,
    IBluetoothLe::IBluetoothLe,
    IModule::{
        IModule, OpenInputStreamArguments::OpenInputStreamArguments,
        OpenInputStreamReturn::OpenInputStreamReturn,
        OpenOutputStreamArguments::OpenOutputStreamArguments,
        OpenOutputStreamReturn::OpenOutputStreamReturn, ScreenRotation::ScreenRotation,
        SupportedPlaybackRateFactors::SupportedPlaybackRateFactors,
    },
    IStreamIn::BnStreamIn,
    IStreamOut::BnStreamOut,
    ITelephony::ITelephony,
    ModuleDebug::ModuleDebug,
    StreamDescriptor::StreamDescriptor,
    VendorParameter::VendorParameter,
};
use android_hardware_audio_core_sounddose::aidl::android::hardware::audio::core::sounddose::ISoundDose::ISoundDose;
use android_hardware_audio_effect::aidl::android::hardware::audio::effect::IEffect::IEffect;
use android_media_audio_common_types::aidl::android::media::audio::common::{
    AudioMMapPolicy::AudioMMapPolicy, AudioMMapPolicyInfo::AudioMMapPolicyInfo,
    AudioMMapPolicyType::AudioMMapPolicyType, AudioMode::AudioMode, AudioPort::AudioPort,
    AudioPortConfig::AudioPortConfig, AudioPortExt::AudioPortExt, MicrophoneInfo::MicrophoneInfo,
    PcmType::PcmType,
};
use binder::{BinderFeatures, ExceptionCode, Interface, Strong};
use aim_hostcall::audio::{Devices, FORMAT_FLOAT, FORMAT_PCM_16};

use crate::config::{self, Channels, ConfigExt, Format, Port, PortConfig, PortExt, Route};
use crate::stream::{self, Core, Params, StreamIn, StreamOut, exception};

/// A stream buffer may not exceed this (the reference's limit).
const MAX_BUFFER_BYTES: i64 = 1 << 30;

#[derive(Clone, Debug)]
struct Patch {
    id: i32,
    sources: Vec<i32>,
    sinks: Vec<i32>,
    minimum_buffer_frames: i32,
    latencies_ms: Vec<i32>,
}

impl Patch {
    fn to_aidl(&self) -> AudioPatch {
        AudioPatch {
            id: self.id,
            sourcePortConfigIds: self.sources.clone(),
            sinkPortConfigIds: self.sinks.clone(),
            minimumStreamBufferSizeFrames: self.minimum_buffer_frames,
            latenciesMs: self.latencies_ms.clone(),
        }
    }

    fn uses(&self, config_id: i32) -> bool {
        self.sources.contains(&config_id) || self.sinks.contains(&config_id)
    }
}

struct OpenStream {
    port_id: i32,
    shared: Weak<stream::Shared>,
}

struct State {
    ports: Vec<Port>,
    routes: Vec<Route>,
    configs: Vec<PortConfig>,
    initial_configs: Vec<PortConfig>,
    patches: Vec<Patch>,
    next_port_id: i32,
    next_patch_id: i32,
    /// Open streams by mix port config id.
    streams: HashMap<i32, OpenStream>,
    master_mute: bool,
    master_volume: f32,
    mic_mute: bool,
}

/// The module once the host's devices are known.
struct Ready {
    state: Mutex<State>,
    devices: Devices,
}

/// Registered at once; the first call that needs the ports asks the host
/// for its devices (which answers within a few seconds, with a null output
/// when CoreAudio does not).
pub struct Module {
    ready: Arc<OnceLock<Ready>>,
}

fn illegal_argument<T>(why: std::fmt::Arguments) -> binder::Result<T> {
    log::error!("{why}");
    exception(ExceptionCode::ILLEGAL_ARGUMENT)
}

fn illegal_state<T>(why: std::fmt::Arguments) -> binder::Result<T> {
    log::error!("{why}");
    exception(ExceptionCode::ILLEGAL_STATE)
}

impl State {
    fn port(&self, id: i32) -> Option<&Port> {
        self.ports.iter().find(|p| p.id == id)
    }

    fn config(&self, id: i32) -> Option<&PortConfig> {
        self.configs.iter().find(|c| c.id == id)
    }

    fn open_stream(&self, config_id: i32) -> bool {
        self.streams
            .get(&config_id)
            .and_then(|s| s.shared.upgrade())
            .is_some_and(|s| !s.is_closed())
    }

    fn open_streams_on_port(&self, port_id: i32) -> usize {
        self.streams
            .iter()
            .filter(|(id, s)| s.port_id == port_id && self.open_stream(**id))
            .count()
    }

    /// Whether a port config (or its port) takes part in a patch.
    fn patched(&self, config_id: i32) -> bool {
        self.patches.iter().any(|p| p.uses(config_id))
    }

    /// Tell each open stream whether a patch connects it to a device.
    fn update_connections(&mut self) {
        self.streams.retain(|_, s| s.shared.strong_count() > 0);
        for (&id, s) in &self.streams {
            if let Some(shared) = s.shared.upgrade() {
                let connected = self.patches.iter().any(|p| p.uses(id));
                shared
                    .connected
                    .store(connected, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }
}

impl Ready {
    fn new() -> Self {
        let devices = crate::host_devices();
        let c = config::primary(&devices);
        Self {
            state: Mutex::new(State {
                ports: c.ports,
                routes: c.routes,
                configs: c.initial_configs.clone(),
                initial_configs: c.initial_configs,
                patches: Vec::new(),
                next_port_id: c.next_id,
                next_patch_id: 1,
                streams: HashMap::new(),
                master_mute: false,
                master_volume: 1.0,
                mic_mute: false,
            }),
            devices,
        }
    }
}

impl Module {
    /// The devices are asked for on a thread of its own at once, so that
    /// they are usually known by the first call.
    pub fn new() -> Self {
        let ready = Arc::new(OnceLock::new());
        let early = ready.clone();
        std::thread::spawn(move || {
            early.get_or_init(Ready::new);
        });
        Self { ready }
    }

    fn ready(&self) -> &Ready {
        self.ready.get_or_init(Ready::new)
    }

    fn devices(&self) -> &Devices {
        &self.ready().devices
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.ready().state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The nominal latency of a stream: one device buffer, which is the
    /// period the host ring is drained or filled at.
    fn nominal_latency_ms(&self, input: bool) -> i32 {
        let d = if input {
            self.devices().input
        } else {
            self.devices().output
        };
        (d.buffer_frames as u64 * 1000).div_ceil(d.sample_rate.max(1) as u64) as i32
    }

    /// `calculateBufferSizeFramesForPcm`: the latency in frames, rounded up
    /// to the 16-frame mixer burst.
    fn minimum_buffer_frames(latency_ms: i32, rate: i32) -> i32 {
        let frames = (latency_ms as i64 * rate as i64 / 1000) as i32;
        (frames + 15) & !15
    }

    /// Checks shared by both stream kinds, then the stream itself.
    fn open_stream(
        &self,
        config_id: i32,
        buffer_frames: i64,
        input: bool,
    ) -> binder::Result<(Arc<Core>, StreamDescriptor)> {
        let mut state = self.lock();
        let Some(config) = state.config(config_id).cloned() else {
            return illegal_argument(format_args!("port config {config_id} not found"));
        };
        let Some(port) = state.port(config.port_id).cloned() else {
            return illegal_argument(format_args!("port {} not found", config.port_id));
        };
        if state.open_stream(config_id) {
            return illegal_state(format_args!("port config {config_id} already has a stream"));
        }
        let PortExt::Mix { max_open, .. } = port.ext else {
            return illegal_argument(format_args!("port config {config_id} is not a mix port's"));
        };
        if port.flags.is_input() != input {
            return illegal_argument(format_args!(
                "port config {config_id} is not an {} mix port's",
                if input { "input" } else { "output" }
            ));
        }
        if max_open != 0 && state.open_streams_on_port(port.id) >= max_open as usize {
            return illegal_state(format_args!(
                "port {} has its maximum of open streams",
                port.id
            ));
        }
        let (Some(format), Some(channels), Some(rate)) =
            (config.format.as_ref(), config.channels, config.rate)
        else {
            return illegal_argument(format_args!(
                "port config {config_id} is not fully specified"
            ));
        };
        let (Some(sample_bytes), true) = (format.sample_bytes(), channels.count() > 0) else {
            return illegal_argument(format_args!("port config {config_id}: unsupported format"));
        };
        let latency = self.nominal_latency_ms(input);
        let minimum = Self::minimum_buffer_frames(latency, rate);
        if buffer_frames < minimum as i64 {
            return illegal_argument(format_args!(
                "buffer of {buffer_frames} frames, must be at least {minimum}"
            ));
        }
        let frame_bytes = sample_bytes * channels.count() as usize;
        if buffer_frames > MAX_BUFFER_BYTES / frame_bytes as i64 {
            return illegal_argument(format_args!(
                "buffer of {buffer_frames} frames is too large"
            ));
        }
        let device_buffer = if input {
            self.devices().input.buffer_frames
        } else {
            self.devices().output.buffer_frames
        };
        // Output: one stream buffer plus one device buffer, so a write
        // lands while the callback drains the previous one (measured: no
        // underruns, about 18 ms from write to callback); input: room for a
        // few reads.
        let ring_frames = if input {
            4 * buffer_frames as u32 + device_buffer
        } else {
            buffer_frames as u32 + device_buffer
        };
        let params = Params {
            input,
            host_format: if format.pcm == PcmType::FLOAT_32_BIT.0 {
                FORMAT_FLOAT
            } else {
                FORMAT_PCM_16
            },
            rate: rate as u32,
            channels: channels.count(),
            frame_bytes,
            buffer_frames: buffer_frames as usize,
            nominal_latency_ms: latency,
            ring_frames,
        };
        let connected = state.patched(config_id);
        let (core, desc) = Core::open(params, connected).map_err(|e| {
            log::error!("cannot open a stream on port config {config_id}: {e}");
            binder::Status::new_exception(ExceptionCode::ILLEGAL_STATE, None)
        })?;
        log::info!("opened {params:?} on port config {config_id}, connected {connected}");
        state.streams.insert(
            config_id,
            OpenStream {
                port_id: port.id,
                shared: Arc::downgrade(&core.shared),
            },
        );
        Ok((core, desc))
    }
}

impl Interface for Module {}

impl IModule for Module {
    fn setModuleDebug(&self, debug: &ModuleDebug) -> binder::Result<()> {
        if debug.streamTransientStateDelayMs < 0 {
            return exception(ExceptionCode::ILLEGAL_ARGUMENT);
        }
        // There are no external devices to simulate, and transient states
        // complete on the next command.
        if debug.simulateDeviceConnections {
            return exception(ExceptionCode::ILLEGAL_STATE);
        }
        Ok(())
    }

    fn getTelephony(&self) -> binder::Result<Option<Strong<dyn ITelephony>>> {
        Ok(None)
    }

    fn getBluetooth(&self) -> binder::Result<Option<Strong<dyn IBluetooth>>> {
        Ok(None)
    }

    fn getBluetoothA2dp(&self) -> binder::Result<Option<Strong<dyn IBluetoothA2dp>>> {
        Ok(None)
    }

    fn getBluetoothLe(&self) -> binder::Result<Option<Strong<dyn IBluetoothLe>>> {
        Ok(None)
    }

    /// Every device port is attached; there is no template to connect.
    fn connectExternalDevice(&self, template: &AudioPort) -> binder::Result<AudioPort> {
        let state = self.lock();
        match state.port(template.id) {
            None => illegal_argument(format_args!("port {} not found", template.id)),
            Some(p) if p.is_mix() => {
                illegal_argument(format_args!("port {} is not a device port", template.id))
            }
            Some(_) => {
                illegal_argument(format_args!("port {} is permanently attached", template.id))
            }
        }
    }

    fn disconnectExternalDevice(&self, port_id: i32) -> binder::Result<()> {
        illegal_argument(format_args!(
            "port {port_id} is not a connected device port"
        ))
    }

    fn getAudioPatches(&self) -> binder::Result<Vec<AudioPatch>> {
        Ok(self.lock().patches.iter().map(Patch::to_aidl).collect())
    }

    fn getAudioPort(&self, port_id: i32) -> binder::Result<AudioPort> {
        match self.lock().port(port_id) {
            Some(p) => Ok(p.to_aidl()),
            None => illegal_argument(format_args!("port {port_id} not found")),
        }
    }

    fn getAudioPortConfigs(&self) -> binder::Result<Vec<AudioPortConfig>> {
        Ok(self
            .lock()
            .configs
            .iter()
            .map(PortConfig::to_aidl)
            .collect())
    }

    fn getAudioPorts(&self) -> binder::Result<Vec<AudioPort>> {
        Ok(self.lock().ports.iter().map(Port::to_aidl).collect())
    }

    fn getAudioRoutes(&self) -> binder::Result<Vec<AudioRoute>> {
        Ok(self.lock().routes.iter().map(route_aidl).collect())
    }

    fn getAudioRoutesForAudioPort(&self, port_id: i32) -> binder::Result<Vec<AudioRoute>> {
        let state = self.lock();
        if state.port(port_id).is_none() {
            return illegal_argument(format_args!("port {port_id} not found"));
        }
        Ok(state
            .routes
            .iter()
            .filter(|r| r.sink == port_id || r.sources.contains(&port_id))
            .map(route_aidl)
            .collect())
    }

    fn openInputStream(
        &self,
        args: &OpenInputStreamArguments,
    ) -> binder::Result<OpenInputStreamReturn> {
        let (core, desc) = self.open_stream(args.portConfigId, args.bufferSizeFrames, true)?;
        Ok(OpenInputStreamReturn {
            stream: Some(BnStreamIn::new_binder(
                StreamIn::new(core),
                BinderFeatures::default(),
            )),
            desc,
        })
    }

    fn openOutputStream(
        &self,
        args: &OpenOutputStreamArguments,
    ) -> binder::Result<OpenOutputStreamReturn> {
        let (core, desc) = self.open_stream(args.portConfigId, args.bufferSizeFrames, false)?;
        Ok(OpenOutputStreamReturn {
            stream: Some(BnStreamOut::new_binder(
                StreamOut::new(core),
                BinderFeatures::default(),
            )),
            desc,
        })
    }

    fn getSupportedPlaybackRateFactors(&self) -> binder::Result<SupportedPlaybackRateFactors> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn setAudioPatch(&self, requested: &AudioPatch) -> binder::Result<AudioPatch> {
        let mut state = self.lock();
        let (sources, sinks) = (&requested.sourcePortConfigIds, &requested.sinkPortConfigIds);
        if sources.is_empty() || sinks.is_empty() || !unique(sources) || !unique(sinks) {
            return illegal_argument(format_args!("bad patch endpoints {sources:?} -> {sinks:?}"));
        }
        let lookup = |ids: &[i32]| -> Option<Vec<PortConfig>> {
            ids.iter().map(|id| state.config(*id).cloned()).collect()
        };
        let (Some(source_configs), Some(sink_configs)) = (lookup(sources), lookup(sinks)) else {
            return illegal_argument(format_args!(
                "unknown patch endpoints {sources:?} -> {sinks:?}"
            ));
        };
        // Sinks reachable from the sources, and whether a non-exclusive
        // route reaches each.
        let mut allowed: HashMap<i32, bool> = HashMap::new();
        for src in &source_configs {
            for r in state
                .routes
                .iter()
                .filter(|r| r.sources.contains(&src.port_id))
            {
                let shared = allowed.entry(r.sink).or_insert(false);
                *shared |= !r.exclusive;
            }
        }
        if let Some(sink) = sink_configs
            .iter()
            .find(|s| !allowed.contains_key(&s.port_id))
        {
            return illegal_argument(format_args!("no route to sink port {}", sink.port_id));
        }
        let existing = match requested.id {
            0 => None,
            id => match state.patches.iter().position(|p| p.id == id) {
                Some(i) => Some(i),
                None => return illegal_argument(format_args!("patch {id} not found")),
            },
        };
        for (&sink_port, &shared) in &allowed {
            let in_use = state.patches.iter().enumerate().any(|(i, p)| {
                Some(i) != existing
                    && p.sinks
                        .iter()
                        .any(|c| state.config(*c).is_some_and(|c| c.port_id == sink_port))
            });
            if !shared && in_use {
                return illegal_state(format_args!("exclusive sink port {sink_port} is in use"));
            }
        }
        // The buffer size follows the highest-rate mix port config.
        let mixes = if state
            .port(source_configs[0].port_id)
            .is_some_and(Port::is_mix)
        {
            &source_configs
        } else {
            &sink_configs
        };
        let Some(mix) = mixes.iter().max_by_key(|c| c.rate.unwrap_or(0)) else {
            return illegal_state(format_args!("patch without a mix port"));
        };
        let input = state.port(mix.port_id).is_some_and(|p| p.flags.is_input());
        let latency = self.nominal_latency_ms(input);
        let rate = mix.rate.unwrap_or(0);
        if mix.format.as_ref().and_then(Format::sample_bytes).is_none() || rate <= 0 {
            return illegal_state(format_args!("patch on a mix config without a PCM format"));
        }
        let mut patch = Patch {
            id: requested.id,
            sources: sources.clone(),
            sinks: sinks.clone(),
            minimum_buffer_frames: Self::minimum_buffer_frames(latency, rate),
            latencies_ms: vec![latency; sinks.len()],
        };
        match existing {
            Some(i) => state.patches[i] = patch.clone(),
            None => {
                patch.id = state.next_patch_id;
                state.next_patch_id += 1;
                state.patches.push(patch.clone());
            }
        }
        state.update_connections();
        log::info!("patch {patch:?}");
        Ok(patch.to_aidl())
    }

    fn setAudioPortConfig(
        &self,
        requested: &AudioPortConfig,
        suggested: &mut AudioPortConfig,
    ) -> binder::Result<bool> {
        let Some((requested, gain)) = PortConfig::from_aidl(requested) else {
            return illegal_argument(format_args!("port config with a session ext"));
        };
        let mut state = self.lock();
        let existing = match requested.id {
            0 => None,
            id => match state.configs.iter().position(|c| c.id == id) {
                Some(i) => Some(i),
                None => return illegal_argument(format_args!("port config {id} not found")),
            },
        };
        let port_id = existing.map_or(requested.port_id, |i| state.configs[i].port_id);
        let Some(port) = state.port(port_id).cloned() else {
            return illegal_argument(format_args!("port {port_id} not found"));
        };
        // No port has gains.
        if gain {
            return illegal_argument(format_args!("port {port_id} has no gains"));
        }
        let mut out = match existing {
            Some(i) => state.configs[i].clone(),
            None => match default_config(&port) {
                Some(c) => c,
                None => {
                    return illegal_argument(format_args!(
                        "port {port_id} only has dynamic profiles"
                    ));
                }
            },
        };
        let dynamic = !port.is_mix() && port.is_dynamic();
        let (mut valid, mut complete) = (true, true);
        match requested.flags {
            Some(f) if f != port.flags => valid = false,
            Some(_) => {}
            None => complete = false,
        }
        match &requested.format {
            Some(f) if (*f == Format::dynamic() && dynamic) || port.profile(f).is_some() => {
                out.format = Some(f.clone())
            }
            Some(_) => valid = false,
            None => complete = false,
        }
        let format = out.format.clone().unwrap_or_else(Format::dynamic);
        let profile = port.profile(&format);
        if !(format == Format::dynamic() && dynamic) && profile.is_none() {
            return illegal_argument(format_args!("port {port_id} does not support {format:?}"));
        }
        match requested.channels {
            Some(c)
                if (c == Channels::None(0) && dynamic)
                    || profile.is_some_and(|p| p.channels.contains(&c)) =>
            {
                out.channels = Some(c)
            }
            Some(_) => valid = false,
            None => complete = false,
        }
        match requested.rate {
            Some(r) if (r == 0 && dynamic) || profile.is_some_and(|p| p.rates.contains(&r)) => {
                out.rate = Some(r)
            }
            Some(_) => valid = false,
            None => complete = false,
        }
        match (&requested.ext, &mut out.ext) {
            (ConfigExt::Unspecified, _) => {}
            (
                ConfigExt::Mix { handle, usecase },
                ConfigExt::Mix {
                    handle: h,
                    usecase: u,
                },
            ) => {
                *h = *handle;
                *u = usecase.clone();
            }
            (ConfigExt::Device(_), ConfigExt::Device(_)) => {}
            _ => valid = false,
        }
        let applied = match existing {
            None if valid && complete => {
                out.id = state.next_port_id;
                state.next_port_id += 1;
                state.configs.push(out.clone());
                true
            }
            Some(i) if valid => {
                state.configs[i] = out.clone();
                true
            }
            _ => false,
        };
        *suggested = out.to_aidl();
        Ok(applied)
    }

    fn resetAudioPatch(&self, patch_id: i32) -> binder::Result<()> {
        let mut state = self.lock();
        let Some(i) = state.patches.iter().position(|p| p.id == patch_id) else {
            return illegal_argument(format_args!("patch {patch_id} not found"));
        };
        state.patches.remove(i);
        state.update_connections();
        Ok(())
    }

    fn resetAudioPortConfig(&self, config_id: i32) -> binder::Result<()> {
        let mut state = self.lock();
        let Some(i) = state.configs.iter().position(|c| c.id == config_id) else {
            return illegal_argument(format_args!("port config {config_id} not found"));
        };
        if state.open_stream(config_id) {
            return illegal_state(format_args!("port config {config_id} has an open stream"));
        }
        if state.patched(config_id) {
            return illegal_state(format_args!("port config {config_id} is in a patch"));
        }
        match state
            .initial_configs
            .iter()
            .find(|c| c.id == config_id)
            .cloned()
        {
            Some(initial) => state.configs[i] = initial,
            None => {
                state.configs.remove(i);
            }
        }
        Ok(())
    }

    fn getMasterMute(&self) -> binder::Result<bool> {
        Ok(self.lock().master_mute)
    }

    fn setMasterMute(&self, mute: bool) -> binder::Result<()> {
        self.lock().master_mute = mute;
        Ok(())
    }

    fn getMasterVolume(&self) -> binder::Result<f32> {
        Ok(self.lock().master_volume)
    }

    fn setMasterVolume(&self, volume: f32) -> binder::Result<()> {
        if !(0.0..=1.0).contains(&volume) {
            return illegal_argument(format_args!("master volume {volume}"));
        }
        self.lock().master_volume = volume;
        Ok(())
    }

    fn getMicMute(&self) -> binder::Result<bool> {
        Ok(self.lock().mic_mute)
    }

    fn setMicMute(&self, mute: bool) -> binder::Result<()> {
        self.lock().mic_mute = mute;
        Ok(())
    }

    fn getMicrophones(&self) -> binder::Result<Vec<MicrophoneInfo>> {
        let state = self.lock();
        Ok(state
            .ports
            .iter()
            .filter(|p| p.flags.is_input() && !p.is_mix())
            .map(|p| {
                let port = p.to_aidl();
                let device = match port.ext {
                    AudioPortExt::Device(d) => d.device,
                    _ => Default::default(),
                };
                MicrophoneInfo {
                    id: p.name.clone(),
                    device,
                    group: 0,
                    indexInTheGroup: 0,
                    ..Default::default()
                }
            })
            .collect())
    }

    fn updateAudioMode(&self, mode: AudioMode) -> binder::Result<()> {
        if mode.0 < AudioMode::NORMAL.0 || mode.0 > AudioMode::CALL_SCREEN.0 {
            return illegal_argument(format_args!("audio mode {mode:?}"));
        }
        Ok(())
    }

    fn updateScreenRotation(&self, _rotation: ScreenRotation) -> binder::Result<()> {
        Ok(())
    }

    fn updateScreenState(&self, _on: bool) -> binder::Result<()> {
        Ok(())
    }

    /// No sound dose computation (it is optional).
    fn getSoundDose(&self) -> binder::Result<Option<Strong<dyn ISoundDose>>> {
        Ok(None)
    }

    fn generateHwAvSyncId(&self) -> binder::Result<i32> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn getVendorParameters(&self, ids: &[String]) -> binder::Result<Vec<VendorParameter>> {
        if ids.is_empty() {
            Ok(Vec::new())
        } else {
            exception(ExceptionCode::ILLEGAL_ARGUMENT)
        }
    }

    fn setVendorParameters(&self, params: &[VendorParameter], _async: bool) -> binder::Result<()> {
        if params.is_empty() {
            Ok(())
        } else {
            exception(ExceptionCode::ILLEGAL_ARGUMENT)
        }
    }

    fn addDeviceEffect(&self, _config: i32, _effect: &Strong<dyn IEffect>) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn removeDeviceEffect(
        &self,
        _config: i32,
        _effect: &Strong<dyn IEffect>,
    ) -> binder::Result<()> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    /// No port has MMAP_NOIRQ: AAudio takes its legacy (AudioTrack) path.
    fn getMmapPolicyInfos(
        &self,
        _kind: AudioMMapPolicyType,
    ) -> binder::Result<Vec<AudioMMapPolicyInfo>> {
        Ok(vec![AudioMMapPolicyInfo {
            mmapPolicy: AudioMMapPolicy::NEVER,
            ..Default::default()
        }])
    }

    fn supportsVariableLatency(&self) -> binder::Result<bool> {
        Ok(false)
    }

    fn getAAudioMixerBurstCount(&self) -> binder::Result<i32> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn getAAudioHardwareBurstMinUsec(&self) -> binder::Result<i32> {
        exception(ExceptionCode::UNSUPPORTED_OPERATION)
    }

    fn prepareToDisconnectExternalDevice(&self, port_id: i32) -> binder::Result<()> {
        illegal_argument(format_args!(
            "port {port_id} is not a connected device port"
        ))
    }
}

fn route_aidl(r: &Route) -> AudioRoute {
    AudioRoute {
        sourcePortIds: r.sources.clone(),
        sinkPortId: r.sink,
        isExclusive: r.exclusive,
    }
}

fn unique(ids: &[i32]) -> bool {
    ids.iter().enumerate().all(|(i, id)| !ids[..i].contains(id))
}

/// `generateDefaultPortConfig`: the first static profile; device ports
/// (dynamic) get a dynamic config.
fn default_config(port: &Port) -> Option<PortConfig> {
    let mut c = PortConfig {
        id: 0,
        port_id: port.id,
        rate: None,
        channels: None,
        format: None,
        flags: Some(port.flags),
        ext: PortConfig::ext_of(port),
    };
    if let Some(p) = port
        .profiles
        .iter()
        .find(|p| p.format != Format::dynamic() && !p.channels.is_empty() && !p.rates.is_empty())
    {
        c.format = Some(p.format.clone());
        c.channels = Some(p.channels[0]);
        c.rate = Some(p.rates[0]);
        return Some(c);
    }
    if port.is_mix() {
        return None;
    }
    c.format = Some(Format::dynamic());
    c.channels = Some(Channels::None(0));
    c.rate = Some(0);
    Some(c)
}
