//! `IComposerClient`: display 0, the macOS window, with one configuration
//! (the window's size, the screen's density and refresh period).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use aim_hostcall::display::mode;
use android_hardware_common::aidl::android::hardware::common::NativeHandle::NativeHandle;
use android_hardware_drm_common::aidl::android::hardware::drm::HdcpLevels::HdcpLevels;
use android_hardware_graphics_common::aidl::android::hardware::graphics::common::{
    Dataspace::Dataspace, DisplayDecorationSupport::DisplayDecorationSupport,
    DisplayHotplugEvent::DisplayHotplugEvent, Hdr::Hdr,
    HdrConversionCapability::HdrConversionCapability, HdrConversionStrategy::HdrConversionStrategy,
    PixelFormat::PixelFormat, Transform::Transform,
};
use android_hardware_graphics_composer3::aidl::android::hardware::graphics::composer3::{
    Buffer::Buffer,
    ChangedCompositionTypes::ChangedCompositionTypes,
    ClientTargetProperty::ClientTargetProperty,
    ClientTargetPropertyWithBrightness::ClientTargetPropertyWithBrightness,
    ClockMonotonicTimestamp::ClockMonotonicTimestamp,
    ColorMode::ColorMode,
    CommandError::CommandError,
    CommandResultPayload::CommandResultPayload,
    ContentType::ContentType,
    DimmingStage::DimmingStage,
    DisplayAttribute::DisplayAttribute,
    DisplayCapability::DisplayCapability,
    DisplayCommand::DisplayCommand,
    DisplayConfiguration::DisplayConfiguration,
    DisplayConfiguration::Dpi::Dpi,
    DisplayConnectionType::DisplayConnectionType,
    DisplayContentSample::DisplayContentSample,
    DisplayContentSamplingAttributes::DisplayContentSamplingAttributes,
    DisplayIdentification::DisplayIdentification,
    FormatColorComponent::FormatColorComponent,
    HdrCapabilities::HdrCapabilities,
    IComposerCallback::IComposerCallback,
    IComposerClient::{
        EX_BAD_CONFIG, EX_BAD_DISPLAY, EX_BAD_LAYER, EX_BAD_PARAMETER, EX_UNSUPPORTED,
        IComposerClient,
    },
    Luts::Luts,
    OutputType::OutputType,
    OverlayProperties::OverlayProperties,
    PerFrameMetadataKey::PerFrameMetadataKey,
    PowerMode::PowerMode,
    PresentFence::PresentFence,
    PresentOrValidate::PresentOrValidate,
    PresentOrValidate::Result::Result as PresentResult,
    ReadbackBufferAttributes::ReadbackBufferAttributes,
    ReleaseFences::{Layer::Layer as ReleaseFence, ReleaseFences},
    RenderIntent::RenderIntent,
    VirtualDisplay::VirtualDisplay,
    VsyncPeriodChangeConstraints::VsyncPeriodChangeConstraints,
    VsyncPeriodChangeTimeline::VsyncPeriodChangeTimeline,
};
use android_hardware_graphics_composer3::binder::{ParcelFileDescriptor, Status, Strong};

use crate::display::Display;
use crate::host::Host;

/// The window's display.
pub const DISPLAY: i64 = 0;
/// Its one configuration.
const CONFIG: i32 = 0;

fn error(code: i32) -> Status {
    Status::new_service_specific_error(code, None)
}

fn check_display(display: i64) -> binder::Result<()> {
    if display == DISPLAY {
        Ok(())
    } else {
        Err(error(EX_BAD_DISPLAY))
    }
}

/// What the composer and its clients share: the host connection, the
/// display, and SurfaceFlinger's callback.
pub struct Shared {
    pub host: Host,
    pub display: Mutex<Display>,
    pub callback: Mutex<Option<Strong<dyn IComposerCallback>>>,
    pub vsync: AtomicBool,
}

impl Shared {
    pub fn new(host: Host) -> Arc<Shared> {
        let display = Mutex::new(Display::new(host.info.mode == mode::WINDOWS));
        Arc::new(Shared {
            host,
            display,
            callback: Mutex::new(None),
            vsync: AtomicBool::new(false),
        })
    }

