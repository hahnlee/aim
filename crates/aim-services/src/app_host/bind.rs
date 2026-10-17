//! `IApplicationThread.bindApplication`'s arguments, as
//! `ActivityManagerService.attachApplicationLocked` makes them at
//! `android-16.0.0_r1`: the core settings (`CoreSettingsObserver`), the
//! application shared memory (`ApplicationSharedMemory`), and the
//! parcel forms the generated call leaves to its caller.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::ffi::CString;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};

use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;

use super::settings::{Settings, Table};
use crate::bundle::{self, Value};
use crate::package::info::{self, ProviderInfo};

/// The Secure settings `CoreSettingsObserver` sends, all ints.
const SECURE_INTS: &[&str] = &[
    "long_press_timeout",
    "multi_press_timeout",
    "key_repeat_timeout",
    "key_repeat_delay",
    "key_repeat_enabled",
    "stylus_pointer_icon_enabled",
];
/// Its System setting, a string.
const SYSTEM_STRINGS: &[&str] = &["time_12_24"];
/// Its Global settings: name, and whether an int (else a string).
const GLOBALS: &[(&str, bool)] = &[
    ("debug_view_attributes", true),
    ("debug_view_attributes_application_package", false),
    ("angle_debug_package", false),
    ("angle_gl_driver_all_angle", true),
    ("angle_gl_driver_selection_pkgs", false),
    ("angle_gl_driver_selection_values", false),
    ("angle_egl_features", false),
    ("show_angle_in_use_dialog_box", false),
    ("enable_gpu_debug_layers", true),
    ("gpu_debug_app", false),
    ("gpu_debug_layers", false),
    ("gpu_debug_layers_gles", false),
    ("gpu_debug_layer_app", false),
    ("updatable_driver_all_apps", true),
    ("updatable_driver_production_opt_in_apps", false),
    ("updatable_driver_prerelease_opt_in_apps", false),
    ("updatable_driver_production_opt_out_apps", false),
    ("updatable_driver_production_denylist", false),
    ("updatable_driver_production_allowlist", false),
    ("updatable_driver_production_denylists", false),
    ("updatable_driver_sphal_libraries", false),
];

/// A `DeviceConfig` flag of the `widget` namespace and its default.
enum Flag {
    Bool(bool),
    Int(i32),
    Float(f32),
}

/// `WidgetFlags`: the flag, its core setting's key, its default.
const WIDGET_FLAGS: &[(&str, &str, Flag)] = &[
    (
        "CursorControlFeature__enable_cursor_drag_from_anywhere",
        "widget__enable_cursor_drag_from_anywhere",
        Flag::Bool(true),
    ),
    (
        "CursorControlFeature__min_angle_from_vertical_to_start_cursor_drag",
        "widget__min_angle_from_vertical_to_start_cursor_drag",
        Flag::Int(45),
    ),
    (
        "CursorControlFeature__finger_to_cursor_distance",
        "widget__finger_to_cursor_distance",
        Flag::Int(-1),
    ),
    (
        "CursorControlFeature__enable_insertion_handle_gestures",
        "widget__enable_insertion_handle_gestures",
        Flag::Bool(false),
    ),
    (
        "CursorControlFeature__insertion_handle_delta_height",
        "widget__insertion_handle_delta_height",
        Flag::Int(25),
    ),
    (
        "CursorControlFeature__insertion_handle_opacity",
        "widget__insertion_handle_opacity",
        Flag::Int(50),
    ),
    (
        "CursorControlFeature__line_slop_ratio",
        "widget__line_slop_ratio",
        Flag::Float(0.5),
    ),
    (
        "CursorControlFeature__enable_new_magnifier",
        "widget__enable_new_magnifier",
        Flag::Bool(false),
    ),
    (
        "CursorControlFeature__magnifier_zoom_factor",
        "widget__magnifier_zoom_factor",
        Flag::Float(1.5),
    ),
    (
        "CursorControlFeature__magnifier_aspect_ratio",
        "widget__magnifier_aspect_ratio",
        Flag::Float(5.5),
    ),
];
/// The flag whose default is the framework's
/// `config_defaultAnalogClockSecondsHandFps` (`loadDeviceConfigContextEntries`).
const ANALOG_CLOCK_SECONDS_HAND_FPS: (&str, &str) = (
    "AnalogClockFeature__analog_clock_seconds_hand_fps",
    "widget__analog_clock_seconds_hand_fps",
);

