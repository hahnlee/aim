//! The Mac's time zone, language and keyboard layout, which the device
//! follows (docs/mac-settings.md). guest-init reads them before init's
//! first action and publishes them as `vendor.aim.mac.*` properties, and
//! republishes the time zone and keyboard layout when the Mac changes
//! them. The appearance is the native uimode service's own.
//! `init.aim.rc` applies the values through Android's own services, as a
//! device's vendor init would. Nothing here changes the Mac's settings.

use std::ffi::{CStr, c_char, c_void};
use std::sync::mpsc::Sender;

/// An IANA zone name in the image's tzdata.
pub const TIME_ZONE_PROP: &str = "vendor.aim.mac.time_zone";
/// The first of the Mac's languages as the device names it
/// (`aim_services::locale`): the language Android starts in, before the
/// service host applies the whole list.
pub const LOCALE_PROP: &str = "vendor.aim.mac.locale";
/// The built-in keyboard's layout: an InputDevices layout's resource name
/// (`aim-keyboard layout`).
pub const KEYBOARD_LAYOUT_PROP: &str = "vendor.aim.mac.keyboard_layout";

/// The Mac keyboard layouts (`com.apple.keylayout.*`) whose characters an
/// InputDevices layout has, key for key on the unmodified and Shift levels.
const KEYBOARD_LAYOUTS: &[(&str, &str)] = &[
    ("Colemak", "keyboard_layout_english_us_colemak"),
    ("DVORAK-QWERTYCMD", "keyboard_layout_english_us_dvorak"),
    ("Dvorak", "keyboard_layout_english_us_dvorak"),
    ("British-PC", "keyboard_layout_english_uk"),
    ("French-PC", "keyboard_layout_french"),
    ("German", "keyboard_layout_german"),
    ("Russian", "keyboard_layout_russian_mac"),
    ("RussianWin", "keyboard_layout_russian"),
    ("USInternational-PC", "keyboard_layout_english_us_intl"),
];

/// The InputDevices layout for the Mac's keyboard layout `id`
/// (`AppleCurrentKeyboardLayoutInputSourceID`). US English for the others:
/// `ABC` and `US` are, and so is the Latin level of the input methods'
/// layouts (`2SetHangul`), whose own characters are an input method's.
pub fn keyboard_layout(id: &str) -> &'static str {
    let name = id.strip_prefix("com.apple.keylayout.").unwrap_or_default();
    KEYBOARD_LAYOUTS
        .iter()
        .find(|(mac, _)| *mac == name)
        .map_or("keyboard_layout_english_us", |(_, android)| android)
}

/// The guest's tzdata: bionic and libcore resolve zone names in it.
pub const TZDATA: &str = "/apex/com.android.tzdata/etc/tz/tzdata";

/// The IANA name `/etc/localtime` links to
/// (`/var/db/timezone/zoneinfo/Asia/Seoul`).
pub fn zone_from_localtime(target: &str) -> Option<&str> {
    let (_, name) = target.split_once("/zoneinfo/")?;
    let valid = !name.is_empty()
        && name.split('/').all(|part| {
            !part.is_empty()
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_+-.".contains(&b))
        });
    valid.then_some(name)
}

/// Whether bionic's tzdata file (`ZoneInfoDb`: a 12-byte version, the
/// index, data and final offsets, then 52-byte entries of a 40-byte name
/// and three integers) has a zone called `name`.
pub fn tzdata_has(tzdata: &[u8], name: &str) -> bool {
    let be = |at: usize| -> Option<usize> {
        Some(u32::from_be_bytes(tzdata.get(at..at + 4)?.try_into().ok()?) as usize)
    };
    if !tzdata.starts_with(b"tzdata") || name.len() >= 40 {
        return false;
    }
    let (Some(index), Some(data)) = (be(12), be(16)) else {
        return false;
    };
    let Some(entries) = tzdata.get(index..data.min(tzdata.len())) else {
        return false;
    };
    entries.chunks_exact(52).any(|entry| {
        let stored = &entry[..40];
        let end = stored.iter().position(|&b| b == 0).unwrap_or(40);
        &stored[..end] == name.as_bytes()
    })
}