    /// Deliver the host's vsyncs while SurfaceFlinger wants them. Never
    /// returns while the display server runs.
    pub fn run_vsync(&self) {
        let mut stats = VsyncStats::default();
        self.host.run_events(|e| {
            if !self.vsync.load(Ordering::Relaxed) {
                return;
            }
            let callback = self.callback.lock().unwrap().clone();
            if let Some(cb) = callback {
                let _ = cb.onVsync(DISPLAY, e.timestamp_ns, e.period_ns as i32);
                stats.add(e.sent_ns, monotonic_ns());
            }
        });
    }

    fn period(&self) -> i32 {
        self.host.info.vsync_period_ns as i32
    }

    fn dpi(&self) -> (f32, f32) {
        (
            self.host.info.dpi_x_milli as f32 / 1000.0,
            self.host.info.dpi_y_milli as f32 / 1000.0,
        )
    }
}

fn monotonic_ns() -> i64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: fills the local timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec * 1_000_000_000 + ts.tv_nsec
}

/// How long vsyncs take from the display server to SurfaceFlinger, logged
/// every [`VsyncStats::EVERY`] vsyncs.
#[derive(Default)]
struct VsyncStats {
    n: u64,
    sum_ns: i64,
    max_ns: i64,
}

impl VsyncStats {
    const EVERY: u64 = 600;

    fn add(&mut self, sent_ns: i64, delivered_ns: i64) {
        let d = delivered_ns - sent_ns;
        self.n += 1;
        self.sum_ns += d;
        self.max_ns = self.max_ns.max(d);
        if self.n == Self::EVERY {
            log::info!(
                "vsync: {} delivered, server to onVsync returned {} us mean, {} us max",
                self.n,
                self.sum_ns / self.n as i64 / 1000,
                self.max_ns / 1000
            );
            *self = VsyncStats::default();
        }
    }
}

pub struct Client(pub Arc<Shared>);

impl binder::Interface for Client {}

impl Client {
    fn execute(&self, index: usize, cmd: &DisplayCommand, out: &mut Vec<CommandResultPayload>) {
        let fail = |out: &mut Vec<CommandResultPayload>, code| {
            out.push(CommandResultPayload::Error(CommandError {
                commandIndex: index as i32,
                errorCode: code,
            }))
        };
        if cmd.display != DISPLAY {
            return fail(out, EX_BAD_DISPLAY);
        }
        let host = &self.0.host;
        let mut d = self.0.display.lock().unwrap();
        for layer in &cmd.layers {
            if !d.has_layer(layer.layer) {
                fail(out, EX_BAD_LAYER);
                continue;
            }
            if let Err(code) = d.command(host, layer) {
                fail(out, code);
            }
            d.cursor.command(host, layer);
        }
        if let Some(m) = &cmd.colorTransformMatrix {
            d.set_color_transform(m);
        }
        if let Some(target) = &cmd.clientTarget {
            let Buffer {
                slot,
                handle,
                fence,
            } = &target.buffer;
            let fence = fence.as_ref().and_then(|f| f.as_ref().try_clone().ok());
            if let Err(code) = d.set_client_target(host, *slot, handle.as_ref(), fence) {
                fail(out, code);
            }
        }
        if cmd.validateDisplay || cmd.presentOrValidateDisplay {
            let layers = d.validate();
            if !layers.is_empty() {
                out.push(CommandResultPayload::ChangedCompositionTypes(
                    ChangedCompositionTypes {
                        display: DISPLAY,
                        layers,
                    },
                ));
            }
            out.push(CommandResultPayload::ClientTargetProperty(
                ClientTargetPropertyWithBrightness {
                    display: DISPLAY,
                    clientTargetProperty: ClientTargetProperty {
                        pixelFormat: PixelFormat::RGBA_8888,
                        dataspace: Dataspace::UNKNOWN,
                    },
                    brightness: 1.0,
                    dimmingStage: DimmingStage::NONE,
                },
            ));
            if cmd.presentOrValidateDisplay {
                out.push(CommandResultPayload::PresentOrValidateResult(
                    PresentOrValidate {
                        display: DISPLAY,
                        result: PresentResult::Validated,
                    },
                ));
            }
        }
        if cmd.acceptDisplayChanges {
            d.accept_changes();
        }
        if cmd.presentDisplay
            && let Some((fence, replaced)) = d.present(host)
        {
            // Each buffer the frame replaced is released once it is shown.
            let layers = replaced
                .into_iter()
                .filter_map(|layer| {
                    Some(ReleaseFence {
                        layer,
                        fence: Some(ParcelFileDescriptor::new(fence.try_clone().ok()?)),
                    })
                })
                .collect::<Vec<_>>();
            if !layers.is_empty() {
                out.push(CommandResultPayload::ReleaseFences(ReleaseFences {
                    display: DISPLAY,
                    layers,
                }));
            }
            out.push(CommandResultPayload::PresentFence(PresentFence {
                display: DISPLAY,
                fence: Some(ParcelFileDescriptor::new(fence)),
                layerPresentFences: None,
            }));
        }
    }
}