/// `CoreSettingsObserver.getCoreSettingsLocked`, in the bundle's
/// (`ArrayMap`'s) order; `analog_clock_fps` is the framework's default.
pub fn core_settings(
    settings: &Settings,
    analog_clock_fps: i32,
) -> Result<Vec<(String, Value)>, String> {
    let mut entries = Vec::new();
    let int = |name: &str, value: &str| {
        value
            .parse::<i32>()
            .map_err(|_| format!("setting {name}: {value:?} is not an int"))
    };
    for name in SECURE_INTS {
        if let Some(value) = settings.get(Table::Secure, name) {
            entries.push((name.to_string(), Value::Int(int(name, value)?)));
        }
    }
    for name in SYSTEM_STRINGS {
        if let Some(value) = settings.get(Table::System, name) {
            entries.push((name.to_string(), Value::String(Some(value.to_string()))));
        }
    }
    for &(name, is_int) in GLOBALS {
        if let Some(value) = settings.get(Table::Global, name) {
            entries.push((
                name.to_string(),
                if is_int {
                    Value::Int(int(name, value)?)
                } else {
                    Value::String(Some(value.to_string()))
                },
            ));
        }
    }
    let flag = |name: &str| settings.device_config("widget", name);
    for (name, key, default) in WIDGET_FLAGS {
        // `DeviceConfig.get*`: the default where the flag is unset or
        // does not parse.
        entries.push((
            key.to_string(),
            match default {
                Flag::Bool(d) => Value::Int(i32::from(
                    flag(name).map_or(*d, |v| v.eq_ignore_ascii_case("true")),
                )),
                Flag::Int(d) => Value::Int(flag(name).and_then(|v| v.parse().ok()).unwrap_or(*d)),
                Flag::Float(d) => {
                    Value::Float(flag(name).and_then(|v| v.parse().ok()).unwrap_or(*d))
                }
            },
        ));
    }
    let (name, key) = ANALOG_CLOCK_SECONDS_HAND_FPS;
    entries.push((
        key.to_string(),
        Value::Int(
            flag(name)
                .and_then(|v| v.parse().ok())
                .unwrap_or(analog_clock_fps),
        ),
    ));
    Ok(info::array_order(entries, |(key, _)| key))
}

/// A bundle of `entries`.
pub struct Bundle(pub Vec<(String, Value)>);

impl WriteParcelable for Bundle {
    fn write_to(&self, p: &mut Parcel) {
        let entries: Vec<(&str, Value)> = self
            .0
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        bundle::write(p, &entries);
    }
}

/// `ProviderInfoList`: `writeTypedList` of the providers.
pub struct ProviderInfoList(pub Vec<ProviderInfo>);

impl WriteParcelable for ProviderInfoList {
    fn write_to(&self, p: &mut Parcel) {
        p.write_i32(self.0.len() as i32);
        for provider in &self.0 {
            p.write_i32(1);
            provider.write_to(p);
        }
    }
}

/// A parcelable the app host never sends (no instrumentation, profiler,
/// autofill or content capture service, font map).
pub enum Absent {}

impl WriteParcelable for Absent {
    fn write_to(&self, _: &mut Parcel) {
        match *self {}
    }
}

/// `writeRawFileDescriptor`.
pub struct RawFileDescriptor(pub aim_binder_driver::File);

impl WriteParcelable for RawFileDescriptor {
    fn write_to(&self, p: &mut Parcel) {
        p.write_file(self.0.clone());
    }
}

/// The `SharedMemory` of `com_android_internal_os_ApplicationSharedMemory
/// .cpp`: the network time (none), the system features cache (empty) and
/// the system server's nonce store (no nonces), mapped read-only by apps.
pub struct ApplicationSharedMemory {
    reader: OwnedFd,
}

/// `INVALID_NETWORK_TIME`.
const INVALID_NETWORK_TIME: i64 = -1;
/// `SystemFeaturesCache`: 512 `int32_t` and an `int64_t` length.
const FEATURES_CACHE: usize = 512 * 4 + 8;
/// `SystemCacheNonce`: the `NonceStore` header, 128 nonces, 8192 bytes.
const NONCE_STORE_HEADER: usize = 24;
const MAX_NONCE: usize = 128;
const MAX_BYTE: usize = 8192;
const SIZE: usize = 8 + FEATURES_CACHE + NONCE_STORE_HEADER + MAX_NONCE * 8 + MAX_BYTE;

