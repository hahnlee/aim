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
const SCREENLAYOUT_LAYOUTDIR_MASK: i32 = 0xc0;
const SCREENLAYOUT_ROUND_NO: i32 = 0x100;
const SCREENLAYOUT_COMPAT_NEEDED: i32 = 0x1000_0000;
const COLOR_MODE_WIDE_COLOR_GAMUT_NO: i32 = 0x1;
const COLOR_MODE_HDR_NO: i32 = 0x4;
const UI_MODE_TYPE_NORMAL: i32 = 0x01;
const UI_MODE_NIGHT_NO: i32 = 0x10;
const UI_MODE_NIGHT_YES: i32 = 0x20;
/// `WindowConfiguration`'s.
const WINDOWING_MODE_FULLSCREEN: i32 = 1;
const WINDOWING_MODE_FREEFORM: i32 = 5;
const ACTIVITY_TYPE_UNDEFINED: i32 = 0;
const ALWAYS_ON_TOP_UNDEFINED: i32 = 0;
const ROTATION_0: i32 = 0;
const ROTATION_UNDEFINED: i32 = -1;
/// `ActivityInfo.CONFIG_*`: what a configuration change changes.
pub const CONFIG_MCC: i32 = 0x1;
pub const CONFIG_MNC: i32 = 0x2;
pub const CONFIG_LOCALE: i32 = 0x4;
pub const CONFIG_TOUCHSCREEN: i32 = 0x8;
pub const CONFIG_KEYBOARD: i32 = 0x10;
pub const CONFIG_KEYBOARD_HIDDEN: i32 = 0x20;
pub const CONFIG_NAVIGATION: i32 = 0x40;
pub const CONFIG_ORIENTATION: i32 = 0x80;
pub const CONFIG_SCREEN_LAYOUT: i32 = 0x100;
pub const CONFIG_UI_MODE: i32 = 0x200;
pub const CONFIG_SCREEN_SIZE: i32 = 0x400;
pub const CONFIG_SMALLEST_SCREEN_SIZE: i32 = 0x800;
pub const CONFIG_DENSITY: i32 = 0x1000;
pub const CONFIG_LAYOUT_DIRECTION: i32 = 0x2000;
pub const CONFIG_COLOR_MODE: i32 = 0x4000;
pub const CONFIG_GRAMMATICAL_GENDER: i32 = 0x8000;
pub const CONFIG_RESOURCES_UNUSED: i32 = 0x800_0000;
pub const CONFIG_FONT_WEIGHT_ADJUSTMENT: i32 = 0x1000_0000;
pub const CONFIG_WINDOW_CONFIGURATION: i32 = 0x2000_0000;
pub const CONFIG_FONT_SCALE: i32 = 0x4000_0000;
pub const CONFIG_ASSETS_PATHS: i32 = 0x8000_0000_u32 as i32;
/// `Configuration.COLOR_MODE_*_MASK`, `SCREENLAYOUT_ROUND_MASK`.
const COLOR_MODE_WIDE_COLOR_GAMUT_MASK: i32 = 0x3;
const COLOR_MODE_HDR_MASK: i32 = 0xc;
const SCREENLAYOUT_ROUND_MASK: i32 = 0x300;
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

impl Configuration {
    /// The configuration of a freeform task of `bounds` on the display of
    /// this global configuration, merged into it, as `TaskFragment
    /// .computeConfigResourceOverrides` resolves a floating task's
    /// (its sizes from its bounds; no insets) and `Configuration
    /// .updateFrom` merges it.
    pub fn of_freeform_task(&self, bounds: Rect) -> Configuration {
        let density = self.density_dpi as f32 / 160.0;
        let dp = |px: i32| (px as f32 / density + 0.5) as i32;
        let (width_dp, height_dp) = (dp(bounds.width()).min(self.screen_width_dp), dp(bounds.height()).min(self.screen_height_dp));
        let mut task = self.clone();
        task.window.windowing_mode = WINDOWING_MODE_FREEFORM;
        task.window.bounds = bounds;
        task.window.app_bounds = Some(bounds);
        task.screen_width_dp = width_dp;
        task.screen_height_dp = height_dp;
        task.smallest_screen_width_dp = dp(bounds.width().min(bounds.height()));
        task.orientation = if width_dp <= height_dp { ORIENTATION_PORTRAIT } else { ORIENTATION_LANDSCAPE };
        // `WindowContainer.computeScreenLayout`, keeping the direction.
        let reduced = reduce_screen_layout(
            self.screen_layout & (SCREENLAYOUT_LONG_MASK | SCREENLAYOUT_SIZE_MASK),
            width_dp.max(height_dp),
            width_dp.min(height_dp),
        );
        task.screen_layout = (self.screen_layout & SCREENLAYOUT_LAYOUTDIR_MASK) | reduced;
        task
    }
}

