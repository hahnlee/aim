//! `display`: DisplayManager for the app host's one display, the built-in
//! one SurfaceFlinger reports, as `LocalDisplayAdapter` and
//! `LogicalDisplay` describe it at `android-16.0.0_r1` (`DisplayInfo`'s
//! parcel form), with no decor taking from apps' area.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, Mutex};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, Binder, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_hardware_display_idisplaymanager as dm;

use super::display::Display;
use super::{AppHost, reply};

/// `Display.FLAG_*` of a built-in display.
const FLAG_SUPPORTS_PROTECTED_BUFFERS: i32 = 1 << 0;
const FLAG_SECURE: i32 = 1 << 1;
const FLAG_TRUSTED: i32 = 1 << 7;
const FLAG_ROTATES_WITH_CONTENT: i32 = 1 << 14;
/// `Display.TYPE_INTERNAL`, `Display.STATE_ON`, `ColorSpace.Named.SRGB`.
const TYPE_INTERNAL: i32 = 1;
const STATE_ON: i32 = 2;
const SRGB: i32 = 0;
/// `PowerManager.BRIGHTNESS_*`: the setting's integer range.
const BRIGHTNESS_ON: f32 = 255.0;
const BRIGHTNESS_OFF: f32 = 1.0;

/// What the display info reads besides SurfaceFlinger's display.
pub struct Resources {
    /// `display_manager_built_in_display_name`, localized.
    pub name: String,
    pub density_dpi: i32,
    /// The settings' brightness range and default, from
    /// `config_screenBrightnessSetting{Minimum,Maximum,Default}`, and
    /// `config_screenBrightnessDimFloat`.
    pub brightness: (i32, i32, i32),
    pub brightness_dim: f32,
}

/// `DisplayInfo` of the default display.
pub struct DisplayInfo<'a> {
    pub display: &'a Display,
    pub resources: &'a Resources,
}

/// `BrightnessSynchronizer.brightnessIntToFloat`.
fn brightness(value: i32) -> f32 {
    (value as f32 - BRIGHTNESS_OFF) / (BRIGHTNESS_ON - BRIGHTNESS_OFF)
}

impl WriteParcelable for DisplayInfo<'_> {
    fn write_to(&self, p: &mut Parcel) {
        let d = self.display;
        let mode = d.active();
        let (w, h) = (d.width, d.height);
        p.write_i32(0); // layer stack
        let secure = if d.secure {
            FLAG_SECURE | FLAG_SUPPORTS_PROTECTED_BUFFERS
        } else {
            0
        };
        p.write_i32(secure | FLAG_TRUSTED | FLAG_ROTATES_WITH_CONTENT);
        p.write_i32(TYPE_INTERNAL);
        p.write_i32(0); // display id
        p.write_i32(0); // display group
        // `writeParcelable(DisplayAddress.Physical)`.
        p.write_string16(Some("android.view.DisplayAddress$Physical"));
        p.write_i64(d.id);
        p.write_string16(None); // no device product info
        p.write_string8(Some(&self.resources.name));
        for v in [w, h, w.min(h), w.min(h), w.max(h), w.max(h), w, h] {
            p.write_i32(v); // app, smallest and largest nominal app, logical
        }
        p.write_i32(-1); // no cutout
        p.write_i32(0); // rotation
        p.write_i32(d.active_mode);
        p.write_f32(d.render_frame_rate);
        p.write_bool(d.has_arr_support);
        p.write_string16(Some("android.view.FrameRateCategoryRate"));
        p.write_f32(d.frame_rate_category_rate.0);
        p.write_f32(d.frame_rate_category_rate.1);
        p.write_i32(d.supported_refresh_rates.len() as i32);
        for r in &d.supported_refresh_rates {
            p.write_f32(*r);
        }
        p.write_i32(d.active_mode); // the default mode
        p.write_i32(-1); // no user-preferred mode
        for _ in 0..2 {
            // The supported modes, and those apps may use.
            p.write_i32(d.modes.len() as i32);
            for m in &d.modes {
                p.write_i32(m.id);
                p.write_i32(m.width);
                p.write_i32(m.height);
                p.write_f32(m.peak_refresh_rate);
                p.write_f32(m.vsync_rate);
                p.write_bool(false); // not synthetic
                p.write_i32(0); // no alternative refresh rates
                p.write_i32(m.supported_hdr_types.len() as i32);
                for t in &m.supported_hdr_types {
                    p.write_i32(*t);
                }
            }
        }
        p.write_i32(d.active_color_mode);
        p.write_i32(d.supported_color_modes.len() as i32);
        for c in &d.supported_color_modes {
            p.write_i32(*c);
        }
        p.write_string16(Some("android.view.Display$HdrCapabilities"));
        p.write_i32(d.hdr_types.len() as i32);
        for t in &d.hdr_types {
            p.write_i32(*t);
        }
        p.write_f32(d.hdr_luminance.0);
        p.write_f32(d.hdr_luminance.1);
        p.write_f32(d.hdr_luminance.2);
        p.write_bool(false); // not forced to SDR
        p.write_bool(false); // no minimal post-processing
        p.write_i32(self.resources.density_dpi);
        p.write_f32(mode.x_dpi);
        p.write_f32(mode.y_dpi);
        p.write_i64(mode.app_vsync_offset);
        p.write_i64(mode.presentation_deadline);
        p.write_i32(STATE_ON);
        p.write_i32(STATE_ON); // committed
        p.write_i32(0); // owner uid
        p.write_string8(None); // owner package
        p.write_string8(Some(&format!("local:{}", d.id)));
        p.write_i32(0); // remove mode
        // `DisplayManagerService.getDisplayInfoForFrameRateOverride`: the
        // render rate when no override applies.
        p.write_f32(d.render_frame_rate);
        let (min, max, default) = self.resources.brightness;
        p.write_f32(brightness(min));
        p.write_f32(brightness(max));
        p.write_f32(brightness(default));
        p.write_f32(self.resources.brightness_dim);
        p.write_i32(1);
        p.write_i32(0); // `RoundedCorners.NO_ROUNDED_CORNERS`
        p.write_i32(0); // no user-disabled HDR types
        p.write_i32(d.install_orientation);
        // `DisplayShape.createDefaultDisplayShape`: the display's rectangle.
        p.write_i32(1);
        p.write_string8(Some(&format!("M0,0 L{w},0 L{w},{h} L0,{h} Z")));
        p.write_i32(w);
        p.write_i32(h);
        p.write_f32(1.0); // physical pixel display size ratio
        p.write_i32(0); // rotation
        p.write_i32(0); // offset x
        p.write_i32(0); // offset y
        p.write_f32(1.0); // scale
        p.write_i32(0); // no layout-limited refresh rate
        p.write_f32(f32::NAN); // HDR/SDR ratio: not HDR
        p.write_i32(0); // no thermal refresh rate throttling
        p.write_string8(Some("default")); // thermal brightness throttling data
        p.write_bool(true); // hosts tasks
    }
}