/// The Mac's current values of the `vendor.aim.mac.*` properties. The time
/// zone is left out when the guest's tzdata (`tzdata`) lacks it.
pub fn properties(tzdata: &[u8]) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if let Ok(target) = std::fs::read_link("/etc/localtime")
        && let Some(zone) = zone_from_localtime(&target.to_string_lossy())
        && tzdata_has(tzdata, zone)
    {
        out.push((TIME_ZONE_PROP, zone.to_string()));
    }
    // SAFETY: CoreFoundation calls on values this function owns.
    unsafe {
        CFPreferencesAppSynchronize(*kCFPreferencesAnyApplication());
        if let Some(locale) = aim_services::locale::preferred_locales().into_iter().next() {
            out.push((LOCALE_PROP, locale));
        }
        let toolbox = cfstring(c"com.apple.HIToolbox");
        CFPreferencesAppSynchronize(toolbox);
        let layout = app_preference("AppleCurrentKeyboardLayoutInputSourceID", toolbox)
            .and_then(|v| v.string());
        CFRelease(toolbox);
        out.push((
            KEYBOARD_LAYOUT_PROP,
            keyboard_layout(layout.as_deref().unwrap_or_default()).to_string(),
        ));
    }
    out
}

/// Sends the name and value of each property whose value changes
/// from `initial`, checking every [`POLL`]: a `readlink` and a preferences
/// read, with no run loop for the Mac's change notifications in
/// guest-init.
pub fn watch(
    events: Sender<(String, String)>,
    tzdata: Vec<u8>,
    initial: Vec<(&'static str, String)>,
) {
    std::thread::Builder::new()
        .name("mac-settings".into())
        .spawn(move || {
            let mut last = initial;
            loop {
                std::thread::sleep(POLL);
                let current = properties(&tzdata);
                for (name, value) in &current {
                    // The language is Android's at its start
                    // (`persist.sys.locale`); the service host follows
                    // the Mac's changes (`aim_services::locale`).
                    if *name != LOCALE_PROP
                        && !last.iter().any(|(n, v)| n == name && v == value)
                        && events.send((name.to_string(), value.clone())).is_err()
                    {
                        return;
                    }
                }
                last = current;
            }
        })
        .expect("mac-settings thread");
}

/// How often [`watch`] reads the Mac's settings.
pub const POLL: std::time::Duration = std::time::Duration::from_secs(2);

type CFTypeRef = *const c_void;

const UTF8: u32 = 0x0800_0100;

aim_hostcall::dylib! {
    static CORE_FOUNDATION = c"/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation" {
        static kCFPreferencesAnyApplication: CFTypeRef;
        fn CFPreferencesAppSynchronize(app: CFTypeRef) -> bool;
        fn CFPreferencesCopyAppValue(key: CFTypeRef, app: CFTypeRef) -> CFTypeRef;
        fn CFStringCreateWithCString(alloc: CFTypeRef, s: *const c_char, encoding: u32) -> CFTypeRef;
        fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, size: isize, encoding: u32) -> bool;
        fn CFGetTypeID(cf: CFTypeRef) -> usize;
        fn CFStringGetTypeID() -> usize;
        fn CFRelease(cf: CFTypeRef);
    }
}

unsafe fn cfstring(s: &CStr) -> CFTypeRef {
    // SAFETY: a NUL-terminated UTF-8 string.
    unsafe { CFStringCreateWithCString(std::ptr::null(), s.as_ptr(), UTF8) }
}

/// An owned CoreFoundation value.
struct Cf(CFTypeRef);

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: a +1 reference this value owns.
        unsafe { CFRelease(self.0) }
    }
}

impl Cf {
    fn string(&self) -> Option<String> {
        // SAFETY: a valid CF value.
        unsafe { to_string(self.0) }
    }
}