impl Configuration {
    /// `Configuration.diff(newer)`: what changes from this configuration to
    /// `newer`, both whole (no field undefined).
    pub fn diff(&self, newer: &Configuration) -> i32 {
        let mut changed = 0;
        let mut differ = |a: i32, b: i32, what: i32| {
            if a != b {
                changed |= what;
            }
        };
        differ(self.font_scale.to_bits() as i32, newer.font_scale.to_bits() as i32, CONFIG_FONT_SCALE);
        differ(self.mcc, newer.mcc, CONFIG_MCC);
        differ(self.mnc, newer.mnc, CONFIG_MNC);
        differ(self.screen_layout & SCREENLAYOUT_LAYOUTDIR_MASK, newer.screen_layout & SCREENLAYOUT_LAYOUTDIR_MASK, CONFIG_LAYOUT_DIRECTION);
        differ(self.touchscreen, newer.touchscreen, CONFIG_TOUCHSCREEN);
        differ(self.keyboard, newer.keyboard, CONFIG_KEYBOARD);
        differ(self.keyboard_hidden, newer.keyboard_hidden, CONFIG_KEYBOARD_HIDDEN);
        differ(self.hard_keyboard_hidden, newer.hard_keyboard_hidden, CONFIG_KEYBOARD_HIDDEN);
        differ(self.navigation, newer.navigation, CONFIG_NAVIGATION);
        differ(self.navigation_hidden, newer.navigation_hidden, CONFIG_KEYBOARD_HIDDEN);
        differ(self.orientation, newer.orientation, CONFIG_ORIENTATION);
        differ(self.screen_layout & !SCREENLAYOUT_LAYOUTDIR_MASK, newer.screen_layout & !SCREENLAYOUT_LAYOUTDIR_MASK, CONFIG_SCREEN_LAYOUT);
        differ(self.color_mode & COLOR_MODE_HDR_MASK, newer.color_mode & COLOR_MODE_HDR_MASK, CONFIG_COLOR_MODE);
        differ(self.color_mode & COLOR_MODE_WIDE_COLOR_GAMUT_MASK, newer.color_mode & COLOR_MODE_WIDE_COLOR_GAMUT_MASK, CONFIG_COLOR_MODE);
        differ(self.ui_mode, newer.ui_mode, CONFIG_UI_MODE);
        differ(self.screen_width_dp, newer.screen_width_dp, CONFIG_SCREEN_SIZE);
        differ(self.screen_height_dp, newer.screen_height_dp, CONFIG_SCREEN_SIZE);
        differ(self.smallest_screen_width_dp, newer.smallest_screen_width_dp, CONFIG_SMALLEST_SCREEN_SIZE);
        differ(self.density_dpi, newer.density_dpi, CONFIG_DENSITY);
        differ(self.assets_seq, newer.assets_seq, CONFIG_ASSETS_PATHS);
        differ(i32::from(self.window != newer.window), 0, CONFIG_WINDOW_CONFIGURATION);
        differ(self.font_weight_adjustment, newer.font_weight_adjustment, CONFIG_FONT_WEIGHT_ADJUSTMENT);
        differ(self.grammatical_gender, newer.grammatical_gender, CONFIG_GRAMMATICAL_GENDER);
        if self.locales != newer.locales {
            changed |= CONFIG_LOCALE | CONFIG_LAYOUT_DIRECTION;
        }
        changed
    }

    /// `Configuration.isLayoutSizeAtLeast`.
    fn is_layout_size_at_least(&self, size: i32) -> bool {
        let current = self.screen_layout & SCREENLAYOUT_SIZE_MASK;
        current != 0 && current >= size
    }
}

