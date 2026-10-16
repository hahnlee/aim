//! The configuration an app's process starts with, as WindowManager
//! computes it for the default display (`DisplayContent
//! .computeScreenConfiguration`, `Configuration.reduceScreenLayout`,
//! `CompatibilityInfo.computeCompatibleScaling`) and ActivityTaskManager
//! sends it (`preBindApplication`), with the parcel forms of
//! `Configuration`, `WindowConfiguration` and `CompatibilityInfo` at
//! `android-16.0.0_r1`. A display without decor: no status or navigation
//! bar takes from the app's bounds.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;

use super::display::Display;
use crate::package::info::ApplicationInfo;

/// `Configuration`'s constants.
const TOUCHSCREEN_FINGER: i32 = 3;
const KEYBOARD_QWERTY: i32 = 2;
const KEYBOARDHIDDEN_NO: i32 = 1;
const HARDKEYBOARDHIDDEN_NO: i32 = 1;
const NAVIGATION_NONAV: i32 = 1;
const NAVIGATIONHIDDEN_YES: i32 = 2;
const ORIENTATION_PORTRAIT: i32 = 1;
const ORIENTATION_LANDSCAPE: i32 = 2;
const SCREENLAYOUT_SIZE_MASK: i32 = 0x0f;
const SCREENLAYOUT_SIZE_SMALL: i32 = 0x01;
const SCREENLAYOUT_SIZE_NORMAL: i32 = 0x02;
const SCREENLAYOUT_SIZE_LARGE: i32 = 0x03;
const SCREENLAYOUT_SIZE_XLARGE: i32 = 0x04;
const SCREENLAYOUT_LONG_MASK: i32 = 0x30;
const SCREENLAYOUT_LONG_NO: i32 = 0x10;
const SCREENLAYOUT_LONG_YES: i32 = 0x20;
const SCREENLAYOUT_LAYOUTDIR_LTR: i32 = 0x40;
const SCREENLAYOUT_LAYOUTDIR_RTL: i32 = 0x80;
const SCREENLAYOUT_ROUND_NO: i32 = 0x100;
const SCREENLAYOUT_COMPAT_NEEDED: i32 = 0x1000_0000;
const COLOR_MODE_WIDE_COLOR_GAMUT_NO: i32 = 0x1;
const COLOR_MODE_HDR_NO: i32 = 0x4;
const UI_MODE_TYPE_NORMAL: i32 = 0x01;
const UI_MODE_NIGHT_NO: i32 = 0x10;
const UI_MODE_NIGHT_YES: i32 = 0x20;
/// `WindowConfiguration`'s.
const WINDOWING_MODE_FULLSCREEN: i32 = 1;
const ACTIVITY_TYPE_UNDEFINED: i32 = 0;
const ALWAYS_ON_TOP_UNDEFINED: i32 = 0;
const ROTATION_0: i32 = 0;
const ROTATION_UNDEFINED: i32 = -1;
/// `CompatibilityInfo`'s.
const DEFAULT_NORMAL_SHORT_DIMENSION: f32 = 320.0;
const MAXIMUM_ASPECT_RATIO: f32 = 854.0 / 480.0;
const NEVER_NEEDS_COMPAT: i32 = 4;
const ALWAYS_NEEDS_COMPAT: i32 = 2;
const NEEDS_SCREEN_COMPAT: i32 = 8;
const NEEDS_COMPAT_RES: i32 = 16;
const SCALING_REQUIRED: i32 = 1;
/// `ApplicationInfo.FLAG_*` the compatibility decision reads.
const FLAG_SUPPORTS_LARGE_SCREENS: i32 = 1 << 11;
const FLAG_RESIZEABLE_FOR_SCREENS: i32 = 1 << 12;
const FLAG_SUPPORTS_SCREEN_DENSITIES: i32 = 1 << 13;
const FLAG_SUPPORTS_XLARGE_SCREENS: i32 = 1 << 19;
/// `Build.VERSION_CODES.O`.
const O: i32 = 26;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    fn write(&self, p: &mut Parcel) {
        for v in [self.left, self.top, self.right, self.bottom] {
            p.write_i32(v);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WindowConfiguration {
    pub bounds: Rect,
    pub app_bounds: Option<Rect>,
    pub max_bounds: Rect,
    pub windowing_mode: i32,
    pub activity_type: i32,
    pub always_on_top: i32,
    pub rotation: i32,
    pub display_rotation: i32,
}

impl WindowConfiguration {
    fn write(&self, p: &mut Parcel) {
        self.bounds.write(p);
        match &self.app_bounds {
            None => p.write_i32(0),
            Some(r) => {
                p.write_i32(1);
                r.write(p);
            }
        }
        self.max_bounds.write(p);
        for v in [
            self.windowing_mode,
            self.activity_type,
            self.always_on_top,
            self.rotation,
            self.display_rotation,
        ] {
            p.write_i32(v);
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Configuration {
    pub font_scale: f32,
    pub mcc: i32,
    pub mnc: i32,
    /// `LocaleList`'s language tags, comma-separated.
    pub locales: String,
    pub user_set_locale: bool,
    pub touchscreen: i32,
    pub keyboard: i32,
    pub keyboard_hidden: i32,
    pub hard_keyboard_hidden: i32,
    pub navigation: i32,
    pub navigation_hidden: i32,
    pub orientation: i32,
    pub screen_layout: i32,
    pub color_mode: i32,
    pub ui_mode: i32,
    pub screen_width_dp: i32,
    pub screen_height_dp: i32,
    pub smallest_screen_width_dp: i32,
    pub density_dpi: i32,
    pub compat_screen_width_dp: i32,
    pub compat_screen_height_dp: i32,
    pub compat_smallest_screen_width_dp: i32,
    pub window: WindowConfiguration,
    pub assets_seq: i32,
    pub seq: i32,
    pub font_weight_adjustment: i32,
    pub grammatical_gender: i32,
}

impl WriteParcelable for Configuration {
    fn write_to(&self, p: &mut Parcel) {
        p.write_f32(self.font_scale);
        p.write_i32(self.mcc);
        p.write_i32(self.mnc);
        // `writeTypedObject(mLocaleList)`: `LocaleList` writes its tags.
        p.write_i32(1);
        p.write_string8(Some(&self.locales));
        p.write_i32(i32::from(self.user_set_locale));
        for v in [
            self.touchscreen,
            self.keyboard,
            self.keyboard_hidden,
            self.hard_keyboard_hidden,
            self.navigation,
            self.navigation_hidden,
            self.orientation,
            self.screen_layout,
            self.color_mode,
            self.ui_mode,
            self.screen_width_dp,
            self.screen_height_dp,
            self.smallest_screen_width_dp,
            self.density_dpi,
            self.compat_screen_width_dp,
            self.compat_screen_height_dp,
            self.compat_smallest_screen_width_dp,
        ] {
            p.write_i32(v);
        }
        self.window.write(p);
        for v in [
            self.assets_seq,
            self.seq,
            self.font_weight_adjustment,
            self.grammatical_gender,
        ] {
            p.write_i32(v);
        }
    }
}

/// What the device's configuration depends on besides the display.
pub struct Device<'a> {
    /// The locales, as `persist.sys.locale` holds them (`ko-KR`).
    pub locales: &'a str,
    pub right_to_left: bool,
    pub night: bool,
    /// `Settings.System.FONT_SCALE`; `Settings.Secure.FONT_WEIGHT_ADJUSTMENT`.
    pub font_scale: f32,
    pub font_weight_adjustment: i32,
    pub seq: i32,
}

impl Configuration {
    /// The global configuration on `display`.
    pub fn of(display: &Display, device: &Device<'_>) -> Configuration {
        let dpi = display.density_dpi();
        let dp = |px: i32| px * 160 / dpi;
        let (width_dp, height_dp) = (dp(display.width), dp(display.height));
        let (short_dp, long_dp) = (width_dp.min(height_dp), width_dp.max(height_dp));
        let direction = if device.right_to_left {
            SCREENLAYOUT_LAYOUTDIR_RTL
        } else {
            SCREENLAYOUT_LAYOUTDIR_LTR
        };
        let screen_layout = reduce_screen_layout(
            reset_screen_layout(direction | SCREENLAYOUT_ROUND_NO),
            long_dp,
            short_dp,
        );
        let (compat_width, compat_height) =
            compatible_size(display.width, display.height, display.density);
        let bounds = Rect {
            left: 0,
            top: 0,
            right: display.width,
            bottom: display.height,
        };
        Configuration {
            font_scale: device.font_scale,
            mcc: 0,
            mnc: 0,
            locales: device.locales.to_string(),
            user_set_locale: true,
            touchscreen: TOUCHSCREEN_FINGER,
            keyboard: KEYBOARD_QWERTY,
            keyboard_hidden: KEYBOARDHIDDEN_NO,
            hard_keyboard_hidden: HARDKEYBOARDHIDDEN_NO,
            navigation: NAVIGATION_NONAV,
            navigation_hidden: NAVIGATIONHIDDEN_YES,
            orientation: if display.width > display.height {
                ORIENTATION_LANDSCAPE
            } else {
                ORIENTATION_PORTRAIT
            },
            screen_layout,
            color_mode: COLOR_MODE_WIDE_COLOR_GAMUT_NO | COLOR_MODE_HDR_NO,
            ui_mode: UI_MODE_TYPE_NORMAL
                | if device.night {
                    UI_MODE_NIGHT_YES
                } else {
                    UI_MODE_NIGHT_NO
                },
            screen_width_dp: width_dp,
            screen_height_dp: height_dp,
            smallest_screen_width_dp: short_dp,
            density_dpi: dpi,
            compat_screen_width_dp: (compat_width as f32 / display.density) as i32,
            compat_screen_height_dp: (compat_height as f32 / display.density) as i32,
            compat_smallest_screen_width_dp: (compat_width.min(compat_height) as f32
                / display.density) as i32,
            window: WindowConfiguration {
                bounds,
                app_bounds: Some(bounds),
                max_bounds: bounds,
                windowing_mode: WINDOWING_MODE_FULLSCREEN,
                activity_type: ACTIVITY_TYPE_UNDEFINED,
                always_on_top: ALWAYS_ON_TOP_UNDEFINED,
                rotation: ROTATION_0,
                display_rotation: ROTATION_0,
            },
            assets_seq: 0,
            seq: device.seq,
            font_weight_adjustment: device.font_weight_adjustment,
            grammatical_gender: 0,
        }
    }
}

/// `Configuration.resetScreenLayout`.
fn reset_screen_layout(layout: i32) -> i32 {
    (layout & !(SCREENLAYOUT_LONG_MASK | SCREENLAYOUT_SIZE_MASK | SCREENLAYOUT_COMPAT_NEEDED))
        | SCREENLAYOUT_LONG_YES
        | SCREENLAYOUT_SIZE_XLARGE
}

/// `Configuration.reduceScreenLayout`.
fn reduce_screen_layout(mut layout: i32, long_dp: i32, short_dp: i32) -> i32 {
    let (size, long, compat_needed) = if long_dp < 470 {
        (SCREENLAYOUT_SIZE_SMALL, false, false)
    } else {
        let size = if long_dp >= 960 && short_dp >= 720 {
            SCREENLAYOUT_SIZE_XLARGE
        } else if long_dp >= 640 && short_dp >= 480 {
            SCREENLAYOUT_SIZE_LARGE
        } else {
            SCREENLAYOUT_SIZE_NORMAL
        };
        (
            size,
            (long_dp * 3) / 5 >= short_dp - 1,
            short_dp > 321 || long_dp > 570,
        )
    };
    if !long {
        layout = (layout & !SCREENLAYOUT_LONG_MASK) | SCREENLAYOUT_LONG_NO;
    }
    if compat_needed {
        layout |= SCREENLAYOUT_COMPAT_NEEDED;
    }
    if size < layout & SCREENLAYOUT_SIZE_MASK {
        layout = (layout & !SCREENLAYOUT_SIZE_MASK) | size;
    }
    layout
}

/// `CompatibilityInfo.computeCompatibleScaling`'s compatible size.
fn compatible_size(width: i32, height: i32, density: f32) -> (i32, i32) {
    let (short, long) = (width.min(height), width.max(height));
    let new_short = (DEFAULT_NORMAL_SHORT_DIMENSION * density + 0.5) as i32;
    let aspect = (long as f32 / short as f32).min(MAXIMUM_ASPECT_RATIO);
    let new_long = (new_short as f32 * aspect + 0.5) as i32;
    if width < height {
        (new_short, new_long)
    } else {
        (new_long, new_short)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompatibilityInfo {
    pub flags: i32,
    pub application_density: i32,
    pub application_scale: f32,
    pub application_inverted_scale: f32,
    pub application_density_scale: f32,
    pub application_density_inverted_scale: f32,
    pub application_display_rotation: i32,
}

impl WriteParcelable for CompatibilityInfo {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.flags);
        p.write_i32(self.application_density);
        p.write_f32(self.application_scale);
        p.write_f32(self.application_inverted_scale);
        p.write_f32(self.application_density_scale);
        p.write_f32(self.application_density_inverted_scale);
        p.write_i32(self.application_display_rotation);
    }
}

impl CompatibilityInfo {
    /// `new CompatibilityInfo(appInfo, screenLayout, sw, forceCompat=false,
    /// scaleFactor=1, densityScaleFactor=1)` on a display of
    /// `density_dpi`.
    pub fn of(app: &ApplicationInfo, config: &Configuration) -> CompatibilityInfo {
        let screen_layout = config.screen_layout;
        let sw = config.smallest_screen_width_dp;
        let density = config.density_dpi;
        let mut flags = if app.target_sdk_version < O {
            NEEDS_COMPAT_RES
        } else {
            0
        };
        let mut info = CompatibilityInfo {
            flags: 0,
            application_density: density,
            application_scale: 1.0,
            application_inverted_scale: 1.0,
            application_density_scale: 1.0,
            application_density_inverted_scale: 1.0,
            application_display_rotation: ROTATION_UNDEFINED,
        };
        if app.requires_smallest_width_dp != 0
            || app.compatible_width_limit_dp != 0
            || app.largest_width_limit_dp != 0
        {
            let mut required = if app.requires_smallest_width_dp != 0 {
                app.requires_smallest_width_dp
            } else {
                app.compatible_width_limit_dp
            };
            if required == 0 {
                required = app.largest_width_limit_dp;
            }
            let compat = if app.compatible_width_limit_dp != 0 {
                app.compatible_width_limit_dp
            } else {
                required
            }
            .max(required);
            let largest = app.largest_width_limit_dp;
            if required as f32 > DEFAULT_NORMAL_SHORT_DIMENSION {
                flags |= NEVER_NEEDS_COMPAT;
            } else if largest != 0 && sw > largest {
                flags |= NEEDS_SCREEN_COMPAT | ALWAYS_NEEDS_COMPAT;
            } else if compat >= sw {
                flags |= NEVER_NEEDS_COMPAT;
            }
        } else {
            const EXPANDABLE: i32 = 2;
            const LARGE_SCREENS: i32 = 8;
            const XLARGE_SCREENS: i32 = 32;
            let mut size_info = 0;
            let mut any_resizeable = false;
            if app.flags & FLAG_SUPPORTS_LARGE_SCREENS != 0 {
                size_info |= LARGE_SCREENS | XLARGE_SCREENS | EXPANDABLE;
                any_resizeable = true;
            }
            if app.flags & FLAG_SUPPORTS_XLARGE_SCREENS != 0 {
                any_resizeable = true;
                size_info |= XLARGE_SCREENS | EXPANDABLE;
            }
            if app.flags & FLAG_RESIZEABLE_FOR_SCREENS != 0 {
                any_resizeable = true;
                size_info |= EXPANDABLE;
            }
            flags |= NEEDS_SCREEN_COMPAT;
            match screen_layout & SCREENLAYOUT_SIZE_MASK {
                SCREENLAYOUT_SIZE_XLARGE => {
                    if size_info & XLARGE_SCREENS != 0 {
                        flags &= !NEEDS_SCREEN_COMPAT;
                    }
                    if app.flags & FLAG_SUPPORTS_XLARGE_SCREENS != 0 {
                        flags |= NEVER_NEEDS_COMPAT;
                    }
                }
                SCREENLAYOUT_SIZE_LARGE => {
                    if size_info & LARGE_SCREENS != 0 {
                        flags &= !NEEDS_SCREEN_COMPAT;
                    }
                    if app.flags & FLAG_SUPPORTS_LARGE_SCREENS != 0 {
                        flags |= NEVER_NEEDS_COMPAT;
                    }
                }
                _ => {}
            }
            if screen_layout & SCREENLAYOUT_COMPAT_NEEDED != 0 {
                if size_info & EXPANDABLE != 0 {
                    flags &= !NEEDS_SCREEN_COMPAT;
                } else if !any_resizeable {
                    flags |= ALWAYS_NEEDS_COMPAT;
                }
            } else {
                flags &= !NEEDS_SCREEN_COMPAT;
                flags |= NEVER_NEEDS_COMPAT;
            }
            if app.flags & FLAG_SUPPORTS_SCREEN_DENSITIES == 0 {
                // The app is scaled to the default density.
                const DENSITY_DEFAULT: i32 = 160;
                info.application_density = DENSITY_DEFAULT;
                info.application_scale = density as f32 / DENSITY_DEFAULT as f32;
                info.application_inverted_scale = 1.0 / info.application_scale;
                flags |= SCALING_REQUIRED;
            }
        }
        info.flags = flags;
        info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The original's configuration of a 1080x1920 display at 320 dpi
    /// (system_server's `bindApplication` to Calculator, captured on the
    /// pinned image).
    #[test]
    fn matches_the_original_on_a_phone_display() {
        let display = Display {
            id: 0,
            width: 1080,
            height: 1920,
            x_dpi: 320.0,
            y_dpi: 320.0,
            refresh_rate: 60.0,
            density: 2.0,
        };
        let device = Device {
            locales: "ko-KR",
            right_to_left: false,
            night: false,
            font_scale: 1.0,
            font_weight_adjustment: 0,
            seq: 13,
        };
        let c = Configuration::of(&display, &device);
        assert_eq!(c.screen_layout, 0x1000_0163);
        assert_eq!(
            (
                c.screen_width_dp,
                c.screen_height_dp,
                c.smallest_screen_width_dp
            ),
            (540, 960, 540)
        );
        assert_eq!(
            (
                c.compat_screen_width_dp,
                c.compat_screen_height_dp,
                c.compat_smallest_screen_width_dp
            ),
            (320, 569, 320)
        );
        assert_eq!((c.color_mode, c.ui_mode, c.density_dpi), (5, 0x11, 320));
        let mut p = Parcel::new();
        c.write_to(&mut p);
        // 3 header words, the locale (present, length, 8 bytes), 18 ints,
        // the window configuration (4 + 5 + 4 + 5 ints), 4 ints.
        assert_eq!(p.data().len(), 4 * 3 + 4 + 4 + 8 + 4 * 18 + 4 * 18 + 4 * 4);
        let app = ApplicationInfo {
            target_sdk_version: 35,
            flags: FLAG_SUPPORTS_LARGE_SCREENS
                | FLAG_RESIZEABLE_FOR_SCREENS
                | FLAG_SUPPORTS_SCREEN_DENSITIES
                | FLAG_SUPPORTS_XLARGE_SCREENS,
            ..Default::default()
        };
        let compat = CompatibilityInfo::of(&app, &c);
        assert_eq!(
            (
                compat.flags,
                compat.application_density,
                compat.application_display_rotation
            ),
            (4, 320, -1)
        );
    }
}