/// Application `app`'s value of `key`, as `defaults read APP KEY` shows
/// it.
unsafe fn app_preference(key: &str, app: CFTypeRef) -> Option<Cf> {
    let key = std::ffi::CString::new(key).ok()?;
    // SAFETY: CF calls on a string this function creates and releases.
    unsafe {
        let key = cfstring(&key);
        let value = CFPreferencesCopyAppValue(key, app);
        CFRelease(key);
        (!value.is_null()).then(|| Cf(value))
    }
}

unsafe fn to_string(value: CFTypeRef) -> Option<String> {
    // SAFETY: a valid CF value, checked to be a string.
    unsafe {
        if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
            return None;
        }
        let mut buf = [0 as c_char; 256];
        CFStringGetCString(value, buf.as_mut_ptr(), buf.len() as isize, UTF8)
            .then(|| CStr::from_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_is_the_path_below_zoneinfo() {
        let zone = |t| zone_from_localtime(t);
        assert_eq!(
            zone("/var/db/timezone/zoneinfo/Asia/Seoul"),
            Some("Asia/Seoul")
        );
        assert_eq!(
            zone("/usr/share/zoneinfo/America/Argentina/Buenos_Aires"),
            Some("America/Argentina/Buenos_Aires")
        );
        assert_eq!(
            zone("/var/db/timezone/zoneinfo/Etc/GMT+9"),
            Some("Etc/GMT+9")
        );
        assert_eq!(zone("/var/db/timezone/zoneinfo/UTC"), Some("UTC"));
        assert_eq!(zone("/var/db/timezone/zoneinfo/"), None);
        assert_eq!(zone("/var/db/timezone/zoneinfo/../../etc"), None);
        assert_eq!(zone("/etc/foo"), None);
    }

    fn tzdata(names: &[&str]) -> Vec<u8> {
        let mut out = b"tzdata2025a\0".to_vec();
        let index = 24u32;
        let data = index + 52 * names.len() as u32;
        for offset in [index, data, data] {
            out.extend(offset.to_be_bytes());
        }
        for name in names {
            let mut entry = [0u8; 52];
            entry[..name.len()].copy_from_slice(name.as_bytes());
            out.extend(entry);
        }
        out
    }

    #[test]
    fn tzdata_index_lookup() {
        let data = tzdata(&[
            "Africa/Abidjan",
            "Asia/Seoul",
            "America/Argentina/ComodRivadavia",
        ]);
        assert!(tzdata_has(&data, "Asia/Seoul"));
        assert!(tzdata_has(&data, "America/Argentina/ComodRivadavia"));
        assert!(!tzdata_has(&data, "Asia/Seo"));
        assert!(!tzdata_has(&data, "Mars/Olympus"));
        assert!(!tzdata_has(b"nottzdata", "Asia/Seoul"));
        assert!(!tzdata_has(&data[..30], "Asia/Seoul"));
    }

    #[test]
    fn mac_keyboard_layouts_as_android_layouts() {
        let l = keyboard_layout;
        assert_eq!(
            l("com.apple.keylayout.Dvorak"),
            "keyboard_layout_english_us_dvorak"
        );
        assert_eq!(
            l("com.apple.keylayout.RussianWin"),
            "keyboard_layout_russian"
        );
        assert_eq!(l("com.apple.keylayout.ABC"), "keyboard_layout_english_us");
        assert_eq!(
            l("com.apple.keylayout.2SetHangul"),
            "keyboard_layout_english_us"
        );
        assert_eq!(l(""), "keyboard_layout_english_us");
    }

    #[test]
    fn reads_the_macs_settings() {
        // No tzdata: the zone is left out. The layout is always named.
        let props = properties(&[]);
        assert!(!props.iter().any(|(n, _)| *n == TIME_ZONE_PROP));
        let layout = props
            .iter()
            .find(|(n, _)| *n == KEYBOARD_LAYOUT_PROP)
            .unwrap();
        assert!(layout.1.starts_with("keyboard_layout_"), "{}", layout.1);
    }
}