#[allow(non_snake_case)]
impl IComposerClient for Client {
    fn createLayer(&self, display: i64, _slots: i32) -> binder::Result<i64> {
        check_display(display)?;
        Ok(self.0.display.lock().unwrap().create_layer())
    }

    fn createVirtualDisplay(
        &self,
        _w: i32,
        _h: i32,
        _f: PixelFormat,
        _s: i32,
    ) -> binder::Result<VirtualDisplay> {
        Err(error(EX_UNSUPPORTED))
    }

    fn destroyLayer(&self, display: i64, layer: i64) -> binder::Result<()> {
        check_display(display)?;
        let mut d = self.0.display.lock().unwrap();
        d.cursor.destroyed(&self.0.host, layer);
        if d.destroy_layer(&self.0.host, layer) {
            Ok(())
        } else {
            Err(error(EX_BAD_LAYER))
        }
    }

    fn destroyVirtualDisplay(&self, _display: i64) -> binder::Result<()> {
        Err(error(EX_BAD_DISPLAY))
    }

    fn executeCommands(
        &self,
        commands: &[DisplayCommand],
    ) -> binder::Result<Vec<CommandResultPayload>> {
        let mut out = Vec::new();
        for (i, cmd) in commands.iter().enumerate() {
            self.execute(i, cmd, &mut out);
        }
        Ok(out)
    }

    fn getActiveConfig(&self, display: i64) -> binder::Result<i32> {
        check_display(display)?;
        Ok(CONFIG)
    }

    fn getColorModes(&self, display: i64) -> binder::Result<Vec<ColorMode>> {
        check_display(display)?;
        Ok(vec![ColorMode::NATIVE])
    }

    fn getDataspaceSaturationMatrix(&self, _dataspace: Dataspace) -> binder::Result<Vec<f32>> {
        let mut m = vec![0.0; 16];
        for i in 0..4 {
            m[i * 5] = 1.0;
        }
        Ok(m)
    }

    fn getDisplayAttribute(
        &self,
        display: i64,
        config: i32,
        attribute: DisplayAttribute,
    ) -> binder::Result<i32> {
        check_display(display)?;
        if config != CONFIG {
            return Err(error(EX_BAD_CONFIG));
        }
        let info = &self.0.host.info;
        Ok(match attribute {
            DisplayAttribute::WIDTH => info.width as i32,
            DisplayAttribute::HEIGHT => info.height as i32,
            DisplayAttribute::VSYNC_PERIOD => self.0.period(),
            DisplayAttribute::DPI_X => info.dpi_x_milli as i32,
            DisplayAttribute::DPI_Y => info.dpi_y_milli as i32,
            DisplayAttribute::CONFIG_GROUP => 0,
            _ => return Err(error(EX_BAD_PARAMETER)),
        })
    }

    fn getDisplayCapabilities(&self, display: i64) -> binder::Result<Vec<DisplayCapability>> {
        check_display(display)?;
        Ok(Vec::new())
    }

    fn getDisplayConfigs(&self, display: i64) -> binder::Result<Vec<i32>> {
        check_display(display)?;
        Ok(vec![CONFIG])
    }

    fn getDisplayConnectionType(&self, display: i64) -> binder::Result<DisplayConnectionType> {
        check_display(display)?;
        Ok(DisplayConnectionType::INTERNAL)
    }

    fn getDisplayIdentificationData(&self, _display: i64) -> binder::Result<DisplayIdentification> {
        Err(error(EX_UNSUPPORTED))
    }

    fn getDisplayName(&self, display: i64) -> binder::Result<String> {
        check_display(display)?;
        Ok("macOS window".into())
    }