/// `display`.
pub struct DisplayManager {
    host: Arc<AppHost>,
    callbacks: Mutex<Vec<(Binder, i64)>>,
}

impl DisplayManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        DisplayManager {
            host,
            callbacks: Mutex::new(Vec::new()),
        }
    }
}

impl Service for DisplayManager {
    fn descriptor(&self) -> &str {
        dm::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match call.code {
            dm::GET_DISPLAY_INFO => {
                let args = dm::GetDisplayInfo::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let display = match self.host.display() {
                    Ok(d) => d,
                    Err(e) => return reply(Err(e)),
                };
                let info = (args.display_id == 0).then(|| DisplayInfo {
                    display: &display.0,
                    resources: &display.1,
                });
                dm::write_get_display_info_reply(&mut p, info.as_ref());
            }
            dm::GET_DISPLAY_IDS => {
                dm::GetDisplayIds::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                dm::write_get_display_ids_reply(&mut p, &Some(vec![0]));
            }
            dm::REGISTER_CALLBACK_WITH_EVENT_MASK => {
                let args = dm::RegisterCallbackWithEventMask::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                // The display never changes, so no event is sent yet.
                if let Some(callback) = args.callback {
                    self.callbacks
                        .lock()
                        .unwrap()
                        .push((callback, args.events_mask));
                }
                dm::write_register_callback_with_event_mask_reply(&mut p);
            }
            dm::GET_PREFERRED_WIDE_GAMUT_COLOR_SPACE_ID => {
                dm::GetPreferredWideGamutColorSpaceId::read(&mut call.data)
                    .map_err(|_| BAD_VALUE)?;
                dm::write_get_preferred_wide_gamut_color_space_id_reply(&mut p, SRGB);
            }
            dm::GET_OVERLAY_SUPPORT => {
                dm::GetOverlaySupport::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let support = match Display::overlay_support(&self.host.process) {
                    Ok(support) => support,
                    Err(e) => {
                        return reply(Err(aim_binder_host::parcel::Exception::new(
                            aim_binder_host::parcel::EX_ILLEGAL_STATE,
                            e,
                        )));
                    }
                };
                p.write_no_exception();
                p.write_raw(&support, &[]);
            }
            code => return reply(Err(self.host.unimplemented("display", code))),
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The original's `getDisplayInfo(0)` reply to Calculator on the
    /// display server's 1080x1920 panel, ko-KR (captured on the pinned
    /// image).
    #[test]
    fn display_info_matches_the_original() {
        let display = Display::phone_for_test();
        let resources = Resources {
            name: "기본으로 제공되는 화면".into(),
            density_dpi: 320,
            brightness: (10, 255, 102),
            brightness_dim: 0.05,
        };
        let mut p = Parcel::new();
        dm::write_get_display_info_reply(
            &mut p,
            Some(&DisplayInfo {
                display: &display,
                resources: &resources,
            }),
        );
        assert_eq!(
            p.data(),
            include_bytes!("../../tests/fixtures/app_host/display-info.reply.bin")
        );
    }
}