/// `SizeConfigurationBuckets`: the size thresholds an activity's
/// resources change at, which it reports (`reportSizeConfigurations`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SizeBuckets {
    pub horizontal: Option<Vec<i32>>,
    pub vertical: Option<Vec<i32>>,
    pub smallest: Option<Vec<i32>>,
    pub screen_layout_size: Option<Vec<i32>>,
    pub screen_layout_long_set: bool,
}

impl aim_service_aidl::ReadParcelable for SizeBuckets {
    /// Its generated `writeToParcel`: which fields are present, then each.
    fn read_from(r: &mut aim_binder_host::parcel::Reader<'_>) -> aim_binder_host::parcel::Result<SizeBuckets> {
        let flags = r.read_i32()?;
        let mut array = |bit: i32| -> aim_binder_host::parcel::Result<Option<Vec<i32>>> {
            if flags & bit == 0 {
                return Ok(None);
            }
            let n = r.read_i32()?;
            (0..n.max(0)).map(|_| r.read_i32()).collect::<aim_binder_host::parcel::Result<Vec<_>>>().map(Some)
        };
        Ok(SizeBuckets {
            horizontal: array(0x1)?,
            vertical: array(0x2)?,
            smallest: array(0x4)?,
            screen_layout_size: array(0x8)?,
            screen_layout_long_set: flags & 0x10 != 0,
        })
    }
}