    fn getDisplayVsyncPeriod(&self, display: i64) -> binder::Result<i32> {
        check_display(display)?;
        Ok(self.0.period())
    }

    fn getDisplayedContentSample(
        &self,
        _display: i64,
        _max_frames: i64,
        _timestamp: i64,
    ) -> binder::Result<DisplayContentSample> {
        Err(error(EX_UNSUPPORTED))
    }

    fn getDisplayedContentSamplingAttributes(
        &self,
        _display: i64,
    ) -> binder::Result<DisplayContentSamplingAttributes> {
        Err(error(EX_UNSUPPORTED))
    }

    fn getDisplayPhysicalOrientation(&self, display: i64) -> binder::Result<Transform> {
        check_display(display)?;
        Ok(Transform::NONE)
    }

    fn getHdrCapabilities(&self, display: i64) -> binder::Result<HdrCapabilities> {
        check_display(display)?;
        Ok(HdrCapabilities::default())
    }

    fn getMaxVirtualDisplayCount(&self) -> binder::Result<i32> {
        Ok(0)
    }

    fn getPerFrameMetadataKeys(&self, _display: i64) -> binder::Result<Vec<PerFrameMetadataKey>> {
        Err(error(EX_UNSUPPORTED))
    }

    fn getReadbackBufferAttributes(
        &self,
        _display: i64,
    ) -> binder::Result<ReadbackBufferAttributes> {
        Err(error(EX_UNSUPPORTED))
    }

    fn getReadbackBufferFence(
        &self,
        _display: i64,
    ) -> binder::Result<Option<ParcelFileDescriptor>> {
        Err(error(EX_UNSUPPORTED))
    }

    fn getRenderIntents(&self, display: i64, mode: ColorMode) -> binder::Result<Vec<RenderIntent>> {
        check_display(display)?;
        if mode != ColorMode::NATIVE {
            return Err(error(EX_BAD_PARAMETER));
        }
        Ok(vec![RenderIntent::COLORIMETRIC])
    }

    fn getSupportedContentTypes(&self, display: i64) -> binder::Result<Vec<ContentType>> {
        check_display(display)?;
        Ok(Vec::new())
    }

    fn getDisplayDecorationSupport(
        &self,
        display: i64,
    ) -> binder::Result<Option<DisplayDecorationSupport>> {
        check_display(display)?;
        Ok(None)
    }

    fn registerCallback(&self, callback: &Strong<dyn IComposerCallback>) -> binder::Result<()> {
        *self.0.callback.lock().unwrap() = Some(callback.clone());
        // SurfaceFlinger configures the displays hotplugged while it
        // registers, before registerCallback returns.
        if let Err(e) = callback.onHotplugEvent(DISPLAY, DisplayHotplugEvent::CONNECTED) {
            log::error!("hotplug of display {DISPLAY}: {e}");
        }
        Ok(())
    }

    fn setActiveConfig(&self, display: i64, config: i32) -> binder::Result<()> {
        check_display(display)?;
        if config == CONFIG {
            Ok(())
        } else {
            Err(error(EX_BAD_CONFIG))
        }
    }

    fn setActiveConfigWithConstraints(
        &self,
        display: i64,
        config: i32,
        _constraints: &VsyncPeriodChangeConstraints,
    ) -> binder::Result<VsyncPeriodChangeTimeline> {
        self.setActiveConfig(display, config)?;
        Ok(VsyncPeriodChangeTimeline {
            newVsyncAppliedTimeNanos: monotonic_ns(),
            refreshRequired: false,
            refreshTimeNanos: 0,
        })
    }

    fn setBootDisplayConfig(&self, _display: i64, _config: i32) -> binder::Result<()> {
        Err(error(EX_UNSUPPORTED))
    }

    fn clearBootDisplayConfig(&self, _display: i64) -> binder::Result<()> {
        Err(error(EX_UNSUPPORTED))
    }

    fn getPreferredBootDisplayConfig(&self, _display: i64) -> binder::Result<i32> {
        Err(error(EX_UNSUPPORTED))
    }

    fn setAutoLowLatencyMode(&self, _display: i64, _on: bool) -> binder::Result<()> {
        Err(error(EX_UNSUPPORTED))
    }