impl ApplicationSharedMemory {
    pub fn new() -> std::io::Result<Self> {
        let mut bytes = vec![0u8; SIZE];
        bytes[..8].copy_from_slice(&INVALID_NETWORK_TIME.to_le_bytes());
        let store = 8 + FEATURES_CACHE;
        let header = [
            MAX_NONCE as i32,
            MAX_BYTE as i32,
            NONCE_STORE_HEADER as i32,
            (NONCE_STORE_HEADER + MAX_NONCE * 8) as i32,
        ];
        for (i, v) in header.iter().enumerate() {
            bytes[store + 4 * i..store + 4 * i + 4].copy_from_slice(&v.to_le_bytes());
        }
        Ok(ApplicationSharedMemory {
            reader: read_only_memory(&bytes)?,
        })
    }

    pub fn file(&self) -> std::io::Result<aim_binder_driver::File> {
        aim_binder_host::server::file_from_fd(self.reader.as_fd())
            .ok_or_else(|| std::io::Error::other("application shared memory fileport failed"))
    }
}

/// A shared memory object holding `bytes`, opened read-only: what a
/// `SharedMemory` set to `PROT_READ` gives the processes it is passed to.
pub(super) fn read_only_memory(bytes: &[u8]) -> std::io::Result<OwnedFd> {
    let name = CString::new(format!("/aim-asm-{:x}-{:x}", std::process::id(), unsafe {
        libc::arc4random()
    }))
    .unwrap();
    // SAFETY: a fresh name; the descriptors are owned below.
    let fd = unsafe {
        libc::shm_open(
            name.as_ptr(),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL,
            0o600,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let writer = unsafe { OwnedFd::from_raw_fd(fd) };
    let result = (|| {
        // Darwin's shared memory objects are sized once and only mapped,
        // never read or written; an empty one is a page.
        let size = bytes.len().max(1);
        if unsafe { libc::ftruncate(writer.as_raw_fd(), size as libc::off_t) } != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                writer.as_raw_fd(),
                0,
            )
        };
        if base == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), base.cast(), bytes.len());
            libc::munmap(base, size);
        }
        let fd = unsafe { libc::shm_open(name.as_ptr(), libc::O_RDONLY, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    })();
    unsafe { libc::shm_unlink(name.as_ptr()) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The original's core settings on a device whose settings set only
    /// the two press timeouts (system_server's `bindApplication` to
    /// Calculator, captured on the pinned image).
    #[test]
    fn core_settings_in_the_original_order() {
        let mut settings = Settings::default();
        let xml = br#"<settings><setting id="1" name="long_press_timeout" value="400" /><setting id="2" name="multi_press_timeout" value="300" /></settings>"#;
        settings.read_for_test(Table::Secure, xml);
        let keys: Vec<String> = core_settings(&settings, 1)
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(
            keys,
            [
                "widget__min_angle_from_vertical_to_start_cursor_drag",
                "widget__insertion_handle_opacity",
                "long_press_timeout",
                "widget__magnifier_zoom_factor",
                "widget__analog_clock_seconds_hand_fps",
                "widget__enable_insertion_handle_gestures",
                "widget__insertion_handle_delta_height",
                "widget__line_slop_ratio",
                "widget__enable_cursor_drag_from_anywhere",
                "multi_press_timeout",
                "widget__enable_new_magnifier",
                "widget__finger_to_cursor_distance",
                "widget__magnifier_aspect_ratio",
            ]
        );
    }

    #[test]
    fn shared_memory_has_the_native_layout() {
        assert_eq!(SIZE, 11304);
        let memory = ApplicationSharedMemory::new().unwrap();
        let base = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                SIZE,
                libc::PROT_READ,
                libc::MAP_SHARED,
                memory.reader.as_raw_fd(),
                0,
            )
        };
        assert_ne!(base, libc::MAP_FAILED);
        let bytes = unsafe { std::slice::from_raw_parts(base.cast::<u8>(), SIZE) }.to_vec();
        unsafe { libc::munmap(base, SIZE) };
        assert_eq!(i64::from_le_bytes(bytes[..8].try_into().unwrap()), -1);
        assert_eq!(
            i32::from_le_bytes(bytes[2064..2068].try_into().unwrap()),
            128
        );
        assert_eq!(
            i32::from_le_bytes(bytes[2072..2076].try_into().unwrap()),
            24
        );
    }
}