impl SizeBuckets {
    /// `SizeConfigurationBuckets.filterDiff`: size changes that cross no
    /// threshold are no change.
    pub fn filter_diff(&self, mut diff: i32, old: &Configuration, new: &Configuration) -> i32 {
        let crosses = |thresholds: &Option<Vec<i32>>, a: i32, b: i32| {
            thresholds.iter().flatten().any(|&t| (a < t && b >= t) || (a >= t && b < t))
        };
        let non_size = SCREENLAYOUT_LAYOUTDIR_MASK | SCREENLAYOUT_ROUND_MASK | SCREENLAYOUT_COMPAT_NEEDED;
        let non_size_unchanged = old.screen_layout & non_size == new.screen_layout & non_size;
        if diff & CONFIG_SCREEN_SIZE != 0
            && !crosses(&self.horizontal, old.screen_width_dp, new.screen_width_dp)
            && !crosses(&self.vertical, old.screen_height_dp, new.screen_height_dp)
        {
            diff &= !CONFIG_SCREEN_SIZE;
        }
        if diff & CONFIG_SMALLEST_SCREEN_SIZE != 0 && !crosses(&self.smallest, old.smallest_screen_width_dp, new.smallest_screen_width_dp) {
            diff &= !CONFIG_SMALLEST_SCREEN_SIZE;
        }
        if diff & CONFIG_SCREEN_LAYOUT != 0 && non_size_unchanged {
            let (old_size, new_size) = (old.screen_layout & SCREENLAYOUT_SIZE_MASK, new.screen_layout & SCREENLAYOUT_SIZE_MASK);
            let size_crosses = old_size != new_size
                && (!new.is_layout_size_at_least(old_size)
                    || self.screen_layout_size.iter().flatten().any(|&s| old.is_layout_size_at_least(s) != new.is_layout_size_at_least(s)));
            let long_crosses = self.screen_layout_long_set && old.screen_layout & SCREENLAYOUT_LONG_MASK != new.screen_layout & SCREENLAYOUT_LONG_MASK;
            if !size_crosses && !long_crosses {
                diff &= !CONFIG_SCREEN_LAYOUT;
            }
        }
        diff
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
    /// scaleFactor=1, densityScaleFactor=1)` in processes whose
    /// `DisplayMetrics.DENSITY_DEVICE` is `device_density`
    /// (`ro.sf.lcd_density`), whatever the display's density: an app that
    /// needs no compatibility then has the default compatibility info,
    /// which `ViewRootImpl` applies to no configuration.
    pub fn of(app: &ApplicationInfo, config: &Configuration, device_density: i32) -> CompatibilityInfo {
        let screen_layout = config.screen_layout;
        let sw = config.smallest_screen_width_dp;
        let density = device_density;
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
                info.application_density_scale = info.application_scale;
                info.application_density_inverted_scale = info.application_inverted_scale;
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
        let display = Display::phone_for_test();
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
        let compat = CompatibilityInfo::of(&app, &c, 320);
        assert_eq!(
            (
                compat.flags,
                compat.application_density,
                compat.application_display_rotation
            ),
            (4, 320, -1)
        );
    }

    /// `DisplayMetrics.DENSITY_DEVICE`, not the display's density, is a
    /// modern app's density, so on a Mac display whose density differs
    /// from `ro.sf.lcd_density` its compatibility info stays the default
    /// (`CompatibilityInfo(ApplicationInfo, ...)`, android-16.0.0_r1).
    #[test]
    fn a_modern_app_keeps_the_device_density_on_another_display_density() {
        let mut c = Configuration::of(&Display::phone_for_test(), &Device {
            locales: "en-US",
            right_to_left: false,
            night: false,
            font_scale: 1.0,
            font_weight_adjustment: 0,
            seq: 1,
        });
        c.density_dpi = 144;
        let app = ApplicationInfo {
            target_sdk_version: 33,
            flags: FLAG_SUPPORTS_LARGE_SCREENS
                | FLAG_RESIZEABLE_FOR_SCREENS
                | FLAG_SUPPORTS_SCREEN_DENSITIES
                | FLAG_SUPPORTS_XLARGE_SCREENS,
            ..Default::default()
        };
        let compat = CompatibilityInfo::of(&app, &c, 320);
        assert_eq!((compat.flags, compat.application_density, compat.application_scale), (4, 320, 1.0));
    }

    #[test]
    fn a_freeform_task_takes_its_sizes_from_its_bounds() {
        let display = Display::phone_for_test();
        let device = Device { locales: "en-US", right_to_left: false, night: false, font_scale: 1.0, font_weight_adjustment: 0, seq: 1 };
        let global = Configuration::of(&display, &device);
        let bounds = Rect { left: 100, top: 200, right: 900, bottom: 1000 };
        let task = global.of_freeform_task(bounds);
        assert_eq!((task.window.windowing_mode, task.window.bounds, task.window.app_bounds), (5, bounds, Some(bounds)));
        assert_eq!(task.window.max_bounds, global.window.max_bounds);
        assert_eq!((task.screen_width_dp, task.screen_height_dp, task.smallest_screen_width_dp), (400, 400, 400));
        assert_eq!(task.orientation, ORIENTATION_PORTRAIT);
        // 400 dp square: small, not long, the direction kept.
        assert_eq!(task.screen_layout, SCREENLAYOUT_LAYOUTDIR_LTR | SCREENLAYOUT_LONG_NO | SCREENLAYOUT_SIZE_SMALL);
    }

    #[test]
    fn a_resize_changes_sizes_that_buckets_may_filter() {
        let display = Display::phone_for_test();
        let device = Device { locales: "en-US", right_to_left: false, night: false, font_scale: 1.0, font_weight_adjustment: 0, seq: 1 };
        let global = Configuration::of(&display, &device);
        let old = global.of_freeform_task(Rect { left: 0, top: 0, right: 800, bottom: 1400 });
        let moved = global.of_freeform_task(Rect { left: 50, top: 0, right: 850, bottom: 1400 });
        // A move changes only the window configuration.
        assert_eq!(old.diff(&moved), CONFIG_WINDOW_CONFIGURATION);
        let wider = global.of_freeform_task(Rect { left: 0, top: 0, right: 900, bottom: 1400 });
        let diff = old.diff(&wider);
        // 400 to 450 dp wide: small to normal too.
        assert_eq!(diff, CONFIG_WINDOW_CONFIGURATION | CONFIG_SCREEN_SIZE | CONFIG_SMALLEST_SCREEN_SIZE | CONFIG_SCREEN_LAYOUT);
        // It crosses no bucket of 600, but one of 420; a larger layout size
        // crosses no layout bucket of none.
        let buckets = |t: i32| SizeBuckets { horizontal: Some(vec![t]), smallest: Some(vec![t]), ..Default::default() };
        assert_eq!(buckets(600).filter_diff(diff, &old, &wider), CONFIG_WINDOW_CONFIGURATION);
        assert_eq!(buckets(420).filter_diff(diff, &old, &wider), diff & !CONFIG_SCREEN_LAYOUT);
    }
}