    fn setClientTargetSlotCount(&self, display: i64, count: i32) -> binder::Result<()> {
        check_display(display)?;
        let count = usize::try_from(count).map_err(|_| error(EX_BAD_PARAMETER))?;
        let mut d = self.0.display.lock().unwrap();
        d.set_slot_count(&self.0.host, count);
        Ok(())
    }

    fn setColorMode(
        &self,
        display: i64,
        mode: ColorMode,
        intent: RenderIntent,
    ) -> binder::Result<()> {
        check_display(display)?;
        if mode == ColorMode::NATIVE && intent == RenderIntent::COLORIMETRIC {
            Ok(())
        } else {
            Err(error(EX_UNSUPPORTED))
        }
    }

    fn setContentType(&self, _display: i64, _type: ContentType) -> binder::Result<()> {
        Err(error(EX_UNSUPPORTED))
    }

    fn setDisplayedContentSamplingEnabled(
        &self,
        _display: i64,
        _enable: bool,
        _mask: FormatColorComponent,
        _max_frames: i64,
    ) -> binder::Result<()> {
        Err(error(EX_UNSUPPORTED))
    }

    fn setPowerMode(&self, display: i64, mode: PowerMode) -> binder::Result<()> {
        check_display(display)?;
        match mode {
            PowerMode::ON | PowerMode::OFF => Ok(()),
            _ => Err(error(EX_UNSUPPORTED)),
        }
    }

    fn setReadbackBuffer(
        &self,
        _display: i64,
        _buffer: &NativeHandle,
        _fence: Option<&ParcelFileDescriptor>,
    ) -> binder::Result<()> {
        Err(error(EX_UNSUPPORTED))
    }

    fn setVsyncEnabled(&self, display: i64, enabled: bool) -> binder::Result<()> {
        check_display(display)?;
        self.0.vsync.store(enabled, Ordering::Relaxed);
        self.0.host.set_vsync(enabled).map_err(|e| {
            log::error!("vsync {enabled}: {e:?}");
            error(EX_BAD_DISPLAY)
        })
    }

    fn setIdleTimerEnabled(&self, display: i64, _timeout_ms: i32) -> binder::Result<()> {
        check_display(display)?;
        Ok(())
    }

    fn getOverlaySupport(&self) -> binder::Result<OverlayProperties> {
        Err(error(EX_UNSUPPORTED))
    }

    fn getHdrConversionCapabilities(&self) -> binder::Result<Vec<HdrConversionCapability>> {
        Err(error(EX_UNSUPPORTED))
    }

    fn setHdrConversionStrategy(&self, _strategy: &HdrConversionStrategy) -> binder::Result<Hdr> {
        Err(error(EX_UNSUPPORTED))
    }

    fn setRefreshRateChangedCallbackDebugEnabled(
        &self,
        _display: i64,
        _enabled: bool,
    ) -> binder::Result<()> {
        Err(error(EX_UNSUPPORTED))
    }

    fn getDisplayConfigurations(
        &self,
        display: i64,
        _max_frame_interval_ns: i32,
    ) -> binder::Result<Vec<DisplayConfiguration>> {
        check_display(display)?;
        let info = &self.0.host.info;
        let (x, y) = self.0.dpi();
        Ok(vec![DisplayConfiguration {
            configId: CONFIG,
            width: info.width as i32,
            height: info.height as i32,
            dpi: Some(Dpi { x, y }),
            configGroup: 0,
            vsyncPeriod: self.0.period(),
            vrrConfig: None,
            hdrOutputType: OutputType::SYSTEM,
        }])
    }

    fn notifyExpectedPresent(
        &self,
        _display: i64,
        _expected: &ClockMonotonicTimestamp,
        _frame_interval_ns: i32,
    ) -> binder::Result<()> {
        Ok(())
    }

    fn getMaxLayerPictureProfiles(&self, _display: i64) -> binder::Result<i32> {
        Err(error(EX_UNSUPPORTED))
    }

    fn startHdcpNegotiation(&self, _display: i64, _levels: &HdcpLevels) -> binder::Result<()> {
        Err(error(EX_UNSUPPORTED))
    }

    fn getLuts(&self, _display: i64, _buffers: &[Buffer]) -> binder::Result<Vec<Luts>> {
        Err(error(EX_UNSUPPORTED))
    }
}
